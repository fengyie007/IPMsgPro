//! Unpackaged desktop toasts require a Start Menu shortcut with an AUMID.
//! https://learn.microsoft.com/windows/win32/shell/enable-desktop-toast-with-appusermodelid
use std::{
    ffi::OsStr,
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Storage::EnhancedStorage::PKEY_AppUserModel_ID,
        System::{
            Com::{
                CoCreateInstance, CoTaskMemFree, IPersistFile,
                StructuredStorage::{
                    PropVariantChangeType, PropVariantToString, PROPVARIANT, PVCHF_DEFAULT,
                },
                CLSCTX_INPROC_SERVER, STGM_READ,
            },
            Variant::VT_LPWSTR,
        },
        UI::Shell::{
            FOLDERID_Programs, IShellLinkW, PropertiesSystem::IPropertyStore, SHGetKnownFolderPath,
            SetCurrentProcessExplicitAppUserModelID, ShellLink, KF_FLAG_DEFAULT,
        },
    },
};

pub(super) const APP_ID: &str = "com.speedipmsg.rustpreview";
const SHORTCUT: &str = "SpeedIpMsg Rust Notifications.lnk";

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

pub(super) fn register() -> Result<(), String> {
    // The caller has initialized COM/WinRT on the notification worker.
    let programs = unsafe {
        let allocated = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None)
            .map_err(|e| format!("读取开始菜单目录失败：{e}"))?;
        let result = PathBuf::from(std::ffi::OsString::from_wide(allocated.as_wide()));
        CoTaskMemFree(Some(allocated.0.cast()));
        result
    };
    fs::create_dir_all(&programs).map_err(|e| format!("创建开始菜单目录失败：{e}"))?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    ensure_shortcut(&programs.join(SHORTCUT), &executable)?;
    let app_id = wide(OsStr::new(APP_ID));
    unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(app_id.as_ptr())) }
        .map_err(|e| format!("设置进程 AppUserModelID 失败：{e}"))
}

fn shell_link() -> Result<IShellLinkW, String> {
    unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("创建通知快捷方式对象失败：{e}"))
}

fn app_id(link: &IShellLinkW) -> Result<String, String> {
    let properties: IPropertyStore = link.cast().map_err(|e| e.to_string())?;
    let value = unsafe { properties.GetValue(&PKEY_AppUserModel_ID) }.map_err(|e| e.to_string())?;
    let mut buffer = [0u16; 256];
    unsafe { PropVariantToString(&value, &mut buffer) }.map_err(|e| e.to_string())?;
    Ok(String::from_utf16_lossy(
        &buffer[..buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len())],
    ))
}

fn set_app_id(link: &IShellLinkW, id: &str) -> Result<(), String> {
    let properties: IPropertyStore = link.cast().map_err(|e| e.to_string())?;
    // windows-rs owns and clears PROPVARIANT storage. Convert the owned BSTR
    // to the canonical VT_LPWSTR property type; never borrow a Rust buffer here.
    let text = PROPVARIANT::from(id);
    let mut value = PROPVARIANT::default();
    unsafe {
        PropVariantChangeType(&mut value, &text, PVCHF_DEFAULT, VT_LPWSTR)
            .map_err(|e| format!("转换快捷方式 AppUserModelID 失败：{e}"))?;
        properties
            .SetValue(&PKEY_AppUserModel_ID, &value)
            .and_then(|_| properties.Commit())
    }
    .map_err(|e| format!("写入快捷方式 AppUserModelID 失败：{e}"))
}

fn load(path: &Path) -> Result<IShellLinkW, String> {
    let link = shell_link()?;
    let persist: IPersistFile = link.cast().map_err(|e| e.to_string())?;
    let path = wide(path.as_os_str());
    unsafe { persist.Load(PCWSTR(path.as_ptr()), STGM_READ) }
        .map_err(|e| format!("读取现有通知快捷方式失败：{e}"))?;
    Ok(link)
}

fn target(link: &IShellLinkW) -> Result<PathBuf, String> {
    let mut buffer = vec![0u16; 32768];
    unsafe { link.GetPath(&mut buffer, std::ptr::null_mut(), 0) }.map_err(|e| e.to_string())?;
    let length = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
    Ok(std::ffi::OsString::from_wide(&buffer[..length]).into())
}

fn ensure_shortcut(path: &Path, executable: &Path) -> Result<(), String> {
    let exists = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            use std::os::windows::fs::MetadataExt;
            if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
                return Err("通知快捷方式位置被目录或链接占用".into());
            }
            let existing = load(path)?;
            if app_id(&existing)? != APP_ID {
                return Err("通知快捷方式名称已被其他应用占用，未覆盖".into());
            }
            if target(&existing)? == executable {
                return Ok(());
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.to_string()),
    };
    let link = shell_link()?;
    let exe = wide(executable.as_os_str());
    let description = wide(OsStr::new("迅秋 Rust 系统通知"));
    unsafe {
        link.SetPath(PCWSTR(exe.as_ptr()))
            .and_then(|_| link.SetDescription(PCWSTR(description.as_ptr())))
            .and_then(|_| link.SetIconLocation(PCWSTR(exe.as_ptr()), 0))
    }
    .map_err(|e| format!("设置通知快捷方式目标失败：{e}"))?;
    set_app_id(&link, APP_ID)?;
    // Save to an exclusively owned temporary file, then publish atomically.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!(
        "{}-{nonce}-{}.tmp.lnk",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let persist: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        let temp = wide(temporary.as_os_str());
        unsafe { persist.Save(PCWSTR(temp.as_ptr()), false) }
            .map_err(|e| format!("保存通知快捷方式失败：{e}"))?;
        let dest = wide(path.as_os_str());
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        if unsafe {
            MoveFileExW(
                temp.as_ptr(),
                dest.as_ptr(),
                MOVEFILE_WRITE_THROUGH | if exists { MOVEFILE_REPLACE_EXISTING } else { 0 },
            )
        } == 0
        {
            return Err(format!(
                "安装通知快捷方式失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    struct Apartment;
    impl Apartment {
        fn new() -> Self {
            unsafe {
                RoInitialize(RO_INIT_MULTITHREADED).unwrap();
            }
            Self
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }

    #[test]
    fn shortcut_has_own_identity_repairs_target_and_preserves_foreign_link() {
        let _apartment = Apartment::new();
        let root = std::env::temp_dir().join(format!(
            "ipmsg-toast-link-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let a = root.join("中文 程序.exe");
        let b = root.join("new version.exe");
        fs::write(&a, b"fixture").unwrap();
        fs::write(&b, b"fixture").unwrap();
        let path = root.join(SHORTCUT);
        ensure_shortcut(&path, &a).unwrap();
        assert_eq!(app_id(&load(&path).unwrap()).unwrap(), APP_ID);
        assert_eq!(target(&load(&path).unwrap()).unwrap(), a);
        let bytes = fs::read(&path).unwrap();
        ensure_shortcut(&path, &a).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "correct shortcut must remain unchanged"
        );
        ensure_shortcut(&path, &b).unwrap();
        assert_eq!(target(&load(&path).unwrap()).unwrap(), b);
        let other = shell_link().unwrap();
        let exe = wide(b.as_os_str());
        unsafe {
            other.SetPath(PCWSTR(exe.as_ptr())).unwrap();
        }
        set_app_id(&other, "com.other.application").unwrap();
        let persist: IPersistFile = other.cast().unwrap();
        let text = wide(path.as_os_str());
        unsafe {
            persist.Save(PCWSTR(text.as_ptr()), false).unwrap();
        }
        drop(persist);
        drop(other);
        let before = fs::read(&path).unwrap();
        assert!(ensure_shortcut(&path, &a).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            3,
            "no temporary shortcut leak"
        );
        assert!(root
            .canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap()));
        fs::remove_dir_all(root).unwrap();
    }
}
