package main

// 最小闭环网络层：UDP 发现（BR_ENTRY/ANSENTRY）、文本收发（SENDMSG/RECVMSG 回执）、
// 用户表与未读统计。消息历史只存内存；用户以 来源IP:端口 为键，同机多实例互不合并。

import (
	"bytes"
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

	"golang.org/x/text/encoding/simplifiedchinese"
)

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

type User struct {
	Key      string `json:"key"` // ip:port
	Nick     string `json:"nick"`
	Host     string `json:"host"`
	Group    string `json:"group"`
	Addr     string `json:"addr"`
	Online   bool   `json:"online"`
	LastSeen int64  `json:"lastSeen"`
}

type Message struct {
	ID    uint64 `json:"id"`
	Out   bool   `json:"out"`
	Text  string `json:"text"`
	Time  int64  `json:"time"`
	Acked bool   `json:"acked"` // 对端 RECVMSG 回执（仅出站方向有意义）
}

// chooseWireFormat：对端公告了 UTF-8 能力就用 UTF-8，否则回退 GBK（飞秋默认）。
func chooseWireFormat(command uint32) WireFormat {
	if command&optCapUTF8 != 0 {
		return wireUTF8
	}
	return wireGBK
}

type WireFormat int

const (
	wireGBK WireFormat = iota
	wireUTF8
)

func encodeFor(command uint32, text string) ([]byte, error) {
	if chooseWireFormat(command) == wireUTF8 {
		return []byte(text), nil
	}
	return simplifiedchinese.GBK.NewEncoder().Bytes([]byte(text))
}

type Hub struct {
	port     uint16
	nick     string
	group    string
	hostname string
	wireVer  string

	conn *net.UDPConn

	mu        sync.Mutex
	users     map[string]*User
	history   map[string][]Message
	unread    map[string]int
	pending   map[uint32]*Message // 已发送、等待 RECVMSG 回执的出站消息
	nextMsgID atomic.Uint64
	nextPktNo atomic.Uint32

	UsersChanged    func()
	ReceivedMessage func(peerKey string)
}

func NewHub(port uint16, nick, group, instanceID string) (*Hub, error) {
	host, err := os.Hostname()
	if err != nil {
		host = "mygo"
	}
	h := &Hub{
		port:     port,
		nick:     nick,
		group:    group,
		hostname: host,
		wireVer:  feiqCompatVersion(instanceID, port),
		users:    map[string]*User{},
		history:  map[string][]Message{},
		unread:   map[string]int{},
		pending:  map[uint32]*Message{},
	}
	h.nextPktNo.Store(uint32(time.Now().UnixNano()) & 0x3fffffff)

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
	return h, nil
}

func (h *Hub) Port() uint16        { return h.port }
func (h *Hub) WireVersion() string { return h.wireVer }

func (h *Hub) Close() {
	if h.conn != nil {
		h.conn.Close()
	}
}

// writePacket 编码并以本次新的包号发送；返回包号供回执匹配。
func (h *Hub) writePacket(addr *net.UDPAddr, command uint32, body string, extra *string) (uint32, error) {
	pktNo := h.nextPktNo.Add(1)
	wireBody, err := encodeFor(command, body)
	if err != nil {
		return 0, err
	}
	var data bytes.Buffer
	fmt.Fprintf(&data, "%s:%d:%s:%s:%d:", h.wireVer, pktNo,
		gbkOrRaw(command, h.nick), gbkOrRaw(command, h.hostname), command)
	data.Write(wireBody)
	data.WriteByte(0)
	if extra != nil {
		encExtra, err := encodeFor(command, *extra)
		if err != nil {
			return 0, err
		}
		data.Write(encExtra)
		data.WriteByte(0)
	}
	if _, err := h.conn.WriteToUDP(data.Bytes(), addr); err != nil {
		return 0, err
	}
	return pktNo, nil
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
		if _, err := h.writePacket(addr, command, h.nick, extra); err != nil {
			log.Printf("广播到 %s 失败: %v", addr, err)
		}
	}
}

// Refresh 重新广播 BR_ENTRY，请求已知用户回应 ANSENTRY。
func (h *Hub) Refresh() { h.announce(cmdBrEntry, &h.group) }

func (h *Hub) AnnounceEntry() { h.announce(cmdBrEntry, &h.group) }

func (h *Hub) AnnounceExit() { h.announce(cmdBrExit, nil) }

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

// SendText 给联系人 key（ip:port）发送文本，立即记入历史并等待回执。
func (h *Hub) SendText(key, text string) error {
	h.mu.Lock()
	user, ok := h.users[key]
	online := ok && user.Online
	h.mu.Unlock()
	if !online {
		return errors.New("联系人不在线")
	}
	addr, err := net.ResolveUDPAddr("udp4", key)
	if err != nil {
		return err
	}
	var command uint32 = cmdSendMsg | optSendCheck
	msg := Message{ID: h.nextMsgID.Add(1), Out: true, Text: text, Time: time.Now().UnixMilli()}

	h.mu.Lock()
	h.history[key] = append(h.history[key], msg)
	track := &h.history[key][len(h.history[key])-1]
	h.mu.Unlock()

	pktNo, err := h.writePacket(addr, command, text, nil)
	if err != nil {
		// 发送失败：从历史里移除这条记录
		h.mu.Lock()
		list := h.history[key]
		for i, m := range list {
			if m.ID == msg.ID {
				h.history[key] = append(list[:i:i], list[i+1:]...)
				break
			}
		}
		h.mu.Unlock()
		return err
	}
	h.mu.Lock()
	h.pending[pktNo] = track
	h.mu.Unlock()
	return nil
}

func (h *Hub) History(key string) []Message {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]Message, len(h.history[key]))
	copy(out, h.history[key])
	return out
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
	go func() {
		buf := make([]byte, maxPacketBytes)
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
	pkt, err := parsePacket(data)
	if err != nil {
		log.Printf("丢弃来自 %s 的报文: %v", src, err)
		return
	}
	key := src.String()
	switch commandMode(pkt.command) {
	case cmdBrEntry:
		h.upsertUser(key, pkt, src)
		if _, err := h.writePacket(src, cmdAnsEntry, h.nick, &h.group); err != nil {
			log.Printf("ANSENTRY 发送失败: %v", err)
		}
	case cmdAnsEntry:
		h.upsertUser(key, pkt, src)
	case cmdBrExit:
		h.mu.Lock()
		if u, ok := h.users[key]; ok {
			u.Online = false
			u.LastSeen = time.Now().UnixMilli()
		}
		h.mu.Unlock()
		h.notifyUsersChanged()
	case cmdSendMsg:
		h.upsertUser(key, pkt, src)
		msg := Message{ID: h.nextMsgID.Add(1), Out: false, Text: pkt.body, Time: time.Now().UnixMilli()}
		h.mu.Lock()
		h.history[key] = append(h.history[key], msg)
		h.unread[key]++
		h.mu.Unlock()
		// 回 RECVMSG，body 为对方包号十进制文本
		if _, err := h.writePacket(src, cmdRecvMsg, strconv.FormatUint(uint64(pkt.packetNo), 10), nil); err != nil {
			log.Printf("RECVMSG 回执发送失败: %v", err)
		}
		if h.ReceivedMessage != nil {
			h.ReceivedMessage(key)
		}
	case cmdRecvMsg:
		pktNo64, err := strconv.ParseUint(strings.TrimSpace(pkt.body), 10, 32)
		if err != nil {
			return
		}
		h.mu.Lock()
		if m, ok := h.pending[uint32(pktNo64)]; ok {
			m.Acked = true
			delete(h.pending, uint32(pktNo64))
		}
		h.mu.Unlock()
		if h.ReceivedMessage != nil {
			h.ReceivedMessage(key)
		}
	}
}

func (h *Hub) upsertUser(key string, pkt packet, src *net.UDPAddr) {
	changed := false
	h.mu.Lock()
	u, ok := h.users[key]
	if !ok {
		u = &User{Key: key}
		h.users[key] = u
		changed = true
	}
	if u.Nick != pkt.username || u.Group != pkt.extra || u.Host != pkt.hostname || !u.Online {
		changed = true
	}
	u.Nick = pkt.username
	u.Host = pkt.hostname
	u.Group = pkt.extra
	u.Addr = src.IP.String()
	u.Online = true
	u.LastSeen = time.Now().UnixMilli()
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
