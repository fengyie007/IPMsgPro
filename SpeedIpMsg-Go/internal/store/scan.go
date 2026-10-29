// Package store 承载 MyGo 版的配置持久化与 SQLite 消息存储。
// 语义逐项对照 tauri-rust/crates/ipmsg-core/src/{config,scan,database}.rs。
package store

import (
	"encoding/json"
	"fmt"
	"net"
	"strconv"
	"strings"
)

// ScanRange 校验规则对照 scan.rs：完整/简写/CIDR/单 IPv4，合并重叠相邻，总量上限 65536。
const (
	MaxScanRanges    = 32
	MaxScanAddresses = 65536
)

type ScanOptions struct {
	Ranges  []string
	Port    uint16
	DelayMs uint32
}

type ScanPlan struct {
	Ranges  [][2]uint32
	Total   uint32
	Port    uint16
	DelayMs uint32
}

func parseIPv4(text string) (uint32, error) {
	ip := net.ParseIP(strings.TrimSpace(text))
	if ip == nil {
		return 0, fmt.Errorf("无效IPv4地址：%s", text)
	}
	ip4 := ip.To4()
	if ip4 == nil {
		return 0, fmt.Errorf("无效IPv4地址：%s", text)
	}
	return uint32(ip4[0])<<24 | uint32(ip4[1])<<16 | uint32(ip4[2])<<8 | uint32(ip4[3]), nil
}

// ParseRange 解析单个扫描范围，返回 [start, end]（含端点）。
// 不假设 /24：.0 或 .255 可能是用户子网中的真实主机。
func ParseRange(text string) ([2]uint32, error) {
	text = strings.TrimSpace(text)
	var zero [2]uint32
	if text == "" || len(text) > 64 {
		return zero, fmt.Errorf("扫描范围为空或过长")
	}
	var start, end uint32
	if ipText, prefix, ok := strings.Cut(text, "/"); ok {
		ip, err := parseIPv4(ipText)
		if err != nil {
			return zero, err
		}
		prefix = strings.TrimSpace(prefix)
		if prefix == "" || len(prefix) > 2 || !allDigits(prefix) {
			return zero, fmt.Errorf("CIDR前缀必须为0～32")
		}
		p, err := strconv.ParseUint(prefix, 10, 32)
		if err != nil || p > 32 {
			return zero, fmt.Errorf("CIDR前缀必须为0～32")
		}
		var mask uint32
		if p > 0 {
			mask = ^uint32(0) << (32 - p)
		}
		network := ip & mask
		broadcast := network | ^mask
		if p <= 30 {
			start, end = network+1, broadcast-1
		} else {
			start, end = network, broadcast
		}
	} else if startText, endText, ok := strings.Cut(text, "-"); ok {
		rangeStart, err := parseIPv4(startText)
		if err != nil {
			return zero, err
		}
		endText = strings.TrimSpace(endText)
		if endText != "" && len(endText) <= 3 && allDigits(endText) {
			last, err := strconv.ParseUint(endText, 10, 8)
			if err != nil {
				return zero, fmt.Errorf("结束段必须为0～255")
			}
			end = (rangeStart & 0xffffff00) | uint32(last)
		} else {
			rangeEnd, err := parseIPv4(endText)
			if err != nil {
				return zero, err
			}
			end = rangeEnd
		}
		start = rangeStart
	} else {
		ip, err := parseIPv4(text)
		if err != nil {
			return zero, err
		}
		start, end = ip, ip
	}
	// 单播正序，不含 0 网段、组播及保留段
	if start < 0x01000000 || end >= 0xe0000000 || end < start {
		return zero, fmt.Errorf("范围必须是正序单播IPv4地址，不含0网段、组播或保留地址")
	}
	if uint64(end)-uint64(start)+1 > MaxScanAddresses {
		return zero, fmt.Errorf("每次扫描最多65536个地址")
	}
	return [2]uint32{start, end}, nil
}

func allDigits(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return len(s) > 0
}

// Plan 校验扫描选项并合并重叠/相邻范围。
func (o ScanOptions) Plan() (ScanPlan, error) {
	if o.Port == 0 {
		return ScanPlan{}, fmt.Errorf("目标端口必须为1～65535")
	}
	if o.DelayMs < 10 || o.DelayMs > 1000 {
		return ScanPlan{}, fmt.Errorf("扫描间隔必须为10～1000毫秒")
	}
	if len(o.Ranges) > MaxScanRanges {
		return ScanPlan{}, fmt.Errorf("扫描范围最多32项")
	}
	ranges := make([][2]uint32, 0, len(o.Ranges))
	for _, r := range o.Ranges {
		parsed, err := ParseRange(r)
		if err != nil {
			return ScanPlan{}, err
		}
		ranges = append(ranges, parsed)
	}
	sortRanges(ranges)
	var merged [][2]uint32
	for _, r := range ranges {
		if n := len(merged); n > 0 && r[0] <= merged[n-1][1]+1 {
			if r[1] > merged[n-1][1] {
				merged[n-1][1] = r[1]
			}
		} else {
			merged = append(merged, r)
		}
	}
	var total uint64
	for _, r := range merged {
		total += uint64(r[1]) - uint64(r[0]) + 1
	}
	if total > MaxScanAddresses {
		return ScanPlan{}, fmt.Errorf("合并后的扫描范围最多65536个地址")
	}
	return ScanPlan{Ranges: merged, Total: uint32(total), Port: o.Port, DelayMs: o.DelayMs}, nil
}

func sortRanges(ranges [][2]uint32) {
	for i := 1; i < len(ranges); i++ {
		for j := i; j > 0 && ranges[j][0] < ranges[j-1][0]; j-- {
			ranges[j], ranges[j-1] = ranges[j-1], ranges[j]
		}
	}
}

// ParseDirectAddress 校验直接用户地址 IPv4:port（对照 config.rs parse_address）。
func ParseDirectAddress(text string) error {
	host, port, err := net.SplitHostPort(text)
	if err != nil {
		return fmt.Errorf("无效IPv4:port：%s", text)
	}
	ip := net.ParseIP(host)
	if ip == nil || ip.To4() == nil {
		return fmt.Errorf("无效IPv4:port：%s", text)
	}
	portNum, err := strconv.ParseUint(port, 10, 16)
	if err != nil {
		return fmt.Errorf("无效IPv4:port：%s", text)
	}
	ip4 := ip.To4()
	if portNum == 0 || ip4.IsUnspecified() || ip4.IsMulticast() || ip4.Equal(net.IPv4bcast) {
		return fmt.Errorf("无效的对端地址：%s", text)
	}
	return nil
}

// jsonFieldNull 检查合并后的配置对象里是否有显式 null 字段。
// Rust serde 对非 Option 字段的 null 会报错，encoding/json 默认静默忽略，这里显式补齐。
func rejectNullFields(data []byte) error {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(data, &object); err != nil {
		return err
	}
	var keys []string
	for key, raw := range object {
		if strings.TrimSpace(string(raw)) == "null" {
			keys = append(keys, key)
		}
	}
	if len(keys) > 0 {
		sortStrings(keys)
		return fmt.Errorf("设置值不能为 null：%s", strings.Join(keys, ", "))
	}
	return nil
}

func sortStrings(s []string) {
	for i := 1; i < len(s); i++ {
		for j := i; j > 0 && s[j] < s[j-1]; j-- {
			s[j], s[j-1] = s[j-1], s[j]
		}
	}
}
