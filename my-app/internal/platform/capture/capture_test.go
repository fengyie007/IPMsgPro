//go:build windows

package capture

// 捕获测试对拍 tauri-rust capture/platform.rs 的同名用例（纯逻辑部分）。

import (
	"bytes"
	"image"
	"image/png"
	"testing"
)

func TestNegativeMonitorCoordinatesAndSizeLimits(t *testing.T) {
	if err := (Bounds{X: -3840, Y: -100, Width: 3840, Height: 2160}).Validate(); err != nil {
		t.Fatal(err)
	}
	for _, size := range [][2]uint32{{0, 10}, {10, 0}, {8000, 4000}, {MaxSide + 1, 1}} {
		if err := (Bounds{Width: size[0], Height: size[1]}).Validate(); err == nil {
			t.Errorf("尺寸 %v 应被拒绝", size)
		}
	}
	if err := (Bounds{X: int32(^uint32(0) >> 1), Width: 2, Height: 2}).Validate(); err == nil {
		t.Error("溢出坐标应被拒绝")
	}
}

func TestNativeBGRAIsOpaqueTopDownPNG(t *testing.T) {
	bounds := Bounds{X: -1, Y: 0, Width: 1, Height: 2}
	pngBytes, err := encodeBGRA(bounds, []byte{0, 0, 255, 0, 255, 0, 0, 123})
	if err != nil {
		t.Fatal(err)
	}
	img, err := png.Decode(bytes.NewReader(pngBytes))
	if err != nil {
		t.Fatal(err)
	}
	c1 := img.At(0, 0)
	r, g, b, a := c1.RGBA()
	if r>>8 != 255 || g>>8 != 0 || b>>8 != 0 || a>>8 != 255 {
		t.Fatalf("首像素应为红色不透明: %d %d %d %d", r>>8, g>>8, b>>8, a>>8)
	}
	c2 := img.At(0, 1)
	r, g, b, a = c2.RGBA()
	if r>>8 != 0 || g>>8 != 0 || b>>8 != 255 || a>>8 != 255 {
		t.Fatalf("次像素应为蓝色不透明: %d %d %d %d", r>>8, g>>8, b>>8, a>>8)
	}
	if _, err := encodeBGRA(bounds, []byte{0, 0, 0, 0}); err == nil {
		t.Fatal("像素长度不符应报错")
	}
	// 解码尺寸
	if img.Bounds().Dx() != 1 || img.Bounds().Dy() != 2 {
		t.Fatalf("PNG 尺寸异常: %v", img.Bounds())
	}
	var _ = image.Rect
}
