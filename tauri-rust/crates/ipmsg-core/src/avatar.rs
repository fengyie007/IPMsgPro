//! FeiQ built-in avatars. The wire ID is 0..83; bitmap resource IDs start at 166.
use crate::protocol::{feiq_compat_version, MAX_PACKET_BYTES};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, sync::RwLock};

pub const AVATAR_COUNT: u8 = 84;
pub const AVATAR_CACHE_LIMIT: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Avatar {
    Builtin { id: u8 },
    Custom,
    Unknown,
}
impl Avatar {
    pub fn valid(&self) -> bool {
        match self {
            Self::Builtin { id } => *id < AVATAR_COUNT,
            Self::Custom | Self::Unknown => true,
        }
    }
}
pub fn validate_id(id: u8) -> Result<(), String> {
    if id < AVATAR_COUNT {
        Ok(())
    } else {
        Err("头像编号必须在0到83之间".into())
    }
}

pub fn parse_avatar(version: &str) -> Option<Avatar> {
    if version.len() > 128 {
        return None;
    }
    let parts: Vec<_> = version.split('#').collect();
    if parts.len() < 5 {
        return None;
    }
    let prefix = parts[0].strip_prefix("1_lbt")?;
    let (dialect, id) = prefix.split_once('_')?;
    if !matches!(dialect, "4" | "6") {
        return None;
    }
    fn number(text: &str) -> Option<u32> {
        if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    }
    let id = number(id)?;
    let custom_bytes = number(parts[3])?;
    number(parts[4])?; // Style size is a separate field, not an avatar identifier.
    if custom_bytes > 0 {
        Some(Avatar::Custom)
    } else if id < u32::from(AVATAR_COUNT) {
        Some(Avatar::Builtin { id: id as u8 })
    } else {
        Some(Avatar::Unknown)
    }
}

/// Shared by text, image and file senders. Refresh only the first field of a
/// queued/retried datagram; keep its packet number, body and binary payload exact.
pub struct LocalIdentity {
    version: RwLock<String>,
}
impl LocalIdentity {
    pub fn new(user_id: &str, port: u16, avatar_id: u8) -> Result<Self, String> {
        let identity = Self {
            version: RwLock::new(feiq_compat_version(user_id, port)),
        };
        identity.set_avatar(avatar_id)?;
        Ok(identity)
    }
    pub fn snapshot(&self) -> String {
        self.version
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn set_avatar(&self, id: u8) -> Result<(), String> {
        validate_id(id)?;
        let mut version = self.version.write().unwrap_or_else(|e| e.into_inner());
        let (_, tail) = version.split_once('#').ok_or("本机协议身份无效")?;
        *version = format!("1_lbt6_{id}#{tail}");
        Ok(())
    }
    pub fn refresh<'a>(&self, packet: &'a [u8]) -> Result<Cow<'a, [u8]>, String> {
        let end = packet
            .iter()
            .take(129)
            .position(|b| *b == b':')
            .ok_or("发包缺少版本字段")?;
        let version = self.version.read().unwrap_or_else(|e| e.into_inner());
        if &packet[..end] == version.as_bytes() {
            return Ok(Cow::Borrowed(packet));
        }
        if version.len() + packet.len() - end > MAX_PACKET_BYTES {
            return Err("更新头像后的报文过长".into());
        }
        let mut result = Vec::with_capacity(version.len() + packet.len() - end);
        result.extend_from_slice(version.as_bytes());
        result.extend_from_slice(&packet[end..]);
        Ok(Cow::Owned(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_custom_and_invalid_profile_fields() {
        for dialect in [4, 6] {
            for id in 0..84 {
                assert_eq!(
                    parse_avatar(&format!("1_lbt{dialect}_{id}#128#mac#0#0#0#4001#9")),
                    Some(Avatar::Builtin { id })
                );
            }
        }
        assert_eq!(
            parse_avatar("1_lbt6_0#128#mac#123#0#0#4001#9"),
            Some(Avatar::Custom)
        );
        assert_eq!(parse_avatar("1_lbt6_84#128#mac#0#0"), Some(Avatar::Unknown));
        for invalid in [
            "1",
            "1_lbt6_3",
            "1_lbt6_-1#128#mac#0#0",
            "1_lbt6_999999999999#128#mac#0#0",
            "1_lbt6_1#128#mac#bad#0",
            "1_lbt6_1#128#mac#0#",
            "1_lbt99_1#128#mac#0#0",
        ] {
            assert_eq!(parse_avatar(invalid), None, "{invalid}");
        }
    }
    #[test]
    fn retry_refresh_preserves_every_byte_after_version() {
        let identity = LocalIdentity::new("me", 2427, 0).unwrap();
        let before = identity.snapshot();
        let mut packet = format!("{before}:42:me:host:2097344:").into_bytes();
        packet.extend(0..=255);
        let boundary = packet.iter().position(|b| *b == b':').unwrap();
        assert!(matches!(
            identity.refresh(&packet).unwrap(),
            Cow::Borrowed(_)
        ));
        identity.set_avatar(83).unwrap();
        let updated = identity.refresh(&packet).unwrap();
        assert!(updated.starts_with(b"1_lbt6_83#"));
        let after = updated.iter().position(|b| *b == b':').unwrap();
        assert_eq!(&updated[after..], &packet[boundary..]);
        assert_eq!(
            identity.snapshot().split_once('#').unwrap().1,
            before.split_once('#').unwrap().1
        );
        assert!(identity.set_avatar(84).is_err());
        assert_eq!(
            parse_avatar(&identity.snapshot()),
            Some(Avatar::Builtin { id: 83 })
        );
    }
}
