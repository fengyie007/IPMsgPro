use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    net::SocketAddrV4,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfig {
    pub nickname: String,
    pub group: String,
    pub direct_users: Vec<String>,
    pub minimize_behavior: String,
    pub notification_sound: bool,
    pub segments: Vec<String>,
    pub ip_scan_ranges: Vec<String>,
    pub scan_port: u16,
    pub scan_delay_ms: u32,
    pub scan_on_startup: bool,
    pub data_dir: String,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            nickname: String::new(),
            group: String::new(),
            direct_users: vec![],
            minimize_behavior: "tray".into(),
            notification_sound: false,
            segments: vec![],
            ip_scan_ranges: vec![],
            scan_port: 2425,
            scan_delay_ms: 20,
            scan_on_startup: true,
            data_dir: String::new(),
        }
    }
}
impl AppConfig {
    pub fn validate(&self) -> Result<(), String> {
        for text in [&self.nickname, &self.group] {
            if text.len() > 256 || text.contains(['\0', '\r', '\n']) {
                return Err("昵称/组名过长或包含控制字符".into());
            }
        }
        if !matches!(self.minimize_behavior.as_str(), "tray" | "taskbar") {
            return Err("无效的关闭行为".into());
        }
        if self.notification_sound || !self.segments.is_empty() || !self.data_dir.is_empty() {
            return Err("Rust核心版暂不支持提示音、自定义广播网段或修改数据目录".into());
        }
        crate::protocol::encode_packet(
            1,
            "preview",
            "localhost",
            crate::protocol::IPMSG_BR_ENTRY,
            &self.nickname,
            Some(&self.group),
        )?;
        if self.direct_users.len() > 256 {
            return Err("直接用户最多256项".into());
        }
        for address in &self.direct_users {
            parse_address(address)?;
        }
        crate::scan::ScanOptions {
            ranges: self.ip_scan_ranges.clone(),
            port: self.scan_port,
            delay_ms: self.scan_delay_ms,
        }
        .plan()?;
        Ok(())
    }
    fn merged(&self, patch: Value) -> Result<Self, String> {
        let patch = patch.as_object().ok_or("设置必须是对象")?;
        let mut value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        let object = value.as_object_mut().ok_or("设置序列化失败")?;
        for (key, field) in patch {
            if !object.contains_key(key) {
                return Err(format!("未知设置：{key}"));
            }
            object.insert(key.clone(), field.clone());
        }
        let next: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        next.validate()?;
        Ok(next)
    }
}
pub fn parse_address(text: &str) -> Result<SocketAddrV4, String> {
    let address: SocketAddrV4 = text.parse().map_err(|_| format!("无效IPv4:port：{text}"))?;
    if address.port() == 0
        || address.ip().is_unspecified()
        || address.ip().is_multicast()
        || address.ip().is_broadcast()
    {
        return Err(format!("无效的对端地址：{text}"));
    }
    Ok(address)
}

pub struct ConfigStore {
    path: PathBuf,
    current: Mutex<AppConfig>,
    writer: tokio::sync::Mutex<()>,
}
impl ConfigStore {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let config = if path.exists() {
            if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 65536 {
                return Err("配置文件过大；未覆盖原文件".into());
            }
            serde_json::from_slice::<AppConfig>(&fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("配置损坏，原文件已保留：{e}"))?
        } else {
            let defaults = AppConfig::default();
            write_atomic(&path, &defaults)?;
            defaults
        };
        config.validate()?;
        Ok(Self {
            path,
            current: Mutex::new(config),
            writer: tokio::sync::Mutex::new(()),
        })
    }
    pub fn get(&self) -> AppConfig {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub async fn save(&self, patch: Value) -> Result<AppConfig, String> {
        let _serial = self.writer.lock().await;
        let next = self.get().merged(patch)?;
        let copy = next.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write_atomic(&path, &copy))
            .await
            .map_err(|e| e.to_string())??;
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = next.clone();
        Ok(next)
    }
}

fn write_atomic(path: &Path, config: &AppConfig) -> Result<(), String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let data = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    // Only clean up a temporary file that this call successfully created.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        replace(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patches_validate_before_changing_state() {
        let current = AppConfig::default();
        assert!(current
            .merged(serde_json::json!({"minimizeBehavior":"exit"}))
            .is_err());
        assert!(current
            .merged(serde_json::json!({"directUsers":["127.0.0.1:0"]}))
            .is_err());
        assert!(current
            .merged(serde_json::json!({"notificationSound":true}))
            .is_err());
        assert_eq!(current.minimize_behavior, "tray");
        let next = current
            .merged(serde_json::json!({"nickname":"测试","directUsers":["127.0.0.1:2426"]}))
            .unwrap();
        assert_eq!(next.nickname, "测试");
    }
    #[test]
    fn scan_settings_validate_without_changing_legacy_defaults() {
        let legacy: AppConfig =
            serde_json::from_value(serde_json::json!({"nickname":"原配置","ipScanRanges":[]}))
                .unwrap();
        assert_eq!(
            (
                legacy.scan_port,
                legacy.scan_delay_ms,
                legacy.scan_on_startup
            ),
            (2425, 20, true)
        );
        legacy.validate().unwrap();
        let next=legacy.merged(serde_json::json!({"ipScanRanges":["10.8.33.0/24"],"scanPort":2426,"scanDelayMs":50,"scanOnStartup":false})).unwrap();
        assert_eq!(next.scan_port, 2426);
        assert_eq!(legacy.scan_port, 2425);
        for patch in [
            serde_json::json!({"scanPort":0}),
            serde_json::json!({"scanDelayMs":9}),
            serde_json::json!({"scanOnStartup":"yes"}),
            serde_json::json!({"ipScanRanges":["224.0.0.1"]}),
            serde_json::json!({"ipScanRanges":["10.0.0.0/16","10.1.0.0/16"]}),
        ] {
            assert!(legacy.merged(patch).is_err());
        }
    }
    #[tokio::test]
    async fn save_is_durable_and_failures_keep_previous_state() {
        let root = std::env::temp_dir().join(format!(
            "speedipmsg-rust-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("config.json");
        let mut store = ConfigStore::open(path.clone()).unwrap();
        store
            .save(serde_json::json!({"nickname":"持久配置","minimizeBehavior":"taskbar"}))
            .await
            .unwrap();
        assert_eq!(ConfigStore::open(path.clone()).unwrap().get(), store.get());
        let before = store.get();
        let bytes = fs::read(&path).unwrap();
        store.path = root.join("missing/config.json");
        assert!(store
            .save(serde_json::json!({"nickname":"不会生效"}))
            .await
            .is_err());
        assert_eq!(store.get(), before);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, b"{broken").unwrap();
        assert!(ConfigStore::open(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
        fs::remove_dir_all(root).unwrap();
    }
}
