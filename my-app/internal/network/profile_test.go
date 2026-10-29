package network

import (
	"net"
	"testing"
	"time"

	"my-app/internal/ipmsg"
)

// 回归：飞秋/C++ 版 peer 的 username 字段放的是登录名或机器名，昵称在 body。
// 显示名必须取 body，ID 仍按 username 构造（否则回执/文件来源校验会对不上）。
func TestProfileBodyIsNicknameNotUsername(t *testing.T) {
	h, err := NewHub(0, "我", "", "inst-local")
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	h.Run()

	conn, err := net.DialUDP("udp4", nil, &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: localPort(h)})
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	group := "研发组"
	wire, err := ipmsg.EncodePacket("1", 7, "DESKTOP-ABC", "desktop-abc", ipmsg.CmdBrEntry, "小明", &group)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := conn.Write(wire); err != nil {
		t.Fatal(err)
	}

	waitUntil(t, 2*time.Second, func() bool { return len(h.ListUsers()) > 0 })

	u := h.ListUsers()[0]
	if u.Nick != "小明" {
		t.Errorf("显示名 = %q，期望 body 里的昵称 小明", u.Nick)
	}
	if u.UserName != "DESKTOP-ABC" {
		t.Errorf("UserName = %q，期望报文 username 字段 DESKTOP-ABC", u.UserName)
	}
	if u.Group != "研发组" {
		t.Errorf("分组 = %q，期望 extra 里的 研发组", u.Group)
	}
	if want := "DESKTOP-ABC@desktop-abc#"; len(u.ID) <= len(want) || u.ID[:len(want)] != want {
		t.Errorf("ID = %q，期望以 %s 开头", u.ID, want)
	}
}
