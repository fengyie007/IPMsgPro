//! Tauri-independent protocol, text messaging, configuration and persistence.

pub mod config;
pub mod database;
pub mod image;
pub mod network;
pub mod protocol;
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
}
