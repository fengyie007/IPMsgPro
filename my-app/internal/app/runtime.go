// Package app 是平台命令分发层，对应 Rust 版 src-tauri/commands.rs + runtime.rs：
// 前端统一 invoke(command, args)，后端以 {success, ...} / {success:false, error} JSON 应答；
// 后端事件经单一信封事件 ipmsg-event 发往前端。
package app

import (
	"encoding/json"
	"fmt"
	"log"
	"net"
	"sync"

	"my-app/internal/network"
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
}

func New(hub *network.Hub, db *store.Database, config *store.ConfigStore, version, dataDir string) *Runtime {
	return &Runtime{Hub: hub, DB: db, Config: config, Version: version, DataDir: dataDir, Port: hub.Port()}
}

func (r *Runtime) SetEmit(fn func(Envelope)) { r.emit = fn }

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
		r.Emit("message.received", map[string]any{
			"id": evt.ID, "from": evt.PeerID,
			"fromUser": userToFrontend(*evt.User, r.Version),
			"content":  evt.Content, "type": "text",
			"timestamp": evt.Timestamp, "command": ipmsgCmdSend,
		})
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
func capabilities() map[string]any {
	return map[string]any{
		"images": false, "imageReceive": false, "imageSend": false,
		"files": false, "screenshot": false, "scan": false, "notificationSound": false,
	}
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
		return ok(map[string]any{"version": r.Version, "dataDir": r.DataDir, "port": r.Port, "capabilities": capabilities()})

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
		return ok(map[string]any{"config": next})
	case "config.loaded":
		r.applyIdentity(r.Config.Get())
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
	case "network.scan_status":
		return ok(map[string]any{"scan": map[string]any{
			"scanId": 0, "revision": 0, "state": "idle", "current": 0, "total": 0,
			"found": 0, "failedSends": 0, "skipped": 0, "port": 2425, "delayMs": 20,
			"ranges": []any{}, "error": nil,
		}})

	// 以下能力按阶段迁移（M2 图片 / M3 截图 / M4 文件 / M5 通知扫描存储）
	case "image.read", "image.select", "image.send", "image.cancel", "image.discard",
		"screenshot.start",
		"notification.test_sound", "notification.test_system", "notification.take_activation",
		"storage.info", "storage.select", "storage.default", "storage.apply", "storage.cancel",
		"network.scan_range", "network.scan_cancel",
		"file.select", "file.send", "file.discard", "file.accept", "file.reject",
		"file.cancel", "file.open_folder", "file.select_folder", "file.resume", "file.pause":
		return fail(fmt.Sprintf("%s 将在后续阶段迁移（见 MyGo迁移计划.md）", command))

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
