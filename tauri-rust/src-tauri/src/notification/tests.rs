use super::*;
use serde_json::json;
use std::sync::atomic::AtomicUsize;

fn message(id: &str, kind: &str) -> Event {
    Event {
        event: "message.received".into(),
        payload: json!({"id":id,"from":"peer","type":kind,"content":"测试"}),
    }
}
fn background() -> Context<'static> {
    Context {
        enabled: true,
        accepting: true,
        visible: true,
        focused: false,
        minimized: false,
        active_conversation: "peer",
        local_id: "local",
    }
}
#[test]
fn only_new_incoming_message_kinds_are_eligible() {
    let now = Instant::now();
    for kind in ["text", "image", "file"] {
        assert!(NotificationGate::default().should_play(&message("new", kind), background(), now));
    }
    for event in [
        "history.get",
        "message.ack",
        "image.send_completed",
        "file.updated",
        "user.discovered",
        "network.scan_complete",
    ] {
        let mut candidate = message("new", "text");
        candidate.event = event.into();
        assert!(!NotificationGate::default().should_play(&candidate, background(), now));
    }
    for from in ["local", "self", ""] {
        let mut candidate = message("new", "text");
        candidate.payload["from"] = json!(from);
        assert!(!NotificationGate::default().should_play(&candidate, background(), now));
    }
    assert!(!NotificationGate::default().should_play(&message("", "text"), background(), now));
    assert!(!NotificationGate::default().should_play(
        &message("new", "unknown"),
        background(),
        now
    ));
}
#[test]
fn foreground_conversation_is_quiet_but_background_other_chat_and_tray_are_not() {
    let reading = Context {
        focused: true,
        ..background()
    };
    assert!(!NotificationGate::default().should_play(
        &message("1", "text"),
        reading,
        Instant::now()
    ));
    for context in [
        background(),
        Context {
            active_conversation: "other",
            ..reading
        },
        Context {
            active_conversation: "",
            ..reading
        },
        Context {
            visible: false,
            ..reading
        },
        Context {
            minimized: true,
            ..reading
        },
    ] {
        assert!(NotificationGate::default().should_play(
            &message("1", "text"),
            context,
            Instant::now()
        ));
    }
    assert!(!NotificationGate::default().should_play(
        &message("1", "text"),
        Context {
            accepting: false,
            ..background()
        },
        Instant::now()
    ));
}
#[test]
fn muted_and_read_messages_do_not_realert_after_a_toggle() {
    let now = Instant::now();
    for context in [
        Context {
            enabled: false,
            ..background()
        },
        Context {
            focused: true,
            ..background()
        },
    ] {
        let mut gate = NotificationGate::default();
        assert!(!gate.should_play(&message("same", "image"), context, now));
        assert!(!gate.should_play(
            &message("same", "image"),
            background(),
            now + Duration::from_secs(2)
        ));
        assert!(gate.should_play(
            &message("different", "image"),
            background(),
            now + Duration::from_secs(2)
        ));
    }
}
#[test]
fn bursts_are_coalesced_and_deduplication_memory_is_bounded() {
    let now = Instant::now();
    let mut gate = NotificationGate::default();
    let mut sounds = 0;
    for n in 0..1000 {
        sounds +=
            usize::from(gate.should_play(&message(&n.to_string(), "file"), background(), now));
    }
    assert_eq!(sounds, 1);
    assert_eq!(gate.seen.len(), SEEN_LIMIT);
    assert_eq!(gate.order.len(), SEEN_LIMIT);
    assert!(gate.should_play(&message("next", "text"), background(), now + COOLDOWN));
    assert!(!gate.should_play(&message("next", "text"), background(), now + COOLDOWN * 2));
}

#[derive(Default)]
struct Counts {
    attempts: AtomicUsize,
    started: AtomicUsize,
    stopped: AtomicUsize,
    cancelled: AtomicUsize,
}
struct FakePlayer {
    counts: Arc<Counts>,
    blocker: Option<(mpsc::Sender<()>, Receiver<()>)>,
    fail_first: bool,
    fail_always: bool,
}
impl FakePlayer {
    fn new(counts: Arc<Counts>) -> Self {
        Self {
            counts,
            blocker: None,
            fail_first: false,
            fail_always: false,
        }
    }
}
impl Player for FakePlayer {
    fn play(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Duration, String> {
        let attempt = self.counts.attempts.fetch_add(1, Ordering::AcqRel);
        if let Some((entered, release)) = self.blocker.take() {
            entered.send(()).unwrap();
            release
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
        }
        if cancelled() {
            self.counts.cancelled.fetch_add(1, Ordering::AcqRel);
            return Err("cancelled".into());
        }
        if self.fail_always || (self.fail_first && attempt == 0) {
            return Err("device unavailable".into());
        }
        self.counts.started.fetch_add(1, Ordering::AcqRel);
        Ok(Duration::from_secs(10)) // no real audio: keep the fake active while races are exercised
    }
    fn stop(&mut self) -> Result<(), String> {
        self.counts.stopped.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
async fn wait_for(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn preview_after_queue(sound: &NotificationSound) -> Result<u64, String> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match sound.preview().await {
                Err(error) if error.contains("正在处理") => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                result => return result,
            }
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn preview_works_while_muted_and_shutdown_stops_playback() {
    let counts = Arc::new(Counts::default());
    let sound =
        NotificationSound::with_player(FakePlayer::new(counts.clone()), false, |_| {}).unwrap();
    assert!(!sound.notify());
    assert_eq!(sound.preview().await.unwrap(), 10_000);
    assert!(!sound.enabled());
    assert_eq!(counts.started.load(Ordering::Acquire), 1);
    sound.shutdown().await.unwrap();
    assert!(!sound.available());
    assert!(sound.preview().await.is_err());
    assert!(counts.stopped.load(Ordering::Acquire) >= 2);
}
#[tokio::test]
async fn disable_invalidates_in_flight_and_queued_automatic_requests() {
    let counts = Arc::new(Counts::default());
    let (entered, started) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let player = FakePlayer {
        blocker: Some((entered, blocked)),
        ..FakePlayer::new(counts.clone())
    };
    let sound = NotificationSound::with_player(player, true, |_| {}).unwrap();
    assert!(sound.notify());
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(sound.notify());
    assert!(!sound.notify()); // one queued request, no unbounded backlog
    sound.set_enabled(false);
    assert!(!sound.notify());
    release.send(()).unwrap();
    wait_for(|| counts.cancelled.load(Ordering::Acquire) == 1).await;
    assert_eq!(preview_after_queue(&sound).await.unwrap(), 10_000); // barrier after queued automatic request
    assert_eq!(counts.attempts.load(Ordering::Acquire), 2);
    assert_eq!(counts.started.load(Ordering::Acquire), 1); // only the explicit muted preview
    sound.shutdown().await.unwrap();
}
#[tokio::test]
async fn shutdown_cancels_pending_preview_and_prevents_late_playback() {
    let counts = Arc::new(Counts::default());
    let (entered, started) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let player = FakePlayer {
        blocker: Some((entered, blocked)),
        ..FakePlayer::new(counts.clone())
    };
    let sound = NotificationSound::with_player(player, true, |_| {}).unwrap();
    sound.notify();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let preview = sound.preview();
    tokio::pin!(preview);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut preview)
            .await
            .is_err()
    );
    sound.close();
    release.send(()).unwrap();
    assert!(preview.await.is_err());
    sound.shutdown().await.unwrap();
    assert_eq!(counts.started.load(Ordering::Acquire), 0);
}
#[tokio::test]
async fn preview_failure_is_not_success_and_worker_can_recover() {
    let counts = Arc::new(Counts::default());
    let player = FakePlayer {
        fail_first: true,
        ..FakePlayer::new(counts.clone())
    };
    let sound = NotificationSound::with_player(player, false, |_| {
        panic!("preview errors must use the IPC result")
    })
    .unwrap();
    assert!(sound
        .preview()
        .await
        .unwrap_err()
        .contains("device unavailable"));
    assert_eq!(sound.preview().await.unwrap(), 10_000);
    assert_eq!(counts.started.load(Ordering::Acquire), 1);
    sound.shutdown().await.unwrap();
}
#[tokio::test]
async fn repeated_automatic_errors_are_throttled_but_preview_still_reports_failure() {
    let counts = Arc::new(Counts::default());
    let reports = Arc::new(AtomicUsize::new(0));
    let report_count = reports.clone();
    let player = FakePlayer {
        fail_always: true,
        ..FakePlayer::new(counts.clone())
    };
    let sound = NotificationSound::with_player(player, true, move |_| {
        report_count.fetch_add(1, Ordering::AcqRel);
    })
    .unwrap();
    sound.notify();
    wait_for(|| reports.load(Ordering::Acquire) == 1).await;
    sound.notify();
    assert!(preview_after_queue(&sound)
        .await
        .unwrap_err()
        .contains("device unavailable"));
    assert_eq!(counts.attempts.load(Ordering::Acquire), 3);
    assert_eq!(reports.load(Ordering::Acquire), 1);
    sound.shutdown().await.unwrap();
}
#[tokio::test]
async fn an_obsolete_request_cannot_stop_a_newer_sound() {
    let counts = Arc::new(Counts::default());
    let sound =
        NotificationSound::with_player(FakePlayer::new(counts.clone()), false, |_| {}).unwrap();
    sound.preview().await.unwrap();
    sound.set_enabled(true);
    preview_after_queue(&sound).await.unwrap();
    let stops = counts.stopped.load(Ordering::Acquire);
    let (reply, response) = oneshot::channel();
    sound
        .sender
        .send(Command::Play(PlayRequest {
            epoch: 0,
            queued: Instant::now(),
            reply: Some(reply),
        }))
        .unwrap();
    assert!(response.await.unwrap().is_err());
    assert_eq!(counts.stopped.load(Ordering::Acquire), stops);
    assert_eq!(counts.started.load(Ordering::Acquire), 2);
    sound.shutdown().await.unwrap();
}
