//! Restart-only relocation of this Rust instance, with a stable bootstrap locator.
mod layout;
use crate::runtime::Runtime;
pub use layout::default_directory;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::Manager;
pub fn native_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let prefix: Vec<u16> = "\\\\?\\".encode_utf16().collect();
        if wide.starts_with(&prefix) {
            let rest = &wide[prefix.len()..];
            let unc: Vec<u16> = "UNC\\".encode_utf16().collect();
            if rest.starts_with(&unc) {
                let mut result: Vec<u16> = "\\\\".encode_utf16().collect();
                result.extend_from_slice(&rest[4..]);
                return std::ffi::OsString::from_wide(&result).into();
            }
            return std::ffi::OsString::from_wide(rest).into();
        }
    }
    path.to_owned()
}
const APP: &str = "com.speedipmsg.rustpreview";
const MARKER: &str = ".speedipmsg-dataset.json";
fn token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    token: String,
    target: PathBuf,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    active: Option<PathBuf>,
    pending: Option<Choice>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    app: String,
    port: u16,
    token: String,
    complete: bool,
}
pub struct Storage {
    pub active: PathBuf,
    bootstrap: PathBuf,
    port: u16,
    selected: Mutex<Option<Choice>>,
    error: Mutex<Option<String>>,
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 65536 {
        return Err("目录定位文件过大".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn locator(root: &Path) -> Result<Locator, String> {
    let path = root.join("data-location.json");
    if path.exists() {
        read_json(&path)
    } else {
        Ok(Locator::default())
    }
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temp = path.with_extension(format!("tmp-{}", token()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::*;
            let a: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let b: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe {
                MoveFileExW(
                    a.as_ptr(),
                    b.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        #[cfg(not(windows))]
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn same(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}
fn inside(path: &Path, parent: &Path) -> bool {
    #[cfg(windows)]
    {
        let p = path.to_string_lossy().to_lowercase();
        let root = parent.to_string_lossy().to_lowercase();
        p == root || p.starts_with(&(root.trim_end_matches('\\').to_owned() + "\\"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(parent)
    }
}
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}
struct OwnedDataset {
    path: PathBuf,
    port: u16,
    token: String,
    keep: bool,
}
impl Drop for OwnedDataset {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Ok(marker) = read_json::<Marker>(&self.path.join(MARKER)) {
            if marker.app == APP
                && marker.port == self.port
                && marker.token == self.token
                && self.path.canonicalize().ok().as_ref() == Some(&self.path)
            {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }
}
fn copy_tree(source: &Path, target: &Path, depth: usize, count: &mut usize) -> Result<(), String> {
    *count += 1;
    if *count > 200000 || depth > 64 {
        return Err("数据目录层级或文件数量超过迁移上限".into());
    }
    let before = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if is_link(&before) {
        return Err("数据目录包含链接或重解析点，已停止迁移".into());
    }
    if before.is_dir() {
        fs::create_dir(target).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_tree(
                &entry.path(),
                &target.join(entry.file_name()),
                depth + 1,
                count,
            )?;
        }
    } else if before.is_file() {
        let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|e| e.to_string())?;
        let bytes = std::io::copy(&mut input, &mut out).map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
        let after = input.metadata().map_err(|e| e.to_string())?;
        if bytes != before.len()
            || after.len() != before.len()
            || after.modified().ok() != before.modified().ok()
        {
            return Err("复制过程中源数据发生变化".into());
        }
    } else {
        return Err("数据目录包含特殊文件".into());
    }
    Ok(())
}
fn create_dataset(target: &Path, port: u16, token: &str) -> Result<OwnedDataset, String> {
    let parent = target.parent().ok_or("无效目标目录")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if !same(&parent.canonicalize().map_err(|e| e.to_string())?, parent) {
        return Err("目标目录经过重解析点，已停止迁移".into());
    }
    fs::create_dir(target).map_err(|e| format!("目标已存在或无法创建，未覆盖任何数据：{e}"))?;
    let target = target.canonicalize().map_err(|e| e.to_string())?;
    if let Err(error) = atomic_json(
        &target.join(MARKER),
        &Marker {
            app: APP.into(),
            port,
            token: token.into(),
            complete: false,
        },
    ) {
        let _ = fs::remove_dir(&target); // Only an empty directory if its marker failed.
        return Err(error);
    }
    Ok(OwnedDataset {
        path: target,
        port,
        token: token.into(),
        keep: false,
    })
}
fn validate_marker(path: &Path, port: u16) -> Result<(), String> {
    let marker: Marker = read_json(&path.join(MARKER))?;
    if marker.app != APP || marker.port != port || !marker.complete {
        return Err("数据目录身份不符或迁移未完成，未加载其他实例数据".into());
    }
    Ok(())
}

enum LocationError {
    Unavailable(String),
    Invalid(String),
}

fn configured_directory(path: &Path, port: u16) -> Result<PathBuf, LocationError> {
    use LocationError::{Invalid, Unavailable};
    let path = path
        .canonicalize()
        .map_err(|e| Unavailable(e.to_string()))?;
    if !path.is_dir() {
        return Err(Unavailable("路径不是文件夹".into()));
    }
    fs::read_dir(&path).map_err(|e| Unavailable(e.to_string()))?;
    // A missing/unreadable marker cannot identify a usable directory. A readable
    // but incompatible marker remains an error, never a reason to mix instances.
    fs::File::open(path.join(MARKER)).map_err(|e| Unavailable(e.to_string()))?;
    validate_marker(&path, port).map_err(Invalid)?;
    for name in ["config.json", "messages.db"] {
        let file = path.join(name);
        if file.try_exists().map_err(|e| Unavailable(e.to_string()))? {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(file)
                .map_err(|e| Unavailable(e.to_string()))?;
        }
    }
    let probe = path.join(format!(".speedipmsg-write-{}.tmp", token()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| Unavailable(e.to_string()))?;
    let result = file
        .write_all(b"SpeedIPMsg directory probe")
        .and_then(|_| file.sync_all());
    drop(file);
    let cleanup = fs::remove_file(&probe);
    result
        .and(cleanup)
        .map_err(|e| Unavailable(e.to_string()))?;
    Ok(path)
}

fn fresh_default(bootstrap: &Path, port: u16, previous: &Locator) -> Result<PathBuf, String> {
    let token = token();
    let target = layout::unused_directory(bootstrap, "data")?;
    let mut fresh = create_dataset(&target, port, &token)?;
    // Never reuse a historical copy left in the bootstrap after relocation.
    ipmsg_core::config::ConfigStore::open(fresh.path.join("config.json"))?;
    let db =
        rusqlite::Connection::open(fresh.path.join("messages.db")).map_err(|e| e.to_string())?;
    db.pragma_update(None, "user_version", 0)
        .map_err(|e| e.to_string())?;
    drop(db);
    complete_dataset(&fresh)?;
    atomic_json(
        &bootstrap.join(format!("data-location-unavailable-{token}.json")),
        previous,
    )?;
    atomic_json(
        &bootstrap.join("data-location.json"),
        &Locator {
            active: Some(fresh.path.clone()),
            pending: None,
        },
    )?;
    fresh.keep = true;
    Ok(fresh.path.clone())
}

fn complete_dataset(owned: &OwnedDataset) -> Result<(), String> {
    atomic_json(
        &owned.path.join(MARKER),
        &Marker {
            app: APP.into(),
            port: owned.port,
            token: owned.token.clone(),
            complete: true,
        },
    )
}
fn migrate(source: &Path, choice: &Choice, port: u16) -> Result<OwnedDataset, String> {
    if !choice.target.is_absolute() || inside(&choice.target, source) {
        return Err("目标目录不能位于当前数据目录内".into());
    }
    if !source.join("messages.db").is_file() || !source.join("config.json").is_file() {
        return Err("当前数据不完整，未执行迁移".into());
    }
    let owned = create_dataset(&choice.target, port, &choice.token)?;
    let target = &owned.path;
    let mut count = 0;
    for name in [
        "config.json",
        "messages.db",
        "messages.db-wal",
        "messages.db-shm",
        "images",
    ] {
        let input = source.join(name);
        if input.exists() {
            copy_tree(&input, &target.join(name), 0, &mut count)?;
        }
    }
    let config: ipmsg_core::config::AppConfig = read_json(&target.join("config.json"))?;
    config.validate()?;
    {
        let db = rusqlite::Connection::open_with_flags(
            target.join("messages.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(|e| e.to_string())?;
        let result: String = db
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if result != "ok" {
            return Err("复制后的数据库校验失败".into());
        }
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
    }
    complete_dataset(&owned)?;
    Ok(owned)
}
impl Storage {
    pub fn open(bootstrap: PathBuf, port: u16) -> Result<Self, String> {
        fs::create_dir_all(&bootstrap).map_err(|e| e.to_string())?;
        let bootstrap = bootstrap.canonicalize().map_err(|e| e.to_string())?;
        let mut located = locator(&bootstrap)?;
        let configured = located.active.clone().unwrap_or_else(|| bootstrap.clone());
        let mut error = None;
        let mut active = if same(&configured, &bootstrap) {
            bootstrap.clone()
        } else {
            match configured_directory(&configured, port) {
                Ok(path) => path,
                Err(LocationError::Invalid(message)) => return Err(message),
                Err(LocationError::Unavailable(message)) => {
                    let path = fresh_default(&bootstrap, port, &located)
                        .map_err(|e| format!("原数据目录不可用，创建默认配置失败：{e}"))?;
                    error = Some(format!("原数据目录不可用（{}：{message}），已在默认位置创建新配置；原定位记录已备份，原数据未作修改。", native_path(&configured).display()));
                    located = Locator {
                        active: Some(path.clone()),
                        pending: None,
                    };
                    path
                }
            }
        };
        if !same(&active, &bootstrap) || active.join(MARKER).exists() {
            validate_marker(&active, port)?;
        }
        if let Some(choice) = located.pending.take() {
            if !same(&choice.target, &active) {
                match migrate(&active, &choice, port) {
                    Ok(mut copied) => {
                        let next = Locator {
                            active: Some(copied.path.clone()),
                            pending: None,
                        };
                        match atomic_json(&bootstrap.join("data-location.json"), &next) {
                            Ok(()) => {
                                active = copied.path.clone();
                                copied.keep = true;
                                located = next;
                            }
                            Err(e) => error = Some(e),
                        }
                    }
                    Err(e) => error = Some(e),
                }
            }
            if error.is_some() {
                located.active = Some(active.clone());
                located.pending = None;
                let _ = atomic_json(&bootstrap.join("data-location.json"), &located);
            }
        }
        Ok(Self {
            active,
            bootstrap,
            port,
            selected: Mutex::new(None),
            error: Mutex::new(error),
        })
    }
    fn is_default_location(&self) -> bool {
        same(&self.active, &self.bootstrap)
            || self
                .active
                .parent()
                .is_some_and(|parent| same(parent, &self.bootstrap))
    }
    pub fn info(&self) -> Result<Value, String> {
        let location = locator(&self.bootstrap)?;
        Ok(
            json!({"success":true,"active":native_path(&self.active),"pending":location.pending.map(|c|native_path(&c.target)),"defaultDirectory":native_path(&self.bootstrap),
        "isDefault":self.is_default_location(),"error":*self.error.lock().unwrap_or_else(|e|e.into_inner())}),
        )
    }
    fn prepare(&self, parent: PathBuf, reset: bool) -> Result<Value, String> {
        let parent = parent.canonicalize().map_err(|e| e.to_string())?;
        if !parent.is_dir() {
            return Err("请选择文件夹".into());
        }
        let token = token();
        let stem = if reset {
            "data".into()
        } else {
            layout::directory_name(self.port)
        };
        let target = layout::unused_directory(&parent, &stem)?;
        if inside(&target, &self.active) {
            return Err("目标不能位于当前数据目录内".into());
        }
        let probe = parent.join(format!(".speedipmsg-write-{token}.tmp"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|e| format!("目标目录不可写：{e}"))?;
        let result = file.write_all(b"SpeedIPMsg directory probe");
        drop(file);
        let _ = fs::remove_file(probe);
        result.map_err(|e| e.to_string())?;
        let choice = Choice {
            token: token.clone(),
            target: target.clone(),
        };
        *self.selected.lock().unwrap_or_else(|e| e.into_inner()) = Some(choice);
        Ok(
            json!({"success":true,"selectionId":token,"source":native_path(&self.active),"target":native_path(&target)}),
        )
    }
    pub fn apply(&self, selection: &str) -> Result<Value, String> {
        let mut chosen = self.selected.lock().unwrap_or_else(|e| e.into_inner());
        let choice = chosen
            .as_ref()
            .filter(|c| c.token == selection)
            .ok_or("目录选择已失效")?
            .clone();
        atomic_json(
            &self.bootstrap.join("data-location.json"),
            &Locator {
                active: Some(self.active.clone()),
                pending: Some(choice),
            },
        )?;
        *chosen = None;
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.info()
    }
    pub fn cancel_pending(&self) -> Result<Value, String> {
        atomic_json(
            &self.bootstrap.join("data-location.json"),
            &Locator {
                active: Some(self.active.clone()),
                pending: None,
            },
        )?;
        self.info()
    }
}
pub async fn select(
    window: &tauri::WebviewWindow,
    state: Arc<Runtime>,
    reset: bool,
) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    if state.image_selecting.swap(true, Ordering::AcqRel) {
        return Err("请先完成已有选择或截图".into());
    }
    let _guard = crate::image::SelectionGuard(state.clone());
    let parent = if reset {
        state.storage.bootstrap.clone()
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel();
        window
            .app_handle()
            .dialog()
            .file()
            .set_parent(window)
            .set_title("选择数据存储位置")
            .pick_folder(move |p| {
                let _ = tx.send(p);
            });
        let Some(path) = rx.await.map_err(|_| "目录选择中断")? else {
            return Ok(json!({"success":true,"cancelled":true}));
        };
        path.into_path().map_err(|_| "请选择本地文件夹")?
    };
    if !state.accepting.load(Ordering::Acquire) {
        return Err("程序正在退出".into());
    }
    tokio::task::spawn_blocking(move || state.storage.prepare(parent, reset))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("ipmsg-storage-test-{}", token()));
        fs::create_dir(&path).unwrap();
        path
    }
    pub(super) fn dataset(path: &Path) {
        ipmsg_core::config::ConfigStore::open(path.join("config.json")).unwrap();
        let db = rusqlite::Connection::open(path.join("messages.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE sample(value TEXT); INSERT INTO sample VALUES ('中文历史');",
        )
        .unwrap();
        fs::create_dir(path.join("images")).unwrap();
        fs::write(path.join("images/a.png"), b"owned image").unwrap();
    }

    fn configured_fixture() -> (PathBuf, PathBuf, Storage) {
        let root = fixture().canonicalize().unwrap();
        let bootstrap = root.join("SpeedIpMsg");
        let parent = root.join("custom");
        fs::create_dir(&parent).unwrap();
        let initial = Storage::open(bootstrap.clone(), 2425).unwrap();
        dataset(&initial.active);
        fs::write(
            initial.active.join("config.json"),
            br#"{"nickname":"old-custom"}"#,
        )
        .unwrap();
        let choice = initial.prepare(parent, false).unwrap();
        initial
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        let configured = Storage::open(bootstrap.clone(), 2425).unwrap();
        (root, bootstrap, configured)
    }

    #[test]
    fn unavailable_configured_directory_creates_fresh_default_and_preserves_recovery_record() {
        let (root, bootstrap, configured) = configured_fixture();
        let original = configured.active.clone();
        let disconnected = root.join("disconnected");
        assert!(inside(&original, &root) && inside(&disconnected, &root));
        let pending = root.join("pending");
        atomic_json(
            &bootstrap.join("data-location.json"),
            &Locator {
                active: Some(original.clone()),
                pending: Some(Choice {
                    token: token(),
                    target: pending.clone(),
                }),
            },
        )
        .unwrap();
        fs::rename(&original, &disconnected).unwrap();
        let fallback = Storage::open(bootstrap.clone(), 2425).unwrap();
        assert!(fallback.is_default_location());
        assert_ne!(fallback.active, bootstrap);
        assert_eq!(fallback.active.parent(), Some(bootstrap.as_path()));
        assert!(fallback.info().unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("已在默认位置创建新配置"));
        assert!(fallback.info().unwrap()["pending"].is_null());
        assert!(!pending.exists());
        assert_eq!(
            ipmsg_core::config::ConfigStore::open(fallback.active.join("config.json"))
                .unwrap()
                .get(),
            ipmsg_core::config::AppConfig::default()
        );
        let db = rusqlite::Connection::open(fallback.active.join("messages.db")).unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='sample'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        drop(db);
        assert_eq!(
            fs::read(bootstrap.join("config.json")).unwrap(),
            br#"{"nickname":"old-custom"}"#
        );
        let backups: Vec<_> = fs::read_dir(&bootstrap)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("data-location-unavailable-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        let saved: Locator = read_json(&backups[0]).unwrap();
        assert_eq!(saved.active.as_ref(), Some(&original));
        assert!(saved.pending.is_some());
        fs::rename(&disconnected, &original).unwrap();
        let reopened = Storage::open(bootstrap, 2425).unwrap();
        assert_eq!(reopened.active, fallback.active);
        assert!(reopened.info().unwrap()["error"].is_null());
        assert_eq!(
            fs::read(original.join("config.json")).unwrap(),
            br#"{"nickname":"old-custom"}"#
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_path_replaced_by_a_file_falls_back_without_overwriting_it() {
        let (root, bootstrap, _) = configured_fixture();
        let blocked = root.join("not-a-directory");
        fs::write(&blocked, b"unrelated file").unwrap();
        atomic_json(
            &bootstrap.join("data-location.json"),
            &Locator {
                active: Some(blocked.clone()),
                pending: None,
            },
        )
        .unwrap();
        let fallback = Storage::open(bootstrap, 2425).unwrap();
        assert!(fallback.is_default_location());
        assert_eq!(fs::read(blocked).unwrap(), b"unrelated file");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incompatible_directory_identity_is_not_silently_reset() {
        let (root, bootstrap, configured) = configured_fixture();
        let before = fs::read(bootstrap.join("data-location.json")).unwrap();
        let mut marker: Marker = read_json(&configured.active.join(MARKER)).unwrap();
        marker.port = 2427;
        atomic_json(&configured.active.join(MARKER), &marker).unwrap();
        assert!(Storage::open(bootstrap.clone(), 2425).is_err());
        assert_eq!(
            fs::read(bootstrap.join("data-location.json")).unwrap(),
            before
        );
        assert!(!bootstrap.join("data").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn readonly_configured_data_uses_fresh_default_without_changing_the_source() {
        let (root, bootstrap, configured) = configured_fixture();
        let config = configured.active.join("config.json");
        let before = fs::read(&config).unwrap();
        let permissions = fs::metadata(&config).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&config, readonly).unwrap();
        let result = Storage::open(bootstrap, 2425);
        fs::set_permissions(&config, permissions).unwrap();
        let fallback = result.unwrap();
        assert!(fallback.is_default_location());
        assert_ne!(fallback.active, configured.active);
        assert_eq!(fs::read(config).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn failed_fallback_publication_preserves_locator_and_reclaims_only_fresh_files() {
        let (root, bootstrap, configured) = configured_fixture();
        let disconnected = root.join("disconnected");
        assert!(inside(&configured.active, &root) && inside(&disconnected, &root));
        fs::rename(&configured.active, &disconnected).unwrap();
        let location = bootstrap.join("data-location.json");
        let before = fs::read(&location).unwrap();
        let permissions = fs::metadata(&location).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&location, readonly).unwrap();
        let result = Storage::open(bootstrap.clone(), 2425);
        fs::set_permissions(&location, permissions).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(location).unwrap(), before);
        assert!(!bootstrap.join("data").exists());
        assert!(disconnected.join("messages.db").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relocation_is_restart_only_preserves_source_and_roundtrips_default() {
        let root = fixture();
        let bootstrap = root.join("bootstrap");
        let custom = root.join("自定义");
        fs::create_dir(&custom).unwrap();
        let storage = Storage::open(bootstrap.clone(), 2427).unwrap();
        dataset(&storage.active);
        let choice = storage.prepare(custom.clone(), false).unwrap();
        let target = PathBuf::from(choice["target"].as_str().unwrap());
        storage
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        assert!(!target.exists());
        assert!(same(&storage.active, &bootstrap.canonicalize().unwrap()));
        let storage = Storage::open(bootstrap.clone(), 2427).unwrap();
        assert!(same(&storage.active, &target.canonicalize().unwrap()));
        assert!(bootstrap.join("messages.db").exists());
        assert_eq!(
            fs::read(storage.active.join("images/a.png")).unwrap(),
            b"owned image"
        );
        let db = rusqlite::Connection::open(storage.active.join("messages.db")).unwrap();
        let value: String = db
            .query_row("SELECT value FROM sample", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "中文历史");
        drop(db);
        let choice = storage
            .prepare(bootstrap.canonicalize().unwrap(), true)
            .unwrap();
        storage
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        let reverted = Storage::open(bootstrap, 2427).unwrap();
        assert_eq!(reverted.info().unwrap()["isDefault"], true);
        assert!(reverted
            .prepare(reverted.active.join("images"), false)
            .is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn occupied_target_falls_back_without_overwriting_and_cancellation_is_safe() {
        let root = fixture();
        let bootstrap = root.join("bootstrap");
        let parent = root.join("parent");
        fs::create_dir(&parent).unwrap();
        let storage = Storage::open(bootstrap.clone(), 2427).unwrap();
        dataset(&storage.active);
        let choice = storage.prepare(parent.clone(), false).unwrap();
        storage
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        let target = PathBuf::from(choice["target"].as_str().unwrap());
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), b"keep").unwrap();
        let storage = Storage::open(bootstrap.clone(), 2427).unwrap();
        assert!(same(&storage.active, &bootstrap.canonicalize().unwrap()));
        assert!(storage.info().unwrap()["error"].is_string());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
        let choice = storage.prepare(parent, false).unwrap();
        storage
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        storage.cancel_pending().unwrap();
        assert!(Storage::open(bootstrap, 2427).unwrap().info().unwrap()["pending"].is_null());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migration_copies_committed_wal_and_rejects_wrong_instance() {
        let root = fixture();
        let bootstrap = root.join("bootstrap");
        let parent = root.join("target");
        fs::create_dir(&parent).unwrap();
        let storage = Storage::open(bootstrap.clone(), 2427).unwrap();
        dataset(&storage.active);
        let source = rusqlite::Connection::open(storage.active.join("messages.db")).unwrap();
        source.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO sample VALUES ('WAL消息');").unwrap();
        assert!(
            storage
                .active
                .join("messages.db-wal")
                .metadata()
                .unwrap()
                .len()
                > 0
        );
        let choice = storage.prepare(parent, false).unwrap();
        storage
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        // Keep the source handle idle to model committed WAL left after a crash.
        let migrated = Storage::open(bootstrap.clone(), 2427).unwrap();
        assert!(migrated.info().unwrap()["error"].is_null());
        let copied = rusqlite::Connection::open(migrated.active.join("messages.db")).unwrap();
        let count: i64 = copied
            .query_row(
                "SELECT COUNT(*) FROM sample WHERE value='WAL消息'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert!(bootstrap.join("data-location.json").is_file());
        assert!(Storage::open(bootstrap, 2428).is_err());
        drop(copied);
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_copy_keeps_source_and_removes_only_owned_destination() {
        let root = fixture();
        let bootstrap = root.join("bootstrap");
        let parent = root.join("target");
        fs::create_dir(&parent).unwrap();
        let storage = Storage::open(bootstrap.clone(), 2427).unwrap();
        dataset(&storage.active);
        fs::write(storage.active.join("messages.db"), b"corrupted database").unwrap();
        let choice = storage.prepare(parent, false).unwrap();
        storage
            .apply(choice["selectionId"].as_str().unwrap())
            .unwrap();
        let destination = PathBuf::from(choice["target"].as_str().unwrap());
        let reopened = Storage::open(bootstrap.clone(), 2427).unwrap();
        assert!(same(&reopened.active, &bootstrap.canonicalize().unwrap()));
        assert!(reopened.info().unwrap()["error"].is_string());
        assert!(reopened.info().unwrap()["pending"].is_null());
        assert!(!destination.exists());
        assert_eq!(
            fs::read(reopened.active.join("messages.db")).unwrap(),
            b"corrupted database"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
