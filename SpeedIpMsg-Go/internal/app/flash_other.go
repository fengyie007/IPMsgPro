//go:build !windows

package app

// flashTaskbar 在非 Windows 平台没有任务栏按钮，无事可做。
func flashTaskbar(uintptr) {}
