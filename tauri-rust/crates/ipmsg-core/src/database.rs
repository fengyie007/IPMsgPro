use crate::image::{
    assets::{valid_asset_id, PendingAsset},
    fragments::{Assembler, TransferKey},
    ImageMetadata,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub content: String,
    #[serde(rename = "type")]
    pub kind: i64,
    pub timestamp: i64,
    pub status: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<crate::file::FileMetadata>,
}
#[derive(Debug)]
pub struct ImageCommit {
    pub inserted: bool,
    pub ack_index: Option<u32>,
}

type Work = Box<dyn FnOnce(&mut Connection) + Send>;
#[derive(Clone)]
pub struct Database {
    sender: mpsc::Sender<Option<Work>>,
    worker: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    images_dir: Arc<PathBuf>,
}
fn row(record: &Row<'_>) -> rusqlite::Result<Record> {
    Ok(Record {
        id: record.get(0)?,
        from_id: record.get(1)?,
        to_id: record.get(2)?,
        content: record.get(3)?,
        kind: record.get(4)?,
        timestamp: record.get(5)?,
        status: record.get(6)?,
        file: None,
        image: None,
    })
}
fn image_row(row: &Row<'_>) -> rusqlite::Result<ImageMetadata> {
    Ok(ImageMetadata {
        asset_id: row.get(0)?,
        file_name: row.get(1)?,
        file_size: row.get(2)?,
        mime: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
    })
}
fn hydrate_images(db: &Connection, records: &mut [Record]) -> Result<(), String> {
    for record in records.iter_mut().filter(|r| r.kind == 2) {
        let encoded: Option<String> = db
            .query_row(
                "SELECT metadata FROM message_files WHERE message_id=?",
                [&record.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        record.file = encoded
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|e| e.to_string())?;
    }
    let mut query = db
        .prepare(
            "SELECT a.asset_id,a.file_name,a.file_size,a.mime,a.width,a.height
        FROM message_images m JOIN image_assets a ON a.asset_id=m.asset_id WHERE m.message_id=?",
        )
        .map_err(|e| e.to_string())?;
    for record in records.iter_mut().filter(|r| r.kind == 1) {
        record.image = query
            .query_row([&record.id], image_row)
            .optional()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn validate_image_record(record: &Record) -> Result<&ImageMetadata, String> {
    let metadata = record.image.as_ref().ok_or("图片消息缺少元数据")?;
    if record.kind != 1 || !valid_asset_id(&metadata.asset_id) {
        return Err("无效图片消息".into());
    }
    Ok(metadata)
}

// The returned instant is the final validation point before SQLite commits.
// The caller can use it to advance the transfer under the same state lock even
// if the filesystem flush inside commit crosses the wall-clock deadline.
fn insert_image_transaction(
    db: &mut Connection,
    record: &Record,
    metadata: &ImageMetadata,
    deadline: Instant,
    before_commit: impl FnOnce(Instant) -> Result<(), String>,
) -> Result<(bool, Instant), String> {
    if Instant::now() >= deadline {
        return Err("图片处理超时".into());
    }
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let fresh = tx.execute(
        "INSERT OR IGNORE INTO seen_messages (id,seen_at) VALUES (?,CAST(strftime('%s','now') AS INTEGER))",
        [&record.id],
    ).map_err(|e| e.to_string())?;
    if fresh > 0 {
        tx.execute(
            "INSERT INTO image_assets (asset_id,file_name,file_size,mime,width,height) VALUES (?,?,?,?,?,?)",
            params![metadata.asset_id, metadata.file_name, metadata.file_size, metadata.mime, metadata.width, metadata.height],
        ).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO messages (id,from_id,to_id,content,type,timestamp,status) VALUES (?,?,?,?,?,?,?)",
            params![record.id, record.from_id, record.to_id, record.content, 1, record.timestamp, record.status],
        ).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO message_images (message_id,asset_id) VALUES (?,?)",
            params![record.id, metadata.asset_id],
        )
        .map_err(|e| e.to_string())?;
    }
    let checked_at = Instant::now();
    if checked_at >= deadline {
        return Err("图片处理超时".into());
    }
    before_commit(checked_at)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((fresh > 0, checked_at))
}

const COLUMNS: &str = "id,from_id,to_id,content,type,timestamp,status";
impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = path.to_owned();
        let images_dir = Arc::new(path.parent().unwrap_or(Path::new(".")).join("images"));
        let (sender, mut receiver) = mpsc::channel::<Option<Work>>(128);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("rust-message-db".into())
            .spawn(move || {
                let init = (|| -> rusqlite::Result<Connection> {
                    let mut db = Connection::open(path)?;
                    db.busy_timeout(std::time::Duration::from_secs(1))?;
                    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
                    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
                    if version > 5 { return Err(rusqlite::Error::InvalidQuery); }
                    let transaction = db.transaction()?;
                    transaction.execute_batch(
                        "CREATE TABLE IF NOT EXISTS messages (
                      id TEXT PRIMARY KEY, from_id TEXT NOT NULL, to_id TEXT NOT NULL,
                      content TEXT NOT NULL, type INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                      status INTEGER NOT NULL);
                    CREATE INDEX IF NOT EXISTS messages_pair ON messages(from_id,to_id,timestamp);
                    CREATE TABLE IF NOT EXISTS seen_messages (id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
                    CREATE INDEX IF NOT EXISTS seen_messages_time ON seen_messages(seen_at);
                    INSERT OR IGNORE INTO seen_messages SELECT id, CAST(strftime('%s','now') AS INTEGER) FROM messages;
                    UPDATE messages SET status=3 WHERE status=0;
                    CREATE TABLE IF NOT EXISTS image_assets (
                      asset_id TEXT PRIMARY KEY, file_name TEXT NOT NULL, file_size INTEGER NOT NULL,
                      mime TEXT NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL);
                    CREATE TABLE IF NOT EXISTS message_images (
                      message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
                      asset_id TEXT NOT NULL REFERENCES image_assets(asset_id));",
                    )?;
                    if version < 3 {
                        transaction.execute_batch("ALTER TABLE image_assets ADD COLUMN temporary INTEGER NOT NULL DEFAULT 0;
                            ALTER TABLE image_assets ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;")?;
                    }
                    transaction.execute_batch("CREATE TABLE IF NOT EXISTS message_files (
                        message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
                        metadata TEXT NOT NULL, local_path TEXT);
                        CREATE TABLE IF NOT EXISTS file_receives(message_id TEXT PRIMARY KEY, checkpoint TEXT NOT NULL);
                        UPDATE message_files SET metadata=json_set(metadata,'$.state',CASE WHEN json_extract(metadata,'$.canResume')=1 THEN 'paused' ELSE 'failed' END,'$.attempt',COALESCE(json_extract(metadata,'$.attempt'),0)+1,'$.error','程序重启，传输已中断')
                        WHERE json_extract(metadata,'$.state') IN ('offered','transferring','finalizing');")?;
                    transaction.pragma_update(None, "user_version", 5)?;
                    transaction.commit()?;
                    Ok(db)
                })();
                match init {
                    Ok(mut db) => {
                        let _ = ready_tx.send(Ok(()));
                        while let Some(Some(work)) = receiver.blocking_recv() {
                            work(&mut db);
                        }
                        let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv().map_err(|e| e.to_string())? {
            Ok(()) => Ok(Self {
                sender,
                worker: Arc::new(Mutex::new(Some(worker))),
                images_dir,
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(Some(Box::new(move |db| {
                let _ = tx.send(f(db));
            })))
            .await
            .map_err(|_| "数据库已关闭")?;
        rx.await.map_err(|_| "数据库线程已退出".to_string())?
    }
    pub fn images_dir(&self) -> &Path {
        self.images_dir.as_path()
    }

    pub async fn image_asset(&self, asset_id: String) -> Result<Option<ImageMetadata>, String> {
        self.call(move |db| db.query_row("SELECT asset_id,file_name,file_size,mime,width,height FROM image_assets WHERE asset_id=? AND temporary<>2",
            [asset_id], image_row).optional().map_err(|e| e.to_string())).await
    }

    /// Import only assets created by the native picker. Ownership stays in this
    /// queued operation even if the command caller disappears during the commit.
    pub async fn register_import(&self, pending: PendingAsset) -> Result<ImageMetadata, String> {
        if !pending.metadata_matches_directory() {
            return Err("无效导入资产".into());
        }
        self.call(move |db| {
            let mut pending = pending;
            let metadata = pending.metadata.clone();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM image_assets WHERE temporary<>0", [], |row| row.get(0)).map_err(|e| e.to_string())?;
            if count >= 8 { return Err("待发送预览过多，请先取消已有预览".into()); }
            tx.execute("INSERT INTO image_assets(asset_id,file_name,file_size,mime,width,height,temporary,created_at) VALUES (?,?,?,?,?,?,1,CAST(strftime('%s','now') AS INTEGER))",
                params![metadata.asset_id,metadata.file_name,metadata.file_size,metadata.mime,metadata.width,metadata.height]).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            pending.keep();
            Ok(metadata)
        }).await
    }

    pub async fn insert_sent_image(&self, record: Record) -> Result<(), String> {
        let metadata = validate_image_record(&record)?.clone();
        self.call(move |db| {
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let stored = tx.query_row("SELECT asset_id,file_name,file_size,mime,width,height FROM image_assets WHERE asset_id=? AND temporary<>2", [&metadata.asset_id], image_row)
                .optional().map_err(|e| e.to_string())?.ok_or("图片预览已过期，请重新选择")?;
            if stored != metadata { return Err("图片元数据不匹配".into()); }
            tx.execute("INSERT INTO messages(id,from_id,to_id,content,type,timestamp,status) VALUES (?,?,?,?,1,?,0)",
                params![record.id,record.from_id,record.to_id,record.content,record.timestamp]).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO seen_messages(id,seen_at) VALUES (?,CAST(strftime('%s','now') AS INTEGER))", [&record.id]).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO message_images(message_id,asset_id) VALUES (?,?)", params![record.id,metadata.asset_id]).map_err(|e| e.to_string())?;
            tx.execute("UPDATE image_assets SET temporary=0 WHERE asset_id=?", [&metadata.asset_id]).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(())
        }).await
    }

    /// Only preview imports are removable. A sent/received history asset is never
    /// deleted here, even after its message has been cleared.
    pub async fn discard_imports(
        &self,
        id: Option<String>,
        before: i64,
    ) -> Result<Vec<String>, String> {
        self.call(move |db| {
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let ids = {
                let mut statement = tx.prepare("SELECT asset_id FROM image_assets WHERE (temporary=2 OR (temporary=1 AND created_at<=?1)) AND (?2 IS NULL OR asset_id=?2)
                    AND NOT EXISTS(SELECT 1 FROM message_images WHERE message_images.asset_id=image_assets.asset_id)").map_err(|e| e.to_string())?;
                let ids = statement.query_map(params![before,id], |row|row.get::<_,String>(0)).map_err(|e|e.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
                ids
            };
            for id in &ids { tx.execute("UPDATE image_assets SET temporary=2 WHERE asset_id=?", [id]).map_err(|e|e.to_string())?; }
            tx.commit().map_err(|e|e.to_string())?;
            Ok(ids)
        }).await
    }

    pub async fn forget_discarded_asset(&self, id: String) -> Result<(), String> {
        self.call(move |db| {
            db.execute(
                "DELETE FROM image_assets WHERE asset_id=? AND temporary=2",
                [id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
        .await
    }

    pub async fn interrupt_sending(&self, id: String) -> Result<(), String> {
        self.call(move |db| {
            db.execute("UPDATE messages SET status=3 WHERE id=? AND status=0", [id])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
    }

    /// Metadata-only insertion retained for tests/importers. If the caller owns
    /// a PendingAsset, it must keep it only when this returns true.
    pub async fn insert_image(&self, record: Record, deadline: Instant) -> Result<bool, String> {
        let metadata = validate_image_record(&record)?.clone();
        self.call(move |db| {
            insert_image_transaction(db, &record, &metadata, deadline, |_| Ok(()))
                .map(|(inserted, _)| inserted)
        })
        .await
    }

    /// Own the pending files inside the queued database operation. Canceling the
    /// caller after enqueue cannot unlink an asset that this transaction commits.
    pub async fn commit_received_image(
        &self,
        record: Record,
        deadline: Instant,
        pending: PendingAsset,
        images: Arc<Mutex<Assembler>>,
        key: TransferKey,
    ) -> Result<ImageCommit, String> {
        let metadata = validate_image_record(&record)?.clone();
        if !pending.metadata_matches_directory()
            || metadata != pending.metadata
            || record.from_id != key.peer_id
            || record.id != format!("image-rx:{}:{}", key.peer_id, key.image_id)
        {
            return Err("图片提交与接收任务不匹配".into());
        }
        self.call(move |db| {
            // Declare the asset before the lock so all error paths release the
            // state lock before PendingAsset's filesystem cleanup runs.
            let mut pending = pending;
            let mut images = images.lock().map_err(|_| "图片接收状态不可用")?;
            let now = Instant::now();
            if now >= deadline || !images.is_finalizing(&key, now) {
                return Err("图片接收已拒绝或超时".into());
            }
            let (inserted, checked_at) =
                insert_image_transaction(db, &record, &metadata, deadline, |at| {
                    if images.is_finalizing(&key, at) {
                        Ok(())
                    } else {
                        Err("图片接收已拒绝或超时".into())
                    }
                })?;
            if inserted {
                pending.keep();
            }
            let ack_index = images.finish(&key, true, checked_at);
            Ok(ImageCommit {
                inserted,
                ack_index,
            })
        })
        .await
    }

    pub async fn insert(&self, record: Record) -> Result<bool, String> {
        if record.image.is_some() {
            return Err("图片必须通过完整图片事务保存".into());
        }
        self.call(move |db| {
            let transaction = db.transaction().map_err(|e| e.to_string())?;
            // Keep only IDs (not content) for seven days so retries cannot resurrect
            // a just-cleared message, including across a process restart.
            transaction.execute("DELETE FROM seen_messages WHERE seen_at < CAST(strftime('%s','now') AS INTEGER)-604800", []).map_err(|e| e.to_string())?;
            let fresh = transaction.execute("INSERT OR IGNORE INTO seen_messages (id,seen_at) VALUES (?,CAST(strftime('%s','now') AS INTEGER))", [&record.id]).map_err(|e| e.to_string())?;
            let inserted = if fresh > 0 {
                transaction.execute(
                    "INSERT OR IGNORE INTO messages (id,from_id,to_id,content,type,timestamp,status) VALUES (?,?,?,?,?,?,?)",
                    params![record.id,record.from_id,record.to_id,record.content,record.kind,record.timestamp,record.status])
                    .map_err(|e| e.to_string())? > 0
            } else { false };
            if inserted {
                if let Some(file) = &record.file {
                    transaction.execute("INSERT INTO message_files(message_id,metadata) VALUES (?,?)",
                        params![record.id, serde_json::to_string(file).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
                }
            }
            transaction.commit().map_err(|e| e.to_string())?;
            Ok(inserted)
        })
        .await
    }
    pub async fn status(&self, id: String, status: i64) -> Result<(), String> {
        self.call(move |db| {
            db.execute(
                "UPDATE messages SET status=? WHERE id=?",
                params![status, id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn file_state(
        &self,
        id: String,
        metadata: crate::file::FileMetadata,
        path: Option<PathBuf>,
    ) -> Result<(), String> {
        self.call(move |db| {
            let tx = db.transaction().map_err(|e| e.to_string())?;
            tx.execute("UPDATE messages SET status=? WHERE id=?", params![metadata.status(), id]).map_err(|e| e.to_string())?;
            tx.execute("UPDATE message_files SET metadata=?,local_path=COALESCE(?,local_path) WHERE message_id=?",
                params![serde_json::to_string(&metadata).map_err(|e| e.to_string())?,path.map(|p|p.to_string_lossy().into_owned()),id]).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())
        }).await
    }
    pub async fn file_path(&self, id: String) -> Result<PathBuf, String> {
        self.call(move |db| {
            let path: Option<String> = db.query_row("SELECT local_path FROM message_files WHERE message_id=? AND json_extract(metadata,'$.state')='completed'", [id], |row| row.get(0))
                .optional().map_err(|e| e.to_string())?.flatten();
            path.map(PathBuf::from).ok_or_else(|| "文件尚未接收完成或历史已清空".into())
        }).await
    }
    pub async fn receive_checkpoint(
        &self,
        id: String,
    ) -> Result<Option<serde_json::Value>, String> {
        self.call(move |db| {
            let text: Option<String> = db
                .query_row(
                    "SELECT checkpoint FROM file_receives WHERE message_id=?",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            text.map(|s| serde_json::from_str(&s))
                .transpose()
                .map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn save_receive(&self, id: String, data: serde_json::Value) -> Result<(), String> {
        self.call(move |db| {
            let count: i64 = db
                .query_row("SELECT COUNT(*) FROM file_receives", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if count >= 32
                && !db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM file_receives WHERE message_id=?)",
                        [&id],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(|e| e.to_string())?
            {
                return Err("续传任务已达32项，请取消不需要的任务".into());
            }
            db.execute(
                "INSERT OR REPLACE INTO file_receives VALUES (?,?)",
                params![id, data.to_string()],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn remove_receive(&self, id: String) -> Result<(), String> {
        self.call(move |db| {
            db.execute("DELETE FROM file_receives WHERE message_id=?", [id])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn receive_ids(&self) -> Result<Vec<String>, String> {
        self.call(|db| {
            let mut s = db
                .prepare("SELECT message_id FROM file_receives")
                .map_err(|e| e.to_string())?;
            let rows = s
                .query_map([], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<String>>>()
                .map_err(|e| e.to_string())?;
            Ok(rows)
        })
        .await
    }
    pub async fn file_metadata(
        &self,
        id: String,
    ) -> Result<Option<crate::file::FileMetadata>, String> {
        self.call(move |db| {
            let text: Option<String> = db
                .query_row(
                    "SELECT metadata FROM message_files WHERE message_id=?",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            text.map(|s| serde_json::from_str(&s))
                .transpose()
                .map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn history(
        &self,
        user: String,
        local: String,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Record>, String> {
        self.call(move |db| {
            let sql = format!("SELECT {COLUMNS} FROM messages WHERE (from_id=? AND to_id=?) OR (from_id=? AND to_id=?) ORDER BY timestamp DESC,rowid DESC LIMIT ? OFFSET ?");
            let mut statement = db.prepare(&sql).map_err(|e| e.to_string())?;
            let mut records = statement.query_map(params![local, user, user, local, limit.min(200), offset], row)
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
            records.reverse();
            hydrate_images(db, &mut records)?;
            Ok(records)
        }).await
    }
    pub async fn recent(&self, local: String, limit: u32) -> Result<Vec<Record>, String> {
        self.call(move |db| {
            let sql = format!("SELECT {COLUMNS} FROM messages WHERE rowid IN
                (SELECT MAX(rowid) FROM messages WHERE from_id=? OR to_id=? GROUP BY CASE WHEN from_id=? THEN to_id ELSE from_id END)
                ORDER BY timestamp DESC,rowid DESC LIMIT ?");
            let mut statement = db.prepare(&sql).map_err(|e| e.to_string())?;
            let mut rows = statement.query_map(params![local, local, local, limit.min(200)], row)
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
            hydrate_images(db, &mut rows)?;
            Ok(rows)
        }).await
    }
    pub async fn search(
        &self,
        keyword: String,
        user: Option<String>,
        local: String,
    ) -> Result<Vec<Record>, String> {
        self.call(move |db| {
            let pattern = format!(
                "%{}%",
                keyword
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            let mut statement = db
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM messages WHERE content LIKE ?1 ESCAPE '\\'
                AND (?2 IS NULL OR (from_id=?3 AND to_id=?2) OR (from_id=?2 AND to_id=?3))
                ORDER BY timestamp DESC,rowid DESC LIMIT 200"
                ))
                .map_err(|e| e.to_string())?;
            let mut rows = statement
                .query_map(params![pattern, user, local], row)
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            hydrate_images(db, &mut rows)?;
            Ok(rows)
        })
        .await
    }
    pub async fn clear(&self, user: Option<String>, local: String) -> Result<Vec<String>, String> {
        self.call(move |db| {
            let transaction = db.transaction().map_err(|e| e.to_string())?;
            let user = user.filter(|s| !s.is_empty());
            let condition =
                "(?1 IS NULL OR (from_id=?2 AND to_id=?1) OR (from_id=?1 AND to_id=?2))";
            let deleted = {
                let mut statement = transaction
                    .prepare(&format!(
                        "SELECT id FROM messages WHERE {condition} ORDER BY rowid"
                    ))
                    .map_err(|e| e.to_string())?;
                let rows = statement
                    .query_map(params![user, local], |row| row.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?;
                rows
            };
            transaction
                .execute(
                    &format!("DELETE FROM messages WHERE {condition}"),
                    params![user, local],
                )
                .map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
            Ok(deleted)
        })
        .await
    }
    pub async fn shutdown(&self) {
        let handle = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            let _ = self.sender.send(None).await;
            let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dedupe_paging_and_clear() {
        let file = std::env::temp_dir().join(format!(
            "speedipmsg-rust-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Database::open(&file).unwrap();
        for n in 0..5 {
            let record = Record {
                id: n.to_string(),
                from_id: "me".into(),
                to_id: "peer".into(),
                content: format!("行{n}%"),
                kind: 0,
                timestamp: n,
                status: 0,
                file: None,
                image: None,
            };
            assert!(db.insert(record.clone()).await.unwrap());
            assert!(!db.insert(record).await.unwrap());
        }
        assert_eq!(
            db.history("peer".into(), "me".into(), 2, 1)
                .await
                .unwrap()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "3"]
        );
        assert_eq!(db.recent("me".into(), 10).await.unwrap().len(), 1);
        assert_eq!(
            db.search("%".into(), None, "me".into())
                .await
                .unwrap()
                .len(),
            5
        );
        db.shutdown().await;
        let db = Database::open(&file).unwrap();
        assert!(db
            .history("peer".into(), "me".into(), 10, 0)
            .await
            .unwrap()
            .iter()
            .all(|m| m.status == 3));
        db.clear(Some("peer".into()), "me".into()).await.unwrap();
        assert!(db.recent("me".into(), 10).await.unwrap().is_empty());
        db.shutdown().await;
        std::fs::remove_file(file).unwrap();
    }
    #[tokio::test]
    async fn scoped_search_and_clear_share_exact_database_boundaries() {
        let path = std::env::temp_dir().join(format!(
            "rust-history-boundary-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Database::open(&path).unwrap();
        let wanted = Record {
            id: "wanted".into(),
            from_id: "me".into(),
            to_id: "peer".into(),
            content: "needle".into(),
            kind: 0,
            timestamp: 1,
            status: 1,
            file: None,
            image: None,
        };
        db.insert(wanted.clone()).await.unwrap();
        for n in 0..205 {
            db.insert(Record {
                id: format!("other{n}"),
                to_id: "other".into(),
                timestamp: 10 + n,
                ..wanted.clone()
            })
            .await
            .unwrap();
        }
        let matches = db
            .search("needle".into(), Some("peer".into()), "me".into())
            .await
            .unwrap();
        assert_eq!(
            matches.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["wanted"]
        );
        let between = Record {
            id: "before-delete-exec".into(),
            timestamp: 999,
            ..wanted.clone()
        };
        db.insert(between.clone()).await.unwrap();
        let deleted = db.clear(Some("peer".into()), "me".into()).await.unwrap();
        assert_eq!(deleted, ["wanted", "before-delete-exec"]);
        db.insert(Record {
            id: "after-delete".into(),
            timestamp: 1000,
            ..wanted.clone()
        })
        .await
        .unwrap();
        assert!(!db.insert(between).await.unwrap());
        assert_eq!(
            db.history("peer".into(), "me".into(), 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        db.shutdown().await;
        let db = Database::open(&path).unwrap();
        assert!(!db.insert(wanted).await.unwrap());
        db.shutdown().await;
        std::fs::remove_file(path).unwrap();
    }

    fn image_fixture() -> (
        PathBuf,
        Database,
        crate::image::assets::AssetStore,
        TransferKey,
    ) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rust-image-commit-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = Database::open(&root.join("messages.db")).unwrap();
        let store = crate::image::assets::AssetStore::new(db.images_dir()).unwrap();
        let key = TransferKey {
            peer_id: "peer@fixture#127.0.0.1:2425".into(),
            image_id: "abcdef12".into(),
        };
        (root, db, store, key)
    }

    fn pending_record(
        store: &crate::image::assets::AssetStore,
        key: &TransferKey,
    ) -> (PendingAsset, Record) {
        let pending = store
            .persist(
                crate::image::dib::DecodedImage {
                    png: b"\x89PNG\r\n\x1a\nfull".to_vec(),
                    thumbnail_png: b"\x89PNG\r\n\x1a\nthumb".to_vec(),
                    width: 1,
                    height: 1,
                },
                &key.image_id,
            )
            .unwrap();
        let record = Record {
            id: format!("image-rx:{}:{}", key.peer_id, key.image_id),
            from_id: key.peer_id.clone(),
            to_id: "local@fixture".into(),
            content: "[图片]".into(),
            kind: 1,
            timestamp: 1,
            status: 1,
            file: None,
            image: Some(pending.metadata.clone()),
        };
        (pending, record)
    }

    fn finalizing(key: &TransferKey) -> Arc<Mutex<Assembler>> {
        use crate::image::fragments::{Action, Fragment};
        let mut images = Assembler::new();
        assert!(matches!(
            images
                .accept(
                    &key.peer_id,
                    Fragment {
                        image_id: key.image_id.clone(),
                        total: 1,
                        count: 1,
                        index: 1,
                        data: vec![1],
                    },
                    Instant::now()
                )
                .unwrap(),
            Action::Finalize { .. }
        ));
        Arc::new(Mutex::new(images))
    }

    async fn block_database(
        db: &Database,
    ) -> (
        std::sync::mpsc::Sender<()>,
        tokio::task::JoinHandle<Result<(), String>>,
    ) {
        let (entered, ready) = oneshot::channel();
        let (resume, waiting) = std::sync::mpsc::channel();
        let database = db.clone();
        let task = tokio::spawn(async move {
            database
                .call(move |_| {
                    let _ = entered.send(());
                    waiting
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })
                .await
        });
        ready.await.unwrap();
        (resume, task)
    }

    async fn wait_until_commit_is_queued(db: &Database) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while db.sender.capacity() == 128 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn clean_image_fixture(root: PathBuf, db: Database) {
        db.shutdown().await;
        for attempt in 0..10 {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => return,
                Err(error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied && attempt < 9 =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(error) => panic!("image fixture cleanup failed: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn committed_image_keeps_asset_and_duplicate_discards_only_new_files() {
        let (root, db, store, key) = image_fixture();
        let (pending, record) = pending_record(&store, &key);
        let original_id = pending.metadata.asset_id.clone();
        let result = db
            .commit_received_image(
                record,
                Instant::now() + std::time::Duration::from_secs(20),
                pending,
                finalizing(&key),
                key.clone(),
            )
            .await
            .unwrap();
        assert!(result.inserted);
        assert_eq!(result.ack_index, Some(1));
        assert!(store.read(&original_id, false).is_ok());
        let (duplicate, record) = pending_record(&store, &key);
        let duplicate_id = duplicate.metadata.asset_id.clone();
        let result = db
            .commit_received_image(
                record,
                Instant::now() + std::time::Duration::from_secs(20),
                duplicate,
                finalizing(&key),
                key,
            )
            .await
            .unwrap();
        assert!(!result.inserted);
        assert_eq!(result.ack_index, Some(1));
        assert!(!db.images_dir().join(duplicate_id).exists());
        assert!(store.read(&original_id, false).is_ok());
        clean_image_fixture(root, db).await;
    }

    #[tokio::test]
    async fn queued_image_rejected_before_execution_never_enters_database() {
        let (root, db, store, key) = image_fixture();
        let (pending, record) = pending_record(&store, &key);
        let id = pending.metadata.asset_id.clone();
        let images = finalizing(&key);
        let (resume, blocked) = block_database(&db).await;
        let database = db.clone();
        let states = images.clone();
        let transfer = key.clone();
        let commit = tokio::spawn(async move {
            database
                .commit_received_image(
                    record,
                    Instant::now() + std::time::Duration::from_secs(20),
                    pending,
                    states,
                    transfer,
                )
                .await
        });
        wait_until_commit_is_queued(&db).await;
        let _ = images.lock().unwrap().finish(&key, false, Instant::now());
        resume.send(()).unwrap();
        blocked.await.unwrap().unwrap();
        assert!(commit.await.unwrap().is_err());
        assert!(db.image_asset(id.clone()).await.unwrap().is_none());
        assert!(db
            .history(key.peer_id, "local@fixture".into(), 50, 0)
            .await
            .unwrap()
            .is_empty());
        assert!(!db.images_dir().join(id).exists());
        clean_image_fixture(root, db).await;
    }

    #[tokio::test]
    async fn canceling_commit_waiter_does_not_unlink_committed_asset() {
        let (root, db, store, key) = image_fixture();
        let (pending, record) = pending_record(&store, &key);
        let id = pending.metadata.asset_id.clone();
        let images = finalizing(&key);
        let (resume, blocked) = block_database(&db).await;
        let database = db.clone();
        let states = images.clone();
        let transfer = key.clone();
        let commit = tokio::spawn(async move {
            database
                .commit_received_image(
                    record,
                    Instant::now() + std::time::Duration::from_secs(20),
                    pending,
                    states,
                    transfer,
                )
                .await
        });
        wait_until_commit_is_queued(&db).await;
        commit.abort();
        assert!(commit.await.unwrap_err().is_cancelled());
        resume.send(()).unwrap();
        blocked.await.unwrap().unwrap();
        // A barrier queued after the image proves its transaction has finished,
        // even though the original caller no longer receives its result.
        db.call(|_| Ok(())).await.unwrap();
        assert!(db.image_asset(id.clone()).await.unwrap().is_some());
        assert!(store.read(&id, false).is_ok());
        assert!(!images.lock().unwrap().is_finalizing(&key, Instant::now()));
        clean_image_fixture(root, db).await;
    }
}
