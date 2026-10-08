use ipmsg_core::{
    config::ConfigStore,
    database::Database,
    image::{dib::decode_image, fragments::parse_fragment},
    network::{Event, Network},
    protocol::*,
    User,
};
use std::{
    io::Cursor,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::UdpSocket, sync::mpsc, time::timeout};

struct Fixture {
    root: PathBuf,
    db: Database,
    net: Arc<Network>,
    events: mpsc::Receiver<Event>,
    port: u16,
}
impl Fixture {
    async fn new(direct: Vec<SocketAddrV4>) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rust-image-send-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = Database::open(&root.join("messages.db")).unwrap();
        let config = Arc::new(ConfigStore::open(root.join("config.json")).unwrap());
        let raw = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        raw.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        let socket: std::net::UdpSocket = raw.into();
        let port = socket.local_addr().unwrap().port();
        let username = format!("test-{port}");
        let local = User {
            id: format!("{username}@fixture"),
            username,
            hostname: "fixture".into(),
            nickname: "测试".into(),
            group: "".into(),
            ip: "127.0.0.1".into(),
            port,
            status: "online".into(),
            version: "1".into(),
        };
        let (tx, events) = mpsc::channel(256);
        let net = Network::new(socket, local, config, db.clone(), tx, direct, vec![]).unwrap();
        net.start().await;
        Self {
            root,
            db,
            net,
            events,
            port,
        }
    }
    async fn event(&mut self, name: &str, seconds: u64) -> Event {
        timeout(Duration::from_secs(seconds), async {
            loop {
                let event = self.events.recv().await.unwrap();
                if event.event == name {
                    return event;
                }
            }
        })
        .await
        .unwrap()
    }
    async fn peer(&mut self) -> (UdpSocket, String) {
        let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        ack(&peer, self.port, IPMSG_BR_ENTRY, "对端", Some("组")).await;
        assert_eq!(mode(recv(&peer).await.0.command), IPMSG_ANSENTRY);
        let event = self.event("user.discovered", 3).await;
        (peer, event.payload["id"].as_str().unwrap().to_string())
    }
    async fn import(&self) -> ipmsg_core::image::ImageMetadata {
        let path = self.root.join("选中的图片.png");
        let image = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(1, 2, vec![255, 0, 0, 0, 0, 0, 255, 255]).unwrap(),
        );
        let mut data = Cursor::new(Vec::new());
        image.write_to(&mut data, image::ImageFormat::Png).unwrap();
        std::fs::write(&path, data.into_inner()).unwrap();
        self.net.import_image_path(path).await.unwrap()
    }
    async fn stop(self) {
        self.net.shutdown().await;
        self.db.shutdown().await;
        for n in 0..10 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(e) if n < 9 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(e) => panic!("{e}"),
            }
        }
    }
}
async fn recv(peer: &UdpSocket) -> (Packet, Vec<u8>) {
    let mut data = [0; 65536];
    let (n, _) = timeout(Duration::from_secs(4), peer.recv_from(&mut data))
        .await
        .unwrap()
        .unwrap();
    (parse_packet(&data[..n]).unwrap(), data[..n].to_vec())
}
async fn ack(peer: &UdpSocket, port: u16, command: u32, body: &str, extra: Option<&str>) {
    let bytes = encode_packet(90, "peer", "fixture", command, body, extra).unwrap();
    peer.send_to(&bytes, (Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
}

#[tokio::test]
async fn screenshot_import_validates_png_and_reclaims_unreferenced_assets() {
    let fixture = Fixture::new(vec![]).await;
    let pixels = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels.clone())
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let bytes = png.into_inner();
    for invalid in [
        vec![],
        b"\xff\xd8\xffnot-png".to_vec(),
        b"\x89PNG\r\n\x1a\nbroken".to_vec(),
        vec![0; 20 * 1024 * 1024 + 1],
    ] {
        assert!(fixture.net.import_screenshot(invalid).await.is_err());
    }
    let imported = fixture.net.import_screenshot(bytes.clone()).await.unwrap();
    assert_eq!((imported.width, imported.height), (2, 1));
    assert!(imported.file_name.starts_with("截图_"));
    let stored = fixture
        .net
        .read_image(&imported.asset_id, false)
        .await
        .unwrap();
    assert_eq!(image::load_from_memory(&stored).unwrap().to_rgba8(), pixels);
    assert!(fixture
        .db
        .history("peer".into(), fixture.net.local().id, 50, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(fixture.net.discard_image(&imported.asset_id).await.unwrap());
    assert!(fixture
        .net
        .read_image(&imported.asset_id, false)
        .await
        .is_err());
    fixture.net.shutdown().await;
    assert!(fixture.net.import_screenshot(bytes).await.is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn image_data_precedes_reference_and_wrong_source_ack_cannot_complete() {
    let mut f = Fixture::new(vec![]).await;
    let (peer, id) = f.peer().await;
    let asset = f.import().await;
    let sent = f.net.send_image(&id, &asset.asset_id).await.unwrap();
    let (first, _) = recv(&peer).await;
    assert_eq!(mode(first.command), IPMSG_SENDIMAGE);
    let part = parse_fragment(&first.body).unwrap();
    assert_eq!(part.count, 1);
    assert_eq!(part.image_id, sent.image_id);
    let decoded = decode_image(&part.data).unwrap();
    assert_eq!((decoded.width, decoded.height), (1, 2));
    let rogue = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    ack(
        &rogue,
        f.port,
        IPMSG_REPORT_RECVIMAGE,
        &format!("{}|1#", sent.image_id),
        None,
    )
    .await;
    let mut buffer = [0; 4096];
    assert!(
        timeout(Duration::from_millis(120), peer.recv_from(&mut buffer))
            .await
            .is_err()
    );
    ack(
        &peer,
        f.port,
        IPMSG_REPORT_RECVIMAGE,
        &format!("{}|1#", sent.image_id),
        None,
    )
    .await;
    let (reference, wire) = recv(&peer).await;
    assert_eq!(reference.command, IPMSG_SENDMSG | IPMSG_SENDCHECKOPT);
    assert!(wire.ends_with(&[0, 0]));
    assert!(decode_text(&reference.body, reference.command)
        .unwrap()
        .starts_with(&format!("/~#>{}<B~", sent.image_id)));
    let (retry, _) = recv(&peer).await;
    assert_eq!(retry.packet_no, reference.packet_no);
    ack(
        &peer,
        f.port,
        IPMSG_RECVMSG,
        &reference.packet_no.to_string(),
        None,
    )
    .await;
    assert_eq!(
        f.event("image.send_completed", 3).await.payload["messageId"],
        sent.message_id
    );
    assert!(!f.net.cancel_image(&sent.message_id));
    assert!(!f.net.discard_image(&asset.asset_id).await.unwrap());
    let rows = f.db.history(id, f.net.local().id, 50, 0).await.unwrap();
    assert_eq!(rows[0].status, 1);
    assert_eq!(rows[0].image.as_ref().unwrap(), &asset);
    f.stop().await;
}

#[tokio::test]
async fn cancellation_and_preview_discard_keep_history_assets_safe() {
    let mut f = Fixture::new(vec![]).await;
    let (_peer, id) = f.peer().await;
    let preview = f.import().await;
    assert!(f.net.discard_image(&preview.asset_id).await.unwrap());
    assert!(f.net.image_asset(&preview.asset_id).await.is_err());
    let asset = f.import().await;
    let sent = f.net.send_image(&id, &asset.asset_id).await.unwrap();
    assert!(f.net.cancel_image(&sent.message_id));
    let event = f.event("image.send_failed", 4).await;
    assert_eq!(event.payload["cancelled"], true);
    assert!(!f.net.discard_image(&asset.asset_id).await.unwrap());
    assert!(f.net.read_image(&asset.asset_id, false).await.is_ok());
    assert_eq!(
        f.db.history(id, f.net.local().id, 50, 0).await.unwrap()[0].status,
        3
    );
    f.stop().await;
}

#[tokio::test]
async fn two_rust_instances_exchange_inline_image_and_persist_receiver_history() {
    let mut sender = Fixture::new(vec![]).await;
    let mut receiver =
        Fixture::new(vec![SocketAddrV4::new(Ipv4Addr::LOCALHOST, sender.port)]).await;
    receiver.net.ui_ready().await.unwrap();
    let peer = sender.event("user.discovered", 3).await.payload["id"]
        .as_str()
        .unwrap()
        .to_string();
    receiver.event("user.discovered", 3).await;
    let asset = sender.import().await;
    let sent = sender.net.send_image(&peer, &asset.asset_id).await.unwrap();
    let received = receiver.event("message.received", 6).await;
    assert_eq!(received.payload["type"], "image");
    assert_eq!(
        sender.event("image.send_completed", 6).await.payload["messageId"],
        sent.message_id
    );
    let read = receiver
        .net
        .read_image(
            received.payload["image"]["assetId"].as_str().unwrap(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        image::load_from_memory(&read).unwrap().to_rgb8().as_raw(),
        &[255, 255, 255, 0, 0, 255]
    );
    assert_eq!(
        receiver
            .db
            .recent(receiver.net.local().id, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    sender.stop().await;
    receiver.stop().await;
}

#[tokio::test]
async fn queue_limit_and_queued_cancellation_are_bounded() {
    let mut f = Fixture::new(vec![]).await;
    let (_peer, id) = f.peer().await;
    let mut assets = Vec::new();
    for _ in 0..5 {
        assets.push(f.import().await);
    }
    let mut sends = Vec::new();
    for asset in assets.iter().take(4) {
        sends.push(f.net.send_image(&id, &asset.asset_id).await.unwrap());
    }
    assert!(f.net.send_image(&id, &assets[4].asset_id).await.is_err());
    assert!(f.net.discard_image(&assets[4].asset_id).await.unwrap());
    for send in &sends {
        assert!(f.net.cancel_image(&send.message_id));
    }
    let mut failed = std::collections::HashSet::new();
    for _ in 0..4 {
        let event = f.event("image.send_failed", 4).await;
        assert_eq!(event.payload["cancelled"], true);
        failed.insert(event.payload["messageId"].as_str().unwrap().to_string());
    }
    assert_eq!(failed.len(), 4);
    let rows = f.db.history(id, f.net.local().id, 20, 0).await.unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|row| row.status == 3));
    f.stop().await;
}

#[tokio::test]
async fn preview_quota_rejects_without_leaking_an_asset() {
    let f = Fixture::new(vec![]).await;
    let mut assets = Vec::new();
    for _ in 0..8 {
        assets.push(f.import().await);
    }
    assert!(f
        .net
        .import_image_path(f.root.join("选中的图片.png"))
        .await
        .is_err());
    assert_eq!(std::fs::read_dir(f.db.images_dir()).unwrap().count(), 8);
    for asset in assets {
        assert!(f.net.discard_image(&asset.asset_id).await.unwrap());
    }
    assert_eq!(std::fs::read_dir(f.db.images_dir()).unwrap().count(), 0);
    f.stop().await;
}

#[tokio::test]
async fn final_window_without_ack_fails_with_bounded_retries_and_no_reference() {
    let mut f = Fixture::new(vec![]).await;
    let (peer, id) = f.peer().await;
    let asset = f.import().await;
    let sent = f.net.send_image(&id, &asset.asset_id).await.unwrap();
    let failed = f.event("image.send_failed", 35).await;
    assert_eq!(failed.payload["messageId"], sent.message_id);
    let mut buffer = [0; 4096];
    let mut count = 0;
    while let Ok((n, _)) = peer.try_recv_from(&mut buffer) {
        count += 1;
        assert_eq!(
            mode(parse_packet(&buffer[..n]).unwrap().command),
            IPMSG_SENDIMAGE
        );
    }
    assert!(count > 0 && count <= 15);
    assert_eq!(
        f.db.history(id, f.net.local().id, 10, 0).await.unwrap()[0].status,
        3
    );
    f.stop().await;
}
