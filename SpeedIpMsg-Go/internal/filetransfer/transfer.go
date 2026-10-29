// transfer.go 对应 tauri-rust file/transfer.rs 与 transfer/streams.rs：
// TCP 文件传输引擎——邀请登记、发送（服务端）、接收（含并行分块与断点续传）、
// 暂停/取消/拒绝、定时重发与过期清理。
//
// 与 Rust 版的结构差异：
//   - tokio 任务换成 goroutine；watch 取消通道换成原子标志 + 可重置的
//     cancelled channel；Semaphore 换成带缓冲 channel；RAII Busy 换成显式
//     defer busy.Add(-1)。
//   - 每次 IO 的 30 秒超时用连接 deadline 表达，取消时由看门狗关闭连接
//     解除阻塞（Rust 是 select 竞争）。
package filetransfer

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"speedipmsg-go/internal/ipmsg"
	"speedipmsg-go/internal/store"
)

// Peer 是文件任务里对端的最小视图（JSON 字段与前端 User 对齐，
// message.received/file.updated 事件直接序列化）。
type Peer struct {
	ID       string `json:"id"`
	Key      string `json:"key"`
	Nick     string `json:"nickname"`
	UserName string `json:"username"` // 协议 user 字段，来源校验用它（昵称可改，登录名是身份）
	Host     string `json:"hostname"`
	Group    string `json:"group"`
	Addr     string `json:"ip"`
	Port     int    `json:"port"`
	Online   bool   `json:"online"`
}

// Link 是 Hub 提供的 UDP/包号能力，避免与 network 包互相依赖。
type Link interface {
	NextPacketNo() uint32
	BuildWire(pktNo uint32, command uint32, body string, extra *string) ([]byte, error)
	SendUDP(addr *net.UDPAddr, wire []byte) error
	LocalID() string
}

// EmitFunc 输出后端事件（file.updated / message.received）。
type EmitFunc func(event string, payload any)

type Selection struct {
	SelectionID string `json:"selectionId"`
	FileName    string `json:"fileName"`
	FileSize    uint64 `json:"fileSize"`
	IsDirectory bool   `json:"isDirectory"`
}

type source struct {
	file      *os.File
	folder    *Snapshot
	modified  time.Time
	selection Selection
	created   time.Time
}

type transferState struct {
	meta         store.FileMetadata
	acknowledged bool
	attempts     int
	retry        time.Time
	lastActivity time.Time
}

type Transfer struct {
	id     string
	peer   Peer
	packet uint32
	fileID uint32
	source *source // 发送任务持有源；接收任务为 nil
	wire   []byte

	mu    sync.Mutex
	state transferState

	cancelMu   sync.Mutex
	cancelFlag bool
	cancelled  chan struct{}

	busy    atomic.Int32
	created time.Time
}

func (t *Transfer) metadata() store.FileMetadata {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.state.meta
}

func (t *Transfer) isCancelled() bool { return t.cancelFlag }

// requestCancel 置位取消标志并关闭 cancelled channel，唤醒所有等待方。
func (t *Transfer) requestCancel() {
	t.cancelMu.Lock()
	defer t.cancelMu.Unlock()
	if !t.cancelFlag {
		t.cancelFlag = true
		close(t.cancelled)
	}
}

// resetCancel 恢复一个已取消的续传任务（对照 Rust cancel.send_replace(false)）。
func (t *Transfer) resetCancel() {
	t.cancelMu.Lock()
	defer t.cancelMu.Unlock()
	if t.cancelFlag {
		t.cancelFlag = false
		t.cancelled = make(chan struct{})
	}
}

func (t *Transfer) done() <-chan struct{} {
	t.cancelMu.Lock()
	defer t.cancelMu.Unlock()
	return t.cancelled
}

type checkpoint struct {
	Peer   Peer   `json:"peer"`
	Packet uint32 `json:"packet"`
	FileID uint32 `json:"file_id"`
	Part   string `json:"part"`
}

// Transfers 是文件传输管理器。downloads 容量 4 对应 Rust mpsc(4)；
// active/uploads 容量 4 对应 Semaphore(4)。
type Transfers struct {
	link Link
	db   *store.Database
	emit EmitFunc
	root string
	port uint16

	mu         sync.Mutex
	selections map[string]*source
	transfers  map[string]*Transfer

	gate       sync.Mutex // 串行化容量敏感操作（对照 Rust gate）
	stop       chan struct{}
	stopOnce   sync.Once
	stopped    atomic.Bool
	runWG      sync.WaitGroup
	listener   net.Listener
	downloads  chan *Transfer
	active     chan struct{}
	uploads    chan struct{}
	handshakes chan struct{}
}

// Start 绑定与 UDP 同端口的 TCP 监听并启动引擎；启动时清理不可续传的遗留分片。
func Start(link Link, db *store.Database, emit EmitFunc, root string, port uint16) (*Transfers, error) {
	if err := os.MkdirAll(root, 0o755); err != nil {
		return nil, err
	}
	resolved, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if resolved, err = filepath.EvalSymlinks(resolved); err != nil {
		return nil, err
	}
	listener, err := net.Listen("tcp4", fmt.Sprintf(":%d", port))
	if err != nil {
		return nil, fmt.Errorf("无法绑定文件传输TCP端口%d：%v", port, err)
	}
	m := &Transfers{
		link:       link,
		db:         db,
		emit:       emit,
		root:       resolved,
		port:       port,
		selections: map[string]*source{},
		transfers:  map[string]*Transfer{},
		stop:       make(chan struct{}),
		listener:   listener,
		downloads:  make(chan *Transfer, 4),
		active:     make(chan struct{}, 4),
		uploads:    make(chan struct{}, 4),
		handshakes: make(chan struct{}, 8),
	}
	// 重启后丢弃不可续传的分片文件，避免 Downloads 里堆积孤儿 .part。
	for _, id := range receiveIDs(db) {
		meta, err := db.FileMetadataByID(id)
		if err != nil || meta == nil || !meta.CanResume {
			_ = m.discardPartial(id)
		}
	}
	m.runWG.Add(2)
	go m.run(listener)
	go m.acceptor(listener)
	return m, nil
}

func receiveIDs(db *store.Database) []string {
	ids, err := db.ReceiveIDs()
	if err != nil {
		return nil
	}
	return ids
}

func (m *Transfers) wire(pktNo uint32, command uint32, body string, extra *string) ([]byte, error) {
	return m.link.BuildWire(pktNo, command, body, extra)
}

// sendUDP 经 Hub 的共享 UDP socket 发送（端口与对端保持一致才能被飞秋识别）。
func (m *Transfers) sendUDP(wire []byte, peer Peer) error {
	addr, err := m.address(&peer)
	if err != nil {
		return err
	}
	return m.link.SendUDP(addr, wire)
}

func (m *Transfers) address(peer *Peer) (*net.UDPAddr, error) {
	addr := &net.UDPAddr{IP: net.ParseIP(peer.Addr), Port: peer.Port}
	if addr.IP == nil || peer.Port <= 0 || peer.Port > 65535 {
		return nil, errors.New("无效文件对端")
	}
	return addr, nil
}

func (m *Transfers) list() []*Transfer {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := make([]*Transfer, 0, len(m.transfers))
	for _, t := range m.transfers {
		out = append(out, t)
	}
	return out
}

func (m *Transfers) get(id string) (*Transfer, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if t, ok := m.transfers[id]; ok {
		return t, nil
	}
	return nil, errors.New("文件任务已结束或不存在")
}

func (m *Transfers) update(t *Transfer) {
	m.emit("file.updated", map[string]any{
		"messageId": t.id,
		"target":    t.peer.ID,
		"file":      t.metadata(),
	})
}

// SelectPath 预览一个待发送文件/文件夹（最多 8 项，600 秒过期）。
func (m *Transfers) SelectPath(path string) (Selection, error) {
	if m.stopped.Load() {
		return Selection{}, errors.New("程序正在退出")
	}
	m.gate.Lock()
	defer m.gate.Unlock()
	m.mu.Lock()
	if len(m.selections) >= 8 {
		m.mu.Unlock()
		return Selection{}, errors.New("最多预览8个文件，请先发送或取消")
	}
	m.mu.Unlock()

	id := fmt.Sprintf("file-%d-%d", time.Now().Unix(), m.link.NextPacketNo())
	src, err := buildSource(path, id)
	if err != nil {
		return Selection{}, err
	}
	if m.stopped.Load() {
		return Selection{}, errors.New("程序正在退出")
	}
	m.mu.Lock()
	m.selections[src.selection.SelectionID] = src
	m.mu.Unlock()
	return src.selection, nil
}

func buildSource(path, id string) (*source, error) {
	meta, err := os.Lstat(path)
	if err != nil {
		return nil, err
	}
	if isLink(meta) {
		return nil, errors.New("不支持链接或重解析点")
	}
	var (
		folder *Snapshot
		file   *os.File
	)
	if meta.IsDir() {
		folder, err = CaptureSnapshot(path)
		if err != nil {
			return nil, err
		}
	} else if meta.Mode().IsRegular() {
		file, err = os.Open(path)
		if err != nil {
			return nil, err
		}
	}
	if folder == nil && file == nil {
		return nil, errors.New("请选择普通文件或文件夹")
	}
	size := uint64(meta.Size())
	if folder != nil {
		size = folder.Size
	}
	if size > MaxFileSize {
		return nil, errors.New("文件或文件夹超过8 GiB")
	}
	name := filepath.Base(path)
	fileName := SanitizeName(name)
	// 与 Rust 一致：用一次编码验证文件名可以上线（GBK 可表示）。
	if _, err := ipmsg.EncodePacket("1", 1, "p", "h", ipmsg.CmdSendMsg, "", &fileName); err != nil {
		return nil, err
	}
	selection := Selection{
		SelectionID: id,
		FileName:    fileName,
		FileSize:    size,
		IsDirectory: meta.IsDir(),
	}
	var modified time.Time
	if file != nil {
		if info, err := file.Stat(); err == nil {
			modified = info.ModTime()
		}
	} else {
		modified = meta.ModTime()
	}
	return &source{file: file, folder: folder, modified: modified, selection: selection, created: time.Now()}, nil
}

// Discard 放弃一个预览（关闭其持有的源文件句柄）。
func (m *Transfers) Discard(id string) {
	m.mu.Lock()
	src, ok := m.selections[id]
	if ok {
		delete(m.selections, id)
	}
	m.mu.Unlock()
	if ok && src.file != nil {
		src.file.Close()
	}
}

// Send 把预览升级为发送任务：入库、发 SENDMSG|SENDCHECK|FILEATTACH 并等待 TCP 拉取。
func (m *Transfers) Send(peer Peer, selection string) (store.Record, error) {
	m.gate.Lock()
	defer m.gate.Unlock()
	if m.stopped.Load() {
		return store.Record{}, errors.New("程序正在退出")
	}
	active := 0
	for _, t := range m.list() {
		meta := t.metadata()
		if !meta.Terminal() && t.source != nil {
			active++
		}
	}
	if active >= 4 {
		return store.Record{}, errors.New("最多同时发送4个文件")
	}
	if len(m.transfers) >= 128 {
		return store.Record{}, errors.New("文件任务记录过多，请稍后重试")
	}
	m.mu.Lock()
	src, ok := m.selections[selection]
	if ok {
		delete(m.selections, selection)
	}
	m.mu.Unlock()
	if !ok {
		return store.Record{}, errors.New("文件预览已过期")
	}
	// 后续任何失败都要释放源文件句柄（Rust 靠 Source 析构）。
	releaseSource := true
	defer func() {
		if releaseSource && src.file != nil {
			src.file.Close()
		}
	}()

	packet := m.link.NextPacketNo()
	attr := uint64(1)
	if src.selection.IsDirectory {
		attr = 2
	}
	meta := store.FileMetadata{
		IsDirectory: src.selection.IsDirectory,
		FileName:    src.selection.FileName,
		FileSize:    src.selection.FileSize,
		State:       "offered",
		Incoming:    false,
	}
	extra := fmt.Sprintf("1:%s:%x:0:%x:\x07",
		strings.ReplaceAll(meta.FileName, ":", "::"), meta.FileSize, attr)
	wire, err := m.wire(packet, ipmsg.CmdSendMsg|ipmsg.OptSendCheck|ipmsg.OptFileAttach,
		fmt.Sprintf("[文件] %s", meta.FileName), &extra)
	if err != nil {
		return store.Record{}, err
	}
	id := fmt.Sprintf("tx-file-%d-%d", time.Now().Unix(), packet)
	record := store.Record{
		ID:        id,
		FromID:    m.link.LocalID(),
		ToID:      peer.ID,
		Content:   fmt.Sprintf("[文件] %s", meta.FileName),
		Kind:      2,
		Timestamp: time.Now().Unix(),
		Status:    0,
		File:      &meta,
	}
	if _, err := m.db.Insert(record); err != nil {
		return store.Record{}, err
	}
	transfer := &Transfer{
		id:     id,
		peer:   peer,
		packet: packet,
		fileID: 1,
		source: src,
		wire:   wire,
		state: transferState{
			meta:         meta,
			acknowledged: false,
			attempts:     1,
			retry:        time.Now().Add(2 * time.Second),
			lastActivity: time.Now(),
		},
		cancelled: make(chan struct{}),
		created:   time.Now(),
	}
	m.mu.Lock()
	m.transfers[id] = transfer
	m.mu.Unlock()
	releaseSource = false
	if err := m.sendUDP(wire, peer); err != nil {
		_, _ = m.finish(transfer, "failed", err.Error(), nil)
	}
	return record, nil
}

// Incoming 处理对端 SENDMSG|FILEATTACH：解析邀请、入库并回 RECVMSG 回执。
func (m *Transfers) Incoming(peer Peer, pkt ipmsg.Packet) error {
	offers, err := Offers(pkt)
	if err != nil {
		return err
	}
	m.gate.Lock()
	defer m.gate.Unlock()
	if m.stopped.Load() {
		return errors.New("程序正在退出")
	}
	total := len(m.transfers)
	pending := 0
	for _, t := range m.transfers {
		if !t.metadata().Terminal() {
			pending++
		}
	}
	if total+len(offers) > 128 || pending+len(offers) > 64 {
		return errors.New("待处理文件邀请过多")
	}
	for _, offer := range offers {
		id := fmt.Sprintf("%s:%d:file:%d", peer.ID, pkt.PacketNo, offer.ID)
		meta := store.FileMetadata{
			IsDirectory: offer.Directory,
			FileName:    offer.Name,
			FileSize:    offer.Size,
			State:       "offered",
			Incoming:    true,
		}
		record := store.Record{
			ID:        id,
			FromID:    peer.ID,
			ToID:      m.link.LocalID(),
			Content:   fmt.Sprintf("[文件] %s", offer.Name),
			Kind:      2,
			Timestamp: time.Now().Unix(),
			Status:    0,
			File:      &meta,
		}
		inserted, err := m.db.Insert(record)
		if err != nil {
			return err
		}
		if inserted {
			transfer := &Transfer{
				id:     id,
				peer:   peer,
				packet: pkt.PacketNo,
				fileID: offer.ID,
				state: transferState{
					meta:         meta,
					acknowledged: true,
					attempts:     0,
					retry:        time.Now(),
					lastActivity: time.Now(),
				},
				cancelled: make(chan struct{}),
				created:   time.Now(),
			}
			m.mu.Lock()
			m.transfers[id] = transfer
			m.mu.Unlock()
			m.emit("message.received", map[string]any{
				"id":        id,
				"from":      peer.ID,
				"fromUser":  peer,
				"content":   record.Content,
				"type":      "file",
				"timestamp": record.Timestamp,
				"file":      meta,
			})
		}
	}
	// 回执只确认邀请本身；接受时才单独发起 TCP 请求。
	ack, err := m.wire(m.link.NextPacketNo(), ipmsg.CmdRecvMsg, strconv.FormatUint(uint64(pkt.PacketNo), 10), nil)
	if err != nil {
		return err
	}
	return m.sendUDP(ack, peer)
}

// Receipt 处理 RECVMSG 回执 / RELEASEFILES 拒收。
func (m *Transfers) Receipt(pkt ipmsg.Packet, addr *net.UDPAddr) {
	number, err := strconv.ParseUint(pkt.Body, 10, 32)
	if err != nil {
		return
	}
	for _, t := range m.list() {
		if t.source == nil || t.packet != uint32(number) ||
			t.peer.UserName != pkt.Username || t.peer.Host != pkt.Hostname {
			continue
		}
		peerAddr, err := m.address(&t.peer)
		if err != nil || peerAddr.String() != addr.String() {
			continue
		}
		if ipmsg.CommandMode(pkt.Command) == ipmsg.CmdReleaseFiles {
			_, _ = m.Cancel(t.id, false)
		} else {
			t.mu.Lock()
			t.state.acknowledged = true
			t.mu.Unlock()
		}
	}
}

// Accept 开始接收一个已邀请的文件（进入下载队列，容量 4）。
func (m *Transfers) Accept(id string) error {
	m.gate.Lock()
	defer m.gate.Unlock()
	if m.stopped.Load() {
		return errors.New("程序正在退出")
	}
	busyDownloads := 0
	for _, t := range m.list() {
		if t.source == nil && t.busy.Load() > 0 {
			busyDownloads++
		}
	}
	if busyDownloads >= 4 {
		return errors.New("接收队列已满，请稍后重试")
	}
	t, err := m.restore(id)
	if err != nil {
		return err
	}
	var previous store.FileMetadata
	{
		t.mu.Lock()
		if t.source != nil ||
			(t.state.meta.State != "offered" && t.state.meta.State != "paused") ||
			t.busy.Load() > 0 {
			t.mu.Unlock()
			return errors.New("文件已处理或不能接收")
		}
		previous = t.state.meta
		t.state.meta.State = "transferring"
		t.state.meta.Error = nil
		t.state.meta.InFlightBytes = 0
		t.state.meta.Attempt++
		t.mu.Unlock()
		t.resetCancel()
		t.busy.Add(1)
	}
	select {
	case m.downloads <- t:
	default:
		t.busy.Add(-1)
		t.mu.Lock()
		t.state.meta = previous
		t.mu.Unlock()
		return errors.New("接收队列已满")
	}
	m.update(t)
	return nil
}

// Cancel 取消（reject=false）或拒绝（reject=true）一个文件任务。
// 返回 false 表示任务已处于无法取消的终态。
func (m *Transfers) Cancel(id string, reject bool) (bool, error) {
	t, err := m.restore(id)
	if err != nil {
		return false, err
	}
	{
		t.mu.Lock()
		if (t.state.meta.Terminal() && t.state.meta.State != "paused") ||
			t.state.meta.State == "finalizing" {
			t.mu.Unlock()
			return false, nil
		}
		if reject {
			t.state.meta.State = "rejected"
			t.state.meta.Error = strPtr("已拒绝文件")
		} else {
			t.state.meta.State = "cancelled"
			t.state.meta.Error = strPtr("文件传输已取消")
		}
		t.state.meta.CanResume = false
		t.mu.Unlock()
		t.requestCancel()
	}
	if err := m.discardPartial(id); err != nil {
		return true, err
	}
	if err := m.db.FileState(t.id, t.metadata(), nil); err != nil {
		return true, err
	}
	m.update(t)
	allDone := true
	for _, other := range m.list() {
		if other.packet == t.packet && other.peer.ID == t.peer.ID && other.source == nil {
			meta := other.metadata()
			if !meta.Terminal() || meta.State == "paused" {
				allDone = false
				break
			}
		}
	}
	if t.source == nil && allDone {
		retained, err := m.retainsInvitation(t)
		if err == nil && !retained {
			if wire, err := m.wire(m.link.NextPacketNo(), ipmsg.CmdReleaseFiles,
				strconv.FormatUint(uint64(t.packet), 10), nil); err == nil {
				_ = m.sendUDP(wire, t.peer)
			}
		}
	}
	return true, nil
}

// CancelCleared 历史清空后取消对应任务；没有元数据记录的连带清理分片。
func (m *Transfers) CancelCleared(ids []string) {
	for _, id := range ids {
		_, _ = m.Cancel(id, false)
		if meta, err := m.db.FileMetadataByID(id); err == nil && meta == nil {
			_ = m.discardPartial(id)
		}
	}
}

// Pause 暂停可续传的下载；等正在落盘的字节排空（最多 2 秒）。
func (m *Transfers) Pause(id string) error {
	t, err := m.get(id)
	if err != nil {
		return err
	}
	{
		t.mu.Lock()
		if t.source != nil || t.state.meta.IsDirectory ||
			!t.state.meta.CanResume || t.state.meta.State != "transferring" {
			t.mu.Unlock()
			return errors.New("当前文件不能暂停")
		}
		t.state.meta.State = "paused"
		t.state.meta.InFlightBytes = 0
		t.state.meta.Error = strPtr("已暂停，可继续接收")
		t.mu.Unlock()
		t.requestCancel()
	}
	if err := m.db.FileState(t.id, t.metadata(), nil); err != nil {
		return err
	}
	m.update(t)
	deadline := time.Now().Add(2 * time.Second)
	for t.busy.Load() > 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	return nil
}

func (m *Transfers) discardPartial(id string) error {
	ckText, err := m.db.ReceiveCheckpoint(id)
	if err != nil {
		return err
	}
	if ckText != nil {
		var ck checkpoint
		if err := json.Unmarshal([]byte(*ckText), &ck); err == nil {
			path, err := checkedPart(m.root, ck.Part)
			if err != nil {
				return err
			}
			if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
				return err
			}
		}
	}
	return m.db.RemoveReceive(id)
}

// retainsInvitation 检查数据库里是否还有同一邀请的续传记录
// （暂停的兄弟任务重启后可能不在内存中）。
func (m *Transfers) retainsInvitation(t *Transfer) (bool, error) {
	for _, id := range receiveIDs(m.db) {
		ckText, err := m.db.ReceiveCheckpoint(id)
		if err != nil || ckText == nil {
			continue
		}
		var ck checkpoint
		if err := json.Unmarshal([]byte(*ckText), &ck); err != nil {
			continue
		}
		if ck.Packet == t.packet && ck.Peer.ID == t.peer.ID {
			return true, nil
		}
	}
	return false, nil
}

// restore 从内存取任务；重启后凭续传检查点与元数据重建。
func (m *Transfers) restore(id string) (*Transfer, error) {
	if t, err := m.get(id); err == nil {
		return t, nil
	}
	ckText, err := m.db.ReceiveCheckpoint(id)
	if err != nil {
		return nil, err
	}
	if ckText == nil {
		return nil, errors.New("任务已结束或没有续传记录")
	}
	var ck checkpoint
	if err := json.Unmarshal([]byte(*ckText), &ck); err != nil {
		return nil, err
	}
	meta, err := m.db.FileMetadataByID(id)
	if err != nil {
		return nil, err
	}
	if meta == nil {
		return nil, errors.New("历史已清空")
	}
	if !meta.CanResume || meta.IsDirectory {
		return nil, errors.New("该任务不能续传")
	}
	// 打开分片属于 receive()：分片缺失也要让恢复出的任务保持可取消。
	meta.InFlightBytes = 0
	meta.State = "paused"
	t := &Transfer{
		id:     id,
		peer:   ck.Peer,
		packet: ck.Packet,
		fileID: ck.FileID,
		state: transferState{
			meta:         *meta,
			acknowledged: true,
			attempts:     0,
			retry:        time.Now(),
			lastActivity: time.Now(),
		},
		cancelled: make(chan struct{}),
		created:   time.Now(),
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if existing, ok := m.transfers[id]; ok {
		return existing, nil
	}
	m.transfers[id] = t
	return t, nil
}

func strPtr(s string) *string { return &s }

// finish 进入终态并落库；completed 时补齐 transferred 并记录本地路径。
// 落库失败会把状态改写为 failed（对照 Rust finish 的回退分支）。
func (m *Transfers) finish(t *Transfer, phase string, errorMsg string, path *string) (bool, error) {
	{
		t.mu.Lock()
		if t.state.meta.Terminal() {
			t.mu.Unlock()
			return false, nil
		}
		t.state.meta.State = phase
		t.state.meta.InFlightBytes = 0
		if errorMsg != "" {
			t.state.meta.Error = strPtr(errorMsg)
		} else {
			t.state.meta.Error = nil
		}
		if path != nil {
			t.state.meta.HasLocalFile = true
		}
		if phase == "completed" {
			t.state.meta.Transferred = t.state.meta.FileSize
		}
		t.mu.Unlock()
	}
	if err := m.db.FileState(t.id, t.metadata(), path); err != nil {
		t.mu.Lock()
		t.state.meta.State = "failed"
		t.state.meta.HasLocalFile = false
		t.state.meta.Error = strPtr(fmt.Sprintf("无法保存文件状态：%v", err))
		t.mu.Unlock()
		_ = m.db.FileState(t.id, t.metadata(), nil)
		m.update(t)
		return false, err
	}
	m.update(t)
	return true, nil
}

func (m *Transfers) progress(t *Transfer, bytes uint64) {
	{
		t.mu.Lock()
		if t.state.meta.Terminal() && t.state.meta.State != "paused" {
			t.mu.Unlock()
			return
		}
		if bytes > t.state.meta.Transferred {
			t.state.meta.Transferred = bytes
		}
		t.state.lastActivity = time.Now()
		t.mu.Unlock()
	}
	if t.metadata().State == "paused" {
		_ = m.db.FileState(t.id, t.metadata(), nil)
	}
	m.update(t)
}

// receiveFailed 接收异常收尾：有检查点的普通文件转为可续传暂停。
func (m *Transfers) receiveFailed(t *Transfer, errorMsg string) {
	if t.metadata().Terminal() {
		return
	}
	resumable := t.source == nil && !t.metadata().IsDirectory
	if resumable {
		ckText, err := m.db.ReceiveCheckpoint(t.id)
		resumable = err == nil && ckText != nil
	}
	if resumable {
		t.mu.Lock()
		t.state.meta.CanResume = true
		t.mu.Unlock()
	}
	phase := "failed"
	if resumable {
		phase = "paused"
	}
	_, _ = m.finish(t, phase, errorMsg, nil)
}

// acquireSlot 带取消地占用一个并发名额（30 秒拿不到视为超时）。
func acquireSlot(t *Transfer, stop <-chan struct{}, slot chan struct{}) error {
	timer := time.NewTimer(30 * time.Second)
	defer timer.Stop()
	select {
	case slot <- struct{}{}:
		return nil
	case <-t.done():
		return errors.New("文件传输已取消")
	case <-stop:
		return errors.New("文件传输已取消")
	case <-timer.C:
		return errors.New("文件传输30秒无响应")
	}
}

// failOffer 把仍在 offered 的任务判为失败（重试耗尽或过期）。
func (m *Transfers) failOffer(t *Transfer, errorMsg string) {
	{
		t.mu.Lock()
		// 定时器检查后 accept/TCP 请求可能已经抢先接管。
		if t.state.meta.State != "offered" {
			t.mu.Unlock()
			return
		}
		t.state.meta.State = "finalizing"
		t.mu.Unlock()
	}
	t.requestCancel()
	_, _ = m.finish(t, "failed", errorMsg, nil)
}

func (m *Transfers) tick() {
	m.mu.Lock()
	for id, s := range m.selections {
		if time.Since(s.created) >= 600*time.Second {
			if s.file != nil {
				s.file.Close()
			}
			delete(m.selections, id)
		}
	}
	m.mu.Unlock()
	for _, t := range m.list() {
		meta := t.metadata()
		if t.source != nil && meta.State == "transferring" && t.busy.Load() == 0 {
			t.mu.Lock()
			idle := time.Since(t.state.lastActivity) > 60*time.Second
			t.mu.Unlock()
			if idle {
				_, _ = m.finish(t, "failed", "对方未继续接收文件", nil)
				continue
			}
		}
		if meta.State == "offered" && time.Since(t.created) > 600*time.Second {
			m.failOffer(t, "文件邀请已过期")
			continue
		}
		action := 0 // 0 无动作 1 重发邀请 2 重试耗尽判失败
		{
			t.mu.Lock()
			if t.source != nil && t.state.meta.State == "offered" &&
				!t.state.acknowledged && !time.Now().Before(t.state.retry) {
				t.state.retry = t.state.retry.Add(2 * time.Second)
				t.state.attempts++
				if t.state.attempts <= 3 {
					action = 1
				} else {
					action = 2
				}
			}
			t.mu.Unlock()
		}
		if action == 1 {
			if addr, err := m.address(&t.peer); err == nil {
				_ = m.link.SendUDP(addr, t.wire)
			}
		} else if action == 2 {
			m.failOffer(t, "对方未确认文件邀请")
		}
	}
	m.mu.Lock()
	for id, t := range m.transfers {
		if t.metadata().Terminal() && t.busy.Load() == 0 &&
			time.Since(t.created) >= 600*time.Second {
			if t.source != nil && t.source.file != nil {
				t.source.file.Close()
			}
			delete(m.transfers, id)
		}
	}
	m.mu.Unlock()
}

func (m *Transfers) run(listener net.Listener) {
	defer m.runWG.Done()
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-m.stop:
			return
		case <-ticker.C:
			m.tick()
		case t := <-m.downloads:
			go m.downloadWorker(t)
		}
	}
}

// acceptor 只负责 accept；stop 时关闭 listener 解除阻塞。
func (m *Transfers) acceptor(listener net.Listener) {
	defer m.runWG.Done()
	for {
		conn, err := listener.Accept()
		if err != nil {
			if m.stopped.Load() || errors.Is(err, net.ErrClosed) {
				return
			}
			time.Sleep(10 * time.Millisecond)
			continue
		}
		select {
		case m.handshakes <- struct{}{}:
			go func() {
				defer func() { <-m.handshakes }()
				_ = m.serve(conn, conn.RemoteAddr())
			}()
		default:
			conn.Close()
		}
	}
}

func (m *Transfers) downloadWorker(t *Transfer) {
	t.busy.Add(1)
	defer t.busy.Add(-1)
	done := make(chan error, 1)
	go func() { done <- m.receive(t) }()
	select {
	case err := <-done:
		if err != nil {
			m.receiveFailed(t, err.Error())
		}
	case <-time.After(time.Hour):
		t.requestCancel()
		m.receiveFailed(t, "文件接收超过1小时")
	}
}

// watchConn 在取消/退出时关闭连接以解除阻塞的读写。
func watchConn(t *Transfer, stop <-chan struct{}, conn net.Conn) (release func()) {
	done := make(chan struct{})
	go func() {
		select {
		case <-t.done():
			conn.Close()
		case <-stop:
			conn.Close()
		case <-done:
		}
	}()
	return func() { close(done) }
}

func wrapIO(err error) error {
	if err == nil {
		return nil
	}
	var ne net.Error
	if errors.As(err, &ne) && ne.Timeout() {
		return errors.New("文件传输30秒无响应")
	}
	return err
}

// dialWithCancel 带取消拨号（30 秒超时）。
func dialWithCancel(t *Transfer, stop <-chan struct{}, addr string) (net.Conn, error) {
	type result struct {
		conn net.Conn
		err  error
	}
	done := make(chan result, 1)
	go func() {
		conn, err := net.DialTimeout("tcp4", addr, 30*time.Second)
		done <- result{conn, err}
	}()
	select {
	case r := <-done:
		return r.conn, r.err
	case <-t.done():
		go func() {
			if r := <-done; r.conn != nil {
				r.conn.Close()
			}
		}()
		return nil, errors.New("文件传输已取消")
	case <-stop:
		go func() {
			if r := <-done; r.conn != nil {
				r.conn.Close()
			}
		}()
		return nil, errors.New("文件传输已取消")
	case <-time.After(30 * time.Second):
		go func() {
			if r := <-done; r.conn != nil {
				r.conn.Close()
			}
		}()
		return nil, errors.New("文件传输30秒无响应")
	}
}

// connection 建立接收连接：先回 RECVMSG 回执，再发 TCP 请求。
func (m *Transfers) connection(t *Transfer, offset uint64) (net.Conn, func(), error) {
	ack, err := m.wire(m.link.NextPacketNo(), ipmsg.CmdRecvMsg,
		strconv.FormatUint(uint64(t.packet), 10), nil)
	if err != nil {
		return nil, nil, err
	}
	addr, err := m.address(&t.peer)
	if err != nil {
		return nil, nil, err
	}
	if err := m.link.SendUDP(addr, ack); err != nil {
		return nil, nil, err
	}
	conn, err := dialWithCancel(t, m.stop, addr.String())
	if err != nil {
		return nil, nil, err
	}
	release := watchConn(t, m.stop, conn)
	command := uint32(ipmsg.CmdGetFileData)
	if t.metadata().IsDirectory {
		command = ipmsg.CmdGetDirFiles
	}
	request, err := m.wire(m.link.NextPacketNo(), command,
		fmt.Sprintf("%x:%x:%x:", t.packet, t.fileID, offset), nil)
	if err != nil {
		conn.Close()
		release()
		return nil, nil, err
	}
	if err := writeAll(conn, request); err != nil {
		conn.Close()
		release()
		return nil, nil, wrapIO(err)
	}
	return conn, release, nil
}

func writeAll(conn net.Conn, data []byte) error {
	written := 0
	for written < len(data) {
		n, err := conn.Write(data[written:])
		if err != nil {
			return err
		}
		written += n
	}
	return nil
}

// receive 下载主体：断点续传、4 MiB 以上并行分块、完成发布。
func (m *Transfers) receive(t *Transfer) error {
	if t.isCancelled() || m.stopped.Load() {
		return errors.New("接收已停止")
	}
	if err := acquireSlot(t, m.stop, m.active); err != nil {
		return err
	}
	defer func() { <-m.active }()
	if t.metadata().IsDirectory {
		return m.receiveDirectory(t)
	}
	size := t.metadata().FileSize

	var pf *PartialFile
	var file *os.File
	ckText, err := m.db.ReceiveCheckpoint(t.id)
	if err != nil {
		return err
	}
	if ckText != nil {
		var ck checkpoint
		if err := json.Unmarshal([]byte(*ckText), &ck); err != nil {
			return err
		}
		pf, file, err = ResumePartial(m.root, ck.Part, size)
	} else {
		pf, file, err = CreatePartial(m.root, m.link.NextPacketNo())
	}
	if err != nil {
		return err
	}
	info, err := file.Stat()
	if err != nil {
		file.Close()
		return err
	}
	offset := uint64(info.Size())
	ck := checkpoint{Peer: t.peer, Packet: t.packet, FileID: t.fileID, Part: pf.Name()}
	ckJSON, err := json.Marshal(ck)
	if err != nil {
		file.Close()
		return err
	}
	if err := m.db.SaveReceive(t.id, string(ckJSON)); err != nil {
		file.Close()
		return err
	}
	{
		t.mu.Lock()
		terminal := t.state.meta.Terminal() || t.isCancelled()
		if !terminal {
			pf.Keep()
			t.state.meta.CanResume = true
			t.state.meta.Transferred = offset
		}
		t.mu.Unlock()
		if terminal {
			file.Close()
			pf.Discard()
			if err := m.discardPartial(t.id); err != nil {
				return err
			}
			return errors.New("接收已停止")
		}
	}
	if err := m.db.FileState(t.id, t.metadata(), nil); err != nil {
		file.Close()
		return err
	}
	m.update(t)

	result := m.receiveFileData(t, file, offset, size)

	info, err = file.Stat()
	if err != nil {
		file.Close()
		return err
	}
	m.progress(t, uint64(info.Size()))
	if result != nil {
		file.Close()
		return result
	}
	if err := file.Sync(); err != nil {
		file.Close()
		return err
	}
	file.Close()
	{
		t.mu.Lock()
		if t.state.meta.Terminal() {
			t.mu.Unlock()
			return errors.New("接收已停止")
		}
		t.state.meta.State = "finalizing"
		t.mu.Unlock()
	}
	path, err := pf.Publish(m.root, t.metadata().FileName)
	if err != nil {
		return err
	}
	t.mu.Lock()
	t.state.meta.CanResume = false
	t.mu.Unlock()
	if _, err := m.finish(t, "completed", "", &path); err != nil {
		return err
	}
	return m.db.RemoveReceive(t.id)
}

// receiveFileData 先尝试并行分块（≥4 MiB 且有空闲名额），失败回退单连接顺序接收。
func (m *Transfers) receiveFileData(t *Transfer, file *os.File, offset, size uint64) error {
	// 续传时文件位置在 0，必须先对齐到断点。
	if _, err := file.Seek(int64(offset), io.SeekStart); err != nil {
		return err
	}
	done := offset
	if size-offset >= 4*1024*1024 {
		extra := 0
	slots:
		for i := 0; i < 3; i++ {
			select {
			case m.active <- struct{}{}:
				extra++
			default:
				break slots
			}
		}
		if extra > 0 {
			err := m.receiveParallel(t, file, offset, size, extra)
			t.mu.Lock()
			t.state.meta.InFlightBytes = 0
			t.mu.Unlock()
			if err == nil {
				return nil
			}
			if t.isCancelled() || m.stopped.Load() {
				return err
			}
			// 并行部分失败：以已写入长度为起点回退顺序接收。
			info, statErr := file.Stat()
			if statErr != nil {
				return statErr
			}
			done = uint64(info.Size())
			if _, err := file.Seek(int64(done), io.SeekStart); err != nil {
				return err
			}
		}
	}
	// 空文件也要发一次请求，让发送方观察到“已被接受”。
	conn, release, err := m.connection(t, done)
	if err != nil {
		return err
	}
	defer func() {
		conn.Close()
		release()
	}()
	buffer := make([]byte, 64*1024)
	last := time.Now()
	for done < size {
		if t.isCancelled() || m.stopped.Load() {
			return errors.New("文件传输已取消")
		}
		count := size - done
		if count > uint64(len(buffer)) {
			count = uint64(len(buffer))
		}
		conn.SetReadDeadline(time.Now().Add(30 * time.Second))
		n, err := conn.Read(buffer[:count])
		if err != nil {
			return wrapIO(err)
		}
		if n == 0 {
			return errors.New("连接中断，可继续接收")
		}
		if _, err := file.Write(buffer[:n]); err != nil {
			return err
		}
		done += uint64(n)
		if time.Since(last) >= 100*time.Millisecond {
			m.progress(t, done)
			last = time.Now()
		}
	}
	return nil
}

// receiveParallel 把剩余数据切成 count 段并行下载到临时分片，按序回填主文件。
func (m *Transfers) receiveParallel(t *Transfer, out *os.File, offset, size uint64, extra int) error {
	count := extra + 1
	chunk := (size - offset + uint64(count) - 1) / uint64(count)
	type chunkResult struct {
		index int
		pf    *PartialFile
		err   error
	}
	progress := make([]atomic.Uint64, count)
	results := make(chan chunkResult, count)
	abort := make(chan struct{})
	var abortOnce sync.Once
	stopChunks := func() { abortOnce.Do(func() { close(abort) }) }
	defer stopChunks()

	for i := 0; i < count; i++ {
		i := i
		start := offset + chunk*uint64(i)
		end := size
		if start+chunk < end {
			end = start + chunk
		}
		go func() {
			pf, file, err := CreatePartial(m.root, m.link.NextPacketNo())
			if err != nil {
				results <- chunkResult{index: i, err: err}
				return
			}
			conn, err := dialWithCancel(t, abort, fmt.Sprintf("%s:%d", t.peer.Addr, t.peer.Port))
			if err != nil {
				file.Close()
				pf.Discard()
				results <- chunkResult{index: i, err: err}
				return
			}
			release := watchConn2(t, abort, conn)
			defer func() {
				conn.Close()
				release()
				file.Close()
			}()
			command := uint32(ipmsg.CmdGetFileData)
			if t.metadata().IsDirectory {
				command = ipmsg.CmdGetDirFiles
			}
			request, err := m.wire(m.link.NextPacketNo(), command,
				fmt.Sprintf("%x:%x:%x:", t.packet, t.fileID, start), nil)
			if err != nil {
				results <- chunkResult{index: i, err: err}
				return
			}
			if err := writeAll(conn, request); err != nil {
				results <- chunkResult{index: i, err: wrapIO(err)}
				return
			}
			buffer := make([]byte, 64*1024)
			var remaining uint64 = end - start
			for remaining > 0 {
				select {
				case <-abort:
					results <- chunkResult{index: i, err: errors.New("并行接收已中止")}
					return
				default:
				}
				if t.isCancelled() {
					results <- chunkResult{index: i, err: errors.New("文件传输已取消")}
					return
				}
				length := remaining
				if length > uint64(len(buffer)) {
					length = uint64(len(buffer))
				}
				conn.SetReadDeadline(time.Now().Add(30 * time.Second))
				n, err := conn.Read(buffer[:length])
				if err != nil {
					results <- chunkResult{index: i, err: wrapIO(err)}
					return
				}
				if n == 0 {
					results <- chunkResult{index: i, err: errors.New("并行连接中断")}
					return
				}
				if _, err := file.Write(buffer[:n]); err != nil {
					results <- chunkResult{index: i, err: err}
					return
				}
				remaining -= uint64(n)
				progress[i].Add(uint64(n))
			}
			results <- chunkResult{index: i, pf: pf}
		}()
	}

	ready := map[int]*PartialFile{}
	next := 0
	received := 0
	timer := time.NewTicker(100 * time.Millisecond)
	defer timer.Stop()
	var failure error
	for received < count && failure == nil {
		select {
		case r := <-results:
			received++
			if r.err != nil {
				failure = r.err
				break
			}
			ready[r.index] = r.pf
		case <-timer.C:
			t.mu.Lock()
			if t.state.meta.Terminal() {
				t.mu.Unlock()
				stopChunks()
				return errors.New("接收已停止")
			}
			var inflight uint64 = offset
			for i := range progress {
				inflight += progress[i].Load()
			}
			t.state.meta.InFlightBytes = inflight
			t.mu.Unlock()
			m.update(t)
		case <-t.done():
			stopChunks()
			return errors.New("接收已停止")
		case <-m.stop:
			stopChunks()
			return errors.New("接收已停止")
		}
		// 已就绪的连续分片按序回填主文件。
		for {
			pf, ok := ready[next]
			if !ok {
				break
			}
			input, err := os.Open(mustCheckedPart(m.root, pf.Name()))
			if err != nil {
				stopChunks()
				return err
			}
			if _, err := io.Copy(out, input); err != nil {
				input.Close()
				stopChunks()
				return err
			}
			input.Close()
			pf.Discard()
			delete(ready, next)
			next++
			info, err := out.Stat()
			if err != nil {
				stopChunks()
				return err
			}
			m.progress(t, uint64(info.Size()))
		}
	}
	if failure != nil {
		stopChunks()
		// 已就绪未回填的分片直接丢弃；results 容量足够且每个 chunk goroutine
		// 恰好发送一次，由后台协程收尾剩余结果并清理其分片文件。
		for _, pf := range ready {
			pf.Discard()
		}
		pending := count - received
		go func() {
			for i := 0; i < pending; i++ {
				r := <-results
				if r.pf != nil {
					r.pf.Discard()
				}
			}
		}()
		return failure
	}
	if next != count {
		return errors.New("并行片段不完整")
	}
	return nil
}

func mustCheckedPart(root, name string) string {
	path, err := checkedPart(root, name)
	if err != nil {
		return filepath.Join(root, name)
	}
	return path
}

// watchConn2 与 watchConn 相同，但额外监听一个中断通道（并行分块用）。
func watchConn2(t *Transfer, extra <-chan struct{}, conn net.Conn) (release func()) {
	done := make(chan struct{})
	go func() {
		select {
		case <-t.done():
			conn.Close()
		case <-extra:
			conn.Close()
		case <-done:
		}
	}()
	return func() { close(done) }
}

// readDirHeader 读取目录流头：4 位 hex 长度 + ':' + GBK 内容。
func (m *Transfers) readDirHeader(t *Transfer, conn net.Conn) (string, uint64, uint32, error) {
	prefix := make([]byte, 5)
	if err := readFull(t, conn, prefix); err != nil {
		return "", 0, 0, err
	}
	if prefix[4] != ':' {
		return "", 0, 0, errors.New("目录头格式错误")
	}
	length, err := strconv.ParseUint(string(prefix[:4]), 16, 32)
	if err != nil {
		return "", 0, 0, errors.New("目录长度无效")
	}
	if length < 5 || length > 8192 {
		return "", 0, 0, errors.New("目录头长度越界")
	}
	bytes := make([]byte, length)
	copy(bytes, prefix)
	if err := readFull(t, conn, bytes[5:]); err != nil {
		return "", 0, 0, err
	}
	return DecodeDirHeader(bytes)
}

func readFull(t *Transfer, conn net.Conn, buf []byte) error {
	total := 0
	for total < len(buf) {
		if t.isCancelled() {
			return errors.New("文件传输已取消")
		}
		conn.SetReadDeadline(time.Now().Add(30 * time.Second))
		n, err := conn.Read(buf[total:])
		if err != nil {
			return wrapIO(err)
		}
		if n == 0 {
			return errors.New("连接中断")
		}
		total += n
	}
	return nil
}

// receiveDirectory 接收目录流：暂存目录 + 路径栈，最终发布到 Downloads。
func (m *Transfers) receiveDirectory(t *Transfer) error {
	conn, release, err := m.connection(t, 0)
	if err != nil {
		return err
	}
	defer func() {
		conn.Close()
		release()
	}()
	staging, err := CreateStaging(m.root, m.link.NextPacketNo())
	if err != nil {
		return err
	}
	stack := []string{staging.Root}
	first := true
	var bytes uint64
	last := time.Now()
	for i := 0; i < MaxDirEntries; i++ {
		name, size, kind, err := m.readDirHeader(t, conn)
		if err != nil {
			staging.Discard()
			return err
		}
		if first {
			if kind != 2 {
				staging.Discard()
				return errors.New("目录流必须从根目录开始")
			}
			first = false
			continue
		}
		if kind == 3 {
			stack = stack[:len(stack)-1]
			if len(stack) == 0 {
				t.mu.Lock()
				if t.state.meta.Terminal() {
					t.mu.Unlock()
					staging.Discard()
					return errors.New("接收已取消")
				}
				t.state.meta.State = "finalizing"
				t.state.meta.FileSize = bytes
				t.mu.Unlock()
				path, err := staging.Publish(m.root, t.metadata().FileName)
				if err != nil {
					return err
				}
				if _, err := m.finish(t, "completed", "", &path); err != nil {
					return err
				}
				staging.Keep()
				return nil
			}
			continue
		}
		parent := stack[len(stack)-1]
		resolved, err := filepath.EvalSymlinks(parent)
		if err != nil || resolved != parent {
			staging.Discard()
			return errors.New("接收目录发生变化")
		}
		path := filepath.Join(parent, name)
		if kind == 2 {
			if len(stack) >= MaxDirDepth {
				staging.Discard()
				return errors.New("目录层级过多")
			}
			if err := os.Mkdir(path, 0o755); err != nil {
				staging.Discard()
				return err
			}
			stack = append(stack, path)
			continue
		}
		if bytes+size > MaxFileSize {
			staging.Discard()
			return errors.New("目录数据超过8 GiB")
		}
		file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
		if err != nil {
			staging.Discard()
			return err
		}
		var remaining uint64 = size
		buffer := make([]byte, 64*1024)
		for remaining > 0 {
			count := remaining
			if count > uint64(len(buffer)) {
				count = uint64(len(buffer))
			}
			conn.SetReadDeadline(time.Now().Add(30 * time.Second))
			n, err := conn.Read(buffer[:count])
			if err != nil {
				file.Close()
				staging.Discard()
				return wrapIO(err)
			}
			if n == 0 {
				file.Close()
				staging.Discard()
				return errors.New("文件夹传输被截断")
			}
			if _, err := file.Write(buffer[:n]); err != nil {
				file.Close()
				staging.Discard()
				return err
			}
			remaining -= uint64(n)
			bytes += uint64(n)
			t.mu.Lock()
			if bytes > t.state.meta.FileSize {
				t.state.meta.FileSize = bytes
			}
			t.mu.Unlock()
			if time.Since(last) > 100*time.Millisecond {
				m.progress(t, bytes)
				last = time.Now()
			}
		}
		if err := file.Sync(); err != nil {
			file.Close()
			staging.Discard()
			return err
		}
		file.Close()
	}
	staging.Discard()
	return errors.New("目录项目过多或缺少结束标记")
}

// serve 处理对端的 TCP 拉取请求：校验来源身份后发送文件/目录流。
func (m *Transfers) serve(conn net.Conn, address net.Addr) error {
	var accumulated []byte
	var pkt ipmsg.Packet
	var original, fileID uint32
	var offset uint64
	var err error
	deadline := time.Now().Add(5 * time.Second)
	for {
		if time.Now().After(deadline) {
			return errors.New("TCP请求超时")
		}
		conn.SetReadDeadline(time.Now().Add(5 * time.Second))
		buffer := make([]byte, 512)
		var n int
		n, err = conn.Read(buffer)
		if err != nil {
			return wrapIO(err)
		}
		if n == 0 {
			return errors.New("TCP请求截断")
		}
		accumulated = append(accumulated, buffer[:n]...)
		if len(accumulated) > 4096 {
			return errors.New("TCP请求过长")
		}
		pkt, original, fileID, offset, err = RequestPacket(accumulated)
		if err == nil {
			break
		}
	}
	var t *Transfer
	for _, candidate := range m.list() {
		if candidate.source == nil || candidate.packet != original || candidate.fileID != fileID {
			continue
		}
		if candidate.peer.Addr != remoteIP(address) ||
			candidate.peer.UserName != pkt.Username || candidate.peer.Host != pkt.Hostname {
			continue
		}
		t = candidate
		break
	}
	if t == nil {
		return errors.New("未授权的文件请求")
	}
	if err := acquireSlot(t, m.stop, m.uploads); err != nil {
		return err
	}
	defer func() { <-m.uploads }()
	meta := t.metadata()
	isDirRequest := ipmsg.CommandMode(pkt.Command) == ipmsg.CmdGetDirFiles
	if meta.IsDirectory != isDirRequest || offset > meta.FileSize ||
		(meta.Terminal() && meta.State != "completed") {
		return errors.New("文件状态或请求类型不匹配")
	}
	if meta.IsDirectory {
		if !t.busy.CompareAndSwap(0, 1) {
			return errors.New("文件夹正在发送")
		}
	} else {
		t.busy.Add(1)
	}
	defer t.busy.Add(-1)
	{
		t.mu.Lock()
		if t.state.meta.State == "finalizing" ||
			(t.state.meta.Terminal() && t.state.meta.State != "completed") {
			t.mu.Unlock()
			return errors.New("文件已结束")
		}
		t.state.acknowledged = true
		t.state.lastActivity = time.Now()
		if !t.state.meta.Terminal() {
			t.state.meta.State = "transferring"
			t.state.meta.Error = nil
		}
		t.mu.Unlock()
	}
	// 1 小时兜底：超时关闭连接让操作以错误收场。
	watchdog := time.AfterFunc(time.Hour, func() { conn.Close() })
	defer watchdog.Stop()

	operation := func() error {
		src := t.source
		if src == nil {
			return errors.New("没有文件源")
		}
		if src.folder != nil {
			var sent uint64
			for _, entry := range src.folder.Entries {
				header, err := EncodeDirHeader(entry.Name, entry.Size, entry.Kind)
				if err != nil {
					return err
				}
				conn.SetWriteDeadline(time.Now().Add(30 * time.Second))
				if err := writeAll(conn, header); err != nil {
					return wrapIO(err)
				}
				if entry.Kind == 1 {
					resolved, err := filepath.EvalSymlinks(entry.Path)
					if err != nil {
						return err
					}
					if !withinDir(resolved, src.folder.Root) {
						return errors.New("发送目录发生变化")
					}
					entryMeta, err := os.Lstat(entry.Path)
					if err != nil || isLink(entryMeta) {
						return errors.New("发送目录发生变化")
					}
					file, err := os.Open(resolved)
					if err != nil {
						return err
					}
					err = m.sendBytes(t, conn, file, entry.Modified, entry.Size, 0, sent)
					file.Close()
					if err != nil {
						return err
					}
					sent += entry.Size
				}
			}
			return nil
		}
		file := src.file
		if file == nil {
			return errors.New("文件源丢失")
		}
		return m.sendBytes(t, conn, file, src.modified, meta.FileSize, offset, 0)
	}
	if err := operation(); err != nil {
		conn.Close()
		if meta.IsDirectory || strings.Contains(err.Error(), "源文件") ||
			strings.Contains(err.Error(), "目录发生变化") {
			_, _ = m.finish(t, "failed", err.Error(), nil)
			return err
		}
		t.mu.Lock()
		if !t.state.meta.Terminal() {
			t.state.meta.Error = strPtr("连接中断，等待对方续传")
			t.state.lastActivity = time.Now()
		}
		t.mu.Unlock()
		m.update(t)
		return err
	}
	if tc, ok := conn.(*net.TCPConn); ok {
		_ = tc.CloseWrite()
	}
	conn.Close()
	_, _ = m.finish(t, "completed", "", nil)
	return nil
}

func remoteIP(addr net.Addr) string {
	if tcp, ok := addr.(*net.TCPAddr); ok {
		return tcp.IP.String()
	}
	host, _, err := net.SplitHostPort(addr.String())
	if err != nil {
		return addr.String()
	}
	return host
}

// withinDir 检查 resolved 是否位于 dir 内部（防目录逃逸）。
func withinDir(resolved, dir string) bool {
	rel, err := filepath.Rel(dir, resolved)
	if err != nil {
		return false
	}
	return rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

// sendBytes 带源文件完整性校验的分段发送（发送前后对比长度与修改时间）。
func (m *Transfers) sendBytes(t *Transfer, conn net.Conn, file *os.File,
	modified time.Time, size, offset, base uint64) error {
	info, err := file.Stat()
	if err != nil {
		return err
	}
	if uint64(info.Size()) != size || !info.ModTime().Equal(modified) {
		return errors.New("源文件已改变")
	}
	sent := offset
	last := time.Now()
	buffer := make([]byte, 64*1024)
	for sent < size {
		if t.isCancelled() {
			return errors.New("文件传输已取消")
		}
		length := size - sent
		if length > uint64(len(buffer)) {
			length = uint64(len(buffer))
		}
		n, err := file.ReadAt(buffer[:length], int64(sent))
		if err != nil && err != io.EOF {
			return err
		}
		if n == 0 {
			return errors.New("源文件长度变化")
		}
		conn.SetWriteDeadline(time.Now().Add(30 * time.Second))
		if err := writeAll(conn, buffer[:n]); err != nil {
			return wrapIO(err)
		}
		sent += uint64(n)
		if time.Since(last) > 100*time.Millisecond {
			m.progress(t, base+sent)
			last = time.Now()
		}
	}
	after, err := file.Stat()
	if err != nil {
		return err
	}
	if uint64(after.Size()) != size || !after.ModTime().Equal(modified) {
		return errors.New("源文件在发送中被修改")
	}
	return nil
}

// Shutdown 停止引擎：取消全部任务、停监听、等主循环退出、清空登记。
func (m *Transfers) Shutdown() {
	if m.stopped.Swap(true) {
		return
	}
	m.gate.Lock()
	defer m.gate.Unlock()
	for _, t := range m.list() {
		t.requestCancel()
	}
	m.stopOnce.Do(func() { close(m.stop) })
	if m.listener != nil {
		m.listener.Close()
	}
	done := make(chan struct{})
	go func() { m.runWG.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
	}
	for _, t := range m.list() {
		m.receiveFailed(t, "程序退出，文件传输中断")
	}
	m.mu.Lock()
	for _, s := range m.selections {
		if s.file != nil {
			s.file.Close()
		}
	}
	for _, t := range m.transfers {
		// Rust 靠 Arc 释放源文件句柄；Go 需要显式关闭。
		if t.source != nil && t.source.file != nil {
			t.source.file.Close()
		}
	}
	m.selections = map[string]*source{}
	m.transfers = map[string]*Transfer{}
	m.mu.Unlock()
}
