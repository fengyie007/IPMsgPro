use super::super::{directory, storage::checked_part};
use super::*;
#[derive(Serialize, Deserialize)]
struct Checkpoint {
    peer: User,
    packet: u32,
    file_id: u32,
    part: String,
}
impl FileTransfers {
    async fn checkpoint(&self, id: &str) -> Result<Option<Checkpoint>, String> {
        self.db
            .receive_checkpoint(id.into())
            .await?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    pub(super) async fn discard_partial(&self, id: &str) -> Result<(), String> {
        if let Some(checkpoint) = self.checkpoint(id).await? {
            let path = checked_part(&self.root, &checkpoint.part)?;
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        self.db.remove_receive(id.into()).await
    }
    pub(super) async fn retains_invitation(&self, t: &Transfer) -> Result<bool, String> {
        // Paused siblings may not have been restored into memory after restart.
        for id in self.db.receive_ids().await? {
            if let Some(checkpoint) = self.checkpoint(&id).await? {
                if checkpoint.packet == t.packet && checkpoint.peer.id == t.peer.id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub(super) async fn restore(&self, id: &str) -> Result<Arc<Transfer>, String> {
        if let Ok(t) = self.get(id) {
            return Ok(t);
        }
        let checkpoint = self
            .checkpoint(id)
            .await?
            .ok_or("任务已结束或没有续传记录")?;
        let mut metadata = self
            .db
            .file_metadata(id.into())
            .await?
            .ok_or("历史已清空")?;
        if !metadata.can_resume || metadata.is_directory {
            return Err("该任务不能续传".into());
        }
        // Opening the partial belongs to receive(): even a missing partial must
        // leave the restored task cancellable after restart.
        metadata.in_flight_bytes = 0;
        metadata.state = "paused".into();
        let (cancel, _) = watch::channel(false);
        let t = Arc::new(Transfer {
            id: id.into(),
            peer: checkpoint.peer,
            packet: checkpoint.packet,
            file_id: checkpoint.file_id,
            source: None,
            wire: vec![],
            state: Mutex::new(State {
                metadata,
                acknowledged: true,
                attempts: 0,
                retry: Instant::now(),
                last_activity: Instant::now(),
            }),
            cancel,
            busy: AtomicU32::new(0),
            created: Instant::now(),
        });
        // Concurrent resume/cancel calls after restart must share one task.
        Ok(self
            .transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.into())
            .or_insert(t)
            .clone())
    }
    pub async fn pause(&self, id: &str) -> Result<(), String> {
        let t = self.get(id)?;
        {
            let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if t.source.is_some()
                || state.metadata.is_directory
                || !state.metadata.can_resume
                || state.metadata.state != "transferring"
            {
                return Err("当前文件不能暂停".into());
            }
            state.metadata.state = "paused".into();
            state.metadata.in_flight_bytes = 0;
            state.metadata.error = Some("已暂停，可继续接收".into());
            t.cancel.send_replace(true);
        }
        self.db.file_state(t.id.clone(), t.metadata(), None).await?;
        self.update(&t).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while t.busy.load(Ordering::Acquire) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        Ok(())
    }
    pub(super) async fn receive_failed(&self, t: &Transfer, error: String) -> Result<(), String> {
        if t.metadata().terminal() {
            return Ok(());
        }
        let resumable = t.source.is_none()
            && !t.metadata().is_directory
            && self.checkpoint(&t.id).await?.is_some();
        {
            let mut s = t.state.lock().unwrap_or_else(|e| e.into_inner());
            s.metadata.can_resume = resumable;
        }
        self.finish(
            t,
            if resumable { "paused" } else { "failed" },
            Some(error),
            None,
        )
        .await
    }
    async fn connection(&self, t: &Transfer, offset: u64) -> Result<TcpStream, String> {
        let ack = self.wire(self.next(), IPMSG_RECVMSG, &t.packet.to_string(), None)?;
        self.socket
            .send_to(&ack, Self::address(&t.peer)?)
            .await
            .map_err(|e| e.to_string())?;
        let mut socket = self
            .io(t, TcpStream::connect(Self::address(&t.peer)?))
            .await?;
        let request = self.wire(
            self.next(),
            if t.metadata().is_directory {
                IPMSG_GETDIRFILES
            } else {
                IPMSG_GETFILEDATA
            },
            &format!("{:x}:{:x}:{offset:x}:", t.packet, t.file_id),
            None,
        )?;
        self.io(t, socket.write_all(&request)).await?;
        Ok(socket)
    }
    pub(super) async fn receive(self: &Arc<Self>, t: Arc<Transfer>) -> Result<(), String> {
        if *t.cancel.borrow() || self.stopped.load(Ordering::Acquire) {
            return Err("接收已停止".into());
        }
        let _permit = self
            .io(&t, async {
                self.active
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(std::io::Error::other)
            })
            .await?;
        if t.metadata().is_directory {
            return self.receive_directory(&t).await;
        }
        let size = t.metadata().file_size;
        let (mut owned, file) = if let Some(checkpoint) = self.checkpoint(&t.id).await? {
            PartialFile::resume(&self.root, &checkpoint.part, size)?
        } else {
            let root = self.root.clone();
            let next = self.next();
            tokio::task::spawn_blocking(move || PartialFile::create(&root, next))
                .await
                .map_err(|e| e.to_string())??
        };
        let offset = file.metadata().map_err(|e| e.to_string())?.len();
        self.db
            .save_receive(
                t.id.clone(),
                serde_json::to_value(Checkpoint {
                    peer: t.peer.clone(),
                    packet: t.packet,
                    file_id: t.file_id,
                    part: owned.name(),
                })
                .map_err(|e| e.to_string())?,
            )
            .await?;
        let cancelled = {
            let mut s = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.metadata.terminal() || *t.cancel.borrow() {
                true
            } else {
                owned.keep();
                s.metadata.can_resume = true;
                s.metadata.transferred = offset;
                false
            }
        };
        if cancelled {
            drop(file);
            self.discard_partial(&t.id).await?;
            return Err("接收已停止".into());
        }
        self.db.file_state(t.id.clone(), t.metadata(), None).await?;
        self.update(&t).await;
        let mut file = tokio::fs::File::from_std(file);
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| e.to_string())?;
        let result = async {
            let mut done = offset;
            if size - offset >= 4 * 1024 * 1024 {
                let mut extra = Vec::new();
                for _ in 0..3 {
                    if let Ok(permit) = self.active.clone().try_acquire_owned() {
                        extra.push(permit);
                    } else {
                        break;
                    }
                }
                if !extra.is_empty() {
                    let result = self
                        .receive_parallel(t.clone(), &mut file, offset, size, extra)
                        .await;
                    t.state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .metadata
                        .in_flight_bytes = 0;
                    if result.is_ok() {
                        return Ok(());
                    }
                    if *t.cancel.borrow() || self.stopped.load(Ordering::Acquire) {
                        return result;
                    }
                    file.flush().await.map_err(|e| e.to_string())?;
                    done = file.metadata().await.map_err(|e| e.to_string())?.len();
                    file.seek(std::io::SeekFrom::Start(done))
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            // Empty files still need a request so the sender observes acceptance.
            let mut socket = self.connection(&t, done).await?;
            let mut buffer = vec![0; 64 * 1024];
            let mut last = Instant::now();
            while done < size {
                let count = (size - done).min(buffer.len() as u64) as usize;
                let n = self.io(&t, socket.read(&mut buffer[..count])).await?;
                if n == 0 {
                    return Err("连接中断，可继续接收".into());
                }
                self.io(&t, file.write_all(&buffer[..n])).await?;
                done += n as u64;
                if last.elapsed() >= Duration::from_millis(100) {
                    self.progress(&t, done).await;
                    last = Instant::now();
                }
            }
            Ok::<_, String>(())
        }
        .await;
        // Drain in-flight disk writes even on pause, before allowing another attempt.
        let flushed = file.flush().await.map_err(|e| e.to_string());
        let length = file.metadata().await.map_err(|e| e.to_string())?.len();
        self.progress(&t, length).await;
        if let Err(error) = result {
            drop(file);
            return Err(error);
        }
        flushed?;
        self.io(&t, file.sync_all()).await?;
        drop(file);
        {
            let mut s = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.metadata.terminal() {
                return Err("接收已停止".into());
            }
            s.metadata.state = "finalizing".into();
        }
        let path = owned.publish(&self.root, &t.metadata().file_name)?;
        {
            t.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .metadata
                .can_resume = false;
        }
        self.finish(&t, "completed", None, Some(path)).await?;
        self.db.remove_receive(t.id.clone()).await?;
        Ok(())
    }
    async fn receive_parallel(
        self: &Arc<Self>,
        t: Arc<Transfer>,
        output: &mut tokio::fs::File,
        offset: u64,
        size: u64,
        mut extra: Vec<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<(), String> {
        use std::collections::BTreeMap;
        use std::sync::atomic::AtomicU64;
        let count = extra.len() + 1;
        let chunk = (size - offset).div_ceil(count as u64);
        let progress = Arc::new((0..count).map(|_| AtomicU64::new(0)).collect::<Vec<_>>());
        let mut jobs = JoinSet::new();
        for i in 0..count {
            let me = self.clone();
            let t = t.clone();
            let totals = progress.clone();
            let permit = if i == 0 { None } else { extra.pop() };
            let start = offset + chunk * i as u64;
            let end = size.min(start + chunk);
            jobs.spawn(async move {
                let _permit = permit;
                let mut socket = me.connection(&t, start).await?;
                let (owned, file) = PartialFile::create(&me.root, me.next())?;
                let mut file = tokio::fs::File::from_std(file);
                let mut remaining = end - start;
                let mut buffer = vec![0; 64 * 1024];
                while remaining > 0 {
                    let length = remaining.min(buffer.len() as u64) as usize;
                    let n = me.io(&t, socket.read(&mut buffer[..length])).await?;
                    if n == 0 {
                        return Err("并行连接中断".into());
                    }
                    me.io(&t, file.write_all(&buffer[..n])).await?;
                    remaining -= n as u64;
                    totals[i].fetch_add(n as u64, Ordering::Release);
                }
                file.flush().await.map_err(|e| e.to_string())?;
                drop(file);
                drop(socket);
                Ok::<_, String>((i, owned))
            });
        }
        let mut ready = BTreeMap::new();
        let mut next = 0;
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        let result=async{
            while !jobs.is_empty(){
                tokio::select!{
                    item=jobs.join_next()=>{let(index,owned)=item.ok_or("并行任务丢失")?.map_err(|e|e.to_string())??;ready.insert(index,owned);},
                    _=timer.tick()=>{
                        {let mut state=t.state.lock().unwrap_or_else(|e|e.into_inner());if state.metadata.terminal(){return Err("接收已停止".into());}
                            state.metadata.in_flight_bytes=offset+progress.iter().map(|n|n.load(Ordering::Acquire)).sum::<u64>();}
                        self.update(&t).await;
                    }
                }
                while let Some(owned)=ready.remove(&next){
                    let mut input=tokio::fs::File::open(checked_part(&self.root,&owned.name())?).await.map_err(|e|e.to_string())?;
                    self.io(&t,tokio::io::copy(&mut input,output)).await?;output.flush().await.map_err(|e|e.to_string())?;
                    let bytes=output.metadata().await.map_err(|e|e.to_string())?.len();self.progress(&t,bytes).await;
                    drop(input);drop(owned);next+=1;
                }
            }
            if next!=count{return Err("并行片段不完整".into());}Ok(())
        }.await;
        jobs.shutdown().await;
        result
    }
    async fn read_header(
        &self,
        t: &Transfer,
        socket: &mut TcpStream,
    ) -> Result<(String, u64, u32), String> {
        let mut prefix = [0u8; 5];
        self.io(t, socket.read_exact(&mut prefix)).await?;
        if prefix[4] != b':' {
            return Err("目录头格式错误".into());
        }
        let len = usize::from_str_radix(
            std::str::from_utf8(&prefix[..4]).map_err(|_| "目录长度无效")?,
            16,
        )
        .map_err(|_| "目录长度无效")?;
        if !(5..=8192).contains(&len) {
            return Err("目录头长度越界".into());
        }
        let mut bytes = vec![0; len];
        bytes[..5].copy_from_slice(&prefix);
        self.io(t, socket.read_exact(&mut bytes[5..])).await?;
        directory::decode(&bytes)
    }
    async fn receive_directory(&self, t: &Transfer) -> Result<(), String> {
        let mut socket = self.connection(t, 0).await?;
        let mut owned = directory::Staging::create(&self.root, self.next())?;
        let mut stack = vec![owned.root.clone()];
        let mut first = true;
        let mut bytes = 0u64;
        let mut last = Instant::now();
        for _ in 0..directory::MAX_ENTRIES {
            let (name, size, kind) = self.read_header(t, &mut socket).await?;
            if first {
                if kind != 2 {
                    return Err("目录流必须从根目录开始".into());
                }
                first = false;
                continue;
            }
            if kind == 3 {
                stack.pop();
                if stack.is_empty() {
                    {
                        let mut s = t.state.lock().unwrap_or_else(|e| e.into_inner());
                        if s.metadata.terminal() {
                            return Err("接收已取消".into());
                        }
                        s.metadata.state = "finalizing".into();
                        s.metadata.file_size = bytes;
                    }
                    let path = owned.publish(&self.root, &t.metadata().file_name)?;
                    self.finish(t, "completed", None, Some(path)).await?;
                    owned.keep();
                    return Ok(());
                }
                continue;
            }
            let parent = stack.last().ok_or("目录栈为空")?;
            if parent.canonicalize().map_err(|e| e.to_string())? != *parent {
                return Err("接收目录发生变化".into());
            }
            let path = parent.join(&name);
            if kind == 2 {
                if stack.len() >= directory::MAX_DEPTH {
                    return Err("目录层级过多".into());
                }
                tokio::fs::create_dir(&path)
                    .await
                    .map_err(|e| e.to_string())?;
                stack.push(path);
                continue;
            }
            if bytes.checked_add(size).is_none_or(|n| n > MAX_FILE_SIZE) {
                return Err("目录数据超过8 GiB".into());
            }
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await
                .map_err(|e| e.to_string())?;
            let mut remaining = size;
            let mut buffer = vec![0; 64 * 1024];
            while remaining > 0 {
                let count = remaining.min(buffer.len() as u64) as usize;
                let n = self.io(t, socket.read(&mut buffer[..count])).await?;
                if n == 0 {
                    return Err("文件夹传输被截断".into());
                }
                self.io(t, file.write_all(&buffer[..n])).await?;
                remaining -= n as u64;
                bytes += n as u64;
                {
                    let mut state = t.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.metadata.file_size = state.metadata.file_size.max(bytes);
                }
                if last.elapsed() > Duration::from_millis(100) {
                    self.progress(t, bytes).await;
                    last = Instant::now();
                }
            }
            self.io(t, file.flush()).await?;
            self.io(t, file.sync_all()).await?;
        }
        Err("目录项目过多或缺少结束标记".into())
    }
    pub(super) async fn serve(
        &self,
        mut socket: TcpStream,
        address: SocketAddr,
    ) -> Result<(), String> {
        let mut bytes = Vec::new();
        let (packet, original, file_id, offset) =
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let mut b = [0; 512];
                    let n = socket.read(&mut b).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        return Err("TCP请求截断".to_string());
                    }
                    bytes.extend_from_slice(&b[..n]);
                    if bytes.len() > 4096 {
                        return Err("TCP请求过长".into());
                    }
                    if let Ok(r) = protocol::request(&bytes) {
                        return Ok(r);
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
        let permit = Arc::new(
            self.io(&t, async {
                self.uploads
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(std::io::Error::other)
            })
            .await?,
        );
        let metadata = t.metadata();
        if metadata.is_directory != (mode(packet.command) == IPMSG_GETDIRFILES)
            || offset > metadata.file_size
            || (metadata.terminal() && metadata.state != "completed")
        {
            return Err("文件状态或请求类型不匹配".into());
        }
        if metadata.is_directory {
            if t.busy
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err("文件夹正在发送".into());
            }
        } else {
            t.busy.fetch_add(1, Ordering::AcqRel);
        }
        let _busy = Busy(t.clone());
        {
            let mut s = t.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.metadata.state == "finalizing"
                || (s.metadata.terminal() && s.metadata.state != "completed")
            {
                return Err("文件已结束".into());
            }
            s.acknowledged = true;
            s.last_activity = Instant::now();
            if !s.metadata.terminal() {
                s.metadata.state = "transferring".into();
                s.metadata.error = None;
            }
        }
        let source = t.source.as_ref().ok_or("没有文件源")?;
        let operation = async {
            if let Some(folder) = &source.folder {
                let mut sent = 0;
                for entry in &folder.entries {
                    let header = directory::encode(&entry.name, entry.size, entry.kind)?;
                    self.io(&t, socket.write_all(&header)).await?;
                    if entry.kind == 1 {
                        let resolved = entry.path.canonicalize().map_err(|e| e.to_string())?;
                        if !resolved.starts_with(&folder.root)
                            || directory::is_link(
                                &std::fs::symlink_metadata(&entry.path)
                                    .map_err(|e| e.to_string())?,
                            )
                        {
                            return Err("发送目录发生变化".into());
                        }
                        let file = Arc::new(File::open(resolved).map_err(|e| e.to_string())?);
                        self.send_bytes(
                            &t,
                            &mut socket,
                            file,
                            entry.modified,
                            entry.size,
                            0,
                            sent,
                            permit.clone(),
                        )
                        .await?;
                        sent += entry.size;
                    }
                }
            } else {
                self.send_bytes(
                    &t,
                    &mut socket,
                    source.file.as_ref().ok_or("文件源丢失")?.clone(),
                    source.modified,
                    metadata.file_size,
                    offset,
                    0,
                    permit.clone(),
                )
                .await?;
            }
            self.io(&t, socket.shutdown()).await?;
            Ok::<_, String>(())
        };
        match tokio::time::timeout(Duration::from_secs(3600), operation).await {
            Ok(Ok(())) => self.finish(&t, "completed", None, None).await,
            result => {
                let error = match result {
                    Ok(Err(e)) => e,
                    _ => "文件发送超过1小时".into(),
                };
                if metadata.is_directory
                    || error.contains("源文件")
                    || error.contains("目录发生变化")
                {
                    self.finish(&t, "failed", Some(error), None).await
                } else {
                    {
                        let mut s = t.state.lock().unwrap_or_else(|e| e.into_inner());
                        if !s.metadata.terminal() {
                            s.metadata.error = Some("连接中断，等待对方续传".into());
                            s.last_activity = Instant::now();
                        }
                    }
                    self.update(&t).await;
                    Ok(())
                }
            }
        }
    }
    async fn send_bytes(
        &self,
        t: &Transfer,
        socket: &mut TcpStream,
        file: Arc<File>,
        modified: Option<SystemTime>,
        size: u64,
        offset: u64,
        base: u64,
        permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<(), String> {
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if metadata.len() != size || metadata.modified().ok() != modified {
            return Err("源文件已改变".into());
        }
        let mut sent = offset;
        let mut last = Instant::now();
        while sent < size {
            let source = file.clone();
            let guard = permit.clone();
            let length = (size - sent).min(64 * 1024) as usize;
            let position = sent;
            let read = async move {
                tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    let mut bytes = vec![0; length];
                    #[cfg(windows)]
                    let n = {
                        use std::os::windows::fs::FileExt;
                        source.seek_read(&mut bytes, position)?
                    };
                    #[cfg(unix)]
                    let n = {
                        use std::os::unix::fs::FileExt;
                        source.read_at(&mut bytes, position)?
                    };
                    bytes.truncate(n);
                    Ok::<_, std::io::Error>(bytes)
                })
                .await
                .map_err(std::io::Error::other)?
            };
            let bytes = self.io(t, read).await?;
            if bytes.is_empty() {
                return Err("源文件长度变化".into());
            }
            self.io(t, socket.write_all(&bytes)).await?;
            sent += bytes.len() as u64;
            if last.elapsed() > Duration::from_millis(100) {
                self.progress(t, base + sent).await;
                last = Instant::now();
            }
        }
        let after = file.metadata().map_err(|e| e.to_string())?;
        if after.len() != size || after.modified().ok() != modified {
            return Err("源文件在发送中被修改".into());
        }
        Ok(())
    }
}
