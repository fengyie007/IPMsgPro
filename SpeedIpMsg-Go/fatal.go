// fatal.go：启动期/致命错误必须可见。
// 打包后是 GUI 子系统程序（无控制台），log.Fatalf 只会静默退出——用户看到的是"双击没反应"。
package main

import (
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
)

var (
	logPath string
	logFile *os.File
)

// setupLog 把日志写到数据目录的 SpeedIpMsg-Go.log（每次启动截断），同时保留 stderr 便于命令行排查。
func setupLog(dataDir string) {
	_ = os.MkdirAll(dataDir, 0o755)
	path := filepath.Join(dataDir, "SpeedIpMsg-Go.log")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
	if err != nil {
		log.Printf("无法创建日志文件 %s: %v", path, err)
		return
	}
	logPath, logFile = path, file
	log.SetOutput(io.MultiWriter(file, os.Stderr))
}

// fatal 记录错误并弹出对话框，然后退出；没有日志文件时退化为仅弹框。
func fatal(format string, args ...any) {
	msg := fmt.Sprintf(format, args...)
	log.Print(msg)
	if logPath != "" {
		msg += "\n\n日志：" + logPath
	}
	showErrorBox("迅秋 MyGo", msg)
	os.Exit(1)
}
