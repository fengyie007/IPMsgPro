use rusqlite::{params, Connection, Row};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    thread,
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
}
type Work = Box<dyn FnOnce(&mut Connection) + Send>;
#[derive(Clone)]
pub struct Database {
    sender: mpsc::Sender<Option<Work>>,
    worker: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
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
    })
}
const COLUMNS: &str = "id,from_id,to_id,content,type,timestamp,status";
impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = path.to_owned();
        let (sender, mut receiver) = mpsc::channel::<Option<Work>>(128);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("rust-message-db".into())
            .spawn(move || {
                let init = (|| -> rusqlite::Result<Connection> {
                    let db = Connection::open(path)?;
                    db.busy_timeout(std::time::Duration::from_secs(1))?;
                    db.execute_batch(
                        "PRAGMA journal_mode=WAL;
                    CREATE TABLE IF NOT EXISTS messages (
                      id TEXT PRIMARY KEY, from_id TEXT NOT NULL, to_id TEXT NOT NULL,
                      content TEXT NOT NULL, type INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                      status INTEGER NOT NULL);
                    CREATE INDEX IF NOT EXISTS messages_pair ON messages(from_id,to_id,timestamp);
                    CREATE TABLE IF NOT EXISTS seen_messages (id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
                    CREATE INDEX IF NOT EXISTS seen_messages_time ON seen_messages(seen_at);
                    INSERT OR IGNORE INTO seen_messages SELECT id, CAST(strftime('%s','now') AS INTEGER) FROM messages;
                    UPDATE messages SET status=3 WHERE status=0;",
                    )?;
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
    pub async fn insert(&self, record: Record) -> Result<bool, String> {
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
            Ok(records)
        }).await
    }
    pub async fn recent(&self, local: String, limit: u32) -> Result<Vec<Record>, String> {
        self.call(move |db| {
            let sql = format!("SELECT {COLUMNS} FROM messages WHERE rowid IN
                (SELECT MAX(rowid) FROM messages WHERE from_id=? OR to_id=? GROUP BY CASE WHEN from_id=? THEN to_id ELSE from_id END)
                ORDER BY timestamp DESC,rowid DESC LIMIT ?");
            let mut statement = db.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = statement.query_map(params![local, local, local, limit.min(200)], row)
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
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
            let rows = statement
                .query_map(params![pattern, user, local], row)
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
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
}
