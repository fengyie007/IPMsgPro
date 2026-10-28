// Package notification 对拍 tauri-rust src-tauri/src/notification.rs：
// 通知策略闸门（去重/冷却/前后台静音）+ 单一有界音频工作线程。
// UDP/UI 线程上不做任何音频 IO。
package notification

import (
	"fmt"
	"runtime"
	"sync"
	"sync/atomic"
	"time"
)

const (
	Cooldown       = time.Second
	SeenTTL        = 5 * time.Minute
	SeenLimit      = 256
	PreviewTimeout = 3 * time.Second
)

// Context 是一次事件评估时的应用状态快照。
type Context struct {
	Enabled            bool
	Accepting          bool
	Visible            bool
	Focused            bool
	Minimized          bool
	ActiveConversation string
	LocalID            string
}

// Event 是待评估的领域事件。
type Event struct {
	Event   string
	Payload map[string]any
}

type seenEntry struct {
	key string
	at  time.Time
}

// NotificationGate 判定事件是否应该发声；静音/正在阅读的消息也登记，
// 防止开关切换后迟到的重复事件补响。
type NotificationGate struct {
	seen      map[string]bool
	order     []seenEntry
	lastSound time.Time
	hasSound  bool
}

func stringArg(m map[string]any, key string) (string, bool) {
	if m == nil {
		return "", false
	}
	s, ok := m[key].(string)
	if !ok || s == "" {
		return "", false
	}
	return s, true
}

func (g *NotificationGate) ShouldPlay(event Event, context Context, now time.Time) bool {
	if event.Event != "message.received" {
		return false
	}
	id, ok := stringArg(event.Payload, "id")
	if !ok {
		return false
	}
	from, ok := stringArg(event.Payload, "from")
	if !ok {
		return false
	}
	if from == context.LocalID || from == "self" || len(id)+len(from) > 8192 {
		return false
	}
	kind, _ := stringArg(event.Payload, "type")
	switch kind {
	case "text", "image", "file", "knock":
	default:
		return false
	}
	// 清理过期的去重记忆
	for len(g.order) > 0 && now.Sub(g.order[0].at) >= SeenTTL {
		delete(g.seen, g.order[0].key)
		g.order = g.order[1:]
	}
	key := from + "\x00" + id
	if g.seen[key] {
		return false
	}
	if len(g.order) >= SeenLimit {
		delete(g.seen, g.order[0].key)
		g.order = g.order[1:]
	}
	if g.seen == nil {
		g.seen = map[string]bool{}
	}
	g.seen[key] = true
	g.order = append(g.order, seenEntry{key: key, at: now})
	reading := context.Visible && context.Focused && !context.Minimized &&
		context.ActiveConversation == from
	if !context.Enabled || !context.Accepting || reading ||
		(g.hasSound && now.Sub(g.lastSound) < Cooldown) {
		return false
	}
	g.lastSound = now
	g.hasSound = true
	return true
}

// Player 是音频后端抽象（测试用假播放器替换）。
type Player interface {
	Play(cancelled func() bool) (time.Duration, error)
	Stop() error
}

type shared struct {
	enabled atomic.Bool
	epoch   atomic.Uint64
	closing atomic.Bool
	alive   atomic.Bool
}

type playRequest struct {
	epoch  uint64
	queued time.Time
	reply  chan playResult
}

type playResult struct {
	duration uint64
	err      error
}

type command struct {
	play *playRequest
}

// NotificationSound 是单一有界播放工作线程的句柄。
type NotificationSound struct {
	shared  *shared
	sender  chan command
	worker  sync.WaitGroup
	workerM sync.Mutex
}

// New 创建原生（MCI）提示音服务；非 Windows 报不支持。
func New(root string, enabled bool, onError func(string)) (*NotificationSound, error) {
	if runtime.GOOS != "windows" {
		return nil, fmt.Errorf("当前平台暂不支持提示音")
	}
	return withPlayer(newNativePlayer(root), enabled, onError)
}

func withPlayer(player Player, enabled bool, onError func(string)) (*NotificationSound, error) {
	s := &shared{}
	s.enabled.Store(enabled)
	s.alive.Store(true) // 与 Rust Shared::new 一致：创建即视为可用，退出时清除
	sound := &NotificationSound{
		shared: s,
		sender: make(chan command, 1),
	}
	sound.worker.Add(1)
	go func() {
		defer sound.worker.Done()
		run(player, sound.sender, s, onError)
	}()
	return sound, nil
}

func (s *NotificationSound) Available() bool {
	return s.shared.alive.Load() && !s.shared.closing.Load()
}

func (s *NotificationSound) Enabled() bool {
	return s.Available() && s.shared.enabled.Load()
}

// SetEnabled 切换开关并作废在途请求（epoch 前进）。
func (s *NotificationSound) SetEnabled(enabled bool) {
	if s.shared.enabled.Swap(enabled) != enabled {
		s.shared.epoch.Add(1)
		s.wake()
	}
}

// Notify 排队一次自动播放；返回 false 表示队列满或不可用（不阻塞调用线程）。
func (s *NotificationSound) Notify() bool {
	if !s.Available() || !s.shared.enabled.Load() {
		return false
	}
	select {
	case s.sender <- command{play: &playRequest{
		epoch:  s.shared.epoch.Load(),
		queued: time.Now(),
	}}:
		return true
	default:
		return false
	}
}

// Preview 同步试听（静音也可用），3 秒超时只作废这一代请求。
func (s *NotificationSound) Preview() (uint64, error) {
	if !s.Available() {
		return 0, fmt.Errorf("提示音服务已停止")
	}
	epoch := s.shared.epoch.Load()
	reply := make(chan playResult, 1)
	select {
	case s.sender <- command{play: &playRequest{epoch: epoch, queued: time.Now(), reply: reply}}:
	default:
		return 0, fmt.Errorf("提示音正在处理，请稍后试听")
	}
	select {
	case result := <-reply:
		if result.err != nil {
			return 0, result.err
		}
		return result.duration, nil
	case <-time.After(PreviewTimeout):
		// 只作废当前这一代，绝不动用户更新的设置代。
		if s.shared.epoch.CompareAndSwap(epoch, epoch+1) {
			s.wake()
		}
		return 0, fmt.Errorf("提示音试听超时")
	}
}

func (s *NotificationSound) wake() {
	select {
	case s.sender <- command{}:
	default:
	}
}

// Close 请求停止（不等待线程退出）。
func (s *NotificationSound) Close() {
	s.shared.closing.Store(true)
	s.shared.epoch.Add(1)
	s.wake()
}

// Shutdown 停止并等待工作线程退出。
func (s *NotificationSound) Shutdown() error {
	s.Close()
	s.workerM.Lock()
	defer s.workerM.Unlock()
	s.worker.Wait()
	return nil
}

func run(player Player, receiver chan command, s *shared, onError func(string)) {
	defer s.alive.Store(false)
	s.alive.Store(true)
	epoch := s.epoch.Load()
	var playingUntil time.Time
	hasPlaying := false
	lastError, lastErrorAt := "", time.Time{}
	report := func(errMsg string) {
		now := time.Now()
		if lastError == "" || lastError != errMsg || now.Sub(lastErrorAt) >= time.Minute {
			lastError = errMsg
			lastErrorAt = now
			if onError != nil {
				onError(errMsg)
			}
		}
	}
	for {
		if s.closing.Load() {
			break
		}
		current := s.epoch.Load()
		if current != epoch {
			if err := player.Stop(); err != nil {
				report(err.Error())
			}
			hasPlaying = false
			epoch = current
		}
		var cmd command
		if hasPlaying {
			wait := time.Until(playingUntil)
			if wait < 0 {
				wait = 0
			}
			select {
			case c, ok := <-receiver:
				if !ok {
					break
				}
				cmd = c
			case <-time.After(wait):
				if err := player.Stop(); err != nil {
					report(err.Error())
				}
				hasPlaying = false
				continue
			}
		} else {
			c, ok := <-receiver
			if !ok {
				break
			}
			cmd = c
		}
		if s.closing.Load() {
			break
		}
		current = s.epoch.Load()
		if current != epoch {
			if err := player.Stop(); err != nil {
				report(err.Error())
			}
			hasPlaying = false
			epoch = current
		}
		if cmd.play == nil {
			continue
		}
		request := cmd.play
		preview := request.reply != nil
		cancelled := func() bool {
			return s.closing.Load() || s.epoch.Load() != request.epoch ||
				(!preview && !s.enabled.Load()) ||
				time.Since(request.queued) >= deadlineFor(preview)
		}
		if cancelled() {
			if request.reply != nil {
				request.reply <- playResult{err: fmt.Errorf("提示音请求已取消")}
			}
			continue
		}
		if !preview && hasPlaying && time.Now().Before(playingUntil) {
			continue
		}
		duration, err := func() (time.Duration, error) {
			if err := player.Stop(); err != nil {
				return 0, err
			}
			return player.Play(cancelled)
		}()
		if err == nil && cancelled() {
			err = fmt.Errorf("提示音请求已取消")
		}
		if err != nil {
			_ = player.Stop()
			hasPlaying = false
			if !preview && !cancelled() {
				report(err.Error())
			}
		} else {
			playingUntil = time.Now().Add(duration)
			hasPlaying = true
		}
		if request.reply != nil {
			if err != nil {
				request.reply <- playResult{err: err}
			} else {
				request.reply <- playResult{duration: uint64(duration.Milliseconds())}
			}
		}
	}
	_ = player.Stop()
}

func deadlineFor(preview bool) time.Duration {
	if preview {
		return PreviewTimeout
	}
	return Cooldown
}
