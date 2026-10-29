// files.go 把文件传输引擎接入 Hub：实现 filetransfer.Link（复用 UDP socket、
// 包号分配与线缆编码），并暴露 SendFile / FileEvent 给命令层。
package network

import (
	"fmt"
	"net"

	"my-app/internal/filetransfer"
	"my-app/internal/ipmsg"
	"my-app/internal/store"
)

// FileEvent 由传输引擎发来的领域事件（file.updated / message.received）。
type FileEvent func(event string, payload any)

// hubLink 让文件引擎借用 Hub 的 UDP 发送与线缆编码（对拍 Rust 共享 socket 的做法）。
type hubLink struct{ h *Hub }

func (l hubLink) NextPacketNo() uint32 { return l.h.nextPktNo.Add(1) }

func (l hubLink) BuildWire(pktNo uint32, command uint32, body string, extra *string) ([]byte, error) {
	return l.h.buildWire(command, pktNo, body, extra)
}

func (l hubLink) SendUDP(addr *net.UDPAddr, wire []byte) error {
	return l.h.sendWire(addr, wire)
}

func (l hubLink) LocalID() string { return l.h.LocalID() }

// StartFiles 在与 UDP 相同的端口上启动 TCP 文件服务；root 为接收目录。
func (h *Hub) StartFiles(root string) error {
	if h.files != nil {
		return fmt.Errorf("文件服务已启动")
	}
	transfers, err := filetransfer.Start(hubLink{h}, h.db, func(event string, payload any) {
		if h.FileEvent != nil {
			h.FileEvent(event, payload)
		}
	}, root, h.port)
	if err != nil {
		return err
	}
	h.files = transfers
	return nil
}

func (h *Hub) FilesEnabled() bool { return h.files != nil }

func (h *Hub) Files() (*filetransfer.Transfers, error) {
	if h.files == nil {
		return nil, fmt.Errorf("文件服务未启动")
	}
	return h.files, nil
}

// UserByID 按会话键（username@hostname#ip:port）查找用户。
func (h *Hub) UserByID(id string) *User {
	h.mu.Lock()
	defer h.mu.Unlock()
	for _, u := range h.users {
		if u.ID == id {
			return u
		}
	}
	return nil
}

// peerOf 把网络用户转成文件引擎的对端视图。
func (h *Hub) peerOf(u *User) filetransfer.Peer {
	return filetransfer.Peer{
		ID: u.ID, Key: u.Key, Nick: u.Nick, UserName: u.UserName, Host: u.Host,
		Group: u.Group, Addr: u.Addr, Port: u.Port, Online: u.Online,
	}
}

// SendFile 把一个已预览的文件/文件夹发送给目标用户（对拍 Rust network.send_file）。
func (h *Hub) SendFile(target, selection string) (store.Record, error) {
	transfers, err := h.Files()
	if err != nil {
		return store.Record{}, err
	}
	user := h.UserByID(target)
	if user == nil {
		return store.Record{}, fmt.Errorf("未找到目标用户")
	}
	return transfers.Send(h.peerOf(user), selection)
}

// handleFileIncoming 处理带 FILEATTACH 的 SENDMSG：文件邀请交给引擎，
// 返回值表示报文已被文件链路消费（true），调用方不应再按文本处理。
func (h *Hub) handleFileIncoming(key string, pkt ipmsg.Packet, src *net.UDPAddr) bool {
	if pkt.Command&ipmsg.OptFileAttach == 0 || h.files == nil {
		return false
	}
	h.mu.Lock()
	user := h.users[key]
	h.mu.Unlock()
	if user == nil {
		return false
	}
	if err := h.files.Incoming(h.peerOf(user), pkt); err != nil {
		if h.FileEvent != nil {
			h.FileEvent("file.receive_failed", map[string]any{"error": err.Error()})
		}
	}
	return true
}
