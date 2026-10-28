package network

// 对拍 tauri-rust knock 行为：回环发送/接收、冷却、无效报文拒绝。

import (
	"net"
	"path/filepath"
	"testing"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/store"
)

func knockHubs(t *testing.T) (*Hub, *Hub, *collected) {
	t.Helper()
	dir := t.TempDir()
	dbA, err := store.Open(filepath.Join(dir, "a.db"))
	if err != nil {
		t.Fatal(err)
	}
	dbB, err := store.Open(filepath.Join(dir, "b.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { dbA.Close(); dbB.Close() })
	a := newScanHub(t, "Alice")
	b := newScanHub(t, "Bob")
	a.AttachDatabase(dbA, "local@a")
	b.AttachDatabase(dbB, "local@b")
	events := &collected{}
	events.wire(b)
	return a, b, events
}

func TestKnockLoopbackAndCooldown(t *testing.T) {
	a, b, events := knockHubs(t)
	bAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: localPort(b)}
	var bobID string
	waitUntil(t, 2*time.Second, func() bool {
		_, _ = a.buildWireAndSend(bAddr, ipmsg.CmdBrEntry, a.Nick(), &a.prof.Load().group)
		for _, u := range a.ListUsers() {
			if u.Nick == "Bob" {
				bobID = u.ID
				return true
			}
		}
		return false
	})
	record, err := a.SendKnock(bobID)
	if err != nil {
		t.Fatal(err)
	}
	if record.Status != 1 || record.Kind != 3 {
		t.Fatalf("振动记录异常: %+v", record)
	}
	waitUntil(t, 2*time.Second, func() bool { return events.receivedCount() > 0 })
	events.mu.Lock()
	evt := events.received[0]
	events.mu.Unlock()
	if evt.Content != "[窗口振动]" || evt.Type != "knock" {
		t.Fatalf("接收事件异常: %+v", evt)
	}
	if _, err := a.SendKnock(bobID); err == nil {
		t.Fatalf("10 秒冷却内第二次应被拒绝")
	}
}

func TestKnockReceiveRejectsMalformed(t *testing.T) {
	a, _, _ := knockHubs(t)
	_ = a
	src := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 11111}
	// 空帧合法；带内容的帧必须被拒绝。
	if err := a.receiveKnock("127.0.0.1:11111", ipmsg.Packet{Command: ipmsg.CmdKnock}, src); err != nil {
		t.Fatalf("空帧应接受: %v", err)
	}
	if err := a.receiveKnock("127.0.0.1:11112", ipmsg.Packet{Command: ipmsg.CmdKnock, Body: "x"}, src); err == nil {
		t.Fatalf("带内容的帧应拒绝")
	}
	if err := a.receiveKnock("127.0.0.1:11113", ipmsg.Packet{Command: ipmsg.CmdKnock, Extra: "1"}, src); err == nil {
		t.Fatalf("带 extra 的帧应拒绝")
	}
}
