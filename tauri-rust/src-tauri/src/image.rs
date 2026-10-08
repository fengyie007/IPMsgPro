use crate::runtime::Runtime;
use ipmsg_core::image::assets::valid_asset_id;
use std::sync::{atomic::Ordering, Arc};
use tauri::{
    http::{Method, Request, Response, StatusCode},
    Manager, UriSchemeContext, UriSchemeResponder,
};

pub(crate) struct SelectionGuard(pub Arc<Runtime>);
impl Drop for SelectionGuard {
    fn drop(&mut self) {
        self.0.image_selecting.store(false, Ordering::Release);
    }
}

pub async fn select(
    window: &tauri::WebviewWindow,
    state: Arc<Runtime>,
) -> Result<serde_json::Value, String> {
    use tauri_plugin_dialog::DialogExt;
    if state.capture.active_label().is_some() {
        return Err("请先完成或取消截图".into());
    }
    if state.image_selecting.swap(true, Ordering::AcqRel) {
        return Err("已有图片选择正在进行".into());
    }
    let _selection = SelectionGuard(state.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(window)
        .set_title("选择要发送的图片")
        .add_filter("图片", &["png", "jpg", "jpeg", "bmp"])
        .pick_file(move |selected| {
            let _ = tx.send(selected);
        });
    let Some(selected) = rx.await.map_err(|_| "图片选择已中断")? else {
        return Ok(serde_json::json!({"success":true,"cancelled":true}));
    };
    if !state.accepting.load(Ordering::Acquire) {
        return Err("程序正在退出".into());
    }
    let path = selected.into_path().map_err(|_| "仅支持本地图片文件")?;
    let image = state.network.import_image_path(path).await?;
    Ok(serde_json::json!({"success":true,"image":image}))
}

pub fn asset_url(id: &str, thumbnail: bool) -> String {
    let part = if thumbnail { "thumb" } else { "full" };
    if cfg!(windows) {
        format!("http://ipmsg-image.localhost/{id}/{part}")
    } else {
        format!("ipmsg-image://localhost/{id}/{part}")
    }
}
fn response(status: StatusCode, bytes: Vec<u8>, png: bool) -> Response<Vec<u8>> {
    let mut result = Response::new(bytes);
    *result.status_mut() = status;
    result.headers_mut().insert(
        "Content-Type",
        if png {
            "image/png"
        } else {
            "text/plain; charset=utf-8"
        }
        .parse()
        .unwrap(),
    );
    result
        .headers_mut()
        .insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    result.headers_mut().insert(
        "Cache-Control",
        if png {
            "private, max-age=3600"
        } else {
            "no-store"
        }
        .parse()
        .unwrap(),
    );
    result
}

// Stream PNG bytes through a dedicated application protocol, not a huge base64
// IPC response. Neither URLs nor commands accept an arbitrary filesystem path.
pub fn serve(
    context: UriSchemeContext<'_, tauri::Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let allowed_window = context.webview_label() == "main";
    let origin_ok = context
        .app_handle()
        .get_webview_window("main")
        .and_then(|w| w.url().ok())
        .is_some_and(|url| {
            (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost"))
                || (cfg!(debug_assertions)
                    && url.scheme() == "http"
                    && url.host_str() == Some("127.0.0.1")
                    && url.port() == Some(1420))
        });
    if !allowed_window || !origin_ok {
        responder.respond(response(
            StatusCode::FORBIDDEN,
            b"Forbidden".to_vec(),
            false,
        ));
        return;
    }
    if request.method() != Method::GET {
        responder.respond(response(
            StatusCode::METHOD_NOT_ALLOWED,
            b"GET required".to_vec(),
            false,
        ));
        return;
    }
    let parts: Vec<_> = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect();
    if parts.len() != 2 || !valid_asset_id(parts[0]) || !matches!(parts[1], "thumb" | "full") {
        responder.respond(response(
            StatusCode::NOT_FOUND,
            b"Image not found".to_vec(),
            false,
        ));
        return;
    }
    let Some(state) = context
        .app_handle()
        .try_state::<Arc<Runtime>>()
        .map(|s| s.inner().clone())
    else {
        responder.respond(response(StatusCode::SERVICE_UNAVAILABLE, vec![], false));
        return;
    };
    let id = parts[0].to_owned();
    let thumbnail = parts[1] == "thumb";
    tauri::async_runtime::spawn(async move {
        if !state.accepting.load(Ordering::Acquire) {
            responder.respond(response(StatusCode::SERVICE_UNAVAILABLE, vec![], false));
            return;
        }
        match state.network.read_image(&id, thumbnail).await {
            Ok(bytes) => responder.respond(response(StatusCode::OK, bytes, true)),
            Err(error) => {
                state.log(
                    "DEBUG",
                    &format!("Image asset unavailable id={id}: {error}"),
                );
                responder.respond(response(
                    StatusCode::NOT_FOUND,
                    b"Image unavailable".to_vec(),
                    false,
                ));
            }
        }
    });
}
