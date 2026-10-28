// Package filetransfer 移植 tauri-rust crates/ipmsg-core/src/file/：
// protocol.go 对应 protocol.rs（邀请解析/文件名净化/TCP 请求解析）。
package filetransfer

import (
	"fmt"
	"strconv"
	"strings"

	"my-app/internal/ipmsg"
)

const MaxFileSize = 8 * 1024 * 1024 * 1024

type Offer struct {
	Directory bool
	ID        uint32
	Name      string
	Size      uint64
}

// SanitizeName 产出安全的 Windows 叶文件名（对照 protocol.rs sanitize_name）。
func SanitizeName(name string) string {
	runes := []rune(name)
	if len(runes) > 180 {
		runes = runes[:180]
	}
	var builder strings.Builder
	for _, c := range runes {
		if c < 0x20 || c == 0x7f || strings.ContainsRune("<>:\"/\\|?*", c) {
			builder.WriteRune('_')
		} else {
			builder.WriteRune(c)
		}
	}
	leaf := strings.Trim(builder.String(), " .")
	if leaf == "" {
		leaf = "未命名文件"
	}
	stem := strings.ToUpper(strings.TrimRight(strings.SplitN(leaf, ".", 2)[0], " "))
	reserved := map[string]bool{"CON": true, "PRN": true, "AUX": true, "NUL": true, "CLOCK$": true, "CONIN$": true, "CONOUT$": true}
	if reserved[stem] ||
		(len(stem) > 3 && (strings.HasPrefix(stem, "COM") || strings.HasPrefix(stem, "LPT")) &&
			strings.Contains("123456789¹²³", stem[3:])) {
		leaf = "_" + leaf
	}
	return leaf
}

func hexField(value string) (uint64, error) {
	if value == "" || len(value) > 16 || !allHex(value) {
		return 0, fmt.Errorf("非法文件数值")
	}
	parsed, err := strconv.ParseUint(value, 16, 64)
	if err != nil {
		return 0, fmt.Errorf("文件数值溢出")
	}
	return parsed, nil
}

func allHex(s string) bool {
	for i := 0; i < len(s); i++ {
		c := s[i]
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f' || c >= 'A' && c <= 'F') {
			return false
		}
	}
	return true
}

// Offers 解析 SENDMSG|FILEATTACHOPT 报文 extra 里的文件邀请列表。
func Offers(pkt ipmsg.Packet) ([]Offer, error) {
	text := pkt.Extra
	var offers []Offer
	ids := map[uint32]bool{}
	for _, row := range strings.Split(text, "\x07") {
		if row == "" {
			continue
		}
		if len(offers) >= 16 {
			return nil, fmt.Errorf("一次最多接收16个文件邀请")
		}
		idText, rest, ok := strings.Cut(row, ":")
		if !ok {
			return nil, fmt.Errorf("文件邀请缺少ID")
		}
		id64, err := strconv.ParseUint(idText, 10, 32)
		if err != nil {
			return nil, fmt.Errorf("非法文件ID")
		}
		id := uint32(id64)
		if ids[id] {
			return nil, fmt.Errorf("文件ID重复")
		}
		ids[id] = true
		// 名称中的 "::" 是转义冒号
		var name strings.Builder
		ended := false
		for i := 0; i < len(rest); i++ {
			if rest[i] == ':' {
				if i+1 < len(rest) && rest[i+1] == ':' {
					name.WriteByte(':')
					i++
					continue
				}
				ended = true
				rest = rest[i+1:]
				break
			}
			name.WriteByte(rest[i])
		}
		if !ended || name.Len() == 0 || name.Len() > 1024 {
			return nil, fmt.Errorf("文件名无效")
		}
		fields := strings.Split(rest, ":")
		if len(fields) < 3 {
			return nil, fmt.Errorf("文件邀请被截断")
		}
		size, err := hexField(fields[0])
		if err != nil {
			return nil, err
		}
		if _, err := hexField(fields[1]); err != nil {
			return nil, err
		}
		attr, err := hexField(fields[2])
		if err != nil {
			return nil, err
		}
		if size > MaxFileSize {
			return nil, fmt.Errorf("文件超过8 GiB上限")
		}
		if kind := attr & 0xff; kind != 1 && kind != 2 {
			return nil, fmt.Errorf("不支持特殊文件")
		}
		offers = append(offers, Offer{
			Directory: attr&0xff == 2,
			ID:        id,
			Name:      SanitizeName(name.String()),
			Size:      size,
		})
	}
	if len(offers) == 0 {
		return nil, fmt.Errorf("文件邀请为空")
	}
	return offers, nil
}

// RequestPacket 解析 TCP 上的 GETFILEDATA / GETDIRFILES 请求，
// 返回完整包（发送方需用 username/hostname/command 校验请求者身份）、
// 原始包号、文件 ID 与偏移（目录请求偏移恒 0）。
func RequestPacket(data []byte) (pkt ipmsg.Packet, originalPacketNo uint32, fileID uint32, offset uint64, err error) {
	pkt, err = ipmsg.ParsePacket(data)
	if err != nil {
		return
	}
	mode := ipmsg.CommandMode(pkt.Command)
	if mode != ipmsg.CmdGetFileData && mode != ipmsg.CmdGetDirFiles {
		return pkt, 0, 0, 0, fmt.Errorf("不支持的TCP命令")
	}
	directory := mode == ipmsg.CmdGetDirFiles
	body := pkt.Body
	if body == "" {
		body = pkt.Extra
	}
	fields := strings.Split(body, ":")
	valid := len(fields) == 4 && fields[3] == "" ||
		directory && len(fields) == 3 && fields[2] == ""
	if !valid {
		return pkt, 0, 0, 0, fmt.Errorf("TCP请求不完整")
	}
	original64, err := hexField(fields[0])
	if err != nil {
		return pkt, 0, 0, 0, fmt.Errorf("包号溢出")
	}
	if original64 > 0xFFFFFFFF {
		return pkt, 0, 0, 0, fmt.Errorf("包号溢出")
	}
	file64, err := hexField(fields[1])
	if err != nil {
		return pkt, 0, 0, 0, fmt.Errorf("文件ID溢出")
	}
	if file64 > 0xFFFFFFFF {
		return pkt, 0, 0, 0, fmt.Errorf("文件ID溢出")
	}
	offset = 0
	if !directory {
		offset, err = hexField(fields[2])
		if err != nil {
			return pkt, 0, 0, 0, err
		}
	}
	return pkt, uint32(original64), uint32(file64), offset, nil
}

// Request 只关心请求三元组的旧接口。
func Request(data []byte) (originalPacketNo uint32, fileID uint32, offset uint64, err error) {
	_, originalPacketNo, fileID, offset, err = RequestPacket(data)
	return
}
