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

fn file_endpoint() -> std::net::UdpSocket {
    // Windows TCP and UDP have different excluded ephemeral port ranges.
    // Ask TCP for an available port, then reserve the same UDP port before
    // handing it to the fixture. Never change the application's fixed-port behavior.
    for _ in 0..1024 {
        let tcp = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        match std::net::UdpSocket::bind(tcp.local_addr().unwrap()) {
            Ok(udp) => return udp,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue
            }
            Err(error) => panic!("cannot allocate test endpoint: {error}"),
        }
    }
    panic!("no shared TCP/UDP loopback test port available");
}
struct Fixture {
    root: PathBuf,
    db: Database,
    net: Arc<Network>,
    events: mpsc::Receiver<Event>,
    port: u16,
}
impl Fixture {
    async fn restart(self) -> Self {
        let Self {
            root,
            db,
            net,
            events,
            port,
        } = self;
        let user = net.local();
        net.shutdown().await;
        db.shutdown().await;
        drop(net);
        drop(db);
        drop(events);
        let db = Database::open(&root.join("messages.db")).unwrap();
        let config = Arc::new(ConfigStore::open(root.join("config.json")).unwrap());
        let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let (tx, events) = mpsc::channel(256);
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
        let socket = file_endpoint();
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
        let socket = file_endpoint();
        socket.set_nonblocking(true).unwrap();
        let peer = UdpSocket::from_std(socket).unwrap();
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
        (0..8 * 1024 * 1024 + 17)
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
            fixture.state(&id, "paused").await;
            fixture
                .net
                .file_transfers()
                .unwrap()
                .cancel(&id, false)
                .await
                .unwrap();
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
    let folder = manager.select_path(fixture.root.clone()).await.unwrap();
    assert!(folder.is_directory);
    manager.discard(&folder.selection_id);
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

#[tokio::test]
async fn directory_roundtrip_preserves_nested_and_empty_folders() {
    let mut a = Fixture::new().await;
    let mut b = Fixture::new().await;
    let root = a.root.join("资料");
    std::fs::create_dir_all(root.join("子目录")).unwrap();
    std::fs::create_dir(root.join("空目录")).unwrap();
    std::fs::write(root.join("子目录/中文.txt"), b"folder contents").unwrap();
    std::fs::write(root.join("empty.txt"), b"").unwrap();
    let peer = User {
        id: format!("{}@fixture#127.0.0.1:{}", b.net.local().username, b.port),
        ..b.net.local()
    };
    for _ in 0..2 {
        let selection = a
            .net
            .file_transfers()
            .unwrap()
            .select_path(root.clone())
            .await
            .unwrap();
        assert!(selection.is_directory);
        let sent = a
            .net
            .file_transfers()
            .unwrap()
            .send(peer.clone(), &selection.selection_id)
            .await
            .unwrap();
        let event = b.event("message.received").await;
        let id = event.payload["id"].as_str().unwrap();
        assert_eq!(event.payload["file"]["isDirectory"], true);
        b.net.file_transfers().unwrap().accept(id).await.unwrap();
        b.state(id, "completed").await;
        a.state(&sent.id, "completed").await;
        let path = b.db.file_path(id.into()).await.unwrap();
        assert!(path.join("空目录").is_dir());
        assert_eq!(
            std::fs::read(path.join("子目录/中文.txt")).unwrap(),
            b"folder contents"
        );
    }
    assert!(b.root.join("downloads/资料 (1)").is_dir());
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn interrupted_file_resumes_after_restart_from_actual_disk_offset() {
    let mut receiver = Fixture::new().await;
    let (peer, _) = receiver.peer().await;
    let listener = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
    let offer = encode_packet(
        777,
        "legacy",
        "fixture",
        IPMSG_SENDMSG | IPMSG_FILEATTACHOPT,
        "",
        Some("1:resume.txt:a:0:1:\x07"),
    )
    .unwrap();
    peer.send_to(&offer, (Ipv4Addr::LOCALHOST, receiver.port))
        .await
        .unwrap();
    let event = receiver.event("message.received").await;
    let id = event.payload["id"].as_str().unwrap().to_owned();
    receiver
        .net
        .file_transfers()
        .unwrap()
        .accept(&id)
        .await
        .unwrap();
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = [0; 4096];
    socket.read(&mut request).await.unwrap();
    socket.write_all(b"abcd").await.unwrap();
    drop(socket);
    receiver.state(&id, "paused").await;
    receiver = receiver.restart().await;
    receiver
        .net
        .file_transfers()
        .unwrap()
        .accept(&id)
        .await
        .unwrap();
    let (mut socket, _) = listener.accept().await.unwrap();
    let n = socket.read(&mut request).await.unwrap();
    assert_eq!(parse_packet(&request[..n]).unwrap().body, b"309:1:4:");
    socket.write_all(b"efghij").await.unwrap();
    drop(socket);
    receiver.state(&id, "completed").await;
    assert_eq!(
        std::fs::read(receiver.db.file_path(id.clone()).await.unwrap()).unwrap(),
        b"abcdefghij"
    );
    assert!(receiver.db.receive_checkpoint(id).await.unwrap().is_none());
    receiver.stop().await;
}

#[tokio::test]
async fn parallel_getfiledata_requests_do_not_share_file_cursors() {
    let mut sender = Fixture::new().await;
    let (peer, target) = sender.peer().await;
    let data: Vec<u8> = (0..8 * 1024 * 1024 + 123)
        .map(|n| ((n / 65536 + n % 251) % 256) as u8)
        .collect();
    let path = sender.root.join("parallel.bin");
    std::fs::write(&path, &data).unwrap();
    let selection = sender
        .net
        .file_transfers()
        .unwrap()
        .select_path(path)
        .await
        .unwrap();
    sender
        .net
        .send_file(&target, &selection.selection_id)
        .await
        .unwrap();
    let offer = packet(&peer).await;
    let read = |offset: u64| async move {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, sender.port))
            .await
            .unwrap();
        socket
            .write_all(
                &encode_packet(
                    800 + offset as u32,
                    "legacy",
                    "fixture",
                    IPMSG_GETFILEDATA,
                    &format!("{:x}:1:{offset:x}:", offer.packet_no),
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await.unwrap();
        bytes
    };
    let (one, two) = tokio::join!(read(0), read(98765));
    assert_eq!(one, data);
    assert_eq!(two, &data[98765..]);
    sender.stop().await;
}

#[tokio::test]
async fn simultaneous_large_bidirectional_downloads_finish_without_slot_deadlock() {
    let mut a = Fixture::new().await;
    let mut b = Fixture::new().await;
    let data_a: Vec<u8> = (0..8 * 1024 * 1024 + 7).map(|n| (n % 251) as u8).collect();
    let data_b: Vec<u8> = (0..9 * 1024 * 1024 + 31).map(|n| (n % 239) as u8).collect();
    for (from, to, data) in [(&a, &b, &data_a), (&b, &a, &data_b)] {
        let path = from.root.join("双向.bin");
        std::fs::write(&path, data).unwrap();
        let manager = from.net.file_transfers().unwrap();
        let selection = manager.select_path(path).await.unwrap();
        manager
            .send(
                User {
                    id: format!("{}@fixture#127.0.0.1:{}", to.net.local().username, to.port),
                    ..to.net.local()
                },
                &selection.selection_id,
            )
            .await
            .unwrap();
    }
    let id_a = a.event("message.received").await.payload["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let id_b = b.event("message.received").await.payload["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let manager_a = a.net.file_transfers().unwrap();
    let manager_b = b.net.file_transfers().unwrap();
    let (one, two) = tokio::join!(manager_a.accept(&id_a), manager_b.accept(&id_b));
    one.unwrap();
    two.unwrap();
    tokio::join!(a.state(&id_a, "completed"), b.state(&id_b, "completed"));
    assert_eq!(
        std::fs::read(a.db.file_path(id_a).await.unwrap()).unwrap(),
        data_b
    );
    assert_eq!(
        std::fs::read(b.db.file_path(id_b).await.unwrap()).unwrap(),
        data_a
    );
    for fixture in [&a, &b] {
        assert_eq!(
            std::fs::read_dir(fixture.root.join("downloads"))
                .unwrap()
                .count(),
            1
        );
    }
    a.stop().await;
    b.stop().await;
}

async fn incoming_test_file(receiver: &mut Fixture, peer: &UdpSocket, size: usize) -> String {
    peer.send_to(
        &encode_packet(
            900,
            "legacy",
            "fixture",
            IPMSG_SENDMSG | IPMSG_FILEATTACHOPT,
            "",
            Some(&format!("1:parallel.bin:{size:x}:0:1:\x07")),
        )
        .unwrap(),
        (Ipv4Addr::LOCALHOST, receiver.port),
    )
    .await
    .unwrap();
    let id = receiver.event("message.received").await.payload["id"]
        .as_str()
        .unwrap()
        .to_owned();
    receiver
        .net
        .file_transfers()
        .unwrap()
        .accept(&id)
        .await
        .unwrap();
    id
}

async fn download_request(listener: &TcpListener) -> (TcpStream, usize) {
    download_request_for(listener, 1).await
}

async fn download_request_for(listener: &TcpListener, file_id: u32) -> (TcpStream, usize) {
    timeout(Duration::from_secs(8), async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0 && bytes.len() + n <= 4096);
            bytes.extend_from_slice(&buffer[..n]);
            if let Ok((_, original, file, offset)) = ipmsg_core::file::protocol::request(&bytes) {
                assert_eq!((original, file), (900, file_id));
                return (socket, offset as usize);
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn parallel_pause_keeps_only_contiguous_prefix_and_resumes_exact_bytes() {
    let mut receiver = Fixture::new().await;
    let (peer, _) = receiver.peer().await;
    let listener = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
    let data: Vec<u8> = (0..4 * 1024 * 1024 + 4096)
        .map(|n| ((n / 65536 + n % 251) % 256) as u8)
        .collect();
    let chunk = data.len() / 4;
    let id = incoming_test_file(&mut receiver, &peer, data.len()).await;
    let mut sockets = Vec::new();
    for _ in 0..4 {
        sockets.push(download_request(&listener).await);
    }
    sockets.sort_by_key(|(_, offset)| *offset);
    assert_eq!(
        sockets
            .iter()
            .map(|(_, offset)| *offset)
            .collect::<Vec<_>>(),
        vec![0, chunk, 2 * chunk, 3 * chunk]
    );
    // Complete the first segment, leave three out-of-order segments unfinished.
    for (socket, offset) in &mut sockets {
        let length = if *offset == 0 { chunk } else { 1024 };
        socket
            .write_all(&data[*offset..*offset + length])
            .await
            .unwrap();
    }
    timeout(Duration::from_secs(8), async {
        loop {
            let e = receiver.events.recv().await.unwrap();
            if e.event == "file.updated"
                && e.payload["messageId"] == id
                && e.payload["file"]["transferred"].as_u64() == Some(chunk as u64)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    let manager = receiver.net.file_transfers().unwrap();
    manager.pause(&id).await.unwrap();
    drop(sockets);
    let metadata = receiver
        .db
        .file_metadata(id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.state, "paused");
    assert_eq!(metadata.transferred, chunk as u64);
    assert_eq!(metadata.in_flight_bytes, 0);
    let parts: Vec<_> = std::fs::read_dir(receiver.root.join("downloads"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(parts.len(), 1, "abandoned segment files must be removed");
    assert_eq!(std::fs::read(&parts[0]).unwrap(), data[..chunk]);
    manager.accept(&id).await.unwrap();
    let (mut socket, offset) = download_request(&listener).await;
    assert_eq!(offset, chunk);
    socket.write_all(&data[offset..]).await.unwrap();
    drop(socket);
    let done = receiver.state(&id, "completed").await;
    assert_eq!(done.payload["file"]["attempt"], 2);
    assert_eq!(
        std::fs::read(receiver.db.file_path(id).await.unwrap()).unwrap(),
        data
    );
    assert_eq!(
        std::fs::read_dir(receiver.root.join("downloads"))
            .unwrap()
            .count(),
        1
    );
    receiver.stop().await;
}

#[tokio::test]
async fn parallel_failure_falls_back_to_single_connection_without_holes() {
    let mut receiver = Fixture::new().await;
    let (peer, _) = receiver.peer().await;
    let listener = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
    let data: Vec<u8> = (0..4 * 1024 * 1024 + 97).map(|n| (n % 251) as u8).collect();
    let id = incoming_test_file(&mut receiver, &peer, data.len()).await;
    let mut sockets = Vec::new();
    for _ in 0..4 {
        sockets.push(download_request(&listener).await);
    }
    // A peer that rejects concurrent requests: no segment has completed.
    drop(sockets);
    let (mut socket, offset) = download_request(&listener).await;
    assert_eq!(offset, 0);
    socket.write_all(&data).await.unwrap();
    drop(socket);
    receiver.state(&id, "completed").await;
    assert_eq!(
        std::fs::read(receiver.db.file_path(id).await.unwrap()).unwrap(),
        data
    );
    assert_eq!(
        std::fs::read_dir(receiver.root.join("downloads"))
            .unwrap()
            .count(),
        1
    );
    receiver.stop().await;
}

#[tokio::test]
async fn missing_partial_after_restart_can_still_be_cancelled() {
    let mut receiver = Fixture::new().await;
    let (peer, _) = receiver.peer().await;
    let listener = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
    let id = incoming_test_file(&mut receiver, &peer, 10).await;
    let (mut socket, _) = download_request(&listener).await;
    socket.write_all(b"abcd").await.unwrap();
    drop(socket);
    receiver.state(&id, "paused").await;
    receiver = receiver.restart().await;
    let checkpoint = receiver
        .db
        .receive_checkpoint(id.clone())
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(
        receiver
            .root
            .join("downloads")
            .join(checkpoint["part"].as_str().unwrap()),
    )
    .unwrap();
    assert!(receiver
        .net
        .file_transfers()
        .unwrap()
        .cancel(&id, false)
        .await
        .unwrap());
    assert!(receiver
        .db
        .receive_checkpoint(id.clone())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        receiver.db.file_metadata(id).await.unwrap().unwrap().state,
        "cancelled"
    );
    receiver.stop().await;
}

#[tokio::test]
async fn cancelling_sibling_offer_does_not_release_paused_file() {
    let mut receiver = Fixture::new().await;
    let (peer, _) = receiver.peer().await;
    let listener = TcpListener::bind(peer.local_addr().unwrap()).await.unwrap();
    peer.send_to(
        &encode_packet(
            900,
            "legacy",
            "fixture",
            IPMSG_SENDMSG | IPMSG_FILEATTACHOPT,
            "",
            Some("1:first.txt:a:0:1:\x072:second.txt:a:0:1:\x07"),
        )
        .unwrap(),
        (Ipv4Addr::LOCALHOST, receiver.port),
    )
    .await
    .unwrap();
    let first = receiver.event("message.received").await.payload["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = receiver.event("message.received").await.payload["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(mode(packet(&peer).await.command), IPMSG_RECVMSG);
    let manager = receiver.net.file_transfers().unwrap();
    manager.accept(&first).await.unwrap();
    let (mut socket, _) = download_request(&listener).await;
    assert_eq!(mode(packet(&peer).await.command), IPMSG_RECVMSG);
    socket.write_all(b"part").await.unwrap();
    drop(socket);
    receiver.state(&first, "paused").await;
    manager.accept(&second).await.unwrap();
    let (mut socket, _) = download_request_for(&listener, 2).await;
    assert_eq!(mode(packet(&peer).await.command), IPMSG_RECVMSG);
    socket.write_all(b"second").await.unwrap();
    drop(socket);
    receiver.state(&second, "paused").await;
    drop(manager);
    receiver = receiver.restart().await;
    let manager = receiver.net.file_transfers().unwrap();
    // Neither paused task is restored yet; persisted siblings also retain the invitation.
    manager.cancel(&second, true).await.unwrap();
    let mut bytes = [0; 4096];
    // Restart legitimately sends BR_EXIT; only RELEASEFILES would invalidate the sibling.
    assert!(timeout(Duration::from_millis(100), async {
        loop {
            let n = peer.recv(&mut bytes).await.unwrap();
            assert_ne!(
                mode(parse_packet(&bytes[..n]).unwrap().command),
                IPMSG_RELEASEFILES
            );
        }
    })
    .await
    .is_err());
    manager.cancel(&first, false).await.unwrap();
    assert_eq!(mode(packet(&peer).await.command), IPMSG_RELEASEFILES);
    assert!(receiver
        .db
        .receive_checkpoint(first)
        .await
        .unwrap()
        .is_none());
    receiver.stop().await;
}
