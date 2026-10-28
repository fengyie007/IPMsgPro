// Package app 是平台命令分发层，对应 Rust 版 src-tauri/commands.rs + runtime.rs：
// 前端统一 invoke(command, args)，后端以 {success, ...} / {success:false, error} JSON 应答；
// 后端事件经单一信封事件 ipmsg-event 发往前端。
package app

import (
	"encoding/json"
	"fmt"
	"log"
	"net"
	"path/filepath"
	"sync"
	"sync/atomic"
	"time"

	"encoding/base64"

	"github.com/egoist/mygo"

	"my-app/internal/media"
	"my-app/internal/network"
	"my-app/internal/notification"
	"my-app/internal/store"
)

type Envelope struct {
	Event   string         `json:"event"`
	Payload map[string]any `json:"payload"`
}

type Runtime struct {
	Hub     *network.Hub
	DB      *store.Database
	Config  *store.ConfigStore
	Version string
	DataDir string
	Port    uint16

	emit func(Envelope)

	mu                 sync.Mutex
	activeConversation string
	knownStatus        map[string]bool
	selecting          atomic.Bool // 文件/图片选择与截图共用互斥
	accepting          atomic.Bool // 启动完成后置 true，等价 Rust accepting 标志

	notifyMu   sync.Mutex
	sound      *notification.NotificationSound
	soundGate  notification.NotificationGate

	shotMu     sync.Mutex
	shot       *screenshotSession
	shotBudget chan struct{}
	mainWin    *mygo.Window
}

func New(hub *network.Hub, db *store.Database, config *store.ConfigStore, version, dataDir string) *Runtime {
	r := &Runtime{
		Hub: hub, DB: db, Config: config, Version: version, DataDir: dataDir, Port: hub.Port(),
		shotBudget: make(chan struct{}, 1),
	}
	hub.SetLocalIP(r.localIP())
	// 提示音：内置 MP3 经 MCI 播放；失败只记录，不阻塞主流程。
	sound, err := notification.New(filepath.Join(dataDir, "sounds"), config.Get().NotificationSound,
		func(msg string) { r.Emit("notification.sound_failed", map[string]any{"error": msg}) })
	if err != nil {
		log.Printf("提示音不可用: %v", err)
	} else {
		r.sound = sound
	}
	return r
}

// ShutdownNotification 退出前停止提示音工作线程。
func (r *Runtime) ShutdownNotification() {
	r.notifyMu.Lock()
	sound := r.sound
	r.sound = nil
	r.notifyMu.Unlock()
	if sound != nil {
		_ = sound.Shutdown()
	}
}

// notificationContext 汇总当前通知策略所需状态（对照 Rust 事件循环）。
func (r *Runtime) notificationContext() notification.Context {
	visible, focused, minimized := false, false, false
	if r.mainWin != nil {
		visible = r.mainWin.IsVisible()
		focused = r.mainWin.IsFocused()
		minimized = r.mainWin.IsMinimized()
	}
	return notification.Context{
		Enabled:            r.sound != nil && r.sound.Enabled(),
		Accepting:          r.accepting.Load(),
		Visible:            visible,
		Focused:            focused,
		Minimized:          minimized,
		ActiveConversation: r.activeConversation,
		LocalID:            r.Hub.LocalID(),
	}
}

// notifyReceive 对 message.received 事件应用通知闸门（提示音 + 系统通知文案）。
func (r *Runtime) notifyReceive(payload map[string]any) {
	event := notification.Event{Event: "message.received", Payload: payload}
	r.notifyMu.Lock()
	gate := r.soundGate
	context := r.notificationContext()
	shouldPlay := gate.ShouldPlay(event, context, time.Now())
	r.soundGate = gate
	sound := r.sound
	r.notifyMu.Unlock()
	if shouldPlay && sound != nil {
		sound.Notify()
	}
}

func (r *Runtime) SetEmit(fn func(Envelope)) {
	r.emit = fn
	r.accepting.Store(true)
}

// WireEvents 把 Hub 回调接成前端领域事件；UsersChanged 按快照差分出
// user.discovered / user.status_changed，避免向列表页重放全量。
func (r *Runtime) WireEvents() {
	r.knownStatus = map[string]bool{}
	r.Hub.UsersChanged = func() { r.diffUsers() }
	r.Hub.MessageAcked = func(_, messageID string) {
		r.Emit("message.ack", map[string]any{"messageId": messageID})
	}
	r.Hub.MessageFailed = func(_, messageID, reason string) {
		r.Emit("message.failed", map[string]any{"messageId": messageID, "error": reason})
	}
	r.Hub.MessageReceived = func(evt network.ReceivedEvent) {
		if evt.Duplicate || evt.User == nil {
			return
		}
		payload := map[string]any{
			"id": evt.ID, "from": evt.PeerID,
			"fromUser": userToFrontend(*evt.User, r.Version),
			"content":  evt.Content, "type": "text",
			"timestamp": evt.Timestamp, "command": ipmsgCmdSend,
		}
		r.notifyReceive(payload)
		r.Emit("message.received", payload)
	}
	r.Hub.ImageFailed = func(peerID, imageID, reason string) {
		r.Emit("image.receive_failed", map[string]any{"peerId": peerID, "imageId": imageID, "error": reason})
	}
	r.Hub.ImageReceived = func(evt network.ReceivedEvent, metadata media.ImageMetadata) {
		if evt.User == nil {
			return
		}
		payload := map[string]any{
			"id": evt.ID, "from": evt.PeerID,
			"fromUser": userToFrontend(*evt.User, r.Version),
			"content":  evt.Content, "type": "image",
			"image": metadata, "timestamp": evt.Timestamp,
			"command": 0xc0 | 0x200000,
		}
		r.notifyReceive(payload)
		r.Emit("message.received", payload)
	}
	r.Hub.FileEvent = func(event string, payload any) {
		m, ok := payload.(map[string]any)
		if !ok {
			return
		}
		// 文件邀请事件的 fromUser 换成前端 User 形状（version/avatar 齐全）。
		if event == "message.received" {
			if from, ok := m["from"].(string); ok {
				if u := r.Hub.UserByID(from); u != nil {
					m["fromUser"] = userToFrontend(*u, r.Version)
				}
			}
			r.notifyReceive(m)
		}
		r.Emit(event, m)
	}
	r.Hub.ScanEvent = func(event string, payload network.ScanStatus) {
		r.Emit(event, statusToMap(payload))
	}
	r.Hub.ImageSendProgress = func(messageID, target string, progress int, stage string, image media.ImageMetadata) {
		r.Emit("image.send_progress", map[string]any{
			"messageId": messageID, "target": target,
			"progress": progress, "stage": stage, "image": image,
		})
	}
	r.Hub.ImageSendFinished = func(messageID, target string, success bool, err error, cancelled bool) {
		event := "image.send_completed"
		payload := map[string]any{"messageId": messageID, "target": target, "progress": 100}
		if !success {
			event = "image.send_failed"
			payload["progress"] = 0
			payload["cancelled"] = cancelled
			if err != nil {
				payload["error"] = err.Error()
			}
		}
		r.Emit(event, payload)
	}
}

const ipmsgCmdSend = 32

func (r *Runtime) diffUsers() {
	r.mu.Lock()
	prev := r.knownStatus
	users := r.Hub.ListUsers()
	next := make(map[string]bool, len(users))
	var discovered, statusChanged []network.User
	for _, u := range users {
		next[u.ID] = u.Online
		if old, seen := prev[u.ID]; !seen {
			discovered = append(discovered, u)
		} else if old != u.Online {
			statusChanged = append(statusChanged, u)
		}
	}
	r.knownStatus = next
	r.mu.Unlock()
	for _, u := range discovered {
		r.Emit("user.discovered", userToFrontend(u, r.Version))
	}
	for _, u := range statusChanged {
		status := "offline"
		if u.Online {
			status = "online"
		}
		r.Emit("user.status_changed", map[string]any{"user": userToFrontend(u, r.Version), "status": status})
	}
}

// Emit 把领域事件打包成信封发往前端；emit 未就绪时丢弃（启动早期允许）。
func (r *Runtime) Emit(event string, payload map[string]any) {
	if r.emit == nil {
		return
	}
	r.emit(Envelope{Event: event, Payload: payload})
}

func ok(extra map[string]any) map[string]any {
	out := map[string]any{"success": true}
	for k, v := range extra {
		out[k] = v
	}
	return out
}

func fail(err string) map[string]any {
	return map[string]any{"success": false, "error": err}
}

// Handle 处理一条前端命令，始终返回 JSON 字符串（不向传输层抛错）。
func (r *Runtime) Handle(command string, argsJSON string) string {
	var args map[string]any
	if argsJSON != "" {
		if err := json.Unmarshal([]byte(argsJSON), &args); err != nil {
			return marshal(fail("命令参数不是合法 JSON 对象"))
		}
	}
	result := r.dispatch(command, args)
	data, err := json.Marshal(result)
	if err != nil {
		return `{"success":false,"error":"响应序列化失败"}`
	}
	return string(data)
}

func marshal(v any) string {
	data, err := json.Marshal(v)
	if err != nil {
		return `{"success":false,"error":"响应序列化失败"}`
	}
	return string(data)
}

// capabilities 对应前端 types/index.ts Capabilities；M1 只交付文本/发现/历史。
func (r *Runtime) capabilities() map[string]any {
	files := r.Hub.FilesEnabled()
	return map[string]any{
		"images": r.Hub.ImagesEnabled(), "imageReceive": r.Hub.ImagesEnabled(), "imageSend": r.Hub.ImagesEnabled(),
		"files": files, "screenshot": true, "scan": true, "notificationSound": r.notificationSoundEnabled(),
	}
}

// notificationSoundEnabled 提示音能力：有可用播放服务即为 true。
func (r *Runtime) notificationSoundEnabled() bool {
	r.notifyMu.Lock()
	defer r.notifyMu.Unlock()
	return r.sound != nil && r.sound.Available()
}

// statusToMap 把 ScanStatus 序列化为通用 map（事件载荷统一形状）。
func statusToMap(v any) map[string]any {
	data, err := json.Marshal(v)
	if err != nil {
		return map[string]any{}
	}
	var out map[string]any
	if err := json.Unmarshal(data, &out); err != nil {
		return map[string]any{}
	}
	return out
}

func (r *Runtime) localIP() string {
	conn, err := net.Dial("udp4", "223.5.5.5:53")
	if err == nil {
		defer conn.Close()
		if addr, ok := conn.LocalAddr().(*net.UDPAddr); ok {
			return addr.IP.String()
		}
	}
	return "127.0.0.1"
}

func (r *Runtime) applyIdentity(cfg store.AppConfig) {
	nick := cfg.Nickname
	if nick == "" {
		nick = r.Hub.Nick()
	}
	r.Hub.SetProfile(nick, cfg.Group, cfg.AvatarID)
}

func (r *Runtime) localUser() map[string]any {
	cfg := r.Config.Get()
	nick := cfg.Nickname
	if nick == "" {
		nick = r.Hub.Nick()
	}
	return map[string]any{
		"success":  true,
		"id":       r.Hub.LocalID(),
		"nickname": nick,
		"username": nick,
		"hostname": r.Hub.Hostname(),
		"group":    cfg.Group,
		"ip":       r.localIP(),
		"port":     r.Hub.Port(),
		"status":   "online",
		"version":  r.Version,
		"avatar":   map[string]any{"kind": "builtin", "id": cfg.AvatarID},
		"mac":      nil,
	}
}

// userToFrontend 输出前端 User 形状。
func userToFrontend(u network.User, version string) map[string]any {
	status := "offline"
	if u.Online {
		status = "online"
	}
	return map[string]any{
		"id": u.ID, "nickname": u.Nick, "username": u.Nick, "hostname": u.Host,
		"group": u.Group, "ip": u.Addr, "port": u.Port, "status": status,
		"version": "", "avatar": map[string]any{"kind": "unknown"}, "mac": nil,
	}
}

func (r *Runtime) dispatch(command string, args map[string]any) map[string]any {
	switch command {
	case "app.info":
		return ok(map[string]any{"version": r.Version, "dataDir": r.DataDir, "port": r.Port,
			"systemNotifications": false, "capabilities": r.capabilities()})

	case "config.get":
		return ok(map[string]any{"config": r.Config.Get()})
	case "config.set":
		raw, err := json.Marshal(args)
		if err != nil {
			return fail("设置参数无效")
		}
		next, err := r.Config.Save(raw)
		if err != nil {
			return fail(err.Error())
		}
		r.applyIdentity(next)
		r.notifyMu.Lock()
		sound := r.sound
		r.notifyMu.Unlock()
		if sound != nil {
			sound.SetEnabled(next.NotificationSound)
		}
		return ok(map[string]any{"config": next})
	case "config.loaded":
		r.applyIdentity(r.Config.Get())
		r.notifyMu.Lock()
		sound := r.sound
		r.notifyMu.Unlock()
		if sound != nil {
			sound.SetEnabled(r.Config.Get().NotificationSound)
		}
		// 启动收尾：公告上线 + 自动 IP 段扫描（已有扫描时不重启），
		// 对拍 Rust ui_ready；与 C++ 版约定一致：由后端在此触发，前端不得重复发起。
		cfg := r.Config.Get()
		if cfg.AnnounceOnStartup {
			r.Hub.AnnounceEntry()
		}
		if cfg.ScanOnStartup && len(cfg.IPScanRanges) > 0 {
			_, err := r.Hub.StartScan(store.ScanOptions{
				Ranges: cfg.IPScanRanges, Port: cfg.ScanPort, DelayMs: cfg.ScanDelayMs,
			}, true)
			if err != nil {
				log.Printf("启动扫描失败: %v", err)
			}
		}
		return ok(nil)

	case "user.local":
		return r.localUser()
	case "user.list":
		users := r.Hub.ListUsers()
		list := make([]map[string]any, 0, len(users))
		avatars := map[string]any{}
		for _, u := range users {
			list = append(list, userToFrontend(u, r.Version))
			avatars[u.ID] = map[string]any{"kind": "unknown"}
		}
		return ok(map[string]any{"users": list, "count": len(list), "avatars": avatars})
	case "user.discover":
		r.Hub.Refresh()
		return ok(nil)

	case "message.send":
		target, _ := args["target"].(string)
		content, _ := args["content"].(string)
		if target == "" || content == "" {
			return fail("发送目标或内容为空")
		}
		messageID, err := r.Hub.SendText(target, content)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"messageId": messageID})
	case "message.knock":
		return fail("窗口振动将在后续阶段迁移")

	case "history.get":
		userID, _ := args["userId"].(string)
		limit := numberArg(args["limit"], 50)
		offset := uint32(numberArg(args["offset"], 0))
		messages, err := r.DB.History(userID, r.Hub.LocalID(), limit, offset)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"messages": messages, "localUserId": r.Hub.LocalID()})
	case "history.get_recent":
		messages, err := r.DB.Recent(r.Hub.LocalID(), 200)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"messages": messages, "localUserId": r.Hub.LocalID()})
	case "history.search":
		keyword, _ := args["keyword"].(string)
		var user *string
		if v, ok := args["userId"].(string); ok && v != "" {
			user = &v
		}
		messages, err := r.DB.Search(keyword, user, r.Hub.LocalID())
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"messages": messages, "localUserId": r.Hub.LocalID()})
	case "history.clear":
		var user *string
		if v, ok := args["userId"].(string); ok && v != "" {
			user = &v
		}
		deleted, err := r.DB.Clear(user, r.Hub.LocalID())
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"deletedIds": deleted})

	case "window.set_active_conversation":
		userID, _ := args["userId"].(string)
		r.mu.Lock()
		r.activeConversation = userID
		r.mu.Unlock()
		return ok(nil)
	case "frontend.error":
		log.Printf("前端错误: %v", args)
		return ok(nil)
	case "tray.start_blink", "tray.stop_blink":
		// Go 侧托盘按未读总数与窗口焦点自动闪烁，命令保留以兼容前端流程。
		return ok(nil)

	// 以下能力按阶段迁移（M2 图片 / M3 截图 / M4 文件 / M5 通知扫描存储）
	case "image.read":
		assetID, _ := args["assetId"].(string)
		thumbnail := false
		if v, ok := args["thumbnail"].(bool); ok {
			thumbnail = v
		}
		if _, _, err := r.Hub.ImageRead(assetID, thumbnail); err != nil {
			return fail(err.Error())
		}
		// URL 由 ipmsg-image 协议处理器服务；数据库登记校验已在 ImageRead 内完成
		query := ""
		if thumbnail {
			query = "?t=1"
		}
		return ok(map[string]any{"url": fmt.Sprintf("ipmsg-image://localhost/%s%s", assetID, query)})
	case "image.select":
		if r.Hub == nil || !r.Hub.ImagesEnabled() {
			return fail("图片服务未启用")
		}
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{
			Title:   "选择图片",
			Filters: []mygo.FileFilter{{Name: "图片", Extensions: []string{"png", "jpg", "jpeg", "bmp"}}},
		})
		if err != nil {
			return fail(err.Error())
		}
		if len(paths) == 0 {
			return ok(map[string]any{"image": nil})
		}
		image, err := r.Hub.ImportImagePath(paths[0])
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"image": image})
	case "image.send":
		target, _ := args["target"].(string)
		assetID, _ := args["assetId"].(string)
		result, err := r.Hub.SendImage(target, assetID)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"messageId": result.MessageID, "imageId": result.ImageID, "image": result.Image})
	case "image.cancel":
		messageID, _ := args["messageId"].(string)
		return ok(map[string]any{"cancelled": r.Hub.CancelImage(messageID)})
	case "image.discard":
		assetID, _ := args["assetId"].(string)
		discarded, err := r.Hub.DiscardImage(assetID)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"discarded": discarded})
	case "image.import_clipboard":
		base64Data, _ := args["base64"].(string)
		data, err := base64.StdEncoding.DecodeString(base64Data)
		if err != nil {
			return fail("剪贴板图片数据无效")
		}
		image, err := r.Hub.ImportClipboard(data)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"image": image})
	case "screenshot.start":
		return r.StartScreenshot()
	case "screenshot.confirm":
		sessionID, _ := args["sessionId"].(string)
		base64Data, _ := args["base64"].(string)
		return r.ConfirmScreenshot(sessionID, base64Data)
	case "screenshot_command":
		sessionID, _ := args["sessionId"].(string)
		command, _ := args["command"].(string)
		return r.screenshotEditorCommand(sessionID, command)
	case "notification.test_sound":
		r.notifyMu.Lock()
		sound := r.sound
		r.notifyMu.Unlock()
		if sound == nil {
			return fail("提示音不可用")
		}
		if _, err := sound.Preview(); err != nil {
			return fail(err.Error())
		}
		return ok(nil)
	case "notification.test_system", "notification.take_activation":
		return fail("系统通知需要 Windows 桌面版（MyGo 版已按迁移决策降级为应用内提示）")
	case
		"storage.info", "storage.select", "storage.default", "storage.apply", "storage.cancel":
		return fail(fmt.Sprintf("%s 将在后续阶段迁移（见 MyGo迁移计划.md）", command))

	case "network.scan_status":
		return ok(map[string]any{"scan": statusToMap(r.Hub.ScanStatus())})
	case "network.scan_range":
		ranges, _ := args["ranges"].([]any)
		var rangeList []string
		for _, item := range ranges {
			if s, ok := item.(string); ok {
				rangeList = append(rangeList, s)
			}
		}
		port := numberArg(args["port"], 2425)
		delay := numberArg(args["delayMs"], 20)
		status, err := r.Hub.StartScan(store.ScanOptions{
			Ranges: rangeList, Port: uint16(port), DelayMs: delay,
		}, false)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"scan": statusToMap(status)})
	case "network.scan_cancel":
		scanID := uint64(numberArg(args["scanId"], 0))
		status, err := r.Hub.CancelScan(scanID)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"scan": statusToMap(status)})

	case "file.select":
		return r.SelectFileOrFolder(false)
	case "file.select_folder":
		return r.SelectFileOrFolder(true)
	case "file.send":
		target, _ := args["target"].(string)
		selectionID, _ := args["selectionId"].(string)
		record, err := r.Hub.SendFile(target, selectionID)
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"message": record})
	case "file.discard":
		selectionID, _ := args["selectionId"].(string)
		if transfers, err := r.Hub.Files(); err != nil {
			return fail(err.Error())
		} else {
			transfers.Discard(selectionID)
		}
		return ok(nil)
	case "file.accept", "file.resume":
		messageID, _ := args["messageId"].(string)
		transfers, err := r.Hub.Files()
		if err != nil {
			return fail(err.Error())
		}
		if err := transfers.Accept(messageID); err != nil {
			return fail(err.Error())
		}
		return ok(nil)
	case "file.pause":
		messageID, _ := args["messageId"].(string)
		transfers, err := r.Hub.Files()
		if err != nil {
			return fail(err.Error())
		}
		if err := transfers.Pause(messageID); err != nil {
			return fail(err.Error())
		}
		return ok(nil)
	case "file.reject", "file.cancel":
		messageID, _ := args["messageId"].(string)
		transfers, err := r.Hub.Files()
		if err != nil {
			return fail(err.Error())
		}
		cancelled, err := transfers.Cancel(messageID, command == "file.reject")
		if err != nil {
			return fail(err.Error())
		}
		return ok(map[string]any{"cancelled": cancelled})
	case "file.open_folder":
		messageID, _ := args["messageId"].(string)
		return r.OpenFileFolder(messageID)

	default:
		return fail("未实现命令：" + command)
	}
}

func numberArg(v any, fallback uint32) uint32 {
	switch n := v.(type) {
	case float64:
		if n >= 0 {
			return uint32(n)
		}
	case int:
		if n >= 0 {
			return uint32(n)
		}
	}
	return fallback
}
