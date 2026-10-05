use super::*;
use crate::image::{
    assets::valid_asset_id,
    dib::decode_image,
    fragments::{parse_fragment, Action, TransferKey},
    ImageMetadata,
};

pub(super) struct ImageJob {
    key: TransferKey,
    peer: User,
    payload: Vec<u8>,
    deadline: Instant,
}

impl Network {
    pub async fn image_asset(&self, id: &str) -> Result<ImageMetadata, String> {
        if !valid_asset_id(id) {
            return Err("无效图片标识".into());
        }
        self.db
            .image_asset(id.to_owned())
            .await?
            .ok_or("图片未登记或不存在".into())
    }
    pub async fn read_image(&self, id: &str, thumbnail: bool) -> Result<Vec<u8>, String> {
        let _permit = self
            .asset_reads
            .acquire()
            .await
            .map_err(|_| "图片服务已关闭")?;
        self.image_asset(id).await?;
        let store = self.assets.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || store.read(&id, thumbnail))
            .await
            .map_err(|e| e.to_string())?
    }
    async fn image_ack(&self, peer: &User, key: &TransferKey, index: u32) -> Result<(), String> {
        self.send_to(
            Self::address(peer)?,
            &self.wire(
                IPMSG_REPORT_RECVIMAGE,
                &format!("{}|{index}#", key.image_id),
                None,
            )?,
        )
        .await
    }
    pub(super) async fn image_failure(&self, key: &TransferKey, error: String) {
        self.diagnostic(
            "WARN",
            format!(
                "Image receive failed peer={} image={}: {error}",
                key.peer_id, key.image_id
            ),
        );
        self.event(
            "image.receive_failed",
            json!({"peerId":key.peer_id,"imageId":key.image_id,"error":error}),
        )
        .await;
    }
    pub(super) async fn register_image_reference(
        &self,
        peer_id: &str,
        image_id: &str,
    ) -> Result<(), String> {
        let now = Instant::now();
        let (result, failures) = {
            let mut images = self
                .images
                .try_lock()
                .map_err(|_| "图片提交处理中，请重试引用")?;
            let result = images.reference(peer_id, image_id, now);
            let failures = images.expire(now);
            (result, failures)
        };
        for (key, error) in failures {
            self.image_failure(&key, error).await;
        }
        result
    }

    async fn reject_image_job(&self, job: &ImageJob, error: String) {
        let now = Instant::now();
        let failures = {
            let Ok(mut images) = self.images.try_lock() else {
                return;
            };
            let mut failures = images.expire(now);
            if now < job.deadline && images.is_finalizing(&job.key, now) {
                images.finish(&job.key, false, now);
                failures.extend(images.expire(now).into_iter().map(|(key, reason)| {
                    let reason = if key == job.key {
                        error.clone()
                    } else {
                        reason
                    };
                    (key, reason)
                }));
            }
            failures
        };
        for (key, reason) in failures {
            self.image_failure(&key, reason).await;
        }
    }
    pub(super) async fn handle_image_fragment(
        &self,
        packet: &Packet,
        address: SocketAddrV4,
    ) -> Result<(), String> {
        if packet.command & IPMSG_FILEATTACHOPT == 0 {
            return Err("图片分片缺少附件选项".into());
        }
        let fragment = parse_fragment(&packet.body)?;
        let peer = self.seen(packet, address, true).await?;
        let now = Instant::now();
        let (action, failures) = {
            // Database commits hold this gate only for metadata SQL. Dropping a
            // retransmittable datagram is preferable to blocking UDP reception.
            let Ok(mut images) = self.images.try_lock() else {
                return Ok(());
            };
            let action = images.accept(&peer.id, fragment, now);
            let failures = images.expire(now);
            (action, failures)
        };
        for (key, error) in failures {
            self.image_failure(&key, error).await;
        }
        match action? {
            Action::Ack { key, index } => self.image_ack(&peer, &key, index).await?,
            Action::Finalize {
                key,
                index: _,
                payload,
            } => {
                let job = ImageJob {
                    key,
                    peer,
                    payload,
                    deadline: now + Duration::from_secs(25),
                };
                if let Err(error) = self.image_jobs.try_send(job) {
                    let job = error.into_inner();
                    self.reject_image_job(&job, "图片处理队列已满".into()).await;
                }
            }
            Action::Pending => {}
        }
        Ok(())
    }
    pub(super) async fn receive_images(self: Arc<Self>) {
        let receiver = self
            .image_receiver
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
            let mut job = tokio::select! {
                _ = cancel.changed() => break,
                job = receiver.recv() => match job { Some(job) => job, None => break },
            };
            if !self
                .images
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_finalizing(&job.key, Instant::now())
            {
                self.reject_image_job(&job, "图片排队超时".into()).await;
                continue;
            }
            // A single blocking decode at a time. A timed-out decode is allowed
            // to finish before the next job, retaining the concurrency limit.
            let payload = std::mem::take(&mut job.payload);
            let mut decode = tokio::task::spawn_blocking(move || decode_image(&payload));
            let decoded = tokio::select! {
                result = &mut decode => result.map_err(|e| e.to_string()).and_then(|r| r),
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(job.deadline)) => {
                    self.reject_image_job(&job, "图片解码超时".into()).await;
                    let _ = decode.await;
                    continue;
                }
                _ = cancel.changed() => { let _ = decode.await; break; }
            };
            let decoded = match decoded {
                Ok(decoded) => decoded,
                Err(error) => {
                    self.reject_image_job(&job, error).await;
                    continue;
                }
            };
            if self.stopping.load(Ordering::Acquire)
                || Instant::now() >= job.deadline
                || !self
                    .images
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_finalizing(&job.key, Instant::now())
            {
                self.reject_image_job(&job, "图片处理已取消或超时".into())
                    .await;
                continue;
            }
            let store = self.assets.clone();
            let wire_id = job.key.image_id.clone();
            let saved = tokio::task::spawn_blocking(move || store.persist(decoded, &wire_id))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let pending_asset = match saved {
                Ok(asset) => asset,
                Err(error) => {
                    self.reject_image_job(&job, error).await;
                    continue;
                }
            };
            let metadata = pending_asset.metadata.clone();
            let message_id = format!("image-rx:{}:{}", job.key.peer_id, job.key.image_id);
            let timestamp = unix_seconds();
            let record = Record {
                id: message_id.clone(),
                from_id: job.peer.id.clone(),
                to_id: self.local().id,
                content: "[图片]".into(),
                kind: 1,
                timestamp,
                status: 1,
                image: Some(metadata.clone()),
            };
            // The DB queue owns the pending asset and checks the receive state
            // under the same gate as its metadata commit. Cancelling this async
            // waiter cannot delete a file after the queued transaction commits.
            if self.stopping.load(Ordering::Acquire) {
                drop(pending_asset);
                self.reject_image_job(&job, "图片接收已取消".into()).await;
                continue;
            }
            let committed = match self
                .db
                .commit_received_image(
                    record,
                    job.deadline,
                    pending_asset,
                    self.images.clone(),
                    job.key.clone(),
                )
                .await
            {
                Ok(committed) => committed,
                Err(error) => {
                    self.reject_image_job(&job, error).await;
                    continue;
                }
            };
            if self.stopping.load(Ordering::Acquire) {
                continue;
            }
            if let Some(index) = committed.ack_index {
                if committed.inserted {
                    self.event("message.received", json!({"id":message_id,"from":job.peer.id,"fromUser":job.peer,
                        "content":"[图片]","type":"image","image":metadata,"timestamp":timestamp,"command":IPMSG_SENDIMAGE | IPMSG_FILEATTACHOPT})).await;
                }
                if let Err(error) = self.image_ack(&job.peer, &job.key, index).await {
                    self.diagnostic("WARN", error);
                }
            }
        }
    }
}
