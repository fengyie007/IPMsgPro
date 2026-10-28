// Package network 实现网络层：UDP 发现（BR_ENTRY/ANSENTRY）、文本收发（SENDMSG/RECVMSG
// 回执 + 有限重试）、用户表与未读统计。消息持久化通过可选的 store.Database；
// 用户以 来源IP:端口 为键，前端联系人 ID 为 username@hostname#ip:port。
package network

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"log"
	"net"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"my-app/internal/filetransfer"
	"my-app/internal/ipmsg"
	"my-app/internal/media"
	"my-app/internal/store"

	"golang.org/x/text/encoding/simplifiedchinese"
)

type User struct {
	ID       string `json:"id"` // username@hostname#ip:port，作为会话键
	Key      string `json:"key"`
	Nick     string `json:"nickname"`
	Host     string `json:"hostname"`
	Group    string `json:"group"`
	Addr     string `json:"ip"`
	Port     int    `json:"port"`
	Online   bool   `json:"online"`
	LastSeen int64  `json:"-"`
	Avatar   ipmsg.AvatarKind `json:"-"`
	AvatarID int              `json:"-"` // 仅 AvatarBuiltin 时有效
	MAC      string           `json:"-"` // 对端版本头自报 MAC（已归一化）
}

type WireFormat int

const (
	WireGBK WireFormat = iota
	WireUTF8
)

// ChooseWireFormat：对端公告了 UTF-8 能力就用 UTF-8，否则回退 GBK（飞秋默认）。
func ChooseWireFormat(command uint32) WireFormat {
	if command&ipmsg.OptCapUTF8 != 0 {
		return WireUTF8
	}
	return WireGBK
}

func encodeFor(command uint32, text string) ([]byte, error) {
	if ChooseWireFormat(command) == WireUTF8 {
		return []byte(text), nil
	}
	return simplifiedchinese.GBK.NewEncoder().Bytes([]byte(text))
}

type profile struct {
	nick     string
	group    string
	avatar   int
	wireVer  string
	hostname string
}

type pendingSend struct {
	key      string
	dbID     string
	addr     *net.UDPAddr
	wire     []byte
	attempts int
	next     time.Time
}

type Hub struct {
	port       uint16
	instanceID string
	prof       atomic.Pointer[profile]

	conn *net.UDPConn
	db   *store.Database

	files *filetransfer.Transfers
	scan  *scanner
	knocks *knockState

	localID string
	localIP string

	stopped atomic.Bool

	mu      sync.Mutex
	users   map[string]*User
	unread  map[string]int
	pending map[uint32]*pendingSend

	nextPktNo atomic.Uint32

	imagesMu      sync.Mutex
	images        *media.Assembler
	assets        *media.AssetStore
	imageJobs     chan imageJob
	imageSendsMu  sync.Mutex
	imageSends    map[string]*imageSendJob
	imageSendGate sync.Mutex
	imageSequence atomic.Uint32

	UsersChanged      func()
	MessageAcked      func(peerKey, messageID string)
	MessageFailed     func(peerKey, messageID, reason string)
	MessageReceived   func(ReceivedEvent)
	FileEvent         func(event string, payload any)
	ScanEvent         ScanEvent
	ImageFailed       func(peerID, imageID, reason string)
	ImageReceived     func(ReceivedEvent, media.ImageMetadata)
	ImageSendProgress func(messageID, target string, progress int, stage string, image media.ImageMetadata)
	ImageSendFinished func(messageID, target string, success bool, err error, cancelled bool)
}

// ReceivedEvent 是去重后的一条入站文本。
type ReceivedEvent struct {
	Key       string // ip:port
	PeerID    string // username@hostname#ip:port
	User      *User
	ID        string // 历史记录 ID（去重键）
	Content   string
	Type      string // "text"/"knock"（空视为 text）
	Timestamp int64
	Duplicate bool
}

func NewHub(port uint16, nick, group, instanceID string) (*Hub, error) {
	host, err := os.Hostname()
	if err != nil {
		host = "mygo"
	}
	h := &Hub{
		port:       port,
		instanceID: instanceID,
		users:      map[string]*User{},
		unread:     map[string]int{},
		pending:    map[uint32]*pendingSend{},
		scan:       newScanner(),
	}
	h.nextPktNo.Store(uint32(time.Now().UnixNano()) & 0x3fffffff)
	h.imageJobs = make(chan imageJob, 4)
	h.imageSends = map[string]*imageSendJob{}
	h.prof.Store(&profile{nick: nick, group: group, avatar: 0, wireVer: ipmsg.FeiqCompatVersion(instanceID, port, 0), hostname: host})

	addr := &net.UDPAddr{IP: net.IPv4zero, Port: int(port)}
	conn, err := net.ListenUDP("udp4", addr)
	if err != nil {
		return nil, fmt.Errorf("监听 UDP %d 失败（端口被占用？）: %w", port, err)
	}
	if err := setBroadcast(conn); err != nil {
		conn.Close()
		return nil, err
	}
	h.conn = conn
	// 请求端口为 0 时记录实际绑定端口（本地跳过判断依赖它）。
	if h.port == 0 {
		h.port = uint16(conn.LocalAddr().(*net.UDPAddr).Port)
	}
	return h, nil
}

// setBroadcast 打开 SO_BROADCAST；Go 的 net 包不会自动设置，直接 WriteToUDP 广播地址会 EACCES。
func setBroadcast(conn *net.UDPConn) error {
	raw, err := conn.SyscallConn()
	if err != nil {
		return err
	}
	var sockErr error
	ctrlErr := raw.Control(func(fd uintptr) {
		sockErr = syscall.SetsockoptInt(syscall.Handle(fd), syscall.SOL_SOCKET, syscall.SO_BROADCAST, 1)
	})
	if ctrlErr != nil {
		return ctrlErr
	}
	return sockErr
}

// AttachDatabase 启用消息持久化；localID 是本机在历史记录中的用户 ID。
func (h *Hub) AttachDatabase(db *store.Database, localID string) {
	h.db = db
	h.localID = localID
}

// SetLocalIP 记录本机出口 IP（扫描跳过自身用）。
func (h *Hub) SetLocalIP(ip string) { h.localIP = ip }

func (h *Hub) localIPString() string { return h.localIP }

// SetProfile 热更新对外资料；wireVer 随头像编号刷新，下个报文生效。
func (h *Hub) SetProfile(nick string, group string, avatar int) {
	p := *h.prof.Load()
	if nick != "" {
		p.nick = nick
	}
	p.group = group
	if avatar >= 0 && avatar <= 83 {
		p.avatar = avatar
	}
	p.wireVer = ipmsg.FeiqCompatVersion(h.instanceID, h.port, p.avatar)
	h.prof.Store(&p)
}

func (h *Hub) Port() uint16        { return h.port }
func (h *Hub) Nick() string        { return h.prof.Load().nick }
func (h *Hub) Group() string       { return h.prof.Load().group }
func (h *Hub) Hostname() string    { return h.prof.Load().hostname }
func (h *Hub) WireVersion() string { return h.prof.Load().wireVer }

// LocalID 返回本机历史记录用户 ID；未附加数据库时仍有合理默认。
func (h *Hub) LocalID() string {
	if h.localID != "" {
		return h.localID
	}
	return "local@" + h.Hostname()
}

func (h *Hub) Close() {
	h.stopped.Store(true)
	h.ShutdownScan()
	if h.conn != nil {
		h.conn.Close()
	}
}

func randHex(n int) string {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

// buildWire 以固定包号编码报文（重试必须复用原包号与内容）。
func (h *Hub) buildWire(command uint32, pktNo uint32, body string, extra *string) ([]byte, error) {
	p := h.prof.Load()
	wireBody, err := encodeFor(command, body)
	if err != nil {
		return nil, err
	}
	var data bytes.Buffer
	fmt.Fprintf(&data, "%s:%d:%s:%s:%d:", p.wireVer, pktNo,
		gbkOrRaw(command, p.nick), gbkOrRaw(command, p.hostname), command)
	data.Write(wireBody)
	data.WriteByte(0)
	if extra != nil {
		encExtra, err := encodeFor(command, *extra)
		if err != nil {
			return nil, err
		}
		data.Write(encExtra)
		data.WriteByte(0)
	}
	return data.Bytes(), nil
}

func (h *Hub) sendWire(addr *net.UDPAddr, wire []byte) error {
	_, err := h.conn.WriteToUDP(wire, addr)
	return err
}

func gbkOrRaw(command uint32, text string) []byte {
	b, err := encodeFor(command, text)
	if err != nil {
		return []byte(text)
	}
	return b
}

// announce 向所有接口的定向广播地址发 BR_ENTRY / BR_EXIT。
func (h *Hub) announce(command uint32, extra *string) {
	for _, addr := range broadcastTargets(h.port) {
		pktNo := h.nextPktNo.Add(1)
		wire, err := h.buildWire(command, pktNo, h.Nick(), extra)
		if err != nil {
			log.Printf("编码广播报文失败: %v", err)
			continue
		}
		if err := h.sendWire(addr, wire); err != nil {
			log.Printf("广播到 %s 失败: %v", addr, err)
		}
	}
}

// Refresh 重新广播 BR_ENTRY，请求已知用户回应 ANSENTRY。
func (h *Hub) Refresh() { h.announce(ipmsg.CmdBrEntry, &h.prof.Load().group) }

func (h *Hub) AnnounceEntry() { h.announce(ipmsg.CmdBrEntry, &h.prof.Load().group) }

func (h *Hub) AnnounceExit() { h.announce(ipmsg.CmdBrExit, nil) }

func broadcastTargets(port uint16) []*net.UDPAddr {
	targets := []*net.UDPAddr{{IP: net.IPv4bcast, Port: int(port)}} // 受限广播兜底
	ifaces, err := net.Interfaces()
	if err != nil {
		return targets
	}
	seen := map[string]bool{"255.255.255.255": true}
	for _, iface := range ifaces {
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
			continue
		}
		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, a := range addrs {
			ipnet, ok := a.(*net.IPNet)
			if !ok {
				continue
			}
			ip4 := ipnet.IP.To4()
			if ip4 == nil || len(ipnet.Mask) != 4 {
				continue
			}
			bcast := make(net.IP, 4)
			for i := 0; i < 4; i++ {
				bcast[i] = ip4[i] | ^ipnet.Mask[i]
			}
			if seen[bcast.String()] {
				continue
			}
			seen[bcast.String()] = true
			targets = append(targets, &net.UDPAddr{IP: bcast, Port: int(port)})
		}
	}
	return targets
}

// SendText 给联系人 id（username@hostname#ip:port）发送文本：先落库再发送，
// 最多三次尝试（2s/4s 间隔重发同一包号），无回执则置失败并通知。
func (h *Hub) SendText(id, text string) (string, error) {
	h.mu.Lock()
	var target *User
	for _, u := range h.users {
		if u.ID == id {
			target = u
			break
		}
	}
	online := target != nil && target.Online
	key := ""
	if target != nil {
		key = target.Key
	}
	h.mu.Unlock()
	if !online {
		return "", errors.New("联系人不在线")
	}
	addr, err := net.ResolveUDPAddr("udp4", key)
	if err != nil {
		return "", err
	}
	var command uint32 = ipmsg.CmdSendMsg | ipmsg.OptSendCheck
	// 先验证可编码（GBK），再落库，再发送
	if _, err := encodeFor(command, text); err != nil {
		return "", err
	}
	dbID := fmt.Sprintf("%d_%s", time.Now().UnixMilli(), randHex(4))
	if h.db != nil {
		record := store.Record{ID: dbID, FromID: h.LocalID(), ToID: id, Content: text, Kind: 0, Timestamp: time.Now().Unix(), Status: 0}
		if _, err := h.db.Insert(record); err != nil {
			return "", fmt.Errorf("保存消息失败: %w", err)
		}
	}
	pktNo := h.nextPktNo.Add(1)
	wire, err := h.buildWire(command, pktNo, text, nil)
	if err != nil {
		if h.db != nil {
			_ = h.db.InterruptSending(dbID)
		}
		return "", err
	}
	if err := h.sendWire(addr, wire); err != nil {
		if h.db != nil {
			_ = h.db.InterruptSending(dbID)
		}
		return "", err
	}
	h.mu.Lock()
	h.pending[pktNo] = &pendingSend{key: key, dbID: dbID, addr: addr, wire: wire, attempts: 1, next: time.Now().Add(2 * time.Second)}
	h.mu.Unlock()
	return dbID, nil
}

func (h *Hub) Unread(key string) int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.unread[key]
}

func (h *Hub) TotalUnread() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	total := 0
	for _, n := range h.unread {
		total += n
	}
	return total
}

// MarkRead 清空某联系人的未读，返回清掉的条数。
func (h *Hub) MarkRead(key string) int {
	h.mu.Lock()
	defer h.mu.Unlock()
	n := h.unread[key]
	delete(h.unread, key)
	return n
}

func (h *Hub) ListUsers() []User {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]User, 0, len(h.users))
	for _, u := range h.users {
		out = append(out, *u)
	}
	return out
}

func (h *Hub) Run() {
	go h.retryLoop()
	go h.imageWorker()

	go func() {
		buf := make([]byte, ipmsg.MaxPacketBytes)
		for {
			n, src, err := h.conn.ReadFromUDP(buf)
			if err != nil {
				if strings.Contains(err.Error(), "use of closed network connection") {
					return
				}
				log.Printf("UDP 接收错误: %v", err)
				time.Sleep(100 * time.Millisecond)
				continue
			}
			if h.isSelf(src) {
				continue
			}
			data := make([]byte, n)
			copy(data, buf[:n])
			h.handle(data, src)
		}
	}()
}

// retryLoop 重发未回执的发送：attempts 2、3 间隔 2s/4s，之后置失败。
func (h *Hub) retryLoop() {
	ticker := time.NewTicker(500 * time.Millisecond)
	defer ticker.Stop()
	for range ticker.C {
		now := time.Now()
		h.mu.Lock()
		var failed []*pendingSend
		for pktNo, entry := range h.pending {
			if entry.next.After(now) {
				continue
			}
			if entry.attempts >= 3 {
				delete(h.pending, pktNo)
				failed = append(failed, entry)
				continue
			}
			if err := h.sendWire(entry.addr, entry.wire); err != nil {
				log.Printf("重发到 %s 失败: %v", entry.addr, err)
			}
			entry.attempts++
			entry.next = now.Add(2 * time.Duration(entry.attempts-1) * time.Second)
		}
		h.mu.Unlock()
		for _, entry := range failed {
			if h.db != nil {
				_ = h.db.InterruptSending(entry.dbID)
			}
			if h.MessageFailed != nil {
				h.MessageFailed(entry.key, entry.dbID, "对方未确认接收")
			}
		}
	}
}

// isSelf 过滤自己发出的广播回环：同 IP 且同端口。
func (h *Hub) isSelf(src *net.UDPAddr) bool {
	if src.Port != int(h.port) {
		return false
	}
	ip4 := src.IP.To4()
	if ip4 == nil {
		return false
	}
	ifaces, err := net.Interfaces()
	if err != nil {
		return false
	}
	for _, iface := range ifaces {
		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, a := range addrs {
			ipnet, ok := a.(*net.IPNet)
			if !ok {
				continue
			}
			if local := ipnet.IP.To4(); local != nil && local.Equal(ip4) {
				return true
			}
		}
	}
	return false
}

func (h *Hub) handle(data []byte, src *net.UDPAddr) {
	pkt, err := ipmsg.ParsePacket(data)
	if err != nil {
		log.Printf("丢弃来自 %s 的报文: %v", src, err)
		return
	}
	key := src.String()
	mode := ipmsg.CommandMode(pkt.Command)
	// RECVMSG / RELEASEFILES 先给文件引擎认领（回执确认/拒收通知），
	// 文本发送重试仍走 pending 表。
	if mode == ipmsg.CmdRecvMsg || mode == ipmsg.CmdReleaseFiles {
		if h.files != nil {
			h.files.Receipt(pkt, src)
		}
	}
	switch mode {
	case ipmsg.CmdBrEntry:
		h.upsertUser(key, pkt, src)
		h.scanResponse(src)
		if _, err := h.buildWireAndSend(src, ipmsg.CmdAnsEntry, h.Nick(), &h.prof.Load().group); err != nil {
			log.Printf("ANSENTRY 发送失败: %v", err)
		}
	case ipmsg.CmdAnsEntry:
		h.upsertUser(key, pkt, src)
		h.scanResponse(src)
	case ipmsg.CmdBrExit:
		h.mu.Lock()
		if u, ok := h.users[key]; ok {
			u.Online = false
			u.LastSeen = time.Now().Unix()
		}
		h.mu.Unlock()
		h.notifyUsersChanged()
	case ipmsg.CmdSendMsg:
		if pkt.Command&ipmsg.OptEncrypt != 0 {
			return
		}
		h.upsertUser(key, pkt, src)
		if h.handleFileIncoming(key, pkt, src) {
			return
		}
		// 文件服务未启动时按占位文本展示；绝不把文件邀请回执为已接受。
		unsupportedFile := pkt.Command&ipmsg.OptFileAttach != 0
		if unsupportedFile {
			pkt.Body = "[收到文件：当前版本暂不支持，请使用原版接收]"
		}
		h.receiveText(key, pkt, src, !unsupportedFile)
	case ipmsg.CmdKnock:
		if err := h.receiveKnock(key, pkt, src); err != nil {
			log.Printf("窗口振动处理失败: %v", err)
		}
	case ipmsg.CmdSendImage:
		if err := h.handleImageFragment(pkt, src, key); err != nil {
			log.Printf("图片分片处理失败: %v", err)
		}
	case ipmsg.CmdReportImage:
		// 分片确认：认领属于某个在途图片发送
		h.imageSendAck(pkt, key)
	case ipmsg.CmdRecvMsg:
		// 图片引用回执优先由发送链路认领（RECVMSG body=包号）
		if h.imageSendAck(pkt, key) {
			return
		}
		pktNo64, err := strconv.ParseUint(strings.TrimSpace(pkt.Body), 10, 32)
		if err != nil {
			return
		}
		h.mu.Lock()
		entry, ok := h.pending[uint32(pktNo64)]
		if ok {
			delete(h.pending, uint32(pktNo64))
		}
		h.mu.Unlock()
		if !ok {
			return
		}
		if h.db != nil {
			if err := h.db.Status(entry.dbID, 1); err != nil {
				log.Printf("回执状态更新失败: %v", err)
			}
		}
		if h.MessageAcked != nil {
			h.MessageAcked(key, entry.dbID)
		}
	}
}

func (h *Hub) buildWireAndSend(src *net.UDPAddr, command uint32, body string, extra *string) (uint32, error) {
	pktNo := h.nextPktNo.Add(1)
	wire, err := h.buildWire(command, pktNo, body, extra)
	if err != nil {
		return 0, err
	}
	return pktNo, h.sendWire(src, wire)
}

// receiveText 落库去重后投递事件；重复报文始终回执但不重复计数。
// ack=false 时跳过 RECVMSG（文件邀请占位文本，见 handle 的 unsupported 分支）。
func (h *Hub) receiveText(key string, pkt ipmsg.Packet, src *net.UDPAddr, ack bool) {
	peerID := fmt.Sprintf("%s@%s#%s", pkt.Username, pkt.Hostname, key)
	recordID := fmt.Sprintf("rx:%s:%d", key, pkt.PacketNo)
	ts := time.Now().Unix()
	duplicate := false
	if h.db != nil {
		record := store.Record{ID: recordID, FromID: peerID, ToID: h.LocalID(), Content: pkt.Body, Kind: 0, Timestamp: ts, Status: 1}
		inserted, err := h.db.Insert(record)
		if err != nil {
			log.Printf("接收消息落库失败: %v", err)
		} else {
			duplicate = !inserted
		}
	}
	if !duplicate {
		h.mu.Lock()
		h.unread[key]++
		h.mu.Unlock()
	}
	if ack {
		if _, err := h.buildWireAndSend(src, ipmsg.CmdRecvMsg, strconv.FormatUint(uint64(pkt.PacketNo), 10), nil); err != nil {
			log.Printf("RECVMSG 回执发送失败: %v", err)
		}
	}
	if h.MessageReceived != nil {
		h.mu.Lock()
		user := h.users[key]
		h.mu.Unlock()
		h.MessageReceived(ReceivedEvent{
			Key: key, PeerID: peerID, User: user, ID: recordID,
			Content: pkt.Body, Timestamp: ts, Duplicate: duplicate,
		})
	}
	// 文本中的图片引用：引用可先于/后于分片到达，只登记不产生新消息
	if imageID := media.ParseReference(pkt.Body); imageID != "" {
		if err := h.registerImageReference(peerID, imageID); err != nil {
			log.Printf("图片引用登记失败: %v", err)
		}
	}
}

func (h *Hub) upsertUser(key string, pkt ipmsg.Packet, src *net.UDPAddr) {
	changed := false
	h.mu.Lock()
	u, ok := h.users[key]
	if !ok {
		u = &User{Key: key, Port: src.Port}
		h.users[key] = u
		changed = true
	}
	if u.Nick != pkt.Username || u.Group != pkt.Extra || u.Host != pkt.Hostname || !u.Online {
		changed = true
	}
	u.Nick = pkt.Username
	u.Host = pkt.Hostname
	u.Group = pkt.Extra
	u.Addr = src.IP.String()
	u.Port = src.Port
	u.ID = fmt.Sprintf("%s@%s#%s", u.Nick, u.Host, key)
	u.Online = true
	u.LastSeen = time.Now().Unix()
	// 头像与 MAC 只信版本头；解析失败保留上一次的值（对拍 Rust is_profile 语义）。
	if kind, id := ipmsg.ParseAvatar(pkt.Version); kind != ipmsg.AvatarNone {
		u.Avatar, u.AvatarID = kind, id
	}
	if mac := ipmsg.FeiqMac(pkt.Version); mac != "" {
		u.MAC = mac
	}
	h.mu.Unlock()
	if changed {
		h.notifyUsersChanged()
	}
}

func (h *Hub) notifyUsersChanged() {
	if h.UsersChanged != nil {
		h.UsersChanged()
	}
}

// ImagesEnabled 报告收图链路是否已启用（AttachAssets 后为真）。
func (h *Hub) ImagesEnabled() bool { return h.assets != nil && h.images != nil }
