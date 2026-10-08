use ipmsg_core::{
    config::ConfigStore,
    database::{Database, Record},
    image::{assets::AssetStore, dib::decode_image, lzw::crc32},
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{net::UdpSocket, sync::mpsc, time::timeout};

fn root() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "rust-image-test-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
// Independent literal-only bit packer from the documented wire format. No production encoder.
fn payload() -> Vec<u8> {
    let width = 17u32;
    let height = 24u32;
    let stride = (width * 3 + 3) & !3;
    let mut dib = Vec::new();
    for value in [40, width, height] {
        dib.extend(value.to_le_bytes());
    }
    dib.extend([1, 0, 24, 0]);
    for value in [0, stride * height, 0, 0, 0, 0] {
        dib.extend(value.to_le_bytes());
    }
    dib.resize(40 + (stride * height) as usize, 0);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = 40 + y * stride as usize + x * 3;
            dib[i] = x as u8;
            dib[i + 1] = y as u8;
            dib[i + 2] = 200;
        }
    }
    let mut result = b"LZW!".to_vec();
    result.extend((dib.len() as u32).to_le_bytes());
    result.extend(crc32(&dib).to_le_bytes());
    let (mut width, mut counter, mut byte, mut used) = (9, 256, 0u8, 0);
    for literal in dib {
        for bit in 0..width {
            byte = (byte << 1) | (((literal as u32 >> bit) & 1) as u8);
            used += 1;
            if used == 8 {
                result.push(byte);
                byte = 0;
                used = 0;
            }
        }
        if width < 12 {
            counter += 1;
            if counter == 1 << width {
                width += 1;
            }
        }
    }
    if used > 0 {
        result.push(byte << (8 - used));
    }
    result
}
fn fragment(id: &str, data: &[u8], index: usize) -> Vec<u8> {
    let count = data.len().div_ceil(512);
    let offset = (index - 1) * 512;
    let size = (data.len() - offset).min(512);
    let mut wire = format!(
        "1:{}:peer:fixture:2097344:{id}|{}|{offset}|{count}|{index}|{size}|0|1|0|00000000#",
        100 + index,
        data.len()
    )
    .into_bytes();
    wire.push(0);
    wire.extend_from_slice(&data[offset..offset + size]);
    wire
}
struct Fixture {
    root: PathBuf,
    net: Arc<Network>,
    db: Database,
    events: mpsc::Receiver<Event>,
    port: u16,
}
impl Fixture {
    async fn start() -> Self {
        let root = root();
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
        let local = User {
            id: "local@fixture".into(),
            username: "local".into(),
            hostname: "fixture".into(),
            nickname: "本机".into(),
            group: "".into(),
            ip: "127.0.0.1".into(),
            port,
            status: "online".into(),
            version: "1".into(),
        };
        let (tx, events) = mpsc::channel(256);
        let net = Network::new(socket, local, config, db.clone(), tx, vec![], vec![]).unwrap();
        net.start().await;
        Self {
            root,
            net,
            db,
            events,
            port,
        }
    }
    async fn event(&mut self, name: &str) -> Event {
        timeout(Duration::from_secs(4), async {
            loop {
                let e = self.events.recv().await.unwrap();
                if e.event == name {
                    return e;
                }
            }
        })
        .await
        .unwrap()
    }
    async fn stop(self) {
        self.net.shutdown().await;
        self.db.shutdown().await;
        for n in 0..10 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && n < 9 => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(e) => panic!("{e}"),
            }
        }
    }
}
async fn response(peer: &UdpSocket) -> Packet {
    let mut b = [0; 4096];
    let (n, _) = timeout(Duration::from_secs(4), peer.recv_from(&mut b))
        .await
        .unwrap()
        .unwrap();
    parse_packet(&b[..n]).unwrap()
}
async fn reference(peer: &UdpSocket, port: u16, id: &str) {
    let wire = encode_packet(
        70,
        "peer",
        "fixture",
        IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
        &format!("/~#>{id}<B~"),
        Some(""),
    )
    .unwrap();
    peer.send_to(&wire, (Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    assert_eq!(response(peer).await.body, b"70");
}
async fn transfer(peer: &UdpSocket, port: u16, id: &str, data: &[u8]) {
    for i in (1..=data.len().div_ceil(512)).rev() {
        peer.send_to(&fragment(id, data, i), (Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let ack = response(peer).await;
        assert_eq!(mode(ack.command), IPMSG_REPORT_RECVIMAGE);
        assert_eq!(ack.body, format!("{id}|{i}#").as_bytes());
    }
}

#[tokio::test]
async fn data_first_receive_persists_once_and_reference_does_not_duplicate() {
    let mut f = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let data = payload();
    transfer(&peer, f.port, "abcdef12", &data).await;
    let event = f.event("message.received").await;
    assert_eq!(event.payload["type"], "image");
    assert_eq!(event.payload["image"]["width"], 17);
    let id = event.payload["image"]["assetId"]
        .as_str()
        .unwrap()
        .to_string();
    let bytes = f.net.read_image(&id, false).await.unwrap();
    let decoded = ::image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (17, 24));
    reference(&peer, f.port, "abcdef12").await;
    reference(&peer, f.port, "abcdef12").await;
    peer.send_to(
        &fragment("abcdef12", &data, 1),
        (Ipv4Addr::LOCALHOST, f.port),
    )
    .await
    .unwrap();
    assert_eq!(response(&peer).await.body, b"abcdef12|1#");
    let partner = event.payload["from"].as_str().unwrap().to_string();
    let rows =
        f.db.history(partner.clone(), "local@fixture".into(), 50, 0)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].image.as_ref().unwrap().asset_id, id);
    assert!(timeout(Duration::from_millis(100), f.events.recv())
        .await
        .is_err());
    assert!(f.net.read_image("../../config.json", false).await.is_err());
    assert!(f.net.read_image("not-registered", false).await.is_err());
    f.net.shutdown().await;
    f.db.shutdown().await;
    let reopened = Database::open(&f.root.join("messages.db")).unwrap();
    let row = reopened
        .history(partner, "local@fixture".into(), 50, 0)
        .await
        .unwrap();
    assert_eq!(row[0].image.as_ref().unwrap().asset_id, id);
    reopened.shutdown().await;
    f.stop().await;
}

#[tokio::test]
async fn reference_first_and_same_id_from_two_peers_keep_separate_images() {
    let mut f = Fixture::start().await;
    let a = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let b = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let data = payload();
    reference(&a, f.port, "1234abcd").await;
    transfer(&a, f.port, "1234abcd", &data).await;
    let first = f.event("message.received").await;
    transfer(&b, f.port, "1234abcd", &data).await;
    let second = f.event("message.received").await;
    assert_ne!(first.payload["id"], second.payload["id"]);
    assert_ne!(
        first.payload["image"]["assetId"],
        second.payload["image"]["assetId"]
    );
    f.stop().await;
}

#[tokio::test]
async fn write_failure_withholds_final_ack_even_after_retry() {
    let mut f = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let data = payload();
    std::fs::remove_dir(f.root.join("images")).unwrap();
    std::fs::write(f.root.join("images"), b"block writes").unwrap();
    let count = data.len().div_ceil(512);
    for i in 1..count {
        peer.send_to(
            &fragment("feedabcd", &data, i),
            (Ipv4Addr::LOCALHOST, f.port),
        )
        .await
        .unwrap();
        response(&peer).await;
    }
    peer.send_to(
        &fragment("feedabcd", &data, count),
        (Ipv4Addr::LOCALHOST, f.port),
    )
    .await
    .unwrap();
    f.event("image.receive_failed").await;
    peer.send_to(
        &fragment("feedabcd", &data, count),
        (Ipv4Addr::LOCALHOST, f.port),
    )
    .await
    .unwrap();
    let mut bytes = [0; 4096];
    assert!(
        timeout(Duration::from_millis(200), peer.recv_from(&mut bytes))
            .await
            .is_err()
    );
    assert!(f
        .db
        .recent("local@fixture".into(), 50)
        .await
        .unwrap()
        .is_empty());
    f.stop().await;
}

#[tokio::test]
async fn conflicting_fragment_emits_one_failure_and_no_successful_message() {
    let mut f = Fixture::start().await;
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let data = payload();
    let first = fragment("badc0ffe", &data, 1);
    peer.send_to(&first, (Ipv4Addr::LOCALHOST, f.port))
        .await
        .unwrap();
    assert_eq!(response(&peer).await.body, b"badc0ffe|1#");
    let mut conflict = first.clone();
    *conflict.last_mut().unwrap() ^= 1;
    peer.send_to(&conflict, (Ipv4Addr::LOCALHOST, f.port))
        .await
        .unwrap();
    f.event("image.receive_failed").await;
    peer.send_to(&conflict, (Ipv4Addr::LOCALHOST, f.port))
        .await
        .unwrap();
    let mut bytes = [0; 4096];
    assert!(
        timeout(Duration::from_millis(120), peer.recv_from(&mut bytes))
            .await
            .is_err()
    );
    while let Ok(Some(event)) = timeout(Duration::from_millis(50), f.events.recv()).await {
        assert_ne!(event.event, "image.receive_failed");
        assert_ne!(event.event, "message.received");
    }
    assert!(f
        .db
        .recent("local@fixture".into(), 50)
        .await
        .unwrap()
        .is_empty());
    f.stop().await;
}

#[tokio::test]
async fn legacy_schema_migrates_and_image_clear_does_not_resurrect() {
    let root = root();
    let path = root.join("messages.db");
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY,from_id TEXT,to_id TEXT,content TEXT,type INTEGER,timestamp INTEGER,status INTEGER);INSERT INTO messages VALUES('old','peer','local','old text',0,1,1);").unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.history("peer".into(), "local".into(), 50, 0)
            .await
            .unwrap()[0]
            .content,
        "old text"
    );
    let store = AssetStore::new(db.images_dir()).unwrap();
    let mut pending_asset = store
        .persist(decode_image(&payload()).unwrap(), "abcd1234")
        .unwrap();
    let metadata = pending_asset.metadata.clone();
    let record = Record {
        id: "image-rx:peer:abcd1234".into(),
        from_id: "peer".into(),
        to_id: "local".into(),
        content: "[图片]".into(),
        kind: 1,
        timestamp: 2,
        status: 1,
        file: None,
        image: Some(metadata.clone()),
    };
    assert!(db
        .insert_image(record.clone(), Instant::now() + Duration::from_secs(2))
        .await
        .unwrap());
    pending_asset.keep();
    let rows = db
        .history("peer".into(), "local".into(), 50, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].image.is_none());
    assert_eq!(rows[1].image.as_ref(), Some(&metadata));
    db.clear(Some("peer".into()), "local".into()).await.unwrap();
    assert!(!db
        .insert_image(record, Instant::now() + Duration::from_secs(2))
        .await
        .unwrap());
    assert!(db.recent("local".into(), 50).await.unwrap().is_empty());
    assert!(store.read(&metadata.asset_id, false).is_ok());
    db.shutdown().await;
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
    drop(connection);
    std::fs::remove_dir_all(root).unwrap();
}
