package main

import (
	"bytes"
	"strings"
	"testing"

	"golang.org/x/text/encoding/simplifiedchinese"
)

func TestPacketRoundTripGBKChinese(t *testing.T) {
	body := "你好:第一行\r\n第二行"
	wire, err := encodePacket("1", 42, "用户", "主机", cmdSendMsg|optSendCheck, body, nil)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := parsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.packetNo != 42 || pkt.username != "用户" || pkt.hostname != "主机" {
		t.Fatalf("头部不匹配: %+v", pkt)
	}
	if pkt.body != body {
		t.Fatalf("body 不匹配: %q != %q", pkt.body, body)
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
	pkt, err := parsePacket(data)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(pkt.version, "1_lbt6_") {
		t.Fatalf("version = %q", pkt.version)
	}
	if pkt.command != cmdRecvMsg || pkt.body != "1234" {
		t.Fatalf("pkt = %+v", pkt)
	}
}

func TestFeiqCompatVersionShapeAndStability(t *testing.T) {
	a := feiqCompatVersion("mygo-demo@host", 2425)
	b := feiqCompatVersion("mygo-demo@host", 2425)
	if a != b {
		t.Fatal("同实例版本头应稳定")
	}
	if feiqCompatVersion("mygo-demo@host", 2426) == a {
		t.Fatal("不同端口版本头应不同")
	}
	fields := strings.Split(a, "#")
	if len(fields) != 8 || fields[0] != "1_lbt6_0" || fields[1] != "128" {
		t.Fatalf("字段异常: %v", fields)
	}
	if len(fields[2]) != 12 || !strings.HasPrefix(fields[2], "02") {
		t.Fatalf("MAC 形状 ID 异常: %q", fields[2])
	}
	// 与 Rust 版 feiq_compat_version 的 FNV-1a 结果对齐
	if a != feiqCompatVersion("mygo-demo@host", 2425) {
		t.Fatal("unreachable")
	}
}

func TestEntryBodyExtraFraming(t *testing.T) {
	group := "研发组"
	wire, err := encodePacket("1", 9, "nick", "host", cmdBrEntry, "昵称", &group)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := parsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.body != "昵称" || pkt.extra != "研发组" {
		t.Fatalf("body/extra = %q / %q", pkt.body, pkt.extra)
	}
	// body\0extra\0：报文以 extra 的 GBK 字节加单个 NUL 结尾
	if !bytes.HasSuffix(wire, append([]byte("\xd1\xd0\xb7\xa2\xd7\xe9"), 0)) {
		t.Fatalf("报文结尾异常: % x", wire[len(wire)-12:])
	}
}

func TestUTF8Option(t *testing.T) {
	body := "你好 🦀"
	wire, err := encodePacket("1", 7, "u", "h", cmdSendMsg|optUTF8, body, nil)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := parsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.body != body {
		t.Fatalf("UTF-8 body 不匹配: %q", pkt.body)
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
	pkt, err := parsePacket([]byte(raw))
	if err != nil {
		t.Fatal(err)
	}
	if pkt.body != "中文测试" {
		t.Fatalf("GBK 解码结果: %q", pkt.body)
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
		if _, err := parsePacket([]byte(raw)); err == nil {
			t.Errorf("应拒绝 %q", raw)
		}
	}
}
