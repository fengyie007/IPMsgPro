//! Bounded FeiQ image reassembly. This module performs no I/O: callers send ACKs
//! returned by `accept`/`finish` and finalize payloads on a bounded worker queue.

use super::lzw::crc32;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const FRAGMENT_BYTES: usize = 512;
const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESERVED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ACTIVE: usize = 4;
const MAX_RECORDS: usize = 1024;
const MAX_PEER_ID_BYTES: usize = 4096;
const ACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
const FINALIZING_TIMEOUT: Duration = Duration::from_secs(25);
const TERMINAL_RETENTION: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct TransferKey {
    pub peer_id: String,
    pub image_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fragment {
    pub image_id: String,
    pub total: usize,
    pub count: u32,
    pub index: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Action {
    Ack {
        key: TransferKey,
        index: u32,
    },
    Finalize {
        key: TransferKey,
        index: u32,
        payload: Vec<u8>,
    },
    /// A finalization is already running, or this transfer was rejected. No ACK.
    Pending,
}

fn valid_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decimal(text: &str) -> Result<usize, String> {
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("图片分片数字字段无效".into());
    }
    text.parse::<usize>().map_err(|_| "图片分片数字溢出".into())
}

fn validate_geometry(total: usize, count: u32, index: u32, length: usize) -> Result<usize, String> {
    if total == 0 || total > MAX_PAYLOAD_BYTES {
        return Err("图片传输载荷为空或超过16MiB".into());
    }
    let expected_count = (total - 1) / FRAGMENT_BYTES + 1;
    if count as usize != expected_count || index == 0 || index > count {
        return Err("图片分片总数或片号无效".into());
    }
    // total/count were bounded above, so both arithmetic operations are safe.
    let offset = (index as usize - 1) * FRAGMENT_BYTES;
    if length != FRAGMENT_BYTES.min(total - offset) {
        return Err("图片分片长度与位置不匹配".into());
    }
    Ok(offset)
}

/// Parse only the bounded textual header. Every byte following #\0 is data,
/// including any later NUL, colon, # or non-UTF-8 byte.
pub fn parse_fragment(body: &[u8]) -> Result<Fragment, String> {
    let hash = body
        .iter()
        .position(|b| *b == b'#')
        .ok_or("图片分片缺少#分隔符")?;
    if hash == 0 || hash > 256 || body.get(hash + 1) != Some(&0) {
        return Err("图片分片头或二进制分隔符无效".into());
    }
    let header = std::str::from_utf8(&body[..hash]).map_err(|_| "图片分片头不是ASCII文本")?;
    if !header.is_ascii() {
        return Err("图片分片头不是ASCII文本".into());
    }
    let fields: Vec<_> = header.split('|').collect();
    if fields.len() != 10 || !valid_id(fields[0]) {
        return Err("图片ID或分片头字段数量无效".into());
    }
    let total = decimal(fields[1])?;
    let offset = decimal(fields[2])?;
    let count = u32::try_from(decimal(fields[3])?).map_err(|_| "图片片数溢出")?;
    let index = u32::try_from(decimal(fields[4])?).map_err(|_| "图片片号溢出")?;
    let length = decimal(fields[5])?;
    for value in &fields[6..9] {
        u32::try_from(decimal(value)?).map_err(|_| "图片保留字段溢出")?;
    }
    // Preserve reserved values without inventing their meaning. The observed
    // final field is eight hexadecimal digits, commonly 00000000.
    if !valid_id(fields[9]) {
        return Err("图片尾部保留字段无效".into());
    }
    if validate_geometry(total, count, index, length)? != offset {
        return Err("图片分片偏移与片号不匹配".into());
    }
    let data = &body[hash + 2..];
    if data.len() != length {
        return Err("图片分片实际数据长度不匹配".into());
    }
    Ok(Fragment {
        image_id: fields[0].into(),
        total,
        count,
        index,
        data: data.to_vec(),
    })
}

/// Recognize a single complete image reference, optionally with FeiQ's verified
/// font suffix. Text surrounding a reference is not silently discarded.
pub fn parse_reference(body: &str) -> Option<String> {
    if body.len() > 1024 {
        return None;
    }
    let plain = crate::text::strip_feiq_font_suffix(body);
    let id = plain.strip_prefix("/~#>")?.strip_suffix("<B~")?;
    valid_id(id).then(|| id.to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Geometry {
    total: usize,
    count: u32,
}

#[derive(Debug)]
enum State {
    ReferenceOnly,
    Collecting {
        payload: Vec<u8>,
        present: Vec<bool>,
        fingerprints: Vec<u32>,
        received: usize,
    },
    Finalizing {
        fingerprints: Vec<u32>,
        held_index: u32,
    },
    Completed {
        fingerprints: Vec<u32>,
    },
    Rejected,
}
impl State {
    fn active(&self) -> bool {
        matches!(
            self,
            Self::ReferenceOnly | Self::Collecting { .. } | Self::Finalizing { .. }
        )
    }
    fn timeout_message(&self) -> &'static str {
        match self {
            Self::ReferenceOnly => "图片引用等待数据超时",
            Self::Collecting { .. } => "图片分片接收超时",
            Self::Finalizing { .. } => "图片解码或保存超时",
            _ => "图片传输已结束",
        }
    }
}

#[derive(Debug)]
struct Transfer {
    state: State,
    geometry: Option<Geometry>,
    deadline: Instant,
    reserved: usize,
}

#[derive(Debug, Default)]
pub struct Assembler {
    transfers: HashMap<TransferKey, Transfer>,
    /// Reservations include payloads handed to the caller until finish/expiry.
    reserved_bytes: usize,
    /// One pending failure per rejected transfer. Pending reports pin terminal
    /// records, bounding this queue by MAX_RECORDS without losing notifications.
    pending_failures: HashMap<TransferKey, String>,
}

impl Assembler {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(peer_id: &str, image_id: &str) -> Result<TransferKey, String> {
        if peer_id.is_empty() || peer_id.len() > MAX_PEER_ID_BYTES || !valid_id(image_id) {
            return Err("图片发送方或ID无效".into());
        }
        Ok(TransferKey {
            peer_id: peer_id.into(),
            image_id: image_id.into(),
        })
    }

    fn prune_terminal(&mut self, now: Instant) {
        let pending = &self.pending_failures;
        self.transfers.retain(|key, transfer| {
            transfer.state.active() || transfer.deadline > now || pending.contains_key(key)
        });
    }

    fn can_admit(&self) -> Result<(), String> {
        if self.transfers.len() >= MAX_RECORDS {
            return Err("图片接收记录已达上限，请稍后重试".into());
        }
        if self.transfers.values().filter(|t| t.state.active()).count() >= MAX_ACTIVE {
            return Err("同时接收的图片过多".into());
        }
        Ok(())
    }

    fn reject(&mut self, key: &TransferKey, transfer: &mut Transfer, now: Instant, reason: &str) {
        if !transfer.state.active() {
            return;
        }
        self.reserved_bytes -= transfer.reserved;
        transfer.reserved = 0;
        transfer.state = State::Rejected;
        transfer.deadline = now + TERMINAL_RETENTION;
        // Reports cannot outgrow the transfer table: prune_terminal retains
        // every record whose failure has not yet been drained by expire().
        debug_assert!(
            self.pending_failures.len() < MAX_RECORDS || self.pending_failures.contains_key(key)
        );
        self.pending_failures
            .entry(key.clone())
            .or_insert_with(|| reason.to_owned());
    }

    pub fn reference(&mut self, peer_id: &str, image_id: &str, now: Instant) -> Result<(), String> {
        let key = Self::key(peer_id, image_id)?;
        self.prune_terminal(now);
        if let Some(mut transfer) = self.transfers.remove(&key) {
            let result = if transfer.state.active() && transfer.deadline <= now {
                let reason = transfer.state.timeout_message().to_owned();
                self.reject(&key, &mut transfer, now, &reason);
                Err(reason)
            } else if matches!(transfer.state, State::Rejected) {
                Err("该图片已拒收".into())
            } else {
                // Neither duplicate references nor references after completion
                // extend deadlines or create another image/history record.
                Ok(())
            };
            self.transfers.insert(key, transfer);
            return result;
        }
        self.can_admit()?;
        self.transfers.insert(
            key,
            Transfer {
                state: State::ReferenceOnly,
                geometry: None,
                deadline: now + ACTIVITY_TIMEOUT,
                reserved: 0,
            },
        );
        Ok(())
    }

    pub fn accept(
        &mut self,
        peer_id: &str,
        fragment: Fragment,
        now: Instant,
    ) -> Result<Action, String> {
        let key = Self::key(peer_id, &fragment.image_id)?;
        validate_geometry(
            fragment.total,
            fragment.count,
            fragment.index,
            fragment.data.len(),
        )?;
        self.prune_terminal(now);
        let mut transfer = match self.transfers.remove(&key) {
            Some(transfer) => transfer,
            None => {
                self.can_admit()?;
                // Admission reserves the full declared payload before allocating
                // its buffer/presence table; a sparse stream cannot exceed budget.
                if fragment.total > MAX_RESERVED_BYTES - self.reserved_bytes {
                    return Err("图片接收缓存额度不足".into());
                }
                Transfer {
                    state: State::ReferenceOnly,
                    geometry: None,
                    deadline: now + ACTIVITY_TIMEOUT,
                    reserved: 0,
                }
            }
        };
        let result = self.accept_existing(&key, &mut transfer, fragment, now);
        self.transfers.insert(key, transfer);
        result
    }

    fn accept_existing(
        &mut self,
        key: &TransferKey,
        transfer: &mut Transfer,
        fragment: Fragment,
        now: Instant,
    ) -> Result<Action, String> {
        if matches!(transfer.state, State::Rejected) {
            return Ok(Action::Pending);
        }
        if transfer.state.active() && transfer.deadline <= now {
            let reason = transfer.state.timeout_message().to_owned();
            self.reject(key, transfer, now, &reason);
            return Err(reason);
        }
        let geometry = Geometry {
            total: fragment.total,
            count: fragment.count,
        };
        if let Some(previous) = transfer.geometry {
            if previous != geometry {
                if !matches!(transfer.state, State::Completed { .. }) {
                    self.reject(key, transfer, now, "同一图片的总长度或分片总数发生变化");
                }
                return Err("同一图片的总长度或分片总数发生变化".into());
            }
        } else {
            if fragment.total > MAX_RESERVED_BYTES - self.reserved_bytes {
                self.reject(key, transfer, now, "图片接收缓存额度不足");
                return Err("图片接收缓存额度不足".into());
            }
            self.reserved_bytes += fragment.total;
            transfer.reserved = fragment.total;
            transfer.geometry = Some(geometry);
            transfer.deadline = now + ACTIVITY_TIMEOUT;
            transfer.state = State::Collecting {
                payload: vec![0; fragment.total],
                present: vec![false; fragment.count as usize],
                fingerprints: vec![0; fragment.count as usize],
                received: 0,
            };
        }

        let index = fragment.index;
        let slot = index as usize - 1;
        match &mut transfer.state {
            State::Collecting {
                payload,
                present,
                fingerprints,
                received,
            } => {
                let offset = slot * FRAGMENT_BYTES;
                let end = offset + fragment.data.len();
                if present[slot] {
                    if &payload[offset..end] != fragment.data.as_slice() {
                        self.reject(key, transfer, now, "同片号重复数据冲突，图片已拒收");
                        return Err("同片号重复数据冲突，图片已拒收".into());
                    }
                    return Ok(Action::Ack {
                        key: key.clone(),
                        index,
                    });
                }
                payload[offset..end].copy_from_slice(&fragment.data);
                fingerprints[slot] = crc32(&fragment.data);
                present[slot] = true;
                *received += 1;
                if *received < geometry.count as usize {
                    return Ok(Action::Ack {
                        key: key.clone(),
                        index,
                    });
                }
                // Transfer the already-contiguous buffer without allocating a
                // second full payload at the point of maximum memory pressure.
                let payload = std::mem::take(payload);
                let fingerprints = std::mem::take(fingerprints);
                transfer.state = State::Finalizing {
                    fingerprints,
                    held_index: index,
                };
                transfer.deadline = now + FINALIZING_TIMEOUT;
                Ok(Action::Finalize {
                    key: key.clone(),
                    index,
                    payload,
                })
            }
            State::Finalizing {
                fingerprints,
                held_index,
            } => {
                if fingerprints[slot] != crc32(&fragment.data) {
                    self.reject(key, transfer, now, "完成处理期间出现冲突分片，图片已拒收");
                    return Err("完成处理期间出现冲突分片，图片已拒收".into());
                }
                if index == *held_index {
                    Ok(Action::Pending)
                } else {
                    Ok(Action::Ack {
                        key: key.clone(),
                        index,
                    })
                }
            }
            State::Completed { fingerprints } => {
                if fingerprints[slot] != crc32(&fragment.data) {
                    return Err("已完成图片的重发分片内容不一致".into());
                }
                Ok(Action::Ack {
                    key: key.clone(),
                    index,
                })
            }
            State::Rejected => Ok(Action::Pending),
            State::ReferenceOnly => unreachable!("a validated fragment initializes collection"),
        }
    }

    pub fn is_finalizing(&self, key: &TransferKey, now: Instant) -> bool {
        self.transfers.get(key).is_some_and(|transfer| {
            matches!(transfer.state, State::Finalizing { .. }) && now < transfer.deadline
        })
    }

    /// Release the completing fragment only after the caller has decoded, saved
    /// and committed the image. Late/repeated callbacks never revive a rejection.
    pub fn finish(&mut self, key: &TransferKey, success: bool, now: Instant) -> Option<u32> {
        let mut transfer = self.transfers.remove(key)?;
        let result = if matches!(transfer.state, State::Finalizing { .. }) {
            if success && now < transfer.deadline {
                let state = std::mem::replace(&mut transfer.state, State::Rejected);
                let State::Finalizing {
                    fingerprints,
                    held_index,
                } = state
                else {
                    unreachable!()
                };
                self.reserved_bytes -= transfer.reserved;
                transfer.reserved = 0;
                transfer.state = State::Completed { fingerprints };
                transfer.deadline = now + TERMINAL_RETENTION;
                Some(held_index)
            } else {
                let reason = if now >= transfer.deadline {
                    "图片解码或保存超时"
                } else {
                    "图片解码或保存失败"
                };
                self.reject(key, &mut transfer, now, reason);
                None
            }
        } else {
            None
        };
        self.transfers.insert(key.clone(), transfer);
        result
    }

    /// Must be called regularly by the host. Drains every rejection/timeout once,
    /// including failures detected earlier by accept(), reference() or finish().
    /// The caller may replace finish(false)'s generic reason with its codec error.
    pub fn expire(&mut self, now: Instant) -> Vec<(TransferKey, String)> {
        self.prune_terminal(now);
        let expired: Vec<_> = self
            .transfers
            .iter()
            .filter(|(_, transfer)| transfer.state.active() && transfer.deadline <= now)
            .map(|(key, transfer)| (key.clone(), transfer.state.timeout_message()))
            .collect();
        for (key, reason) in expired {
            if let Some(mut transfer) = self.transfers.remove(&key) {
                self.reject(&key, &mut transfer, now, reason);
                self.transfers.insert(key, transfer);
            }
        }
        let failures = self.pending_failures.drain().collect();
        // Expired terminal records were pinned only until their report was drained.
        self.prune_terminal(now);
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(id: &str, bytes: &[u8], index: u32) -> Fragment {
        let offset = (index as usize - 1) * FRAGMENT_BYTES;
        Fragment {
            image_id: id.into(),
            total: bytes.len(),
            count: ((bytes.len() - 1) / FRAGMENT_BYTES + 1) as u32,
            index,
            data: bytes[offset..(offset + FRAGMENT_BYTES).min(bytes.len())].to_vec(),
        }
    }
    fn key(peer: &str, id: &str) -> TransferKey {
        TransferKey {
            peer_id: peer.into(),
            image_id: id.into(),
        }
    }
    fn finalize(action: Action) -> (TransferKey, u32, Vec<u8>) {
        match action {
            Action::Finalize {
                key,
                index,
                payload,
            } => (key, index, payload),
            _ => panic!("expected finalization"),
        }
    }

    #[test]
    fn parser_preserves_binary_and_wire_id_case() {
        let mut packet = b"Ab12CD34|5|0|1|1|5|0|1|0|00000000#\0".to_vec();
        packet.extend_from_slice(&[0, b'#', b':', 0xff, 0]);
        let parsed = parse_fragment(&packet).unwrap();
        assert_eq!(parsed.image_id, "Ab12CD34");
        assert_eq!(parsed.data, [0, b'#', b':', 0xff, 0]);
    }

    #[test]
    fn parser_rejects_bad_geometry_and_framing() {
        for body in [
            b"abc|1|0|1|1|1|0|1|0|00000000#\0x".as_slice(),
            b"abcdef01|0|0|0|1|1|0|1|0|00000000#\0x",
            b"abcdef01|1|1|1|1|1|0|1|0|00000000#\0x",
            b"abcdef01|1|0|1|0|1|0|1|0|00000000#\0x",
            b"abcdef01|1|0|2|1|1|0|1|0|00000000#\0x",
            b"abcdef01|1|0|1|2|1|0|1|0|00000000#\0x",
            b"abcdef01|1|0|1|1|2|0|1|0|00000000#\0xy",
            b"abcdef01|1|0|1|1|1|0|1|0|00000000#x",
            b"abcdef01|1|0|1|1|1|0|1|0|00000000#\0xy",
            b"abcdef01|16777217|0|32769|1|512|0|1|0|00000000#\0x",
            b"abcdef01|99999999999999999999|0|1|1|1|0|1|0|00000000#\0x",
            b"abcdef01|-1|0|1|1|1|0|1|0|00000000#\0x",
        ] {
            assert!(parse_fragment(body).is_err(), "accepted {:?}", body);
        }
        let mut huge = vec![b'x'; 257];
        huge.extend_from_slice(b"#\0x");
        assert!(parse_fragment(&huge).is_err());
    }

    #[test]
    fn exact_full_and_short_last_fragment_geometry() {
        let mut full = b"abcdef01|512|0|1|1|512|0|2|0|00000000#\0".to_vec();
        full.extend_from_slice(&[7; 512]);
        assert_eq!(parse_fragment(&full).unwrap().data.len(), 512);
        let last = b"abcdef01|513|512|2|2|1|0|1|0|00000000#\0x";
        assert_eq!(parse_fragment(last).unwrap().index, 2);
    }

    #[test]
    fn reference_is_exact_and_may_have_a_font_suffix() {
        assert_eq!(parse_reference("/~#>Ab12CD34<B~"), Some("Ab12CD34".into()));
        assert_eq!(
            parse_reference(
                "/~#>abcdef01<B~{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}"
            ),
            Some("abcdef01".into())
        );
        for text in [
            "prefix/~#>abcdef01<B~",
            "/~#>abcdef01<B~suffix",
            "/~#>abc<B~",
            "/~#>abcdefgh<B~",
            "/~#>abcdef01<B~ ",
            "/~#>abcdef01<B~{/font;broken;}",
        ] {
            assert_eq!(parse_reference(text), None);
        }
    }

    #[test]
    fn out_of_order_duplicates_and_completion_ack() {
        let now = Instant::now();
        let mut assembler = Assembler::new();
        let bytes: Vec<u8> = (0..1100).map(|n| (n % 251) as u8).collect();
        let k = key("peer", "abcdef01");
        assert_eq!(
            assembler
                .accept("peer", fragment(&k.image_id, &bytes, 3), now)
                .unwrap(),
            Action::Ack {
                key: k.clone(),
                index: 3
            }
        );
        assert!(matches!(
            assembler
                .accept("peer", fragment(&k.image_id, &bytes, 3), now)
                .unwrap(),
            Action::Ack { .. }
        ));
        assembler
            .accept("peer", fragment(&k.image_id, &bytes, 1), now)
            .unwrap();
        let (got_key, held, payload) = finalize(
            assembler
                .accept("peer", fragment(&k.image_id, &bytes, 2), now)
                .unwrap(),
        );
        assert_eq!(got_key, k);
        assert_eq!(held, 2);
        assert_eq!(payload, bytes);
        assert!(assembler.is_finalizing(&k, now));
        assert_eq!(
            assembler
                .accept("peer", fragment(&k.image_id, &bytes, 2), now)
                .unwrap(),
            Action::Pending
        );
        assert!(matches!(
            assembler
                .accept("peer", fragment(&k.image_id, &bytes, 1), now)
                .unwrap(),
            Action::Ack { index: 1, .. }
        ));
        assert_eq!(assembler.finish(&k, true, now), Some(2));
        assert_eq!(assembler.reserved_bytes, 0);
        assert_eq!(assembler.finish(&k, true, now), None);
        assert!(matches!(
            assembler
                .accept("peer", fragment(&k.image_id, &bytes, 2), now)
                .unwrap(),
            Action::Ack { index: 2, .. }
        ));
    }

    #[test]
    fn same_wire_id_from_different_peers_is_independent() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (one, _, p) = finalize(
            a.accept("peer-a", fragment("abcdef01", b"one", 1), now)
                .unwrap(),
        );
        let (two, _, q) = finalize(
            a.accept("peer-b", fragment("abcdef01", b"two", 1), now)
                .unwrap(),
        );
        assert_ne!(one, two);
        assert_eq!(p, b"one");
        assert_eq!(q, b"two");
        assert_eq!(a.finish(&one, true, now), Some(1));
        assert!(a.is_finalizing(&two, now));
    }

    #[test]
    fn decode_failure_cannot_be_reacknowledged_as_a_new_partial_image() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let bytes = vec![4; 513];
        a.accept("p", fragment("abcdef01", &bytes, 1), now).unwrap();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", &bytes, 2), now).unwrap());
        assert_eq!(a.finish(&k, false, now), None);
        for index in [2, 1, 2] {
            assert_eq!(
                a.accept("p", fragment("abcdef01", &bytes, index), now)
                    .unwrap(),
                Action::Pending
            );
        }
        assert_eq!(a.finish(&k, true, now), None);
        assert!(a.reference("p", "abcdef01", now).is_err());
        assert_eq!(a.reserved_bytes, 0);
    }

    #[test]
    fn conflicting_chunks_and_metadata_are_rejected() {
        let now = Instant::now();
        let bytes = vec![1; 513];
        let mut a = Assembler::new();
        a.accept("p", fragment("abcdef01", &bytes, 1), now).unwrap();
        let mut conflict = fragment("abcdef01", &bytes, 1);
        conflict.data[0] = 9;
        assert!(a.accept("p", conflict, now).is_err());
        assert_eq!(
            a.accept("p", fragment("abcdef01", &bytes, 2), now).unwrap(),
            Action::Pending
        );
        let mut b = Assembler::new();
        b.accept("p", fragment("abcdef01", &bytes, 1), now).unwrap();
        assert!(b
            .accept("p", fragment("abcdef01", &vec![1; 514], 2), now)
            .is_err());
        assert_eq!(b.reserved_bytes, 0);
    }

    #[test]
    fn finalizing_conflict_rejects_but_completed_conflict_does_not_destroy_success() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", b"yes", 1), now).unwrap());
        assert!(a.accept("p", fragment("abcdef01", b"bad", 1), now).is_err());
        assert_eq!(a.finish(&k, true, now), None);
        let mut b = Assembler::new();
        let (k, _, _) = finalize(b.accept("p", fragment("abcdef01", b"yes", 1), now).unwrap());
        b.finish(&k, true, now);
        assert!(b.accept("p", fragment("abcdef01", b"bad", 1), now).is_err());
        assert!(matches!(
            b.accept("p", fragment("abcdef01", b"yes", 1), now).unwrap(),
            Action::Ack { .. }
        ));
    }

    #[test]
    fn references_before_or_after_data_never_require_another_history_message() {
        let now = Instant::now();
        let mut a = Assembler::new();
        a.reference("p", "abcdef01", now).unwrap();
        a.reference("p", "abcdef01", now + Duration::from_secs(1))
            .unwrap();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", b"ok", 1), now).unwrap());
        a.reference("p", "abcdef01", now).unwrap();
        assert!(a.is_finalizing(&k, now));
        assert_eq!(a.finish(&k, true, now), Some(1));
        a.reference("p", "abcdef01", now).unwrap();
        assert_eq!(a.transfers.len(), 1);
        assert!(matches!(
            a.accept("p", fragment("abcdef01", b"ok", 1), now).unwrap(),
            Action::Ack { .. }
        ));
    }

    #[test]
    fn timeouts_are_once_only_and_late_finalization_cannot_succeed() {
        let now = Instant::now();
        let mut a = Assembler::new();
        a.reference("reference", "abcdef01", now).unwrap();
        a.accept("collecting", fragment("abcdef01", &vec![0; 513], 1), now)
            .unwrap();
        let (k, _, _) = finalize(
            a.accept("finalizing", fragment("abcdef01", b"ok", 1), now)
                .unwrap(),
        );
        assert!(!a.is_finalizing(&k, now + FINALIZING_TIMEOUT));
        assert_eq!(a.expire(now + FINALIZING_TIMEOUT).len(), 1);
        assert_eq!(a.finish(&k, true, now + FINALIZING_TIMEOUT), None);
        assert!(a.expire(now + FINALIZING_TIMEOUT).is_empty());
        assert_eq!(a.expire(now + ACTIVITY_TIMEOUT).len(), 2);
        assert_eq!(a.reserved_bytes, 0);
        assert!(a.expire(now + ACTIVITY_TIMEOUT).is_empty());
    }

    #[test]
    fn finish_checks_deadline_without_waiting_for_expire_tick() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", b"ok", 1), now).unwrap());
        assert_eq!(a.finish(&k, true, now + FINALIZING_TIMEOUT), None);
        assert_eq!(
            a.accept(
                "p",
                fragment("abcdef01", b"ok", 1),
                now + FINALIZING_TIMEOUT
            )
            .unwrap(),
            Action::Pending
        );
    }

    #[test]
    fn activity_deadlines_do_not_slide_on_duplicate_traffic() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let bytes = vec![0; 513];
        a.accept("p", fragment("abcdef01", &bytes, 1), now).unwrap();
        a.accept(
            "p",
            fragment("abcdef01", &bytes, 1),
            now + Duration::from_secs(119),
        )
        .unwrap();
        assert!(a
            .accept("p", fragment("abcdef01", &bytes, 2), now + ACTIVITY_TIMEOUT)
            .is_err());
        assert_eq!(a.reserved_bytes, 0);
    }

    #[test]
    fn active_and_byte_limits_are_checked_before_allocation() {
        let now = Instant::now();
        let mut refs = Assembler::new();
        for n in 0..MAX_ACTIVE {
            refs.reference("p", &format!("{n:08x}"), now).unwrap();
        }
        assert!(refs.reference("p", "ffffffff", now).is_err());
        let mut a = Assembler::new();
        let large = |id: &str| Fragment {
            image_id: id.into(),
            total: MAX_PAYLOAD_BYTES,
            count: (MAX_PAYLOAD_BYTES / 512) as u32,
            index: 1,
            data: vec![0; 512],
        };
        a.accept("p", large("00000001"), now).unwrap();
        a.accept("p", large("00000002"), now).unwrap();
        assert_eq!(a.reserved_bytes, MAX_RESERVED_BYTES);
        assert!(a.accept("p", large("00000003"), now).is_err());
        assert_eq!(a.transfers.len(), 2);
        let invalid = Fragment {
            image_id: "00000004".into(),
            total: usize::MAX,
            count: u32::MAX,
            index: u32::MAX,
            data: vec![0],
        };
        assert!(a.accept("p", invalid, now).is_err());
        a.reference("p", "00000003", now).unwrap();
        assert!(a.accept("p", large("00000003"), now).is_err());
        assert_eq!(
            a.expire(now),
            vec![(key("p", "00000003"), "图片接收缓存额度不足".into())]
        );
        assert!(a.expire(now).is_empty());
    }

    fn assert_failure_once(a: &mut Assembler, expected: TransferKey, now: Instant, reason: &str) {
        let failures = a.expire(now);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, expected);
        assert!(failures[0].1.contains(reason), "{}", failures[0].1);
        assert!(a.expire(now).is_empty());
    }

    #[test]
    fn accept_conflicts_are_reported_once_without_waiting_for_timeout() {
        let now = Instant::now();
        let bytes = vec![1; 513];
        let mut a = Assembler::new();
        a.accept("p", fragment("abcdef01", &bytes, 1), now).unwrap();
        let mut conflict = fragment("abcdef01", &bytes, 1);
        conflict.data[0] = 9;
        assert!(a.accept("p", conflict, now).is_err());
        assert_eq!(
            a.accept("p", fragment("abcdef01", &bytes, 2), now).unwrap(),
            Action::Pending
        );
        assert_failure_once(&mut a, key("p", "abcdef01"), now, "重复数据冲突");
        assert_eq!(
            a.accept("p", fragment("abcdef01", &bytes, 2), now).unwrap(),
            Action::Pending
        );
        assert!(a.expire(now).is_empty());

        let mut b = Assembler::new();
        b.accept("p", fragment("abcdef02", &bytes, 1), now).unwrap();
        assert!(b
            .accept("p", fragment("abcdef02", &vec![1; 514], 2), now)
            .is_err());
        assert_failure_once(
            &mut b,
            key("p", "abcdef02"),
            now,
            "总长度或分片总数发生变化",
        );
    }

    #[test]
    fn reference_and_accept_detected_timeouts_remain_observable() {
        let now = Instant::now();
        let expired = now + ACTIVITY_TIMEOUT;
        let mut a = Assembler::new();
        a.reference("p", "abcdef01", now).unwrap();
        assert!(a.reference("p", "abcdef01", expired).is_err());
        assert!(a.reference("p", "abcdef01", expired).is_err());
        assert_failure_once(&mut a, key("p", "abcdef01"), expired, "引用等待数据超时");

        let mut b = Assembler::new();
        let bytes = vec![1; 513];
        b.accept("p", fragment("abcdef02", &bytes, 1), now).unwrap();
        assert!(b
            .accept("p", fragment("abcdef02", &bytes, 2), expired)
            .is_err());
        assert_failure_once(&mut b, key("p", "abcdef02"), expired, "分片接收超时");
    }

    #[test]
    fn finish_failure_is_reported_once_and_late_success_cannot_replace_it() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", b"ok", 1), now).unwrap());
        assert_eq!(a.finish(&k, false, now), None);
        assert_eq!(a.finish(&k, false, now), None);
        assert_eq!(a.finish(&k, true, now), None);
        assert_eq!(
            a.accept("p", fragment("abcdef01", b"ok", 1), now).unwrap(),
            Action::Pending
        );
        assert_failure_once(&mut a, k, now, "解码或保存失败");
    }

    #[test]
    fn finalizing_conflict_reports_failure_but_completed_conflict_does_not() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", b"yes", 1), now).unwrap());
        assert!(a.accept("p", fragment("abcdef01", b"bad", 1), now).is_err());
        assert_eq!(a.finish(&k, true, now), None);
        assert_failure_once(&mut a, k, now, "完成处理期间出现冲突分片");

        let mut b = Assembler::new();
        let (k, _, _) = finalize(b.accept("p", fragment("abcdef01", b"yes", 1), now).unwrap());
        assert_eq!(b.finish(&k, true, now), Some(1));
        assert!(b.accept("p", fragment("abcdef01", b"bad", 1), now).is_err());
        assert!(b.expire(now).is_empty());
    }

    #[test]
    fn expire_merges_queued_rejections_with_new_timeouts() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (failed, _, _) = finalize(a.accept("p", fragment("abcdef01", b"bad", 1), now).unwrap());
        a.finish(&failed, false, now);
        a.reference("p", "abcdef02", now).unwrap();
        let failures: HashMap<_, _> = a.expire(now + ACTIVITY_TIMEOUT).into_iter().collect();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[&failed], "图片解码或保存失败");
        assert_eq!(failures[&key("p", "abcdef02")], "图片引用等待数据超时");
        assert!(a.expire(now + ACTIVITY_TIMEOUT).is_empty());
    }

    #[test]
    fn a_late_finish_reports_timeout_not_success() {
        let now = Instant::now();
        let mut a = Assembler::new();
        let (k, _, _) = finalize(a.accept("p", fragment("abcdef01", b"ok", 1), now).unwrap());
        assert_eq!(a.finish(&k, true, now + FINALIZING_TIMEOUT), None);
        assert_failure_once(&mut a, k, now + FINALIZING_TIMEOUT, "解码或保存超时");
    }

    #[test]
    fn unexpired_terminal_records_are_not_evicted_for_new_images() {
        let now = Instant::now();
        let mut a = Assembler::new();
        for n in 0..MAX_RECORDS {
            let id = format!("{n:08x}");
            let (k, _, _) = finalize(a.accept("p", fragment(&id, b"x", 1), now).unwrap());
            a.finish(&k, false, now);
        }
        assert!(a.accept("p", fragment("ffffffff", b"x", 1), now).is_err());
        assert_eq!(
            a.accept("p", fragment("00000000", b"x", 1), now).unwrap(),
            Action::Pending
        );
        assert_eq!(a.transfers.len(), MAX_RECORDS);
        assert_eq!(a.pending_failures.len(), MAX_RECORDS);
        // Even nominally expired failures stay pinned until somebody observes
        // them; repeatedly admitting new transfers cannot overflow the queue.
        assert!(a
            .accept("p", fragment("ffffffff", b"x", 1), now + TERMINAL_RETENTION)
            .is_err());
        assert_eq!(a.pending_failures.len(), MAX_RECORDS);
        let failures = a.expire(now + TERMINAL_RETENTION);
        assert_eq!(failures.len(), MAX_RECORDS);
        assert!(a.pending_failures.is_empty());
        assert!(a.expire(now + TERMINAL_RETENTION).is_empty());
        assert!(a.transfers.is_empty());
        assert!(matches!(
            a.accept("p", fragment("ffffffff", b"x", 1), now + TERMINAL_RETENTION)
                .unwrap(),
            Action::Finalize { .. }
        ));
    }
}
