// image.go 对应 tauri-rust database.rs 的图片资产方法。
// 事务语义用 INSERT...SELECT ... WHERE EXISTS 表达（插件事务不支持语句间分支）。
package store

import (
	"fmt"

	"speedipmsg-go/internal/media"

	"github.com/egoist/mygo/plugins/sqlite"
)

// ImageAsset 查询已登记且未废弃的图片资产；nil 表示不存在。
func (d *Database) ImageAsset(id string) (*media.ImageMetadata, error) {
	rows, err := d.query(
		"SELECT asset_id,file_name,file_size,mime,width,height FROM image_assets WHERE asset_id=? AND temporary<>2", id)
	if err != nil {
		return nil, err
	}
	if len(rows.Values) == 0 {
		return nil, nil
	}
	v := rows.Values[0]
	fileSize, _ := v[2].(int64)
	width, _ := v[4].(int64)
	height, _ := v[5].(int64)
	fileName, _ := v[1].(string)
	mime, _ := v[3].(string)
	return &media.ImageMetadata{
		AssetID:  v[0].(string),
		FileName: fileName,
		FileSize: uint64(fileSize),
		Mime:     mime,
		Width:    uint32(width),
		Height:   uint32(height),
	}, nil
}

// CommitReceivedImage 原子提交收到的图片：去重键 + 资产 + 消息 + 关联，
// 仅当本事务新登记了去重键才写入（重复传输不重复入库）。
func (d *Database) CommitReceivedImage(record Record, metadata media.ImageMetadata) (bool, error) {
	results, err := d.conn.Transaction(d.ctx, []sqlite.Statement{
		{SQL: "INSERT OR IGNORE INTO seen_messages (id,seen_at) VALUES (?,CAST(strftime('%s','now') AS INTEGER))", Args: []any{record.ID}},
		{SQL: "INSERT INTO image_assets (asset_id,file_name,file_size,mime,width,height) SELECT ?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM seen_messages WHERE id=?)",
			Args: []any{metadata.AssetID, metadata.FileName, metadata.FileSize, metadata.Mime, metadata.Width, metadata.Height, record.ID}},
		{SQL: "INSERT INTO messages (id,from_id,to_id,content,type,timestamp,status) SELECT ?,?,?,?," + itoaLiteral(record.Kind) + "," + itoaLiteral(record.Timestamp) + "," + itoaLiteral(record.Status) + " WHERE EXISTS(SELECT 1 FROM seen_messages WHERE id=?)",
			Args: []any{record.ID, record.FromID, record.ToID, record.Content, record.ID}},
		{SQL: "INSERT INTO message_images (message_id,asset_id) SELECT ?,? WHERE EXISTS(SELECT 1 FROM seen_messages WHERE id=?)",
			Args: []any{record.ID, metadata.AssetID, record.ID}},
	})
	if err != nil {
		return false, err
	}
	return results[0].Changes > 0, nil
}

// InsertSentImage 把发送的图片从预览升级为历史资产；元数据必须与已登记预览完全一致。
func (d *Database) InsertSentImage(record Record, metadata media.ImageMetadata) error {
	results, err := d.conn.Transaction(d.ctx, []sqlite.Statement{
		{SQL: "INSERT INTO messages (id,from_id,to_id,content,type,timestamp,status) SELECT ?,?,?,?,1,?,0 WHERE EXISTS(SELECT 1 FROM image_assets WHERE asset_id=? AND temporary<>2 AND file_name=? AND file_size=? AND mime=? AND width=? AND height=?)",
			Args: []any{record.ID, record.FromID, record.ToID, record.Content, record.Timestamp, metadata.AssetID, metadata.FileName, metadata.FileSize, metadata.Mime, metadata.Width, metadata.Height}},
		{SQL: "INSERT INTO seen_messages (id,seen_at) SELECT ?,CAST(strftime('%s','now') AS INTEGER) WHERE EXISTS(SELECT 1 FROM messages WHERE id=?)",
			Args: []any{record.ID, record.ID}},
		{SQL: "INSERT INTO message_images (message_id,asset_id) SELECT ?,? WHERE EXISTS(SELECT 1 FROM messages WHERE id=?)",
			Args: []any{record.ID, metadata.AssetID, record.ID}},
		{SQL: "UPDATE image_assets SET temporary=0 WHERE asset_id=?", Args: []any{metadata.AssetID}},
	})
	if err != nil {
		return err
	}
	if results[0].Changes == 0 {
		return fmt.Errorf("图片预览已过期，请重新选择")
	}
	return nil
}

// RegisterImport 登记原生选择器导入的待发送预览；预览上限 8 项。
func (d *Database) RegisterImport(metadata media.ImageMetadata) error {
	rows, err := d.query("SELECT COUNT(*) FROM image_assets WHERE temporary<>0")
	if err != nil {
		return err
	}
	if count, _ := rows.Values[0][0].(int64); count >= 8 {
		return fmt.Errorf("待发送预览过多，请先取消已有预览")
	}
	_, err = d.conn.Transaction(d.ctx, []sqlite.Statement{
		{SQL: "INSERT INTO image_assets (asset_id,file_name,file_size,mime,width,height,temporary,created_at) VALUES (?,?,?,?,?,?,1,CAST(strftime('%s','now') AS INTEGER))",
			Args: []any{metadata.AssetID, metadata.FileName, metadata.FileSize, metadata.Mime, metadata.Width, metadata.Height}},
	})
	return err
}

// DiscardImports 把未引用的废弃/过期预览标记为 temporary=2，返回被标记的资产 ID；
// 文件删除由调用方完成，失败时墓碑可重试。
func (d *Database) DiscardImports(id *string, before int64) ([]string, error) {
	args := []any{before, nil, nil}
	if id != nil {
		args = []any{before, *id, *id}
	}
	rows, err := d.query(
		"UPDATE image_assets SET temporary=2 WHERE (temporary=2 OR (temporary=1 AND created_at<=?)) AND (? IS NULL OR asset_id=?) AND NOT EXISTS(SELECT 1 FROM message_images WHERE message_images.asset_id=image_assets.asset_id) RETURNING asset_id",
		args...)
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

// ForgetDiscardedAsset 删除已标记废弃的资产行。
func (d *Database) ForgetDiscardedAsset(id string) error {
	_, err := d.exec("DELETE FROM image_assets WHERE asset_id=? AND temporary=2", id)
	return err
}

func itoaLiteral(v int64) string {
	return fmt.Sprintf("%d", v)
}
