use ipmsg_core::{
    config::ConfigStore,
    database::Database,
    network::{Event, Network},
    protocol::*,
    User,
};
use std::{
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
    network: Arc<Network>,
    db: Database,
    events: mpsc::Receiver<Event>,
    root: PathBuf,
    port: u16,
}
impl Fixture {
    async fn start() -> Self {
        Self::start_with_direct(vec![]).await
    }
    async fn start_with_direct(direct: Vec<SocketAddrV4>) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "speedipmsg-rust-loopback-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        // Match the desktop socket factory, not just std::net defaults.
        let raw = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        raw.set_reuse_address(false).unwrap();
        raw.set_broadcast(true).unwrap();
        raw.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        let socket: std::net::UdpSocket = raw.into();
        let port = socket.local_addr().unwrap().port();
        let config = Arc::new(ConfigStore::open(root.join("config.json")).unwrap());
        let db = Database::open(&root.join("messages.db")).unwrap();
        let (tx, events) = mpsc::channel(256);
        let local = User {
            id: "local@rust-test".into(),
            nickname: "Rust测试".into(),
            username: "local".into(),
            hostname: "rust-test".into(),
            group: "测试组".into(),
            ip: "127.0.0.1".into(),
            port,
            status: "online".into(),
            version: "1".into(),
        };
        // Tests use loopback only: no LAN broadcast, production port or user database.
        let network = Network::new(socket, local, config, db.clone(), tx, direct, vec![]).unwrap();
        network.start().await;
        Self {
            network,
            db,
            events,
            root,
            port,
        }
    }
    async fn next(&mut self, name: &str) -> Event {
        timeout(Duration::from_secs(2), async {
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
    async fn stop(self) {
        self.network.shutdown().await;
        self.db.shutdown().await;
        // Windows can briefly retain a file handle after a database closes.
        // Retry only cleanup of this fixture's uniquely-owned directory.
        for attempt in 0..10 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied && attempt < 9 =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("test directory cleanup failed: {error}"),
            }
        }
    }
}
fn peer_id(socket: &UdpSocket) -> String {
    format!("peer@fixture#{}", socket.local_addr().unwrap())
}

async fn packet(socket: &UdpSocket) -> Packet {
    let mut data = [0u8; 65536];
    let (n, _) = timeout(Duration::from_secs(2), socket.recv_from(&mut data))
        .await
        .unwrap()
        .unwrap();
    parse_packet(&data[..n]).unwrap()
}
async fn send(
    socket: &UdpSocket,
    port: u16,
    n: u32,
    command: u32,
    body: &str,
    extra: Option<&str>,
) {
    let bytes = encode_packet(n, "peer", "fixture", command, body, extra).unwrap();
    socket
        .send_to(&bytes, (Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn probing_closed_udp_port_does_not_stop_receiving() {
    let closed = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = match closed.local_addr().unwrap() {
        std::net::SocketAddr::V4(address) => address,
        _ => unreachable!(),
    };
    drop(closed);
    let test = Fixture::start_with_direct(vec![address]).await;
    test.network.ui_ready().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    send(
        &peer,
        test.port,
        3000,
        IPMSG_BR_ENTRY,
        "仍应能发现",
        Some(""),
    )
    .await;
    let mut data = [0; 2048];
    let reply = timeout(Duration::from_secs(2), peer.recv_from(&mut data)).await;
    test.stop().await;
    let (size, _) = reply
        .expect("probing a closed port must not kill the receive loop")
        .unwrap();
    assert_eq!(
        mode(parse_packet(&data[..size]).unwrap().command),
        IPMSG_ANSENTRY
    );
}

#[tokio::test]
async fn real_udp_discovery_ack_validation_and_receive_deduplication() {
    let mut test = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    send(
        &peer,
        test.port,
        900,
        IPMSG_BR_ENTRY,
        "测试对端",
        Some("研发组"),
    )
    .await;
    assert_eq!(mode(packet(&peer).await.command), IPMSG_ANSENTRY);
    let user = test.next("user.discovered").await;
    assert_eq!(user.payload["group"], "研发组");
    assert_eq!(test.network.users().len(), 1);

    let id = test
        .network
        .send_message(&peer_id(&peer), "第一行\n第二行")
        .await
        .unwrap();
    let text = packet(&peer).await;
    assert_eq!(
        decode_text(&text.body, text.command).unwrap(),
        "第一行\n第二行"
    );
    let rogue = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    send(
        &rogue,
        test.port,
        901,
        IPMSG_RECVMSG,
        &text.packet_no.to_string(),
        None,
    )
    .await;
    assert!(timeout(Duration::from_millis(80), test.events.recv())
        .await
        .is_err());
    send(
        &peer,
        test.port,
        902,
        IPMSG_RECVMSG,
        &text.packet_no.to_string(),
        None,
    )
    .await;
    assert_eq!(test.next("message.ack").await.payload["messageId"], id);

    let incoming = "1122{/font;-8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 8404992;}";
    send(
        &peer,
        test.port,
        903,
        IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
        incoming,
        None,
    )
    .await;
    send(
        &peer,
        test.port,
        903,
        IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
        incoming,
        None,
    )
    .await;
    for _ in 0..2 {
        let ack = packet(&peer).await;
        assert_eq!(mode(ack.command), IPMSG_RECVMSG);
        assert_eq!(ack.body, b"903");
    }
    assert_eq!(
        test.next("message.received").await.payload["content"],
        "1122"
    );
    assert!(timeout(Duration::from_millis(80), test.events.recv())
        .await
        .is_err());
    let rows = test
        .db
        .history(peer_id(&peer), "local@rust-test".into(), 50, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.status == 1));
    test.stop().await;
}

#[tokio::test]
async fn unsupported_image_is_reported_once_and_never_acknowledged() {
    let mut test = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let bytes = b"1:10:peer:fixture:2097344:abcdef01|3|0|1|1|3|0|1|0|00000000#\0xyz";
    for _ in 0..2 {
        peer.send_to(bytes, (Ipv4Addr::LOCALHOST, test.port))
            .await
            .unwrap();
    }
    let hint = test.next("message.received").await;
    assert!(hint.payload["content"]
        .as_str()
        .unwrap()
        .contains("暂不支持"));
    let mut data = [0; 1024];
    assert!(
        timeout(Duration::from_millis(100), peer.recv_from(&mut data))
            .await
            .is_err()
    );
    assert!(timeout(Duration::from_millis(80), test.events.recv())
        .await
        .is_err());
    test.stop().await;
}

#[tokio::test]
async fn unacknowledged_message_retries_then_fails() {
    let mut test = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    send(&peer, test.port, 910, IPMSG_BR_ENTRY, "对端", Some("")).await;
    packet(&peer).await;
    test.next("user.discovered").await;
    let id = test
        .network
        .send_message(&peer_id(&peer), "timeout")
        .await
        .unwrap();
    let first = packet(&peer).await;
    // Read retries with a larger deadline; each must retain the original packet number.
    for _ in 0..2 {
        let mut bytes = [0; 2048];
        let (n, _) = timeout(Duration::from_secs(4), peer.recv_from(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parse_packet(&bytes[..n]).unwrap().packet_no,
            first.packet_no
        );
    }
    let failed = timeout(Duration::from_secs(4), async {
        loop {
            let event = test.events.recv().await.unwrap();
            if event.event == "message.failed" {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(failed.payload["messageId"], id);
    assert_eq!(
        test.db
            .history(peer_id(&peer), "local@rust-test".into(), 10, 0)
            .await
            .unwrap()[0]
            .status,
        3
    );
    test.stop().await;
}

#[tokio::test]
async fn temporary_text_ports_do_not_replace_discovered_listener_or_duplicate_messages() {
    let mut test = Fixture::start().await;
    let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let temporary = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let another = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    send(
        &listener,
        test.port,
        920,
        IPMSG_BR_ENTRY,
        "对端",
        Some("组"),
    )
    .await;
    packet(&listener).await;
    test.next("user.discovered").await;
    for source in [&temporary, &another] {
        send(
            source,
            test.port,
            921,
            IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
            "只收一次",
            None,
        )
        .await;
        assert_eq!(packet(&listener).await.body, b"921");
    }
    assert_eq!(
        test.network.users()[0].port,
        listener.local_addr().unwrap().port()
    );
    test.next("message.received").await;
    assert!(timeout(Duration::from_millis(80), test.events.recv())
        .await
        .is_err());
    assert_eq!(
        test.db
            .history(peer_id(&listener), "local@rust-test".into(), 10, 0)
            .await
            .unwrap()
            .len(),
        1
    );
    test.stop().await;
}

#[tokio::test]
async fn failed_initialization_remains_retryable_instead_of_claiming_ready() {
    let test = Fixture::start().await;
    test.network.shutdown().await;
    assert!(test.network.ui_ready().await.is_err());
    assert!(test.network.ui_ready().await.is_err());
    test.stop().await;
}

#[tokio::test]
async fn identical_wire_users_at_two_endpoints_remain_independent() {
    let mut test = Fixture::start().await;
    let cpp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let feiq = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    for (socket, nickname) in [(&cpp, "原版"), (&feiq, "飞秋")] {
        send(
            socket,
            test.port,
            1000,
            IPMSG_BR_ENTRY,
            nickname,
            Some("测试组"),
        )
        .await;
        assert_eq!(mode(packet(socket).await.command), IPMSG_ANSENTRY);
        test.next("user.discovered").await;
    }
    assert_eq!(test.network.users().len(), 2);
    assert_ne!(peer_id(&cpp), peer_id(&feiq));
    let cpp_id = test
        .network
        .send_message(&peer_id(&cpp), "发给原版")
        .await
        .unwrap();
    let cpp_packet = packet(&cpp).await;
    let feiq_id = test
        .network
        .send_message(&peer_id(&feiq), "发给飞秋")
        .await
        .unwrap();
    let feiq_packet = packet(&feiq).await;
    assert_eq!(
        decode_text(&cpp_packet.body, cpp_packet.command).unwrap(),
        "发给原版"
    );
    assert_eq!(
        decode_text(&feiq_packet.body, feiq_packet.command).unwrap(),
        "发给飞秋"
    );
    send(
        &feiq,
        test.port,
        1001,
        IPMSG_RECVMSG,
        &cpp_packet.packet_no.to_string(),
        None,
    )
    .await;
    assert!(timeout(Duration::from_millis(80), test.events.recv())
        .await
        .is_err());
    send(
        &cpp,
        test.port,
        1002,
        IPMSG_RECVMSG,
        &cpp_packet.packet_no.to_string(),
        None,
    )
    .await;
    assert_eq!(test.next("message.ack").await.payload["messageId"], cpp_id);
    send(
        &feiq,
        test.port,
        1003,
        IPMSG_RECVMSG,
        &feiq_packet.packet_no.to_string(),
        None,
    )
    .await;
    assert_eq!(test.next("message.ack").await.payload["messageId"], feiq_id);
    // Matching packet numbers from different endpoints are different messages.
    for (socket, body) in [(&cpp, "原版回复"), (&feiq, "飞秋回复")] {
        send(
            socket,
            test.port,
            2000,
            IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
            body,
            None,
        )
        .await;
        assert_eq!(packet(socket).await.body, b"2000");
        let received = test.next("message.received").await;
        assert_eq!(received.payload["from"], peer_id(socket));
        assert_eq!(received.payload["content"], body);
    }
    let temporary = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    send(
        &temporary,
        test.port,
        2001,
        IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
        "不能猜测归属",
        None,
    )
    .await;
    let diagnostic = test.next("network.diagnostic").await;
    assert!(diagnostic.payload["message"]
        .as_str()
        .unwrap()
        .contains("无法确定监听端点"));
    assert_eq!(test.network.users().len(), 2);
    for socket in [&cpp, &feiq] {
        assert_eq!(
            test.db
                .history(peer_id(socket), "local@rust-test".into(), 50, 0)
                .await
                .unwrap()
                .len(),
            2
        );
    }
    test.stop().await;
}

#[tokio::test]
async fn malformed_datagrams_produce_diagnostics_without_creating_users() {
    let mut test = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    peer.send_to(b"invalid", (Ipv4Addr::LOCALHOST, test.port))
        .await
        .unwrap();
    let event = test.next("network.diagnostic").await;
    assert!(event.payload["message"]
        .as_str()
        .unwrap()
        .contains("Packet rejected source="));
    assert!(test.network.users().is_empty());
    test.stop().await;
}
