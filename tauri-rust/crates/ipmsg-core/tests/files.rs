use ipmsg_core::{
    config::ConfigStore,
    database::Database,
    network::{Event, Network},
    protocol::*,
    User,
};
use std::{
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::mpsc,
    time::timeout,
};
struct Fixture {
    root: PathBuf,
    db: Database,
    net: Arc<Network>,
    events: mpsc::Receiver<Event>,
    port: u16,
}
impl Fixture {
    async fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rust-file-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = Database::open(&root.join("messages.db")).unwrap();
        let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        let user = User {
            id: format!("rust-{port}@fixture"),
            username: format!("rust-{port}"),
            hostname: "fixture".into(),
            nickname: "测试".into(),
            group: "".into(),
            ip: "127.0.0.1".into(),
            port,
            status: "online".into(),
            version: "1".into(),
        };
        let (tx, events) = mpsc::channel(256);
        let config = Arc::new(ConfigStore::open(root.join("config.json")).unwrap());
        let net = Network::new(socket, user, config, db.clone(), tx, vec![], vec![]).unwrap();
        net.enable_files(root.join("downloads")).await.unwrap();
        net.start().await;
        Self {
            root,
            db,
            net,
            events,
            port,
        }
    }
    async fn event(&mut self, name: &str) -> Event {
        timeout(Duration::from_secs(8), async {
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
    async fn state(&mut self, id: &str, phase: &str) -> Event {
        timeout(Duration::from_secs(8), async {
            loop {
                let e = self.events.recv().await.unwrap();
                if e.event == "file.updated"
                    && e.payload["messageId"] == id
                    && e.payload["file"]["state"] == phase
                {
                    return e;
                }
            }
        })
        .await
        .unwrap()
    }
    async fn peer(&mut self) -> (UdpSocket, String) {
        let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        peer.send_to(
            &encode_packet(1, "legacy", "fixture", IPMSG_BR_ENTRY, "飞秋", Some("")).unwrap(),
            (Ipv4Addr::LOCALHOST, self.port),
        )
        .await
        .unwrap();
        let e = self.event("user.discovered").await;
        let mut b = [0; 4096];
        let n = peer.recv(&mut b).await.unwrap();
        assert_ne!(
            parse_packet(&b[..n]).unwrap().command & IPMSG_FILEATTACHOPT,
            0
        );
        (peer, e.payload["id"].as_str().unwrap().into())
    }
    async fn stop(self) {
        self.net.shutdown().await;
        self.db.shutdown().await;
        assert!(self.root.starts_with(std::env::temp_dir()));
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rust-file-test-"));
        for n in 0..20 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(_) if n < 19 => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(e) => panic!("{e}"),
            }
        }
    }
}
async fn packet(peer: &UdpSocket) -> Packet {
    let mut bytes = [0; 4096];
    let n = timeout(Duration::from_secs(5), peer.recv(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    parse_packet(&bytes[..n]).unwrap()
}

#[tokio::test]
async fn rust_tcp_roundtrip_zero_file_same_name_and_history() {
    let mut a = Fixture::new().await;
    let mut b = Fixture::new().await;
    // The file manager sends a real UDP invitation to the receiver's listening endpoint.
    let peer = User {
        id: format!("{}@fixture#127.0.0.1:{}", b.net.local().username, b.port),
        ..b.net.local()
    };
    for (n, data) in [
        (0..1024 * 1024 + 17)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>(),
        b"second payload".to_vec(),
        Vec::new(),
    ]
    .into_iter()
    .enumerate()
    {
        let path = a.root.join("中文.txt");
        std::fs::write(&path, &data).unwrap();
        let selected = a
            .net
            .file_transfers()
            .unwrap()
            .select_path(path)
            .await
            .unwrap();
        let sent = a
            .net
            .file_transfers()
            .unwrap()
            .send(peer.clone(), &selected.selection_id)
            .await
            .unwrap();
        let offer = b.event("message.received").await;
        assert_eq!(offer.payload["type"], "file");
        let id = offer.payload["id"].as_str().unwrap().to_string();
        assert!(b.db.file_path(id.clone()).await.is_err());
        b.net.file_transfers().unwrap().accept(&id).await.unwrap();
        let done = b.state(&id, "completed").await;
        assert_eq!(done.payload["file"]["transferred"], data.len() as u64);
        a.state(&sent.id, "completed").await;
        let saved = b.db.file_path(id.clone()).await.unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), data);
        assert_eq!(
            saved.file_name().unwrap().to_string_lossy(),
            if n == 0 {
                "中文.txt".into()
            } else {
                format!("中文 ({n}).txt")
            }
        );
        let history =
            b.db.history(
                offer.payload["from"].as_str().unwrap().into(),
                b.net.local().id,
                50,
                0,
            )
            .await
            .unwrap();
        assert!(history
            .iter()
            .any(|r| r.id == id && r.status == 2 && r.file.as_ref().unwrap().has_local_file));
    }
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn legacy_offset_request_split_header_and_wrong_identity() {
    let mut fixture = Fixture::new().await;
    let (peer, target) = fixture.peer().await;
    let path = fixture.root.join("sample.bin");
    std::fs::write(&path, b"0123456789").unwrap();
    let selection = fixture
        .net
        .file_transfers()
        .unwrap()
        .select_path(path)
        .await
        .unwrap();
    let sent = fixture
        .net
        .send_file(&target, &selection.selection_id)
        .await
        .unwrap();
    let invitation = packet(&peer).await;
    assert_eq!(invitation.command & IPMSG_UTF8OPT, 0);
    assert_ne!(invitation.command & IPMSG_FILEATTACHOPT, 0);
    let mut bad = TcpStream::connect((Ipv4Addr::LOCALHOST, fixture.port))
        .await
        .unwrap();
    bad.write_all(
        &encode_packet(
            9,
            "wrong",
            "fixture",
            IPMSG_GETFILEDATA,
            &format!("{:x}:1:0:", invitation.packet_no),
            None,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let mut output = Vec::new();
    timeout(Duration::from_secs(3), bad.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    assert!(output.is_empty());
    let mut tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, fixture.port))
        .await
        .unwrap();
    let request = encode_packet(
        10,
        "legacy",
        "fixture",
        IPMSG_GETFILEDATA,
        &format!("{:x}:1:4:", invitation.packet_no),
        None,
    )
    .unwrap();
    tcp.write_all(&request[..12]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    tcp.write_all(&request[12..]).await.unwrap();
    output.clear();
    tcp.read_to_end(&mut output).await.unwrap();
    assert_eq!(output, b"456789");
    fixture.state(&sent.id, "completed").await;
    fixture.stop().await;
}

#[tokio::test]
async fn rejected_invitation_never_connects_and_clear_deduplicates_retries() {
    let mut fixture = Fixture::new().await;
    let (peer, _) = fixture.peer().await;
    let tcp = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
    let wire = encode_packet(
        500,
        "legacy",
        "fixture",
        IPMSG_SENDMSG | IPMSG_FILEATTACHOPT | IPMSG_SENDCHECKOPT,
        "",
        Some("0:CON.txt:4:0:1:\x07"),
    )
    .unwrap();
    peer.send_to(&wire, (Ipv4Addr::LOCALHOST, fixture.port))
        .await
        .unwrap();
    let offer = fixture.event("message.received").await;
    let id = offer.payload["id"].as_str().unwrap().to_string();
    assert_eq!(offer.payload["file"]["fileName"], "_CON.txt");
    assert_eq!(mode(packet(&peer).await.command), IPMSG_RECVMSG);
    fixture
        .net
        .file_transfers()
        .unwrap()
        .cancel(&id, true)
        .await
        .unwrap();
    let release = packet(&peer).await;
    assert_eq!(mode(release.command), IPMSG_RELEASEFILES);
    assert_eq!(release.body, b"500");
    assert!(timeout(Duration::from_millis(100), tcp.accept())
        .await
        .is_err());
    fixture
        .db
        .clear(None, fixture.net.local().id)
        .await
        .unwrap();
    peer.send_to(&wire, (Ipv4Addr::LOCALHOST, fixture.port))
        .await
        .unwrap();
    let _ = packet(&peer).await;
    assert!(fixture
        .db
        .recent(fixture.net.local().id, 100)
        .await
        .unwrap()
        .is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn truncated_download_and_cancel_remove_partial_files() {
    for cancel in [false, true] {
        let mut fixture = Fixture::new().await;
        let (peer, _) = fixture.peer().await;
        let listener = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
        let wire = encode_packet(
            600,
            "legacy",
            "fixture",
            IPMSG_SENDMSG | IPMSG_FILEATTACHOPT,
            "",
            Some("0:../bad.exe:10000:0:1:\x07"),
        )
        .unwrap();
        peer.send_to(&wire, (Ipv4Addr::LOCALHOST, fixture.port))
            .await
            .unwrap();
        let offer = fixture.event("message.received").await;
        let id = offer.payload["id"].as_str().unwrap().to_string();
        fixture
            .net
            .file_transfers()
            .unwrap()
            .accept(&id)
            .await
            .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        let _ = socket.read(&mut buffer).await.unwrap();
        socket.write_all(b"partial").await.unwrap();
        if cancel {
            fixture
                .net
                .file_transfers()
                .unwrap()
                .cancel(&id, false)
                .await
                .unwrap();
            fixture.state(&id, "cancelled").await;
        } else {
            drop(socket);
            fixture.state(&id, "failed").await;
        }
        fixture.net.shutdown().await;
        for _ in 0..40 {
            if std::fs::read_dir(fixture.root.join("downloads"))
                .unwrap()
                .next()
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(std::fs::read_dir(fixture.root.join("downloads"))
            .unwrap()
            .next()
            .is_none());
        fixture.stop().await;
    }
}

#[tokio::test]
async fn selections_and_tcp_binding_are_bounded() {
    let fixture = Fixture::new().await;
    let manager = fixture.net.file_transfers().unwrap();
    assert!(manager.select_path(fixture.root.clone()).await.is_err());
    let path = fixture.root.join("file.txt");
    std::fs::write(&path, b"test").unwrap();
    let mut ids = Vec::new();
    for _ in 0..8 {
        ids.push(
            manager
                .select_path(path.clone())
                .await
                .unwrap()
                .selection_id,
        );
    }
    assert!(manager.select_path(path.clone()).await.is_err());
    manager.discard(&ids[0]);
    assert!(manager.select_path(path).await.is_ok());
    assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, fixture.port))
        .await
        .is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn source_change_and_cancel_prevent_later_tcp_reads() {
    let mut fixture = Fixture::new().await;
    let (peer, target) = fixture.peer().await;
    let manager = fixture.net.file_transfers().unwrap();
    for changed in [true, false] {
        let path = fixture.root.join("source.txt");
        std::fs::write(&path, b"original").unwrap();
        let selection = manager.select_path(path.clone()).await.unwrap();
        let sent = fixture
            .net
            .send_file(&target, &selection.selection_id)
            .await
            .unwrap();
        let invitation = packet(&peer).await;
        if changed {
            std::fs::write(&path, b"replaced content").unwrap();
        } else {
            manager.cancel(&sent.id, false).await.unwrap();
        }
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, fixture.port))
            .await
            .unwrap();
        socket
            .write_all(
                &encode_packet(
                    12,
                    "legacy",
                    "fixture",
                    IPMSG_GETFILEDATA,
                    &format!("{:x}:1:0:", invitation.packet_no),
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        fixture
            .state(&sent.id, if changed { "failed" } else { "cancelled" })
            .await;
    }
    fixture.stop().await;
}

#[tokio::test]
async fn unacknowledged_file_offer_times_out_and_restart_keeps_failure() {
    let mut fixture = Fixture::new().await;
    let (peer, target) = fixture.peer().await;
    let path = fixture.root.join("file.txt");
    std::fs::write(&path, b"123").unwrap();
    let manager = fixture.net.file_transfers().unwrap();
    let selection = manager.select_path(path).await.unwrap();
    let sent = fixture
        .net
        .send_file(&target, &selection.selection_id)
        .await
        .unwrap();
    let first = packet(&peer).await;
    // A wrong port cannot acknowledge the offer.
    let wrong = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    wrong
        .send_to(
            &encode_packet(
                15,
                "legacy",
                "fixture",
                IPMSG_RECVMSG,
                &first.packet_no.to_string(),
                None,
            )
            .unwrap(),
            (Ipv4Addr::LOCALHOST, fixture.port),
        )
        .await
        .unwrap();
    fixture.state(&sent.id, "failed").await;
    assert_eq!(packet(&peer).await.packet_no, first.packet_no);
    assert_eq!(packet(&peer).await.packet_no, first.packet_no);
    fixture.net.shutdown().await;
    fixture.db.shutdown().await;
    let db = Database::open(&fixture.root.join("messages.db")).unwrap();
    let history = db
        .history(target, fixture.net.local().id, 50, 0)
        .await
        .unwrap();
    assert_eq!(history[0].file.as_ref().unwrap().state, "failed");
    assert_eq!(history[0].status, 3);
    db.shutdown().await;
    fixture.stop().await;
}
