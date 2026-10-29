// file.go 对应 tauri-rust database.rs 的文件消息方法与 file/mod.rs 的 FileMetadata。
// 事务语义用 INSERT...SELECT ... WHERE NOT EXISTS 表达（插件事务不支持语句间分支）。
package store

import (
	"encoding/json"
	"fmt"

	"github.com/egoist/mygo/plugins/sqlite"
)

// FileMetadata 与 Rust file/mod.rs FileMetadata 对齐（camelCase JSON 字段一致，
// 前端 MessageBubble/文件气泡直接消费该结构）。
type FileMetadata struct {
	IsDirectory   bool    `json:"isDirectory"`
	CanResume     bool    `json:"canResume"`
	Attempt       uint32  `json:"attempt"`
	InFlightBytes uint64  `json:"inFlightBytes"`
	FileName      string  `json:"fileName"`
	FileSize      uint64  `json:"fileSize"`
	State         string  `json:"state"`
	Transferred   uint64  `json:"transferred"`
	Incoming      bool    `json:"incoming"`
	HasLocalFile  bool    `json:"hasLocalFile"`
	Error         *string `json:"error"`
}

// Terminal 判断状态机是否已结束（paused 也算终态，可续传）。
func (m FileMetadata) Terminal() bool {
	switch m.State {
	case "completed", "failed", "cancelled", "rejected", "paused":
		return true
	}
	return false
}

// Status 折算成 messages.status（0 发送中 / 2 完成 / 3 失败）。
func (m FileMetadata) Status() int64 {
	if m.State == "completed" {
		return 2
	}
	if m.Terminal() {
		return 3
	}
	return 0
}

// FileState 写回文件元数据与消息状态；path 仅在首次完成时落库（COALESCE 保持旧值）。
func (d *Database) FileState(id string, meta FileMetadata, path *string) error {
	metaJSON, err := json.Marshal(meta)
	if err != nil {
		return err
	}
	var pathArg any
	if path != nil {
		pathArg = *path
	}
	_, err = d.conn.Transaction(d.ctx, []sqlite.Statement{
		{SQL: "UPDATE messages SET status=? WHERE id=?", Args: []any{meta.Status(), id}},
		{SQL: "UPDATE message_files SET metadata=?,local_path=COALESCE(?,local_path) WHERE message_id=?",
			Args: []any{string(metaJSON), pathArg, id}},
	})
	return err
}

// FilePath 返回已接收完成文件的本地保存路径。
func (d *Database) FilePath(id string) (string, error) {
	rows, err := d.query(
		"SELECT local_path FROM message_files WHERE message_id=? AND json_extract(metadata,'$.state')='completed'", id)
	if err != nil {
		return "", err
	}
	if len(rows.Values) == 0 || rows.Values[0][0] == nil {
		return "", fmt.Errorf("文件尚未接收完成或历史已清空")
	}
	path, _ := rows.Values[0][0].(string)
	return path, nil
}

// ReceiveCheckpoint 读取续传检查点 JSON（nil 表示没有记录）。
func (d *Database) ReceiveCheckpoint(id string) (*string, error) {
	rows, err := d.query("SELECT checkpoint FROM file_receives WHERE message_id=?", id)
	if err != nil {
		return nil, err
	}
	if len(rows.Values) == 0 || rows.Values[0][0] == nil {
		return nil, nil
	}
	text, _ := rows.Values[0][0].(string)
	return &text, nil
}

// SaveReceive 保存续传检查点；最多 32 项，与 Rust 版 save_receive 一致。
func (d *Database) SaveReceive(id string, data string) error {
	rows, err := d.query("SELECT COUNT(*) FROM file_receives")
	if err != nil {
		return err
	}
	if count, _ := rows.Values[0][0].(int64); count >= 32 {
		exists, err := d.query("SELECT EXISTS(SELECT 1 FROM file_receives WHERE message_id=?)", id)
		if err != nil {
			return err
		}
		if has, _ := exists.Values[0][0].(int64); has == 0 {
			return fmt.Errorf("续传任务已达32项，请取消不需要的任务")
		}
	}
	_, err = d.exec("INSERT OR REPLACE INTO file_receives VALUES (?,?)", id, data)
	return err
}

// RemoveReceive 删除续传检查点。
func (d *Database) RemoveReceive(id string) error {
	_, err := d.exec("DELETE FROM file_receives WHERE message_id=?", id)
	return err
}

// ReceiveIDs 列出所有带续传检查点的消息 ID。
func (d *Database) ReceiveIDs() ([]string, error) {
	rows, err := d.query("SELECT message_id FROM file_receives")
	if err != nil {
		return nil, err
	}
	ids := make([]string, 0, len(rows.Values))
	for _, v := range rows.Values {
		if s, ok := v[0].(string); ok {
			ids = append(ids, s)
		}
	}
	return ids, nil
}

// FileMetadataByID 读取消息的文件元数据；nil 表示不存在或不是文件消息。
func (d *Database) FileMetadataByID(id string) (*FileMetadata, error) {
	rows, err := d.query("SELECT metadata FROM message_files WHERE message_id=?", id)
	if err != nil {
		return nil, err
	}
	if len(rows.Values) == 0 || rows.Values[0][0] == nil {
		return nil, nil
	}
	text, _ := rows.Values[0][0].(string)
	var meta FileMetadata
	if err := json.Unmarshal([]byte(text), &meta); err != nil {
		return nil, err
	}
	return &meta, nil
}

// hydrateFiles 为 kind==2 的记录补齐文件元数据（对照 Rust hydrate_images 的文件部分）。
func (d *Database) hydrateFiles(records []Record) error {
	for i := range records {
		if records[i].Kind != 2 {
			continue
		}
		meta, err := d.FileMetadataByID(records[i].ID)
		if err != nil {
			return err
		}
		records[i].File = meta
	}
	return nil
}
