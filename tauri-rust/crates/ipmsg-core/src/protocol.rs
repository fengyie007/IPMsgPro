//! IPMsg v1 framing and encoding (also accepts FeiQ's extended version string).
//! Payload bytes remain raw until the caller explicitly requests text decoding.

use encoding_rs::GBK;
use std::net::Ipv4Addr;

pub const IPMSG_VERSION: u32 = 1;
pub const IPMSG_DEFAULT_PORT: u16 = 2425;
pub const IPMSG_NOOPERATION: u32 = 0x00;
pub const IPMSG_BR_ENTRY: u32 = 0x01;
pub const IPMSG_BR_EXIT: u32 = 0x02;
pub const IPMSG_ANSENTRY: u32 = 0x03;
pub const IPMSG_BR_ABSENCE: u32 = 0x04;
pub const IPMSG_SENDMSG: u32 = 0x20;
pub const IPMSG_RECVMSG: u32 = 0x21;
pub const IPMSG_READMSG: u32 = 0x30;
pub const IPMSG_DELMSG: u32 = 0x31;
pub const IPMSG_ANSREADMSG: u32 = 0x32;
pub const IPMSG_GETINFO: u32 = 0x40;
pub const IPMSG_SENDINFO: u32 = 0x41;
pub const IPMSG_GETABSENCEINFO: u32 = 0x50;
pub const IPMSG_SENDABSENCEINFO: u32 = 0x51;
pub const IPMSG_GETFILEDATA: u32 = 0x60;
pub const IPMSG_RELEASEFILES: u32 = 0x61;
pub const IPMSG_GETDIRFILES: u32 = 0x62;
pub const IPMSG_GETPUBKEY: u32 = 0x72;
pub const IPMSG_ANSPUBKEY: u32 = 0x73;
pub const IPMSG_SENDIMAGE: u32 = 0xc0;
pub const IPMSG_REPORT_RECVIMAGE: u32 = 0xc1;

pub const IPMSG_SENDCHECKOPT: u32 = 0x0000_0100;
pub const IPMSG_ABSENCEOPT: u32 = 0x0000_0100;
pub const IPMSG_FILEATTACHOPT: u32 = 0x0020_0000;
pub const IPMSG_ENCRYPTOPT: u32 = 0x0040_0000;
pub const IPMSG_UTF8OPT: u32 = 0x0080_0000;
pub const IPMSG_CAPUTF8OPT: u32 = 0x0100_0000;

/// Largest legal IPv4 UDP payload (including the IPMsg header).
pub const MAX_PACKET_BYTES: usize = 65_507;
/// This MVP deliberately bounds individual text messages, measured on the wire.
pub const MAX_TEXT_BYTES: usize = 32 * 1024;
const MAX_IDENTITY_BYTES: usize = 512;
const MAX_VERSION_BYTES: usize = 128;

pub const fn mode(command: u32) -> u32 {
    command & 0xff
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub version: String,
    pub packet_no: u32,
    pub username: String,
    pub hostname: String,
    pub command: u32,
    pub body: Vec<u8>,
    pub extra: Vec<u8>,
}

fn decimal(bytes: &[u8], field: &str) -> Result<u32, String> {
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(format!("无效的 {field} 数字字段"));
    }
    // ASCII digits are always valid UTF-8; checked parsing also detects overflow.
    std::str::from_utf8(bytes)
        .map_err(|_| format!("无效的 {field} 字段"))?
        .parse::<u32>()
        .map_err(|_| format!("{field} 超出范围"))
}

fn version(bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty()
        || bytes.len() > MAX_VERSION_BYTES
        || !bytes.iter().all(u8::is_ascii_graphic)
    {
        return Err("无效的 IPMsg 版本字段".into());
    }
    let end = bytes
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(bytes.len());
    if decimal(&bytes[..end], "version")? != IPMSG_VERSION {
        return Err("不支持的 IPMsg 版本".into());
    }
    if end < bytes.len() && bytes[end] != b'_' {
        return Err("无效的 IPMsg 扩展版本字段".into());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "无效的 IPMsg 版本字段".into())
}

fn validate_identity(value: &str, name: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().any(|c| c == ':' || c.is_control()) {
        return Err(format!("{name} 不能为空或含冒号、控制字符"));
    }
    Ok(())
}

/// Decode exactly the supplied bytes. A capability announcement alone does not
/// select UTF-8: only UTF8OPT does. Invalid input never silently becomes U+FFFD.
pub fn decode_text(bytes: &[u8], command: u32) -> Result<String, String> {
    if bytes.len() > MAX_PACKET_BYTES {
        return Err("文本数据超出 UDP 大小限制".into());
    }
    if command & IPMSG_UTF8OPT != 0 {
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| "无效的 UTF-8 文本".into())
    } else {
        GBK.decode_without_bom_handling_and_without_replacement(bytes)
            .map(|text| text.into_owned())
            .ok_or_else(|| "无效的 GBK 文本".into())
    }
}

fn encode_text(text: &str, command: u32) -> Result<Vec<u8>, String> {
    if text.contains('\0') {
        return Err("文本不能包含 NUL 字符".into());
    }
    if command & IPMSG_UTF8OPT != 0 {
        Ok(text.as_bytes().to_vec())
    } else {
        let (bytes, _, errors) = GBK.encode(text);
        if errors {
            return Err("文本包含 GBK 无法表示的字符，对端需要支持 UTF-8".into());
        }
        Ok(bytes.into_owned())
    }
}

/// Parse at the first five colons only. Image and unknown command bodies are
/// preserved whole (including NULs); this crate does not decode image payloads.
pub fn parse_packet(data: &[u8]) -> Result<Packet, String> {
    if data.is_empty() || data.len() > MAX_PACKET_BYTES {
        return Err("IPMsg 报文为空或超过 UDP 大小限制".into());
    }
    let parts: Vec<&[u8]> = data.splitn(6, |byte| *byte == b':').collect();
    if parts.len() != 6 {
        return Err("IPMsg 报文头不足五个分隔符".into());
    }
    let version = version(parts[0])?;
    let packet_no = decimal(parts[1], "packetNo")?;
    let command = decimal(parts[4], "command")?;
    if parts[2].len() > MAX_IDENTITY_BYTES || parts[3].len() > MAX_IDENTITY_BYTES {
        return Err("IPMsg 用户名或主机名过长".into());
    }
    let username = decode_text(parts[2], command)?;
    let hostname = decode_text(parts[3], command)?;
    validate_identity(&username, "用户名")?;
    validate_identity(&hostname, "主机名")?;

    let text_framed = matches!(
        mode(command),
        IPMSG_NOOPERATION
            | IPMSG_BR_ENTRY
            | IPMSG_BR_EXIT
            | IPMSG_ANSENTRY
            | IPMSG_BR_ABSENCE
            | IPMSG_SENDMSG
            | IPMSG_RECVMSG
            | IPMSG_READMSG
            | IPMSG_DELMSG
            | IPMSG_ANSREADMSG
            | IPMSG_GETINFO
            | IPMSG_SENDINFO
            | IPMSG_GETABSENCEINFO
            | IPMSG_SENDABSENCEINFO
            | IPMSG_GETFILEDATA
            | IPMSG_RELEASEFILES
            | IPMSG_GETDIRFILES
            | IPMSG_GETPUBKEY
            | IPMSG_ANSPUBKEY
            | IPMSG_REPORT_RECVIMAGE
    );
    let (body, extra) = if text_framed {
        let mut sections = parts[5].split(|byte| *byte == 0);
        (
            sections.next().unwrap_or_default().to_vec(),
            sections.next().unwrap_or_default().to_vec(),
        )
    } else {
        (parts[5].to_vec(), Vec::new())
    };
    if mode(command) == IPMSG_SENDMSG && body.len() > MAX_TEXT_BYTES {
        return Err("文本消息超过 32 KiB".into());
    }
    Ok(Packet {
        version,
        packet_no,
        username,
        hostname,
        command,
        body,
        extra,
    })
}

/// Encode a textual IPMsg packet. `Some(extra)` emits body-NUL-extra-NUL;
/// `None` emits body-NUL. Binary images need their own future transport API.
pub fn encode_packet(
    packet_no: u32,
    username: &str,
    hostname: &str,
    command: u32,
    body: &str,
    extra: Option<&str>,
) -> Result<Vec<u8>, String> {
    if mode(command) == IPMSG_SENDIMAGE {
        return Err("核心版尚不支持编码二进制图片报文".into());
    }
    validate_identity(username, "用户名")?;
    validate_identity(hostname, "主机名")?;
    let username = encode_text(username, command)?;
    let hostname = encode_text(hostname, command)?;
    if username.len() > MAX_IDENTITY_BYTES || hostname.len() > MAX_IDENTITY_BYTES {
        return Err("IPMsg 用户名或主机名过长".into());
    }
    let body = encode_text(body, command)?;
    if mode(command) == IPMSG_SENDMSG && body.len() > MAX_TEXT_BYTES {
        return Err("文本消息超过 32 KiB".into());
    }
    let extra = extra.map(|value| encode_text(value, command)).transpose()?;
    let mut data = format!("1:{packet_no}:").into_bytes();
    data.extend_from_slice(&username);
    data.push(b':');
    data.extend_from_slice(&hostname);
    data.extend_from_slice(format!(":{command}:").as_bytes());
    data.extend_from_slice(&body);
    data.push(0);
    if let Some(extra) = extra {
        data.extend_from_slice(&extra);
        data.push(0);
    }
    if data.len() > MAX_PACKET_BYTES {
        return Err("IPMsg 报文超过 UDP 大小限制".into());
    }
    Ok(data)
}

/// Compute an IPv4 subnet's directed-broadcast address. Interface discovery and
/// filtering out loopback/point-to-point adapters are the caller's responsibility.
pub fn subnet_broadcast(ip: Ipv4Addr, prefix: u8) -> Option<Ipv4Addr> {
    if prefix > 32 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some(Ipv4Addr::from(u32::from(ip) | !mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbk_round_trip_preserves_chinese_colons_newlines_and_emoji_xml() {
        let body = "你好:第一行\r\n第二行<msg><emoji type=\"1\" id=\"e2_02\" /></msg>";
        let wire = encode_packet(
            42,
            "用户",
            "主机",
            IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
            body,
            None,
        )
        .unwrap();
        let parsed = parse_packet(&wire).unwrap();
        assert_eq!(parsed.packet_no, 42);
        assert_eq!(parsed.username, "用户");
        assert_eq!(parsed.hostname, "主机");
        assert_eq!(decode_text(&parsed.body, parsed.command).unwrap(), body);
        assert!(wire.ends_with(&[0]));
        assert!(!wire
            .windows("你好".len())
            .any(|bytes| bytes == "你好".as_bytes()));
    }

    #[test]
    fn utf8_round_trip_includes_non_gbk_characters() {
        let body = "你好 🦀\nhttps://example.test:123/path";
        let command = IPMSG_SENDMSG | IPMSG_UTF8OPT;
        let packet =
            parse_packet(&encode_packet(7, "u", "host", command, body, None).unwrap()).unwrap();
        assert_eq!(decode_text(&packet.body, command).unwrap(), body);
        assert!(encode_packet(7, "u", "host", IPMSG_SENDMSG, body, None).is_err());
    }

    #[test]
    fn capability_bit_does_not_change_wire_encoding() {
        let wire = encode_packet(
            9,
            "u",
            "h",
            IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
            "昵称",
            Some("研发组"),
        )
        .unwrap();
        let packet = parse_packet(&wire).unwrap();
        assert_eq!(packet.body, GBK.encode("昵称").0.as_ref());
        assert_eq!(
            decode_text(&packet.extra, packet.command).unwrap(),
            "研发组"
        );
    }

    #[test]
    fn empty_extra_gets_its_own_terminator() {
        let wire = encode_packet(9, "u", "h", IPMSG_BR_ENTRY, "nick", Some("")).unwrap();
        assert!(wire.ends_with(&[0, 0]));
        let packet = parse_packet(&wire).unwrap();
        assert_eq!(packet.body, b"nick");
        assert!(packet.extra.is_empty());
    }

    #[test]
    fn accepts_feiq_extended_version_and_unterminated_legacy_body() {
        let data = b"1_lbt6_0#128#001122334455#0#0#0#4001#9:12:alice:host:33:1234";
        let packet = parse_packet(data).unwrap();
        assert!(packet.version.starts_with("1_lbt6_"));
        assert_eq!(packet.command, IPMSG_RECVMSG);
        assert_eq!(packet.body, b"1234");
    }

    #[test]
    fn malformed_headers_are_rejected_without_panics() {
        for bytes in [
            &b""[..],
            &b"1:1:u:h:32"[..],
            &b"2:1:u:h:32:x"[..],
            &b"1evil:1:u:h:32:x"[..],
            &b"1::u:h:32:x"[..],
            &b"1:-1:u:h:32:x"[..],
            &b"1:+1:u:h:32:x"[..],
            &b"1:1 :u:h:32:x"[..],
            &b"1:4294967296:u:h:32:x"[..],
            &b"1:1:u:h:4294967296:x"[..],
            &b"1:1:u:h:-1:x"[..],
            &b"1:1::h:32:x"[..],
            &b"1:1:u::32:x"[..],
            &b"1:1:u\0:h:32:x"[..],
            &b"1:1:u:h\n:32:x"[..],
        ] {
            assert!(parse_packet(bytes).is_err(), "accepted {bytes:?}");
        }
    }

    #[test]
    fn u32_boundaries_and_mode_mask_are_explicit() {
        let packet = parse_packet(b"1:4294967295:u:h:4294967295:x").unwrap();
        assert_eq!(packet.packet_no, u32::MAX);
        assert_eq!(packet.command, u32::MAX);
        assert_eq!(mode(packet.command), 255);
        assert_eq!(mode(IPMSG_SENDMSG | IPMSG_SENDCHECKOPT), IPMSG_SENDMSG);
        assert_eq!(parse_packet(b"1:0:u:h:0:").unwrap().packet_no, 0);
    }

    #[test]
    fn invalid_encoded_text_is_reported_not_replaced() {
        assert!(decode_text(&[0xff], IPMSG_UTF8OPT).is_err());
        assert!(decode_text(&[0x81], 0).is_err());
        // Framing must not decode text payloads implicitly.
        let packet = parse_packet(b"1:1:u:h:8388640:\xff\0").unwrap();
        assert_eq!(packet.body, [0xff]);
        assert!(decode_text(&packet.body, packet.command).is_err());
    }

    #[test]
    fn image_and_unknown_binary_bodies_remain_byte_exact() {
        for command in [IPMSG_SENDIMAGE | IPMSG_FILEATTACHOPT, 0xee] {
            let binary = b"ab12cd34|14|0|1|1|14|0|1|0|00000000#\0\xff:\0\x81LZW!";
            let mut data = format!("1:99:u:h:{command}:").into_bytes();
            data.extend_from_slice(binary);
            let packet = parse_packet(&data).unwrap();
            assert_eq!(packet.body, binary);
            assert!(packet.extra.is_empty());
        }
        assert!(encode_packet(1, "u", "h", IPMSG_SENDIMAGE, "not binary", None).is_err());
    }

    #[test]
    fn text_extra_does_not_leak_into_the_message_body() {
        let packet = parse_packet(b"1:5:u:h:32:hello:world\0file:info\0\nGN:ignored").unwrap();
        assert_eq!(packet.body, b"hello:world");
        assert_eq!(packet.extra, b"file:info");
    }

    #[test]
    fn local_nuls_and_header_delimiters_are_rejected() {
        for (user, host, body, extra) in [
            ("u:x", "h", "ok", None),
            ("u", "h:x", "ok", None),
            ("", "h", "ok", None),
            ("u", "h", "bad\0body", None),
            ("u", "h", "ok", Some("bad\0group")),
        ] {
            assert!(encode_packet(1, user, host, IPMSG_SENDMSG, body, extra).is_err());
        }
    }

    #[test]
    fn wire_and_text_limits_are_enforced() {
        let body = "a".repeat(MAX_TEXT_BYTES);
        let wire = encode_packet(1, "u", "h", IPMSG_SENDMSG, &body, None).unwrap();
        assert_eq!(parse_packet(&wire).unwrap().body.len(), MAX_TEXT_BYTES);
        assert!(encode_packet(1, "u", "h", IPMSG_SENDMSG, &(body + "a"), None).is_err());
        let mut excessive_text = b"1:1:u:h:32:".to_vec();
        excessive_text.extend(vec![b'a'; MAX_TEXT_BYTES + 1]);
        assert!(parse_packet(&excessive_text).is_err());
        assert!(parse_packet(&vec![b'a'; MAX_PACKET_BYTES + 1]).is_err());
        assert!(encode_packet(
            1,
            "u",
            "h",
            IPMSG_BR_ENTRY,
            "",
            Some(&"a".repeat(MAX_PACKET_BYTES))
        )
        .is_err());
    }

    #[test]
    fn every_truncated_header_is_rejected() {
        let prefix = b"1:123:user:host:32:";
        for length in 0..prefix.len() {
            assert!(parse_packet(&prefix[..length]).is_err());
        }
        assert!(parse_packet(prefix).is_ok());
    }

    #[test]
    fn directed_broadcast_uses_the_prefix_not_a_fixed_slash_24() {
        let ip = Ipv4Addr::new(10, 8, 34, 11);
        assert_eq!(
            subnet_broadcast(ip, 24),
            Some(Ipv4Addr::new(10, 8, 34, 255))
        );
        assert_eq!(
            subnet_broadcast(ip, 23),
            Some(Ipv4Addr::new(10, 8, 35, 255))
        );
        assert_eq!(
            subnet_broadcast(ip, 16),
            Some(Ipv4Addr::new(10, 8, 255, 255))
        );
        assert_eq!(subnet_broadcast(ip, 0), Some(Ipv4Addr::BROADCAST));
        assert_eq!(subnet_broadcast(ip, 32), Some(ip));
        assert_eq!(subnet_broadcast(ip, 33), None);
    }
}
