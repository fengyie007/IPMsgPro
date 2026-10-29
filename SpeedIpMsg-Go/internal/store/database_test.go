package store

// 数据库测试对拍 tauri-rust database.rs 的同名用例语义。

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func tempDB(t *testing.T) (string, *Database) {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "messages.db")
	db, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	return path, db
}

func TestDedupePagingAndClear(t *testing.T) {
	path, db := tempDB(t)
	for n := 0; n < 5; n++ {
		record := Record{ID: fmt.Sprint(n), FromID: "me", ToID: "peer", Content: fmt.Sprintf("行%d%%", n), Kind: 0, Timestamp: int64(n), Status: 0}
		if ok, err := db.Insert(record); err != nil || !ok {
			t.Fatalf("首插应成功: %v %v", ok, err)
		}
		if ok, err := db.Insert(record); err != nil || ok {
			t.Fatalf("重复插入应被去重: %v %v", ok, err)
		}
	}
	page, err := db.History("peer", "me", 2, 1)
	if err != nil {
		t.Fatal(err)
	}
	if len(page) != 2 || page[0].ID != "2" || page[1].ID != "3" {
		t.Fatalf("分页结果异常: %+v", page)
	}
	if recent, err := db.Recent("me", 10); err != nil || len(recent) != 1 {
		t.Fatalf("recent 应只有 1 条: %d %v", len(recent), err)
	}
	if hits, err := db.Search("%", nil, "me"); err != nil || len(hits) != 5 {
		t.Fatalf("LIKE 转义后应命中 5 条: %d %v", len(hits), err)
	}
	db.Close()

	// 重启后：发送中(status=0)应置为失败(status=3)
	reopened, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	history, err := reopened.History("peer", "me", 10, 0)
	if err != nil {
		t.Fatal(err)
	}
	for _, record := range history {
		if record.Status != 3 {
			t.Fatalf("重启后 status 应为 3: %+v", record)
		}
	}
	ids, err := reopened.Clear(ptr("peer"), "me")
	if err != nil {
		t.Fatal(err)
	}
	if len(ids) != 5 {
		t.Fatalf("clear 应删除 5 条: %d", len(ids))
	}
	if recent, err := reopened.Recent("me", 10); err != nil || len(recent) != 0 {
		t.Fatalf("清空后 recent 应为空: %d %v", len(recent), err)
	}
}

func TestScopedSearchAndClearShareExactBoundaries(t *testing.T) {
	path, db := tempDB(t)
	wanted := Record{ID: "wanted", FromID: "me", ToID: "peer", Content: "needle", Kind: 0, Timestamp: 1, Status: 1}
	if ok, err := db.Insert(wanted); err != nil || !ok {
		t.Fatal(err)
	}
	for n := 0; n < 205; n++ {
		other := Record{ID: fmt.Sprintf("other%d", n), FromID: "me", ToID: "other", Content: "needle", Kind: 0, Timestamp: int64(10 + n), Status: 1}
		if ok, err := db.Insert(other); err != nil || !ok {
			t.Fatal(err)
		}
	}
	matches, err := db.Search("needle", ptr("peer"), "me")
	if err != nil {
		t.Fatal(err)
	}
	if len(matches) != 1 || matches[0].ID != "wanted" {
		t.Fatalf("范围内搜索应只命中 wanted: %+v", matches)
	}
	between := Record{ID: "before-delete-exec", FromID: "me", ToID: "peer", Content: "needle", Kind: 0, Timestamp: 999, Status: 1}
	if ok, err := db.Insert(between); err != nil || !ok {
		t.Fatal(err)
	}
	deleted, err := db.Clear(ptr("peer"), "me")
	if err != nil {
		t.Fatal(err)
	}
	if len(deleted) != 2 || deleted[0] != "wanted" || deleted[1] != "before-delete-exec" {
		t.Fatalf("clear 边界异常: %v", deleted)
	}
	after := Record{ID: "after-delete", FromID: "me", ToID: "peer", Content: "needle", Kind: 0, Timestamp: 1000, Status: 1}
	if ok, err := db.Insert(after); err != nil || !ok {
		t.Fatal(err)
	}
	// 清空前的 ID 再来一次：去重记录还在，不得复活
	if ok, err := db.Insert(between); err != nil || ok {
		t.Fatalf("清空后旧 ID 不应复活: %v %v", ok, err)
	}
	history, err := db.History("peer", "me", 10, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 1 || history[0].ID != "after-delete" {
		t.Fatalf("清空后 peer 历史应只剩 after-delete: %+v", history)
	}
	db.Close()

	reopened, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	if ok, err := reopened.Insert(wanted); err != nil || ok {
		t.Fatalf("重启后旧 ID 仍不应复活: %v %v", ok, err)
	}
}

func TestSchemaVersionWALAndQuickCheck(t *testing.T) {
	path, db := tempDB(t)
	if rows, err := db.query("PRAGMA user_version"); err != nil || rows.Values[0][0].(int64) != schemaVersion {
		t.Fatalf("user_version 应为 %d: %v %v", schemaVersion, rows, err)
	}
	if rows, err := db.query("PRAGMA journal_mode"); err != nil || rows.Values[0][0].(string) != "wal" {
		t.Fatalf("journal_mode 应为 wal: %v %v", rows, err)
	}
	// 外键约束生效：message_images 引用不存在的消息应失败
	if _, err := db.exec("INSERT INTO message_images (message_id,asset_id) VALUES ('ghost','a')"); err == nil {
		t.Fatal("foreign_keys=ON 下应拒绝孤儿引用")
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("数据库文件应存在: %v", err)
	}
}

func ptr[T any](v T) *T { return &v }
