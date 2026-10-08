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
            active_conversation: Mutex::new(String::new()),
            events: Mutex::new(Some(rx)),
            event_task: Mutex::new(None),
            log_file: Mutex::new(log_file),
            verbose: options.verbose,
        });
        state.log(
            "INFO",
            &format!(
                "Rust preview started on port {} as {}",
                state.port,
                state.network.local().id
            ),
        );
        Ok(state)
    }
    pub fn info(&self) -> Value {
        json!({"success":true,"version":"0.1.0","port":self.port,"dataDir":self.data_dir.to_string_lossy(),
            "capabilities":{"images":true,"imageReceive":true,"imageSend":true,"files":false,"screenshot":cfg!(windows),"scan":false,"notificationSound":false}})
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
                    if event.event == "message.received" {
                        if let Some(window) = app.get_webview_window("main") {
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
    crate::capture::shutdown(app, &state);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        state.log("INFO", "Shutdown requested");
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
