// avatar.go 对拍 tauri-rust avatar.rs 与 protocol.rs feiq_mac：
// 飞秋内置头像编号 0..83，从版本头解析头像与对端自报 MAC。
package ipmsg

import (
	"fmt"
	"strconv"
	"strings"
)

const AvatarCount = 84

// ValidateAvatarID 校验头像编号范围。
func ValidateAvatarID(id int) error {
	if id < 0 || id >= AvatarCount {
		return fmt.Errorf("头像编号必须在0到83之间")
	}
	return nil
}

// AvatarKind 描述版本头里的头像形态。
type AvatarKind string

const (
	AvatarBuiltin AvatarKind = "builtin"
	AvatarCustom  AvatarKind = "custom"
	AvatarUnknown AvatarKind = "unknown"
	AvatarNone    AvatarKind = "" // 版本头不是飞秋形态
)

func avatarNumber(text string) (uint32, bool) {
	if text == "" || len(text) > 10 {
		return 0, false
	}
	for i := 0; i < len(text); i++ {
		if text[i] < '0' || text[i] > '9' {
			return 0, false
		}
	}
	value, err := strconv.ParseUint(text, 10, 64)
	if err != nil {
		return 0, false
	}
	return uint32(value), true
}

// ParseAvatar 从版本头解析头像：builtin(0..83)/custom/unknown；非飞秋头返回 AvatarNone。
func ParseAvatar(version string) (AvatarKind, int) {
	if len(version) > 128 {
		return AvatarNone, 0
	}
	parts := strings.Split(version, "#")
	if len(parts) < 5 {
		return AvatarNone, 0
	}
	prefix, ok := strings.CutPrefix(parts[0], "1_lbt")
	if !ok {
		return AvatarNone, 0
	}
	dialect, idText, found := strings.Cut(prefix, "_")
	if !found || (dialect != "4" && dialect != "6") {
		return AvatarNone, 0
	}
	id, ok := avatarNumber(idText)
	if !ok {
		return AvatarNone, 0
	}
	customBytes, ok := avatarNumber(parts[3])
	if !ok {
		return AvatarNone, 0
	}
	// 第 5 段是样式尺寸，不是头像编号，但必须是合法数字。
	if _, ok := avatarNumber(parts[4]); !ok {
		return AvatarNone, 0
	}
	if customBytes > 0 {
		return AvatarCustom, 0
	}
	if id < AvatarCount {
		return AvatarBuiltin, int(id)
	}
	return AvatarUnknown, 0
}

const (
	// MaxVersionBytes 与 Rust MAX_VERSION_BYTES 一致：版本头长度上限。
	MaxVersionBytes = 128
)

func allDigitsASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return true
}

// FeiqMac 读取对端版本头里自报的 MAC（并非 ARP/硬件校验），
// 归一化为大写连字符形式；无效时返回空串。
func FeiqMac(version string) string {
	if len(version) > MaxVersionBytes {
		return ""
	}
	fields := strings.Split(version, "#")
	if len(fields) < 3 {
		return ""
	}
	prefix, ok := strings.CutPrefix(fields[0], "1_lbt")
	if !ok {
		return ""
	}
	dialect, avatar, found := strings.Cut(prefix, "_")
	if !found || (dialect != "4" && dialect != "6") ||
		avatar == "" || !allDigitsASCII(avatar) {
		return ""
	}
	raw := fields[2]
	var compact string
	switch {
	case len(raw) == 12 && allHexASCII(raw):
		compact = raw
	case len(raw) == 17:
		parts := strings.Split(raw, "-")
		if len(parts) != 6 {
			return ""
		}
		for _, part := range parts {
			if len(part) != 2 || !allHexASCII(part) {
				return ""
			}
		}
		compact = strings.Join(parts, "")
	default:
		return ""
	}
	first, err := strconv.ParseUint(compact[:2], 16, 8)
	if err != nil || first&1 != 0 || allZeros(compact) {
		return ""
	}
	upper := strings.ToUpper(compact)
	var builder strings.Builder
	for i := 0; i < 6; i++ {
		if i > 0 {
			builder.WriteByte('-')
		}
		builder.WriteString(upper[i*2 : i*2+2])
	}
	return builder.String()
}

func allHexASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		b := s[i]
		if !(b >= '0' && b <= '9' || b >= 'a' && b <= 'f' || b >= 'A' && b <= 'F') {
			return false
		}
	}
	return true
}

func allZeros(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] != '0' {
			return false
		}
	}
	return true
}
