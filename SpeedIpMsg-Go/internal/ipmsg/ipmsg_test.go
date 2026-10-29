package ipmsg

import (
	"bytes"
	"strings"
	"testing"

	"golang.org/x/text/encoding/simplifiedchinese"
)

func TestPacketRoundTripGBKChinese(t *testing.T) {
	body := "你好:第一行\r\n第二行"
	wire, err := EncodePacket("1", 42, "用户", "主机", CmdSendMsg|OptSendCheck, body, nil)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := ParsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.PacketNo != 42 || pkt.Username != "用户" || pkt.Hostname != "主机" {
		t.Fatalf("头部不匹配: %+v", pkt)
	}
	if pkt.Body != body {
		t.Fatalf("body 不匹配: %q != %q", pkt.Body, body)
	}
	if bytes.Contains(wire, []byte("你好")) {
		t.Fatal("线上报文应为 GBK 编码，不应包含 UTF-8 中文")
	}
	if !bytes.HasSuffix(wire, []byte{0}) {
		t.Fatal("文本报文应以 NUL 结尾")
	}
}

func TestParseAcceptsFeiqExtendedVersion(t *testing.T) {
	data := []byte("1_lbt6_0#128#001122334455#0#0#0#4001#9:12:alice:host:33:1234")
	pkt, err := ParsePacket(data)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(pkt.Version, "1_lbt6_") {
		t.Fatalf("version = %q", pkt.Version)
	}
	if pkt.Command != CmdRecvMsg || pkt.Body != "1234" {
		t.Fatalf("pkt = %+v", pkt)
	}
}

func TestFeiqCompatVersionShapeAndStability(t *testing.T) {
	a := FeiqCompatVersion("mygo-demo@host", 2425, 0)
	b := FeiqCompatVersion("mygo-demo@host", 2425, 0)
	if a != b {
		t.Fatal("同实例版本头应稳定")
	}
	if FeiqCompatVersion("mygo-demo@host", 2426, 0) == a {
		t.Fatal("不同端口版本头应不同")
	}
	if !strings.HasPrefix(FeiqCompatVersion("mygo-demo@host", 2425, 83), "1_lbt6_83#") {
		t.Fatal("头像编号应嵌入版本头")
	}
	fields := strings.Split(a, "#")
	if len(fields) != 8 || fields[0] != "1_lbt6_0" || fields[1] != "128" {
		t.Fatalf("字段异常: %v", fields)
	}
	if len(fields[2]) != 12 || !strings.HasPrefix(fields[2], "02") {
		t.Fatalf("MAC 形状 ID 异常: %q", fields[2])
	}
}

func TestEntryBodyExtraFraming(t *testing.T) {
	group := "研发组"
	wire, err := EncodePacket("1", 9, "nick", "host", CmdBrEntry, "昵称", &group)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := ParsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.Body != "昵称" || pkt.Extra != "研发组" {
		t.Fatalf("body/extra = %q / %q", pkt.Body, pkt.Extra)
	}
	// body\0extra\0：报文以 extra 的 GBK 字节加单个 NUL 结尾
	if !bytes.HasSuffix(wire, append([]byte("\xd1\xd0\xb7\xa2\xd7\xe9"), 0)) {
		t.Fatalf("报文结尾异常: % x", wire[len(wire)-12:])
	}
}

func TestUTF8Option(t *testing.T) {
	body := "你好 🦀"
	wire, err := EncodePacket("1", 7, "u", "h", CmdSendMsg|OptUTF8, body, nil)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := ParsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.Body != body {
		t.Fatalf("UTF-8 body 不匹配: %q", pkt.Body)
	}
}

func TestDecodeGBKFromRealEncoder(t *testing.T) {
	// 用 GBK 编码器产出线上字节，确认解码还原
	gbkBytes, err := simplifiedchinese.GBK.NewEncoder().Bytes([]byte("中文测试"))
	if err != nil {
		t.Fatal(err)
	}
	// 头部命令不含 UTF8OPT 时按 GBK 解码
	raw := "1:1:u:h:32:" + string(gbkBytes) + "\x00"
	pkt, err := ParsePacket([]byte(raw))
	if err != nil {
		t.Fatal(err)
	}
	if pkt.Body != "中文测试" {
		t.Fatalf("GBK 解码结果: %q", pkt.Body)
	}
}

func TestMalformedPacketsRejected(t *testing.T) {
	for _, raw := range []string{
		"",
		"1:1:u:h:32",
		"2:1:u:h:32:x",
		"1:1:u:h:9999999999:x",
		"1:1::h:32:x",
		"1:1:u::32:x",
	} {
		if _, err := ParsePacket([]byte(raw)); err == nil {
			t.Errorf("应拒绝 %q", raw)
		}
	}
}
