use crate::runtime::Runtime;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};
use tauri::State;

fn string(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("缺少字符串参数：{key}"))
}
fn count(args: &Value, key: &str, default: u32) -> Result<u32, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("无效分页参数：{key}")),
    }
}

#[tauri::command]
pub async fn ipmsg_command(
    state: State<'_, Arc<Runtime>>,
    window: tauri::WebviewWindow,
    command: String,
    args: Value,
) -> Result<Value, String> {
    // Tauri requires Result for async commands with borrowed State inputs.
    // Keep domain errors in the JSON envelope expected by the frontend bridge.
    Ok(
        match dispatch(state.inner().clone(), window, &command, args).await {
            Ok(value) => value,
            Err(error) => json!({"success":false,"error":error}),
        },
    )
}
async fn dispatch(
    state: Arc<Runtime>,
    window: tauri::WebviewWindow,
    command: &str,
    args: Value,
) -> Result<Value, String> {
    if window.label() != "main" || !crate::capture::local_window(&window) {
        return Err("此窗口不允许调用该命令".into());
    }
    if !state.accepting.load(Ordering::Acquire) {
        return Err("程序正在退出".into());
    }
    match command {
        "app.info" => Ok(state.info()),
        "notification.test_sound" => {
            let sound = state
                .sound
                .as_ref()
                .ok_or("提示音服务不可用，需要Windows桌面版")?;
            let duration = sound.preview().await?;
            Ok(json!({"success":true,"durationMs":duration}))
        }
        "network.scan_range" => {
            let options: ipmsg_core::scan::ScanOptions =
                serde_json::from_value(args).map_err(|e| format!("扫描参数无效：{e}"))?;
            Ok(json!({"success":true,"scan":state.network.start_scan(options).await?}))
        }
        "network.scan_cancel" => {
            let id = args
                .get("scanId")
                .and_then(Value::as_u64)
                .ok_or("缺少有效扫描任务编号")?;
            Ok(json!({"success":true,"scan":state.network.cancel_scan(id).await?}))
        }
        "network.scan_status" => Ok(json!({"success":true,"scan":state.network.scan_status()})),
        "screenshot.start" => crate::capture::start(&window, state.clone()).await,
        "file.select" => crate::files::select(&window, state.clone()).await,
        "file.send" => Ok(
            json!({"success":true,"message":state.network.send_file(&string(&args,"target")?,&string(&args,"selectionId")?).await?}),
        ),
        "file.discard" => {
            state
                .network
                .file_transfers()?
                .discard(&string(&args, "selectionId")?);
            Ok(json!({"success":true}))
        }
        "file.accept" => {
            state
                .network
                .file_transfers()?
                .accept(&string(&args, "messageId")?)
                .await?;
            Ok(json!({"success":true}))
        }
        "file.reject" | "file.cancel" => Ok(
            json!({"success":true,"cancelled":state.network.file_transfers()?.cancel(&string(&args,"messageId")?,command=="file.reject").await?}),
        ),
        "file.open_folder" => {
            crate::files::open_folder(state.clone(), string(&args, "messageId")?).await
        }
        "image.read" => {
            let id = string(&args, "assetId")?;
            let thumbnail = match args.get("thumbnail") {
                None => true,
                Some(value) => value.as_bool().ok_or("thumbnail必须是布尔值")?,
            };
            state.network.image_asset(&id).await?;
            Ok(json!({"success":true,"url":crate::image::asset_url(&id,thumbnail)}))
        }
        "image.select" => crate::image::select(&window, state.clone()).await,
        "image.send" => {
            let sent = state
                .network
                .send_image(&string(&args, "target")?, &string(&args, "assetId")?)
                .await?;
            Ok(
                json!({"success":true,"messageId":sent.message_id,"imageId":sent.image_id,"image":sent.image}),
            )
        }
        "image.cancel" => Ok(
            json!({"success":true,"cancelled":state.network.cancel_image(&string(&args,"messageId")?)}),
        ),
        "image.discard" => {
            let discarded = state
                .network
                .discard_image(&string(&args, "assetId")?)
                .await?;
            Ok(json!({"success":true,"discarded":discarded}))
        }
        "config.get" => Ok(json!({"success":true,"config":state.config.get()})),
        "config.set" => {
            let config = state.config.save(args).await?;
            if let Some(sound) = &state.sound {
                sound.set_enabled(config.notification_sound);
            }
            state
                .network
                .apply_config(&config)
                .await
                .map_err(|e| format!("设置已保存，但网络公告失败：{e}"))?;
            Ok(json!({"success":true,"config":config}))
        }
        "config.loaded" => {
            state.network.ui_ready().await?;
            Ok(json!({"success":true}))
        }
        "user.local" => {
            let mut user =
                serde_json::to_value(state.network.local()).map_err(|e| e.to_string())?;
            user["success"] = json!(true);
            Ok(user)
        }
        "user.list" => {
            let users = state.network.users();
            Ok(json!({"success":true,"count":users.len(),"users":users}))
        }
        "user.discover" => {
            state.network.discover().await?;
            Ok(json!({"success":true}))
        }
        "message.send" => {
            let target = string(&args, "target")?;
            let content = string(&args, "content")?;
            let id = state.network.send_message(&target, &content).await?;
            Ok(json!({"success":true,"messageId":id}))
        }
        "history.get" => {
            let local = state.network.local().id;
            let messages = state
                .database
                .history(
                    string(&args, "userId")?,
                    local.clone(),
                    count(&args, "limit", 50)?,
                    count(&args, "offset", 0)?,
                )
                .await?;
            Ok(json!({"success":true,"messages":messages,"localUserId":local}))
        }
        "history.get_recent" => {
            let local = state.network.local().id;
            let messages = state
                .database
                .recent(local.clone(), count(&args, "limit", 100)?)
                .await?;
            Ok(json!({"success":true,"messages":messages,"localUserId":local}))
        }
        "history.search" => {
            let keyword = string(&args, "keyword")?;
            if keyword.len() > 1024 {
                return Err("搜索关键字过长".into());
            }
            let user = match args.get("userId") {
                None | Some(Value::Null) => None,
                Some(_) => Some(string(&args, "userId")?),
            };
            let messages = state
                .database
                .search(keyword, user, state.network.local().id)
                .await?;
            Ok(json!({"success":true,"messages":messages,"localUserId":state.network.local().id}))
        }
        "history.clear" => {
            let user = match args.get("userId") {
                None | Some(Value::Null) => None,
                Some(_) => Some(string(&args, "userId")?),
            };
            let deleted = state.database.clear(user, state.network.local().id).await?;
            if let Ok(files) = state.network.file_transfers() {
                files.cancel_cleared(&deleted).await;
            }
            Ok(json!({"success":true,"deletedIds":deleted}))
        }
        "window.set_active_conversation" => {
            *state
                .active_conversation
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = string(&args, "userId")?;
            Ok(json!({"success":true}))
        }
        "frontend.error" => {
            state.log("ERROR", &string(&args, "message")?);
            Ok(json!({"success":true}))
        }
        _ => Err(format!("Rust核心版暂不支持命令：{command}")),
    }
}
