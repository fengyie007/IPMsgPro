use super::protocol::{sanitize_name, MAX_FILE_SIZE};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_DEPTH: usize = 32;
pub fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
pub struct Entry {
    pub name: String,
    pub kind: u32,
    pub size: u64,
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}
pub struct Snapshot {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub size: u64,
}
impl Snapshot {
    pub fn capture(path: &Path) -> Result<Self, String> {
        let mut snapshot = Self {
            root: path.canonicalize().map_err(|e| e.to_string())?,
            entries: vec![],
            size: 0,
        };
        snapshot.walk(path, 0)?;
        Ok(snapshot)
    }
    fn walk(&mut self, path: &Path, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH || self.entries.len() >= MAX_ENTRIES {
            return Err("文件夹超过4096项或32层限制".into());
        }
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if is_link(&meta) {
            return Err("文件夹包含链接或重解析点，不能发送".into());
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("文件夹名称无效")?;
        let kind = if meta.is_dir() {
            2
        } else if meta.is_file() {
            1
        } else {
            return Err("文件夹包含特殊文件".into());
        };
        if kind == 2 && depth >= MAX_DEPTH {
            return Err("文件夹层级过多".into());
        }
        encode(name, meta.len(), kind)?;
        self.entries.push(Entry {
            name: sanitize_name(name),
            kind,
            size: if kind == 1 { meta.len() } else { 0 },
            path: path.to_owned(),
            modified: meta.modified().ok(),
        });
        if kind == 1 {
            self.size = self
                .size
                .checked_add(meta.len())
                .filter(|n| *n <= MAX_FILE_SIZE)
                .ok_or("文件夹总量超过8 GiB")?;
        } else {
            let mut children = fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .take(MAX_ENTRIES + 1)
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if children.len() > MAX_ENTRIES {
                return Err("文件夹项目过多".into());
            }
            children.sort();
            for child in children {
                self.walk(&child, depth + 1)?;
            }
            if self.entries.len() >= MAX_ENTRIES {
                return Err("文件夹项目过多".into());
            }
            self.entries.push(Entry {
                name: ".".into(),
                kind: 3,
                size: 0,
                path: path.to_owned(),
                modified: None,
            });
        }
        Ok(())
    }
}
pub fn encode(name: &str, size: u64, kind: u32) -> Result<Vec<u8>, String> {
    let text = format!("{}:{size:x}:{kind:x}:", name.replace(':', "::"));
    let (bytes, _, bad) = encoding_rs::GBK.encode(&text);
    if bad || bytes.len() + 5 > 8192 {
        return Err("目录项名称不能用GBK表示或过长".into());
    }
    let mut header = format!("{:04x}:", bytes.len() + 5).into_bytes();
    header.extend_from_slice(&bytes);
    Ok(header)
}
pub fn decode(bytes: &[u8]) -> Result<(String, u64, u32), String> {
    if bytes.len() < 5 || bytes[4] != b':' {
        return Err("无效目录头".into());
    }
    let expected = usize::from_str_radix(
        std::str::from_utf8(&bytes[..4]).map_err(|_| "无效目录长度")?,
        16,
    )
    .map_err(|_| "无效目录长度")?;
    if expected != bytes.len() || expected > 8192 {
        return Err("目录长度错误".into());
    }
    let text = crate::protocol::decode_text(&bytes[5..], 0)?;
    let mut chars = text.chars().peekable();
    let mut name = String::new();
    let mut ended = false;
    while let Some(c) = chars.next() {
        if c == ':' {
            if chars.peek() == Some(&':') {
                chars.next();
                name.push(':');
            } else {
                ended = true;
                break;
            }
        } else {
            name.push(c);
        }
    }
    if !ended || name.is_empty() {
        return Err("目录文件名为空".into());
    }
    let tail: String = chars.collect();
    let fields: Vec<_> = tail.trim_end_matches('\0').split(':').collect();
    if fields.len() < 2 {
        return Err("目录头不完整".into());
    }
    let size = u64::from_str_radix(fields[0], 16).map_err(|_| "无效目录文件长度")?;
    let kind = u32::from_str_radix(fields[1], 16).map_err(|_| "无效目录属性")? & 0xff;
    if !matches!(kind, 1 | 2 | 3) || size > MAX_FILE_SIZE || kind != 1 && size != 0 {
        return Err("不支持的目录项".into());
    }
    if kind != 3 && (name == "." || name == ".." || name.contains(['/', '\\'])) {
        return Err("目录项不能包含路径".into());
    }
    Ok((sanitize_name(&name), size, kind))
}
pub struct Staging {
    pub root: PathBuf,
    keep: bool,
}
impl Staging {
    pub fn create(root: &Path, id: u32) -> Result<Self, String> {
        for n in 0..1000 {
            let dir = root.join(format!(".ipmsg-dir-{id}-{n}.part"));
            match fs::create_dir(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        root: dir.canonicalize().map_err(|e| e.to_string())?,
                        keep: false,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("无法创建接收目录".into())
    }
    pub fn publish(&mut self, parent: &Path, name: &str) -> Result<PathBuf, String> {
        let name = sanitize_name(name);
        for n in 0..10000 {
            let dest = parent.join(if n == 0 {
                name.clone()
            } else {
                format!("{name} ({n})")
            });
            match super::storage::publish_new(&self.root, &dest) {
                Ok(()) => {
                    self.root = dest.clone();
                    return Ok(dest);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("同名目录过多".into())
    }
    pub fn keep(&mut self) {
        self.keep = true;
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep && self.root.canonicalize().ok().as_ref() == Some(&self.root) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_lengths_escaping_and_traversal() {
        let data = encode("中文:a.txt", 12, 1).unwrap();
        assert_eq!(decode(&data).unwrap(), ("中文_a.txt".into(), 12, 1));
        for name in ["../x", "/root", "a\\b", ".."] {
            assert!(decode(&encode(name, 0, 2).unwrap()).is_err());
        }
        assert!(decode(b"ffff:bad").is_err());
    }
}
