use super::*;
use crate::platform::DEFAULT_PORT;

pub(super) fn directory_name(port: u16) -> String {
    if port == DEFAULT_PORT {
        "SpeedIpMsg".into()
    } else {
        format!("SpeedIpMsg-{port}")
    }
}

pub fn default_directory(local_data: &Path, port: u16) -> PathBuf {
    local_data.join(directory_name(port))
}

pub(super) fn unused_directory(parent: &Path, stem: &str) -> Result<PathBuf, String> {
    for n in 1..10000 {
        let target = parent.join(if n == 1 {
            stem.to_owned()
        } else {
            format!("{stem}-{n}")
        });
        match fs::symlink_metadata(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(target),
            Err(e) => return Err(e.to_string()),
            Ok(_) => {}
        }
    }
    Err("同名数据目录过多，请选择其他位置".into())
}

fn publish_new(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        // No REPLACE_EXISTING: an unrelated destination must never be replaced.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    #[cfg(not(windows))]
    {
        if target.exists() {
            return Err("目标目录已存在".into());
        }
        fs::rename(source, target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Storage {
    /// Upgrade only this application's former per-port directory. Runtime has
    /// exclusively bound the UDP port. C++ history is never read here.
    pub fn open_compatible(
        bootstrap: PathBuf,
        legacy_root: PathBuf,
        port: u16,
    ) -> Result<Self, String> {
        let parent = bootstrap.parent().ok_or("无效默认数据目录")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let parent = parent.canonicalize().map_err(|e| e.to_string())?;
        let bootstrap = parent.join(bootstrap.file_name().ok_or("无效默认数据目录")?);
        match fs::symlink_metadata(&bootstrap) {
            Ok(metadata) => {
                if is_link(&metadata) || !metadata.is_dir() {
                    return Err("默认数据目录被文件或链接占用".into());
                }
                if bootstrap.join(MARKER).exists() {
                    // Existing new data is authoritative, even if damaged: do
                    // not silently revert to an older backup on any error.
                    validate_marker(&bootstrap, port)?;
                    return Self::open(bootstrap, port);
                }
                if fs::read_dir(&bootstrap)
                    .map_err(|e| e.to_string())?
                    .next()
                    .is_some()
                {
                    return Err("默认数据目录已被占用，未覆盖已有文件".into());
                }
                fs::remove_dir(&bootstrap).map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let mut legacy_present = false;
        for name in ["data-location.json", "config.json", "messages.db"] {
            legacy_present |= legacy_root
                .join(name)
                .try_exists()
                .map_err(|e| e.to_string())?;
        }
        let legacy = if legacy_present {
            Some(Self::open(legacy_root, port)?)
        } else {
            None
        };
        let new_token = token();
        let stage = parent.join(format!(".speedipmsg-upgrade-{new_token}"));
        let result = (|| {
            let mut copied = if let Some(old) = &legacy {
                if old.is_default_location() {
                    migrate(
                        &old.active,
                        &Choice {
                            token: new_token.clone(),
                            target: stage.clone(),
                        },
                        port,
                    )?
                } else {
                    create_dataset(&stage, port, &new_token)?
                }
            } else {
                create_dataset(&stage, port, &new_token)?
            };
            // Retain an explicitly selected custom location; only relocate the
            // bootstrap locator rather than moving that data back to this disk.
            let active = legacy
                .as_ref()
                .filter(|old| !old.is_default_location())
                .map(|old| old.active.clone());
            atomic_json(
                &copied.path.join("data-location.json"),
                &Locator {
                    active,
                    pending: None,
                },
            )?;
            complete_dataset(&copied)?;
            publish_new(&copied.path, &bootstrap)?;
            copied.keep = true;
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            if let Some(old) = legacy {
                *old.error.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(format!("简化默认目录失败，继续使用原目录：{error}"));
                return Ok(old);
            }
            return Err(error);
        }
        let upgraded = Self::open(bootstrap, port)?;
        if let Some(old) = legacy {
            *upgraded.error.lock().unwrap_or_else(|e| e.into_inner()) =
                old.error.into_inner().unwrap_or_else(|e| e.into_inner());
        }
        Ok(upgraded)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{dataset, fixture};
    use super::*;

    #[test]
    fn flat_default_upgrades_once_and_keeps_the_original_backup() {
        let root = fixture();
        let legacy = root.join("com.speedipmsg.rustpreview/port-2427");
        let old = Storage::open(legacy.clone(), DEFAULT_PORT).unwrap();
        dataset(&old.active);
        let bootstrap = default_directory(&root, DEFAULT_PORT);
        assert_eq!(bootstrap, root.join("SpeedIpMsg"));
        assert_eq!(default_directory(&root, 2428), root.join("SpeedIpMsg-2428"));
        let upgraded =
            Storage::open_compatible(bootstrap.clone(), legacy.clone(), DEFAULT_PORT).unwrap();
        assert!(same(&upgraded.active, &bootstrap.canonicalize().unwrap()));
        assert!(upgraded.info().unwrap()["error"].is_null());
        assert_eq!(
            fs::read(upgraded.active.join("images/a.png")).unwrap(),
            b"owned image"
        );
        assert!(legacy.join("messages.db").is_file());
        fs::write(
            legacy.join("images/a.png"),
            b"older application changed its backup",
        )
        .unwrap();
        let reopened = Storage::open_compatible(bootstrap, legacy, DEFAULT_PORT).unwrap();
        assert_eq!(
            fs::read(reopened.active.join("images/a.png")).unwrap(),
            b"owned image"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_location_and_pending_selection_survive_the_upgrade() {
        let root = fixture();
        let legacy = root.join("legacy/port-2427");
        let custom = root.join("自选位置");
        fs::create_dir(&custom).unwrap();
        let old = Storage::open(legacy.clone(), DEFAULT_PORT).unwrap();
        dataset(&old.active);
        let choice = old.prepare(custom.clone(), false).unwrap();
        assert_eq!(
            PathBuf::from(choice["target"].as_str().unwrap()),
            native_path(&custom.canonicalize().unwrap().join("SpeedIpMsg"))
        );
        old.apply(choice["selectionId"].as_str().unwrap()).unwrap();
        let bootstrap = default_directory(&root, DEFAULT_PORT);
        let upgraded = Storage::open_compatible(bootstrap.clone(), legacy, DEFAULT_PORT).unwrap();
        assert!(same(
            &upgraded.active,
            &PathBuf::from(choice["target"].as_str().unwrap())
                .canonicalize()
                .unwrap()
        ));
        assert!(bootstrap.join("data-location.json").is_file());
        assert!(!bootstrap.join("messages.db").exists());
        assert_eq!(upgraded.info().unwrap()["isDefault"], false);
        let second = upgraded.prepare(custom.clone(), false).unwrap();
        assert_eq!(
            PathBuf::from(second["target"].as_str().unwrap()),
            native_path(&custom.canonicalize().unwrap().join("SpeedIpMsg-2"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_upgrade_preserves_source_without_creating_empty_history() {
        let root = fixture();
        let legacy = root.join("legacy");
        let old = Storage::open(legacy.clone(), DEFAULT_PORT).unwrap();
        dataset(&old.active);
        fs::write(old.active.join("messages.db"), b"broken database").unwrap();
        let bootstrap = default_directory(&root, DEFAULT_PORT);
        let storage =
            Storage::open_compatible(bootstrap.clone(), legacy.clone(), DEFAULT_PORT).unwrap();
        assert!(same(&storage.active, &legacy.canonicalize().unwrap()));
        assert!(storage.info().unwrap()["error"].is_string());
        assert!(!bootstrap.exists());
        assert_eq!(
            fs::read(storage.active.join("messages.db")).unwrap(),
            b"broken database"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_install_and_existing_destinations_are_distinguished() {
        let root = fixture();
        let legacy = root.join("absent");
        let bootstrap = default_directory(&root, DEFAULT_PORT);
        let fresh =
            Storage::open_compatible(bootstrap.clone(), legacy.clone(), DEFAULT_PORT).unwrap();
        assert!(same(&fresh.active, &bootstrap.canonicalize().unwrap()));
        assert!(!legacy.exists());
        assert!(Storage::open_compatible(bootstrap.clone(), legacy.clone(), 2428).is_err());
        let mut marker: Marker = read_json(&bootstrap.join(MARKER)).unwrap();
        marker.complete = false;
        atomic_json(&bootstrap.join(MARKER), &marker).unwrap();
        assert!(Storage::open_compatible(bootstrap, legacy.clone(), DEFAULT_PORT).is_err());
        let occupied = root.join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("keep.txt"), b"existing user file").unwrap();
        assert!(Storage::open_compatible(occupied.clone(), legacy, DEFAULT_PORT).is_err());
        assert_eq!(
            fs::read(occupied.join("keep.txt")).unwrap(),
            b"existing user file"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_previously_restored_default_is_flattened_using_its_active_data() {
        let root = fixture();
        let legacy = root.join("legacy");
        let old = Storage::open(legacy.clone(), DEFAULT_PORT).unwrap();
        dataset(&old.active);
        let mut restored =
            create_dataset(&old.active.join("data-previous"), DEFAULT_PORT, &token()).unwrap();
        dataset(&restored.path);
        fs::write(restored.path.join("images/a.png"), b"latest restored data").unwrap();
        complete_dataset(&restored).unwrap();
        atomic_json(
            &old.active.join("data-location.json"),
            &Locator {
                active: Some(restored.path.clone()),
                pending: None,
            },
        )
        .unwrap();
        restored.keep = true;
        let bootstrap = default_directory(&root, DEFAULT_PORT);
        let upgraded = Storage::open_compatible(bootstrap.clone(), legacy, DEFAULT_PORT).unwrap();
        assert!(same(&upgraded.active, &bootstrap.canonicalize().unwrap()));
        assert_eq!(
            fs::read(upgraded.active.join("images/a.png")).unwrap(),
            b"latest restored data"
        );
        assert!(restored.path.join("messages.db").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
