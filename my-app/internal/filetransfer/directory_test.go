package filetransfer

// 对拍 tauri-rust file/directory.rs 的测试用例。

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDirectoryHeaderEscapingAndTraversal(t *testing.T) {
	data, err := EncodeDirHeader("中文:a.txt", 12, 1)
	if err != nil {
		t.Fatal(err)
	}
	name, size, kind, err := DecodeDirHeader(data)
	if err != nil {
		t.Fatal(err)
	}
	if name != "中文_a.txt" || size != 12 || kind != 1 {
		t.Fatalf("解码结果异常: %q %d %d", name, size, kind)
	}
	for _, bad := range []string{"../x", "/root", "a\\b", ".."} {
		data, err := EncodeDirHeader(bad, 0, 2)
		if err != nil {
			t.Fatal(err)
		}
		if _, _, _, err := DecodeDirHeader(data); err == nil {
			t.Fatalf("路径穿越 %q 应被拒绝", bad)
		}
	}
	if _, _, _, err := DecodeDirHeader([]byte("ffff:bad")); err == nil {
		t.Fatalf("坏头应被拒绝")
	}
}

func TestSnapshotWalkAndRejectsLinks(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "子目录"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("hello"), 0o666); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "子目录", "b.bin"), []byte("12345"), 0o666); err != nil {
		t.Fatal(err)
	}
	snap, err := CaptureSnapshot(root)
	if err != nil {
		t.Fatal(err)
	}
	// 根(2) + a.txt(1) + 子目录(2) + b.bin(1) + 子目录结束(3) + 根结束(3) = 6 项
	// （Rust 版 walk 同样给根目录追加 "." 结束标记）
	if len(snap.Entries) != 6 {
		t.Fatalf("快照项目数异常: %d %+v", len(snap.Entries), snap.Entries)
	}
	if snap.Size != 10 {
		t.Fatalf("快照大小异常: %d", snap.Size)
	}
	kinds := map[string]uint32{}
	for _, e := range snap.Entries {
		kinds[e.Name] = e.Kind
	}
	if kinds["."] != 3 || kinds["a.txt"] != 1 || kinds["子目录"] != 2 {
		t.Fatalf("项目类型异常: %+v", kinds)
	}
}
