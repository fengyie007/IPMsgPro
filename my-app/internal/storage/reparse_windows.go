//go:build windows

package storage

import (
	"os"
	"syscall"
)

// reparsePoint 检查 Windows 重解析点属性（符号链接/目录联接）。
func reparsePoint(meta os.FileInfo) bool {
	if attrs, ok := meta.Sys().(*syscall.Win32FileAttributeData); ok {
		return attrs.FileAttributes&0x400 != 0
	}
	return false
}
