package network

// deadline_test.go：扫描给共享 UDP 连接设了 1 秒写超时，用完必须还回去，
// 否则之后所有发送都撞在已过期的 deadline 上，报 "write udp4 ... i/o timeout"。

import (
	"net"
	"testing"
	"time"

	"speedipmsg-go/internal/store"
)

func TestScanRestoresWriteDeadline(t *testing.T) {
	a := newScanHub(t, "Alice")
	port := localPort(a)
	if _, err := a.StartScan(store.ScanOptions{
		Ranges: []string{"127.0.0.1"}, Port: uint16(port), DelayMs: 10,
	}, false); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 5*time.Second, func() bool {
		return !a.ScanStatus().State.active()
	})
	// 越过扫描设的 1 秒写超时。
	time.Sleep(1500 * time.Millisecond)

	peer := newScanHub(t, "Bob")
	defer peer.Close()
	target := new(net.UDPAddr)
	target.IP = net.IPv4(127, 0, 0, 1)
	target.Port = localPort(peer)
	wire, err := a.buildWire(0x01, 42, "hello", nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := a.sendWire(target, wire); err != nil {
		t.Fatalf("扫描后发送失败（写超时未清理）: %v", err)
	}
}
