use crate::runtime::Runtime;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
};
use tauri::{Emitter, Manager, WebviewWindow};
pub async fn select(window: &WebviewWindow, state: Arc<Runtime>) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    if state.image_selecting.swap(true, Ordering::AcqRel) {
        return Err("请先完成已有选择或截图".into());
    }
    let _guard = crate::image::SelectionGuard(state.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(window)
        .set_title("选择要发送的文件")
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let Some(path) = rx.await.map_err(|_| "文件选择中断")? else {
        return Ok(json!({"success":true,"cancelled":true}));
    };
    if !state.accepting.load(Ordering::Acquire) {
        return Err("程序正在退出".into());
    }
    let selection = state
        .network
        .file_transfers()?
        .select_path(path.into_path().map_err(|_| "仅支持本地文件")?)
        .await?;
    Ok(json!({"success":true,"files":[selection]}))
}
pub fn dropped(app: tauri::AppHandle, paths: Vec<PathBuf>) {
    let state = app.state::<Arc<Runtime>>().inner().clone();
    if !state.accepting.load(Ordering::Acquire) {
        return;
    }
    // Capture the conversation at the native drop, not after slow disk metadata IO.
    let target = state
        .active_conversation
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    tauri::async_runtime::spawn(async move {
        let result = async {
            if target.is_empty() {
                return Err("请先选择一个聊天对象".to_string());
            }
            if paths.is_empty() || paths.len() > 8 {
                return Err("一次最多拖入8个普通文件".into());
            }
            if state.image_selecting.swap(true, Ordering::AcqRel) {
                return Err("请先完成已有选择或截图".into());
            }
            let _guard = crate::image::SelectionGuard(state.clone());
            let manager = state.network.file_transfers()?;
            let mut selections = Vec::new();
            for path in paths {
                match manager.select_path(path).await {
                    Ok(selection) => selections.push(selection),
                    Err(error) => {
                        for selection in &selections {
                            manager.discard(&selection.selection_id);
                        }
                        return Err(error);
                    }
                }
            }
            Ok(selections)
        }
        .await;
        let payload = match result {
            Ok(files) => json!({"target":target,"files":files}),
            Err(error) => json!({"target":target,"error":error}),
        };
        if let Err(error) = app.emit_to(
            "main",
            "ipmsg-event",
            json!({"event":"file.selected","payload":payload}),
        ) {
            state.log("ERROR", &error.to_string());
        }
    });
}
pub async fn open_folder(state: Arc<Runtime>, id: String) -> Result<Value, String> {
    let path = state.database.file_path(id).await?;
    tokio::task::spawn_blocking(move || {
        if !path.is_file() {
            return Err("本地文件已移动或删除".to_string());
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("explorer.exe")
                .arg("/select,")
                .arg(&path)
                .creation_flags(0x08000000)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(not(windows))]
        {
            return Err("当前平台暂不支持打开文件夹".into());
        }
        #[allow(unreachable_code)]
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({"success":true}))
}
