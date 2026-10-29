// screenshot.go 对应 tauri-rust capture.rs：单一截图会话、独立编辑器窗口、
// 确认/取消/窗口关闭的互斥与迟到结果拒绝。
package app

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"sync"
	"time"

	"github.com/egoist/mygo"

	"speedipmsg-go/internal/media"
	"speedipmsg-go/internal/platform/capture"
)

type screenshotPhase int

const (
	shotEditing screenshotPhase = iota
	shotImporting
	shotFinished
)

type screenshotSession struct {
	id       string
	png      []byte
	editor   *mygo.Window
	deadline time.Time

	mu        sync.Mutex
	phase     screenshotPhase
	result    *media.ImageMetadata
	finished  bool
	finishErr error
	done      chan struct{}
}

// SetMainWindow 注册主窗口（截图时隐藏/恢复）。
func (r *Runtime) SetMainWindow(win *mygo.Window) {
	r.mainWin = win
}

func (r *Runtime) randomSessionID() string {
	var b [8]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// StartScreenshot 执行一次完整截图会话：隐藏主窗口 → 捕获 → 编辑器窗口 →
// 确认导入 / 取消。返回 {image} 或 {cancelled}。
func (r *Runtime) StartScreenshot() map[string]any {
	select {
	case r.shotBudget <- struct{}{}:
	default:
		return fail("已有截图会话进行中")
	}
	defer func() { <-r.shotBudget }()

	r.shotMu.Lock()
	if r.shot != nil {
		r.shotMu.Unlock()
		return fail("已有截图会话进行中")
	}
	if r.mainWin == nil {
		r.shotMu.Unlock()
		return fail("主窗口未就绪")
	}
	bounds, err := capture.MonitorOfWindow(r.mainWin.NativeHandle())
	if err != nil {
		r.shotMu.Unlock()
		return fail(err.Error())
	}
	r.mainWin.Hide()
	png, err := capture.Capture(bounds)
	if err != nil {
		r.mainWin.Show()
		r.shotMu.Unlock()
		return fail(err.Error())
	}
	session := &screenshotSession{
		id:       r.randomSessionID(),
		png:      png,
		deadline: time.Now().Add(5 * time.Minute),
		done:     make(chan struct{}),
	}
	r.shot = session
	r.shotMu.Unlock()

	editor := mygo.NewWindow(mygo.WindowOptions{
		Title:          "迅秋截图",
		URL:            "/?capture=" + session.id,
		X:              int(bounds.X),
		Y:              int(bounds.Y),
		Width:          int(bounds.Width),
		Height:         int(bounds.Height),
		UseContentSize: true,
		Frameless:      true,
		AlwaysOnTop:    true,
		SkipTaskbar:    true,
		DisableResize:  true,
	})
	if editor == nil {
		r.finishScreenshot(session, fmt.Errorf("无法创建截图窗口"))
		r.mainWin.Show()
		return fail("无法创建截图窗口")
	}
	session.editor = editor
	editor.OnClosed(func() { r.finishScreenshot(session, nil) })

	timeout := time.AfterFunc(time.Until(session.deadline), func() {
		r.finishScreenshot(session, fmt.Errorf("截图会话超时"))
	})
	defer timeout.Stop()

	<-session.done

	r.shotMu.Lock()
	result := session.result
	finishedErr := session.finishErr
	r.shot = nil
	r.shotMu.Unlock()
	if finishedErr != nil {
		return fail(finishedErr.Error())
	}
	if result == nil {
		return ok(map[string]any{"cancelled": true})
	}
	return ok(map[string]any{"image": *result})
}

// finishScreenshot 结束会话：关闭编辑器、恢复主窗口、唤醒等待者。
// 幂等：迟到/重复的结束（确认、取消、关窗、超时）只有第一次生效。
func (r *Runtime) finishScreenshot(session *screenshotSession, err error) {
	session.mu.Lock()
	if session.finished {
		session.mu.Unlock()
		return
	}
	session.finished = true
	session.finishErr = err
	close(session.done)
	session.mu.Unlock()

	if session.editor != nil {
		session.editor.Destroy()
		session.editor = nil
	}
	if r.mainWin != nil {
		r.mainWin.Show()
	}
	r.shotMu.Lock()
	if r.shot == session {
		r.shot = nil
	}
	r.shotMu.Unlock()
}

// screenshotEditorCommand 处理编辑器窗口的 read/ready/cancel。
func (r *Runtime) screenshotEditorCommand(sessionID, command string) map[string]any {
	r.shotMu.Lock()
	session := r.shot
	r.shotMu.Unlock()
	if session == nil || session.id != sessionID {
		return fail("截图会话不存在或已结束")
	}
	switch command {
	case "read":
		session.mu.Lock()
		finished := session.finished
		session.mu.Unlock()
		if finished {
			return fail("截图会话已结束")
		}
		return ok(map[string]any{"url": schemeURL("ipmsg-capture", sessionID)})
	case "ready":
		return ok(nil)
	case "cancel":
		r.finishScreenshot(session, nil)
		return ok(nil)
	default:
		return fail("未知截图命令：" + command)
	}
}

// ConfirmScreenshot 处理编辑器的确认：导入标注后的 PNG 并结束会话。
func (r *Runtime) ConfirmScreenshot(sessionID string, pngBase64 string) map[string]any {
	r.shotMu.Lock()
	session := r.shot
	r.shotMu.Unlock()
	if session == nil || session.id != sessionID {
		return fail("截图会话不存在或已结束")
	}
	session.mu.Lock()
	if session.finished || session.phase == shotFinished {
		session.mu.Unlock()
		return fail("截图会话已结束")
	}
	session.phase = shotImporting
	session.mu.Unlock()

	png, err := base64.StdEncoding.DecodeString(pngBase64)
	if err != nil {
		session.mu.Lock()
		session.phase = shotEditing
		session.mu.Unlock()
		return fail("标注图片数据无效")
	}
	image, err := r.Hub.ImportScreenshot(png)
	if err != nil {
		session.mu.Lock()
		session.phase = shotEditing
		session.mu.Unlock()
		return fail(err.Error())
	}
	session.mu.Lock()
	session.result = &image
	session.mu.Unlock()
	r.finishScreenshot(session, nil)
	return ok(map[string]any{"image": image})
}

// ServeScreenshotPNG 服务活动会话的原始截图（ipmsg-capture 协议）；
// 仅限当前会话 ID，仅编辑器窗口可读取。
func (r *Runtime) ServeScreenshotPNG(sessionID string) ([]byte, error) {
	r.shotMu.Lock()
	session := r.shot
	r.shotMu.Unlock()
	if session == nil || session.id != sessionID {
		return nil, fmt.Errorf("截图会话不存在或已结束")
	}
	session.mu.Lock()
	defer session.mu.Unlock()
	if session.phase == shotFinished {
		return nil, fmt.Errorf("截图会话已结束")
	}
	return session.png, nil
}
