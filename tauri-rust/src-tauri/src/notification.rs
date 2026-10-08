//! Notification policy and a single bounded audio worker. No audio IO on UDP/UI threads.
mod native;
#[cfg(test)]
mod tests;
use ipmsg_core::network::Event;
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

const COOLDOWN: Duration = Duration::from_secs(1);
const SEEN_TTL: Duration = Duration::from_secs(300);
const SEEN_LIMIT: usize = 256;
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub enabled: bool,
    pub accepting: bool,
    pub visible: bool,
    pub focused: bool,
    pub minimized: bool,
    pub active_conversation: &'a str,
    pub local_id: &'a str,
}
#[derive(Default)]
pub struct NotificationGate {
    seen: HashSet<String>,
    order: VecDeque<(String, Instant)>,
    last_sound: Option<Instant>,
}
impl NotificationGate {
    pub fn should_play(&mut self, event: &Event, context: Context<'_>, now: Instant) -> bool {
        if event.event != "message.received" {
            return false;
        }
        let Some(id) = event.payload["id"].as_str().filter(|s| !s.is_empty()) else {
            return false;
        };
        let Some(from) = event.payload["from"].as_str().filter(|s| !s.is_empty()) else {
            return false;
        };
        if from == context.local_id
            || from == "self"
            || id.len() + from.len() > 8192
            || !matches!(
                event.payload["type"].as_str(),
                Some("text" | "image" | "file")
            )
        {
            return false;
        }
        while self
            .order
            .front()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= SEEN_TTL)
        {
            if let Some((key, _)) = self.order.pop_front() {
                self.seen.remove(&key);
            }
        }
        let key = format!("{from}\0{id}");
        if self.seen.contains(&key) {
            return false;
        }
        if self.order.len() >= SEEN_LIMIT {
            if let Some((key, _)) = self.order.pop_front() {
                self.seen.remove(&key);
            }
        }
        self.seen.insert(key.clone());
        self.order.push_back((key, now));
        // Record muted/visible messages too: later duplicate events cannot sound after a toggle.
        let reading = context.visible
            && context.focused
            && !context.minimized
            && context.active_conversation == from;
        if !context.enabled
            || !context.accepting
            || reading
            || self
                .last_sound
                .is_some_and(|at| now.saturating_duration_since(at) < COOLDOWN)
        {
            return false;
        }
        self.last_sound = Some(now);
        true
    }
}

trait Player: Send + 'static {
    fn play(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Duration, String>;
    fn stop(&mut self) -> Result<(), String>;
}
struct Shared {
    enabled: AtomicBool,
    epoch: AtomicU64,
    closing: AtomicBool,
    alive: AtomicBool,
}
struct PlayRequest {
    epoch: u64,
    queued: Instant,
    reply: Option<oneshot::Sender<Result<u64, String>>>,
}
enum Command {
    Play(PlayRequest),
    Wake,
}
pub struct NotificationSound {
    shared: Arc<Shared>,
    sender: SyncSender<Command>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl NotificationSound {
    pub fn new(
        root: PathBuf,
        enabled: bool,
        on_error: impl Fn(String) + Send + 'static,
    ) -> Result<Self, String> {
        if !cfg!(windows) {
            return Err("当前平台暂不支持提示音".into());
        }
        Self::with_player(native::NativePlayer::new(root), enabled, on_error)
    }
    fn with_player(
        player: impl Player,
        enabled: bool,
        on_error: impl Fn(String) + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(enabled),
            epoch: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            alive: AtomicBool::new(true),
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        let state = shared.clone();
        let worker = std::thread::Builder::new()
            .name("ipmsg-notification".into())
            .spawn(move || run(player, receiver, state, on_error))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            sender,
            worker: Mutex::new(Some(worker)),
        })
    }
    pub fn available(&self) -> bool {
        self.shared.alive.load(Ordering::Acquire) && !self.shared.closing.load(Ordering::Acquire)
    }
    pub fn enabled(&self) -> bool {
        self.available() && self.shared.enabled.load(Ordering::Acquire)
    }
    pub fn set_enabled(&self, enabled: bool) {
        if self.shared.enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            let _ = self.sender.try_send(Command::Wake);
        }
    }
    pub fn notify(&self) -> bool {
        if !self.available() || !self.shared.enabled.load(Ordering::Acquire) {
            return false;
        }
        self.sender
            .try_send(Command::Play(PlayRequest {
                epoch: self.shared.epoch.load(Ordering::Acquire),
                queued: Instant::now(),
                reply: None,
            }))
            .is_ok()
    }
    pub async fn preview(&self) -> Result<u64, String> {
        if !self.available() {
            return Err("提示音服务已停止".into());
        }
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        let (reply, response) = oneshot::channel();
        self.sender
            .try_send(Command::Play(PlayRequest {
                epoch,
                queued: Instant::now(),
                reply: Some(reply),
            }))
            .map_err(|_| "提示音正在处理，请稍后试听")?;
        match tokio::time::timeout(PREVIEW_TIMEOUT, response).await {
            Ok(result) => result.map_err(|_| "提示音播放线程已停止")?,
            Err(_) => {
                // Only invalidate this generation, never a newer user setting.
                if self
                    .shared
                    .epoch
                    .compare_exchange(epoch, epoch + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    let _ = self.sender.try_send(Command::Wake);
                }
                Err("提示音试听超时".into())
            }
        }
    }
    pub fn close(&self) {
        self.shared.closing.store(true, Ordering::Release);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.sender.try_send(Command::Wake);
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.close();
        let worker = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(worker) = worker {
            tokio::task::spawn_blocking(move || worker.join())
                .await
                .map_err(|e| e.to_string())?
                .map_err(|_| "提示音工作线程异常退出")?;
        }
        Ok(())
    }
}
impl Drop for NotificationSound {
    fn drop(&mut self) {
        self.close();
    }
}
struct WorkerLife(Arc<Shared>);
impl Drop for WorkerLife {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
    }
}

fn run(
    mut player: impl Player,
    receiver: Receiver<Command>,
    shared: Arc<Shared>,
    on_error: impl Fn(String),
) {
    let _life = WorkerLife(shared.clone());
    let mut epoch = shared.epoch.load(Ordering::Acquire);
    let mut playing_until: Option<Instant> = None;
    let mut last_error: Option<(String, Instant)> = None;
    let mut report = |error: String| {
        let now = Instant::now();
        if last_error.as_ref().is_none_or(|(previous, at)| {
            previous != &error || now.duration_since(*at) >= Duration::from_secs(60)
        }) {
            last_error = Some((error.clone(), now));
            on_error(error);
        }
    };
    loop {
        if shared.closing.load(Ordering::Acquire) {
            break;
        }
        let current = shared.epoch.load(Ordering::Acquire);
        if current != epoch {
            if let Err(error) = player.stop() {
                report(error);
            }
            playing_until = None;
            epoch = current;
        }
        let command = if let Some(until) = playing_until {
            match receiver.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Err(error) = player.stop() {
                        report(error);
                    }
                    playing_until = None;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };
        if shared.closing.load(Ordering::Acquire) {
            break;
        }
        let current = shared.epoch.load(Ordering::Acquire);
        if current != epoch {
            if let Err(error) = player.stop() {
                report(error);
            }
            playing_until = None;
            epoch = current;
        }
        let Command::Play(request) = command else {
            continue;
        };
        let preview = request.reply.is_some();
        let cancelled = || {
            shared.closing.load(Ordering::Acquire)
                || shared.epoch.load(Ordering::Acquire) != request.epoch
                || (!preview && !shared.enabled.load(Ordering::Acquire))
                || request.queued.elapsed() >= if preview { PREVIEW_TIMEOUT } else { COOLDOWN }
        };
        if cancelled() {
            if let Some(reply) = request.reply {
                let _ = reply.send(Err("提示音请求已取消".into()));
            }
            continue;
        }
        if !preview && playing_until.is_some_and(|until| until > Instant::now()) {
            continue;
        }
        let result = {
            player
                .stop()
                .and_then(|_| player.play(&cancelled))
                .and_then(|duration| {
                    if cancelled() {
                        Err("提示音请求已取消".into())
                    } else {
                        playing_until = Some(Instant::now() + duration);
                        Ok(duration.as_millis() as u64)
                    }
                })
        };
        if result.is_err() {
            let _ = player.stop();
            playing_until = None;
            if !preview && !cancelled() {
                if let Err(error) = &result {
                    report(error.clone());
                }
            }
        }
        if let Some(reply) = request.reply {
            let _ = reply.send(result);
        }
    }
    let _ = player.stop();
}
