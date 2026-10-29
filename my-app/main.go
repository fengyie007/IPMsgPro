package main

// 迅秋 MyGo 版（迁移中）：UDP 发现 + 文本会话 + 持久化 + 未读托盘闪烁。
// 构建：cd my-app && npm run build；运行：my-app.exe [--port=2425] [--name=...] [--group=...]
// 数据目录：%LOCALAPPDATA%\SpeedIpMsgGo（非默认端口追加 _<端口>）。
// 决策与进度见仓库根 MyGo迁移计划.md。

import (
	"bytes"
	"flag"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"log"
	"math"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strings"
	"sync/atomic"
	"time"

	"github.com/egoist/mygo"

	"my-app/internal/app"
	"my-app/internal/ipmsg"
	"my-app/internal/media"
	"my-app/internal/network"
	"my-app/internal/storage"
	"my-app/internal/store"
)

// EnvelopeEvent 是唯一的前端事件通道：{event, payload}，对应旧 bridge 的 ipmsg-event。
var EnvelopeEvent = mygo.NewEvent[app.Envelope]("ipmsg-event")

const appVersion = "0.1.0-m1"

var (
	hub     *network.Hub
	tray    *mygo.Tray
	backend *app.Runtime
	// exiting 为真时放行窗口关闭：mygo 的 App.Quit 会依次询问各窗口的
	// OnClose，若主窗口仍无条件 PreventDefault，整个退出流程会被取消。
	exiting atomic.Bool
)

// Backend 是暴露给前端的绑定：统一命令分发（对应 Rust 的 ipmsg_command）。
type Backend struct{}

func (Backend) Command(command string, argsJSON string) string {
	return backend.Handle(command, argsJSON)
}

func resolveDataDir(port uint16) string {
	base := os.Getenv("LOCALAPPDATA")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			fatal("定位用户目录失败: %v", err)
		}
		base = filepath.Join(home, "AppData", "Local")
	}
	dir := filepath.Join(base, "SpeedIpMsgGo")
	if port != ipmsg.DefaultPort {
		dir = fmt.Sprintf("%s_%d", dir, port)
	}
	return dir
}

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
	// 打包后没有控制台：任何 panic 都要落日志并弹框，否则表现为“双击没反应”。
	defer func() {
		if r := recover(); r != nil {
			fatal("程序异常退出: %v\n%s", r, debug.Stack())
		}
	}()

	port := flag.Uint("port", ipmsg.DefaultPort, "UDP 端口（默认 2425，与飞秋一致）")
	name := flag.String("name", "迅秋Go", "对外昵称（可被设置覆盖）")
	group := flag.String("group", "", "分组名")
	flag.Parse()

	dataDir := resolveDataDir(uint16(*port))
	setupLog(dataDir)
	exePath, _ := os.Executable()
	log.Printf("启动: version=%s port=%d arch=%s exe=%s", appVersion, *port, runtime.GOARCH, exePath)
	if err := requireWebView(); err != nil {
		fatal("%v", err)
	}
	// 存储管理：bootstrap 记录活动数据集；迁移只发生在启动时机（对拍 storage.rs）。
	var storageErr string
	boot, err := storage.Open(dataDir, uint16(*port))
	if err != nil {
		fatal("解析数据目录失败: %v", err)
	}
	dataDir = boot.Active
	storageErr = boot.OpenError()
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		fatal("创建数据目录失败: %v", err)
	}
	db, err := store.Open(filepath.Join(dataDir, "ipmsg.db"))
	if err != nil {
		fatal("打开数据库失败: %v\n（打包运行时 mygo-sqlite3.dll 必须与 exe 在同一目录）", err)
	}
	defer db.Close()
	configStore, err := store.OpenConfig(filepath.Join(dataDir, "config.json"))
	if err != nil {
		fatal("打开配置失败: %v", err)
	}

	hostname, _ := os.Hostname()
	hub, err = network.NewHub(uint16(*port), *name, *group, "mygo@"+hostname)
	if err != nil {
		fatal("启动网络失败: %v", err)
	}
	defer hub.Close()
	hub.AttachDatabase(db, "local@"+hostname)

	// 文件传输：TCP 与 UDP 同端口，接收目录为 <Downloads>/SpeedIpMsgGo。
	downloads := os.Getenv("USERPROFILE")
	if downloads == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			fatal("定位接收目录失败: %v", err)
		}
		downloads = home
	}
	if err := hub.StartFiles(filepath.Join(downloads, "Downloads", "SpeedIpMsgGo")); err != nil {
		fatal("启动文件服务失败: %v", err)
	}

	backend = app.New(hub, db, configStore, appVersion, dataDir)
	backend.Storage = boot
	backend.StorageError = storageErr
	backend.WireEvents()
	mygo.Bind(Backend{})

	// 图片资产协议：仅服务数据库已登记的资产（校验在 ImageRead 内完成）。
	// 必须在窗口创建前注册。
	assetStore, err := media.NewAssetStore(filepath.Join(dataDir, "images"))
	if err != nil {
		fatal("初始化图片目录失败: %v", err)
	}
	hub.AttachAssets(assetStore)
	mygo.Protocol.HandleFunc("ipmsg-image", func(w http.ResponseWriter, req *http.Request) {
		assetID := strings.TrimPrefix(req.URL.Path, "/")
		_, data, err := hub.ImageRead(assetID, req.URL.Query().Get("t") == "1")
		if err != nil {
			http.Error(w, err.Error(), http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "image/png")
		_, _ = w.Write(data)
	})
	// 截图原图协议：仅服务当前活动会话，仅编辑器窗口可读取。
	mygo.Protocol.HandleFunc("ipmsg-capture", func(w http.ResponseWriter, req *http.Request) {
		sessionID := strings.TrimPrefix(req.URL.Path, "/")
		data, err := backend.ServeScreenshotPNG(sessionID)
		if err != nil {
			http.Error(w, err.Error(), http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "image/png")
		_, _ = w.Write(data)
	})

	log.Printf("迅秋 MyGo 启动: port=%d dataDir=%s wire=%s", hub.Port(), dataDir, hub.WireVersion())

	// 任何退出入口（含系统会话结束）都先置位，否则主窗口的“关闭即隐藏”
	// 逻辑会取消整个退出流程。
	mygo.App.OnBeforeQuit(func(*mygo.QuitEvent) { exiting.Store(true) })
	mygo.App.WhenReady(func() {
		win := mygo.NewWindow(mygo.WindowOptions{
			Title:  "迅秋 (MyGo)",
			Width:  1000,
			Height: 680,
			URL:    "/",
		})
		if win == nil {
			fatal("创建主窗口失败：WebView2 运行时不可用或初始化失败，详见日志")
		}
		backend.SetMainWindow(win)
		win.OnClose(func(e *mygo.CloseEvent) {
			// mygo 的 App.Quit 会依次关闭窗口并询问 OnClose；无条件
			// PreventDefault 会取消整个退出流程。退出中放行关闭。
			if exiting.Load() {
				return
			}
			e.PreventDefault()
			win.Hide()
		})
		// 原生拖放：文件/文件夹直接登记为当前会话的发送预览。
		win.OnFileDrop(func(e *mygo.FileDropEvent) {
			backend.HandleFileDrop(e.Paths)
		})

		tray, err := mygo.NewTray(mygo.TrayOptions{
			ID:      "speedipmsg-mygo",
			Icon:    trayIconPNG(color.RGBA{R: 59, G: 130, B: 246, A: 255}),
			ToolTip: fmt.Sprintf("迅秋 MyGo · %s", hub.Nick()),
			Menu: mygo.NewMenu([]*mygo.MenuItem{
				{Label: "打开主窗口", Click: func(*mygo.MenuItem, *mygo.Window) { win.Show() }},
				mygo.Separator(),
				{Label: "退出", Click: func(*mygo.MenuItem, *mygo.Window) {
					exiting.Store(true)
					// 关闭传输/提示音要等后台工作线程，最多约 2 秒；放到后台执行，
					// 避免阻塞托盘菜单所在的消息循环。
					go func() {
						if transfers, err := hub.Files(); err == nil {
							transfers.Shutdown()
						}
						backend.ShutdownNotification()
						hub.AnnounceExit()
						time.Sleep(100 * time.Millisecond)
						mygo.App.Quit()
					}()
				}},
			}),
		})
		if err != nil {
			log.Printf("创建托盘失败: %v", err)
		} else {
			tray.OnClick(func() { win.Show() })
			go runBlink(win, tray)
		}

		backend.SetEmit(func(env app.Envelope) {
			_ = EnvelopeEvent.Emit(win, env)
		})

		hub.Run()
		hub.AnnounceEntry()
	})

	if err := mygo.App.Run(); err != nil {
		fatal("运行失败: %v", err)
	}
}
