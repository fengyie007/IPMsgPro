mod capture;
mod commands;
mod files;
mod image;
mod notification;
mod platform;
mod runtime;
mod storage;
mod system_notification;

use runtime::{request_exit, Runtime};
use std::sync::{atomic::Ordering, Arc};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<Arc<Runtime>>() {
        if state.quitting.load(Ordering::Acquire) {
            return;
        }
        if let Some(label) = state.capture.active_label() {
            if let Some(editor) = app.get_webview_window(&label) {
                let _ = editor.set_focus();
            }
            return;
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.request_user_attention(None);
    }
}
fn create_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("迅秋 Rust 预览版")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => request_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn startup_error(error: &str) {
    eprintln!("Rust预览版启动失败：{error}");
    // Startup can fail before any webview exists; a native error is visible in Release too.
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let text: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "迅秋 Rust 预览版启动失败"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

pub fn run() {
    let options = match platform::Options::parse() {
        Ok(options) => options,
        Err(error) => {
            startup_error(&error);
            return;
        }
    };
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("ipmsg-image", image::serve)
        .register_asynchronous_uri_scheme_protocol("ipmsg-capture", capture::serve)
        .invoke_handler(tauri::generate_handler![
            image::import_clipboard_image,
            commands::ipmsg_command,
            capture::screenshot_command,
            capture::screenshot_confirm
        ])
        .setup(move |app| {
            let state =
                tauri::async_runtime::block_on(Runtime::create(app.handle(), options.clone()))
                    .map_err(std::io::Error::other)?;
            app.manage(state.clone());
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(format!("迅秋 Rust 预览版 — {}", state.port))
                .inner_size(960.0, 640.0)
                .min_inner_size(720.0, 480.0)
                .center()
                .data_directory(state.data_dir.join("webview"))
                .devtools(cfg!(debug_assertions))
                .build()?;
            match create_tray(app.handle()) {
                Ok(()) => state.tray_available.store(true, Ordering::Release),
                Err(error) => state.log(
                    "ERROR",
                    &format!("托盘创建失败，关闭窗口将退出而非隐藏：{error}"),
                ),
            }
            state.start_events(app.handle().clone());
            tauri::async_runtime::block_on(state.network.start());
            Ok(())
        })
        .on_webview_event(|webview, event| {
            if webview.label() == "main" {
                if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) =
                    event
                {
                    if webview.url().is_ok_and(|url| capture::local_url(&url)) {
                        files::dropped(webview.app_handle().clone(), paths.clone());
                    }
                }
            }
        })
        .on_window_event(|window, event| {
            if window.label().starts_with("capture-") {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                }
                if matches!(
                    event,
                    WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
                ) {
                    if let Some(state) = window.try_state::<Arc<Runtime>>() {
                        capture::window_closed(window.app_handle(), state.inner(), window.label());
                    }
                }
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                if let Some(state) = window.try_state::<Arc<Runtime>>() {
                    if state.finished.load(Ordering::Acquire) {
                        return;
                    }
                    api.prevent_close();
                    if !state.quitting.load(Ordering::Acquire)
                        && state.config.get().minimize_behavior == "tray"
                        && state.tray_available.load(Ordering::Acquire)
                    {
                        if let Err(error) = window.hide() {
                            state.log("ERROR", &error.to_string());
                        }
                    } else {
                        request_exit(window.app_handle());
                    }
                }
            }
        })
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(|app, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                if let Some(state) = app.try_state::<Arc<Runtime>>() {
                    if !state.finished.load(Ordering::Acquire) {
                        api.prevent_exit();
                        request_exit(app);
                    }
                }
            }
        }),
        Err(error) => startup_error(&error.to_string()),
    }
}
