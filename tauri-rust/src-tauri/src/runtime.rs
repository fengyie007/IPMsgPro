use crate::platform::{local_networks, Options};
use ipmsg_core::{
    config::ConfigStore,
    database::Database,
    network::{Event, Network},
    User,
};
use serde_json::{json, Value};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};

pub struct Runtime {
    pub network: Arc<Network>,
    pub database: Database,
    pub config: Arc<ConfigStore>,
    pub data_dir: PathBuf,
    pub port: u16,
    pub accepting: AtomicBool,
    pub quitting: AtomicBool,
    pub finished: AtomicBool,
    pub tray_available: AtomicBool,
    pub image_selecting: AtomicBool,
    pub capture: crate::capture::Capture,
    pub sound: Option<crate::notification::NotificationSound>,
    pub active_conversation: Mutex<String>,
    pub events: Mutex<Option<tokio::sync::mpsc::Receiver<Event>>>,
    event_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    log_file: Mutex<fs::File>,
    verbose: bool,
}
impl Runtime {
    pub async fn create(app: &AppHandle, options: Options) -> Result<Arc<Self>, String> {
        // Acquire the listening port before opening any writable per-instance files.
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| e.to_string())?;
        socket.set_reuse_address(false).map_err(|e| e.to_string())?;
        socket.set_broadcast(true).map_err(|e| e.to_string())?;
        socket
            .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, options.port).into())
            .map_err(|e| format!("无法绑定端口{}（可能已有实例运行）：{e}", options.port))?;
        let data_dir = app
            .path()
            .app_local_data_dir()
            .map_err(|e| e.to_string())?
            .join(format!("port-{}", options.port));
        fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let config = Arc::new(ConfigStore::open(data_dir.join("config.json"))?);
        let values = config.get();
        let database = Database::open(&data_dir.join("messages.db"))?;
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir.join("rust-preview.log"))
            .map_err(|e| e.to_string())?;
        let username = format!(
            "{}-rust-{}",
            std::env::var("USERNAME").unwrap_or_else(|_| "user".into()),
            options.port
        );
        let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());
        let (ip, broadcasts) = local_networks();
        let local = User {
            id: format!("{username}@{hostname}"),
            nickname: if values.nickname.is_empty() {
                username.clone()
            } else {
                values.nickname
            },
            username,
            hostname,
            group: values.group,
            ip: ip.to_string(),
            port: options.port,
            status: "online".into(),
            version: "0.1.0".into(),
        };
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let sound_events = tx.clone();
        let (sound, sound_error) = if cfg!(windows) {
            match crate::notification::NotificationSound::new(
                data_dir.join("sounds"),
                values.notification_sound,
                move |error| {
                    let _ = sound_events.try_send(Event {
                        event: "notification.sound_failed".into(),
                        payload: json!({"error":error}),
                    });
                },
            ) {
                Ok(sound) => (Some(sound), None),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };
        let network = Network::new(
            socket.into(),
            local,
            config.clone(),
            database.clone(),
            tx,
            options.direct,
            broadcasts,
        )?;
        let state = Arc::new(Self {
            network,
            database,
            config,
            data_dir,
            port: options.port,
            accepting: AtomicBool::new(true),
            quitting: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            tray_available: AtomicBool::new(false),
            image_selecting: AtomicBool::new(false),
            capture: crate::capture::Capture::default(),
            sound,
            active_conversation: Mutex::new(String::new()),
            events: Mutex::new(Some(rx)),
            event_task: Mutex::new(None),
            log_file: Mutex::new(log_file),
            verbose: options.verbose,
        });
        // Bind TCP to the same port as UDP; never advertise a fallback port.
        state
            .network
            .enable_files(
                app.path()
                    .download_dir()
                    .map_err(|e| e.to_string())?
                    .join("SpeedIpMsgRust"),
            )
            .await?;
        state.log(
            "INFO",
            &format!(
                "Rust preview started on port {} as {}",
                state.port,
                state.network.local().id
            ),
        );
        if let Some(error) = sound_error {
            state.log("WARN", &format!("提示音服务未启动：{error}"));
        }
        Ok(state)
    }
    pub fn info(&self) -> Value {
        json!({"success":true,"version":"0.1.0","port":self.port,"dataDir":self.data_dir.to_string_lossy(),
            "capabilities":{"images":true,"imageReceive":true,"imageSend":true,"files":self.network.file_transfers().is_ok(),"screenshot":cfg!(windows),"scan":true,"notificationSound":self.sound.as_ref().is_some_and(|s|s.available())}})
    }
    pub fn log(&self, level: &str, message: &str) {
        if level == "DEBUG" && !self.verbose {
            return;
        }
        let text = format!(
            "[{}] [{level}] {message}\n",
            ipmsg_core::network::unix_seconds()
        );
        eprint!("{text}");
        if let Ok(mut file) = self.log_file.lock() {
            let _ = file.write_all(text.as_bytes());
            let _ = file.flush();
        }
    }
    pub fn start_events(self: &Arc<Self>, app: AppHandle) {
        let receiver = self.events.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut receiver) = receiver {
            let state = self.clone();
            let task = tauri::async_runtime::spawn(async move {
                let mut notifications = crate::notification::NotificationGate::default();
                while let Some(event) = receiver.recv().await {
                    if event.event == "network.diagnostic" {
                        state.log(
                            event.payload["level"].as_str().unwrap_or("DEBUG"),
                            event.payload["message"]
                                .as_str()
                                .unwrap_or("网络诊断事件无内容"),
                        );
                        continue;
                    }
                    state.log("DEBUG", &format!("Event {}", event.event));
                    if event.event == "notification.sound_failed" {
                        state.log(
                            "WARN",
                            &format!(
                                "提示音播放失败：{}",
                                event.payload["error"].as_str().unwrap_or("未知设备错误")
                            ),
                        );
                    }
                    if event.event == "message.received" {
                        let window = app.get_webview_window("main");
                        let active = state
                            .active_conversation
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone();
                        let local = state.network.local();
                        let context = crate::notification::Context {
                            enabled: state.sound.as_ref().is_some_and(|sound| sound.enabled()),
                            accepting: state.accepting.load(Ordering::Acquire),
                            visible: window
                                .as_ref()
                                .is_some_and(|window| window.is_visible().unwrap_or(false)),
                            focused: window
                                .as_ref()
                                .is_some_and(|window| window.is_focused().unwrap_or(false)),
                            minimized: window
                                .as_ref()
                                .is_some_and(|window| window.is_minimized().unwrap_or(false)),
                            active_conversation: &active,
                            local_id: &local.id,
                        };
                        if notifications.should_play(&event, context, std::time::Instant::now()) {
                            if let Some(sound) = &state.sound {
                                sound.notify();
                            }
                        }
                        if let Some(window) = window {
                            if !window.is_focused().unwrap_or(false) {
                                let _ = window.request_user_attention(Some(
                                    tauri::UserAttentionType::Informational,
                                ));
                            }
                        }
                    }
                    if let Err(error) = app.emit_to("main", "ipmsg-event", event) {
                        state.log("ERROR", &error.to_string());
                    }
                }
            });
            *self.event_task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        }
    }
}

pub fn request_exit(app: &AppHandle) {
    let state = app.state::<Arc<Runtime>>().inner().clone();
    if state.quitting.swap(true, Ordering::AcqRel) {
        return;
    }
    state.accepting.store(false, Ordering::Release);
    if let Some(sound) = &state.sound {
        sound.close();
    }
    crate::capture::shutdown(app, &state);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        state.log("INFO", "Shutdown requested");
        if let Some(sound) = &state.sound {
            match tokio::time::timeout(Duration::from_secs(2), sound.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => state.log("WARN", &error),
                Err(_) => state.log("WARN", "提示音服务关闭超时"),
            }
        }
        if tokio::time::timeout(Duration::from_secs(3), state.capture.wait_idle())
            .await
            .is_err()
        {
            state.log(
                "WARN",
                "Screenshot cleanup exceeded deadline; shutdown continues",
            );
        }
        if tokio::time::timeout(Duration::from_secs(5), state.network.shutdown())
            .await
            .is_err()
        {
            state.log("ERROR", "Network shutdown exceeded deadline");
        }
        if tokio::time::timeout(Duration::from_secs(3), state.database.shutdown())
            .await
            .is_err()
        {
            state.log("ERROR", "Database shutdown exceeded deadline");
        }
        if let Some(task) = state
            .event_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
        state.finished.store(true, Ordering::Release);
        app.exit(0);
    });
}
