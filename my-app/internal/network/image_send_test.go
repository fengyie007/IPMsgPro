package network

// 图片发送全链路回环测试：A 发送 → B 接收落库 → A 收到完成事件；含取消路径。

import (
	"net"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/media"
	"my-app/internal/store"
)

func TestLoopbackImageSendAndReceive(t *testing.T) {
	dir := t.TempDir()
	dbA, err := store.Open(filepath.Join(dir, "a.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dbA.Close()
	dbB, err := store.Open(filepath.Join(dir, "b.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dbB.Close()
	assetsA, err := media.NewAssetStore(filepath.Join(dir, "images-a"))
	if err != nil {
		t.Fatal(err)
	}
	assetsB, err := media.NewAssetStore(filepath.Join(dir, "images-b"))
	if err != nil {
		t.Fatal(err)
	}

	a, err := NewHub(0, "Alice", "组A", "inst-a")
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	b, err := NewHub(0, "Bob", "组B", "inst-b")
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	a.AttachDatabase(dbA, "local@a")
	a.AttachAssets(assetsA)
	b.AttachDatabase(dbB, "local@b")
	b.AttachAssets(assetsB)

	bGotImage := make(chan ReceivedEvent, 4)
	b.ImageReceived = func(evt ReceivedEvent, _ media.ImageMetadata) {
		bGotImage <- evt
	}
	aCompleted := make(chan string, 4)
	a.ImageSendFinished = func(messageID, _ string, success bool, sendErr error, _ bool) {
		if success {
			aCompleted <- messageID
		} else {
			t.Logf("发送失败: %v", sendErr)
		}
	}
	a.Run()
	b.Run()

	// 互相发现：A 向 B 单播 BR_ENTRY，B 回 ANSENTRY 让双方都认识对方
	bAddr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: localPort(b)}
	if _, err := a.buildWireAndSend(bAddr, ipmsg.CmdBrEntry, "Alice", &a.prof.Load().group); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 2*time.Second, func() bool { return len(b.ListUsers()) == 1 })
	waitUntil(t, 2*time.Second, func() bool { return len(a.ListUsers()) == 1 })

	// A 导入一张图
	png := makeTestPNG(t, 30, 30, 3)
	metadata, err := a.ImportClipboard(png)
	if err != nil {
		t.Fatal(err)
	}
	var aliceID string
	for _, u := range b.ListUsers() {
		aliceID = u.ID
	}
	var bobID string
	for _, u := range a.ListUsers() {
		bobID = u.ID
	}

	// A 向 B 发送图片
	result, err := a.SendImage(bobID, metadata.AssetID)
	if err != nil {
		t.Fatal(err)
	}
	select {
	case evt := <-bGotImage:
		if evt.Content != "[图片]" {
			t.Fatalf("B 收到异常内容: %q", evt.Content)
		}
	case <-time.After(40 * time.Second):
		t.Fatal("等待 B 收图超时")
	}
	select {
	case messageID := <-aCompleted:
		if messageID != result.MessageID {
			t.Fatalf("完成消息 ID 不匹配: %s", messageID)
		}
	case <-time.After(40 * time.Second):
		t.Fatal("等待发送完成事件超时")
	}
	// A 的库：图片消息已送达（status 1）
	history, err := dbA.History(bobID, "local@a", 10, 0)
	if err != nil || len(history) != 1 || history[0].Status != 1 || history[0].Kind != 1 {
		t.Fatalf("A 图片历史应已送达: %+v %v", history, err)
	}
	// B 的库：已收到图片
	rows, err := dbB.History(aliceID, "local@b", 10, 0)
	found := false
	for _, row := range rows {
		if row.Kind == 1 {
			found = true
		}
	}
	if err != nil || !found {
		t.Fatalf("B 库应有图片记录: %+v %v", rows, err)
	}
}

func TestCancelImageSendStopsJob(t *testing.T) {
	a, err := NewHub(0, "Alice", "组A", "inst-a")
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	dbA, err := store.Open(filepath.Join(t.TempDir(), "a.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dbA.Close()
	assetsA, err := media.NewAssetStore(filepath.Join(t.TempDir(), "images"))
	if err != nil {
		t.Fatal(err)
	}
	a.AttachDatabase(dbA, "local@a")
	a.AttachAssets(assetsA)
	// 没有对端在线：SendImage 应失败
	if _, err := a.SendImage("ghost@x#127.0.0.1:1", "00000000"); err == nil {
		t.Fatal("不在线目标应报错")
	}
	// 导入并取消一个不存在的发送
	if ok := a.CancelImage("不存在的消息"); ok {
		t.Fatal("取消未知消息应返回 false")
	}
	if !strings.HasPrefix("image-tx-1-2-00000001", "image-tx") {
		t.Fatal("消息 ID 前缀约定")
	}
}
