//! A single, cancellable screenshot session. The editor never receives main-window IPC access.
mod platform;

use crate::runtime::Runtime;
use ipmsg_core::image::{dib::MAX_IMPORT_BYTES, ImageMetadata};
use platform::Bounds;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{
    http::{Request, Response, StatusCode},
    Manager, PhysicalPosition, PhysicalSize, State, UriSchemeContext, UriSchemeResponder,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

type Outcome = Result<Option<ImageMetadata>, String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Preparing,
    Loading,
    Editing,
    Importing,
}

struct Token {
    id: String,
    label: String,
    cancelled: AtomicBool,
    // Workers retain this permit after cancellation until their results are reclaimed.
    _permit: OwnedSemaphorePermit,
}
struct Session {
    token: Arc<Token>,
    phase: Phase,
    frame: Option<(Bounds, Arc<Vec<u8>>)>,
    reply: oneshot::Sender<Outcome>,
}
pub struct Capture {
    session: Mutex<Option<Session>>,
    gate: Arc<Semaphore>,
    next: AtomicU64,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            session: Mutex::new(None),
            gate: Arc::new(Semaphore::new(1)),
            next: AtomicU64::new(0),
        }
    }
}
impl Capture {
    fn reserve(&self) -> Result<(Arc<Token>, oneshot::Receiver<Outcome>), String> {
        let permit = self
            .gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| "已有截图或截图清理正在进行")?;
        let id = format!(
            "{}-{}",
            std::process::id(),
            self.next.fetch_add(1, Ordering::Relaxed)
        );
        let token = Arc::new(Token {
            label: format!("capture-{id}"),
            id,
            cancelled: AtomicBool::new(false),
            _permit: permit,
        });
        let (reply, receiver) = oneshot::channel();
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(Session {
            token: token.clone(),
            phase: Phase::Preparing,
            frame: None,
            reply,
        });
        Ok((token, receiver))
    }
    fn authorize(&self, label: &str, id: &str) -> Result<Arc<Token>, String> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|s| {
                s.token.label == label
                    && s.token.id == id
                    && !s.token.cancelled.load(Ordering::Acquire)
            })
            .map(|s| s.token.clone())
            .ok_or_else(|| "截图会话已结束或此窗口无权访问".into())
    }
    fn transition(&self, token: &Token, from: Phase, to: Phase) -> Result<(), String> {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let s = session
            .as_mut()
            .filter(|s| {
                s.token.id == token.id
                    && s.phase == from
                    && !token.cancelled.load(Ordering::Acquire)
            })
            .ok_or("截图会话状态已改变")?;
        s.phase = to;
        Ok(())
    }
    fn take(&self, token: &Token) -> Option<Session> {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(|s| s.token.id == token.id) {
            token.cancelled.store(true, Ordering::Release);
            session.take()
        } else {
            None
        }
    }
    pub fn active_label(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|s| s.token.label.clone())
    }
    pub async fn wait_idle(&self) {
        let _permit = self.gate.acquire().await;
    }
}

pub fn local_url(url: &tauri::Url) -> bool {
    (url.scheme() == "tauri" && url.host_str() == Some("localhost") && url.port().is_none())
        || (matches!(url.scheme(), "http" | "https")
            && url.host_str() == Some("tauri.localhost")
            && url.port().is_none())
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(1420))
}
pub fn local_window(window: &WebviewWindow) -> bool {
    window.url().is_ok_and(|url| local_url(&url))
}

fn close_editor(app: &tauri::AppHandle, state: &Runtime, label: &str) {
    if let Some(editor) = app.get_webview_window(label) {
        for result in [
            editor.set_always_on_top(false),
            editor.hide(),
            editor.destroy(),
        ] {
            if let Err(error) = result {
                state.log("ERROR", &format!("关闭截图窗口失败：{error}"));
            }
        }
    }
}
fn discard_late(state: Arc<Runtime>, image: ImageMetadata, token: Arc<Token>) {
    tauri::async_runtime::spawn(async move {
        let _token = token;
        if let Err(error) = state.network.discard_image(&image.asset_id).await {
            state.log("WARN", &format!("截图资产清理待重试：{error}"));
        }
    });
}
fn finish(app: &tauri::AppHandle, state: &Arc<Runtime>, token: &Arc<Token>, outcome: Outcome) {
    let Some(session) = state.capture.take(token) else {
        if let Ok(Some(image)) = outcome {
            discard_late(state.clone(), image, token.clone());
        }
        return;
    };
    close_editor(app, state, &token.label);
    // Check again on the UI thread so queued cleanup cannot reopen a quitting app.
    let restore_app = app.clone();
    let restore_state = state.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if restore_state.accepting.load(Ordering::Acquire)
            && !restore_state.quitting.load(Ordering::Acquire)
            && restore_state.capture.active_label().is_none()
        {
            if let Some(main) = restore_app.get_webview_window("main") {
                for result in [main.show(), main.unminimize(), main.set_focus()] {
                    if let Err(error) = result {
                        restore_state.log("ERROR", &format!("恢复主窗口失败：{error}"));
                    }
                }
            }
        }
    }) {
        state.log("ERROR", &format!("调度主窗口恢复失败：{error}"));
    }
    if let Err(Ok(Some(image))) = session.reply.send(outcome) {
        discard_late(state.clone(), image, token.clone());
    }
}

// Also covers a dropped command future, timeout, and unexpected editor destruction.
struct SessionGuard {
    app: tauri::AppHandle,
    state: Arc<Runtime>,
    token: Arc<Token>,
}

// A timed-out WebView creation can still complete on its worker. Keep ownership
// with the result so dropping an unobserved result also destroys that window.
struct PendingEditor {
    window: WebviewWindow,
    state: Arc<Runtime>,
    token: Arc<Token>,
    retained: bool,
}
impl Drop for PendingEditor {
    fn drop(&mut self) {
        if !self.retained {
            if !self.token.cancelled.load(Ordering::Acquire) {
                // Remove the session before Destroyed is emitted; an initialization
                // failure must not race into a successful user-cancel result.
                finish(
                    self.window.app_handle(),
                    &self.state,
                    &self.token,
                    Err("截图编辑器初始化中断".into()),
                );
            }
            close_editor(self.window.app_handle(), &self.state, &self.token.label);
        }
    }
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        finish(&self.app, &self.state, &self.token, Ok(None));
    }
}

pub async fn start(main: &WebviewWindow, state: Arc<Runtime>) -> Result<Value, String> {
    if !cfg!(windows) {
        return Err("当前平台暂不支持截图".into());
    }
    if state.image_selecting.swap(true, Ordering::AcqRel) {
        return Err("请先完成图片选择或截图".into());
    }
    let _selection = crate::image::SelectionGuard(state.clone());
    let (token, receiver) = state.capture.reserve()?;
    let app = main.app_handle().clone();
    let _guard = SessionGuard {
        app: app.clone(),
        state: state.clone(),
        token: token.clone(),
    };
    if !state.accepting.load(Ordering::Acquire) {
        return Err("程序正在退出".into());
    }
    let prepared = tokio::time::timeout(
        Duration::from_secs(20),
        prepare(main, state.clone(), token.clone()),
    )
    .await;
    match prepared {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            finish(&app, &state, &token, Err(error));
        }
        Err(_) => {
            finish(&app, &state, &token, Err("截图初始化超时".into()));
        }
    }
    // No detached long-lived timer: the same command owns both the editor deadline and result.
    let outcome = tokio::time::timeout(Duration::from_secs(15 * 60), receiver).await;
    match outcome {
        Ok(Ok(Ok(Some(image)))) => Ok(json!({"success":true,"image":image})),
        Ok(Ok(Ok(None))) => Ok(json!({"success":true,"cancelled":true})),
        Ok(Ok(Err(error))) => Err(error),
        Ok(Err(_)) => Err("截图会话已中断".into()),
        Err(_) => {
            finish(
                &app,
                &state,
                &token,
                Err("截图会话已超时，请重新截图".into()),
            );
            Err("截图会话已超时，请重新截图".into())
        }
    }
}

async fn prepare(
    main: &WebviewWindow,
    state: Arc<Runtime>,
    token: Arc<Token>,
) -> Result<(), String> {
    let monitor = main
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("找不到主窗口所在显示器")?;
    let bounds = Bounds {
        x: monitor.position().x,
        y: monitor.position().y,
        width: monitor.size().width,
        height: monitor.size().height,
    }
    .validate()?;
    main.hide().map_err(|e| e.to_string())?;
    // A UI queue barrier ensures hide has been processed before the worker's DwmFlush.
    let (hidden, wait_hidden) = oneshot::channel();
    main.run_on_main_thread(move || {
        let _ = hidden.send(());
    })
    .map_err(|e| e.to_string())?;
    wait_hidden.await.map_err(|_| "隐藏主窗口被中断")?;
    let worker_token = token.clone();
    let png = tokio::task::spawn_blocking(move || {
        if worker_token.cancelled.load(Ordering::Acquire) {
            return Err("截图已取消".into());
        }
        platform::capture(bounds)
    })
    .await
    .map_err(|e| e.to_string())??;
    {
        let mut session = state
            .capture
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let s = session
            .as_mut()
            .filter(|s| s.token.id == token.id && !token.cancelled.load(Ordering::Acquire))
            .ok_or("截图已取消")?;
        s.frame = Some((bounds, Arc::new(png)));
        s.phase = Phase::Loading;
    }
    let build_app = main.app_handle().clone();
    let build_state = state.clone();
    let build_token = token.clone();
    let mut pending = tokio::task::spawn_blocking(move || {
        let window = WebviewWindowBuilder::new(
            &build_app,
            &build_token.label,
            WebviewUrl::App(format!("index.html?capture={}", build_token.id).into()),
        )
        .title("截图与标注")
        .decorations(false)
        .resizable(false)
        .visible(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .devtools(cfg!(debug_assertions))
        .on_navigation(local_url)
        .build()
        .map_err(|e| e.to_string())?;
        Ok::<_, String>(PendingEditor {
            window,
            state: build_state,
            token: build_token,
            retained: false,
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    let editor = &pending.window;
    if token.cancelled.load(Ordering::Acquire) || !state.accepting.load(Ordering::Acquire) {
        close_editor(main.app_handle(), &state, &token.label);
        return Err("截图已取消".into());
    }
    editor
        .set_position(PhysicalPosition::new(bounds.x, bounds.y))
        .map_err(|e| e.to_string())?;
    editor
        .set_size(PhysicalSize::new(bounds.width, bounds.height))
        .map_err(|e| e.to_string())?;
    editor.set_fullscreen(true).map_err(|e| e.to_string())?;
    // Hidden webviews load first; ready is sent only after the PNG has decoded.
    for _ in 0..150 {
        if token.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let loading = state
            .capture
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|s| s.token.id == token.id && s.phase == Phase::Loading);
        if !loading {
            editor
                .show()
                .and_then(|_| editor.set_focus())
                .map_err(|e| e.to_string())?;
            if token.cancelled.load(Ordering::Acquire) {
                close_editor(main.app_handle(), &state, &token.label);
            } else {
                pending.retained = true;
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("截图编辑器加载超时".into())
}

pub fn window_closed(app: &tauri::AppHandle, state: &Arc<Runtime>, label: &str) {
    let token = state
        .capture
        .session
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|s| s.token.label == label)
        .map(|s| s.token.clone());
    if let Some(token) = token {
        finish(app, state, &token, Ok(None));
    }
}
pub fn shutdown(app: &tauri::AppHandle, state: &Arc<Runtime>) {
    if let Some(label) = state.capture.active_label() {
        window_closed(app, state, &label);
    }
}

#[tauri::command]
pub async fn screenshot_command(
    state: State<'_, Arc<Runtime>>,
    window: WebviewWindow,
    session_id: String,
    command: String,
    error: Option<String>,
) -> Result<Value, String> {
    if !local_window(&window) || !state.accepting.load(Ordering::Acquire) {
        return Err("截图窗口不可用".into());
    }
    let token = state.capture.authorize(window.label(), &session_id)?;
    match command.as_str() {
        "read" => {
            let session = state
                .capture
                .session
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let (bounds, _) = session
                .as_ref()
                .and_then(|s| s.frame.as_ref())
                .ok_or("截图尚未就绪")?;
            let url = if cfg!(windows) {
                format!("http://ipmsg-capture.localhost/{}/frame", token.id)
            } else {
                format!("ipmsg-capture://localhost/{}/frame", token.id)
            };
            Ok(json!({"url":url,"bounds":bounds}))
        }
        "ready" => {
            if state
                .capture
                .transition(&token, Phase::Loading, Phase::Editing)
                .is_err()
            {
                let editing = state
                    .capture
                    .session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .is_some_and(|s| s.token.id == token.id && s.phase == Phase::Editing);
                if !editing {
                    return Err("截图会话状态已改变".into());
                }
            }
            if token.cancelled.load(Ordering::Acquire) {
                return Err("截图已取消".into());
            }
            Ok(json!({"success":true}))
        }
        "cancel" => {
            let outcome = error
                .map(|e| Err(e.chars().take(512).collect()))
                .unwrap_or(Ok(None));
            finish(window.app_handle(), &state, &token, outcome);
            Ok(json!({"success":true}))
        }
        _ => Err("截图窗口不允许此操作".into()),
    }
}

#[tauri::command]
pub async fn screenshot_confirm(
    state: State<'_, Arc<Runtime>>,
    window: WebviewWindow,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    if !local_window(&window) || !state.accepting.load(Ordering::Acquire) {
        return Err("截图窗口不可用".into());
    }
    let id = request
        .headers()
        .get("x-capture-session")
        .and_then(|v| v.to_str().ok())
        .ok_or("缺少截图会话")?;
    let token = state.capture.authorize(window.label(), id)?;
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes)
            if bytes.len() <= MAX_IMPORT_BYTES && bytes.starts_with(b"\x89PNG\r\n\x1a\n") =>
        {
            bytes.clone()
        }
        _ => {
            finish(
                window.app_handle(),
                &state,
                &token,
                Err("截图必须是20 MiB以内的PNG".into()),
            );
            return Err("截图格式或大小无效".into());
        }
    };
    state
        .capture
        .transition(&token, Phase::Editing, Phase::Importing)?;
    let app = window.app_handle().clone();
    let state = state.inner().clone();
    let task_state = state.clone();
    let task_app = app.clone();
    let task_token = token.clone();
    let task = tauri::async_runtime::spawn(async move {
        let outcome = task_state.network.import_screenshot(bytes).await.map(Some);
        finish(&task_app, &task_state, &task_token, outcome);
    });
    // If a slow import finishes after this deadline/cancellation, finish discards its asset.
    match tokio::time::timeout(Duration::from_secs(30), task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => finish(&app, &state, &token, Err(format!("截图导入失败：{error}"))),
        Err(_) => finish(&app, &state, &token, Err("截图导入超时".into())),
    }
    Ok(())
}

pub fn serve(
    context: UriSchemeContext<'_, tauri::Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle();
    let window = app.get_webview_window(context.webview_label());
    let state = app.try_state::<Arc<Runtime>>();
    let parts: Vec<_> = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect();
    let allowed = window.as_ref().is_some_and(local_window)
        && request.method() == "GET"
        && parts.len() == 2
        && parts[1] == "frame"
        && state.as_ref().is_some_and(|s| {
            s.accepting.load(Ordering::Acquire)
                && s.capture
                    .authorize(context.webview_label(), parts[0])
                    .is_ok()
        });
    let frame = if allowed {
        state.as_ref().and_then(|s| {
            s.capture
                .session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .filter(|session| session.token.id == parts[0])
                .and_then(|session| session.frame.as_ref().map(|(_, bytes)| bytes.clone()))
        })
    } else {
        None
    };
    let origin = window
        .and_then(|w| w.url().ok())
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_default();
    let mut response = Response::builder()
        .status(if frame.is_some() {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        })
        .header("Content-Type", "image/png")
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Vary", "Origin");
    if allowed {
        response = response.header("Access-Control-Allow-Origin", origin);
    }
    responder.respond(
        response
            .body(
                frame
                    .map(|bytes| bytes.as_ref().clone())
                    .unwrap_or_default(),
            )
            .unwrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_scope_and_duplicate_confirmation() {
        let capture = Capture::default();
        let (token, _reply) = capture.reserve().unwrap();
        assert!(capture.reserve().is_err());
        assert!(capture.authorize("main", &token.id).is_err());
        assert!(capture.authorize(&token.label, "stale").is_err());
        assert!(capture.authorize(&token.label, &token.id).is_ok());
        assert!(capture
            .transition(&token, Phase::Editing, Phase::Importing)
            .is_err());
        capture
            .transition(&token, Phase::Preparing, Phase::Loading)
            .unwrap();
        capture
            .transition(&token, Phase::Loading, Phase::Editing)
            .unwrap();
        capture
            .transition(&token, Phase::Editing, Phase::Importing)
            .unwrap();
        assert!(capture
            .transition(&token, Phase::Editing, Phase::Importing)
            .is_err());
    }
    #[test]
    fn cancellation_rejects_late_results_and_holds_worker_budget() {
        let capture = Capture::default();
        let (token, _reply) = capture.reserve().unwrap();
        let worker = token.clone();
        drop(capture.take(&token));
        assert!(token.cancelled.load(Ordering::Acquire));
        assert!(capture.authorize(&token.label, &token.id).is_err());
        assert!(capture
            .transition(&token, Phase::Preparing, Phase::Loading)
            .is_err());
        assert!(capture.take(&token).is_none());
        drop(token);
        assert!(capture.reserve().is_err());
        drop(worker);
        assert!(capture.reserve().is_ok());
    }
    #[test]
    fn editor_origin_is_local_only() {
        assert!(local_url(
            &"http://tauri.localhost/index.html?capture=1"
                .parse()
                .unwrap()
        ));
        for url in [
            "https://example.com",
            "file:///C:/secret",
            "http://localhost:9000",
            "http://tauri.localhost.evil/index.html",
            "http://tauri.localhost:9000/index.html",
        ] {
            assert!(!local_url(&url.parse().unwrap()));
        }
    }
}
