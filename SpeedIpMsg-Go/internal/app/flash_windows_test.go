//go:build windows

package app

import (
	"testing"
	"unsafe"
)

// FLASHWINFO 的布局必须和 Win32 一致，否则 FlashWindowEx 会拒绝（返回 FALSE）。
func TestFlashWInfoLayout(t *testing.T) {
	var fi flashWInfo
	if got, want := unsafe.Sizeof(fi), uintptr(32); got != want {
		t.Fatalf("FLASHWINFO 大小 = %d，期望 %d", got, want)
	}
	if got, want := unsafe.Offsetof(fi.HWnd), uintptr(8); got != want {
		t.Fatalf("hwnd 偏移 = %d，期望 %d", got, want)
	}
	if got, want := unsafe.Offsetof(fi.Flags), uintptr(16); got != want {
		t.Fatalf("dwFlags 偏移 = %d，期望 %d", got, want)
	}
}
