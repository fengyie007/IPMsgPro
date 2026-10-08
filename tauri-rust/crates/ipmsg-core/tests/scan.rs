use ipmsg_core::{
    config::ConfigStore,
    database::Database,
    network::{Event, Network},
    protocol::*,
    scan::{ScanOptions, ScanPhase, ScanStatus},
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
    config: Arc<ConfigStore>,
    db: Database,
    events: mpsc::Receiver<Event>,
    root: PathBuf,
    port: u16,
}
impl Fixture {
    async fn new(event_capacity: usize) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rust-scan-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.set_broadcast(true).unwrap();
        socket
            .bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        let socket: std::net::UdpSocket = socket.into();
        let port = socket.local_addr().unwrap().port();
        let config = Arc::new(ConfigStore::open(root.join("config.json")).unwrap());
        let db = Database::open(&root.join("messages.db")).unwrap();
        let local = User {
            id: "local@scan-test".into(),
            username: "local".into(),
            hostname: "scan-test".into(),
            nickname: "扫描测试".into(),
            group: "".into(),
            ip: "127.0.0.1".into(),
            port,
            status: "online".into(),
            version: "1".into(),
        };
        let (tx, events) = mpsc::channel(event_capacity);
        let network = Network::new(
            socket,
            local,
            config.clone(),
            db.clone(),
            tx,
            vec![],
            vec![],
        )
        .unwrap();
        network.start().await;
        Self {
            network,
            config,
            db,
            events,
            root,
            port,
        }
    }
    async fn state(&self, condition: impl Fn(&ScanStatus) -> bool) -> ScanStatus {
        timeout(Duration::from_secs(4), async {
            loop {
                let state = self.network.scan_status();
                if condition(&state) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn stop(self) {
        self.network.shutdown().await;
        self.db.shutdown().await;
        assert!(self.root.starts_with(std::env::temp_dir()));
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rust-scan-test-"));
        for attempt in 0..10 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(_) if attempt < 9 => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(error) => panic!("{error}"),
            }
        }
    }
}
fn options(port: u16, ranges: &[&str], delay_ms: u32) -> ScanOptions {
    ScanOptions {
        ranges: ranges.iter().map(|s| s.to_string()).collect(),
        port,
        delay_ms,
    }
}
async fn receive(peer: &UdpSocket) -> Packet {
    let mut bytes = [0; 4096];
    let n = timeout(Duration::from_secs(2), peer.recv(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    parse_packet(&bytes[..n]).unwrap()
}
async fn reply(peer: &UdpSocket, port: u16, number: u32) {
    peer.send_to(
        &encode_packet(
            number,
            "peer",
            "scan-peer",
            IPMSG_ANSENTRY,
            "测试联系人",
            Some("组"),
        )
        .unwrap(),
        (Ipv4Addr::LOCALHOST, port),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn real_probe_deduplicates_ranges_and_responses_and_keeps_terminal_snapshot() {
    let mut fixture = Fixture::new(128).await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = peer.local_addr().unwrap().port();
    let started = fixture
        .network
        .start_scan(options(
            port,
            &["127.0.0.1", "127.0.0.1-1", "127.0.0.1/32"],
            10,
        ))
        .await
        .unwrap();
    assert_eq!(started.total, 1);
    let packet = receive(&peer).await;
    assert_eq!(mode(packet.command), IPMSG_BR_ENTRY);
    assert!(packet.version.starts_with("1_lbt6_0#"));
    let wrong = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    reply(&wrong, fixture.port, 100).await;
    for n in 101..104 {
        reply(&peer, fixture.port, n).await;
    }
    let completed = fixture.state(|s| s.state == ScanPhase::Completed).await;
    assert_eq!(
        (
            completed.current,
            completed.total,
            completed.found,
            completed.failed_sends
        ),
        (1, 1, 1, 0)
    );
    assert!(fixture.network.users().iter().any(|u| u.port == port));
    let mut extra = [0; 4096];
    assert!(timeout(Duration::from_millis(100), peer.recv(&mut extra))
        .await
        .is_err());
    reply(&peer, fixture.port, 105).await;
    // Observe delivery through the normal user event, then verify late profiles do not mutate the terminal scan.
    timeout(Duration::from_secs(1), async {
        while let Some(event) = fixture.events.recv().await {
            if event.event == "network.scan_complete" {
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(fixture.network.scan_status().revision, completed.revision);
    fixture.stop().await;
}

#[tokio::test]
async fn cancel_interrupts_pacing_and_old_ids_cannot_cancel_a_new_scan() {
    let fixture = Fixture::new(128).await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = peer.local_addr().unwrap().port();
    let first = fixture
        .network
        .start_scan(options(port, &["127.0.0.1-254"], 1000))
        .await
        .unwrap();
    receive(&peer).await;
    assert!(fixture
        .network
        .start_scan(options(port, &["127.0.0.1"], 10))
        .await
        .is_err());
    assert!(fixture
        .network
        .cancel_scan(first.scan_id + 1)
        .await
        .is_err());
    let cancelled = timeout(
        Duration::from_millis(750),
        fixture.network.cancel_scan(first.scan_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cancelled.state, ScanPhase::Cancelled);
    assert!(cancelled.current < cancelled.total);
    let current = cancelled.current;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fixture.network.scan_status().current, current);
    let second = fixture
        .network
        .start_scan(options(port, &["127.0.0.1"], 10))
        .await
        .unwrap();
    assert!(second.scan_id > first.scan_id);
    receive(&peer).await;
    assert!(fixture.network.cancel_scan(first.scan_id).await.is_err());
    fixture.state(|s| s.state == ScanPhase::Waiting).await;
    fixture.network.shutdown().await;
    assert_eq!(fixture.network.scan_status().state, ScanPhase::Cancelled);
    assert!(fixture
        .network
        .start_scan(options(port, &["127.0.0.1"], 10))
        .await
        .is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn bounded_event_queue_does_not_block_scan_or_status_query() {
    let fixture = Fixture::new(1).await;
    assert!(fixture
        .network
        .start_scan(options(fixture.port, &[], 10))
        .await
        .is_err());
    assert_eq!(fixture.network.scan_status().scan_id, 0);
    fixture
        .network
        .start_scan(options(fixture.port, &["127.0.0.1"], 10))
        .await
        .unwrap();
    let status = fixture.state(|s| s.state == ScanPhase::Completed).await;
    assert_eq!((status.current, status.skipped, status.found), (1, 1, 0));
    assert_eq!(fixture.events.len(), 1); // terminal notification was dropped, snapshot is still correct
    fixture.stop().await;
}

#[tokio::test]
async fn saved_auto_scan_runs_once_and_settings_changes_apply_to_next_scan() {
    let fixture = Fixture::new(128).await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = peer.local_addr().unwrap().port();
    fixture.config.save(serde_json::json!({"ipScanRanges":["127.0.0.1"],"scanPort":port,"scanDelayMs":10,"scanOnStartup":true})).await.unwrap();
    fixture.network.ui_ready().await.unwrap();
    fixture.network.ui_ready().await.unwrap();
    receive(&peer).await;
    let done = fixture.state(|s| s.state == ScanPhase::Completed).await;
    let updated = fixture
        .config
        .save(serde_json::json!({"scanDelayMs":30}))
        .await
        .unwrap();
    fixture.network.apply_config(&updated).await.unwrap();
    fixture.network.ui_ready().await.unwrap();
    let mut bytes = [0; 4096];
    assert!(timeout(Duration::from_millis(100), peer.recv(&mut bytes))
        .await
        .is_err());
    assert_eq!(fixture.network.scan_status().scan_id, done.scan_id);
    assert_eq!(fixture.network.scan_status().delay_ms, 10);
    let loaded = ConfigStore::open(fixture.root.join("config.json"))
        .unwrap()
        .get();
    assert_eq!(loaded.scan_delay_ms, 30);
    assert_eq!(loaded.scan_port, port);
    fixture.stop().await;
}

#[tokio::test]
async fn disabled_auto_scan_stays_idle_and_manual_scan_keeps_text_receiving() {
    let mut fixture = Fixture::new(128).await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = peer.local_addr().unwrap().port();
    fixture.config.save(serde_json::json!({"ipScanRanges":["127.0.0.1-3"],"scanPort":port,"scanDelayMs":20,"scanOnStartup":false})).await.unwrap();
    fixture.network.ui_ready().await.unwrap();
    assert_eq!(fixture.network.scan_status().state, ScanPhase::Idle);
    fixture
        .network
        .start_scan(options(port, &["127.0.0.1-3"], 20))
        .await
        .unwrap();
    receive(&peer).await;
    fixture.state(|s| s.state == ScanPhase::Waiting).await;
    // The other loopback destinations have no socket; Windows ICMP/10054 must not stop normal reception.
    peer.send_to(
        &encode_packet(
            200,
            "peer",
            "scan-peer",
            IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
            "扫描中仍能收信",
            None,
        )
        .unwrap(),
        (Ipv4Addr::LOCALHOST, fixture.port),
    )
    .await
    .unwrap();
    assert_eq!(mode(receive(&peer).await.command), IPMSG_RECVMSG);
    let message = timeout(Duration::from_secs(2), async {
        loop {
            let event = fixture.events.recv().await.unwrap();
            if event.event == "message.received" {
                return event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(message.payload["content"], "扫描中仍能收信");
    assert_eq!(
        fixture
            .state(|s| s.state == ScanPhase::Completed)
            .await
            .found,
        0
    ); // text is not a discovery response
    fixture.stop().await;
}
