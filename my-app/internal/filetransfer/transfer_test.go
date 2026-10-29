package filetransfer

// 回环集成测试：两套 Transfers 引擎通过真实 UDP 路由 + 真实 TCP 走完
// 发送邀请 → 回执 → 接受 → 下载 → 发布 的完整链路，对拍 tauri-rust 的
// file/transfer.rs + transfer/streams.rs 行为。

import (
	"bytes"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/store"
)

type eventLog struct {
	mu     sync.Mutex
	events []event
}

type event struct {
	name    string
	payload map[string]any
}

func (l *eventLog) record(name string, payload any) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.events = append(l.events, event{name, payload.(map[string]any)})
}

func (l *eventLog) last(name string) (map[string]any, bool) {
	l.mu.Lock()
	defer l.mu.Unlock()
	for i := len(l.events) - 1; i >= 0; i-- {
		if l.events[i].name == name {
			return l.events[i].payload, true
		}
	}
	return nil, false
}

func (l *eventLog) waitEvent(t *testing.T, name string, timeout time.Duration) map[string]any {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if payload, ok := l.last(name); ok {
			return payload
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("等待事件 %s 超时", name)
	return nil
}

// fakeLink 把 UDP 发送直接路由给对端引擎（真实编码/解析，网络层由 TCP 实测）。
type fakeLink struct {
	mu      sync.Mutex
	counter uint32
	local   Peer
	output  func(pkt ipmsg.Packet, from Peer)
}

func (l *fakeLink) NextPacketNo() uint32 {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.counter == 0 {
		l.counter = uint32(time.Now().UnixNano()) & 0x3fffffff
	}
	l.counter++
	return l.counter
}

func (l *fakeLink) currentPacket() uint32 {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.counter
}

func (l *fakeLink) BuildWire(pktNo uint32, command uint32, body string, extra *string) ([]byte, error) {
	return ipmsg.EncodePacket("1", pktNo, l.local.Nick, l.local.Host, command, body, extra)
}

func (l *fakeLink) SendUDP(_ *net.UDPAddr, wire []byte) error {
	pkt, err := ipmsg.ParsePacket(wire)
	if err != nil {
		return err
	}
	l.output(pkt, l.local)
	return nil
}

func (l *fakeLink) LocalID() string { return l.local.ID }

type enginePair struct {
	a, b     *Transfers
	dbA, dbB *store.Database
	linkA    *fakeLink
	linkB    *fakeLink
	logA     *eventLog
	logB     *eventLog
	peerA    Peer // A 的身份（B 眼中的对端）
	peerB    Peer // B 的身份（A 眼中的对端）
	rootA    string
	rootB    string
}

func freePort(t *testing.T) int {
	t.Helper()
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port
}

// routeIn 把 UDP 包送达对端引擎：文件邀请走 Incoming，回执/拒收走 Receipt。
func routeIn(target *Transfers, peer Peer) func(pkt ipmsg.Packet, from Peer) {
	addr := &net.UDPAddr{IP: net.ParseIP(peer.Addr), Port: peer.Port}
	return func(pkt ipmsg.Packet, from Peer) {
		mode := ipmsg.CommandMode(pkt.Command)
		switch {
		case mode == ipmsg.CmdSendMsg && pkt.Command&ipmsg.OptFileAttach != 0:
			_ = target.Incoming(peer, pkt)
		case mode == ipmsg.CmdRecvMsg || mode == ipmsg.CmdReleaseFiles:
			target.Receipt(pkt, addr)
		}
	}
}

func newEnginePair(t *testing.T, localNick string) *enginePair {
	t.Helper()
	tempA := t.TempDir()
	tempB := t.TempDir()
	portA, portB := freePort(t), freePort(t)
	p := &enginePair{
		rootA: tempA,
		rootB: tempB,
		peerA: Peer{ID: fmt.Sprintf("alice@h1#127.0.0.1:%d", portA), Nick: "alice", UserName: "alice", Host: "h1",
			Addr: "127.0.0.1", Port: portA, Online: true, Key: "alice-key"},
		peerB: Peer{ID: fmt.Sprintf("%s@h2#127.0.0.1:%d", localNick, portB), Nick: localNick, UserName: localNick, Host: "h2",
			Addr: "127.0.0.1", Port: portB, Online: true, Key: "bob-key"},
	}
	var err error
	dbA, err := store.Open(filepath.Join(tempA, "test.db"))
	if err != nil {
		t.Fatal(err)
	}
	dbB, err := store.Open(filepath.Join(tempB, "test.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { dbA.Close(); dbB.Close() })
	p.dbA, p.dbB = dbA, dbB
	p.logA, p.logB = &eventLog{}, &eventLog{}
	p.linkA = &fakeLink{local: p.peerA}
	p.linkB = &fakeLink{local: p.peerB}

	p.a, err = Start(p.linkA, dbA, func(name string, payload any) { p.logA.record(name, payload) }, tempA, uint16(portA))
	if err != nil {
		t.Fatal(err)
	}
	p.b, err = Start(p.linkB, dbB, func(name string, payload any) { p.logB.record(name, payload) }, tempB, uint16(portB))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { p.a.Shutdown(); p.b.Shutdown() })

	p.linkA.output = routeIn(p.b, p.peerA)
	p.linkB.output = routeIn(p.a, p.peerB)
	return p
}

func waitFor(t *testing.T, what string, timeout time.Duration, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("等待 %s 超时", what)
}

func (p *enginePair) transferState(e *Transfers, id string) store.FileMetadata {
	for _, tr := range e.list() {
		if tr.id == id {
			return tr.metadata()
		}
	}
	return store.FileMetadata{}
}

func randomBytes(n int) []byte {
	data := make([]byte, n)
	rand.Read(data)
	return data
}

func sendFile(t *testing.T, p *enginePair, name string, content []byte) (txID, offerID string) {
	t.Helper()
	src := filepath.Join(p.rootA, name)
	if err := os.WriteFile(src, content, 0o666); err != nil {
		t.Fatal(err)
	}
	selection, err := p.a.SelectPath(src)
	if err != nil {
		t.Fatal(err)
	}
	record, err := p.a.Send(p.peerB, selection.SelectionID)
	if err != nil {
		t.Fatal(err)
	}
	offer := p.logB.waitEvent(t, "message.received", 5*time.Second)
	fileMeta := offer["file"].(store.FileMetadata)
	if fileMeta.FileName != name || !fileMeta.Incoming {
		t.Fatalf("邀请元数据异常: %+v", fileMeta)
	}
	return record.ID, offer["id"].(string)
}

func TestFileTransferLoopback(t *testing.T) {
	p := newEnginePair(t, "bob")
	content := randomBytes(1024 * 700) // < 4 MiB，走单连接路径
	txID, offerID := sendFile(t, p, "报告.txt", content)

	if err := p.b.Accept(offerID); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "B 完成", 10*time.Second, func() bool {
		return p.transferState(p.b, offerID).State == "completed"
	})
	waitFor(t, "A 完成", 10*time.Second, func() bool {
		return p.transferState(p.a, txID).State == "completed"
	})

	received := p.transferState(p.b, offerID)
	if !received.HasLocalFile || received.Transferred != uint64(len(content)) {
		t.Fatalf("B 元数据异常: %+v", received)
	}
	path, err := p.dbB.FilePath(offerID)
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, content) {
		t.Fatalf("接收内容不一致: got %d bytes want %d", len(data), len(content))
	}
	// 发布后分片清理，接收根只留正式文件与数据库（含 WAL 伴生文件）。
	entries, _ := os.ReadDir(p.rootB)
	for _, e := range entries {
		if e.Name() == "报告.txt" || e.Name() == "test.db" ||
			e.Name() == "test.db-wal" || e.Name() == "test.db-shm" {
			continue
		}
		t.Fatalf("接收根目录残留: %s", e.Name())
	}
}

func TestFileTransferEmptyFile(t *testing.T) {
	p := newEnginePair(t, "bob")
	txID, offerID := sendFile(t, p, "空文件.dat", nil)
	if err := p.b.Accept(offerID); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "B 完成", 10*time.Second, func() bool {
		return p.transferState(p.b, offerID).State == "completed"
	})
	waitFor(t, "A 完成", 10*time.Second, func() bool {
		return p.transferState(p.a, txID).State == "completed"
	})
}

func TestFileTransferParallelChunks(t *testing.T) {
	p := newEnginePair(t, "bob")
	content := randomBytes(5 * 1024 * 1024) // ≥ 4 MiB，触发并行分块
	txID, offerID := sendFile(t, p, "大文件.bin", content)
	if err := p.b.Accept(offerID); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "B 完成", 30*time.Second, func() bool {
		return p.transferState(p.b, offerID).State == "completed"
	})
	waitFor(t, "A 完成", 10*time.Second, func() bool {
		return p.transferState(p.a, txID).State == "completed"
	})
	path, err := p.dbB.FilePath(offerID)
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, content) {
		t.Fatalf("并行接收内容不一致: got %d bytes want %d", len(data), len(content))
	}
}

func TestFileTransferResumeAfterRestart(t *testing.T) {
	p := newEnginePair(t, "bob")
	content := randomBytes(128 * 1024)
	src := filepath.Join(p.rootA, "续传.bin")
	if err := os.WriteFile(src, content, 0o666); err != nil {
		t.Fatal(err)
	}

	// A 侧用确定包号登记发送任务：SelectPath 耗 776，Send 耗 777。
	p.linkA.mu.Lock()
	p.linkA.counter = 775
	p.linkA.mu.Unlock()
	selection, err := p.a.SelectPath(src)
	if err != nil {
		t.Fatal(err)
	}
	record, err := p.a.Send(p.peerB, selection.SelectionID)
	if err != nil {
		t.Fatal(err)
	}
	_ = record

	// B 侧模拟崩溃残留：一半内容的分片 + 续传检查点 + 可续传元数据。
	// Send 的邀请路由到 B 时因去重不会重复入库。
	packetNo := p.linkA.currentPacket()
	offerID := fmt.Sprintf("%s:%d:file:%d", p.peerA.ID, packetNo, 1)
	pf, file, err := CreatePartial(p.rootB, 1)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.Write(content[:len(content)/2]); err != nil {
		t.Fatal(err)
	}
	file.Close()
	pf.Keep()
	ck, err := json.Marshal(checkpoint{Peer: p.peerA, Packet: packetNo, FileID: 1, Part: pf.Name()})
	if err != nil {
		t.Fatal(err)
	}
	half := store.FileMetadata{
		FileName:    "续传.bin",
		FileSize:    uint64(len(content)),
		State:       "paused",
		Incoming:    true,
		CanResume:   true,
		Transferred: uint64(len(content) / 2),
	}
	record2 := store.Record{
		ID: offerID, FromID: p.peerA.ID, ToID: p.peerB.ID, Content: "[文件] 续传.bin",
		Kind: 2, Timestamp: time.Now().Unix(), Status: 3, File: &half,
	}
	if _, err := p.dbB.Insert(record2); err != nil {
		t.Fatal(err)
	}
	if err := p.dbB.SaveReceive(offerID, string(ck)); err != nil {
		t.Fatal(err)
	}

	// 接受 → 从断点续传到完成。
	if err := p.b.Accept(offerID); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "B 续传完成", 15*time.Second, func() bool {
		return p.transferState(p.b, offerID).State == "completed"
	})
	waitFor(t, "A 完成", 10*time.Second, func() bool {
		return p.transferState(p.a, record.ID).State == "completed"
	})
	path, err := p.dbB.FilePath(offerID)
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, content) {
		t.Fatalf("续传内容不一致: got %d bytes want %d", len(data), len(content))
	}
}

func TestFileTransferRejectNotifiesSender(t *testing.T) {
	p := newEnginePair(t, "bob")
	txID, offerID := sendFile(t, p, "被拒.bin", []byte("x"))

	ok, err := p.b.Cancel(offerID, true)
	if err != nil || !ok {
		t.Fatalf("拒绝失败: %v %v", ok, err)
	}
	waitFor(t, "A 收到拒收", 5*time.Second, func() bool {
		return p.transferState(p.a, txID).State == "cancelled"
	})
	if meta := p.transferState(p.b, offerID); meta.State != "rejected" {
		t.Fatalf("B 状态异常: %+v", meta)
	}
}
