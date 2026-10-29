// Package storage 对拍 tauri-rust src-tauri/src/storage.rs + storage/layout.rs：
// 仅限重启时机的数据目录迁移。启动引导目录（bootstrap）里的 data-location.json
// 记录活动数据集与待迁移选择；数据集身份由 .speedipmsg-dataset.json 标记校验。
// 旧版目录布局兼容导入（open_default/open_compatible）按迁移决策 6
// （历史数据不迁移）刻意不移植：MyGo 版从不读取旧实例数据。
package storage

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

const (
	// AppID 是数据集标记与系统通知身份共用的应用标识。
	AppID = "com.speedipmsg.mygo"
	// MarkerFile 是数据集目录内的身份标记。
	MarkerFile = ".speedipmsg-dataset.json"
	// LocatorFile 是 bootstrap 目录内的定位文件。
	LocatorFile = "data-location.json"

	maxLocatorBytes = 65536
	maxCopyEntries  = 200000
	maxCopyDepth    = 64
	// DefaultPort 与 ipmsg.DefaultPort 一致（2425），避免包循环。
	DefaultPort = 2425
)

func token() string {
	return fmt.Sprintf("%d-%d-%d", os.Getpid(), time.Now().UnixNano(),
		tokenSeq.Add(1))
}

var tokenSeq atomic.Uint64

// DirectoryName 对拍 layout.rs directory_name：默认端口用平铺根目录名。
func DirectoryName(port uint16) string {
	if port == DefaultPort {
		return "SpeedIpMsg"
	}
	return fmt.Sprintf("SpeedIpMsg-%d", port)
}

// DefaultDirectory 对拍 layout.rs default_directory。
func DefaultDirectory(localData string, port uint16) string {
	return filepath.Join(localData, DirectoryName(port))
}

// UnusedDirectory 在 parent 下找第一个不存在的 sibling 名（stem / stem-2..）。
func UnusedDirectory(parent, stem string) (string, error) {
	for n := 1; n < 10000; n++ {
		name := stem
		if n > 1 {
			name = fmt.Sprintf("%s-%d", stem, n)
		}
		target := filepath.Join(parent, name)
		if _, err := os.Lstat(target); err != nil {
			if os.IsNotExist(err) {
				return target, nil
			}
			return "", err
		}
	}
	return "", fmt.Errorf("同名数据目录过多，请选择其他位置")
}

type locator struct {
	Active  string  `json:"active,omitempty"`
	Pending *Choice `json:"pending,omitempty"`
}

// Choice 是一次待迁移的目标目录登记（token 防串台）。
type Choice struct {
	Token  string `json:"token"`
	Target string `json:"target"`
}

type marker struct {
	App      string `json:"app"`
	Port     uint16 `json:"port"`
	Token    string `json:"token"`
	Complete bool   `json:"complete"`
}

func readJSON(path string, out any) error {
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if info.Size() > maxLocatorBytes {
		return fmt.Errorf("目录定位文件过大")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	return json.Unmarshal(data, out)
}

func readLocator(root string) (locator, error) {
	var loc locator
	path := filepath.Join(root, LocatorFile)
	if _, err := os.Stat(path); err != nil {
		return loc, nil
	}
	err := readJSON(path, &loc)
	return loc, err
}

// atomicJSON 独占临时文件后原子替换（os.Rename 在 Windows 上即
// MOVEFILE_REPLACE_EXISTING，与 Rust 写法等价）。
func atomicJSON(path string, value any) error {
	temp := path + ".tmp-" + token()
	file, err := os.OpenFile(temp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
	if err != nil {
		return err
	}
	data, err := json.MarshalIndent(value, "", "  ")
	if err == nil {
		if _, err = file.Write(data); err == nil {
			err = file.Sync()
		}
	}
	file.Close()
	if err != nil {
		_ = os.Remove(temp)
		return err
	}
	if err := os.Rename(temp, path); err != nil {
		_ = os.Remove(temp)
		return err
	}
	return nil
}

// samePath Windows 下大小写不敏感比较。
func samePath(a, b string) bool {
	return strings.EqualFold(a, b)
}

// insideDir 判断 path 是否位于 parent 之内（大小写不敏感）。
func insideDir(path, parent string) bool {
	p := strings.TrimRight(strings.ToLower(path), "\\")
	root := strings.TrimRight(strings.ToLower(parent), "\\")
	return p == root || strings.HasPrefix(p, root+string(filepath.Separator))
}

func isLinkMeta(meta os.FileInfo) bool {
	if meta.Mode()&os.ModeSymlink != 0 {
		return true
	}
	return reparsePoint(meta)
}

type ownedDataset struct {
	path  string
	port  uint16
	token string
	keep  bool
}

// Discard 删除仍归自己所有的未保留数据集（身份标记必须匹配，绝不误删）。
func (d *ownedDataset) Discard() {
	if d.keep {
		return
	}
	var m marker
	if err := readJSON(filepath.Join(d.path, MarkerFile), &m); err != nil {
		return
	}
	if m.App == AppID && m.Port == d.port && m.Token == d.token {
		if resolved, err := filepath.EvalSymlinks(d.path); err == nil && resolved == d.path {
			_ = os.RemoveAll(d.path)
		}
	}
}

// copyTree 复制文件/目录树，拒绝链接/特殊文件，校验复制后大小与修改时间。
func copyTree(source, target string, depth int, count *int) error {
	*count++
	if *count > maxCopyEntries || depth > maxCopyDepth {
		return fmt.Errorf("数据目录层级或文件数量超过迁移上限")
	}
	before, err := os.Lstat(source)
	if err != nil {
		return err
	}
	if isLinkMeta(before) {
		return fmt.Errorf("数据目录包含链接或重解析点，已停止迁移")
	}
	if before.IsDir() {
		if err := os.Mkdir(target, 0o755); err != nil {
			return err
		}
		entries, err := os.ReadDir(source)
		if err != nil {
			return err
		}
		for _, entry := range entries {
			if err := copyTree(filepath.Join(source, entry.Name()), filepath.Join(target, entry.Name()), depth+1, count); err != nil {
				return err
			}
		}
		return nil
	}
	if !before.Mode().IsRegular() {
		return fmt.Errorf("数据目录包含特殊文件")
	}
	input, err := os.Open(source)
	if err != nil {
		return err
	}
	defer input.Close()
	out, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
	if err != nil {
		return err
	}
	written, copyErr := out.ReadFrom(input)
	syncErr := out.Sync()
	out.Close()
	if copyErr != nil {
		return copyErr
	}
	if syncErr != nil {
		return syncErr
	}
	after, err := os.Stat(target)
	if err != nil {
		return err
	}
	if uint64(written) != uint64(before.Size()) || uint64(after.Size()) != uint64(before.Size()) {
		return fmt.Errorf("复制过程中源数据发生变化")
	}
	if afterInfo, err := input.Stat(); err == nil && !afterInfo.ModTime().Equal(before.ModTime()) {
		return fmt.Errorf("复制过程中源数据发生变化")
	}
	return nil
}

func createDataset(target string, port uint16, token string) (*ownedDataset, error) {
	parent := filepath.Dir(target)
	if err := os.MkdirAll(parent, 0o755); err != nil {
		return nil, err
	}
	resolved, err := filepath.EvalSymlinks(parent)
	if err != nil || resolved != parent {
		return nil, fmt.Errorf("目标目录经过重解析点，已停止迁移")
	}
	if err := os.Mkdir(target, 0o755); err != nil {
		return nil, fmt.Errorf("目标已存在或无法创建，未覆盖任何数据：%v", err)
	}
	resolved, err = filepath.EvalSymlinks(target)
	if err != nil {
		_ = os.Remove(target)
		return nil, err
	}
	owned := &ownedDataset{path: resolved, port: port, token: token}
	m := marker{App: AppID, Port: port, Token: token, Complete: false}
	if err := atomicJSON(filepath.Join(resolved, MarkerFile), &m); err != nil {
		_ = os.Remove(resolved) // 标记写入失败时目录还是空的，可以安全删除
		return nil, err
	}
	return owned, nil
}

func validateMarker(dir string, port uint16) error {
	var m marker
	if err := readJSON(filepath.Join(dir, MarkerFile), &m); err != nil {
		return err
	}
	if m.App != AppID || m.Port != port || !m.Complete {
		return fmt.Errorf("数据目录身份不符或迁移未完成，未加载其他实例数据")
	}
	return nil
}

type locationError struct {
	message string
	invalid bool // true=数据身份不符（致命）；false=目录不可用（可回退默认）
}

func (e *locationError) Error() string { return e.message }

func configuredDirectory(path string, port uint16) (string, error) {
	resolved, err := filepath.EvalSymlinks(path)
	if err != nil {
		return "", &locationError{message: err.Error()}
	}
	info, err := os.Stat(resolved)
	if err != nil || !info.IsDir() {
		return "", &locationError{message: "路径不是文件夹"}
	}
	if _, err := os.ReadDir(resolved); err != nil {
		return "", &locationError{message: err.Error()}
	}
	// 标记缺失/不可读就不能认定目录可用；可读但身份不符是致命错误，绝不混用实例。
	if f, err := os.Open(filepath.Join(resolved, MarkerFile)); err != nil {
		return "", &locationError{message: err.Error()}
	} else {
		f.Close()
	}
	if err := validateMarker(resolved, port); err != nil {
		return "", &locationError{message: err.Error(), invalid: true}
	}
	for _, name := range []string{"config.json", "ipmsg.db"} {
		file := filepath.Join(resolved, name)
		if _, err := os.Stat(file); err == nil {
			f, err := os.OpenFile(file, os.O_RDWR, 0)
			if err != nil {
				return "", &locationError{message: err.Error()}
			}
			f.Close()
		}
	}
	probe := filepath.Join(resolved, fmt.Sprintf(".speedipmsg-write-%s.tmp", token()))
	f, err := os.OpenFile(probe, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
	if err != nil {
		return "", &locationError{message: err.Error()}
	}
	_, writeErr := f.WriteString("SpeedIPMsg directory probe")
	syncErr := f.Sync()
	f.Close()
	removeErr := os.Remove(probe)
	if writeErr != nil {
		return "", &locationError{message: writeErr.Error()}
	}
	if syncErr != nil {
		return "", &locationError{message: syncErr.Error()}
	}
	if removeErr != nil {
		return "", &locationError{message: removeErr.Error()}
	}
	return resolved, nil
}

func completeDataset(owned *ownedDataset) error {
	return atomicJSON(filepath.Join(owned.path, MarkerFile), &marker{
		App: AppID, Port: owned.port, Token: owned.token, Complete: true,
	})
}

// freshDefault 在 bootstrap 下开一个全新默认数据集，并备份原定位记录。
func freshDefault(bootstrap string, port uint16, previous locator) (string, error) {
	tok := token()
	target, err := UnusedDirectory(bootstrap, "data")
	if err != nil {
		return "", err
	}
	fresh, err := createDataset(target, port, tok)
	if err != nil {
		return "", err
	}
	// 绝不复用 bootstrap 里历史遗留的副本：重建空的 config 与零版本 DB。
	if err := os.WriteFile(filepath.Join(fresh.path, "config.json"), []byte("{}\n"), 0o666); err != nil {
		return "", err
	}
	if err := completeDataset(fresh); err != nil {
		return "", err
	}
	if err := atomicJSON(filepath.Join(bootstrap, fmt.Sprintf("data-location-unavailable-%s.json", tok)), previous); err != nil {
		return "", err
	}
	if err := atomicJSON(filepath.Join(bootstrap, LocatorFile), locator{Active: fresh.path}); err != nil {
		return "", err
	}
	fresh.keep = true
	return fresh.path, nil
}

// migrate 把当前活动数据复制到新目标并补全标记。
func migrate(source string, choice *Choice, port uint16) (*ownedDataset, error) {
	if !filepath.IsAbs(choice.Target) || insideDir(choice.Target, source) {
		return nil, fmt.Errorf("目标目录不能位于当前数据目录内")
	}
	if info, err := os.Stat(filepath.Join(source, "ipmsg.db")); err != nil || !info.Mode().IsRegular() {
		return nil, fmt.Errorf("当前数据不完整，未执行迁移")
	}
	if info, err := os.Stat(filepath.Join(source, "config.json")); err != nil || !info.Mode().IsRegular() {
		return nil, fmt.Errorf("当前数据不完整，未执行迁移")
	}
	owned, err := createDataset(choice.Target, port, choice.Token)
	if err != nil {
		return nil, err
	}
	count := 0
	for _, name := range []string{
		"config.json", "ipmsg.db", "ipmsg.db-wal", "ipmsg.db-shm",
		"ipmsg.db-probe", "images", "sounds",
	} {
		input := filepath.Join(source, name)
		if _, err := os.Stat(input); err == nil {
			if err := copyTree(input, filepath.Join(owned.path, name), 0, &count); err != nil {
				return nil, err
			}
		}
	}
	if err := completeDataset(owned); err != nil {
		return nil, err
	}
	return owned, nil
}

// Storage 管理数据目录的解析与重启时机迁移。
type Storage struct {
	Active    string
	bootstrap string
	port      uint16

	mu       sync.Mutex
	selected *Choice
	error    string
}

// Open 解析活动数据目录；bootstrap 不可用时自动回退新默认位置，
// 待迁移的 pending 选择在本次启动时执行（对拍 Storage::open）。
func Open(bootstrap string, port uint16) (*Storage, error) {
	if err := os.MkdirAll(bootstrap, 0o755); err != nil {
		return nil, err
	}
	resolved, err := filepath.EvalSymlinks(bootstrap)
	if err != nil {
		return nil, err
	}
	bootstrap = resolved
	loc, err := readLocator(bootstrap)
	if err != nil {
		return nil, err
	}
	configured := loc.Active
	if configured == "" {
		configured = bootstrap
	}
	var active string
	var openErr string
	if samePath(configured, bootstrap) {
		active = bootstrap
	} else {
		path, err := configuredDirectory(configured, port)
		if err != nil {
			locErr, ok := err.(*locationError)
			if ok && locErr.invalid {
				return nil, err
			}
			path, err = freshDefault(bootstrap, port, loc)
			if err != nil {
				return nil, fmt.Errorf("原数据目录不可用，创建默认配置失败：%v", err)
			}
			openErr = fmt.Sprintf("原数据目录不可用（%s：%v），已在默认位置创建新配置；原定位记录已备份，原数据未作修改。", configured, locErr.message)
			loc = locator{Active: path}
		}
		active = path
	}
	if !samePath(active, bootstrap) {
		if _, err := os.Stat(filepath.Join(active, MarkerFile)); err == nil {
			if err := validateMarker(active, port); err != nil {
				return nil, err
			}
		}
	}
	if pending := loc.Pending; pending != nil {
		loc.Pending = nil
		if !samePath(pending.Target, active) {
			copied, err := migrate(active, pending, port)
			if err == nil {
				next := locator{Active: copied.path}
				if err := atomicJSON(filepath.Join(bootstrap, LocatorFile), next); err == nil {
					active = copied.path
					copied.keep = true
					loc = next
				} else {
					openErr = err.Error()
				}
			} else {
				openErr = err.Error()
			}
		}
		if openErr != "" {
			loc.Active = active
			_ = atomicJSON(filepath.Join(bootstrap, LocatorFile), loc)
		}
	}
	return &Storage{
		Active: active, bootstrap: bootstrap, port: port, error: openErr,
	}, nil
}

func (s *Storage) isDefaultLocation() bool {
	if samePath(s.Active, s.bootstrap) {
		return true
	}
	return samePath(filepath.Dir(s.Active), s.bootstrap)
}

// Info 输出前端存储设置页需要的快照。
func (s *Storage) Info() (map[string]any, error) {
	loc, err := readLocator(s.bootstrap)
	if err != nil {
		return nil, err
	}
	var pending any
	if loc.Pending != nil {
		pending = loc.Pending.Target
	}
	return map[string]any{
		"success":          true,
		"active":           s.Active,
		"pending":          pending,
		"defaultDirectory": s.bootstrap,
		"isDefault":        s.isDefaultLocation(),
		"error":            s.errorText(),
	}, nil
}

func (s *Storage) errorText() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.error
}

// Bootstrap 返回启动引导目录（存储设置页的"默认位置"）。
func (s *Storage) Bootstrap() string { return s.bootstrap }

// Prepare 预检目标父目录并登记一次待迁移选择（真正迁移在下一次启动执行）。
func (s *Storage) Prepare(parent string, reset bool) (map[string]any, error) {
	resolved, err := filepath.EvalSymlinks(parent)
	if err != nil {
		return nil, err
	}
	if info, err := os.Stat(resolved); err != nil || !info.IsDir() {
		return nil, fmt.Errorf("请选择文件夹")
	}
	tok := token()
	stem := DirectoryName(s.port)
	if reset {
		stem = "data"
	}
	target, err := UnusedDirectory(resolved, stem)
	if err != nil {
		return nil, err
	}
	if insideDir(target, s.Active) {
		return nil, fmt.Errorf("目标不能位于当前数据目录内")
	}
	probe := filepath.Join(resolved, fmt.Sprintf(".speedipmsg-write-%s.tmp", tok))
	f, err := os.OpenFile(probe, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
	if err != nil {
		return nil, fmt.Errorf("目标目录不可写：%v", err)
	}
	_, writeErr := f.WriteString("SpeedIPMsg directory probe")
	f.Close()
	_ = os.Remove(probe)
	if writeErr != nil {
		return nil, writeErr
	}
	s.mu.Lock()
	s.selected = &Choice{Token: tok, Target: target}
	s.mu.Unlock()
	return map[string]any{
		"success": true, "selectionId": tok, "source": s.Active, "target": target,
	}, nil
}

// Apply 落盘 pending 选择；下一次启动时完成迁移。
func (s *Storage) Apply(selection string) (map[string]any, error) {
	s.mu.Lock()
	chosen := s.selected
	s.selected = nil
	s.mu.Unlock()
	if chosen == nil || chosen.Token != selection {
		return nil, fmt.Errorf("目录选择已失效")
	}
	if err := atomicJSON(filepath.Join(s.bootstrap, LocatorFile), locator{
		Active: s.Active, Pending: chosen,
	}); err != nil {
		return nil, err
	}
	s.mu.Lock()
	s.error = ""
	s.mu.Unlock()
	return s.Info()
}

// CancelPending 放弃待迁移选择。
func (s *Storage) CancelPending() (map[string]any, error) {
	if err := atomicJSON(filepath.Join(s.bootstrap, LocatorFile), locator{
		Active: s.Active,
	}); err != nil {
		return nil, err
	}
	return s.Info()
}

// OpenError 返回启动期存储回退提示（可能为空）。
func (s *Storage) OpenError() string { return s.errorText() }
