// system.go 对拍 tauri-rust system_notification.rs 的通知文案构造（可移植部分）。
// Windows WinRT Toast 后端（AUMID 快捷方式 + ToastNotifier + 激活回调）需要
// 纯 Go 手写整套 COM/WinRT 接口，按迁移决策 4 降级：MyGo 版暂不提供系统通知，
// 能力位 systemNotifications=false，前端设置页灰置并显示说明。
package notification

import (
	"fmt"
	"strings"
)

// Notice 是一条待显示的通知。
type Notice struct {
	Title  string
	Body   string
	Target string
}

// NoticeFromEvent 从 message.received 事件构造通知文案：
// 默认不泄露文本内容（预览开关打开时文本才显示摘要）。
func NoticeFromEvent(event Event, preview bool) Notice {
	name := "新消息"
	if fromUser, ok := event.Payload["fromUser"].(map[string]any); ok {
		if nick, ok := fromUser["nickname"].(string); ok && nick != "" {
			name = nick
		}
	}
	kind, _ := stringArg(event.Payload, "type")
	body := ""
	if preview && kind == "text" {
		body, _ = stringArg(event.Payload, "content")
		if body == "" {
			body = "新消息"
		}
	} else {
		switch kind {
		case "image":
			body = "收到一张图片"
		case "file":
			body = "收到文件邀请"
		case "knock":
			body = "对方发送了窗口振动"
		default:
			body = "收到一条新消息"
		}
	}
	target, _ := stringArg(event.Payload, "from")
	return Notice{
		Title:  fmt.Sprintf("迅秋 · %s", Clean(name, 80)),
		Body:   Clean(body, 240),
		Target: target,
	}
}

// Clean 过滤控制字符并截断长度。
func Clean(text string, limit int) string {
	var builder strings.Builder
	count := 0
	for _, c := range text {
		if count >= limit {
			break
		}
		isControl := c < 0x20 || c == 0x7f
		if isControl && c != '\n' && c != '\t' {
			continue
		}
		if c == '\uFFFE' || c == '\uFFFF' {
			continue
		}
		builder.WriteRune(c)
		count++
	}
	return builder.String()
}
