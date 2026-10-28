// Package media 承载图片链路的编解码，逐行对照 tauri-rust/crates/ipmsg-core/src/image/。
// lzw.go 对应 lzw.rs：飞秋 LZW 变体——LSB-first 码位打包进 MSB-first 字节，
// 槽位 256..4095 饱和后循环复用，码宽封顶 12 位。
package media

import (
	"errors"
	"fmt"
)

const (
	MaxDIBBytes     = 64 * 1024 * 1024
	MaxPayloadBytes = 16 * 1024 * 1024
)

var crcTable = func() [256]uint32 {
	var table [256]uint32
	for index := 0; index < 256; index++ {
		value := uint32(index)
		for bit := 0; bit < 8; bit++ {
			if value&1 != 0 {
				value = 0xedb88320 ^ (value >> 1)
			} else {
				value >>= 1
			}
		}
		table[index] = value
	}
	return table
}()

func CRC32(b []byte) uint32 {
	crc := ^uint32(0)
	for _, byteValue := range b {
		crc = crcTable[(crc^uint32(byteValue))&0xff] ^ (crc >> 8)
	}
	return ^crc
}

// codes 逐位读取码流：第 bit 位取自字节的高位端。
type codes struct {
	input []byte
	bit   int
}

func (c *codes) next(width uint32) (int, bool) {
	if len(c.input)*8-c.bit < int(width) {
		return 0, false
	}
	var code int
	for position := uint32(0); position < width; position++ {
		value := (c.input[c.bit/8] >> (7 - c.bit%8)) & 1
		code |= int(value) << position
		c.bit++
	}
	return code, true
}

// Decompress 解码不含 12 字节 `LZW!` 容器头的原始码流；
// 输出长度必须与声明完全一致，不接受部分输出。
func Decompress(input []byte, expectedLen int) ([]byte, error) {
	if len(input) == 0 || len(input) > MaxPayloadBytes {
		return nil, errors.New("LZW压缩流为空或超过大小限制")
	}
	if expectedLen == 0 || expectedLen > MaxDIBBytes {
		return nil, errors.New("LZW声明的DIB长度无效或超过64 MiB")
	}
	c := &codes{input: input}
	first, ok := c.next(9)
	if !ok {
		return nil, errors.New("LZW首码被截断")
	}
	if first > 255 {
		return nil, errors.New("LZW首码不是字面值")
	}
	dictionary := make([][]byte, 4096)
	for value := 0; value < 256; value++ {
		dictionary[value] = []byte{byte(value)}
	}
	previous := []byte{byte(first)}
	output := make([]byte, 0, expectedLen)
	output = append(output, byte(first))
	next := 256
	width := uint32(9)
	// 首码已消费；此计数器不随槽位回绕
	counter := 257
	for {
		code, ok := c.next(width)
		if !ok {
			break
		}
		var entry []byte
		if code == next {
			// 回绕后的槽位仍含旧数据；KwKwK 必须优先
			entry = make([]byte, 0, len(previous)+1)
			entry = append(entry, previous...)
			entry = append(entry, previous[0])
		} else {
			if code >= len(dictionary) || dictionary[code] == nil {
				return nil, errors.New("LZW引用未定义词条")
			}
			entry = append([]byte(nil), dictionary[code]...)
		}
		if len(entry) > expectedLen-len(output) {
			return nil, errors.New("LZW解码结果超过声明长度")
		}
		output = append(output, entry...)
		word := append(append([]byte(nil), previous...), entry[0])
		if next == len(dictionary) {
			dictionary = append(dictionary, word)
		} else {
			dictionary[next] = word
		}
		next++
		if next == 4096 {
			next = 256
		}
		previous = entry
		if width < 12 {
			counter++
			if counter == 1<<width {
				width++
			}
		}
	}
	if len(output) != expectedLen {
		return nil, errors.New("LZW解码结果短于声明长度")
	}
	return output, nil
}

// Compress 编码出与 Decompress 同一循环字典变体的码流。
func Compress(input []byte) ([]byte, error) {
	if len(input) == 0 || len(input) > MaxDIBBytes {
		return nil, errors.New("DIB为空或超过64 MiB")
	}
	slots := make([][]byte, 4096)
	dictionary := make(map[string]int, 4096)
	for code := 0; code < 256; code++ {
		slots[code] = []byte{byte(code)}
		dictionary[string(slots[code])] = code
	}
	next, width, counter := 256, uint32(9), 256
	var byteValue byte
	var used uint32
	output := make([]byte, 0, len(input)/2+16)
	emit := func(code int) error {
		for bit := uint32(0); bit < width; bit++ {
			byteValue = (byteValue << 1) | byte((code>>bit)&1)
			used++
			if used == 8 {
				output = append(output, byteValue)
				byteValue = 0
				used = 0
			}
		}
		if len(output) > MaxPayloadBytes-12 {
			return errors.New("压缩后的图片超过16 MiB")
		}
		if width < 12 {
			counter++
			if counter == 1<<width {
				width++
			}
		}
		return nil
	}
	prefix := []byte{input[0]}
	prefixCode := int(input[0])
	for _, value := range input[1:] {
		prefix = append(prefix, value)
		if code, ok := dictionary[string(prefix)]; ok {
			prefixCode = code
			continue
		}
		if err := emit(prefixCode); err != nil {
			return nil, err
		}
		// 槽位循环：若旧词条仍映射到 next，先从字典移除
		if old, ok := dictionary[string(slots[next])]; ok && old == next {
			delete(dictionary, string(slots[next]))
		}
		slots[next] = append([]byte(nil), prefix...)
		dictionary[string(prefix)] = next
		next++
		if next == 4096 {
			next = 256
		}
		prefix = prefix[:1]
		prefix[0] = value
		prefixCode = int(value)
	}
	if err := emit(prefixCode); err != nil {
		return nil, err
	}
	if used > 0 {
		output = append(output, byteValue<<(8-used))
	}
	if len(output) > MaxPayloadBytes-12 {
		return nil, fmt.Errorf("压缩后的图片超过16 MiB")
	}
	return output, nil
}
