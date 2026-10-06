use super::{dib::DecodedImage, ImageMetadata};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_PNG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_THUMB_BYTES: u64 = 2 * 1024 * 1024;

pub fn valid_asset_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 96 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

/// Owns files created for one image until the database accepts them. Dropping
/// a detached blocking task's result also drops this guard and reclaims files.
#[derive(Debug)]
pub struct PendingAsset {
    pub metadata: ImageMetadata,
    owned_directory: PathBuf,
    kept: bool,
}
impl PendingAsset {
    pub fn keep(&mut self) {
        self.kept = true;
    }
    pub(crate) fn metadata_matches_directory(&self) -> bool {
        self.owned_directory
            .file_name()
            .and_then(|name| name.to_str())
            == Some(self.metadata.asset_id.as_str())
    }
}
impl Drop for PendingAsset {
    fn drop(&mut self) {
        if !self.kept {
            if let Ok(canonical) = self.owned_directory.canonicalize() {
                if canonical == self.owned_directory {
                    let _ = fs::remove_dir_all(&self.owned_directory);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct AssetStore {
    root: PathBuf,
}
impl AssetStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        fs::create_dir_all(root.as_ref()).map_err(|e| e.to_string())?;
        Ok(Self {
            root: root.as_ref().canonicalize().map_err(|e| e.to_string())?,
        })
    }
    pub fn persist(&self, decoded: DecodedImage, wire_id: &str) -> Result<PendingAsset, String> {
        if decoded.png.is_empty()
            || decoded.png.len() as u64 > MAX_PNG_BYTES
            || decoded.thumbnail_png.is_empty()
            || decoded.thumbnail_png.len() as u64 > MAX_THUMB_BYTES
        {
            return Err("图片输出超出大小限制".into());
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!(
            "rx-{:x}-{time:x}-{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let directory = self.root.join(&id);
        // create_dir is exclusive: never overwrite or clean up another asset.
        fs::create_dir(&directory).map_err(|e| e.to_string())?;
        // Construct the guard before writing: every failure or dropped task result
        // reclaims only the directory exclusively created by this invocation.
        let pending = PendingAsset {
            metadata: ImageMetadata {
                asset_id: id,
                file_name: format!("飞秋图片_{wire_id}.png"),
                file_size: decoded.png.len() as u64,
                mime: "image/png".into(),
                width: decoded.width,
                height: decoded.height,
            },
            owned_directory: directory,
            kept: false,
        };
        for (name, bytes) in [
            ("full.png", &decoded.png),
            ("thumb.png", &decoded.thumbnail_png),
        ] {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(pending.owned_directory.join(name))
                .map_err(|e| e.to_string())?;
            output
                .write_all(bytes)
                .and_then(|_| output.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(pending)
    }
    // Caller must establish that the ID is registered in image_assets before reading.
    pub fn read(&self, id: &str, thumbnail: bool) -> Result<Vec<u8>, String> {
        if !valid_asset_id(id) {
            return Err("无效图片标识".into());
        }
        let path = self
            .root
            .join(id)
            .join(if thumbnail { "thumb.png" } else { "full.png" })
            .canonicalize()
            .map_err(|_| "图片文件不存在")?;
        if !path.starts_with(&self.root) {
            return Err("图片路径超出应用存储目录".into());
        }
        let limit = if thumbnail {
            MAX_THUMB_BYTES
        } else {
            MAX_PNG_BYTES
        };
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > limit {
            return Err("图片文件过大".into());
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > limit || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("图片文件格式或大小无效".into());
        }
        Ok(bytes)
    }
    /// Caller first marks an unreferenced import as discarded in the database.
    /// Report deletion failures so that tombstones can be retried instead of lost.
    pub fn remove_unreferenced(&self, id: &str) -> Result<(), String> {
        if !valid_asset_id(id) {
            return Err("无效资产标识".into());
        }
        let path = self.root.join(id);
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        if canonical != path {
            return Err("拒绝删除重定向的资产目录".into());
        }
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    }

    // Only for a newly-written, uncommitted asset; never used for history GC.
    pub fn discard_uncommitted(&self, id: &str) {
        if valid_asset_id(id) {
            let path = self.root.join(id);
            if let Ok(canonical) = path.canonicalize() {
                if canonical == path {
                    let _ = fs::remove_dir_all(&path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> AssetStore {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rust-pending-asset-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        AssetStore::new(root).unwrap()
    }
    fn cleanup_fixture(root: &Path) {
        // Every caller passes only its fixture's PID/time/sequence-owned root.
        // Windows may briefly keep a scanner handle after the asset closes.
        for attempt in 0..10 {
            match fs::remove_dir_all(root) {
                Ok(()) => return,
                Err(error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied && attempt < 9 =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => panic!("asset fixture cleanup failed: {error}"),
            }
        }
    }

    fn decoded() -> DecodedImage {
        // Persistence tests exercise ownership, not the separately-tested codec.
        DecodedImage {
            png: b"\x89PNG\r\n\x1a\nowned full".to_vec(),
            thumbnail_png: b"\x89PNG\r\n\x1a\nowned thumb".to_vec(),
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn asset_ids_cannot_address_arbitrary_files() {
        assert!(valid_asset_id("rx-123-abc-0"));
        for id in ["", "..", "../secret", "C:\\secret", "a/b", "%2e%2e", "你好"] {
            assert!(!valid_asset_id(id));
        }
    }

    #[test]
    fn pending_drop_removes_only_its_owned_directory_and_keep_preserves_files() {
        let store = fixture();
        let mut kept = store.persist(decoded(), "11111111").unwrap();
        let kept_directory = kept.owned_directory.clone();
        let mut pending = store.persist(decoded(), "22222222").unwrap();
        let pending_directory = pending.owned_directory.clone();
        // Public metadata is not trusted as a deletion path.
        pending.metadata.asset_id = kept.metadata.asset_id.clone();
        kept.keep();
        drop(kept);
        drop(pending);
        assert!(!pending_directory.exists());
        assert!(kept_directory.join("full.png").exists());
        assert!(kept_directory.join("thumb.png").exists());
        cleanup_fixture(&store.root);
    }

    #[tokio::test]
    async fn detached_blocking_result_reclaims_uncommitted_files() {
        let store = fixture();
        let root = store.root.clone();
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let (resume, blocked) = std::sync::mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            let pending = store.persist(decoded(), "33333333").unwrap();
            ready.send(pending.metadata.asset_id.clone()).unwrap();
            blocked
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            pending
        });
        let id = waiting.await.unwrap();
        assert!(root.join(&id).exists());
        // A running spawn_blocking task survives abort; its unobserved result
        // must nevertheless clean up the files instead of leaking an asset.
        task.abort();
        drop(task);
        resume.send(()).unwrap();
        for _ in 0..100 {
            if !root.join(&id).exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!root.join(&id).exists());
        cleanup_fixture(&root);
    }
}
