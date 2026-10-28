//go:build windows

// Package capture 移植 tauri-rust capture/platform.rs：单显示器物理像素截图。
// 所有原生句柄在捕获调用内释放；纯标准库 syscall，无新增依赖。
package capture

import (
	"bytes"
	"fmt"
	"image"
	"image/png"
	"syscall"
	"unsafe"
)

// MaxSide/MaxPixels 与 media 包的图片上限一致（避免包依赖复制常量）。
const (
	MaxSide   = 16384
	MaxPixels = 16_000_000
)

type Bounds struct {
	X      int32
	Y      int32
	Width  uint32
	Height uint32
}

func (b Bounds) Validate() error {
	if b.Width == 0 || b.Height == 0 || b.Width > MaxSide || b.Height > MaxSide ||
		uint64(b.Width)*uint64(b.Height) > MaxPixels ||
		int64(b.X)+int64(b.Width) > int64(maxInt32) ||
		int64(b.Y)+int64(b.Height) > int64(maxInt32) {
		return fmt.Errorf("显示器尺寸无效或超过1600万像素截图上限")
	}
	return nil
}

const maxInt32 = int64(^uint32(0) >> 1)

var (
	user32              = syscall.NewLazyDLL("user32.dll")
	procGetDC           = user32.NewProc("GetDC")
	procReleaseDC       = user32.NewProc("ReleaseDC")
	procSetThreadDpi    = user32.NewProc("SetThreadDpiAwarenessContext")
	procMonitorFromWin  = user32.NewProc("MonitorFromWindow")
	procGetMonitorInfoW = user32.NewProc("GetMonitorInfoW")

	gdi32               = syscall.NewLazyDLL("gdi32.dll")
	procCreateCompatDC  = gdi32.NewProc("CreateCompatibleDC")
	procCreateCompatBmp = gdi32.NewProc("CreateCompatibleBitmap")
	procSelectObject    = gdi32.NewProc("SelectObject")
	procDeleteObject    = gdi32.NewProc("DeleteObject")
	procDeleteDC        = gdi32.NewProc("DeleteDC")
	procBitBlt          = gdi32.NewProc("BitBlt")
	procGetDIBits       = gdi32.NewProc("GetDIBits")

	dwmapi       = syscall.NewLazyDLL("dwmapi.dll")
	procDwmFlush = dwmapi.NewProc("DwmFlush")
)

const (
	srccopy                 = 0x00CC0020
	captureBlt              = 0x40000000
	biRGB                   = 0
	dibRGBColors            = 0
	dpiPerMonitorV2         = ^uintptr(3) // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = ((DPI_CONTEXT)-4)
	monitorDefaultToNearest = 2
)

// MonitorOfWindow 返回窗口所在显示器的物理像素边界。
func MonitorOfWindow(hwnd uintptr) (Bounds, error) {
	if hwnd == 0 {
		return Bounds{}, fmt.Errorf("主窗口句柄无效")
	}
	monitor, _, _ := procMonitorFromWin.Call(hwnd, monitorDefaultToNearest)
	if monitor == 0 {
		return Bounds{}, fmt.Errorf("无法定位主窗口所在显示器")
	}
	// MONITORINFO: cbSize + rcMonitor(16) + rcWork(16) + dwFlags(4) = 40
	var info [40]byte
	le := uint32(len(info))
	buf := (*byte)(unsafe.Pointer(&info[0]))
	*(*uint32)(unsafe.Pointer(buf)) = le
	r1, _, _ := procGetMonitorInfoW.Call(monitor, uintptr(unsafe.Pointer(buf)))
	if r1 == 0 {
		return Bounds{}, fmt.Errorf("获取显示器信息失败")
	}
	// rcMonitor 在偏移 4
	rcX := int32(*(*uint32)(unsafe.Pointer(&info[4])))
	rcY := int32(*(*uint32)(unsafe.Pointer(&info[8])))
	rcW := uint32(*(*uint32)(unsafe.Pointer(&info[12])))
	rcH := uint32(*(*uint32)(unsafe.Pointer(&info[16])))
	bounds := Bounds{X: rcX, Y: rcY, Width: rcW, Height: rcH}
	if err := bounds.Validate(); err != nil {
		return Bounds{}, err
	}
	return bounds, nil
}

// Capture 截取 bounds 区域，返回 PNG（GDI BGRA → 不透明 RGBA）。
func Capture(bounds Bounds) ([]byte, error) {
	if err := bounds.Validate(); err != nil {
		return nil, err
	}
	// 独立 DPI 上下文：物理像素定位与捕获
	prev, _, prevErr := procSetThreadDpi.Call(dpiPerMonitorV2)
	if prev == 0 {
		return nil, fmt.Errorf("无法设置截图DPI上下文: %v", prevErr)
	}
	defer procSetThreadDpi.Call(prev)
	// 隐藏主窗口后等待桌面合成器完成一帧
	if r, _, err := procDwmFlush.Call(); r == ^uintptr(0) || r>>31 != 0 {
		return nil, fmt.Errorf("等待桌面刷新失败: %v", err)
	}

	screen, _, err := procGetDC.Call(0)
	if screen == 0 {
		return nil, fmt.Errorf("无法访问桌面: %v", err)
	}
	memory, _, err := procCreateCompatDC.Call(screen)
	bitmap, _, err2 := procCreateCompatBmp.Call(screen, uintptr(bounds.Width), uintptr(bounds.Height))
	if memory == 0 || bitmap == 0 {
		releaseHandles(screen, 0, bitmap, 0)
		return nil, fmt.Errorf("无法分配截图缓冲区: %v %v", err, err2)
	}
	old, _, _ := procSelectObject.Call(memory, bitmap)
	if old == 0 || old == ^uintptr(0) {
		releaseHandles(screen, memory, bitmap, 0)
		return nil, fmt.Errorf("无法选择截图位图")
	}
	success := false
	defer func() {
		if !success {
			procSelectObject.Call(memory, old)
		}
		releaseHandles(screen, memory, bitmap, old)
	}()
	if r, _, err := procBitBlt.Call(memory,
		0, 0, uintptr(bounds.Width), uintptr(bounds.Height),
		screen, uintptr(int32(bounds.X)), uintptr(int32(bounds.Y)),
		srccopy|captureBlt); r == 0 {
		return nil, fmt.Errorf("桌面截图失败: %v", err)
	}
	procSelectObject.Call(memory, old)
	old = 0

	// BITMAPINFOHEADER，biHeight 取负 = 自上而下
	var bmi [40]byte
	putU32 := func(at int, v uint32) {
		bmi[at] = byte(v)
		bmi[at+1] = byte(v >> 8)
		bmi[at+2] = byte(v >> 16)
		bmi[at+3] = byte(v >> 24)
	}
	putU32(0, 40)
	putU32(4, uint32(bounds.Width))
	putU32(8, uint32(bounds.Height))
	bmi[12] = 1
	bmi[14] = 32
	putU32(16, biRGB)

	pixels := make([]byte, int(bounds.Width)*int(bounds.Height)*4)
	got, _, err := procGetDIBits.Call(memory, bitmap, 0, uintptr(bounds.Height),
		uintptr(unsafe.Pointer(&pixels[0])), uintptr(unsafe.Pointer(&bmi[0])), dibRGBColors)
	if got != uintptr(bounds.Height) {
		return nil, fmt.Errorf("读取截图像素失败: %v", err)
	}
	success = true
	return encodeBGRA(bounds, pixels)
}

func releaseHandles(screen, memory, bitmap, old uintptr) {
	if old != 0 {
		procSelectObject.Call(memory, old)
	}
	if bitmap != 0 {
		procDeleteObject.Call(bitmap)
	}
	if memory != 0 {
		procDeleteDC.Call(memory)
	}
	if screen != 0 {
		procReleaseDC.Call(0, screen)
	}
}

// encodeBGRA 把 GDI 的 BGRA 像素转成不透明 RGBA 的 PNG。
func encodeBGRA(bounds Bounds, pixels []byte) ([]byte, error) {
	if err := bounds.Validate(); err != nil {
		return nil, err
	}
	if len(pixels) != int(bounds.Width)*int(bounds.Height)*4 {
		return nil, fmt.Errorf("截图像素数据不完整")
	}
	for i := 0; i < len(pixels); i += 4 {
		pixels[i], pixels[i+2] = pixels[i+2], pixels[i]
		pixels[i+3] = 255 // GDI 的第四字节不是透明度
	}
	img := image.NewNRGBA(image.Rect(0, 0, int(bounds.Width), int(bounds.Height)))
	copy(img.Pix, pixels)
	var out bytes.Buffer
	if err := png.Encode(&out, img); err != nil {
		return nil, fmt.Errorf("PNG编码失败：%w", err)
	}
	return out.Bytes(), nil
}
