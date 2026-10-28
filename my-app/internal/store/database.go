package store

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo/plugins/sqlite"
)

// Record 与 Rust 版 database.rs Record 对齐（JSON 字段名一致，type 字段名保持 "type"）。
type Record struct {
	ID        string        `json:"id"`
	FromID    string        `json:"fromId"`
	ToID      string        `json:"toId"`
	Content   string        `json:"content"`
	Kind      int64         `json:"type"`
	Timestamp int64         `json:"timestamp"`
	Status    int64         `json:"status"`
	Image     any           `json:"-"`
	File      *FileMetadata `json:"file,omitempty"`
}

// schemaVersion 与 Rust 版 schema 6 对齐；建表语义逐条照抄 database.rs open()。
const schemaVersion = 6

const columns = "id,from_id,to_id,content,type,timestamp,status"

// Database 是单连接 SQLite 封装。mygo sqlite 插件在连接内串行化所有操作
// （gate channel），事务用插件的 Transaction API（BEGIN IMMEDIATE），无需额外锁。
type Database struct {
	conn *sqlite.DB
	ctx  context.Context
}

func Open(path string) (*Database, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("创建数据目录失败：%w", err)
	}
	ctx := context.Background()
	conn, err := sqlite.Open(ctx, path, sqlite.OpenOptions{BusyTimeout: time.Second})
	if err != nil {
		return nil, fmt.Errorf("打开数据库失败：%w", err)
	}
	db := &Database{conn: conn, ctx: ctx}
	if err := db.init(); err != nil {
		conn.Close()
		return nil, err
	}
	return db, nil
}

func (d *Database) init() error {
	if _, err := d.conn.Execute(d.ctx, "PRAGMA foreign_keys=ON"); err != nil {
		return fmt.Errorf("PRAGMA foreign_keys 失败：%w", err)
	}
	if _, err := d.conn.Query(d.ctx, "PRAGMA journal_mode=WAL"); err != nil {
		return fmt.Errorf("PRAGMA journal_mode 失败：%w", err)
	}
	versionRows, err := d.conn.Query(d.ctx, "PRAGMA user_version")
	if err != nil {
		return fmt.Errorf("读取 schema 版本失败：%w", err)
	}
	var version int64
	if len(versionRows.Values) > 0 && len(versionRows.Values[0]) > 0 {
		if v, ok := versionRows.Values[0][0].(int64); ok {
			version = v
		}
	}
	if version > schemaVersion {
		return fmt.Errorf("数据库 schema 版本 %d 高于本程序支持的 %d", version, schemaVersion)
	}

	statements := []sqlite.Statement{
		{SQL: "CREATE TABLE IF NOT EXISTS messages (id TEXT PRIMARY KEY, from_id TEXT NOT NULL, to_id TEXT NOT NULL, content TEXT NOT NULL, type INTEGER NOT NULL, timestamp INTEGER NOT NULL, status INTEGER NOT NULL)"},
		{SQL: "CREATE INDEX IF NOT EXISTS messages_pair ON messages(from_id,to_id,timestamp)"},
		{SQL: "CREATE TABLE IF NOT EXISTS seen_messages (id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL)"},
		{SQL: "CREATE INDEX IF NOT EXISTS seen_messages_time ON seen_messages(seen_at)"},
		{SQL: "INSERT OR IGNORE INTO seen_messages SELECT id, CAST(strftime('%s','now') AS INTEGER) FROM messages"},
		{SQL: "UPDATE messages SET status=3 WHERE status=0"},
		{SQL: "CREATE TABLE IF NOT EXISTS image_assets (asset_id TEXT PRIMARY KEY, file_name TEXT NOT NULL, file_size INTEGER NOT NULL, mime TEXT NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL)"},
		{SQL: "CREATE TABLE IF NOT EXISTS message_images (message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE, asset_id TEXT NOT NULL REFERENCES image_assets(asset_id))"},
	}
	if version < 3 {
		statements = append(statements,
			sqlite.Statement{SQL: "ALTER TABLE image_assets ADD COLUMN temporary INTEGER NOT NULL DEFAULT 0"},
			sqlite.Statement{SQL: "ALTER TABLE image_assets ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0"},
		)
	}
	statements = append(statements,
		sqlite.Statement{SQL: "CREATE TABLE IF NOT EXISTS message_files (message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE, metadata TEXT NOT NULL, local_path TEXT)"},
		sqlite.Statement{SQL: "CREATE TABLE IF NOT EXISTS file_receives (message_id TEXT PRIMARY KEY, checkpoint TEXT NOT NULL)"},
		sqlite.Statement{SQL: "UPDATE message_files SET metadata=json_set(metadata,'$.state',CASE WHEN json_extract(metadata,'$.canResume')=1 THEN 'paused' ELSE 'failed' END,'$.attempt',COALESCE(json_extract(metadata,'$.attempt'),0)+1,'$.error','程序重启，传输已中断') WHERE json_extract(metadata,'$.state') IN ('offered','transferring','finalizing')"},
		sqlite.Statement{SQL: "CREATE TABLE IF NOT EXISTS peer_avatars (peer_id TEXT PRIMARY KEY, avatar TEXT NOT NULL, updated_at INTEGER NOT NULL)"},
		sqlite.Statement{SQL: "CREATE INDEX IF NOT EXISTS peer_avatars_updated ON peer_avatars(updated_at)"},
		sqlite.Statement{SQL: fmt.Sprintf("PRAGMA user_version=%d", schemaVersion)},
	)
	if _, err := d.conn.Transaction(d.ctx, statements); err != nil {
		return fmt.Errorf("初始化 schema 失败：%w", err)
	}
	return nil
}

func (d *Database) Close() error {
	_, _ = d.conn.Execute(d.ctx, "PRAGMA wal_checkpoint(TRUNCATE)")
	return d.conn.Close()
}

func (d *Database) exec(sql string, args ...any) (sqlite.Result, error) {
	return d.conn.Execute(d.ctx, sql, args...)
}

func (d *Database) query(sql string, args ...any) (sqlite.Rows, error) {
	return d.conn.Query(d.ctx, sql, args...)
}

// Insert 去重语义与 Rust 版一致：seen_messages 只保留 7 天内的 ID，
// 清空后的重发不能复活旧消息（含跨进程重启）。
// 语句在同一事务内原子提交：1. 清理过期 seen；2. 仅当 seen 无该 ID 时插入消息；
// 3. 仅当上一步真正插入（changes()>0）时登记文件元数据
// （message_files 外键引用 messages，必须在消息行之后写入）；4. 登记 seen。
func (d *Database) Insert(record Record) (bool, error) {
	if record.Image != nil {
		return false, fmt.Errorf("图片必须通过完整图片事务保存")
	}
	statements := []sqlite.Statement{
		{SQL: "DELETE FROM seen_messages WHERE seen_at < CAST(strftime('%s','now') AS INTEGER)-604800"},
		{SQL: "INSERT INTO messages (id,from_id,to_id,content,type,timestamp,status) SELECT ?,?,?,?,?,?,? WHERE NOT EXISTS(SELECT 1 FROM seen_messages WHERE id=?)",
			Args: []any{record.ID, record.FromID, record.ToID, record.Content, record.Kind, record.Timestamp, record.Status, record.ID}},
	}
	if record.File != nil {
		metaJSON, err := json.Marshal(record.File)
		if err != nil {
			return false, err
		}
		statements = append(statements, sqlite.Statement{
			SQL:  "INSERT INTO message_files(message_id,metadata) SELECT ?,? WHERE (SELECT changes())>0",
			Args: []any{record.ID, string(metaJSON)}})
	}
	statements = append(statements, sqlite.Statement{
		SQL:  "INSERT OR IGNORE INTO seen_messages (id,seen_at) VALUES (?,CAST(strftime('%s','now') AS INTEGER))",
		Args: []any{record.ID}})
	results, err := d.conn.Transaction(d.ctx, statements)
	if err != nil {
		return false, err
	}
	return results[1].Changes > 0, nil
}

func (d *Database) Status(id string, status int64) error {
	_, err := d.exec("UPDATE messages SET status=? WHERE id=?", status, id)
	return err
}

// InterruptSending 程序退出/发送中断时把"发送中"置为失败（对照 database.rs interrupt_sending）。
func (d *Database) InterruptSending(id string) error {
	_, err := d.exec("UPDATE messages SET status=3 WHERE id=? AND status=0", id)
	return err
}

func rowToRecord(values []any) (Record, error) {
	if len(values) != 7 {
		return Record{}, fmt.Errorf("消息行列数异常：%d", len(values))
	}
	id, ok := values[0].(string)
	if !ok {
		return Record{}, fmt.Errorf("消息 id 类型异常")
	}
	fromID, _ := values[1].(string)
	toID, _ := values[2].(string)
	content, _ := values[3].(string)
	kind, _ := values[4].(int64)
	timestamp, _ := values[5].(int64)
	status, _ := values[6].(int64)
	return Record{ID: id, FromID: fromID, ToID: toID, Content: content, Kind: kind, Timestamp: timestamp, Status: status}, nil
}

func rowsToRecords(rows sqlite.Rows) ([]Record, error) {
	out := make([]Record, 0, len(rows.Values))
	for _, values := range rows.Values {
		record, err := rowToRecord(values)
		if err != nil {
			return nil, err
		}
		out = append(out, record)
	}
	return out, nil
}

func (d *Database) History(user, local string, limit, offset uint32) ([]Record, error) {
	if limit > 200 {
		limit = 200
	}
	rows, err := d.query(
		"SELECT "+columns+" FROM messages WHERE (from_id=? AND to_id=?) OR (from_id=? AND to_id=?) ORDER BY timestamp DESC,rowid DESC LIMIT ? OFFSET ?",
		local, user, user, local, limit, offset)
	if err != nil {
		return nil, err
	}
	records, err := rowsToRecords(rows)
	if err != nil {
		return nil, err
	}
	// Rust 版 DESC 取页后 reverse，让调用方拿到时间正序
	for i, j := 0, len(records)-1; i < j; i, j = i+1, j-1 {
		records[i], records[j] = records[j], records[i]
	}
	if err := d.hydrateFiles(records); err != nil {
		return nil, err
	}
	return records, nil
}

func (d *Database) Recent(local string, limit uint32) ([]Record, error) {
	if limit > 200 {
		limit = 200
	}
	rows, err := d.query(
		"SELECT "+columns+" FROM messages WHERE rowid IN (SELECT MAX(rowid) FROM messages WHERE from_id=? OR to_id=? GROUP BY CASE WHEN from_id=? THEN to_id ELSE from_id END) ORDER BY timestamp DESC,rowid DESC LIMIT ?",
		local, local, local, limit)
	if err != nil {
		return nil, err
	}
	records, err := rowsToRecords(rows)
	if err != nil {
		return nil, err
	}
	if err := d.hydrateFiles(records); err != nil {
		return nil, err
	}
	return records, nil
}

// Search 关键字做 LIKE 转义；user 为 nil 时全局搜索。
func (d *Database) Search(keyword string, user *string, local string) ([]Record, error) {
	pattern := "%" +
		strings.ReplaceAll(strings.ReplaceAll(strings.ReplaceAll(keyword, "\\", "\\\\"), "%", "\\%"), "_", "\\_") +
		"%"
	args := []any{pattern, nil, local}
	if user != nil {
		args = []any{pattern, *user, local}
	}
	rows, err := d.query(
		"SELECT "+columns+" FROM messages WHERE content LIKE ?1 ESCAPE '\\' AND (?2 IS NULL OR (from_id=?3 AND to_id=?2) OR (from_id=?2 AND to_id=?3)) ORDER BY timestamp DESC,rowid DESC LIMIT 200",
		args...)
	if err != nil {
		return nil, err
	}
	records, err := rowsToRecords(rows)
	if err != nil {
		return nil, err
	}
	if err := d.hydrateFiles(records); err != nil {
		return nil, err
	}
	return records, nil
}

// Clear 删除指定会话（或全部）消息，返回被删 ID 列表；迟到事件凭这份名单判断去重边界。
// DELETE ... RETURNING 在单条语句内完成"取 ID + 删除"，与 Rust 版事务内先 SELECT 后 DELETE 等价。
func (d *Database) Clear(user *string, local string) ([]string, error) {
	if user != nil && *user == "" {
		user = nil
	}
	const scope = "WHERE (?1 IS NULL OR (from_id=?2 AND to_id=?1) OR (from_id=?1 AND to_id=?2))"
	args := []any{nil, local}
	if user != nil {
		args = []any{*user, local}
	}
	rows, err := d.query("DELETE FROM messages "+scope+" RETURNING id", args...)
	if err != nil {
		return nil, err
	}
	deleted := make([]string, 0, len(rows.Values))
	for _, values := range rows.Values {
		if id, ok := values[0].(string); ok {
			deleted = append(deleted, id)
		}
	}
	return deleted, nil
}

// RecordFromJSON 便于上层从事件载荷构造记录。
func RecordFromJSON(data []byte) (Record, error) {
	var record Record
	if err := json.Unmarshal(data, &record); err != nil {
		return Record{}, err
	}
	return record, nil
}
