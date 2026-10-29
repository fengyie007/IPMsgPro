//go:build !windows

package filetransfer

import (
	"os"
)

// isLink 非 Windows 平台只看符号链接。
func isLink(meta os.FileInfo) bool {
	return meta.Mode()&os.ModeSymlink != 0
}

// publishNew 非 Windows 用硬链接 + 删除（失败即目标已存在），目录用 mkdir + rename。
func publishNew(source, dest string) error {
	meta, err := os.Lstat(source)
	if err != nil {
		return err
	}
	if meta.IsDir() {
		if err := os.Mkdir(dest, 0o755); err != nil {
			return err
		}
		if err := os.Rename(source, dest); err != nil {
			_ = os.Remove(dest)
			return err
		}
		return nil
	}
	if err := os.Link(source, dest); err != nil {
		return err
	}
	if err := os.Remove(source); err != nil {
		_ = os.Remove(dest)
		return err
	}
	return nil
}
