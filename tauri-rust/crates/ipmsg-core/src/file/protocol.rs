use crate::protocol::{
    decode_text, mode, parse_packet, Packet, IPMSG_GETDIRFILES, IPMSG_GETFILEDATA,
};
use std::collections::HashSet;
pub const MAX_FILE_SIZE: u64 = 8 * 1024 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct Offer {
    pub directory: bool,
    pub id: u32,
    pub name: String,
    pub size: u64,
}

pub fn sanitize_name(name: &str) -> String {
    let mut leaf: String = name
        .chars()
        .take(180)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    leaf = leaf.trim_matches([' ', '.']).to_owned();
    if leaf.is_empty() {
        leaf = "未命名文件".into();
    }
    let stem = leaf
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || (stem.starts_with("COM") || stem.starts_with("LPT"))
            && ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&&stem[3..])
    {
        leaf.insert(0, '_');
    }
    leaf
}
fn hex(value: &str) -> Result<u64, String> {
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("非法文件数值".into());
    }
    u64::from_str_radix(value, 16).map_err(|_| "文件数值溢出".into())
}
pub fn offers(packet: &Packet) -> Result<Vec<Offer>, String> {
    let text = decode_text(&packet.extra, packet.command)?;
    let mut offers = Vec::new();
    let mut ids = HashSet::new();
    for row in text.split('\x07').filter(|row| !row.is_empty()) {
        if offers.len() >= 16 {
            return Err("一次最多接收16个文件邀请".into());
        }
        let (id, rest) = row.split_once(':').ok_or("文件邀请缺少ID")?;
        let id = id.parse::<u32>().map_err(|_| "非法文件ID")?;
        if !ids.insert(id) {
            return Err("文件ID重复".into());
        }
        let mut name = String::new();
        let mut chars = rest.chars().peekable();
        let mut ended = false;
        while let Some(c) = chars.next() {
            if c == ':' {
                if chars.peek() == Some(&':') {
                    chars.next();
                    name.push(':');
                } else {
                    ended = true;
                    break;
                }
            } else {
                name.push(c);
            }
        }
        if !ended || name.is_empty() || name.len() > 1024 {
            return Err("文件名无效".into());
        }
        let suffix: String = chars.collect();
        let fields: Vec<_> = suffix.split(':').collect();
        if fields.len() < 3 {
            return Err("文件邀请被截断".into());
        }
        let size = hex(fields[0])?;
        let _mtime = hex(fields[1])?;
        let attr = hex(fields[2])?;
        if size > MAX_FILE_SIZE {
            return Err("文件超过8 GiB上限".into());
        }
        if !matches!(attr & 0xff, 1 | 2) {
            return Err("不支持特殊文件".into());
        }
        offers.push(Offer {
            directory: attr & 0xff == 2,
            id,
            name: sanitize_name(&name),
            size,
        });
    }
    if offers.is_empty() {
        return Err("文件邀请为空".into());
    }
    Ok(offers)
}
pub fn request(bytes: &[u8]) -> Result<(Packet, u32, u32, u64), String> {
    let packet = parse_packet(bytes)?;
    if !matches!(mode(packet.command), IPMSG_GETFILEDATA | IPMSG_GETDIRFILES) {
        return Err("不支持的TCP命令".into());
    }
    let body = if packet.body.is_empty() {
        &packet.extra
    } else {
        &packet.body
    };
    let fields: Vec<_> = std::str::from_utf8(body)
        .map_err(|_| "无效TCP请求")?
        .split(':')
        .collect();
    let directory = mode(packet.command) == IPMSG_GETDIRFILES;
    if !(fields.len() == 4 && fields[3].is_empty()
        || directory && fields.len() == 3 && fields[2].is_empty())
    {
        return Err("TCP请求不完整".into());
    }
    let original = u32::try_from(hex(fields[0])?).map_err(|_| "包号溢出")?;
    let file = u32::try_from(hex(fields[1])?).map_err(|_| "文件ID溢出")?;
    let offset = if directory { 0 } else { hex(fields[2])? };
    Ok((packet, original, file, offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;
    #[test]
    fn names_are_safe_windows_leaves() {
        for name in [
            "../a.exe",
            "C:\\a.txt",
            "aux.txt",
            "con ",
            "LPT1",
            "NUL",
            "...",
            "a:b",
            "中文\0.txt",
        ] {
            let result = sanitize_name(name);
            assert!(!result.contains(['/', '\\', ':', '\0']));
            assert!(!result.ends_with([' ', '.']));
            assert!(!result.is_empty());
        }
        assert_eq!(sanitize_name("中文.txt"), "中文.txt");
        assert_eq!(sanitize_name("aux.txt"), "_aux.txt");
    }
    #[test]
    fn legacy_offer_and_request_formats() {
        let wire = encode_packet(
            123,
            "peer",
            "host",
            IPMSG_SENDMSG | IPMSG_FILEATTACHOPT,
            "文件",
            Some("10:中文::测试.txt:ff:0:1:\x07"),
        )
        .unwrap();
        let parsed = offers(&parse_packet(&wire).unwrap()).unwrap();
        assert_eq!(parsed[0].id, 10);
        assert_eq!(parsed[0].size, 255);
        assert_eq!(parsed[0].name, "中文_测试.txt");
        let wire = encode_packet(9, "peer", "host", IPMSG_GETFILEDATA, "7b:a:ff:", None).unwrap();
        let (_, packet, file, offset) = request(&wire).unwrap();
        assert_eq!((packet, file, offset), (123, 10, 255));
        for extra in [
            "1:x:1:0:4:\x07",
            "1:x:ffffffffffffffff:0:1:\x07",
            "1:x:-1:0:1:\x07",
            "1:x:1:0:1:\x071:y:1:0:1:\x07",
        ] {
            let wire = encode_packet(
                1,
                "p",
                "h",
                IPMSG_SENDMSG | IPMSG_FILEATTACHOPT,
                "",
                Some(extra),
            )
            .unwrap();
            assert!(offers(&parse_packet(&wire).unwrap()).is_err());
        }
    }
}
