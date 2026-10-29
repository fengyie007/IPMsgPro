package store

// 配置测试对拍 tauri-rust config.rs 的同名用例语义：旧配置补默认、严格类型、
// 校验失败保留原状态与原文件、保存持久化。

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func mustPatch(t *testing.T, v any) json.RawMessage {
	t.Helper()
	raw, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

func openLegacyConfig(t *testing.T, content string) (*ConfigStore, string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "config.json")
	if content != "" {
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	store, err := OpenConfig(path)
	if err != nil {
		t.Fatal(err)
	}
	return store, path
}

func TestLegacyConfigGetsDurableBooleans(t *testing.T) {
	store, path := openLegacyConfig(t, `{"nickname":"legacy"}`)
	if !store.Get().AnnounceOnStartup || !store.Get().AnnounceOnExit || !store.Get().KnockEffects {
		t.Fatal("旧配置应补默认 true")
	}
	for _, key := range []string{"announceOnStartup", "announceOnExit", "knockEffects"} {
		if _, err := store.Save(mustPatch(t, map[string]any{key: false})); err != nil {
			t.Fatal(err)
		}
		reopened, err := OpenConfig(path)
		if err != nil {
			t.Fatal(err)
		}
		if got := map[string]bool{
			"announceOnStartup": reopened.Get().AnnounceOnStartup,
			"announceOnExit":    reopened.Get().AnnounceOnExit,
			"knockEffects":      reopened.Get().KnockEffects,
		}[key]; got {
			t.Fatalf("%s 重启后应持久化为 false", key)
		}
		before, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		for _, invalid := range []any{0, "true", nil} {
			if _, err := store.Save(mustPatch(t, map[string]any{key: invalid})); err == nil {
				t.Fatalf("%s = %v 应被拒绝", key, invalid)
			}
			after, _ := os.ReadFile(path)
			if string(after) != string(before) {
				t.Fatalf("非法补丁不得改写配置文件")
			}
		}
	}
	if !store.Get().ScanOnStartup {
		t.Fatal("scanOnStartup 默认应为 true")
	}
}

func TestAvatarDefaultsAndValidationPreserveSavedConfig(t *testing.T) {
	store, path := openLegacyConfig(t, `{"nickname":"legacy"}`)
	if store.Get().AvatarID != 0 {
		t.Fatal("avatarId 默认应为 0")
	}
	for _, id := range []int{83, 0} {
		if _, err := store.Save(mustPatch(t, map[string]any{"avatarId": id})); err != nil {
			t.Fatal(err)
		}
		reopened, err := OpenConfig(path)
		if err != nil {
			t.Fatal(err)
		}
		if reopened.Get().AvatarID != id {
			t.Fatalf("avatarId 应持久化为 %d", id)
		}
		before, _ := os.ReadFile(path)
		for _, invalid := range []any{84, -1, 1.5, "5", nil} {
			if _, err := store.Save(mustPatch(t, map[string]any{"avatarId": invalid})); err == nil {
				t.Fatalf("avatarId = %v 应被拒绝", invalid)
			}
			if store.Get().AvatarID != id {
				t.Fatalf("非法补丁不得改变内存状态")
			}
			after, _ := os.ReadFile(path)
			if string(after) != string(before) {
				t.Fatalf("非法补丁不得改写配置文件")
			}
		}
	}
}

func TestPatchesValidateBeforeChangingState(t *testing.T) {
	store, _ := openLegacyConfig(t, "")
	for _, patch := range []map[string]any{
		{"minimizeBehavior": "exit"},
		{"directUsers": []string{"127.0.0.1:0"}},
		{"notificationSound": "yes"},
		{"unknownField": 1},
		{"nickname": "含\x00控制符"},
		{"dataDir": "D:/somewhere"},
	} {
		if _, err := store.Save(mustPatch(t, patch)); err == nil {
			t.Fatalf("补丁应被拒绝: %v", patch)
		}
	}
	if store.Get().MinimizeBehavior != "tray" {
		t.Fatal("校验失败不得改变状态")
	}
	next, err := store.Save(mustPatch(t, map[string]any{"nickname": "测试", "directUsers": []string{"127.0.0.1:2426"}}))
	if err != nil {
		t.Fatal(err)
	}
	if next.Nickname != "测试" {
		t.Fatalf("合法补丁应生效: %+v", next)
	}
}

func TestScanSettingsValidateWithoutChangingLegacyDefaults(t *testing.T) {
	store, _ := openLegacyConfig(t, `{"nickname":"原配置","ipScanRanges":[]}`)
	if store.Get().ScanPort != 2425 || store.Get().ScanDelayMs != 20 || !store.Get().ScanOnStartup {
		t.Fatal("扫描默认值应为 2425/20/true")
	}
	next, err := store.Save(mustPatch(t, map[string]any{
		"ipScanRanges": []string{"10.8.33.0/24"}, "scanPort": 2426, "scanDelayMs": 50, "scanOnStartup": false,
	}))
	if err != nil {
		t.Fatal(err)
	}
	if next.ScanPort != 2426 || next.ScanOnStartup {
		t.Fatalf("合法扫描设置应生效: %+v", next)
	}
	for _, patch := range []map[string]any{
		{"scanPort": 0},
		{"scanDelayMs": 9},
		{"scanOnStartup": "yes"},
		{"ipScanRanges": []string{"224.0.0.1"}},
		{"ipScanRanges": []string{"10.0.0.0/16", "10.1.0.0/16"}},
	} {
		if _, err := store.Save(mustPatch(t, patch)); err == nil {
			t.Fatalf("扫描补丁应被拒绝: %v", patch)
		}
	}
}

func TestSaveIsDurableAndFailuresKeepPreviousState(t *testing.T) {
	store, path := openLegacyConfig(t, "")
	if _, err := store.Save(mustPatch(t, map[string]any{"nickname": "持久配置", "minimizeBehavior": "taskbar"})); err != nil {
		t.Fatal(err)
	}
	reopened, err := OpenConfig(path)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(reopened.Get(), store.Get()) {
		t.Fatal("重启后配置应一致")
	}
	before := store.Get()
	bytes, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	// 目录丢失时保存失败，内存与文件均保持原样
	store.path = filepath.Join(filepath.Dir(path), "missing", "config.json")
	if _, err := store.Save(mustPatch(t, map[string]any{"nickname": "不会生效"})); err == nil {
		t.Fatal("写盘失败应报错")
	}
	if !reflect.DeepEqual(store.Get(), before) {
		t.Fatal("写盘失败不得改变内存状态")
	}
	after, _ := os.ReadFile(path)
	if string(after) != string(bytes) {
		t.Fatal("写盘失败不得改写原文件")
	}
	// 损坏的配置：打开报错且保留原文件
	if err := os.WriteFile(path, []byte("{broken"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := OpenConfig(path); err == nil {
		t.Fatal("损坏配置应报错")
	}
	check, _ := os.ReadFile(path)
	if string(check) != "{broken" {
		t.Fatal("损坏配置文件不得被覆盖")
	}
}
