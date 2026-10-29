//go:build windows

package main

import (
	"fmt"
	"syscall"
	"unsafe"

	"github.com/egoist/mygo"
)

var procMessageBoxW = syscall.NewLazyDLL("user32.dll").NewProc("MessageBoxW")

const (
	mbOK             = 0x00000000
	mbIconError      = 0x00000010
	mbSetForeground  = 0x00010000
	webView2Download = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
)

// showErrorBox 用 Win32 MessageBox 报错，不依赖 WebView2 或 mygo 窗口。
func showErrorBox(title, text string) {
	t, _ := syscall.UTF16PtrFromString(title)
	m, _ := syscall.UTF16PtrFromString(text)
	procMessageBoxW.Call(0, uintptr(unsafe.Pointer(m)), uintptr(unsafe.Pointer(t)), mbOK|mbIconError|mbSetForeground)
}

// requireWebView 在启动早期确认 WebView2 运行时可用，否则给出明确提示（Win10 默认可能没装）。
func requireWebView() error {
	if mygo.WebViewAvailable() {
		return nil
	}
	return fmt.Errorf("缺少 Microsoft Edge WebView2 运行时，无法显示界面。\n请安装后重试：%s", webView2Download)
}
