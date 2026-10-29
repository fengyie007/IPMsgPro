package network

// scan_test.go 对拍 tauri-rust 网络扫描行为：范围解析已由 store 层用例覆盖，
// 这里验证 Hub 集成——扫描发现对端、限频进度事件、取消与互斥。

import (
	"sync"
	"testing"
	"time"

	"my-app/internal/store"
)

type scanLog struct {
	mu    sync.Mutex
	names []string
	last  ScanStatus
}

func (l *scanLog) record(event string, payload ScanStatus) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.names = append(l.names, event)
	l.last = payload
}

func (l *scanLog) events() []string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return append([]string(nil), l.names...)
}

func newScanHub(t *testing.T, name string) *Hub {
	t.Helper()
	h, err := NewHub(0, name, "", "inst-"+name)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(h.Close)
	h.Run()
	return h
}

func TestScanDiscoversPeerAndCompletes(t *testing.T) {
	a := newScanHub(t, "Alice")
	b := newScanHub(t, "Bob")
	port := localPort(b)
	log := &scanLog{}
	a.ScanEvent = log.record

	status, err := a.StartScan(store.ScanOptions{
		Ranges:  []string{"127.0.0.1-127.0.0.3"},
		Port:    uint16(port),
		DelayMs: 10,
	}, false)
	if err != nil {
		t.Fatal(err)
	}
	if status.State != "running" || status.Total != 3 {
		t.Fatalf("初始状态异常: %+v", status)
	}
	waitUntil(t, 10*time.Second, func() bool {
		s := a.ScanStatus()
		return s.State == "completed" || s.State == "failed"
	})
	final := a.ScanStatus()
	if final.State != "completed" {
		t.Fatalf("扫描应完成: %+v", final)
	}
	if final.Found != 1 || final.FailedSends != 0 {
		t.Fatalf("应发现 1 个对端: %+v", final)
	}
	if final.Current != 3 || final.Skipped != 0 {
		t.Fatalf("探测计数异常: %+v", final)
	}
	for _, name := range log.events() {
		if name != "network.scan_progress" && name != "network.scan_complete" {
			t.Fatalf("未知事件: %s", name)
		}
	}
}

func TestScanCancelIsMutex(t *testing.T) {
	a := newScanHub(t, "Alice")
	port := localPort(a)

	status, err := a.StartScan(store.ScanOptions{
		Ranges:  []string{"127.0.0.1-127.0.0.9"},
		Port:    uint16(port),
		DelayMs: 200,
	}, false)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := a.StartScan(store.ScanOptions{
		Ranges: []string{"127.0.0.1"}, Port: uint16(port), DelayMs: 10,
	}, false); err == nil {
		t.Fatalf("并发扫描应被拒绝")
	}
	if skipped, err := a.StartScan(store.ScanOptions{
		Ranges: []string{"127.0.0.1"}, Port: uint16(port), DelayMs: 10,
	}, true); err != nil || skipped.ScanID != status.ScanID {
		t.Fatalf("skipExisting 应返回现状: %+v %v", skipped, err)
	}
	cancelled, err := a.CancelScan(status.ScanID)
	if err != nil {
		t.Fatal(err)
	}
	if cancelled.State != "cancelled" {
		t.Fatalf("取消状态异常: %+v", cancelled)
	}
	waitUntil(t, 3*time.Second, func() bool {
		return a.ScanStatus().State == "cancelled"
	})
	if _, err := a.CancelScan(99999); err == nil {
		t.Fatalf("取消不存在的扫描应报错")
	}
}

func TestScanInvalidRangeIsRejected(t *testing.T) {
	a := newScanHub(t, "Alice")
	if _, err := a.StartScan(store.ScanOptions{
		Ranges: []string{"224.0.0.1"}, Port: 2425, DelayMs: 10,
	}, false); err == nil {
		t.Fatalf("组播地址应被拒绝")
	}
	if _, err := a.StartScan(store.ScanOptions{
		Ranges: []string{}, Port: 2425, DelayMs: 10,
	}, false); err == nil {
		t.Fatalf("空范围应被拒绝")
	}
}

// 扫描不再跳过本机：本机地址照常探测，自身报文由接收侧 isSelf 丢弃。
func TestScanProbesOwnAddress(t *testing.T) {
	a := newScanHub(t, "Alice")
	port := localPort(a)
	status, err := a.StartScan(store.ScanOptions{
		Ranges: []string{"127.0.0.1"}, Port: uint16(port), DelayMs: 10,
	}, false)
	if err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 5*time.Second, func() bool {
		s := a.ScanStatus()
		return s.State == "completed" || s.State == "failed"
	})
	final := a.ScanStatus()
	if final.ScanID != status.ScanID {
		t.Fatalf("scanId 应保持: %+v", final)
	}
	if final.Current != 1 || final.Skipped != 0 || final.FailedSends != 0 {
		t.Fatalf("本机应正常探测: %+v", final)
	}
	if final.State != "completed" {
		t.Fatalf("扫描应完成: %+v", final)
	}
	if n := len(a.ListUsers()); n != 0 {
		t.Fatalf("本机回环报文不应变成联系人，实际 %d 个", n)
	}
}
