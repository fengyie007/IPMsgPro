package media

// DIB/容器测试对拍 tauri-rust dib.rs 的同名用例。

import (
	"bytes"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"testing"
)

func jpegEncode(buf *bytes.Buffer, img image.Image, quality int) error {
	return jpeg.Encode(buf, img, &jpeg.Options{Quality: quality})
}

// pack 在 lzw_test.go 中定义（同包复用）。

func makeDIB(width int32, height int32, bits uint16, pixels []byte) []byte {
	dib := make([]byte, 40)
	putLE := func(at int, v uint32) {
		dib[at] = byte(v)
		dib[at+1] = byte(v >> 8)
		dib[at+2] = byte(v >> 16)
		dib[at+3] = byte(v >> 24)
	}
	putLE(0, 40)
	putLE(4, uint32(width))
	putLE(8, uint32(height))
	dib[12] = 1
	dib[13] = 0
	dib[14] = byte(bits)
	dib[15] = 0
	putLE(20, uint32(len(pixels)))
	return append(dib, pixels...)
}

func wrap(t *testing.T, dib []byte) []byte {
	t.Helper()
	codes := make([]int, len(dib))
	for i, v := range dib {
		codes[i] = int(v)
	}
	payload := []byte("LZW!")
	var trailer [8]byte
	trailer[0] = byte(len(dib))
	trailer[1] = byte(len(dib) >> 8)
	trailer[2] = byte(len(dib) >> 16)
	trailer[3] = byte(len(dib) >> 24)
	crc := CRC32(dib)
	trailer[4] = byte(crc)
	trailer[5] = byte(crc >> 8)
	trailer[6] = byte(crc >> 16)
	trailer[7] = byte(crc >> 24)
	return append(append(payload, trailer[:]...), pack(t, codes)...)
}

func decodePNGPixels(t *testing.T, data []byte) *image.NRGBA {
	t.Helper()
	img, err := png.Decode(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	out := image.NewNRGBA(image.Rect(0, 0, img.Bounds().Dx(), img.Bounds().Dy()))
	for y := 0; y < out.Bounds().Dy(); y++ {
		for x := 0; x < out.Bounds().Dx(); x++ {
			out.SetNRGBA(x, y, color.NRGBAModel.Convert(img.At(x, y)).(color.NRGBA))
		}
	}
	return out
}

func TestOutgoingPNGIsWhiteCompositedAndBottomUp(t *testing.T) {
	img := image.NewNRGBA(image.Rect(0, 0, 1, 2))
	img.SetNRGBA(0, 0, color.NRGBA{255, 0, 0, 0})
	img.SetNRGBA(0, 1, color.NRGBA{0, 0, 255, 128})
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	wire, err := EncodePNGForWire(buf.Bytes())
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.HasPrefix(wire, []byte("LZW!")) {
		t.Fatal("发送载荷应以 LZW! 开头")
	}
	result, err := DecodeImage(wire)
	if err != nil {
		t.Fatal(err)
	}
	pixels := decodePNGPixels(t, result.PNG)
	if got := []byte{pixels.Pix[0], pixels.Pix[1], pixels.Pix[2], pixels.Pix[4], pixels.Pix[5], pixels.Pix[6]}; !bytes.Equal(got, []byte{255, 255, 255, 127, 127, 255}) {
		t.Fatalf("白底合成像素异常: %v", got)
	}
}

func TestImportsBMPAndRejectsNonImages(t *testing.T) {
	// 手工构造 2x1 24bpp BMP：像素 (1,2,3) (4,5,6)，自底向上，行填充只在行尾
	pixels := []byte{3, 2, 1, 6, 5, 4, 0, 0}
	bmp := makeBMP(t, 2, 1, 24, pixels)
	imported, err := ImportImage(bmp)
	if err != nil {
		t.Fatal(err)
	}
	px := decodePNGPixels(t, imported.PNG)
	if got := []byte{px.Pix[0], px.Pix[1], px.Pix[2], px.Pix[4], px.Pix[5], px.Pix[6]}; !bytes.Equal(got, []byte{1, 2, 3, 4, 5, 6}) {
		t.Fatalf("BMP 导入像素异常: %v", got)
	}
	if _, err := ImportImage([]byte("GIF89a")); err == nil {
		t.Error("GIF 应被拒绝")
	}
	if _, err := ImportImage([]byte("not an image")); err == nil {
		t.Error("垃圾输入应被拒绝")
	}
	if _, err := EncodePNGForWire(bmp); err == nil {
		t.Error("发送资产必须是 PNG")
	}
}

func makeBMP(t *testing.T, width int32, height int32, bits uint16, pixels []byte) []byte {
	t.Helper()
	dib := makeDIB(width, height, bits, pixels)
	out := []byte{'B', 'M'}
	total := uint32(14 + len(dib))
	out = append(out, byte(total), byte(total>>8), byte(total>>16), byte(total>>24), 0, 0, 0, 0)
	offset := uint32(54)
	out = append(out, byte(offset), byte(offset>>8), byte(offset>>16), byte(offset>>24))
	return append(out, dib...)
}

func TestBottomUpDIBAndRowPaddingAreCorrect(t *testing.T) {
	dib := makeDIB(1, 2, 24, []byte{0, 0, 255, 77, 0, 255, 0, 99})
	decoded, err := DecodeImage(wrap(t, dib))
	if err != nil {
		t.Fatal(err)
	}
	if decoded.Width != 1 || decoded.Height != 2 {
		t.Fatalf("尺寸异常: %dx%d", decoded.Width, decoded.Height)
	}
	px := decodePNGPixels(t, decoded.PNG)
	if got := []byte{px.Pix[0], px.Pix[1], px.Pix[2], px.Pix[4], px.Pix[5], px.Pix[6]}; !bytes.Equal(got, []byte{0, 255, 0, 255, 0, 0}) {
		t.Fatalf("自底向上行序异常: %v", got)
	}
}

func TestTopDownAnd32BitUnusedAlphaAreCorrect(t *testing.T) {
	dib := makeDIB(1, -2, 32, []byte{0, 0, 255, 0, 255, 0, 0, 0})
	decoded, err := DecodeImage(wrap(t, dib))
	if err != nil {
		t.Fatal(err)
	}
	px := decodePNGPixels(t, decoded.PNG)
	if got := px.Pix[:8]; !bytes.Equal(got, []byte{255, 0, 0, 255, 0, 0, 255, 255}) {
		t.Fatalf("自顶向下/alpha 应忽略: %v", got)
	}
}

func TestCorruptedContainerAndDIBAreRejected(t *testing.T) {
	valid := makeDIB(1, 1, 24, []byte{0, 0, 255, 0})
	badCRC := wrap(t, valid)
	badCRC[8] ^= 1
	if _, err := DecodeImage(badCRC); err == nil {
		t.Fatal("坏 CRC 应被拒绝")
	}
	for end := 0; end < 12; end++ {
		if _, err := DecodeImage(badCRC[:end]); err == nil {
			t.Fatalf("截断头 %d 应被拒绝", end)
		}
	}
	short := append([]byte(nil), wrap(t, valid)...)
	short = short[:len(short)-1]
	if _, err := DecodeImage(short); err == nil {
		t.Fatal("截断码流应被拒绝")
	}
	type field struct {
		at    int
		value uint32
	}
	for _, f := range []field{
		{0, 256}, {4, 0}, {4, 0x7fffffff}, {8, 0}, {8, 0x80000000},
		{16, 3}, {20, 100}, {32, 1},
	} {
		dib := append([]byte(nil), valid...)
		dib[f.at] = byte(f.value)
		dib[f.at+1] = byte(f.value >> 8)
		dib[f.at+2] = byte(f.value >> 16)
		dib[f.at+3] = byte(f.value >> 24)
		if _, _, _, err := decodeDIB(dib); err == nil {
			t.Errorf("偏移 %d 的非法字段应被拒绝", f.at)
		}
	}
	extra := append(append([]byte(nil), valid...), 0)
	if _, _, _, err := decodeDIB(extra); err == nil {
		t.Error("多余字节应被拒绝")
	}
	if _, _, _, err := decodeDIB(valid[:len(valid)-1]); err == nil {
		t.Error("缺字节应被拒绝")
	}
	palette := append([]byte(nil), valid...)
	palette[14] = 8
	if _, _, _, err := decodeDIB(palette); err == nil {
		t.Error("颜色表应被拒绝")
	}
	noPlanes := append([]byte(nil), valid...)
	noPlanes[12] = 0
	if _, _, _, err := decodeDIB(noPlanes); err == nil {
		t.Error("非单平面应被拒绝")
	}
}

func TestZeroSizeImageIsValidForUncompressedDIB(t *testing.T) {
	dib := makeDIB(1, 1, 24, []byte{0, 0, 255, 0})
	dib[20] = 0
	dib[21] = 0
	dib[22] = 0
	dib[23] = 0
	if _, err := DecodeImage(wrap(t, dib)); err != nil {
		t.Fatal(err)
	}
}

func TestRawPNGPreservesPixelsAndThumbnailIsBounded(t *testing.T) {
	src := image.NewNRGBA(image.Rect(0, 0, 640, 480))
	for y := 0; y < 480; y++ {
		for x := 0; x < 640; x++ {
			src.SetNRGBA(x, y, color.NRGBA{byte(x), byte(y), 64, 128})
		}
	}
	var raw bytes.Buffer
	if err := png.Encode(&raw, src); err != nil {
		t.Fatal(err)
	}
	decoded, err := DecodeImage(raw.Bytes())
	if err != nil {
		t.Fatal(err)
	}
	px := decodePNGPixels(t, decoded.PNG)
	for i := range px.Pix {
		if px.Pix[i] != src.Pix[i] {
			t.Fatalf("像素 %d 不一致: %d != %d", i, px.Pix[i], src.Pix[i])
		}
	}
	thumb := decodePNGPixels(t, decoded.ThumbnailPNG)
	if thumb.Bounds().Dx() > thumbWidth || thumb.Bounds().Dy() > thumbHeight {
		t.Fatalf("缩略图超界: %v", thumb.Bounds())
	}
}

func TestRawJPEGIsDecodedButUnknownFormatsAreNot(t *testing.T) {
	src := image.NewNRGBA(image.Rect(0, 0, 4, 3))
	for y := 0; y < 3; y++ {
		for x := 0; x < 4; x++ {
			src.SetNRGBA(x, y, color.NRGBA{10, 20, 30, 255})
		}
	}
	var buf bytes.Buffer
	if err := jpegEncode(&buf, src, 95); err != nil {
		t.Fatal(err)
	}
	decoded, err := DecodeImage(buf.Bytes())
	if err != nil {
		t.Fatal(err)
	}
	if decoded.Width != 4 || decoded.Height != 3 {
		t.Fatalf("JPEG 尺寸异常: %dx%d", decoded.Width, decoded.Height)
	}
	if _, err := DecodeImage([]byte("GIF89a")); err == nil {
		t.Error("GIF 应被拒绝")
	}
	if _, err := DecodeImage([]byte("BMfake")); err == nil {
		t.Error("伪 BMP 应被拒绝")
	}
}

func TestPNGOutputWriterEnforcesItsCap(t *testing.T) {
	writer := &limitedWriter{limit: 2}
	if _, err := writer.Write([]byte{1, 2}); err != nil {
		t.Fatal(err)
	}
	if _, err := writer.Write([]byte{3}); err == nil {
		t.Fatal("超限应报错")
	}
	if !bytes.Equal(writer.buf.Bytes(), []byte{1, 2}) {
		t.Fatal("超限前的字节应保留")
	}
}
