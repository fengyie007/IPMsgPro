package main

// MyGo 最小闭环 demo：UDP 发现 + 单会话文本收发 + 未读托盘闪烁。
// 构建：cd my-app && go build .（或 npm run build 走 mygo 打包）。
// 运行：my-app.exe [--port=2425] [--name=MyGoDemo] [--group=MyGo]

import (
	"bytes"
	"flag"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"log"
	"math"
	"os"
	"time"

	"github.com/egoist/mygo"
)

// 绑定到前端的事件：`mygo generate` 会生成类型安全的 events.usersChanged / events.messageChanged。
var (
	UsersChanged   = mygo.NewEvent[bool]("users-changed")
	MessageChanged = mygo.NewEvent[string]("message-changed")
)

var (
	hub      *Hub
	mainWin  *mygo.Window
	trayIcon *mygo.Tray
)

type RunInfo struct {
	Port        uint16 `json:"port"`
	Nick        string `json:"nick"`
	Group       string `json:"group"`
	Hostname    string `json:"hostname"`
	WireVersion string `json:"wireVersion"`
	TotalUnread int    `json:"totalUnread"`
}

// Chat 是暴露给前端的绑定结构：`mygo generate` 生成 Chat.* 类型化客户端。
type Chat struct{}

func (Chat) Info() RunInfo {
	return RunInfo{
		Port:        hub.Port(),
		Nick:        hub.nick,
		Group:       hub.group,
		Hostname:    hub.hostname,
		WireVersion: hub.WireVersion(),
		TotalUnread: hub.TotalUnread(),
	}
}

func (Chat) ListUsers() []User { return hub.ListUsers() }

func (Chat) Unread() map[string]int {
	out := map[string]int{}
	for _, u := range hub.ListUsers() {
		if n := hub.Unread(u.Key); n > 0 {
			out[u.Key] = n
		}
	}
	return out
}

func (Chat) History(key string) []Message { return hub.History(key) }

func (Chat) Send(key, text string) error { return hub.SendText(key, text) }

func (Chat) MarkRead(key string) { hub.MarkRead(key) }

func (Chat) Refresh() { hub.Refresh() }

// trayIconPNG 生成 32x32 单色圆形 PNG（彩色/灰色两枚用于未读闪烁）。
func trayIconPNG(c color.RGBA) []byte {
	const size = 32
	const radius = 13.0
	center := float64(size)/2 - 0.5
	img := image.NewNRGBA(image.Rect(0, 0, size, size))
	for y := 0; y < size; y++ {
		for x := 0; x < size; x++ {
			dx, dy := float64(x)-center, float64(y)-center
			d := math.Sqrt(dx*dx + dy*dy)
			a := 255 * (radius + 1 - d)
			if a > 255 {
				a = 255
			}
			if a <= 0 {
				continue
			}
			img.SetNRGBA(x, y, color.NRGBA{R: c.R, G: c.G, B: c.B, A: uint8(a)})
		}
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		log.Fatal(err)
	}
	return buf.Bytes()
}

// runBlink 每 500ms 检查一次：有未读且窗口未聚焦时在彩色/灰色图标间交替，否则恢复彩色。
func runBlink(win *mygo.Window, tray *mygo.Tray) {
	colorIcon := trayIconPNG(color.RGBA{R: 59, G: 130, B: 246, A: 255})
	grayIcon := trayIconPNG(color.RGBA{R: 128, G: 128, B: 128, A: 255})
	swapped, showGray := false, false
	for range time.Tick(500 * time.Millisecond) {
		shouldBlink := hub.TotalUnread() > 0 && !win.IsFocused()
		if !shouldBlink {
			if swapped {
				swapped = false
				if err := tray.SetIcon(colorIcon, false); err != nil {
					log.Printf("恢复托盘图标失败: %v", err)
				}
			}
			continue
		}
		icon := colorIcon
		if showGray {
			icon = grayIcon
		}
		showGray = !showGray
		if err := tray.SetIcon(icon, false); err != nil {
			log.Printf("切换托盘图标失败: %v", err)
		}
		swapped = true
	}
}

func main() {
	port := flag.Uint("port", ipmsgDefaultPort, "UDP 端口（默认 2425，与飞秋一致）")
	name := flag.String("name", "MyGoDemo", "对外昵称")
	group := flag.String("group", "MyGo", "分组名")
	flag.Parse()

	hostname, err := os.Hostname()
	if err != nil {
		hostname = "mygo"
	}
	hub, err = NewHub(uint16(*port), *name, *group, fmt.Sprintf("mygo-demo@%s", hostname))
	if err != nil {
		log.Fatal(err)
	}
	defer hub.Close()
	log.Printf("MyGo demo 启动: port=%d nick=%q wire=%s", hub.Port(), hub.nick, hub.WireVersion())

	mygo.Bind(Chat{})

	mygo.App.WhenReady(func() {
		win := mygo.NewWindow(mygo.WindowOptions{
			Title:  "MyGo IPMsg Demo",
			Width:  860,
			Height: 560,
			URL:    "/",
		})
		mainWin = win
		// 关闭窗口 = 隐藏到托盘；退出走托盘菜单。
		win.OnClose(func(e *mygo.CloseEvent) {
			e.PreventDefault()
			win.Hide()
		})

		tray, err := mygo.NewTray(mygo.TrayOptions{
			ID:      "mygo-demo",
			Icon:    trayIconPNG(color.RGBA{R: 59, G: 130, B: 246, A: 255}),
			ToolTip: fmt.Sprintf("MyGo Demo · %s", hub.nick),
			Menu: mygo.NewMenu([]*mygo.MenuItem{
				{Label: "打开主窗口", Click: func(*mygo.MenuItem, *mygo.Window) { win.Show() }},
				mygo.Separator(),
				{Label: "退出", Click: func(*mygo.MenuItem, *mygo.Window) {
					hub.AnnounceExit()
					time.Sleep(100 * time.Millisecond) // 让下线公告发出
					mygo.App.Quit()
				}},
			}),
		})
		if err != nil {
			log.Printf("创建托盘失败: %v", err)
		} else {
			trayIcon = tray
			tray.OnClick(func() { win.Show() })
			go runBlink(win, tray)
		}

		hub.UsersChanged = func() {
			if mainWin != nil {
				UsersChanged.Emit(mainWin, true)
			}
		}
		hub.ReceivedMessage = func(peer string) {
			if mainWin != nil {
				MessageChanged.Emit(mainWin, peer)
			}
		}

		hub.Run()
		hub.AnnounceEntry()
	})

	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}
