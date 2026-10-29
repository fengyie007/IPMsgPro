package app

// runtime_test.go 验证命令分发层的关键回归：storage.* 命令已真实接线
// （曾因旧构建产物报"将在后续阶段迁移"）。

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"speedipmsg-go/internal/network"
	"speedipmsg-go/internal/storage"
	"speedipmsg-go/internal/store"
)

func newTestRuntime(t *testing.T) *Runtime {
	t.Helper()
	dir := t.TempDir()
	db, err := store.Open(filepath.Join(dir, "test.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	config, err := store.OpenConfig(filepath.Join(dir, "config.json"))
	if err != nil {
		t.Fatal(err)
	}
	hub, err := network.NewHub(0, "Tester", "", "inst-test")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(hub.Close)
	boot, err := storage.Open(filepath.Join(dir, "bootstrap"), 2425)
	if err != nil {
		t.Fatal(err)
	}
	r := New(hub, db, config, "test", boot.Active)
	r.Storage = boot
	r.accepting.Store(true)
	return r
}

func TestStorageInfoIsWired(t *testing.T) {
	r := newTestRuntime(t)
	result := r.Handle("storage.info", `{}`)
	if !strings.Contains(result, `"success":true`) {
		t.Fatalf("storage.info 应成功: %s", result)
	}
	for _, key := range []string{"active", "defaultDirectory", "isDefault"} {
		if !strings.Contains(result, key) {
			t.Fatalf("storage.info 缺少字段 %s: %s", key, result)
		}
	}
}

func TestStoragePrepareApplyCancelFlow(t *testing.T) {
	r := newTestRuntime(t)
	parent := t.TempDir()
	result := r.Handle("storage.info", `{}`)
	if !strings.Contains(result, `"success":true`) {
		t.Fatalf("storage.info 应成功: %s", result)
	}
	// prepare 经 Handle 需要 Dialog；直接走 Storage 层验证 prepare/apply/cancel。
	prepared, err := r.Storage.Prepare(parent, false)
	if err != nil {
		t.Fatal(err)
	}
	selection, _ := prepared["selectionId"].(string)
	if applied, err := r.Storage.Apply(selection); err != nil || applied["pending"] == nil {
		t.Fatalf("apply 应登记 pending: %v %v", applied, err)
	}
	if cancelled, err := r.Storage.CancelPending(); err != nil || cancelled["pending"] != nil {
		t.Fatalf("cancel 应清空 pending: %v %v", cancelled, err)
	}
}

func TestStorageDefaultPreparesReset(t *testing.T) {
	r := newTestRuntime(t)
	// 与 Rust 一致：默认位置上"恢复默认位置"是无效操作（前端禁用该按钮）。
	if result := r.Handle("storage.default", `{}`); strings.Contains(result, `"success":true`) {
		t.Fatalf("默认位置上 storage.default 应被拒绝: %s", result)
	}
	// 模拟已迁移到自定义位置：登记 pending 并重启重开 Storage（迁移要求源数据完整）。
	writeFile := func(path, content string) {
		if err := os.WriteFile(path, []byte(content), 0o666); err != nil {
			t.Fatal(err)
		}
	}
	writeFile(filepath.Join(r.Storage.Bootstrap(), "config.json"), "{}")
	writeFile(filepath.Join(r.Storage.Bootstrap(), "ipmsg.db"), "data")
	parent := t.TempDir()
	prepared, err := r.Storage.Prepare(parent, false)
	if err != nil {
		t.Fatal(err)
	}
	selection, _ := prepared["selectionId"].(string)
	if _, err := r.Storage.Apply(selection); err != nil {
		t.Fatal(err)
	}
	boot, err := storage.Open(r.Storage.Bootstrap(), 2425)
	if err != nil {
		t.Fatal(err)
	}
	if samePath(boot.Active, r.Storage.Bootstrap()) {
		t.Fatalf("重启后应切到新数据集: %s", boot.Active)
	}
	r2 := New(r.Hub, r.DB, r.Config, "test", boot.Active)
	r2.Storage = boot
	r2.accepting.Store(true)
	result := r2.Handle("storage.default", `{}`)
	if !strings.Contains(result, `"success":true`) || !strings.Contains(result, "selectionId") {
		t.Fatalf("迁移后 storage.default 应返回选择: %s", result)
	}
}

func samePath(a, b string) bool {
	return strings.EqualFold(a, b)
}

// image.read 返回的地址必须是前端（WebView2）能直接加载的形式：Windows 用
// mygo 映射的 http://<scheme>.localhost/，其它平台用 <scheme>://localhost/。
// 曾返回裸 ipmsg-image://localhost/…，WebView2 不认，图片一律加载失败。
func TestSchemeURLLoadableForm(t *testing.T) {
	got := schemeURL("ipmsg-image", "rx-1-2-3")
	if runtime.GOOS == "windows" {
		if got != "http://ipmsg-image.localhost/rx-1-2-3" {
			t.Fatalf("Windows 上应返回 http://<scheme>.localhost：%s", got)
		}
	} else if got != "ipmsg-image://localhost/rx-1-2-3" {
		t.Fatalf("非 Windows 应返回 <scheme>://localhost：%s", got)
	}
}
