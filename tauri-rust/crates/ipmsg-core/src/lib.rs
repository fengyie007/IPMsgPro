//! Tauri-independent protocol, text messaging, configuration and persistence.

pub mod avatar;
pub mod config;
pub mod database;
pub mod file;
pub mod image;
pub mod network;
pub mod protocol;
pub mod scan;
pub mod text;

use serde::{Deserialize, Serialize};

/// User snapshot exchanged with the frontend. Status is online, away or offline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub nickname: String,
    pub username: String,
    pub hostname: String,
    pub group: String,
    pub ip: String,
    pub port: u16,
    pub status: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<avatar::Avatar>,
    /// Normalized MAC field advertised by the peer (may be virtual).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::User;
    #[test]
    fn old_user_snapshots_without_mac_still_deserialize() {
        let user: User = serde_json::from_value(serde_json::json!({"id":"old","nickname":"peer","username":"peer",
            "hostname":"host","group":"","ip":"127.0.0.1","port":2425,"status":"offline","version":"1"})).unwrap();
        assert!(user.mac.is_none());
        assert!(serde_json::to_value(user).unwrap().get("mac").is_none());
    }
}
