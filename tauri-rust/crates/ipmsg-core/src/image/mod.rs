pub mod assets;
pub mod dib;
pub mod fragments;
pub mod lzw;

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageMetadata {
    pub asset_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub mime: String,
    pub width: u32,
    pub height: u32,
}
