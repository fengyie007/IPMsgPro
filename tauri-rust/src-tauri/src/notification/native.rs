use super::Player;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const AUDIO: &[u8] = include_bytes!("../../resources/notification.mp3");
struct SoundFile {
    path: PathBuf,
}
impl SoundFile {
    fn create(root: &Path) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root).map_err(|e| format!("无法创建提示音缓存：{e}"))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = root.join(format!(
            "notification-{}-{nonce}-{}.mp3",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let owned = Self { path };
        let result = file.write_all(AUDIO).and_then(|_| file.flush());
        drop(file);
        result.map_err(|e| e.to_string())?;
        Ok(owned)
    }
}
impl Drop for SoundFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
pub(super) struct NativePlayer {
    root: PathBuf,
    sound: Option<SoundFile>,
    opened: bool,
}
impl NativePlayer {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            sound: None,
            opened: false,
        }
    }
}

#[cfg(windows)]
fn command(wide: &[u16], output: &mut [u16]) -> Result<(), String> {
    use windows_sys::Win32::Media::Multimedia::{mciGetErrorStringW, mciSendStringW};
    let code = unsafe {
        mciSendStringW(
            wide.as_ptr(),
            if output.is_empty() {
                std::ptr::null_mut()
            } else {
                output.as_mut_ptr()
            },
            output.len() as u32,
            std::ptr::null_mut(),
        )
    };
    if code == 0 {
        return Ok(());
    }
    let mut text = [0u16; 256];
    unsafe {
        mciGetErrorStringW(code, text.as_mut_ptr(), text.len() as u32);
    }
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Err(format!(
        "提示音设备错误 {code}：{}",
        String::from_utf16_lossy(&text[..end])
    ))
}
#[cfg(windows)]
fn instruction(text: &str, output: &mut [u16]) -> Result<(), String> {
    command(
        &text.encode_utf16().chain(Some(0)).collect::<Vec<_>>(),
        output,
    )
}

impl Player for NativePlayer {
    fn play(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Duration, String> {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            if cancelled() {
                return Err("提示音请求已取消".into());
            }
            if self.sound.is_none() {
                self.sound = Some(SoundFile::create(&self.root)?);
            }
            let path = &self.sound.as_ref().ok_or("提示音缓存不可用")?.path;
            let open = |typed: bool| {
                let mut wide: Vec<u16> = "open \"".encode_utf16().collect();
                wide.extend(path.as_os_str().encode_wide());
                wide.extend(
                    if typed {
                        "\" type mpegvideo alias speedipmsg_notify"
                    } else {
                        "\" alias speedipmsg_notify"
                    }
                    .encode_utf16(),
                );
                wide.push(0);
                command(&wide, &mut [])
            };
            if cancelled() {
                return Err("提示音请求已取消".into());
            }
            open(true).or_else(|_| open(false))?;
            self.opened = true;
            instruction("set speedipmsg_notify time format milliseconds", &mut [])?;
            let mut length = [0u16; 32];
            instruction("status speedipmsg_notify length", &mut length)?;
            let end = length.iter().position(|&c| c == 0).unwrap_or(length.len());
            let length = String::from_utf16_lossy(&length[..end])
                .trim()
                .parse::<u64>()
                .map_err(|_| "无法读取提示音时长")?;
            if length == 0 || length > 15_000 {
                return Err("提示音时长无效".into());
            }
            if cancelled() {
                return Err("提示音请求已取消".into());
            }
            instruction("play speedipmsg_notify from 0", &mut [])?;
            Ok(Duration::from_millis(length + 100))
        }
        #[cfg(not(windows))]
        {
            let _ = cancelled;
            Err("当前平台暂不支持提示音".into())
        }
    }
    fn stop(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        if self.opened {
            let _ = instruction("stop speedipmsg_notify", &mut []);
            instruction("close speedipmsg_notify", &mut [])?;
            self.opened = false;
        }
        Ok(())
    }
}
impl Drop for NativePlayer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_sound_uses_owned_unicode_paths_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!(
            "ipmsg-sound-test-{}-{}-中文",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let sentinel = root.join("notification.mp3");
        fs::write(&sentinel, b"user file").unwrap();
        let one = SoundFile::create(&root).unwrap();
        let first = one.path.clone();
        let two = SoundFile::create(&root).unwrap();
        let second = two.path.clone();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), AUDIO);
        assert_eq!(fs::read(&sentinel).unwrap(), b"user file");
        drop(one);
        drop(two);
        assert!(!first.exists());
        assert!(!second.exists());
        fs::remove_file(sentinel).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
