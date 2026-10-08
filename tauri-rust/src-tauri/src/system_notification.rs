//! Windows notification centre integration. The worker owns its own toasts only.
#[cfg(windows)]
mod identity;
use ipmsg_core::network::Event;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct Notice {
    pub title: String,
    pub body: String,
    pub target: Option<String>,
}
pub fn notice(event: &Event, preview: bool) -> Notice {
    let name = event.payload["fromUser"]["nickname"]
        .as_str()
        .unwrap_or("新消息");
    let kind = event.payload["type"].as_str().unwrap_or("text");
    let body = if preview && kind == "text" {
        event.payload["content"].as_str().unwrap_or("新消息")
    } else {
        match kind {
            "image" => "收到一张图片",
            "file" => "收到文件邀请",
            _ => "收到一条新消息",
        }
    };
    Notice {
        title: format!("迅秋 · {}", clean(name, 80)),
        body: clean(body, 240),
        target: event.payload["from"].as_str().map(str::to_owned),
    }
}
fn clean(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| {
            (!c.is_control() || *c == '\n' || *c == '\t') && *c != '\u{fffe}' && *c != '\u{ffff}'
        })
        .take(limit)
        .collect()
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn xml(n: &Notice) -> String {
    format!("<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual><audio silent=\"true\"/></toast>",escape(&n.title),escape(&n.body))
}
struct Shared {
    enabled: AtomicBool,
    closed: AtomicBool,
    epoch: AtomicU64,
}
enum Command {
    Show(
        Notice,
        u64,
        Instant,
        Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ),
    Wake,
}
pub struct SystemNotifications {
    shared: Arc<Shared>,
    tx: mpsc::SyncSender<Command>,
    task: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl SystemNotifications {
    pub fn new(
        enabled: bool,
        port: u16,
        events: tokio::sync::mpsc::Sender<Event>,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(enabled),
            closed: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
        });
        let (tx, rx) = mpsc::sync_channel(8);
        let state = shared.clone();
        let task = std::thread::Builder::new()
            .name("ipmsg-system-notifications".into())
            .spawn(move || {
                let mut backend = Native::default();
                let mut epoch = 0;
                let mut last_error: Option<Instant> = None;
                while let Ok(command) = rx.recv() {
                    if state.closed.load(Ordering::Acquire) {
                        break;
                    }
                    let current = state.epoch.load(Ordering::Acquire);
                    if current != epoch {
                        backend.clear();
                        epoch = current;
                    }
                    let Command::Show(notice, generation, at, reply) = command else {
                        continue;
                    };
                    if generation != current
                        || at.elapsed() > Duration::from_secs(3)
                        || (reply.is_none() && !state.enabled.load(Ordering::Acquire))
                    {
                        if let Some(reply) = reply {
                            let _ = reply.send(Err("通知请求已取消".into()));
                        }
                        continue;
                    }
                    let valid = || {
                        state.epoch.load(Ordering::Acquire) == generation
                            && !state.closed.load(Ordering::Acquire)
                            && at.elapsed() < Duration::from_secs(3)
                    };
                    let mut result = backend.show(&notice, port, events.clone(), &valid);
                    if !valid() {
                        backend.clear();
                        result = Err("通知请求已取消或超时".into());
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    } else if let Err(error) = result {
                        if last_error.is_none_or(|at| at.elapsed() > Duration::from_secs(60)) {
                            last_error = Some(Instant::now());
                            let _ = events.try_send(Event {
                                event: "notification.system_failed".into(),
                                payload: json!({"error":error}),
                            });
                        }
                    }
                }
                backend.clear();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            tx,
            task: Mutex::new(Some(task)),
        })
    }
    pub fn set_enabled(&self, enabled: bool) {
        if self.shared.enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            let _ = self.tx.try_send(Command::Wake);
        }
    }
    pub fn invalidate(&self) {
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.try_send(Command::Wake);
    }
    pub fn notify(&self, notice: Notice) {
        if !self.shared.closed.load(Ordering::Acquire) {
            let _ = self.tx.try_send(Command::Show(
                notice,
                self.shared.epoch.load(Ordering::Acquire),
                Instant::now(),
                None,
            ));
        }
    }
    pub async fn test(&self) -> Result<(), String> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err("通知服务已停止".into());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .try_send(Command::Show(
                Notice {
                    title: "迅秋".into(),
                    body: "系统通知测试成功".into(),
                    target: None,
                },
                self.shared.epoch.load(Ordering::Acquire),
                Instant::now(),
                Some(tx),
            ))
            .map_err(|_| "通知队列繁忙")?;
        tokio::time::timeout(Duration::from_secs(4), rx)
            .await
            .map_err(|_| "系统通知测试超时")?
            .map_err(|_| "通知服务已停止")?
    }
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.try_send(Command::Wake);
    }
    pub async fn shutdown(&self) {
        self.close();
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            let _ = tokio::task::spawn_blocking(move || task.join()).await;
        }
    }
}
impl Drop for SystemNotifications {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(windows)]
fn check_notification_setting(
    setting: windows::core::Result<windows::UI::Notifications::NotificationSetting>,
) -> Result<(), String> {
    use windows::{core::HRESULT, UI::Notifications::NotificationSetting};
    match setting {
        Ok(NotificationSetting::Enabled) => Ok(()),
        // A newly registered desktop AUMID may have no settings record until
        // its first Show. This is not a disabled setting: let Show report the
        // actual submission result instead of blocking that first notification.
        Err(error) if error.code() == HRESULT(0x80070490u32 as i32) => Ok(()),
        Ok(_) => Err("Windows已禁用此应用的通知，请检查系统通知设置".into()),
        Err(error) => Err(format!("读取通知设置 ToastNotifier.Setting 失败：{error}")),
    }
}

#[cfg(windows)]
#[derive(Default)]
struct Native {
    notifier: Option<windows::UI::Notifications::ToastNotifier>,
    active: VecDeque<(windows::UI::Notifications::ToastNotification, i64)>,
    initialized: bool,
    sequence: u64,
}
#[cfg(windows)]
impl Native {
    fn initialize(&mut self) -> Result<(), String> {
        use windows::{
            core::HSTRING,
            Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED},
            UI::Notifications::ToastNotificationManager,
        };
        if self.notifier.is_some() {
            return Ok(());
        }
        if !self.initialized {
            unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
                .map_err(|e| format!("初始化通知线程 RoInitialize 失败：{e}"))?;
            self.initialized = true;
        }
        register_identity()?;
        self.notifier = Some(
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(identity::APP_ID))
                .map_err(|e| format!("创建通知器 CreateToastNotifierWithId 失败：{e}"))?,
        );
        Ok(())
    }
    fn show(
        &mut self,
        n: &Notice,
        port: u16,
        events: tokio::sync::mpsc::Sender<Event>,
        valid: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        use windows::{
            core::{IInspectable, HSTRING},
            Data::Xml::Dom::XmlDocument,
            Foundation::TypedEventHandler,
            UI::Notifications::ToastNotification,
        };
        self.initialize()?;
        let notifier = self.notifier.as_ref().ok_or("通知服务不可用")?;
        if let Err(error) = check_notification_setting(notifier.Setting()) {
            self.notifier = None;
            return Err(error);
        }
        let document = XmlDocument::new().map_err(|e| format!("创建通知 XML 失败：{e}"))?;
        document
            .LoadXml(&HSTRING::from(xml(n)))
            .map_err(|e| format!("加载通知 XML 失败：{e}"))?;
        let toast = ToastNotification::CreateToastNotification(&document)
            .map_err(|e| format!("创建 ToastNotification 失败：{e}"))?;
        self.sequence += 1;
        toast
            .SetTag(&HSTRING::from(format!("{:x}", self.sequence)))
            .map_err(|e| format!("设置通知标记 SetTag 失败：{e}"))?;
        toast
            .SetGroup(&HSTRING::from(format!("p{port}")))
            .map_err(|e| format!("设置通知分组 SetGroup 失败：{e}"))?;
        let target = n.target.clone();
        let token = toast
            .Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, _| {
                    if let Some(target) = &target {
                        let event = Event {
                            event: "notification.activated".into(),
                            payload: json!({"userId":target}),
                        };
                        let sender = events.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = sender.send(event).await;
                        });
                    }
                    Ok(())
                },
            ))
            .map_err(|e| format!("订阅通知点击 Activated 失败：{e}"))?;
        if !valid() {
            let _ = toast.RemoveActivated(token);
            return Err("通知请求已取消".into());
        }
        if let Err(error) = notifier.Show(&toast) {
            let _ = toast.RemoveActivated(token);
            self.notifier = None;
            return Err(format!("显示通知 ToastNotifier.Show 失败：{error}"));
        }
        self.active.push_back((toast, token));
        while self.active.len() > 16 {
            if let Some((toast, token)) = self.active.pop_front() {
                let _ = toast.RemoveActivated(token);
                let _ = notifier.Hide(&toast);
            }
        }
        Ok(())
    }
    fn clear(&mut self) {
        for (toast, token) in self.active.drain(..) {
            let _ = toast.RemoveActivated(token);
            if let Some(notifier) = &self.notifier {
                let _ = notifier.Hide(&toast);
            }
        }
    }
}
#[cfg(windows)]
impl Drop for Native {
    fn drop(&mut self) {
        self.clear();
        self.notifier = None;
        if self.initialized {
            unsafe {
                windows::Win32::System::WinRT::RoUninitialize();
            }
        }
    }
}
#[cfg(windows)]
fn register_identity() -> Result<(), String> {
    identity::register()?;
    use windows_sys::Win32::System::Registry::*;
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
    let name: Vec<u16> = format!("Software\\Classes\\AppUserModelId\\{}", identity::APP_ID)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut key = std::ptr::null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            name.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return Err(format!("注册系统通知身份失败：{result}"));
    }
    let key = Key(key);
    let field: Vec<u16> = "DisplayName".encode_utf16().chain(Some(0)).collect();
    let value: Vec<u16> = "迅秋 Rust".encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        RegSetValueExW(
            key.0,
            field.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    if result != 0 {
        return Err(format!("注册通知名称失败：{result}"));
    }
    Ok(())
}
#[cfg(not(windows))]
#[derive(Default)]
struct Native;
#[cfg(not(windows))]
impl Native {
    fn clear(&mut self) {}
    fn show(
        &mut self,
        _: &Notice,
        _: u16,
        _: tokio::sync::mpsc::Sender<Event>,
        _: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        Err("系统通知需要Windows桌面版".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn missing_first_notification_setting_allows_submission() {
        use windows::{
            core::{Error, HRESULT},
            UI::Notifications::NotificationSetting,
        };
        assert!(check_notification_setting(Err(Error::from_hresult(HRESULT(
            0x80070490u32 as i32
        ))))
        .is_ok());
        assert!(check_notification_setting(Ok(NotificationSetting::Enabled)).is_ok());
    }
    #[cfg(windows)]
    #[test]
    fn disabled_settings_and_other_query_errors_still_block_submission() {
        use windows::{
            core::{Error, HRESULT},
            UI::Notifications::NotificationSetting,
        };
        for setting in [
            NotificationSetting::DisabledForApplication,
            NotificationSetting::DisabledForUser,
            NotificationSetting::DisabledByGroupPolicy,
            NotificationSetting::DisabledByManifest,
        ] {
            assert!(check_notification_setting(Ok(setting))
                .unwrap_err()
                .contains("Windows已禁用"));
        }
        for code in [0x80070005u32, 0x80004005u32] {
            let error = check_notification_setting(Err(Error::from_hresult(HRESULT(code as i32))))
                .unwrap_err();
            assert!(error.contains("ToastNotifier.Setting"));
            assert!(error.contains(&format!("{code:08X}")));
        }
    }
    #[test]
    fn text_is_private_by_default_and_xml_is_escaped() {
        let event = Event {
            event: "message.received".into(),
            payload: json!({"from":"peer","fromUser":{"nickname":"<张&李>"},"type":"text","content":"<script>秘密\u{0}"}),
        };
        assert!(!xml(&notice(&event, false)).contains("秘密"));
        let data = xml(&notice(&event, true));
        assert!(data.contains("&lt;张&amp;李&gt;"));
        assert!(data.contains("&lt;script&gt;秘密"));
        assert!(!data.contains('\0'));
        assert!(data.contains("silent=\"true\""));
    }
    #[test]
    fn file_and_image_notifications_never_expose_paths() {
        for kind in ["image", "file"] {
            let event = Event {
                event: "message.received".into(),
                payload: json!({"from":"peer","type":kind,"content":"C:\\private\\secret"}),
            };
            assert!(!notice(&event, true).body.contains("private"));
        }
    }
}
