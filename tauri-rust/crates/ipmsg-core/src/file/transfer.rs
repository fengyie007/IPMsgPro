use super::{
    protocol::{self, MAX_FILE_SIZE},
    storage::PartialFile,
    FileMetadata,
};
use crate::{
    database::{Database, Record},
    network::{unix_seconds, Event},
    protocol::*,
    User,
};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::HashMap,
    fs::File,
    future::Future,
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, watch, Semaphore},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub selection_id: String,
    pub file_name: String,
    pub file_size: u64,
}
struct Source {
    file: File,
    modified: Option<SystemTime>,
    selection: Selection,
    created: Instant,
}
struct State {
    metadata: FileMetadata,
    acknowledged: bool,
    attempts: u8,
    retry: Instant,
}
struct Transfer {
    id: String,
    peer: User,
    packet: u32,
    file_id: u32,
    source: Option<Source>,
    wire: Vec<u8>,
    state: Mutex<State>,
    cancel: watch::Sender<bool>,
    busy: AtomicBool,
    created: Instant,
}
impl Transfer {
    fn metadata(&self) -> FileMetadata {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .metadata
            .clone()
    }
}
struct Busy(Arc<Transfer>);
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

pub struct FileTransfers {
    socket: Arc<UdpSocket>,
    local: User,
    version: String,
    packet: Arc<AtomicU32>,
    db: Database,
    events: mpsc::Sender<Event>,
    root: PathBuf,
    selections: Mutex<HashMap<String, Source>>,
    transfers: Mutex<HashMap<String, Arc<Transfer>>>,
    gate: tokio::sync::Mutex<()>,
    downloads: mpsc::Sender<Arc<Transfer>>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
    active: Arc<Semaphore>,
    stopped: AtomicBool,
}
impl FileTransfers {
    pub async fn start(
        socket: Arc<UdpSocket>,
        local: User,
        version: String,
        packet: Arc<AtomicU32>,
        db: Database,
        events: mpsc::Sender<Event>,
        root: PathBuf,
    ) -> Result<Arc<Self>, String> {
        let listener = TcpListener::bind(socket.local_addr().map_err(|e| e.to_string())?)
            .await
            .map_err(|e| format!("无法绑定文件传输TCP端口{}：{e}", local.port))?;
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| e.to_string())?;
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|e| e.to_string())?;
        let (downloads, receiver) = mpsc::channel(4);
        let (stop, _) = watch::channel(false);
        let manager = Arc::new(Self {
            socket,
            local,
            version,
            packet,
            db,
            events,
            root,
            selections: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            gate: tokio::sync::Mutex::new(()),
            downloads,
            stop,
            task: Mutex::new(None),
            active: Arc::new(Semaphore::new(4)),
            stopped: AtomicBool::new(false),
        });
        let worker = manager.clone();
        *manager.task.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(tokio::spawn(
                async move { worker.run(listener, receiver).await },
            ));
        Ok(manager)
    }
    fn next(&self) -> u32 {
        loop {
            let n = self.packet.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
            if n != 0 {
                return n;
            }
        }
    }
    fn wire(
        &self,
        packet: u32,
        command: u32,
        body: &str,
        extra: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        encode_packet_with_version(
            &self.version,
            packet,
            &self.local.username,
            &self.local.hostname,
            command,
            body,
            extra,
        )
    }
    fn address(peer: &User) -> Result<SocketAddrV4, String> {
        format!("{}:{}", peer.ip, peer.port)
            .parse()
            .map_err(|_| "无效文件对端".into())
    }
    async fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self
            .events
            .send(Event {
                event: event.into(),
                payload,
            })
            .await;
    }
    fn list(&self) -> Vec<Arc<Transfer>> {
        self.transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
    fn get(&self, id: &str) -> Result<Arc<Transfer>, String> {
        self.transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| "文件任务已结束或不存在".into())
    }
    async fn update(&self, t: &Transfer) {
        self.emit(
            "file.updated",
            json!({"messageId":t.id,"target":t.peer.id,"file":t.metadata()}),
        )
        .await;
    }
    pub async fn select_path(&self, path: PathBuf) -> Result<Selection, String> {
        if self.stopped.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let _gate = self.gate.lock().await;
        if self
            .selections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
            >= 8
        {
            return Err("最多预览8个文件，请先发送或取消".into());
        }
        let id = format!("file-{}-{}", unix_seconds(), self.next());
        let source = tokio::task::spawn_blocking(move || {
            let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("暂不支持文件夹或链接，请选择普通文件".to_string());
            }
            let file = File::open(&path).map_err(|e| e.to_string())?;
            let info = file.metadata().map_err(|e| e.to_string())?;
            if !info.is_file() || info.len() > MAX_FILE_SIZE {
                return Err("仅支持8 GiB以内的普通文件".into());
            }
            let name = path
                .file_name()
                .ok_or("文件名无效")?
                .to_str()
                .ok_or("文件名编码无效")?;
            let file_name = protocol::sanitize_name(name);
            // Reject unrepresentable GBK names before opening an invitation, without lossy replacement.
            encode_packet(1, "p", "h", IPMSG_SENDMSG, "", Some(&file_name))?;
            Ok(Source {
                modified: info.modified().ok(),
                file,
                selection: Selection {
                    selection_id: id,
                    file_name,
                    file_size: info.len(),
                },
                created: Instant::now(),
            })
        })
        .await
        .map_err(|e| e.to_string())??;
        if self.stopped.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let selection = source.selection.clone();
        self.selections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(selection.selection_id.clone(), source);
        Ok(selection)
    }
    pub fn discard(&self, id: &str) {
        self.selections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
    pub async fn send(&self, peer: User, selection: &str) -> Result<Record, String> {
        let _gate = self.gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        if self
            .list()
            .iter()
            .filter(|t| !t.metadata().terminal() && t.source.is_some())
            .count()
            >= 4
        {
            return Err("最多同时发送4个文件".into());
        }
        if self.list().len() >= 128 {
            return Err("文件任务记录过多，请稍后重试".into());
        }
        let source = self
            .selections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(selection)
            .ok_or("文件预览已过期")?;
        let packet = self.next();
        let metadata = FileMetadata {
            file_name: source.selection.file_name.clone(),
            file_size: source.selection.file_size,
            state: "offered".into(),
            transferred: 0,
            incoming: false,
            has_local_file: false,
            error: None,
        };
        let extra = format!(
            "1:{}:{:x}:0:1:\x07",
            metadata.file_name.replace(':', "::"),
            metadata.file_size
        );
        let wire = self.wire(
            packet,
            IPMSG_SENDMSG | IPMSG_SENDCHECKOPT | IPMSG_FILEATTACHOPT,
            &format!("[文件] {}", metadata.file_name),
            Some(&extra),
        )?;
        let id = format!("tx-file-{}-{packet}", unix_seconds());
        let record = Record {
            id: id.clone(),
            from_id: self.local.id.clone(),
            to_id: peer.id.clone(),
            content: format!("[文件] {}", metadata.file_name),
            kind: 2,
            timestamp: unix_seconds(),
            status: 0,
            image: None,
            file: Some(metadata.clone()),
        };
        self.db.insert(record.clone()).await?;
        let (cancel, _) = watch::channel(false);
        let transfer = Arc::new(Transfer {
            id: id.clone(),
            peer,
            packet,
            file_id: 1,
            source: Some(source),
            wire: wire.clone(),
            state: Mutex::new(State {
                metadata,
                acknowledged: false,
                attempts: 1,
                retry: Instant::now() + Duration::from_secs(2),
            }),
            cancel,
            busy: AtomicBool::new(false),
            created: Instant::now(),
        });
        self.transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, transfer.clone());
        if let Err(error) = self
            .socket
            .send_to(&wire, Self::address(&transfer.peer)?)
            .await
        {
            self.finish(&transfer, "failed", Some(error.to_string()), None)
                .await?;
        }
        Ok(record)
    }
    pub async fn incoming(&self, peer: User, packet: &Packet) -> Result<(), String> {
        let offers = protocol::offers(packet)?;
        let _gate = self.gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        if self.list().len() + offers.len() > 128
            || self
                .list()
                .iter()
                .filter(|t| !t.metadata().terminal())
                .count()
                + offers.len()
                > 64
        {
            return Err("待处理文件邀请过多".into());
        }
        for offer in offers {
            let id = format!("{}:{}:file:{}", peer.id, packet.packet_no, offer.id);
            let metadata = FileMetadata {
                file_name: offer.name.clone(),
                file_size: offer.size,
                state: "offered".into(),
                transferred: 0,
                incoming: true,
                has_local_file: false,
                error: None,
            };
            let record = Record {
                id: id.clone(),
                from_id: peer.id.clone(),
                to_id: self.local.id.clone(),
                content: format!("[文件] {}", offer.name),
                kind: 2,
                timestamp: unix_seconds(),
                status: 0,
                image: None,
                file: Some(metadata.clone()),
            };
            if self.db.insert(record.clone()).await? {
                let (cancel, _) = watch::channel(false);
                self.transfers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        id.clone(),
                        Arc::new(Transfer {
                            id: id.clone(),
                            peer: peer.clone(),
                            packet: packet.packet_no,
                            file_id: offer.id,
                            source: None,
                            wire: vec![],
                            state: Mutex::new(State {
                                metadata: metadata.clone(),
                                acknowledged: true,
                                attempts: 0,
                                retry: Instant::now(),
                            }),
                            cancel,
                            busy: AtomicBool::new(false),
                            created: Instant::now(),
                        }),
                    );
                self.emit("message.received",json!({"id":id,"from":peer.id,"fromUser":peer,"content":record.content,"type":"file","timestamp":record.timestamp,"file":metadata})).await;
            }
        }
        // Receipt acknowledges the invitation only; accepting starts TCP separately.
        let ack = self.wire(
            self.next(),
            IPMSG_RECVMSG,
            &packet.packet_no.to_string(),
            None,
        )?;
        self.socket
            .send_to(&ack, Self::address(&peer)?)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub async fn receipt(&self, packet: &Packet, address: SocketAddrV4) {
        let Ok(number) = std::str::from_utf8(&packet.body)
            .unwrap_or("")
            .parse::<u32>()
        else {
            return;
        };
        for t in self.list().into_iter().filter(|t| {
            t.source.is_some()
                && t.packet == number
                && t.peer.username == packet.username
                && t.peer.hostname == packet.hostname
                && Self::address(&t.peer).ok() == Some(address)
        }) {
            if mode(packet.command) == IPMSG_RELEASEFILES {
                let _ = self.cancel(&t.id, false).await;
            } else {
                t.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .acknowledged = true;
            }
        }
    }
    pub async fn accept(&self, id: &str) -> Result<(), String> {
        let _gate = self.gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let t = self.get(id)?;
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if t.source.is_some() || state.metadata.state != "offered" {
                return Err("文件已处理或不能接收".into());
            }
            state.metadata.state = "transferring".into();
        }
        if self.downloads.try_send(t.clone()).is_err() {
            t.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .metadata
                .state = "offered".into();
            return Err("接收队列已满".into());
        }
        self.update(&t).await;
        Ok(())
    }
    pub async fn cancel(&self, id: &str, reject: bool) -> Result<bool, String> {
        let t = self.get(id)?;
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.metadata.terminal() || state.metadata.state == "finalizing" {
                return Ok(false);
            }
            state.metadata.state = if reject { "rejected" } else { "cancelled" }.into();
            state.metadata.error = Some(
                if reject {
                    "已拒绝文件"
                } else {
                    "文件传输已取消"
                }
                .into(),
            );
            t.cancel.send_replace(true);
        }
        self.db.file_state(t.id.clone(), t.metadata(), None).await?;
        self.update(&t).await;
        if t.source.is_none()
            && self
                .list()
                .iter()
                .filter(|other| {
                    other.packet == t.packet && other.peer.id == t.peer.id && other.source.is_none()
                })
                .all(|other| other.metadata().terminal())
        {
            if let Ok(wire) =
                self.wire(self.next(), IPMSG_RELEASEFILES, &t.packet.to_string(), None)
            {
                let _ = self.socket.send_to(&wire, Self::address(&t.peer)?).await;
            }
        }
        Ok(true)
    }
    pub async fn cancel_cleared(&self, ids: &[String]) {
        for id in ids {
            let _ = self.cancel(id, false).await;
        }
    }
    async fn finish(
        &self,
        t: &Transfer,
        phase: &str,
        error: Option<String>,
        path: Option<PathBuf>,
    ) -> Result<(), String> {
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.metadata.terminal() {
                return Ok(());
            }
            state.metadata.state = phase.into();
            state.metadata.error = error;
            state.metadata.has_local_file = path.is_some();
            if phase == "completed" {
                state.metadata.transferred = state.metadata.file_size;
            }
        }
        if let Err(error) = self.db.file_state(t.id.clone(), t.metadata(), path).await {
            {
                let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
                state.metadata.state = "failed".into();
                state.metadata.has_local_file = false;
                state.metadata.error = Some(format!("无法保存文件状态：{error}"));
            }
            let _ = self.db.file_state(t.id.clone(), t.metadata(), None).await;
            self.update(t).await;
            return Err(error);
        }
        self.update(t).await;
        Ok(())
    }
    async fn progress(&self, t: &Transfer, bytes: u64) {
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.metadata.terminal() {
                return;
            }
            state.metadata.transferred = state.metadata.transferred.max(bytes);
        }
        self.update(t).await;
    }
    async fn io<T>(
        &self,
        t: &Transfer,
        operation: impl Future<Output = std::io::Result<T>>,
    ) -> Result<T, String> {
        let mut cancel = t.cancel.subscribe();
        if *cancel.borrow() || self.stopped.load(Ordering::Acquire) {
            return Err("文件传输已取消".into());
        }
        tokio::select! {
            _=cancel.changed()=>Err("文件传输已取消".into()),
            result=tokio::time::timeout(Duration::from_secs(30),operation)=>result.map_err(|_|"文件传输30秒无响应".to_string())?.map_err(|e|e.to_string())
        }
    }
    async fn receive(&self, t: Arc<Transfer>) -> Result<(), String> {
        let _permit = self
            .active
            .clone()
            .try_acquire_owned()
            .map_err(|_| "当前传输已达4个，请稍后重试")?;
        let mut socket = self
            .io(&t, TcpStream::connect(Self::address(&t.peer)?))
            .await?;
        let ack = self.wire(self.next(), IPMSG_RECVMSG, &t.packet.to_string(), None)?;
        self.socket
            .send_to(&ack, Self::address(&t.peer)?)
            .await
            .map_err(|e| e.to_string())?;
        let request = self.wire(
            self.next(),
            IPMSG_GETFILEDATA,
            &format!("{:x}:{:x}:0:", t.packet, t.file_id),
            None,
        )?;
        self.io(&t, socket.write_all(&request)).await?;
        let root = self.root.clone();
        let next = self.next();
        let (mut owned, file) =
            tokio::task::spawn_blocking(move || PartialFile::create(&root, next))
                .await
                .map_err(|e| e.to_string())??;
        let mut file = tokio::fs::File::from_std(file);
        let size = t.metadata().file_size;
        let mut read = 0;
        let mut buffer = vec![0; 64 * 1024];
        let mut last = Instant::now();
        while read < size {
            let count = ((size - read) as usize).min(buffer.len());
            let n = self.io(&t, socket.read(&mut buffer[..count])).await?;
            if n == 0 {
                return Err("连接提前结束，文件不完整".into());
            }
            self.io(&t, file.write_all(&buffer[..n])).await?;
            read += n as u64;
            if last.elapsed() >= Duration::from_millis(100) {
                self.progress(&t, read).await;
                last = Instant::now();
            }
        }
        self.io(&t, file.flush()).await?;
        self.io(&t, file.sync_all()).await?;
        drop(file);
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.metadata.terminal() {
                return Err("文件传输已取消".into());
            }
            state.metadata.state = "finalizing".into();
        }
        let name = t.metadata().file_name;
        let root = self.root.clone();
        let (mut owned, path) = tokio::task::spawn_blocking(move || {
            let path = owned.publish(&root, &name)?;
            Ok::<_, String>((owned, path))
        })
        .await
        .map_err(|e| e.to_string())??;
        self.finish(&t, "completed", None, Some(path)).await?;
        owned.keep();
        Ok(())
    }
    async fn serve(&self, mut socket: TcpStream, address: SocketAddr) -> Result<(), String> {
        let mut bytes = Vec::new();
        let (packet, original, file_id, offset) =
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if bytes.len() >= 4096 {
                        return Err("TCP文件请求过长".to_string());
                    }
                    let mut chunk = [0; 512];
                    let n = socket.read(&mut chunk).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        return Err("TCP请求被截断".into());
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if bytes.len() > 4096 {
                        return Err("TCP文件请求过长".into());
                    }
                    if let Ok(request) = protocol::request(&bytes) {
                        return Ok(request);
                    }
                }
            })
            .await
            .map_err(|_| "TCP请求超时")??;
        let t = self
            .list()
            .into_iter()
            .find(|t| {
                t.source.is_some()
                    && t.packet == original
                    && t.file_id == file_id
                    && t.peer.ip == address.ip().to_string()
                    && t.peer.username == packet.username
                    && t.peer.hostname == packet.hostname
            })
            .ok_or("未授权的文件请求")?;
        let _permit = self
            .active
            .clone()
            .try_acquire_owned()
            .map_err(|_| "同时传输过多")?;
        if t.busy.swap(true, Ordering::AcqRel) {
            return Err("该文件已有TCP连接".into());
        }
        let _busy = Busy(t.clone());
        let metadata = t.metadata();
        if (metadata.terminal() && metadata.state != "completed") || offset > metadata.file_size {
            return Err("文件任务已取消或偏移无效".into());
        }
        let source = t.source.as_ref().ok_or("缺少文件源")?;
        let info = source.file.metadata().map_err(|e| e.to_string())?;
        if info.len() != metadata.file_size || info.modified().ok() != source.modified {
            return self
                .finish(&t, "failed", Some("源文件已修改，请重新发送".into()), None)
                .await;
        }
        let mut file =
            tokio::fs::File::from_std(source.file.try_clone().map_err(|e| e.to_string())?);
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            state.acknowledged = true;
            if state.metadata.state == "finalizing"
                || (state.metadata.terminal() && state.metadata.state != "completed")
            {
                return Err("文件任务已结束".into());
            }
            if !state.metadata.terminal() {
                state.metadata.state = "transferring".into();
            }
        }
        let operation = async {
            self.io(&t, file.seek(std::io::SeekFrom::Start(offset)))
                .await?;
            let mut sent = offset;
            let mut buffer = vec![0; 64 * 1024];
            let mut last = Instant::now();
            while sent < metadata.file_size {
                let count = ((metadata.file_size - sent) as usize).min(buffer.len());
                let n = self.io(&t, file.read(&mut buffer[..count])).await?;
                if n == 0 {
                    return Err("源文件长度发生变化".into());
                }
                self.io(&t, socket.write_all(&buffer[..n])).await?;
                sent += n as u64;
                if last.elapsed() >= Duration::from_millis(100) {
                    self.progress(&t, sent).await;
                    last = Instant::now();
                }
            }
            let info = source.file.metadata().map_err(|e| e.to_string())?;
            if info.len() != metadata.file_size || info.modified().ok() != source.modified {
                return Err("发送过程中源文件被修改".into());
            }
            self.io(&t, socket.shutdown()).await?;
            Ok::<_, String>(())
        };
        match tokio::time::timeout(Duration::from_secs(3600), operation).await {
            Ok(Ok(())) => self.finish(&t, "completed", None, None).await,
            result => {
                self.finish(
                    &t,
                    "failed",
                    Some(match result {
                        Ok(Err(e)) => e,
                        _ => "文件传输超过1小时".into(),
                    }),
                    None,
                )
                .await
            }
        }
    }
    async fn fail_offer(&self, t: &Transfer, error: &str) -> Result<(), String> {
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            // An accept/TCP request may have won since the timer inspected it.
            if state.metadata.state != "offered" {
                return Ok(());
            }
            state.metadata.state = "finalizing".into();
            t.cancel.send_replace(true);
        }
        self.finish(t, "failed", Some(error.into()), None).await
    }
    async fn tick(&self) {
        self.selections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, s| s.created.elapsed() < Duration::from_secs(600));
        for t in self.list() {
            let metadata = t.metadata();
            if metadata.state == "offered" && t.created.elapsed() > Duration::from_secs(600) {
                let _ = self.fail_offer(&t, "文件邀请已过期").await;
                continue;
            }
            let action = {
                let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
                if t.source.is_some()
                    && state.metadata.state == "offered"
                    && !state.acknowledged
                    && Instant::now() >= state.retry
                {
                    state.retry += Duration::from_secs(2);
                    state.attempts += 1;
                    Some(state.attempts <= 3)
                } else {
                    None
                }
            };
            match action {
                Some(true) => {
                    if let Ok(address) = Self::address(&t.peer) {
                        let _ = self.socket.send_to(&t.wire, address).await;
                    }
                }
                Some(false) => {
                    let _ = self.fail_offer(&t, "对方未确认文件邀请").await;
                }
                None => {}
            }
        }
        self.transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, t| {
                !t.metadata().terminal()
                    || t.busy.load(Ordering::Acquire)
                    || t.created.elapsed() < Duration::from_secs(600)
            });
    }
    async fn run(
        self: Arc<Self>,
        listener: TcpListener,
        mut receiver: mpsc::Receiver<Arc<Transfer>>,
    ) {
        let mut stop = self.stop.subscribe();
        let mut jobs = JoinSet::new();
        let handshakes = Arc::new(Semaphore::new(8));
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                _=stop.changed()=>break,
                _=timer.tick()=>self.tick().await,
                Some(_)=jobs.join_next(),if !jobs.is_empty()=>{},
                incoming=listener.accept()=>if let Ok((socket,address))=incoming {
                    if let Ok(permit)=handshakes.clone().try_acquire_owned(){let me=self.clone();jobs.spawn(async move {let _permit=permit;let _=me.serve(socket,address).await;});}
                },
                Some(t)=receiver.recv()=>{let me=self.clone();jobs.spawn(async move {
                    let result=tokio::time::timeout(Duration::from_secs(3600),me.receive(t.clone())).await;
                    if !matches!(result,Ok(Ok(()))){let error=match result{Ok(Err(e))=>e,_=>"文件接收超过1小时".into()};let _=me.finish(&t,"failed",Some(error),None).await;}
                });}
            }
        }
        jobs.shutdown().await;
    }
    pub async fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let _gate = self.gate.lock().await;
        for t in self.list() {
            t.cancel.send_replace(true);
        }
        self.stop.send_replace(true);
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut task) = task {
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        for t in self.list() {
            let _ = self
                .finish(&t, "failed", Some("程序退出，文件传输中断".into()), None)
                .await;
        }
        self.selections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}
