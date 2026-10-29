package filetransfer

// 对拍 tauri-rust file/storage.rs 的语义：临时文件只增不覆盖，发布重名自动绕行。

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestPartialCreatePublishResume(t *testing.T) {
	root := t.TempDir()
	pf, file, err := CreatePartial(root, 7)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.Write([]byte("hello")); err != nil {
		t.Fatal(err)
	}
	file.Close()

	// 续传：分片未变时按名打开成功，超过预期大小拒绝。
	if _, _, err := ResumePartial(root, pf.Name(), 3); err == nil {
		t.Fatalf("超尺寸续传应被拒绝")
	}
	resumed, rfile, err := ResumePartial(root, pf.Name(), 100)
	if err != nil {
		t.Fatal(err)
	}
	info, _ := rfile.Stat()
	if info.Size() != 5 {
		t.Fatalf("续传长度异常: %d", info.Size())
	}
	rfile.Close()
	resumed.Discard()

	// 发布为正式文件。
	dest, err := pf.Publish(root, "报告.txt")
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(dest) != "报告.txt" {
		t.Fatalf("发布名称异常: %q", dest)
	}
	data, err := os.ReadFile(dest)
	if err != nil || string(data) != "hello" {
		t.Fatalf("发布内容异常: %q %v", data, err)
	}

	// 发布后不能再当作分片续传。
	if _, _, err := ResumePartial(root, "报告.txt", 100); err == nil {
		t.Fatalf("正式文件不能当作分片续传")
	}

	// 同名发布自动绕行为 "报告 (1).txt"。
	pf2, file2, err := CreatePartial(root, 8)
	if err != nil {
		t.Fatal(err)
	}
	file2.Close()
	dest2, err := pf2.Publish(root, "报告.txt")
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(dest2) != "报告 (1).txt" {
		t.Fatalf("重名绕行异常: %q", dest2)
	}
}

func TestCheckedPartValidation(t *testing.T) {
	if _, err := checkedPart(t.TempDir(), "../evil.part"); err == nil {
		t.Fatalf("路径穿越应被拒绝")
	}
	if _, err := checkedPart(t.TempDir(), "not-a-part"); err == nil {
		t.Fatalf("非分片名应被拒绝")
	}
	ok := ".ipmsg-123-45-6-0.part"
	path, err := checkedPart(t.TempDir(), ok)
	if err != nil || !strings.HasSuffix(path, ok) {
		t.Fatalf("合法续传标识被拒绝: %v %q", err, path)
	}
}

func TestReservedNamesSanitized(t *testing.T) {
	if got := SanitizeName("aux.txt"); got != "_aux.txt" {
		t.Fatalf("保留名未转义: %q", got)
	}
	if got := SanitizeName("中文.txt"); got != "中文.txt" {
		t.Fatalf("中文名被破坏: %q", got)
	}
	if got := SanitizeName("con "); got != "_con" {
		t.Fatalf("保留名未转义: %q", got)
	}
}
