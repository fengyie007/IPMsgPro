//! Validation shared by persisted scan settings and one-shot scan commands.
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const MAX_RANGES: usize = 32;
pub const MAX_ADDRESSES: u64 = 65_536;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanOptions {
    pub ranges: Vec<String>,
    pub port: u16,
    pub delay_ms: u32,
}
#[derive(Clone, Debug)]
pub struct ScanPlan {
    pub ranges: Vec<(u32, u32)>,
    pub total: u32,
    pub port: u16,
    pub delay_ms: u32,
}
fn address(text: &str) -> Result<u32, String> {
    text.parse::<Ipv4Addr>()
        .map(u32::from)
        .map_err(|_| format!("无效IPv4地址：{text}"))
}
pub fn parse_range(text: &str) -> Result<(u32, u32), String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 64 {
        return Err("扫描范围为空或过长".into());
    }
    let (start, end) = if let Some((ip, prefix)) = text.split_once('/') {
        let ip = address(ip.trim())?;
        if prefix.is_empty() || prefix.len() > 2 || !prefix.bytes().all(|b| b.is_ascii_digit()) {
            return Err("CIDR前缀必须为0～32".into());
        }
        let prefix: u32 = prefix.parse().map_err(|_| "无效CIDR前缀")?;
        if prefix > 32 {
            return Err("CIDR前缀必须为0～32".into());
        }
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        let network = ip & mask;
        let broadcast = network | !mask;
        if prefix <= 30 {
            (network + 1, broadcast - 1)
        } else {
            (network, broadcast)
        }
    } else if let Some((start, end)) = text.split_once('-') {
        let start = address(start.trim())?;
        let end = end.trim();
        let end = if !end.is_empty() && end.len() <= 3 && end.bytes().all(|b| b.is_ascii_digit()) {
            (start & 0xffffff00) | u32::from(end.parse::<u8>().map_err(|_| "结束段必须为0～255")?)
        } else {
            address(end)?
        };
        (start, end)
    } else {
        let ip = address(text)?;
        (ip, ip)
    };
    // Do not assume /24: a .0 or .255 may be a host in the user's actual subnet.
    if start < 0x01000000 || end >= 0xe0000000 || end < start {
        return Err("范围必须是正序单播IPv4地址，不含0网段、组播或保留地址".into());
    }
    if u64::from(end) - u64::from(start) + 1 > MAX_ADDRESSES {
        return Err("每次扫描最多65536个地址".into());
    }
    Ok((start, end))
}
impl ScanOptions {
    pub fn plan(&self) -> Result<ScanPlan, String> {
        if self.port == 0 {
            return Err("目标端口必须为1～65535".into());
        }
        if !(10..=1000).contains(&self.delay_ms) {
            return Err("扫描间隔必须为10～1000毫秒".into());
        }
        if self.ranges.len() > MAX_RANGES {
            return Err("扫描范围最多32项".into());
        }
        let mut ranges = self
            .ranges
            .iter()
            .map(|r| parse_range(r))
            .collect::<Result<Vec<_>, _>>()?;
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (start, end) in ranges {
            if let Some(previous) = merged.last_mut().filter(|previous| start <= previous.1 + 1) {
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        let total: u64 = merged
            .iter()
            .map(|&(start, end)| u64::from(end) - u64::from(start) + 1)
            .sum();
        if total > MAX_ADDRESSES {
            return Err("合并后的扫描范围最多65536个地址".into());
        }
        Ok(ScanPlan {
            ranges: merged,
            total: total as u32,
            port: self.port,
            delay_ms: self.delay_ms,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    Idle,
    Running,
    Waiting,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}
impl ScanPhase {
    pub fn active(self) -> bool {
        matches!(self, Self::Running | Self::Waiting | Self::Cancelling)
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub scan_id: u64,
    pub revision: u64,
    pub state: ScanPhase,
    pub current: u32,
    pub total: u32,
    pub found: u32,
    pub failed_sends: u32,
    pub skipped: u32,
    pub port: u16,
    pub delay_ms: u32,
    pub ranges: Vec<String>,
    pub error: Option<String>,
}
impl Default for ScanStatus {
    fn default() -> Self {
        Self {
            scan_id: 0,
            revision: 0,
            state: ScanPhase::Idle,
            current: 0,
            total: 0,
            found: 0,
            failed_sends: 0,
            skipped: 0,
            port: 2425,
            delay_ms: 20,
            ranges: vec![],
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_forms_and_boundaries() {
        for (text, start, end) in [
            ("10.8.33.1-254", "10.8.33.1", "10.8.33.254"),
            (" 10.8.33.9/24 ", "10.8.33.1", "10.8.33.254"),
            ("10.8.33.0/31", "10.8.33.0", "10.8.33.1"),
            ("127.0.0.1/32", "127.0.0.1", "127.0.0.1"),
            ("127.0.0.1", "127.0.0.1", "127.0.0.1"),
            ("10.0.0.255-10.0.1.0", "10.0.0.255", "10.0.1.0"),
        ] {
            assert_eq!(
                parse_range(text).unwrap(),
                (address(start).unwrap(), address(end).unwrap())
            );
        }
        for text in [
            "",
            "10.0.0.2-1",
            "10.0.0.1-256",
            "10.0.0.1/33",
            "10.0.0.1/",
            "10.0.0.1/-1",
            "0.0.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "10.0.0.0/15",
            "01.2.3.4",
            "10.0.0.1-2-3",
        ] {
            assert!(parse_range(text).is_err(), "{text}");
        }
    }
    #[test]
    fn merge_deduplicates_overlaps_and_caps_combined_total() {
        let plan = ScanOptions {
            ranges: vec![
                "10.0.0.1-5".into(),
                "10.0.0.3-7".into(),
                "10.0.0.7-9".into(),
            ],
            port: 2425,
            delay_ms: 20,
        }
        .plan()
        .unwrap();
        assert_eq!(plan.total, 9);
        assert_eq!(plan.ranges.len(), 1);
        let mut options = ScanOptions {
            ranges: vec!["10.0.0.0-10.0.255.255".into()],
            port: 2425,
            delay_ms: 10,
        };
        assert_eq!(options.plan().unwrap().total, 65536);
        options.ranges.push("10.1.0.1".into());
        assert!(options.plan().is_err());
        options.ranges.clear();
        assert_eq!(options.plan().unwrap().total, 0);
        options.delay_ms = 0;
        assert!(options.plan().is_err());
        options.delay_ms = 20;
        options.port = 0;
        assert!(options.plan().is_err());
    }
}
