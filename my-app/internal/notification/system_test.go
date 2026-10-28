package notification

// 对拍 tauri-rust system_notification.rs 的文案构造测试。

import "testing"

func TestTextIsPrivateByDefaultAndEscaped(t *testing.T) {
	event := Event{Event: "message.received", Payload: map[string]any{
		"from": "peer", "fromUser": map[string]any{"nickname": "<张&李>"},
		"type": "text", "content": "<script>秘密\x00",
	}}
	if notice := NoticeFromEvent(event, false); contains(notice.Body, "秘密") {
		t.Fatalf("默认预览关闭不得泄露内容: %q", notice.Body)
	}
	notice := NoticeFromEvent(event, true)
	if !contains(notice.Body, "<script>秘密") || contains(notice.Body, "\x00") {
		t.Fatalf("预览应保留原文并去掉控制符: %q", notice.Body)
	}
	if !contains(notice.Title, "<张&李>") {
		t.Fatalf("标题应保留昵称: %q", notice.Title)
	}
}

func TestFileAndImageNotificationsNeverExposePaths(t *testing.T) {
	for _, kind := range []string{"image", "file", "knock"} {
		event := Event{Event: "message.received", Payload: map[string]any{
			"from": "peer", "type": kind, "content": `C:\private\secret`,
		}}
		notice := NoticeFromEvent(event, true)
		if contains(notice.Body, "private") {
			t.Fatalf("%s 通知不得暴露路径: %q", kind, notice.Body)
		}
		if kind == "knock" && notice.Body != "对方发送了窗口振动" {
			t.Fatalf("振动文案异常: %q", notice.Body)
		}
	}
}

func contains(s, sub string) bool {
	return len(sub) == 0 || len(s) >= len(sub) && indexOf(s, sub) >= 0
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}
