use super::protocol::sanitize_name;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Own only a file created exclusively by this receive; never delete peer paths.
pub struct PartialFile {
    path: PathBuf,
    keep: bool,
}
impl PartialFile {
    pub fn create(root: &Path, id: u32) -> Result<(Self, std::fs::File), String> {
        for sequence in 0..1000 {
            let path = root.join(format!(".ipmsg-{id}-{sequence}.part"));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok((Self { path, keep: false }, file)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("无法分配接收临时文件".into())
    }
    pub fn publish(&mut self, root: &Path, name: &str) -> Result<PathBuf, String> {
        let name = sanitize_name(name);
        let name_path = Path::new(&name);
        let stem = name_path.file_stem().unwrap_or_default().to_string_lossy();
        let ext = name_path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for n in 0..10000 {
            let dest = root.join(if n == 0 {
                name.clone()
            } else {
                format!("{stem} ({n}){ext}")
            });
            match publish_new(&self.path, &dest) {
                Ok(()) => {
                    self.path = dest.clone();
                    return Ok(dest);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("无法安全保存接收文件：{e}")),
            }
        }
        Err("同名文件过多".into())
    }
    pub fn keep(&mut self) {
        self.keep = true;
    }
}
#[cfg(windows)]
fn publish_new(source: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let dest: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
    // Flags=0 deliberately excludes MOVEFILE_REPLACE_EXISTING; works on FAT too.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(source.as_ptr(), dest.as_ptr(), 0)
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn publish_new(source: &Path, dest: &Path) -> std::io::Result<()> {
    fs::hard_link(source, dest)?;
    if let Err(e) = fs::remove_file(source) {
        let _ = fs::remove_file(dest);
        return Err(e);
    }
    Ok(())
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}
