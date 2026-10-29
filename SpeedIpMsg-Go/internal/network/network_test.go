package network

// 回环集成测试：发现 → 文本 → 回执 → 落库 → 去重，覆盖带库与不带库两种形态。

import (
	"net"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"speedipmsg-go/internal/ipmsg"
	"speedipmsg-go/internal/store"
)

func localPort(h *Hub) int {
	return h.conn.LocalAddr().(*net.UDPAddr).Port
}

func waitUntil(t *testing.T, timeout time.Duration, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("等待条件超时")
}

type collected struct {
	mu       sync.Mutex
	received []ReceivedEvent
	acked    []string
	failed   []string
}

func (c *collected) wire(h *Hub) {
	h.MessageReceived = func(evt ReceivedEvent) {
		c.mu.Lock()
		c.received = append(c.received, evt)
		c.mu.Unlock()
	}
	h.MessageAcked = func(_, messageID string) {
		c.mu.Lock()
		c.acked = append(c.acked, messageID)
		c.mu.Unlock()
	}
	h.MessageFailed = func(_, messageID, _ string) {
		c.mu.Lock()
		c.failed = append(c.failed, messageID)
		c.mu.Unlock()
	}
}

func (c *collected) receivedCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.received)
}

func TestLoopbackDiscoveryMessagingPersistence(t *testing.T) {
	dir := t.TempDir()
	dbA, err := store.Open(filepath.Join(dir, "a.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dbA.Close()
	dbB, err := store.Open(filepath.Join(dir, "b.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dbB.Close()

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
	var eventsA, eventsB collected
	eventsA.wire(a)
	eventsB.wire(b)
	a.AttachDatabase(dbA, "local@a")
	b.AttachDatabase(dbB, "local@b")
	a.Run()
	b.Run()

	// A 直接向 B 单播 BR_ENTRY，B 回 ANSENTRY 让 A 认识 Bob
	bAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: localPort(b)}
	if _, err := a.buildWireAndSend(bAddr, ipmsg.CmdBrEntry, a.Nick(), &a.prof.Load().group); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 2*time.Second, func() bool {
		for _, u := range a.ListUsers() {
			if u.Nick == "Bob" && u.Online {
				return true
			}
		}
		return false
	})

	var bobID string
	for _, u := range a.ListUsers() {
		if u.Nick == "Bob" {
			bobID = u.ID
		}
	}
	if bobID == "" {
		t.Fatal("未取得 Bob 的联系人 ID")
	}
	messageID, err := a.SendText(bobID, "你好Bob")
	if err != nil {
		t.Fatal(err)
	}
	// B 收到消息（未读+1），A 收到回执
	waitUntil(t, 2*time.Second, func() bool { return eventsB.receivedCount() == 1 })
	if got := b.Unread("127.0.0.1:" + itoa(localPort(a))); got != 1 {
		t.Fatalf("B 未读 = %d, want 1", got)
	}
	waitUntil(t, 2*time.Second, func() bool { return len(eventsA.acked) == 1 })
	if eventsA.acked[0] != messageID {
		t.Fatalf("回执消息 ID 不匹配: %s != %s", eventsA.acked[0], messageID)
	}
	// A 的库：已发送且已送达；B 的库：已接收
	histA, err := dbA.History(bobID, "local@a", 10, 0)
	if err != nil || len(histA) != 1 || histA[0].Status != 1 {
		t.Fatalf("A 库应为已送达: %+v %v", histA, err)
	}
	eventsB.mu.Lock()
	peerID := eventsB.received[0].PeerID
	eventsB.mu.Unlock()
	histB, err := dbB.History(peerID, "local@b", 10, 0)
	if err != nil || len(histB) != 1 || histB[0].Content != "你好Bob" || histB[0].Kind != 0 {
		t.Fatalf("B 库接收记录异常: %+v %v", histB, err)
	}

	// B 反向发送
	waitUntil(t, 2*time.Second, func() bool {
		for _, u := range b.ListUsers() {
			if u.Nick == "Alice" {
				return true
			}
		}
		return false
	})
	var aliceID string
	for _, u := range b.ListUsers() {
		if u.Nick == "Alice" {
			aliceID = u.ID
		}
	}
	if _, err := b.SendText(aliceID, "你好Alice"); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 2*time.Second, func() bool { return eventsA.receivedCount() == 1 })

	// 去重：同一条线上报文重放给 B，事件 Duplicate=true 且未读不再增加
	var command uint32 = ipmsg.CmdSendMsg | ipmsg.OptSendCheck
	wire, err := ipmsg.EncodePacket("1_lbt6_0#128#001122334455#0#0#0#4001#9", 424242, "Carol", "carolhost", command, "重复消息", nil)
	if err != nil {
		t.Fatal(err)
	}
	carolAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 55555}
	b.handle(wire, carolAddr)
	b.handle(wire, carolAddr)
	time.Sleep(100 * time.Millisecond)
	eventsB.mu.Lock()
	dupCount := 0
	last := eventsB.received[len(eventsB.received)-1]
	for _, evt := range eventsB.received {
		if evt.ID == last.ID {
			dupCount++
		}
	}
	eventsB.mu.Unlock()
	if dupCount != 2 || !last.Duplicate {
		t.Fatalf("重放应产生两次事件且第二次为 duplicate: %d %v", dupCount, last.Duplicate)
	}
	if got := b.Unread("127.0.0.1:55555"); got != 1 {
		t.Fatalf("重复报文不应累计未读: %d", got)
	}

	// MarkRead 清零
	if n := a.MarkRead("127.0.0.1:" + itoa(localPort(b))); n != 1 {
		t.Fatalf("MarkRead 返回 %d, want 1", n)
	}
	if a.TotalUnread() != 0 {
		t.Fatal("已读后未读应为 0")
	}
}

func TestSetProfileUpdatesWireVersion(t *testing.T) {
	h, err := NewHub(0, "Alice", "组A", "inst")
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	if !strings.HasPrefix(h.WireVersion(), "1_lbt6_0#128#") {
		t.Fatalf("初始版本头异常: %s", h.WireVersion())
	}
	h.SetProfile("Alice2", "新组", 83)
	if h.Nick() != "Alice2" || h.Group() != "新组" {
		t.Fatal("资料热更新未生效")
	}
	if want := ipmsg.FeiqCompatVersion("inst", h.Port(), 83); h.WireVersion() != want {
		t.Fatalf("版本头应随头像刷新: %s != %s", h.WireVersion(), want)
	}
}

// 改名只改 body 里的昵称，协议 username 字段（对端建联系人的键）必须保持稳定，
// 否则对端会把同一个人当成两个。对拍 Rust：username=稳定登录名，昵称在 body。
func TestRenameKeepsWireIdentity(t *testing.T) {
	h, err := NewHub(0, "旧名", "组A", "inst")
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	if h.Login() == "" || h.Login() == "旧名" {
		t.Fatalf("登录名应为稳定账户名而非昵称: %q", h.Login())
	}

	before, err := h.buildWire(ipmsg.CmdBrEntry, 1, h.Nick(), &h.prof.Load().group)
	if err != nil {
		t.Fatal(err)
	}
	h.SetProfile("新名", "组A", 0)
	after, err := h.buildWire(ipmsg.CmdBrEntry, 2, h.Nick(), &h.prof.Load().group)
	if err != nil {
		t.Fatal(err)
	}

	parse := func(wire []byte) ipmsg.Packet {
		pkt, err := ipmsg.ParsePacket(wire)
		if err != nil {
			t.Fatal(err)
		}
		return pkt
	}
	oldPkt, newPkt := parse(before), parse(after)
	if oldPkt.Username != newPkt.Username {
		t.Fatalf("改名后 username 变了（对端会当成两个人）: %q -> %q", oldPkt.Username, newPkt.Username)
	}
	if newPkt.Body != "新名" {
		t.Fatalf("昵称应随改名更新到 body: %q", newPkt.Body)
	}
	if oldPkt.Body != "旧名" {
		t.Fatalf("改名前 body 应为旧昵称: %q", oldPkt.Body)
	}
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var buf [6]byte
	i := len(buf)
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	return string(buf[i:])
}
