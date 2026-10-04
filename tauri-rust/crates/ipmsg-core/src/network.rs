use crate::{
    config::{parse_address, AppConfig, ConfigStore},
    database::{Database, Record},
    protocol::*,
    text::strip_feiq_font_suffix,
    User,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, watch},
    task::JoinHandle,
};

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub event: String,
    pub payload: Value,
}
struct Peer {
    user: User,
    flags: u32,
    profile_seen: bool,
    seen: Instant,
    probe: Option<Instant>,
    misses: u8,
}
struct Pending {
    id: String,
    peer: User,
    wire: Vec<u8>,
    attempts: u8,
    retry_at: Instant,
}

pub struct Network {
    socket: Arc<UdpSocket>,
    local: RwLock<User>,
    peers: Mutex<HashMap<String, Peer>>,
    pending: Mutex<HashMap<u32, Pending>>,
    unsupported_images: Mutex<BTreeSet<String>>,
    config: Arc<ConfigStore>,
    db: Database,
    events: mpsc::Sender<Event>,
    packet: AtomicU32,
    ready: AtomicBool,
    stopping: AtomicBool,
    cancel: watch::Sender<bool>,
    tasks: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
    send_gate: tokio::sync::Mutex<()>,
    cli_peers: Vec<SocketAddrV4>,
    broadcasts: Vec<Ipv4Addr>,
}
pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn endpoint_id(username: &str, hostname: &str, address: SocketAddrV4) -> String {
    let escape = |s: &str| {
        s.replace('%', "%25")
            .replace('@', "%40")
            .replace('#', "%23")
    };
    format!("{}@{}#{address}", escape(username), escape(hostname))
}

impl Network {
    // Wire identities are not globally unique: FeiQ and the C++ instance can
    // advertise the same username@host from different listening endpoints.
    fn resolve_peer_id(&self, packet: &Packet, address: SocketAddrV4) -> Result<String, String> {
        let exact = endpoint_id(&packet.username, &packet.hostname, address);
        let peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(
            mode(packet.command),
            IPMSG_BR_ENTRY | IPMSG_ANSENTRY | IPMSG_BR_ABSENCE
        ) || peers.contains_key(&exact)
        {
            return Ok(exact);
        }
        let ip = address.ip().to_string();
        let mut candidates = peers.values().filter(|peer| {
            peer.profile_seen
                && peer.user.username == packet.username
                && peer.user.hostname == packet.hostname
                && peer.user.ip == ip
        });
        match (candidates.next(), candidates.next()) {
            (Some(peer), None) => Ok(peer.user.id.clone()),
            (None, _) => Ok(exact),
            _ => Err(format!(
                "临时源端口{address}对应多个同名联系人，无法确定监听端点"
            )),
        }
    }

    fn diagnostic(&self, level: &str, message: String) {
        // Diagnostics must not block UDP reception if the UI/event pump is busy.
        let _ = self.events.try_send(Event {
            event: "network.diagnostic".into(),
            payload: json!({"level":level,"message":message}),
        });
    }

    pub fn new(
        socket: std::net::UdpSocket,
        local: User,
        config: Arc<ConfigStore>,
        db: Database,
        events: mpsc::Sender<Event>,
        cli_peers: Vec<SocketAddrV4>,
        broadcasts: Vec<Ipv4Addr>,
    ) -> Result<Arc<Self>, String> {
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let socket = UdpSocket::from_std(socket).map_err(|e| e.to_string())?;
        let (cancel, _) = watch::channel(false);
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            local: RwLock::new(local),
            peers: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            unsupported_images: Mutex::new(BTreeSet::new()),
            config,
            db,
            events,
            packet: AtomicU32::new(unix_seconds() as u32),
            ready: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            cancel,
            tasks: tokio::sync::Mutex::new(vec![]),
            send_gate: tokio::sync::Mutex::new(()),
            cli_peers,
            broadcasts,
        }))
    }
    pub fn local(&self) -> User {
        self.local.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn users(&self) -> Vec<User> {
        self.peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|p| p.user.clone())
            .collect()
    }
    fn next_packet(&self) -> u32 {
        loop {
            let value = self.packet.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
            if value != 0 {
                return value;
            }
        }
    }
    async fn event(&self, name: &str, payload: Value) {
        let _ = self
            .events
            .send(Event {
                event: name.into(),
                payload,
            })
            .await;
    }
    fn wire(&self, command: u32, body: &str, extra: Option<&str>) -> Result<Vec<u8>, String> {
        let local = self.local();
        encode_packet(
            self.next_packet(),
            &local.username,
            &local.hostname,
            command,
            body,
            extra,
        )
    }
    async fn send_to(&self, address: SocketAddrV4, wire: &[u8]) -> Result<(), String> {
        self.socket
            .send_to(wire, address)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    fn address(user: &User) -> Result<SocketAddrV4, String> {
        parse_address(&format!("{}:{}", user.ip, user.port))
    }
    fn direct_addresses(&self) -> Vec<SocketAddrV4> {
        let mut addresses: BTreeSet<_> = self.cli_peers.iter().copied().collect();
        addresses.extend(
            self.config
                .get()
                .direct_users
                .iter()
                .filter_map(|s| parse_address(s).ok()),
        );
        addresses.extend(self.users().iter().filter_map(|u| Self::address(u).ok()));
        addresses.into_iter().collect()
    }
    pub async fn start(self: &Arc<Self>) {
        let mut tasks = self.tasks.lock().await;
        if !tasks.is_empty() {
            return;
        }
        let me = self.clone();
        tasks.push(tokio::spawn(async move { me.receive_loop().await }));
        let me = self.clone();
        tasks.push(tokio::spawn(async move { me.maintenance().await }));
    }
    pub async fn ui_ready(&self) -> Result<(), String> {
        if !self.ready.swap(true, Ordering::AcqRel) {
            if let Err(error) = self.discover().await {
                self.ready.store(false, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    }
    pub async fn discover(&self) -> Result<(), String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        let local = self.local();
        let wire = self.wire(
            IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
            &local.nickname,
            Some(&local.group),
        )?;
        let mut destinations: BTreeSet<_> = self.direct_addresses().into_iter().collect();
        for ip in &self.broadcasts {
            destinations.insert(SocketAddrV4::new(*ip, 2425));
            destinations.insert(SocketAddrV4::new(*ip, local.port));
        }
        let mut sent = 0;
        let mut errors = vec![];
        for address in destinations {
            match self.send_to(address, &wire).await {
                Ok(()) => {
                    sent += 1;
                    self.diagnostic(
                        "DEBUG",
                        format!("Discovery TX target={address} bytes={}", wire.len()),
                    );
                }
                Err(e) => {
                    self.diagnostic("WARN", format!("Discovery TX failed target={address}: {e}"));
                    errors.push(format!("{address}: {e}"));
                }
            }
        }
        if sent == 0 && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(())
    }
    pub async fn apply_config(&self, config: &AppConfig) -> Result<(), String> {
        {
            let mut local = self.local.write().unwrap_or_else(|e| e.into_inner());
            local.nickname = if config.nickname.is_empty() {
                local.username.clone()
            } else {
                config.nickname.clone()
            };
            local.group = config.group.clone();
        }
        if self.ready.load(Ordering::Acquire) {
            self.discover().await?;
        }
        Ok(())
    }
    pub async fn send_message(&self, target: &str, content: &str) -> Result<String, String> {
        let _send = self.send_gate.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("程序正在退出".into());
        }
        if content.is_empty() {
            return Err("消息不能为空".into());
        }
        let (peer, flags) = self
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(target)
            .map(|p| (p.user.clone(), p.flags))
            .ok_or("未找到对端，请先刷新或直接添加用户")?;
        if self.pending.lock().unwrap_or_else(|e| e.into_inner()).len() >= 256 {
            return Err("待确认消息过多".into());
        }
        let local = self.local();
        let packet = self.next_packet();
        let mut command = IPMSG_SENDMSG | IPMSG_SENDCHECKOPT;
        if flags & (IPMSG_CAPUTF8OPT | IPMSG_UTF8OPT) != 0 {
            command |= IPMSG_UTF8OPT;
        }
        let wire = encode_packet(
            packet,
            &local.username,
            &local.hostname,
            command,
            content,
            None,
        )?;
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let id = format!("{ms}_{packet}");
        self.db
            .insert(Record {
                id: id.clone(),
                from_id: local.id,
                to_id: peer.id.clone(),
                content: content.into(),
                kind: 0,
                timestamp: unix_seconds(),
                status: 0,
            })
            .await?;
        // Persist and register before touching the socket: an ACK can arrive immediately.
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                packet,
                Pending {
                    id: id.clone(),
                    peer: peer.clone(),
                    wire: wire.clone(),
                    attempts: 1,
                    retry_at: Instant::now() + Duration::from_secs(2),
                },
            );
        if let Err(error) = self.send_to(Self::address(&peer)?, &wire).await {
            self.fail(packet, error).await;
        }
        Ok(id)
    }
    async fn fail(&self, packet: u32, error: String) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&packet);
        if let Some(pending) = pending {
            let _ = self.db.status(pending.id.clone(), 3).await;
            self.event(
                "message.failed",
                json!({"messageId":pending.id,"error":error}),
            )
            .await;
        }
    }
    async fn seen(
        &self,
        packet: &Packet,
        address: SocketAddrV4,
        announce: bool,
    ) -> Result<User, String> {
        let id = self.resolve_peer_id(packet, address)?;
        let previous = self
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|p| p.user.clone());
        let is_profile = matches!(
            mode(packet.command),
            IPMSG_BR_ENTRY | IPMSG_ANSENTRY | IPMSG_BR_ABSENCE
        );
        let nickname = if is_profile {
            decode_text(&packet.body, packet.command)?
        } else {
            previous
                .as_ref()
                .map(|u| u.nickname.clone())
                .unwrap_or_else(|| packet.username.clone())
        };
        let group = if is_profile {
            decode_text(&packet.extra, packet.command)?
        } else {
            previous
                .as_ref()
                .map(|u| u.group.clone())
                .unwrap_or_default()
        };
        // A text sender may use a temporary UDP source port. Only discovery
        // messages establish a new listening endpoint for an already-known peer.
        let port = if !is_profile {
            previous
                .as_ref()
                .filter(|user| user.ip == address.ip().to_string())
                .map(|user| user.port)
                .unwrap_or(address.port())
        } else {
            address.port()
        };
        let user = User {
            id: id.clone(),
            nickname: if nickname.is_empty() {
                packet.username.clone()
            } else {
                nickname
            },
            username: packet.username.clone(),
            hostname: packet.hostname.clone(),
            group,
            ip: address.ip().to_string(),
            port,
            status: if (is_profile && packet.command & IPMSG_ABSENCEOPT != 0)
                || (!is_profile && previous.as_ref().is_some_and(|user| user.status == "away"))
            {
                "away"
            } else {
                "online"
            }
            .into(),
            version: packet.version.clone(),
        };
        let changed = previous.as_ref() != Some(&user);
        {
            let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
            if !peers.contains_key(&id) && peers.len() >= 4096 {
                return Err("通讯录已达本版本上限".into());
            }
            let peer = peers.entry(id).or_insert(Peer {
                user: user.clone(),
                flags: packet.command,
                profile_seen: is_profile,
                seen: Instant::now(),
                probe: None,
                misses: 0,
            });
            peer.profile_seen |= is_profile;
            peer.user = user.clone();
            peer.seen = Instant::now();
            peer.misses = 0;
            peer.flags |= packet.command & (IPMSG_CAPUTF8OPT | IPMSG_UTF8OPT);
        }
        if is_profile {
            self.diagnostic(
                "DEBUG",
                format!(
                    "Discovery RX command=0x{:x} source={address} peer={}",
                    packet.command, user.id
                ),
            );
        }
        if announce && changed {
            self.event("user.discovered", json!(user)).await;
        }
        Ok(user)
    }
    async fn incoming(
        &self,
        sender: User,
        id: String,
        content: String,
        command: u32,
    ) -> Result<(), String> {
        let timestamp = unix_seconds();
        let is_new = self
            .db
            .insert(Record {
                id: id.clone(),
                from_id: sender.id.clone(),
                to_id: self.local().id,
                content: content.clone(),
                kind: 0,
                timestamp,
                status: 1,
            })
            .await?;
        if is_new {
            self.event(
                "message.received",
                json!({"id":id,"from":sender.id,"fromUser":sender,
                "content":content,"type":"text","timestamp":timestamp,"command":command}),
            )
            .await;
        }
        Ok(())
    }
    async fn handle(&self, packet: Packet, address: SocketAddrV4) -> Result<(), String> {
        let local = self.local();
        if packet.username == local.username
            && packet.hostname == local.hostname
            && address.port() == local.port
        {
            return Ok(());
        }
        let peer_id = self.resolve_peer_id(&packet, address)?;
        match mode(packet.command) {
            IPMSG_BR_ENTRY | IPMSG_ANSENTRY | IPMSG_BR_ABSENCE => {
                self.seen(&packet, address, true).await?;
                if mode(packet.command) == IPMSG_BR_ENTRY {
                    self.send_to(
                        address,
                        &self.wire(
                            IPMSG_ANSENTRY | IPMSG_CAPUTF8OPT,
                            &local.nickname,
                            Some(&local.group),
                        )?,
                    )
                    .await?;
                }
            }
            IPMSG_BR_EXIT => {
                let user = {
                    let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
                    peers
                        .get_mut(&peer_id)
                        .filter(|p| {
                            p.user.ip == address.ip().to_string() && p.user.port == address.port()
                        })
                        .map(|p| {
                            p.user.status = "offline".into();
                            p.user.clone()
                        })
                };
                if let Some(user) = user {
                    self.event(
                        "user.status_changed",
                        json!({"user":user,"status":"offline"}),
                    )
                    .await;
                }
            }
            IPMSG_SENDMSG => {
                if packet.command & IPMSG_ENCRYPTOPT != 0 {
                    return Ok(());
                }
                let sender = self.seen(&packet, address, true).await?;
                let unsupported_file = packet.command & IPMSG_FILEATTACHOPT != 0;
                let text = decode_text(&packet.body, packet.command)?;
                let content = if unsupported_file {
                    "[收到文件：Rust核心版暂不支持，请使用原版接收]".into()
                } else if text.starts_with("/~#>") {
                    "[收到图片：Rust核心版暂不支持，请使用原版查看]".into()
                } else {
                    strip_feiq_font_suffix(&text)
                };
                let id = format!("{}:{}", sender.id, packet.packet_no);
                let reply_to = Self::address(&sender)?;
                self.incoming(
                    sender,
                    id,
                    content,
                    if unsupported_file { 0 } else { packet.command },
                )
                .await?;
                // Never ACK a file invitation as accepted; normal text retries still get ACKs.
                if !unsupported_file && packet.command & IPMSG_SENDCHECKOPT != 0 {
                    self.send_to(
                        reply_to,
                        &self.wire(IPMSG_RECVMSG, &packet.packet_no.to_string(), None)?,
                    )
                    .await?;
                }
            }
            IPMSG_RECVMSG => {
                let ack = std::str::from_utf8(&packet.body)
                    .map_err(|_| "无效回执")?
                    .parse::<u32>()
                    .map_err(|_| "无效回执包号")?;
                let pending = {
                    let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                    if pending.get(&ack).is_some_and(|p| {
                        p.peer.username == packet.username
                            && p.peer.hostname == packet.hostname
                            && p.peer.ip == address.ip().to_string()
                            && p.peer.port == address.port()
                    }) {
                        pending.remove(&ack)
                    } else {
                        None
                    }
                };
                if let Some(pending) = pending {
                    self.seen(&packet, address, true).await?;
                    match self.db.status(pending.id.clone(), 1).await {
                        Ok(()) => {
                            self.event(
                                "message.ack",
                                json!({"messageId":pending.id,"packetNo":ack,"from":peer_id}),
                            )
                            .await
                        }
                        Err(error) => {
                            self.event(
                                "message.failed",
                                json!({"messageId":pending.id,"error":error}),
                            )
                            .await
                        }
                    }
                }
            }
            IPMSG_SENDIMAGE => {
                // One clear placeholder per unsupported image; no binary decode or image ACK.
                if let Some(id) = packet
                    .body
                    .get(..8)
                    .filter(|id| id.iter().all(u8::is_ascii_hexdigit))
                {
                    let key = format!("image:{peer_id}:{}", String::from_utf8_lossy(id));
                    let first = {
                        let mut images = self
                            .unsupported_images
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if images.len() >= 256 {
                            false
                        } else {
                            images.insert(key.clone())
                        }
                    };
                    if first {
                        let sender = self.seen(&packet, address, true).await?;
                        self.incoming(
                            sender,
                            key,
                            "[收到图片：Rust核心版暂不支持，请使用原版查看]".into(),
                            0,
                        )
                        .await?;
                    }
                }
            }
            IPMSG_GETINFO => {
                self.send_to(
                    address,
                    &self.wire(IPMSG_SENDINFO, "SpeedIpMsg Rust Preview 0.1.0", None)?,
                )
                .await?;
            }
            _ => {}
        }
        Ok(())
    }
    async fn receive_loop(self: Arc<Self>) {
        let mut cancel = self.cancel.subscribe();
        let mut buffer = vec![0; 65536];
        loop {
            if *cancel.borrow() {
                break;
            }
            tokio::select! {
                _ = cancel.changed() => break,
                received = self.socket.recv_from(&mut buffer) => match received {
                    Ok((size, SocketAddr::V4(address))) => {
                        match parse_packet(&buffer[..size]) {
                            Ok(packet) => {
                                let command = packet.command;
                                if let Err(error) = self.handle(packet, address).await {
                                    self.diagnostic("DEBUG", format!("Packet handling failed source={address} command=0x{command:x}: {error}"));
                                }
                            }
                            Err(error) => self.diagnostic("DEBUG", format!("Packet rejected source={address} bytes={size}: {error}")),
                        }
                    },
                    Ok(_) => {},
                    Err(error) if matches!(error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionRefused |
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock |
                        std::io::ErrorKind::NetworkUnreachable | std::io::ErrorKind::HostUnreachable |
                        std::io::ErrorKind::TimedOut) => {
                        // Windows reports an ICMP port-unreachable from a previous
                        // UDP probe as WSAECONNRESET (10054). The listening socket is
                        // still usable; exiting here disables all future reception.
                        self.diagnostic("DEBUG", format!("UDP receive recoverable error; continuing: {error}"));
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => {
                        self.diagnostic("ERROR", format!("UDP receive loop stopped: {error}"));
                        break;
                    }
                }
            }
        }
    }
    async fn maintenance(self: Arc<Self>) {
        let mut cancel = self.cancel.subscribe();
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        let mut last_probe = Instant::now();
        loop {
            if *cancel.borrow() {
                break;
            }
            tokio::select! { _ = cancel.changed() => break, _ = timer.tick() => {} }
            let now = Instant::now();
            let (retry, expired) = {
                let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                let mut retry = vec![];
                let mut expired = vec![];
                for (&packet, item) in pending.iter_mut().filter(|(_, p)| p.retry_at <= now) {
                    if item.attempts >= 3 {
                        expired.push(packet);
                    } else {
                        item.attempts += 1;
                        item.retry_at = now + Duration::from_secs(2);
                        retry.push((packet, item.peer.clone(), item.wire.clone()));
                    }
                }
                (retry, expired)
            };
            for packet in expired {
                self.fail(packet, "对方未确认消息，可能已离线".into()).await;
            }
            for (packet, user, wire) in retry {
                if let Ok(address) = Self::address(&user) {
                    if let Err(error) = self.send_to(address, &wire).await {
                        self.fail(packet, error).await;
                    }
                }
            }
            if self.ready.load(Ordering::Acquire)
                && now.duration_since(last_probe) >= Duration::from_secs(60)
            {
                last_probe = now;
                let offline = {
                    let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
                    let mut offline = vec![];
                    for p in peers.values_mut() {
                        if p.probe.is_some_and(|probe| p.seen <= probe) {
                            p.misses = p.misses.saturating_add(1);
                        } else {
                            p.misses = 0;
                        }
                        p.probe = Some(now);
                        if p.misses >= 3 && p.user.status != "offline" {
                            p.user.status = "offline".into();
                            offline.push(p.user.clone());
                        }
                    }
                    offline
                };
                for user in offline {
                    self.event(
                        "user.status_changed",
                        json!({"user":user,"status":"offline"}),
                    )
                    .await;
                }
                let local = self.local();
                if let Ok(wire) = self.wire(
                    IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
                    &local.nickname,
                    Some(&local.group),
                ) {
                    for address in self.direct_addresses() {
                        let _ = self.send_to(address, &wire).await;
                    }
                }
            }
        }
    }
    pub async fn shutdown(&self) {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        let _send = self.send_gate.lock().await;
        let local = self.local();
        if let Ok(wire) = self.wire(IPMSG_BR_EXIT, &local.nickname, None) {
            for address in self.direct_addresses() {
                let _ = self.send_to(address, &wire).await;
            }
        }
        self.cancel.send_replace(true);
        for mut task in self.tasks.lock().await.drain(..) {
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        let pending: Vec<_> = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect();
        for packet in pending {
            self.fail(packet, "程序退出，消息未确认".into()).await;
        }
    }
}
