mod directory;
pub mod protocol;
mod storage;
mod transfer;
use serde::{Deserialize, Serialize};
pub use transfer::{FileTransfers, Selection};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    #[serde(default)]
    pub is_directory: bool,
    #[serde(default)]
    pub can_resume: bool,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub in_flight_bytes: u64,
    pub file_name: String,
    pub file_size: u64,
    pub state: String,
    pub transferred: u64,
    pub incoming: bool,
    pub has_local_file: bool,
    pub error: Option<String>,
}
impl FileMetadata {
    pub fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "completed" | "failed" | "cancelled" | "rejected" | "paused"
        )
    }
    pub fn status(&self) -> i64 {
        if self.state == "completed" {
            2
        } else if self.terminal() {
            3
        } else {
            0
        }
    }
}
