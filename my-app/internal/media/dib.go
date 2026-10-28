// dib.go 对应 tauri-rust dib.rs：在转成受管 PNG 资产前校验图片载荷。
// 飞秋 LZW 容器装的是 DIB（不是完整 BMP 文件）。
// 已知限制：Go 标准库不解析 EXIF 方向（Rust 版有），导入相机 JPEG 可能不旋转；BMP 仅支持 24/32 位 BI_RGB。
package media

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"image"
	"image/color"
	"image/png"
)

const (
	MaxPixels         = 16_000_000
	MaxSide           = 16384
	MaxPNGBytes       = 64 * 1024 * 1024
	MaxThumbnailBytes = 2 * 1024 * 1024
	MaxImportBytes    = 20 * 1024 * 1024
	thumbWidth        = 320
	thumbHeight       = 240
)

type DecodedImage struct {
	PNG          []byte
	ThumbnailPNG []byte
	Width        uint32
	Height       uint32
}

func u16At(b []byte, at int) (uint16, error) {
	if at+2 > len(b) {
		return 0, fmt.Errorf("图片头被截断")
	}
	return binary.LittleEndian.Uint16(b[at:]), nil
}

func u32At(b []byte, at int) (uint32, error) {
	if at+4 > len(b) {
		return 0, fmt.Errorf("图片头被截断")
	}
	return binary.LittleEndian.Uint32(b[at:]), nil
}

func checkDimensions(width, height uint32) error {
	if width == 0 || height == 0 || width > MaxSide || height > MaxSide ||
		uint64(width)*uint64(height) > MaxPixels {
		return fmt.Errorf("图片尺寸为空或超过1600万像素限制")
	}
	return nil
}

// decodeDIB 解析 DIB 头并输出自上而下的 RGBA 像素（alpha 恒 255）。
func decodeDIB(dib []byte) (uint32, uint32, []byte, error) {
	if len(dib) < 40 || len(dib) > MaxDIBBytes {
		return 0, 0, nil, fmt.Errorf("DIB长度无效")
	}
	header, err := u32At(dib, 0)
	if err != nil {
		return 0, 0, nil, err
	}
	if header != 40 && header != 52 && header != 56 && header != 108 && header != 124 ||
		int(header) > len(dib) {
		return 0, 0, nil, fmt.Errorf("不支持或截断的DIB头")
	}
	signedWidth, err := u32At(dib, 4)
	if err != nil {
		return 0, 0, nil, err
	}
	signedHeight, err := u32At(dib, 8)
	if err != nil {
		return 0, 0, nil, err
	}
	width := signedWidth
	if int32(signedWidth) <= 0 || int32(signedHeight) == 0 {
		return 0, 0, nil, fmt.Errorf("DIB宽高无效")
	}
	// 先在 int64 域取绝对值，避免 i32::MIN 溢出
	sh := int64(int32(signedHeight))
	if sh < 0 {
		sh = -sh
	}
	height64 := uint64(sh)
	if height64 > MaxSide {
		return 0, 0, nil, fmt.Errorf("DIB高度超过限制")
	}
	height := uint32(height64)
	if err := checkDimensions(width, height); err != nil {
		return 0, 0, nil, err
	}
	bits, err := u16At(dib, 14)
	if err != nil {
		return 0, 0, nil, err
	}
	planes, err := u16At(dib, 12)
	if err != nil {
		return 0, 0, nil, err
	}
	if planes != 1 || (bits != 24 && bits != 32) {
		return 0, 0, nil, fmt.Errorf("仅支持单平面24/32位DIB")
	}
	compression, err := u32At(dib, 16)
	if err != nil {
		return 0, 0, nil, err
	}
	if compression != 0 {
		return 0, 0, nil, fmt.Errorf("暂不支持压缩或位掩码DIB")
	}
	clrUsed, err := u32At(dib, 32)
	if err != nil {
		return 0, 0, nil, err
	}
	if clrUsed != 0 {
		return 0, 0, nil, fmt.Errorf("暂不支持带颜色表的DIB")
	}
	stride := (uint64(width)*uint64(bits) + 31) / 32 * 4
	pixels := stride * height64
	expected := uint64(header) + pixels
	if expected != uint64(len(dib)) {
		return 0, 0, nil, fmt.Errorf("DIB像素范围与数据长度不一致")
	}
	sizeImage, err := u32At(dib, 20)
	if err != nil {
		return 0, 0, nil, err
	}
	if sizeImage != 0 && uint64(sizeImage) != pixels {
		return 0, 0, nil, fmt.Errorf("DIB声明的像素长度不一致")
	}

	rgba := make([]byte, uint64(width)*height64*4)
	bytesPerPixel := int(bits / 8)
	for y := uint32(0); y < height; y++ {
		sourceY := y
		if int32(signedHeight) > 0 {
			sourceY = height - 1 - y
		}
		source := int(header) + int(sourceY)*int(stride)
		for x := uint32(0); x < width; x++ {
			from := source + int(x)*bytesPerPixel
			to := (int(y)*int(width) + int(x)) * 4
			rgba[to] = dib[from+2]
			rgba[to+1] = dib[from+1]
			rgba[to+2] = dib[from]
			rgba[to+3] = 255
			// BI_RGB 的第四字节未使用（常为 0），绝不当作 alpha
		}
	}
	return width, height, rgba, nil
}

// decodeFileBytes 用标准库解码 PNG/JPEG，输出自上而下 RGBA 像素。
func decodeFileBytes(payload []byte) (uint32, uint32, []byte, error) {
	img, _, err := image.Decode(bytes.NewReader(payload))
	if err != nil {
		return 0, 0, nil, fmt.Errorf("图片解码失败：%w", err)
	}
	bounds := img.Bounds()
	width, height := uint32(bounds.Dx()), uint32(bounds.Dy())
	if err := checkDimensions(width, height); err != nil {
		return 0, 0, nil, err
	}
	rgba := make([]byte, int(width)*int(height)*4)
	target := 0
	for y := 0; y < int(height); y++ {
		for x := 0; x < int(width); x++ {
			// NRGBAModel 输出非预乘 alpha（At().RGBA() 是预乘的，会二次压缩颜色）
			n := color.NRGBAModel.Convert(img.At(bounds.Min.X+x, bounds.Min.Y+y)).(color.NRGBA)
			rgba[target] = n.R
			rgba[target+1] = n.G
			rgba[target+2] = n.B
			rgba[target+3] = n.A
			target += 4
		}
	}
	return width, height, rgba, nil
}

// limitedWriter 在超出上限时报错（对应 Rust LimitedBytes）。
type limitedWriter struct {
	buf   bytes.Buffer
	limit int
}

func (w *limitedWriter) Write(data []byte) (int, error) {
	if w.buf.Len()+len(data) > w.limit {
		return 0, fmt.Errorf("图片编码结果超过大小限制")
	}
	return w.buf.Write(data)
}

func encodePNG(width, height uint32, rgba []byte, limit int) ([]byte, error) {
	img := image.NewNRGBA(image.Rect(0, 0, int(width), int(height)))
	copy(img.Pix, rgba)
	writer := &limitedWriter{limit: limit}
	if err := png.Encode(writer, img); err != nil {
		return nil, fmt.Errorf("PNG编码失败：%w", err)
	}
	return writer.buf.Bytes(), nil
}

// thumbnailRGBA 缩放到完全容纳于 320x240 的边界内（保持纵横比，最近邻采样）。
func thumbnailRGBA(width, height uint32, rgba []byte) (uint32, uint32, []byte) {
	scaleW := float64(thumbWidth) / float64(width)
	scaleH := float64(thumbHeight) / float64(height)
	scale := scaleW
	if scaleH < scale {
		scale = scaleH
	}
	if scale >= 1 {
		return width, height, rgba
	}
	newWidth := uint32(float64(width) * scale)
	newHeight := uint32(float64(height) * scale)
	if newWidth == 0 {
		newWidth = 1
	}
	if newHeight == 0 {
		newHeight = 1
	}
	out := make([]byte, int(newWidth)*int(newHeight)*4)
	for y := uint32(0); y < newHeight; y++ {
		sourceY := int(float64(y) / scale)
		if sourceY >= int(height) {
			sourceY = int(height) - 1
		}
		for x := uint32(0); x < newWidth; x++ {
			sourceX := int(float64(x) / scale)
			if sourceX >= int(width) {
				sourceX = int(width) - 1
			}
			copy(out[(int(y)*int(newWidth)+int(x))*4:], rgba[(sourceY*int(width)+sourceX)*4:])
		}
	}
	return newWidth, newHeight, out
}

func normalize(width, height uint32, rgba []byte) (*DecodedImage, error) {
	if err := checkDimensions(width, height); err != nil {
		return nil, err
	}
	tw, th, thumb := thumbnailRGBA(width, height, rgba)
	thumbnailPNG, err := encodePNG(tw, th, thumb, MaxThumbnailBytes)
	if err != nil {
		return nil, err
	}
	fullPNG, err := encodePNG(width, height, rgba, MaxPNGBytes)
	if err != nil {
		return nil, err
	}
	return &DecodedImage{PNG: fullPNG, ThumbnailPNG: thumbnailPNG, Width: width, Height: height}, nil
}

// DecodeImage 解码一段完整载荷；LZW/DIB 失败绝不裁剪补齐。
// 返回的图片字节全部由校验过的像素数据生成。
func DecodeImage(payload []byte) (*DecodedImage, error) {
	if len(payload) == 0 || len(payload) > MaxPayloadBytes {
		return nil, fmt.Errorf("图片传输载荷为空或超过16 MiB")
	}
	var width, height uint32
	var rgba []byte
	if bytes.HasPrefix(payload, []byte("LZW!")) {
		if len(payload) < 12 {
			return nil, fmt.Errorf("LZW图片头被截断")
		}
		expected, err := u32At(payload, 4)
		if err != nil {
			return nil, err
		}
		crc, err := u32At(payload, 8)
		if err != nil {
			return nil, err
		}
		dib, err := Decompress(payload[12:], int(expected))
		if err != nil {
			return nil, err
		}
		if CRC32(dib) != crc {
			return nil, fmt.Errorf("图片CRC32校验失败")
		}
		width, height, rgba, err = decodeDIB(dib)
		if err != nil {
			return nil, err
		}
	} else if bytes.HasPrefix(payload, []byte("\x89PNG\r\n\x1a\n")) ||
		bytes.HasPrefix(payload, []byte("\xff\xd8\xff")) {
		width, height, rgba, err := decodeFileBytes(payload)
		if err != nil {
			return nil, err
		}
		return normalize(width, height, rgba)
	} else {
		return nil, fmt.Errorf("不支持的图片载荷格式")
	}
	return normalize(width, height, rgba)
}

// ImportImage 导入本地选择的 PNG/JPEG/BMP 原图。
func ImportImage(bytesData []byte) (*DecodedImage, error) {
	if len(bytesData) == 0 || len(bytesData) > MaxImportBytes {
		return nil, fmt.Errorf("原图片为空或超过20 MiB")
	}
	switch {
	case bytes.HasPrefix(bytesData, []byte("\x89PNG\r\n\x1a\n")),
		bytes.HasPrefix(bytesData, []byte("\xff\xd8\xff")):
		width, height, rgba, err := decodeFileBytes(bytesData)
		if err != nil {
			return nil, err
		}
		return normalize(width, height, rgba)
	case bytes.HasPrefix(bytesData, []byte("BM")):
		width, height, rgba, err := decodeBMP(bytesData)
		if err != nil {
			return nil, err
		}
		return normalize(width, height, rgba)
	default:
		return nil, fmt.Errorf("仅支持PNG、JPEG和BMP图片")
	}
}

// decodeBMP 解析 24/32 位非压缩 BMP（文件头 + DIB + 像素，含 bfOffBits 跳位）。
func decodeBMP(data []byte) (uint32, uint32, []byte, error) {
	if len(data) < 14+40 {
		return 0, 0, nil, fmt.Errorf("BMP文件头被截断")
	}
	offset := binary.LittleEndian.Uint32(data[10:14])
	dib := data[14:]
	header, err := u32At(dib, 0)
	if err != nil {
		return 0, 0, nil, err
	}
	if header != 40 && header != 52 && header != 56 && header != 108 && header != 124 {
		return 0, 0, nil, fmt.Errorf("不支持的BMP头")
	}
	width, err := u32At(dib, 4)
	if err != nil {
		return 0, 0, nil, err
	}
	signedHeight, err := u32At(dib, 8)
	if err != nil {
		return 0, 0, nil, err
	}
	bits, err := u16At(dib, 14)
	if err != nil {
		return 0, 0, nil, err
	}
	compression, err := u32At(dib, 16)
	if err != nil {
		return 0, 0, nil, err
	}
	if int32(width) <= 0 || int32(signedHeight) == 0 || (bits != 24 && bits != 32) || compression != 0 {
		return 0, 0, nil, fmt.Errorf("仅支持单平面24/32位非压缩BMP")
	}
	height64 := uint64(int64(int32(signedHeight)))
	if height64 < 0 {
		height64 = -height64
	}
	height := uint32(height64)
	if err := checkDimensions(width, height); err != nil {
		return 0, 0, nil, err
	}
	stride := (uint64(width)*uint64(bits) + 31) / 32 * 4
	pixels := stride * height64
	start := int(offset)
	if start < int(header)+14 || start > len(data) {
		return 0, 0, nil, fmt.Errorf("BMP像素起点无效")
	}
	if uint64(len(data)-start) < pixels {
		return 0, 0, nil, fmt.Errorf("BMP像素数据不足")
	}
	rgba := make([]byte, uint64(width)*height64*4)
	bytesPerPixel := int(bits / 8)
	bottomUp := int32(signedHeight) > 0
	for y := uint32(0); y < height; y++ {
		sourceY := y
		if bottomUp {
			sourceY = height - 1 - y
		}
		source := start + int(sourceY)*int(stride)
		for x := uint32(0); x < width; x++ {
			from := source + int(x)*bytesPerPixel
			to := (int(y)*int(width) + int(x)) * 4
			rgba[to] = data[from+2]
			rgba[to+1] = data[from+1]
			rgba[to+2] = data[from]
			rgba[to+3] = 255
		}
	}
	return width, height, rgba, nil
}

// EncodePNGForWire 从已登记的 PNG 资产构造飞秋 LZW! 载荷：
// 透明像素合成白底，输出 40 字节头 + 自底向上 24 位 DIB，编码后用解码器自检。
func EncodePNGForWire(pngBytes []byte) ([]byte, error) {
	if len(pngBytes) > MaxPNGBytes || !bytes.HasPrefix(pngBytes, []byte("\x89PNG\r\n\x1a\n")) {
		return nil, fmt.Errorf("发送资产必须是受限PNG")
	}
	width, height, rgba, err := decodeFileBytes(pngBytes)
	if err != nil {
		return nil, err
	}
	if err := checkDimensions(width, height); err != nil {
		return nil, err
	}
	stride := (uint64(width)*3 + 3) & ^uint64(3)
	length := 40 + stride*uint64(height)
	if length > MaxDIBBytes {
		return nil, fmt.Errorf("发送DIB超过上限")
	}
	dib := make([]byte, length)
	binary.LittleEndian.PutUint32(dib[0:4], 40)
	binary.LittleEndian.PutUint32(dib[4:8], width)
	binary.LittleEndian.PutUint32(dib[8:12], height)
	binary.LittleEndian.PutUint16(dib[12:14], 1)
	dib[14] = 24
	binary.LittleEndian.PutUint32(dib[20:24], uint32(stride*uint64(height)))
	for y := uint32(0); y < height; y++ {
		for x := uint32(0); x < width; x++ {
			at := (int(y)*int(width) + int(x)) * 4
			alpha := uint32(rgba[at+3])
			at2 := 40 + int(height-1-y)*int(stride) + int(x)*3
			for i, channel := range []uint32{uint32(rgba[at+2]), uint32(rgba[at+1]), uint32(rgba[at])} {
				// 白底合成：(c*alpha + 255*(255-alpha) + 127) / 255
				dib[at2+i] = byte((channel*alpha + 255*(255-alpha) + 127) / 255)
			}
		}
	}
	compressed, err := Compress(dib)
	if err != nil {
		return nil, err
	}
	roundTrip, err := Decompress(compressed, len(dib))
	if err != nil || !bytes.Equal(roundTrip, dib) {
		return nil, fmt.Errorf("发送图片LZW校验失败")
	}
	payload := []byte("LZW!")
	var trailer [8]byte
	binary.LittleEndian.PutUint32(trailer[0:4], uint32(len(dib)))
	binary.LittleEndian.PutUint32(trailer[4:8], CRC32(dib))
	payload = append(payload, trailer[:]...)
	payload = append(payload, compressed...)
	return payload, nil
}
