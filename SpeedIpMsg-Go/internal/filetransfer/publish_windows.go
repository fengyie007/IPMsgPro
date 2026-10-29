//go:build windows

package filetransfer

import (
	"os"
	"syscall"
	"unsafe"
)

var moveFileExProc = syscall.NewLazyDLL("kernel32.dll").NewProc("MoveFileExW")

// isLink 判断重解析点（含目录联接）与符号链接（对照 Rust directory::is_link）。
func isLink(meta os.FileInfo) bool {
	if meta.Mode()&os.ModeSymlink != 0 {
		return true
	}
	if attrs, ok := meta.Sys().(*syscall.Win32FileAttributeData); ok {
		return attrs.FileAttributes&0x400 != 0
	}
	return false
}

// publishNew 用 MoveFileExW(flags=0) 移动：刻意排除 MOVEFILE_REPLACE_EXISTING，
// 绝不覆盖已存在的目标，FAT 卷同样可用（对照 Rust storage.rs publish_new）。
func publishNew(source, dest string) error {
	src, err := syscall.UTF16PtrFromString(source)
	if err != nil {
		return err
	}
	dst, err := syscall.UTF16PtrFromString(dest)
	if err != nil {
		return err
	}
	r1, _, callErr := moveFileExProc.Call(uintptr(unsafe.Pointer(src)), uintptr(unsafe.Pointer(dst)), 0)
	if r1 == 0 {
		if callErr != syscall.Errno(0) {
			return callErr
		}
		return syscall.EINVAL
	}
	return nil
}
