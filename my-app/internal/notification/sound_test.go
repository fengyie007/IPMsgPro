package notification

// 对拍 tauri-rust notification/tests.rs：策略闸门 4 项 + 播放工作线程 6 项。

import (
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func messageEvent(id, kind string) Event {
	return Event{Event: "message.received", Payload: map[string]any{
		"id": id, "from": "peer", "type": kind, "content": "测试",
	}}
}

func background() Context {
	return Context{
		Enabled: true, Accepting: true, Visible: true, Focused: false, Minimized: false,
		ActiveConversation: "peer", LocalID: "local",
	}
}

func TestOnlyNewIncomingMessageKindsAreEligible(t *testing.T) {
	now := time.Now()
	for _, kind := range []string{"text", "image", "file", "knock"} {
		var gate NotificationGate
		if !gate.ShouldPlay(messageEvent("new", kind), background(), now) {
			t.Fatalf("类型 %s 应该发声", kind)
		}
	}
	for _, name := range []string{"history.get", "message.ack", "image.send_completed",
		"file.updated", "user.discovered", "network.scan_complete"} {
		var gate NotificationGate
		candidate := messageEvent("new", "text")
		candidate.Event = name
		if gate.ShouldPlay(candidate, background(), now) {
			t.Fatalf("事件 %s 不应发声", name)
		}
	}
	for _, from := range []string{"local", "self", ""} {
		var gate NotificationGate
		candidate := messageEvent("new", "text")
		candidate.Payload["from"] = from
		if gate.ShouldPlay(candidate, background(), now) {
			t.Fatalf("来自 %q 不应发声", from)
		}
	}
	var gate NotificationGate
	if gate.ShouldPlay(messageEvent("", "text"), background(), now) {
		t.Fatalf("空 ID 不应发声")
	}
	if gate.ShouldPlay(messageEvent("new", "unknown"), background(), now) {
		t.Fatalf("未知类型不应发声")
	}
}

func TestForegroundConversationIsQuietButBackgroundIsNot(t *testing.T) {
	reading := background()
	reading.Focused = true
	var quiet NotificationGate
	if quiet.ShouldPlay(messageEvent("1", "text"), reading, time.Now()) {
		t.Fatalf("正在阅读的会话应静音")
	}
	cases := []Context{background()}
	other := reading
	other.ActiveConversation = "other"
	cases = append(cases, other)
	none := reading
	none.ActiveConversation = ""
	cases = append(cases, none)
	hidden := reading
	hidden.Visible = false
	cases = append(cases, hidden)
	mini := reading
	mini.Minimized = true
	cases = append(cases, mini)
	for i, context := range cases {
		var gate NotificationGate
		if !gate.ShouldPlay(messageEvent("1", "text"), context, time.Now()) {
			t.Fatalf("场景 %d 应该发声", i)
		}
	}
	var mutedGate NotificationGate
	muted := background()
	muted.Accepting = false
	if mutedGate.ShouldPlay(messageEvent("1", "text"), muted, time.Now()) {
		t.Fatalf("accepting=false 不应发声")
	}
}

func TestMutedAndReadMessagesDoNotRealertAfterToggle(t *testing.T) {
	now := time.Now()
	for _, context := range []Context{
		{Enabled: false, ActiveConversation: "peer", Accepting: true, Visible: true, LocalID: "local"},
		{Focused: true, ActiveConversation: "peer", Accepting: true, Visible: true, Enabled: true, LocalID: "local"},
	} {
		var gate NotificationGate
		if gate.ShouldPlay(messageEvent("same", "image"), context, now) {
			t.Fatalf("静音场景不应发声")
		}
		if gate.ShouldPlay(messageEvent("same", "image"), background(), now.Add(2*time.Second)) {
			t.Fatalf("切换开关后同一条不应补响")
		}
		if !gate.ShouldPlay(messageEvent("different", "image"), background(), now.Add(2*time.Second)) {
			t.Fatalf("新消息应该发声")
		}
	}
}

func TestBurstsAreCoalescedAndMemoryBounded(t *testing.T) {
	now := time.Now()
	var gate NotificationGate
	sounds := 0
	for n := 0; n < 1000; n++ {
		if gate.ShouldPlay(messageEvent(fmt.Sprint(n), "file"), background(), now) {
			sounds++
		}
	}
	if sounds != 1 {
		t.Fatalf("爆发应合并为一次: %d", sounds)
	}
	if len(gate.seen) != SeenLimit || len(gate.order) != SeenLimit {
		t.Fatalf("去重记忆应有界: %d %d", len(gate.seen), len(gate.order))
	}
	if !gate.ShouldPlay(messageEvent("next", "text"), background(), now.Add(Cooldown)) {
		t.Fatalf("冷却后应发声")
	}
	if gate.ShouldPlay(messageEvent("next", "text"), background(), now.Add(2*Cooldown)) {
		t.Fatalf("同一条不应响第二次")
	}
}

type counts struct {
	attempts, started, stopped, cancelled atomic.Int32
}

type fakePlayer struct {
	mu         sync.Mutex
	counts     *counts
	entered    chan struct{}
	release    chan struct{}
	failFirst  bool
	failAlways bool
}

func (p *fakePlayer) Play(cancelled func() bool) (time.Duration, error) {
	attempt := p.counts.attempts.Add(1)
	p.mu.Lock()
	entered, release := p.entered, p.release
	p.mu.Unlock()
	if entered != nil {
		entered <- struct{}{}
		select {
		case <-release:
		case <-time.After(2 * time.Second):
			return 0, fmt.Errorf("release timeout")
		}
		p.mu.Lock()
		p.entered, p.release = nil, nil
		p.mu.Unlock()
	}
	if cancelled() {
		p.counts.cancelled.Add(1)
		return 0, fmt.Errorf("cancelled")
	}
	if p.failAlways || (p.failFirst && attempt == 1) {
		return 0, fmt.Errorf("device unavailable")
	}
	p.counts.started.Add(1)
	return 10 * time.Second, nil
}

func (p *fakePlayer) Stop() error {
	p.counts.stopped.Add(1)
	return nil
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("等待 %s 超时", what)
}

func TestPreviewWorksWhileMutedAndShutdownStopsPlayback(t *testing.T) {
	c := &counts{}
	sound, err := withPlayer(&fakePlayer{counts: c}, false, func(string) {})
	if err != nil {
		t.Fatal(err)
	}
	if sound.Notify() {
		t.Fatalf("静音时 notify 应失败")
	}
	duration, err := sound.Preview()
	if err != nil || duration != 10000 {
		t.Fatalf("试听异常: %v %v", duration, err)
	}
	if sound.Enabled() {
		t.Fatalf("试听不应改变静音状态")
	}
	if c.started.Load() != 1 {
		t.Fatalf("应播放一次: %d", c.started.Load())
	}
	_ = sound.Shutdown()
	if sound.Available() {
		t.Fatalf("关闭后应不可用")
	}
	if _, err := sound.Preview(); err == nil {
		t.Fatalf("关闭后试听应报错")
	}
	if c.stopped.Load() < 2 {
		t.Fatalf("关闭应至少 stop 两次: %d", c.stopped.Load())
	}
}

func TestDisableInvalidatesInFlightAndQueuedRequests(t *testing.T) {
	c := &counts{}
	player := &fakePlayer{counts: c, entered: make(chan struct{}), release: make(chan struct{})}
	sound, err := withPlayer(player, true, func(string) {})
	if err != nil {
		t.Fatal(err)
	}
	if !sound.Notify() {
		t.Fatalf("notify 应成功")
	}
	<-player.entered
	if !sound.Notify() {
		t.Fatalf("第二个请求应排队")
	}
	if sound.Notify() {
		t.Fatalf("队列只有一个位置")
	}
	sound.SetEnabled(false)
	if sound.Notify() {
		t.Fatalf("禁用后 notify 应失败")
	}
	player.release <- struct{}{}
	waitFor(t, "取消在途请求", func() bool { return c.cancelled.Load() == 1 })
	for {
		duration, err := sound.Preview()
		if err == nil {
			if duration != 10000 {
				t.Fatalf("试听时长异常: %d", duration)
			}
			break
		}
		if err.Error() != "提示音正在处理，请稍后试听" {
			t.Fatalf("试听异常: %v", err)
		}
		time.Sleep(5 * time.Millisecond)
	}
	if c.attempts.Load() != 2 || c.started.Load() != 1 {
		t.Fatalf("计数异常: attempts=%d started=%d", c.attempts.Load(), c.started.Load())
	}
	_ = sound.Shutdown()
}

func TestShutdownCancelsPendingPreview(t *testing.T) {
	c := &counts{}
	player := &fakePlayer{counts: c, entered: make(chan struct{}), release: make(chan struct{})}
	sound, _ := withPlayer(player, true, func(string) {})
	sound.Notify()
	<-player.entered
	type previewResult struct {
		duration uint64
		err      error
	}
	done := make(chan previewResult, 1)
	go func() {
		d, err := sound.Preview()
		done <- previewResult{d, err}
	}()
	time.Sleep(20 * time.Millisecond)
	sound.Close()
	player.release <- struct{}{}
	result := <-done
	if result.err == nil {
		t.Fatalf("关闭应取消试听")
	}
	_ = sound.Shutdown()
	if c.started.Load() != 0 {
		t.Fatalf("关闭后不应有播放: %d", c.started.Load())
	}
}

func TestPreviewFailureIsNotSuccessAndWorkerRecovers(t *testing.T) {
	c := &counts{}
	player := &fakePlayer{counts: c, failFirst: true}
	sound, err := withPlayer(player, false, func(string) {
		t.Fatalf("试听错误不应走 onError")
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := sound.Preview(); err == nil || err.Error() != "device unavailable" {
		t.Fatalf("首次试听应失败: %v", err)
	}
	if duration, err := sound.Preview(); err != nil || duration != 10000 {
		t.Fatalf("恢复后试听应成功: %v %v", duration, err)
	}
	if c.started.Load() != 1 {
		t.Fatalf("应成功播放一次: %d", c.started.Load())
	}
	_ = sound.Shutdown()
}

func TestRepeatedAutomaticErrorsThrottledButPreviewReports(t *testing.T) {
	c := &counts{}
	reports := atomic.Int32{}
	player := &fakePlayer{counts: c, failAlways: true}
	sound, _ := withPlayer(player, true, func(string) { reports.Add(1) })
	sound.Notify()
	waitFor(t, "首次错误上报", func() bool { return reports.Load() == 1 })
	sound.Notify()
	for {
		_, err := sound.Preview()
		if err == nil {
			t.Fatalf("试听应失败")
		}
		if err.Error() == "提示音正在处理，请稍后试听" {
			time.Sleep(5 * time.Millisecond)
			continue
		}
		if err.Error() != "device unavailable" {
			t.Fatalf("试听错误异常: %v", err)
		}
		break
	}
	if c.attempts.Load() != 3 || reports.Load() != 1 {
		t.Fatalf("计数异常: attempts=%d reports=%d", c.attempts.Load(), reports.Load())
	}
	_ = sound.Shutdown()
}

func TestEmbeddedSoundFileCacheIsUniqueAndDiscarded(t *testing.T) {
	// 对拍 native.rs 内嵌音缓存测试（跨平台部分；MCI 部分实机验证）。
	root := t.TempDir()
	sentinel := root + "/notification.mp3"
	if err := writeFile(sentinel, []byte("user file")); err != nil {
		t.Fatal(err)
	}
	one, err := createSoundFile(root)
	if err != nil {
		t.Fatal(err)
	}
	two, err := createSoundFile(root)
	if err != nil {
		t.Fatal(err)
	}
	if one.path == two.path {
		t.Fatalf("缓存文件应唯一")
	}
	data, err := readFile(one.path)
	if err != nil || string(data) != string(audio) {
		t.Fatalf("缓存内容应等于内嵌音源: %v", err)
	}
	kept, err := readFile(sentinel)
	if err != nil || string(kept) != "user file" {
		t.Fatalf("用户文件不应被覆盖")
	}
	one.Discard()
	two.Discard()
	if _, err := readFile(one.path); err == nil {
		t.Fatalf("缓存文件应被删除")
	}
}
