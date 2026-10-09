use super::*;

const COOLDOWN: Duration = Duration::from_secs(10);
const LIMIT: usize = 4096;

impl Network {
    pub async fn send_knock(&self, target: &str) -> Result<Record, String> {
        let _send = self.send_gate.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let peer = self
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(target)
            .map(|peer| peer.user.clone())
            .ok_or("未找到对端，请先刷新联系人")?;
        if peer.status == "offline" {
            return Err("对方已离线，不能发送窗口振动".into());
        }
        {
            let mut sent = self.knock_sent.lock().unwrap_or_else(|e| e.into_inner());
            sent.retain(|_, at| at.elapsed() < COOLDOWN);
            if sent.contains_key(target) {
                return Err("发送窗口振动过于频繁，请等待10秒后再试".into());
            }
            if sent.len() >= LIMIT {
                return Err("窗口振动发送过于频繁".into());
            }
        }
        let local = self.local();
        let packet = self.next_packet();
        // FeiQ KNOCK has an empty body and no requested receipt. Never retry it:
        // peers may not deduplicate effects or acknowledge this command.
        let wire = encode_packet_with_version(
            &self.wire_version.snapshot(),
            packet,
            &local.username,
            &local.hostname,
            IPMSG_KNOCK,
            "",
            None,
        )?;
        let mut record = Record {
            id: format!(
                "{}_{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                packet
            ),
            from_id: local.id,
            to_id: peer.id.clone(),
            content: "[窗口振动]".into(),
            kind: 3,
            timestamp: unix_seconds(),
            status: 0,
            image: None,
            file: None,
        };
        self.db.insert(record.clone()).await?;
        self.knock_sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(target.into(), Instant::now());
        record.status = match self.send_to(Self::address(&peer)?, &wire).await {
            Ok(()) => 1, // For KNOCK this means sent, not acknowledged/delivered.
            Err(error) => {
                self.diagnostic("WARN", format!("窗口振动发送失败：{error}"));
                3
            }
        };
        self.db.status(record.id.clone(), record.status).await?;
        Ok(record)
    }

    pub(super) async fn receive_knock(
        &self,
        packet: &Packet,
        address: SocketAddrV4,
    ) -> Result<(), String> {
        // Accept the empty legacy frame with zero, one or two terminators.
        if packet.body.len() > 2
            || packet.body.iter().any(|byte| *byte != 0)
            || !packet.extra.is_empty()
        {
            return Err("无效的窗口振动报文".into());
        }
        let sender = self.seen(packet, address, true).await?;
        let id = format!("{}:{}", sender.id, packet.packet_no);
        let timestamp = unix_seconds();
        let fresh = self
            .db
            .insert(Record {
                id: id.clone(),
                from_id: sender.id.clone(),
                to_id: self.local().id,
                content: "[窗口振动]".into(),
                kind: 3,
                timestamp,
                status: 1,
                image: None,
                file: None,
            })
            .await?;
        if fresh {
            self.event(
                "message.received",
                json!({"id":id,"from":sender.id,"fromUser":sender,
                "content":"[窗口振动]","type":"knock","timestamp":timestamp}),
            )
            .await;
        }
        if packet.command & IPMSG_SENDCHECKOPT != 0 {
            self.send_to(
                Self::address(&sender)?,
                &self.wire(IPMSG_RECVMSG, &packet.packet_no.to_string(), None)?,
            )
            .await?;
        }
        Ok(())
    }
}
