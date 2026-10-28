package filetransfer

// 文件协议测试对拍 tauri-rust file/protocol.rs 的同名用例。

import (
	"testing"

	"my-app/internal/ipmsg"
)

func TestNamesAreSafeWindowsLeaves(t *testing.T) {
	for _, name := range []string{
		"../a.exe", `C:\a.txt`, "aux.txt", "con ", "LPT1", "NUL", "...", "a:b", "中文\x00.txt",
	} {
		result := SanitizeName(name)
		for _, bad := range []string{"/", "\\", ":", "\x00"} {
			if contains(result, bad) {
				t.Fatalf("%q 净化后仍含 %q: %q", name, bad, result)
			}
		}
		if result == "" || result[len(result)-1] == ' ' || result[len(result)-1] == '.' {
			t.Fatalf("%q 净化结果异常: %q", name, result)
		}
	}
	if got := SanitizeName("中文.txt"); got != "中文.txt" {
		t.Fatalf("普通中文名应保留: %q", got)
	}
	if got := SanitizeName("aux.txt"); got != "_aux.txt" {
		t.Fatalf("保留名应加前缀: %q", got)
	}
}

func contains(s, sub string) bool {
	return len(sub) > 0 && len(s) >= len(sub) && indexOf(s, sub) >= 0
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

func TestLegacyOfferAndRequestFormats(t *testing.T) {
	extra := "10:中文::测试.txt:ff:0:1:\x07"
	wire, err := ipmsg.EncodePacket("1", 123, "peer", "host",
		ipmsg.CmdSendMsg|ipmsg.OptFileAttach, "文件", &extra)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := ipmsg.ParsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	offers, err := Offers(pkt)
	if err != nil {
		t.Fatal(err)
	}
	if offers[0].ID != 10 || offers[0].Size != 255 || offers[0].Name != "中文_测试.txt" {
		t.Fatalf("邀请解析异常: %+v", offers[0])
	}

	wire, err = ipmsg.EncodePacket("1", 9, "peer", "host", ipmsg.CmdGetFileData, "7b:a:ff:", nil)
	if err != nil {
		t.Fatal(err)
	}
	original, file, offset, err := Request(wire)
	if err != nil {
		t.Fatal(err)
	}
	if original != 123 || file != 10 || offset != 255 {
		t.Fatalf("TCP 请求解析异常: %d %d %d", original, file, offset)
	}

	for _, bad := range []string{
		"1:x:1:0:4:\x07",
		"1:x:ffffffffffffffff:0:1:\x07",
		"1:x:-1:0:1:\x07",
		"1:x:1:0:1:\x071:y:1:0:1:\x07",
	} {
		wire, err := ipmsg.EncodePacket("1", 1, "p", "h", ipmsg.CmdSendMsg|ipmsg.OptFileAttach, "", &bad)
		if err != nil {
			t.Fatal(err)
		}
		pkt, err := ipmsg.ParsePacket(wire)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := Offers(pkt); err == nil {
			t.Errorf("应拒绝 %q", bad)
		}
	}
}
