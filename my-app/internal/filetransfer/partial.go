// partial.go 对应 tauri-rust file/storage.rs：接收临时文件（.part）的
// 创建、续传打开与安全发布。只删除本程序自己创建的文件，绝不触碰对端路径。
package filetransfer

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type PartialFile struct {
	path string
	keep bool
}

func (p *PartialFile) Name() string {
	return filepath.Base(p.path)
}

// ResumePartial 打开一次中断留下的 .part 续传；文件被改动或超过预期大小则拒绝。
func ResumePartial(root, name string, size uint64) (*PartialFile, *os.File, error) {
	path, err := checkedPart(root, name)
	if err != nil {
		return nil, nil, err
	}
	meta, err := os.Lstat(path)
	if err != nil {
		return nil, nil, err
	}
	if !meta.Mode().IsRegular() || isLink(meta) || uint64(meta.Size()) > size {
		return nil, nil, fmt.Errorf("续传文件已改变或不安全")
	}
	file, err := os.OpenFile(path, os.O_RDWR, 0)
	if err != nil {
		return nil, nil, err
	}
	return &PartialFile{path: path, keep: true}, file, nil
}

// CreatePartial 以独占方式创建新的接收临时文件。
func CreatePartial(root string, id uint32) (*PartialFile, *os.File, error) {
	nonce := time.Now().UnixNano()
	for sequence := 0; sequence < 1000; sequence++ {
		path := filepath.Join(root, fmt.Sprintf(".ipmsg-%d-%d-%d-%d.part", os.Getpid(), nonce, id, sequence))
		file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
		if err == nil {
			return &PartialFile{path: path}, file, nil
		}
		if os.IsExist(err) {
			continue
		}
		return nil, nil, err
	}
	return nil, nil, fmt.Errorf("无法分配接收临时文件")
}

// Publish 把 .part 移动为 root 下不冲突的正式文件名（绝不覆盖已有文件）。
func (p *PartialFile) Publish(root, name string) (string, error) {
	name = SanitizeName(name)
	stem, ext := stemAndExt(name)
	for n := 0; n < 10000; n++ {
		dest := name
		if n > 0 {
			dest = fmt.Sprintf("%s (%d)%s", stem, n, ext)
		}
		destPath := filepath.Join(root, dest)
		if err := publishNew(p.path, destPath); err != nil {
			if os.IsExist(err) {
				continue
			}
			return "", fmt.Errorf("无法安全保存接收文件：%v", err)
		}
		p.path = destPath
		return destPath, nil
	}
	return "", fmt.Errorf("同名文件过多")
}

// Keep 阻止 Discard 删除文件（发布成功或目录保留时调用）。
func (p *PartialFile) Keep() { p.keep = true }

// Discard 删除仍处于临时状态的 .part 文件。
func (p *PartialFile) Discard() {
	if !p.keep {
		_ = os.Remove(p.path)
	}
}

// checkedPart 校验续传标识只能指向本程序创建的 .part 文件名。
func checkedPart(root, name string) (string, error) {
	if !strings.HasPrefix(name, ".ipmsg-") || !strings.HasSuffix(name, ".part") {
		return "", fmt.Errorf("无效续传标识")
	}
	for i := 0; i < len(name); i++ {
		b := name[i]
		if !(b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= '0' && b <= '9' || b == '.' || b == '-') {
			return "", fmt.Errorf("无效续传标识")
		}
	}
	return filepath.Join(root, name), nil
}

func stemAndExt(name string) (string, string) {
	ext := filepath.Ext(name)
	return strings.TrimSuffix(name, ext), ext
}
