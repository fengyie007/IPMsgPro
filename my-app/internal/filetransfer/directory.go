// directory.go 对应 tauri-rust file/directory.rs：文件夹快照、目录流头编解码
// 与接收暂存目录。目录流格式为 "{长度4位hex}:{名称(转义::)}:{size:x}:{kind:x}:" 的 GBK 文本。
package filetransfer

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"golang.org/x/text/encoding/simplifiedchinese"
)

const (
	MaxDirEntries = 4096
	MaxDirDepth   = 32
)

// DirEntry kind：1=文件 2=目录 3=目录结束标记（"."）。
type DirEntry struct {
	Name     string
	Kind     uint32
	Size     uint64
	Path     string
	Modified time.Time // 零值表示未知
}

type Snapshot struct {
	Root    string
	Entries []DirEntry
	Size    uint64
}

// CaptureSnapshot 在发送前固化文件夹内容与总大小。
func CaptureSnapshot(path string) (*Snapshot, error) {
	resolved, err := filepath.EvalSymlinks(path)
	if err != nil {
		return nil, err
	}
	s := &Snapshot{Root: resolved}
	if err := s.walk(path, 0); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Snapshot) walk(path string, depth int) error {
	if depth > MaxDirDepth || len(s.Entries) >= MaxDirEntries {
		return errors.New("文件夹超过4096项或32层限制")
	}
	meta, err := os.Lstat(path)
	if err != nil {
		return err
	}
	if isLink(meta) {
		return errors.New("文件夹包含链接或重解析点，不能发送")
	}
	name := filepath.Base(path)
	var kind uint32
	switch {
	case meta.IsDir():
		kind = 2
	case meta.Mode().IsRegular():
		kind = 1
	default:
		return errors.New("文件夹包含特殊文件")
	}
	if kind == 2 && depth >= MaxDirDepth {
		return errors.New("文件夹层级过多")
	}
	if _, err := EncodeDirHeader(name, uint64(meta.Size()), kind); err != nil {
		return err
	}
	entry := DirEntry{Name: SanitizeName(name), Kind: kind, Path: path, Modified: meta.ModTime()}
	if kind == 1 {
		entry.Size = uint64(meta.Size())
		if s.Size+entry.Size > MaxFileSize {
			return errors.New("文件夹总量超过8 GiB")
		}
		s.Size += entry.Size
	}
	s.Entries = append(s.Entries, entry)
	if kind == 2 {
		dirEntries, err := os.ReadDir(path)
		if err != nil {
			return err
		}
		if len(dirEntries) > MaxDirEntries {
			return errors.New("文件夹项目过多")
		}
		children := make([]string, 0, len(dirEntries))
		for _, d := range dirEntries {
			children = append(children, filepath.Join(path, d.Name()))
		}
		sort.Strings(children)
		for _, child := range children {
			if err := s.walk(child, depth+1); err != nil {
				return err
			}
		}
		if len(s.Entries) >= MaxDirEntries {
			return errors.New("文件夹项目过多")
		}
		s.Entries = append(s.Entries, DirEntry{Name: ".", Kind: 3, Path: path})
	}
	return nil
}

// EncodeDirHeader 编码目录流头；名称必须能用 GBK 表示且总长不超过 8192。
func EncodeDirHeader(name string, size uint64, kind uint32) ([]byte, error) {
	text := fmt.Sprintf("%s:%x:%x:", strings.ReplaceAll(name, ":", "::"), size, kind)
	body, err := simplifiedchinese.GBK.NewEncoder().Bytes([]byte(text))
	if err != nil || len(body)+5 > 8192 {
		return nil, errors.New("目录项名称不能用GBK表示或过长")
	}
	header := fmt.Sprintf("%04x:", len(body)+5)
	return append([]byte(header), body...), nil
}

// DecodeDirHeader 解码目录流头，返回净化后的名称、大小与类型。
func DecodeDirHeader(b []byte) (string, uint64, uint32, error) {
	if len(b) < 5 || b[4] != ':' {
		return "", 0, 0, errors.New("无效目录头")
	}
	expected, err := strconv.ParseUint(string(b[:4]), 16, 32)
	if err != nil {
		return "", 0, 0, errors.New("无效目录长度")
	}
	if int(expected) != len(b) || expected > 8192 {
		return "", 0, 0, errors.New("目录长度错误")
	}
	decoded, err := simplifiedchinese.GBK.NewDecoder().Bytes(b[5:])
	if err != nil {
		return "", 0, 0, errors.New("无效目录头")
	}
	text := string(decoded)
	// 与 Offers 相同的名称转义解析： "::" 表示字面冒号。
	var name strings.Builder
	ended := false
	stop := 0
	for i := 0; i < len(text); i++ {
		c := text[i]
		if c == ':' {
			if i+1 < len(text) && text[i+1] == ':' {
				i++
				name.WriteByte(':')
				continue
			}
			stop = i
			ended = true
			break
		}
		name.WriteByte(c)
	}
	if !ended || name.Len() == 0 {
		return "", 0, 0, errors.New("目录文件名为空")
	}
	tail := strings.TrimRight(text[stop+1:], "\x00")
	fields := strings.Split(tail, ":")
	if len(fields) < 2 {
		return "", 0, 0, errors.New("目录头不完整")
	}
	size, err := strconv.ParseUint(fields[0], 16, 64)
	if err != nil {
		return "", 0, 0, errors.New("无效目录文件长度")
	}
	kind, err := strconv.ParseUint(fields[1], 16, 32)
	if err != nil {
		return "", 0, 0, errors.New("无效目录属性")
	}
	kindAttr := uint32(kind) & 0xff
	if (kindAttr != 1 && kindAttr != 2 && kindAttr != 3) || size > MaxFileSize || kindAttr != 1 && size != 0 {
		return "", 0, 0, errors.New("不支持的目录项")
	}
	if kindAttr != 3 && (name.String() == "." || name.String() == ".." || strings.ContainsAny(name.String(), "/\\")) {
		return "", 0, 0, errors.New("目录项不能包含路径")
	}
	return SanitizeName(name.String()), size, kindAttr, nil
}

// Staging 是接收文件夹的临时目录；发布成功后保留，放弃时整体删除。
type Staging struct {
	Root string
	keep bool
}

func CreateStaging(root string, id uint32) (*Staging, error) {
	for n := 0; n < 1000; n++ {
		dir := filepath.Join(root, fmt.Sprintf(".ipmsg-dir-%d-%d.part", id, n))
		if err := os.Mkdir(dir, 0o755); err == nil {
			resolved, err := filepath.EvalSymlinks(dir)
			if err != nil {
				return nil, err
			}
			return &Staging{Root: resolved}, nil
		} else if os.IsExist(err) {
			continue
		} else {
			return nil, err
		}
	}
	return nil, errors.New("无法创建接收目录")
}

func (s *Staging) Publish(parent, name string) (string, error) {
	name = SanitizeName(name)
	for n := 0; n < 10000; n++ {
		dest := name
		if n > 0 {
			dest = fmt.Sprintf("%s (%d)", name, n)
		}
		destPath := filepath.Join(parent, dest)
		if err := publishNew(s.Root, destPath); err != nil {
			if os.IsExist(err) {
				continue
			}
			return "", err
		}
		s.Root = destPath
		return destPath, nil
	}
	return "", errors.New("同名目录过多")
}

func (s *Staging) Keep() { s.keep = true }

func (s *Staging) Discard() {
	if !s.keep {
		_ = os.RemoveAll(s.Root)
	}
}
