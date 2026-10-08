use super::*;
use crate::image::{
    assets::valid_asset_id,
    dib::{encode_png_for_wire, import_image, MAX_IMPORT_BYTES},
    ImageMetadata,
};
use std::{io::Read, path::PathBuf};
use tokio::sync::Notify;
use tokio::time::Instant as Clock;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSendResult {
    pub message_id: String,
    pub image_id: String,
    pub image: ImageMetadata,
}

pub(super) struct ImageSendJob {
    result: ImageSendResult,
    peer: User,
    deadline: Clock,
    tracker: Mutex<Tracker>,
    changed: Notify,
}
#[derive(Default)]
struct Tracker {
    sent: Vec<bool>,
    acked: Vec<bool>,
    acknowledged: usize,
    reference_packet: u32,
    reference_sent: bool,
    reference_ack: bool,
    cancelled: bool,
    sealed: bool,
    done: bool,
    last_progress: Option<(u32, &'static str)>,
    last_emit: Option<Clock>,
}
impl ImageSendJob {
    fn check(&self, stopping: bool, deadline: Clock) -> Result<(), String> {
        let state = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
        if state.cancelled || stopping {
            return Err("图片发送已取消".into());
        }
        if state.sealed || state.done {
            return Err("图片发送已经结束".into());
        }
        if Clock::now() >= deadline {
            return Err("图片发送超过120秒期限".into());
        }
        Ok(())
    }
    fn cancel(&self) -> bool {
        let mut state = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed || state.done {
            return false;
        }
        state.cancelled = true;
        self.changed.notify_one();
        true
    }
}
fn peer_matches(user: &User, packet: &Packet, address: SocketAddrV4) -> bool {
    user.username == packet.username
        && user.hostname == packet.hostname
        && user.ip == address.ip().to_string()
        && user.port == address.port()
}

impl Network {
    pub async fn discard_image(&self, id: &str) -> Result<bool, String> {
        if !valid_asset_id(id) {
            return Err("无效图片标识".into());
        }
        let ids = self.db.discard_imports(Some(id.into()), i64::MAX).await?;
        let removed = !ids.is_empty();
        self.remove_import_files(ids).await?;
        Ok(removed)
    }
    async fn remove_import_files(&self, ids: Vec<String>) -> Result<(), String> {
        for id in ids {
            let store = self.assets.clone();
            let asset = id.clone();
            tokio::task::spawn_blocking(move || store.remove_unreferenced(&asset))
                .await
                .map_err(|e| e.to_string())??;
            self.db.forget_discarded_asset(id).await?;
        }
        Ok(())
    }
    pub(super) async fn clean_image_imports(&self, all: bool) {
        let cutoff = if all { i64::MAX } else { unix_seconds() - 600 };
        match self.db.discard_imports(None, cutoff).await {
            Ok(ids) => {
                if let Err(error) = self.remove_import_files(ids).await {
                    self.diagnostic("WARN", format!("预览清理失败，将重试：{error}"));
                }
            }
            Err(error) => self.diagnostic("WARN", error),
        }
    }
    pub async fn import_image_path(&self, path: PathBuf) -> Result<ImageMetadata, String> {
        self.import_image_source(move || {
            let mut input = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let info = input.metadata().map_err(|e| e.to_string())?;
            if !info.is_file() || info.len() == 0 || info.len() > MAX_IMPORT_BYTES as u64 {
                return Err("仅支持不超过20 MiB的图片文件".to_string());
            }
            let mut bytes = Vec::new();
            (&mut input)
                .take(MAX_IMPORT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let name = path
                .with_extension("png")
                .file_name()
                .ok_or("图片文件名无效")?
                .to_string_lossy()
                .into_owned();
            Ok((bytes, name))
        })
        .await
    }

    /// Import an editor result without exposing filesystem paths to its window.
    pub async fn import_screenshot(&self, png: Vec<u8>) -> Result<ImageMetadata, String> {
        if png.len() > MAX_IMPORT_BYTES || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("截图必须是20 MiB以内的PNG".into());
        }
        self.import_image_source(move || Ok((png, format!("截图_{}.png", unix_seconds()))))
            .await
    }

    async fn import_image_source(
        &self,
        source: impl FnOnce() -> Result<(Vec<u8>, String), String> + Send + 'static,
    ) -> Result<ImageMetadata, String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        self.clean_image_imports(false).await;
        let permit = self
            .image_codec
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "图片处理已关闭")?;
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let store = self.assets.clone();
        let pending = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let (bytes, name) = source()?;
            let decoded = import_image(&bytes)?;
            let mut pending = store.persist(decoded, "import")?;
            pending.metadata.file_name = name;
            Ok::<_, String>(pending)
        })
        .await
        .map_err(|e| e.to_string())??;
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        self.db.register_import(pending).await
    }
    pub async fn send_image(
        &self,
        target: &str,
        asset_id: &str,
    ) -> Result<ImageSendResult, String> {
        let _gate = self.image_send_gate.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        if self
            .image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
            >= 4
        {
            return Err("图片发送队列已满，请稍后重试".into());
        }
        let permit = self
            .image_send_queue
            .clone()
            .try_reserve_owned()
            .map_err(|_| "图片发送队列已满")?;
        let peer = self
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(target)
            .map(|p| p.user.clone())
            .ok_or("请先发现目标用户")?;
        let image = self.image_asset(asset_id).await?;
        let image_id = format!(
            "{:08x}",
            self.image_sequence.fetch_add(1, Ordering::Relaxed)
        );
        let message_id = format!(
            "image-tx-{}-{}-{image_id}",
            unix_seconds(),
            self.next_packet()
        );
        self.db
            .insert_sent_image(Record {
                id: message_id.clone(),
                from_id: self.local().id,
                to_id: peer.id.clone(),
                content: "[图片]".into(),
                kind: 1,
                timestamp: unix_seconds(),
                status: 0,
                file: None,
                image: Some(image.clone()),
            })
            .await?;
        let result = ImageSendResult {
            message_id,
            image_id,
            image,
        };
        let job = Arc::new(ImageSendJob {
            result: result.clone(),
            peer,
            deadline: Clock::now() + Duration::from_secs(120),
            tracker: Mutex::new(Tracker::default()),
            changed: Notify::new(),
        });
        self.image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(result.message_id.clone(), job.clone());
        permit.send(job);
        Ok(result)
    }
    pub fn cancel_image(&self, message_id: &str) -> bool {
        self.image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(message_id)
            .is_some_and(|job| job.cancel())
    }
    pub(super) fn cancel_image_sends(&self) {
        for job in self
            .image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            job.cancel();
        }
    }
    pub(super) async fn interrupt_image_sends(&self) {
        let jobs: Vec<_> = self
            .image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, job)| job)
            .collect();
        for job in jobs {
            let _ = self
                .db
                .interrupt_sending(job.result.message_id.clone())
                .await;
        }
    }
    pub(super) async fn expire_image_sends(&self) {
        let jobs: Vec<_> = self
            .image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for job in jobs {
            let should_finish = {
                let state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
                !state.sealed && (state.cancelled || Clock::now() >= job.deadline)
            };
            if should_finish {
                self.finish_image_send(&job, Some("图片发送已取消或超时".into()))
                    .await;
            }
        }
    }

    pub(super) fn image_send_ack(&self, packet: &Packet, address: SocketAddrV4) -> bool {
        let mut valid = false;
        let jobs: Vec<_> = self
            .image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for job in jobs {
            if !peer_matches(&job.peer, packet, address) {
                continue;
            }
            let mut state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
            if state.sealed || state.done {
                continue;
            }
            let Ok(body) = std::str::from_utf8(&packet.body) else {
                continue;
            };
            match mode(packet.command) {
                IPMSG_RECVMSG => {
                    if state.reference_sent
                        && body.parse::<u32>().ok() == Some(state.reference_packet)
                    {
                        state.reference_ack = true;
                        valid = true;
                        job.changed.notify_one();
                    }
                }
                IPMSG_REPORT_RECVIMAGE => {
                    let Some((id, part)) = body.strip_suffix('#').and_then(|s| s.split_once('|'))
                    else {
                        continue;
                    };
                    if id != job.result.image_id
                        || part.is_empty()
                        || !part.bytes().all(|b| b.is_ascii_digit())
                    {
                        continue;
                    }
                    let Ok(index) = part.parse::<usize>() else {
                        continue;
                    };
                    if index > 0 && index <= state.sent.len() && state.sent[index - 1] {
                        valid = true;
                        if !state.acked[index - 1] {
                            state.acked[index - 1] = true;
                            state.acknowledged += 1;
                            job.changed.notify_one();
                        }
                    }
                }
                _ => {}
            }
        }
        valid
    }
    async fn image_progress(&self, job: &ImageSendJob, progress: u32, stage: &'static str) {
        {
            let mut state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
            if state.done || state.sealed {
                return;
            }
            if state.last_progress == Some((progress, stage)) {
                return;
            }
            if state.last_progress.is_some_and(|(_, s)| s == stage)
                && state
                    .last_emit
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
            {
                return;
            }
            state.last_progress = Some((progress, stage));
            state.last_emit = Some(Clock::now());
        }
        self.event("image.send_progress",json!({"messageId":job.result.message_id,"target":job.peer.id,"progress":progress,"stage":stage,"image":job.result.image})).await;
    }
    async fn finish_image_send(&self, job: &ImageSendJob, mut error: Option<String>) {
        let cancelled = {
            let mut state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
            if state.sealed || state.done {
                return;
            }
            state.sealed = true;
            job.changed.notify_one();
            if error.is_none() && Clock::now() >= job.deadline {
                error = Some("图片发送超过120秒期限".into());
            }
            let cancelled = state.cancelled || self.stopping.load(Ordering::Acquire);
            if cancelled {
                error = Some("图片发送已取消".into());
            }
            cancelled
        };
        if let Err(db_error) = self
            .db
            .status(
                job.result.message_id.clone(),
                if error.is_some() { 3 } else { 1 },
            )
            .await
        {
            error = Some(format!("图片状态保存失败：{db_error}"));
        }
        job.tracker.lock().unwrap_or_else(|e| e.into_inner()).done = true;
        let event = if error.is_some() {
            "image.send_failed"
        } else {
            "image.send_completed"
        };
        self.event(event,json!({"messageId":job.result.message_id,"target":job.peer.id,"progress":if error.is_some(){0}else{100},"error":error,"cancelled":cancelled})).await;
        self.image_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&job.result.message_id);
    }
    async fn wait_image(
        &self,
        job: &ImageSendJob,
        until: Clock,
        overall: Clock,
        ready: impl Fn(&Tracker) -> bool,
    ) -> Result<bool, String> {
        loop {
            let notified = job.changed.notified();
            job.check(self.stopping.load(Ordering::Acquire), overall)?;
            if ready(&job.tracker.lock().unwrap_or_else(|e| e.into_inner())) {
                return Ok(true);
            }
            if Clock::now() >= until {
                return Ok(false);
            }
            tokio::select! {_=notified=>{}, _=tokio::time::sleep_until(until)=>return Ok(false)}
        }
    }
    async fn transmit_image(&self, job: &ImageSendJob) -> Result<(), String> {
        let overall = job.deadline;
        job.check(self.stopping.load(Ordering::Acquire), overall)?;
        self.image_progress(job, 0, "encoding").await;
        let permit = tokio::select! {
            permit=self.image_codec.clone().acquire_owned()=>permit.map_err(|_|"图片编码服务已关闭")?,
            _=job.changed.notified()=>{job.check(self.stopping.load(Ordering::Acquire),overall)?;return Err("图片编码已取消".into());}
            _=tokio::time::sleep_until(overall)=>return Err("图片编码排队超时".into()),
        };
        job.check(self.stopping.load(Ordering::Acquire), overall)?;
        let store = self.assets.clone();
        let id = job.result.image.asset_id.clone();
        let mut work = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let bytes = store.read(&id, false)?;
            encode_png_for_wire(&bytes)
        });
        let payload = tokio::select! {
            result=&mut work=>result.map_err(|e|e.to_string())??,
            _=job.changed.notified()=>{
                let error=job.check(self.stopping.load(Ordering::Acquire),overall).err().unwrap_or("图片编码已中断".into());
                self.finish_image_send(job,Some(error.clone())).await;
                let _=work.await;
                return Err(error);
            }
            _=tokio::time::sleep_until(overall)=>{
                let error="图片编码超过发送期限".to_string();self.finish_image_send(job,Some(error.clone())).await;
                let _=work.await;return Err(error);
            }
        };
        job.check(self.stopping.load(Ordering::Acquire), overall)?;
        let count = payload.len().div_ceil(512);
        {
            let mut state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
            state.sent = vec![false; count];
            state.acked = vec![false; count];
        }
        self.image_progress(job, 0, "transferring").await;
        for base in (0..count).step_by(32) {
            let end = (base + 32).min(count);
            let final_window = end == count;
            let rounds = if final_window { 15 } else { 4 };
            let interval = if final_window { 2 } else { 1 };
            let window_deadline = (Clock::now()
                + Duration::from_secs(if final_window { 30 } else { 4 }))
            .min(overall);
            let mut complete = false;
            for _ in 0..rounds {
                for index in base..end {
                    job.check(self.stopping.load(Ordering::Acquire), overall)?;
                    {
                        let mut state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
                        if state.acked[index] {
                            continue;
                        }
                        state.sent[index] = true;
                    }
                    let offset = index * 512;
                    let length = (payload.len() - offset).min(512);
                    let mut body = format!(
                        "{}|{}|{offset}|{count}|{}|{length}|0|1|0|00000000#",
                        job.result.image_id,
                        payload.len(),
                        index + 1
                    )
                    .into_bytes();
                    body.push(0);
                    body.extend_from_slice(&payload[offset..offset + length]);
                    let local = self.local();
                    let packet = encode_image_packet(
                        &self.wire_version,
                        self.next_packet(),
                        &local.username,
                        &local.hostname,
                        &body,
                    )?;
                    self.send_to(Self::address(&job.peer)?, &packet).await?;
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                complete = self
                    .wait_image(
                        job,
                        (Clock::now() + Duration::from_secs(interval)).min(window_deadline),
                        overall,
                        |state| state.acked[base..end].iter().all(|v| *v),
                    )
                    .await?;
                let progress = {
                    let state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
                    ((state.acknowledged * 100 / count) as u32).min(99)
                };
                self.image_progress(job, progress, "transferring").await;
                if complete || Clock::now() >= window_deadline {
                    break;
                }
            }
            if !complete {
                return Err("图片分片确认超时，可尝试通过原版文件通道发送".into());
            }
        }
        // Never insert the chat reference before all image data is acknowledged.
        self.image_progress(job, 99, "waiting_reference").await;
        let number = self.next_packet();
        {
            let mut state = job.tracker.lock().unwrap_or_else(|e| e.into_inner());
            state.reference_packet = number;
            state.reference_sent = true;
        }
        let local = self.local();
        let reference = format!(
            "/~#>{}<B~{{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}}",
            job.result.image_id
        );
        let packet = encode_packet_with_version(
            &self.wire_version,
            number,
            &local.username,
            &local.hostname,
            IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
            &reference,
            Some(""),
        )?;
        for _ in 0..4 {
            job.check(self.stopping.load(Ordering::Acquire), overall)?;
            self.send_to(Self::address(&job.peer)?, &packet).await?;
            if self
                .wait_image(
                    job,
                    (Clock::now() + Duration::from_secs(1)).min(overall),
                    overall,
                    |state| state.reference_ack,
                )
                .await?
            {
                return Ok(());
            }
        }
        Err("图片已传输，但图片引用未确认".into())
    }
    pub(super) async fn send_images(self: Arc<Self>) {
        let receiver = self
            .image_send_receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(mut receiver) = receiver else {
            return;
        };
        let mut cancel = self.cancel.subscribe();
        loop {
            if *cancel.borrow() {
                break;
            }
            let job = tokio::select! {_=cancel.changed()=>break,job=receiver.recv()=>match job{Some(job)=>job,None=>break}};
            self.image_progress(&job, 0, "queued").await;
            let result = self.transmit_image(&job).await;
            self.finish_image_send(&job, result.err()).await;
        }
    }
}
