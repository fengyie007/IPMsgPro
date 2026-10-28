package network

// 回环集成测试：两个 Hub 在 127.0.0.1 临时端口上完成 发现 → 文本 → 回执 的最小闭环。

import (
	"net"
	"testing"
	"time"

	"my-app/internal/ipmsg"
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

func TestLoopbackDiscoveryAndMessaging(t *testing.T) {
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
	a.Run()
	b.Run()

	// A 直接向 B 单播 BR_ENTRY（测试里不走广播），B 应回 ANSENTRY 让 A 认识 Bob
	bAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: localPort(b)}
	if _, err := a.writePacket(bAddr, ipmsg.CmdBrEntry, a.nick, &a.group); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 2*time.Second, func() bool {
		users := a.ListUsers()
		return len(users) == 1 && users[0].Nick == "Bob" && users[0].Online
	})

	// 同一联系人在两侧视角键不同：A 眼里是 keyB（B 的地址），B 眼里是 keyA（A 的地址）
	keyB := "127.0.0.1:" + itoa(localPort(b))
	keyA := "127.0.0.1:" + itoa(localPort(a))
	if err := a.SendText(keyB, "你好Bob"); err != nil {
		t.Fatal(err)
	}
	// B 收到消息且未读+1，并自动回复 RECVMSG
	waitUntil(t, 2*time.Second, func() bool {
		hist := b.History(keyA)
		return len(hist) == 1 && hist[0].Text == "你好Bob" && !hist[0].Out
	})
	if got := b.Unread(keyA); got != 1 {
		t.Fatalf("B 未读 = %d, want 1", got)
	}
	waitUntil(t, 2*time.Second, func() bool {
		hist := a.History(keyB)
		return len(hist) == 1 && hist[0].Acked
	})

	// B 也应通过 BR_ENTRY 认识 Alice，可以反向发送
	waitUntil(t, 2*time.Second, func() bool {
		users := b.ListUsers()
		return len(users) == 1 && users[0].Nick == "Alice"
	})
	if err := b.SendText(keyA, "你好Alice"); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 2*time.Second, func() bool {
		hist := a.History(keyB)
		return len(hist) == 2 && hist[1].Text == "你好Alice" && !hist[1].Out
	})
	// A 收到 B 的消息后未读+1
	if got := a.Unread(keyB); got != 1 {
		t.Fatalf("A 未读 = %d, want 1", got)
	}
	// 标记已读后清零
	if n := a.MarkRead(keyB); n != 1 {
		t.Fatalf("MarkRead 返回 %d, want 1", n)
	}
	if a.Unread(keyB) != 0 || a.TotalUnread() != 0 {
		t.Fatal("已读后未读应为 0")
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
