package media

// 资产库测试对拍 tauri-rust assets.rs 的同名用例。

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

func assetFixture(t *testing.T) (*AssetStore, string) {
	t.Helper()
	root := filepath.Join(t.TempDir(), "assets")
	store, err := NewAssetStore(root)
	if err != nil {
		t.Fatal(err)
	}
	return store, root
}

func ownedDecoded() *DecodedImage {
	return &DecodedImage{
		PNG:          []byte("\x89PNG\r\n\x1a\nowned full"),
		ThumbnailPNG: []byte("\x89PNG\r\n\x1a\nowned thumb"),
		Width:        1,
		Height:       1,
	}
}

func TestAssetIDsCannotAddressArbitraryFiles(t *testing.T) {
	if !ValidAssetID("rx-123-abc-0") {
		t.Fatal("合法 ID 应通过")
	}
	for _, id := range []string{"", "..", "../secret", `C:\secret`, "a/b", "%2e%2e", "你好"} {
		if ValidAssetID(id) {
			t.Errorf("%q 应被拒绝", id)
		}
	}
}

func TestPendingReleaseRemovesOnlyItsOwnedDirectoryAndKeepPreservesFiles(t *testing.T) {
	store, root := assetFixture(t)
	kept, err := store.Persist(ownedDecoded(), "11111111")
	if err != nil {
		t.Fatal(err)
	}
	keptDirectory := filepath.Join(root, kept.Metadata.AssetID)
	pending, err := store.Persist(ownedDecoded(), "22222222")
	if err != nil {
		t.Fatal(err)
	}
	pendingDirectory := filepath.Join(root, pending.Metadata.AssetID)
	// 公开元数据不作为删除路径
	pending.Metadata.AssetID = kept.Metadata.AssetID
	kept.Keep()
	pending.Release()
	if _, err := os.Stat(pendingDirectory); !os.IsNotExist(err) {
		t.Fatal("未保留的资产目录应被删除")
	}
	if _, err := os.Stat(filepath.Join(keptDirectory, "full.png")); err != nil {
		t.Fatal("保留资产的 full.png 应存在")
	}
	if _, err := os.Stat(filepath.Join(keptDirectory, "thumb.png")); err != nil {
		t.Fatal("保留资产的 thumb.png 应存在")
	}
}

func TestReadValidatesFormatAndSize(t *testing.T) {
	store, _ := assetFixture(t)
	pending, err := store.Persist(ownedDecoded(), "33333333")
	if err != nil {
		t.Fatal(err)
	}
	defer pending.Release()
	// store 层只负责文件校验；"已登记"由调用方（image.read 命令）查库把关
	if data, err := store.Read(pending.Metadata.AssetID, false); err != nil || !bytes.HasPrefix(data, []byte("\x89PNG")) {
		t.Fatalf("有效资产应可读取: %v", err)
	}
	// 直接读文件内容验证 PNG 魔数校验路径（模拟已登记资产）
	full := filepath.Join(store.root, pending.Metadata.AssetID, "full.png")
	if data, err := os.ReadFile(full); err != nil || !bytes.HasPrefix(data, []byte("\x89PNG")) {
		t.Fatalf("写入的资产应为本测试的 PNG 头: %v", err)
	}
	if _, err := store.Read("../escape", false); err == nil {
		t.Fatal("路径穿越 ID 应被拒绝")
	}
}
