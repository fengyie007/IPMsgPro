//go:build !windows

// Package capture：非 Windows 平台暂不支持截图（对照 Rust cfg(not(windows)) 分支）。
package capture

import "fmt"

type Bounds struct {
	X      int32
	Y      int32
	Width  uint32
	Height uint32
}

func (b Bounds) Validate() error { return nil }

func MonitorOfWindow(uintptr) (Bounds, error) {
	return Bounds{}, fmt.Errorf("当前平台暂不支持截图")
}

func Capture(Bounds) ([]byte, error) {
	return nil, fmt.Errorf("当前平台暂不支持截图")
}
