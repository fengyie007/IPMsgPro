//go:build windows

package app

import (
	"os/exec"
	"syscall"
)

// revealInExplorer 在资源管理器中定位文件（CREATE_NO_WINDOW 避免闪黑框）。
func revealInExplorer(path string) error {
	cmd := exec.Command("explorer.exe", "/select,", path)
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: 0x08000000}
	return cmd.Start()
}
