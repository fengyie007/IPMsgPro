// text.go 对应 tauri-rust text.rs：保守的飞秋文本兼容，不解释富文本。
package ipmsg

import (
	"strconv"
	"strings"
)

const (
	fontMarker         = "{/font;"
	maxFontSuffixBytes = 512
)

func trimSpaces(text string) string {
	return strings.Trim(text, " \t")
}

func signedInteger(text string) (int64, bool) {
	digits := strings.TrimPrefix(text, "-")
	if digits == "" || !allASCIIDigits(digits) {
		return 0, false
	}
	value, err := strconv.ParseInt(text, 10, 32)
	if err != nil {
		return 0, false
	}
	return value, true
}

func allASCIIDigits(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return len(s) > 0
}

// validFontFields 校验 LOGFONT：五个 LONG、八个 BYTE、最后是字体名 + 颜色。
func validFontFields(fields string) bool {
	for _, r := range fields {
		if r == '{' || r == '}' || r == ';' || r == '\r' || r == '\n' || r == 0 {
			return false
		}
	}
	for index := 0; index < 13; index++ {
		fields = trimSpaces(fields)
		end := strings.IndexAny(fields, " \t")
		if end < 0 {
			return false
		}
		value, ok := signedInteger(fields[:end])
		if !ok {
			return false
		}
		if index >= 5 && (value < 0 || value > 255) {
			return false
		}
		if end+1 > len(fields) {
			return false
		}
		fields = fields[end+1:]
	}
	fields = trimSpaces(fields)
	// 只有最后一个 token 是颜色；字体名本身可含空格
	colorStart := strings.LastIndexAny(fields, " \t")
	if colorStart < 0 {
		return false
	}
	if trimSpaces(fields[:colorStart]) == "" {
		return false
	}
	color := fields[colorStart+1:]
	return color != "" && allASCIIDigits(color)
}

// StripFeiqFontSuffix 移除末尾一个完整的 {/font;...;} 块，保留其前的原文。
// 无效、残缺或非末尾的标记按普通文本处理。
func StripFeiqFontSuffix(body string) string {
	start := strings.LastIndex(body, fontMarker)
	if start < 0 {
		return body
	}
	suffix := body[start:]
	if len(suffix) > maxFontSuffixBytes || len(suffix) < len(fontMarker)+2 {
		return body
	}
	fields, ok := strings.CutSuffix(suffix[len(fontMarker):], ";}")
	if !ok {
		return body
	}
	if validFontFields(fields) {
		return body[:start]
	}
	return body
}
