//go:build windows

package app

import (
	"syscall"
	"unsafe"
)

var procFlashWindowEx = syscall.NewLazyDLL("user32.dll").NewProc("FlashWindowEx")

// flashWInfo 对应 Win32 的 FLASHWINFO：除 hwnd 外全是 32 位字段，
// 64 位下 cbSize 之后有 4 字节对齐填充，总长 32 字节。
type flashWInfo struct {
	Size    uint32
	HWnd    uintptr
	Flags   uint32
	Count   uint32
	Timeout uint32
}

const (
	flashwTray = 0x2
	// 与 Rust tao 的 UserAttentionType::Informational 一致：闪 4 次后自行停止。
	flashCount = 4
)

// flashTaskbar 闪烁任务栏按钮请求注意。直接用 Win32 而不走 mygo.Window.FlashFrame：
// 后者用 FLASHW_ALL|FLASHW_TIMERNOFG（连标题栏一起、闪到窗口回前台为止），
// 实测收不到提示；这里照抄 Rust 版的 FLASHW_TRAY + 4 次。
// 窗口隐藏到托盘时没有任务栏按钮，调用无副作用。
func flashTaskbar(hwnd uintptr) {
	if hwnd == 0 {
		return
	}
	fi := flashWInfo{Size: uint32(unsafe.Sizeof(flashWInfo{})), HWnd: hwnd, Flags: flashwTray, Count: flashCount}
	procFlashWindowEx.Call(uintptr(unsafe.Pointer(&fi)))
}
