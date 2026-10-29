// knock.go 对拍 tauri-rust network/knock.rs：窗口振动（IPMSG_KNOCK 0xD1）。
// FeiQ 的振动为空 body、不要求回执，也绝不重试（对端可能不去重特效）。
package network

import (
	"fmt"
	"net"
	"strconv"
	"sync"
	"time"

	"speedipmsg-go/internal/ipmsg"
	"speedipmsg-go/internal/store"
)

const (
	knockCooldown = 10 * time.Second
	knockLimit    = 4096
)

type knockState struct {
	mu   sync.Mutex
	sent map[string]time.Time
}

// SendKnock 向目标用户发送窗口振动；同一目标 10 秒内只允许一次。
func (h *Hub) SendKnock(target string) (store.Record, error) {
	if h.knocks == nil {
		h.knocks = &knockState{sent: map[string]time.Time{}}
	}
	if h.stopped.Load() {
		return store.Record{}, fmt.Errorf("程序正在退出")
	}
	user := h.UserByID(target)
	if user == nil {
		return store.Record{}, fmt.Errorf("未找到对端，请先刷新联系人")
	}
	if !user.Online {
		return store.Record{}, fmt.Errorf("对方已离线，不能发送窗口振动")
	}
	h.knocks.mu.Lock()
	for key, at := range h.knocks.sent {
		if time.Since(at) >= knockCooldown {
			delete(h.knocks.sent, key)
		}
	}
	if _, busy := h.knocks.sent[target]; busy {
		h.knocks.mu.Unlock()
		return store.Record{}, fmt.Errorf("发送窗口振动过于频繁，请等待10秒后再试")
	}
	if len(h.knocks.sent) >= knockLimit {
		h.knocks.mu.Unlock()
		return store.Record{}, fmt.Errorf("窗口振动发送过于频繁")
	}
	h.knocks.sent[target] = time.Now()
	h.knocks.mu.Unlock()

	packet := h.nextPktNo.Add(1)
	// KNOCK 不在文本帧命令表内：body 为空且不带结尾 NUL，与飞秋一致。
	wire, err := h.buildWire(ipmsg.CmdKnock, packet, "", nil)
	if err != nil {
		return store.Record{}, err
	}
	record := store.Record{
		ID:        fmt.Sprintf("%d_%d", time.Now().UnixMilli(), packet),
		FromID:    h.LocalID(),
		ToID:      user.ID,
		Content:   "[窗口振动]",
		Kind:      3,
		Timestamp: time.Now().Unix(),
		Status:    0,
	}
	if _, err := h.db.Insert(record); err != nil {
		return store.Record{}, err
	}
	status := int64(1) // 对 KNOCK 而言 1 表示"已发出"，不是"已送达"
	if err := h.sendWire(&net.UDPAddr{IP: net.ParseIP(user.Addr), Port: user.Port}, wire); err != nil {
		status = 3
	}
	if err := h.db.Status(record.ID, status); err != nil {
		return store.Record{}, err
	}
	record.Status = status
	return record, nil
}

// receiveKnock 处理对端振动：空 body、0～2 个结尾 NUL 都接受（旧版兼容）。
func (h *Hub) receiveKnock(key string, pkt ipmsg.Packet, src *net.UDPAddr) error {
	if len(pkt.Body) > 2 || !allZeroBytes(pkt.Body) || pkt.Extra != "" {
		return fmt.Errorf("无效的窗口振动报文")
	}
	h.upsertUser(key, pkt, src)
	h.mu.Lock()
	user := h.users[key]
	h.mu.Unlock()
	if user == nil {
		return fmt.Errorf("未知的振动来源")
	}
	peerID := user.ID
	id := fmt.Sprintf("%s:%d", peerID, pkt.PacketNo)
	timestamp := time.Now().Unix()
	record := store.Record{
		ID: id, FromID: peerID, ToID: h.LocalID(),
		Content: "[窗口振动]", Kind: 3, Timestamp: timestamp, Status: 1,
	}
	inserted, err := h.db.Insert(record)
	if err != nil {
		return err
	}
	if inserted && h.MessageReceived != nil {
		h.MessageReceived(ReceivedEvent{
			Key: key, PeerID: peerID, User: user, ID: id,
			Content: "[窗口振动]", Type: "knock", Timestamp: timestamp,
		})
	}
	if pkt.Command&ipmsg.OptSendCheck != 0 {
		if _, err := h.buildWireAndSend(src, ipmsg.CmdRecvMsg, strconv.FormatUint(uint64(pkt.PacketNo), 10), nil); err != nil {
			return err
		}
	}
	return nil
}

func allZeroBytes(body string) bool {
	for i := 0; i < len(body); i++ {
		if body[i] != 0 {
			return false
		}
	}
	return true
}
