package media

// LZW 测试对拍 tauri-rust lzw.rs 的同名用例。

import (
	"bytes"
	"fmt"
	"testing"
)

// pack 独立夹具打包器：按显式序数边界确定码宽，不依赖解码器的计数器。
func pack(t *testing.T, codes []int) []byte {
	t.Helper()
	var result []byte
	var byteValue byte
	used := 0
	for index, code := range codes {
		var width uint32
		switch {
		case index <= 255:
			width = 9
		case index <= 767:
			width = 10
		case index <= 1791:
			width = 11
		default:
			width = 12
		}
		if code >= 1<<width {
			t.Fatalf("码 %d 超出宽度 %d", code, width)
		}
		for bit := uint32(0); bit < width; bit++ {
			byteValue = (byteValue << 1) | byte((code>>bit)&1)
			used++
			if used == 8 {
				result = append(result, byteValue)
				byteValue = 0
				used = 0
			}
		}
	}
	if used != 0 {
		result = append(result, byteValue<<(8-used))
	}
	return result
}

func seeded(size int) []byte {
	var seed uint32 = 123456789
	data := make([]byte, size)
	for i := range data {
		seed = seed*1664525 + 1013904223
		data[i] = byte(seed >> 24)
	}
	return data
}

func TestCompressorMatchesKnownCodesAndCyclesAgainstValidatedDecoder(t *testing.T) {
	if got, err := Compress([]byte("ABC")); err != nil || !bytes.Equal(got, pack(t, []int{65, 66, 67})) {
		t.Fatalf("ABC 编码异常: %v %v", got, err)
	}
	if got, err := Compress([]byte("AAAAAA")); err != nil || !bytes.Equal(got, pack(t, []int{65, 256, 257})) {
		t.Fatalf("AAAAAA 编码异常: %v %v", got, err)
	}
	for _, size := range []int{1, 255, 256, 1024, 4096, 100000} {
		data := seeded(size)
		encoded, err := Compress(data)
		if err != nil {
			t.Fatal(err)
		}
		decoded, err := Decompress(encoded, len(data))
		if err != nil || !bytes.Equal(decoded, data) {
			t.Fatalf("size=%d 往返失败: %v", size, err)
		}
	}
	for _, data := range [][]byte{bytes.Repeat([]byte{0}, 100000), bytes.Repeat([]byte("ABABABA"), 20000)} {
		encoded, err := Compress(data)
		if err != nil {
			t.Fatal(err)
		}
		decoded, err := Decompress(encoded, len(data))
		if err != nil || !bytes.Equal(decoded, data) {
			t.Fatalf("特殊数据往返失败: %v", err)
		}
	}
	if _, err := Compress(nil); err == nil {
		t.Fatal("空输入应报错")
	}
}

func TestStandardCRCVectors(t *testing.T) {
	if CRC32(nil) != 0 {
		t.Fatal("crc32(\"\") 应为 0")
	}
	if got := CRC32([]byte("123456789")); got != 0xcbf43926 {
		t.Fatalf("标准 CRC 向量不匹配: %x", got)
	}
}

func TestLiteralCodesCrossEveryWidthBoundary(t *testing.T) {
	for _, length := range []int{1, 255, 256, 257, 767, 768, 769, 1791, 1792, 1793, 10000} {
		expected := make([]byte, length)
		codes := make([]int, length)
		for i := range expected {
			expected[i] = byte(i % 251)
			codes[i] = int(expected[i])
		}
		decoded, err := Decompress(pack(t, codes), length)
		if err != nil || !bytes.Equal(decoded, expected) {
			t.Fatalf("len=%d 解码失败: %v", length, err)
		}
	}
}

func TestCyclicDictionaryReplacesOldEntries(t *testing.T) {
	var codes []int
	var expected []byte
	for i := 0; i < 4097; i++ {
		value := i % 251
		codes = append(codes, value)
		expected = append(expected, byte(value))
	}
	codes = append(codes, 256)
	expected = append(expected, byte(3840%251), byte(3841%251))
	decoded, err := Decompress(pack(t, codes), len(expected))
	if err != nil || !bytes.Equal(decoded, expected) {
		t.Fatalf("循环字典回绕失败: %v", err)
	}
}

func TestKwKwKTakesPrecedenceAfterWrap(t *testing.T) {
	var codes []int
	var expected []byte
	for i := 0; i < 3841; i++ {
		value := i % 251
		codes = append(codes, value)
		expected = append(expected, byte(value))
	}
	codes = append(codes, 256)
	expected = append(expected, byte(3840%251), byte(3840%251))
	decoded, err := Decompress(pack(t, codes), len(expected))
	if err != nil || !bytes.Equal(decoded, expected) {
		t.Fatalf("回绕后 KwKwK 失败: %v", err)
	}
	decoded, err = Decompress(pack(t, []int{65, 66, 256, 258}), 7)
	if err != nil || string(decoded) != "ABABABA" {
		t.Fatalf("KwKwK 基础用例失败: %q %v", decoded, err)
	}
}

func TestMalformedOrWrongSizedStreamsAreRejected(t *testing.T) {
	p65 := pack(t, []int{65})
	p66 := pack(t, []int{65, 66})
	p400 := pack(t, []int{65, 400})
	cases := []struct {
		input []byte
		want  int
	}{
		{nil, 40},
		{[]byte{0}, 40},
		{pack(t, []int{300}), 1},
		{p400, 2},
		{p65, 0},
		{p65, MaxDIBBytes + 1},
		{p65, 2},
		{p66, 1},
	}
	for i, c := range cases {
		if _, err := Decompress(c.input, c.want); err == nil {
			t.Errorf("case %d 应报错", i)
		}
	}
	// 编码侧大小上限
	huge := make([]byte, MaxDIBBytes+1)
	if _, err := Compress(huge); err == nil {
		t.Error("超过 64 MiB 的输入应报错")
	}
	_ = fmt.Sprint()
}
