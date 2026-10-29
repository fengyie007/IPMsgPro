package network

// 图片接收回环集成测试：A → B 的分片流，覆盖"分片先行"与"引用先行"两条路径。

import (
	"bytes"
	"image"
	"image/color"
	"image/png"
	"net"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/media"
	"my-app/internal/store"
)

func makeTestPNG(t *testing.T, width, height int, seed byte) []byte {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			img.SetNRGBA(x, y, color.NRGBA{seed, byte(x), byte(y), 255})
		}
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

// sendImageFragments 按 A 的视角把 LZW! 载荷切片发给 B（单播）。
func sendImageFragments(t *testing.T, a *Hub, bAddr *net.UDPAddr, imageID string, payload []byte) {
	t.Helper()
	const fragmentSize = 512
	count := (len(payload)-1)/fragmentSize + 1
	for index := 1; index <= count; index++ {
		offset := (index - 1) * fragmentSize
		end := offset + fragmentSize
		if end > len(payload) {
			end = len(payload)
		}
		header := formatImageHeader(imageID, len(payload), offset, count, index, end-offset)
		body := append([]byte(header), 0)
		body = append(body, payload[offset:end]...)
		wire, err := ipmsg.EncodeImagePacket(a.WireVersion(), 7777, "Alice", "hosta", body)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := a.conn.WriteToUDP(wire, bAddr); err != nil {
			t.Fatal(err)
		}
	}
}

func formatImageHeader(imageID string, total, offset, count, index, length int) string {
	// 布局对照 fragments.rs parse_fragment：id|total|offset|count|index|length|0|1|0|00000000#
	return imageID + "|" + itoa64(int64(total)) + "|" + itoa64(int64(offset)) + "|" +
		itoa64(int64(count)) + "|" + itoa64(int64(index)) + "|" + itoa64(int64(length)) + "|0|1|0|00000000#"
}

func itoa64(n int64) string {
	if n == 0 {
		return "0"
	}
	neg := n < 0
	if neg {
		n = -n
	}
	var buf [24]byte
	i := len(buf)
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	if neg {
		i--
		buf[i] = '-'
	}
	return string(buf[i:])
}

func TestLoopbackImageReceive(t *testing.T) {
	dir := t.TempDir()
	dbB, err := store.Open(filepath.Join(dir, "b.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dbB.Close()
	assetStore, err := media.NewAssetStore(filepath.Join(dir, "images"))
	if err != nil {
		t.Fatal(err)
	}

	a, err := NewHub(0, "Alice", "组A", "inst-a")
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	b, err := NewHub(0, "Bob", "组B", "inst-b")
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	b.AttachDatabase(dbB, "local@b")
	b.AttachAssets(assetStore)

	imageEvents := make(chan ReceivedEvent, 4)
	imageMeta := make(chan media.ImageMetadata, 4)
	b.ImageReceived = func(evt ReceivedEvent, metadata media.ImageMetadata) {
		imageEvents <- evt
		imageMeta <- metadata
	}
	a.Run()
	b.Run()

	// A 先以文本方式"认识" B：发一条 BR_ENTRY 让 B 回 ANSENTRY（B 由此认识 A）
	bAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: localPort(b)}
	if _, err := a.buildWireAndSend(bAddr, ipmsg.CmdBrEntry, "Alice", &a.prof.Load().group); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 2*time.Second, func() bool { return len(b.ListUsers()) == 1 })

	// 生成一张足够大的图（>512B 分片）
	pngBytes := makeTestPNG(t, 40, 40, 7)
	payload, err := media.EncodePNGForWire(pngBytes)
	if err != nil {
		t.Fatal(err)
	}
	if len(payload) <= 512 {
		t.Fatalf("测试载荷应跨多片: %d", len(payload))
	}

	// 引用先行：先发引用文本，再发分片
	reference := "/~#>abcdef01<B~{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}"
	referenceWire, err := ipmsg.EncodePacket(b.WireVersion(), 42, "Alice", "hosta", ipmsg.CmdSendMsg|ipmsg.OptSendCheck, reference, nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := a.conn.WriteToUDP(referenceWire, bAddr); err != nil {
		t.Fatal(err)
	}
	sendImageFragments(t, a, bAddr, "abcdef01", payload)

	select {
	case evt := <-imageEvents:
		meta := <-imageMeta
		if evt.Content != "[图片]" {
			t.Fatalf("图片消息内容异常: %q", evt.Content)
		}
		if meta.Width != 40 || meta.Height != 40 {
			t.Fatalf("尺寸异常: %dx%d", meta.Width, meta.Height)
		}
		// 资产已落盘且可读
		if _, err := assetStore.Read(meta.AssetID, true); err != nil {
			t.Fatalf("缩略图应可读: %v", err)
		}
		full, err := assetStore.Read(meta.AssetID, false)
		if err != nil {
			t.Fatalf("原图应可读: %v", err)
		}
		// 像素一致性：解码回 PNG 后尺寸一致
		decoded, err := media.DecodeImage(full)
		if err != nil || decoded.Width != 40 {
			t.Fatalf("原图解码异常: %v", err)
		}
		// 数据库只登记图片记录（kind 1）；纯图片引用不得落成一条
		// "/~#>id<B~" 的文本消息（对拍 Rust parse_reference 分支）。
		rows, err := dbB.History(evt.PeerID, "local@b", 10, 0)
		if err != nil || len(rows) != 1 {
			t.Fatalf("历史应只含图片记录: %+v %v", rows, err)
		}
		foundImage := false
		for _, row := range rows {
			if row.Kind == 0 || strings.Contains(row.Content, "/~#>") {
				t.Fatalf("图片引用不应落成文本消息: %+v", row)
			}
			if row.Kind == 1 && row.ID == evt.ID {
				foundImage = true
			}
		}
		if !foundImage {
			t.Fatalf("图片记录缺失: %+v", rows)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("等待图片事件超时")
	}

	// 重复分片重放：不应产生第二条图片消息
	sendImageFragments(t, a, bAddr, "abcdef01", payload)
	select {
	case <-imageEvents:
		t.Fatal("重复传输不应产生新事件")
	case <-time.After(500 * time.Millisecond):
	}
}

func TestImagePacketRoundTripBinarySafe(t *testing.T) {
	body := append([]byte("abcdef01|14|0|1|1|14|0|1|0|00000000#"), 0)
	body = append(body, 0xff, ':', 0x00, 0x81)
	wire, err := ipmsg.EncodeImagePacket("1_lbt6_0#128#001122334455#0#0#0#4001#9", 7, "本机", "主机", body)
	if err != nil {
		t.Fatal(err)
	}
	pkt, err := ipmsg.ParsePacket(wire)
	if err != nil {
		t.Fatal(err)
	}
	if pkt.Command != ipmsg.CmdSendImage|ipmsg.OptFileAttach {
		t.Fatalf("命令异常: %x", pkt.Command)
	}
	if !bytes.Equal([]byte(pkt.Body), body) {
		t.Fatal("二进制载荷应逐字节保留")
	}
}

// 纯图片引用（含飞秋字体尾标）不得产生文本消息或未读，只登记引用并回执；
// 曾把 "/~#>id<B~" 原样显示成聊天文本（对拍 Rust parse_reference 分支）。
func TestPureImageReferenceCreatesNoTextMessage(t *testing.T) {
	h, err := NewHub(0, "Bob", "组B", "inst-b")
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	var events collected
	events.wire(h)

	// 对端先"认识"：直接注入 BR_ENTRY
	entry, err := ipmsg.EncodePacket(h.WireVersion(), 1, "Alice", "hosta", ipmsg.CmdBrEntry, "Alice", nil)
	if err != nil {
		t.Fatal(err)
	}
	peerAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 45678}
	h.handle(entry, peerAddr)

	reference := "/~#>abcdef01<B~{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}"
	wire, err := ipmsg.EncodePacket(h.WireVersion(), 42, "Alice", "hosta", ipmsg.CmdSendMsg|ipmsg.OptSendCheck, reference, nil)
	if err != nil {
		t.Fatal(err)
	}
	h.handle(wire, peerAddr)
	time.Sleep(100 * time.Millisecond)

	if n := events.receivedCount(); n != 0 {
		events.mu.Lock()
		got := events.received
		events.mu.Unlock()
		t.Fatalf("纯引用不应产生文本消息，得到 %d 条: %+v", n, got)
	}
	if got := h.Unread(peerAddr.String()); got != 0 {
		t.Fatalf("纯引用不应累计未读: %d", got)
	}
}
