package store

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"golang.org/x/text/encoding/simplifiedchinese"
)

// AppConfig 与 Rust 版 AppConfig 字段一一对应（camelCase JSON，未知字段拒绝）。
type AppConfig struct {
	Nickname            string   `json:"nickname"`
	AvatarID            int      `json:"avatarId"`
	Group               string   `json:"group"`
	DirectUsers         []string `json:"directUsers"`
	MinimizeBehavior    string   `json:"minimizeBehavior"`
	NotificationSound   bool     `json:"notificationSound"`
	SystemNotifications bool     `json:"systemNotifications"`
	NotificationPreview bool     `json:"notificationPreview"`
	AnnounceOnStartup   bool     `json:"announceOnStartup"`
	AnnounceOnExit      bool     `json:"announceOnExit"`
	KnockEffects        bool     `json:"knockEffects"`
	Segments            []string `json:"segments"`
	IPScanRanges        []string `json:"ipScanRanges"`
	ScanPort            uint16   `json:"scanPort"`
	ScanDelayMs         uint32   `json:"scanDelayMs"`
	ScanOnStartup       bool     `json:"scanOnStartup"`
	DataDir             string   `json:"dataDir"`
}

func DefaultConfig() AppConfig {
	return AppConfig{
		MinimizeBehavior:  "tray",
		AnnounceOnStartup: true,
		AnnounceOnExit:    true,
		KnockEffects:      true,
		DirectUsers:       []string{},
		Segments:          []string{},
		IPScanRanges:      []string{},
		ScanPort:          2425,
		ScanDelayMs:       20,
		ScanOnStartup:     true,
	}
}

// Validate 对照 config.rs validate。
func (c AppConfig) Validate() error {
	if c.AvatarID < 0 || c.AvatarID > 83 {
		return fmt.Errorf("无效头像编号")
	}
	for _, text := range []string{c.Nickname, c.Group} {
		if len(text) > 256 || strings.ContainsAny(text, "\x00\r\n") {
			return fmt.Errorf("昵称/组名过长或包含控制字符")
		}
	}
	// 昵称/组名必须能按 GBK 上线（与 Rust 版 encode_packet 校验一致）
	for _, text := range []string{c.Nickname, c.Group} {
		if _, err := simplifiedchinese.GBK.NewEncoder().Bytes([]byte(text)); err != nil {
			return fmt.Errorf("昵称/组名包含 GBK 无法表示的字符")
		}
	}
	if c.MinimizeBehavior != "tray" && c.MinimizeBehavior != "taskbar" {
		return fmt.Errorf("无效的关闭行为")
	}
	if len(c.Segments) != 0 || c.DataDir != "" {
		return fmt.Errorf("当前版本不支持自定义广播网段或修改数据目录")
	}
	if len(c.DirectUsers) > 256 {
		return fmt.Errorf("直接用户最多256项")
	}
	for _, address := range c.DirectUsers {
		if err := ParseDirectAddress(address); err != nil {
			return err
		}
	}
	_, err := ScanOptions{Ranges: c.IPScanRanges, Port: c.ScanPort, DelayMs: c.ScanDelayMs}.Plan()
	return err
}

// merged 应用 patch 并校验；任何错误都不改变当前配置（对照 config.rs merged）。
func (c AppConfig) merged(patch json.RawMessage) (AppConfig, error) {
	var patchObject map[string]json.RawMessage
	if err := json.Unmarshal(patch, &patchObject); err != nil {
		return AppConfig{}, fmt.Errorf("设置必须是对象")
	}
	current, err := json.Marshal(c)
	if err != nil {
		return AppConfig{}, err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(current, &fields); err != nil {
		return AppConfig{}, err
	}
	for key, value := range patchObject {
		if _, ok := fields[key]; !ok {
			return AppConfig{}, fmt.Errorf("未知设置：%s", key)
		}
		fields[key] = value
	}
	mergedBytes, err := json.Marshal(fields)
	if err != nil {
		return AppConfig{}, err
	}
	if err := rejectNullFields(mergedBytes); err != nil {
		return AppConfig{}, err
	}
	// 先以默认值打底，缺失字段保持默认（serde(default) 语义）；未知字段由严格模式拒绝
	next := DefaultConfig()
	decoder := json.NewDecoder(bytes.NewReader(mergedBytes))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&next); err != nil {
		return AppConfig{}, fmt.Errorf("无效设置：%w", err)
	}
	if err := next.Validate(); err != nil {
		return AppConfig{}, err
	}
	return next, nil
}

// ConfigStore：JSON 单一来源；打开时损坏/过大即报错且不覆盖原文件（对照 config.rs ConfigStore）。
type ConfigStore struct {
	path string
	mu   sync.Mutex
	cur  AppConfig
}

const maxConfigBytes = 65536

func OpenConfig(path string) (*ConfigStore, error) {
	var config AppConfig
	if _, err := os.Stat(path); err == nil {
		raw, err := os.ReadFile(path)
		if err != nil {
			return nil, fmt.Errorf("读取配置失败：%w", err)
		}
		if len(raw) > maxConfigBytes {
			return nil, fmt.Errorf("配置文件过大；未覆盖原文件")
		}
		decoder := json.NewDecoder(bytes.NewReader(raw))
		decoder.DisallowUnknownFields()
		config = DefaultConfig()
		if err := decoder.Decode(&config); err != nil {
			return nil, fmt.Errorf("配置损坏，原文件已保留：%w", err)
		}
	} else {
		config = DefaultConfig()
		if err := writeConfigAtomic(path, config); err != nil {
			return nil, err
		}
	}
	if err := config.Validate(); err != nil {
		return nil, err
	}
	return &ConfigStore{path: path, cur: config}, nil
}

func (s *ConfigStore) Get() AppConfig {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cur
}

// Save 合并 patch、校验、原子落盘；失败时保留原配置与原文件。
func (s *ConfigStore) Save(patch json.RawMessage) (AppConfig, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	next, err := s.cur.merged(patch)
	if err != nil {
		return s.cur, err
	}
	if err := writeConfigAtomic(s.path, next); err != nil {
		return s.cur, err
	}
	s.cur = next
	return next, nil
}

// writeConfigAtomic：独占创建临时文件 → 写入 → fsync → 原子替换（对照 config.rs write_atomic）。
// Go 的 os.Rename 在 Windows 上使用 MoveFileEx(REPLACE_EXISTING)。
func writeConfigAtomic(path string, config AppConfig) error {
	data, err := json.MarshalIndent(config, "", "  ")
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	temp, err := os.CreateTemp(dir, filepath.Base(path)+".tmp-*")
	if err != nil {
		return err
	}
	tempName := temp.Name()
	cleanup := func() { _ = os.Remove(tempName) }
	if _, err := temp.Write(data); err != nil {
		temp.Close()
		cleanup()
		return err
	}
	if err := temp.Sync(); err != nil {
		temp.Close()
		cleanup()
		return err
	}
	if err := temp.Close(); err != nil {
		cleanup()
		return err
	}
	if err := os.Rename(tempName, path); err != nil {
		cleanup()
		return err
	}
	return nil
}
