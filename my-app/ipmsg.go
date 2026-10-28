package main

// IPMsg v1 线上协议的最小编解码，报文格式与常量和 tauri-rust/crates/ipmsg-core/src/protocol.rs
// 保持一致（该实现已通过真实飞秋互通验证）。

import (
	"bytes"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"golang.org/x/text/encoding/simplifiedchinese"
)

const (
	ipmsgDefaultPort = 2425

	cmdNoOperation   = 0x00
	cmdBrEntry       = 0x01
	cmdBrExit        = 0x02
	cmdAnsEntry      = 0x03
	cmdBrAbsence     = 0x04
	cmdSendMsg       = 0x20
	cmdRecvMsg       = 0x21
	optSendCheck     = 0x00000100
	optAbsence       = 0x00000100
	optFileAttach    = 0x00200000
	optEncrypt       = 0x00400000
	optUTF8          = 0x00800000
	optCapUTF8       = 0x01000000
	maxPacketBytes   = 65507
	maxTextBytes     = 32 * 1024
	maxIdentityBytes = 512
	maxVersionBytes  = 128
	commandModeMask  = 0xff
)

// textFramedCommands 的 body/extra 按 NUL 分段；图片等二进制命令不在最小闭环范围内。
var textFramedCommands = map[uint32]bool{
	cmdNoOperation: true,
	cmdBrEntry:     true,
	cmdBrExit:      true,
	cmdAnsEntry:    true,
	cmdBrAbsence:   true,
	cmdSendMsg:     true,
	cmdRecvMsg:     true,
	0x30:           true, // READMSG
	0x31:           true, // DELMSG
	0x32:           true, // ANSREADMSG
	0x40:           true, // GETINFO
	0x41:           true, // SENDINFO
	0x72:           true, // GETPUBKEY
	0x73:           true, // ANSPUBKEY
	0xc1:           true, // REPORT_RECVIMAGE
}

func commandMode(command uint32) uint32 { return command & commandModeMask }

type packet struct {
	version  string
	packetNo uint32
	username string
	hostname string
	command  uint32
	body     string
	extra    string
}

// decodeWireText：UTF8OPT 时按 UTF-8，否则按 GBK（与对端飞秋/原版一致）。
func decodeWireText(b []byte, command uint32) (string, error) {
	if command&optUTF8 != 0 {
		return string(b), nil
	}
	dec := simplifiedchinese.GBK.NewDecoder()
	out, err := dec.Bytes(b)
	if err != nil {
		return "", fmt.Errorf("无效的 GBK 文本: %w", err)
	}
	return string(out), nil
}

func encodeWireText(text string, command uint32) ([]byte, error) {
	if strings.ContainsRune(text, 0) {
		return nil, errors.New("文本不能包含 NUL 字符")
	}
	if command&optUTF8 != 0 {
		return []byte(text), nil
	}
	enc := simplifiedchinese.GBK.NewEncoder()
	out, err := enc.Bytes([]byte(text))
	if err != nil {
		return nil, fmt.Errorf("文本包含 GBK 无法表示的字符: %w", err)
	}
	return out, nil
}

func validateIdentity(value, name string) error {
	if value == "" {
		return fmt.Errorf("%s 不能为空", name)
	}
	if strings.ContainsAny(value, ":") {
		return fmt.Errorf("%s 不能包含冒号", name)
	}
	for _, r := range value {
		if r < 0x20 || r == 0x7f {
			return fmt.Errorf("%s 不能包含控制字符", name)
		}
	}
	return nil
}

func parseVersion(b []byte) (string, error) {
	if len(b) == 0 || len(b) > maxVersionBytes {
		return "", errors.New("无效的 IPMsg 版本字段")
	}
	for _, c := range b {
		if c <= 0x20 || c >= 0x7f {
			return "", errors.New("无效的 IPMsg 版本字段")
		}
	}
	end := len(b)
	for i, c := range b {
		if c < '0' || c > '9' {
			end = i
			break
		}
	}
	n, err := strconv.ParseUint(string(b[:end]), 10, 32)
	if err != nil || n != 1 {
		return "", errors.New("不支持的 IPMsg 版本")
	}
	if end < len(b) && b[end] != '_' {
		return "", errors.New("无效的 IPMsg 扩展版本字段")
	}
	return string(b), nil
}

func parseDecimal(b []byte, field string) (uint32, error) {
	if len(b) == 0 || len(b) > 10 {
		return 0, fmt.Errorf("无效的 %s 数字字段", field)
	}
	n, err := strconv.ParseUint(string(b), 10, 32)
	if err != nil {
		return 0, fmt.Errorf("无效的 %s 数字字段", field)
	}
	return uint32(n), nil
}

// parsePacket 在前五个冒号处拆分；文本命令的 body/extra 按第一个、第二个 NUL 截断。
func parsePacket(data []byte) (packet, error) {
	if len(data) == 0 || len(data) > maxPacketBytes {
		return packet{}, errors.New("IPMsg 报文为空或超过 UDP 大小限制")
	}
	parts := bytes.SplitN(data, []byte{':'}, 6)
	if len(parts) != 6 {
		return packet{}, errors.New("IPMsg 报文头不足五个分隔符")
	}
	version, err := parseVersion(parts[0])
	if err != nil {
		return packet{}, err
	}
	packetNo, err := parseDecimal(parts[1], "packetNo")
	if err != nil {
		return packet{}, err
	}
	command, err := parseDecimal(parts[4], "command")
	if err != nil {
		return packet{}, err
	}
	if len(parts[2]) > maxIdentityBytes || len(parts[3]) > maxIdentityBytes {
		return packet{}, errors.New("IPMsg 用户名或主机名过长")
	}
	username, err := decodeWireText(parts[2], command)
	if err != nil {
		return packet{}, err
	}
	hostname, err := decodeWireText(parts[3], command)
	if err != nil {
		return packet{}, err
	}
	if err := validateIdentity(username, "用户名"); err != nil {
		return packet{}, err
	}
	if err := validateIdentity(hostname, "主机名"); err != nil {
		return packet{}, err
	}
	var body, extra string
	if textFramedCommands[commandMode(command)] {
		rest := parts[5]
		var rawBody, rawExtra []byte
		if i := bytes.IndexByte(rest, 0); i >= 0 {
			rawBody, rawExtra = rest[:i], rest[i+1:]
			if j := bytes.IndexByte(rawExtra, 0); j >= 0 {
				rawExtra = rawExtra[:j]
			}
		} else {
			rawBody = rest
		}
		if body, err = decodeWireText(rawBody, command); err != nil {
			return packet{}, err
		}
		if extra, err = decodeWireText(rawExtra, command); err != nil {
			return packet{}, err
		}
	} else {
		body = string(parts[5])
	}
	if commandMode(command) == cmdSendMsg && len(body) > maxTextBytes {
		return packet{}, errors.New("文本消息超过 32 KiB")
	}
	return packet{version, packetNo, username, hostname, command, body, extra}, nil
}

// feiqCompatVersion 复刻 Rust 版的飞秋兼容头；没有它飞秋会把我们当成普通 IPMsg 拒收图片类功能。
// MAC 形状的 ID 由实例名+端口派生，仅作互通标识，不是认证凭据。
func feiqCompatVersion(instanceID string, port uint16) string {
	var hash uint64 = 0xcbf29ce484222325
	for _, b := range []byte(instanceID) {
		hash = (hash ^ uint64(b)) * 0x100000001b3
	}
	hash = (hash ^ 0xff) * 0x100000001b3
	hash = (hash ^ uint64(port>>8)) * 0x100000001b3
	hash = (hash ^ uint64(port&0xff)) * 0x100000001b3
	return fmt.Sprintf("1_lbt6_0#128#02%010X#0#0#0#4001#9", hash&0xffffffffff)
}

// encodePacket 按 wireVersion 编码文本报文；extra 为 nil 时只有 body-NUL，非 nil 时是 body-NUL-extra-NUL。
func encodePacket(wireVersion string, packetNo uint32, username, hostname string, command uint32, body string, extra *string) ([]byte, error) {
	if strings.Contains(wireVersion, ":") {
		return nil, errors.New("版本字段不能包含冒号")
	}
	if _, err := parseVersion([]byte(wireVersion)); err != nil {
		return nil, err
	}
	if err := validateIdentity(username, "用户名"); err != nil {
		return nil, err
	}
	if err := validateIdentity(hostname, "主机名"); err != nil {
		return nil, err
	}
	encUser, err := encodeWireText(username, command)
	if err != nil {
		return nil, err
	}
	encHost, err := encodeWireText(hostname, command)
	if err != nil {
		return nil, err
	}
	encBody, err := encodeWireText(body, command)
	if err != nil {
		return nil, err
	}
	if commandMode(command) == cmdSendMsg && len(encBody) > maxTextBytes {
		return nil, errors.New("文本消息超过 32 KiB")
	}
	var data bytes.Buffer
	fmt.Fprintf(&data, "%s:%d:", wireVersion, packetNo)
	data.Write(encUser)
	data.WriteByte(':')
	data.Write(encHost)
	fmt.Fprintf(&data, ":%d:", command)
	data.Write(encBody)
	data.WriteByte(0)
	if extra != nil {
		encExtra, err := encodeWireText(*extra, command)
		if err != nil {
			return nil, err
		}
		data.Write(encExtra)
		data.WriteByte(0)
	}
	if data.Len() > maxPacketBytes {
		return nil, errors.New("IPMsg 报文超过 UDP 大小限制")
	}
	return data.Bytes(), nil
}
