//go:build windows

// native_windows.go 对拍 tauri-rust notification/native.rs：
// 内置提示音写为用户缓存目录的临时 MP3，经 MCI 播放；不覆盖已有文件。
package notification

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"
	"sync/atomic"
	"syscall"
	"time"
	"unsafe"
)

//go:embed notification.mp3
var audio []byte

var (
	modwinmm               = syscall.NewLazyDLL("winmm.dll")
	procMciSendStringW     = modwinmm.NewProc("mciSendStringW")
	procMciGetErrorStringW = modwinmm.NewProc("mciGetErrorStringW")
)

func mciCommand(text string) error {
	wide, err := syscall.UTF16PtrFromString(text)
	if err != nil {
		return err
	}
	code, _, _ := procMciSendStringW.Call(uintptr(unsafe.Pointer(wide)), 0, 0, 0)
	if code == 0 {
		return nil
	}
	buf := make([]uint16, 256)
	procMciGetErrorStringW.Call(code, uintptr(unsafe.Pointer(&buf[0])), uintptr(len(buf)))
	end := 0
	for end < len(buf) && buf[end] != 0 {
		end++
	}
	return fmt.Errorf("提示音设备错误 %d：%s", code, syscall.UTF16ToString(buf[:end]))
}

// soundFile 是一份独占的临时提示音文件，关闭时删除。
type soundFile struct{ path string }

func createSoundFile(root string) (*soundFile, error) {
	if err := os.MkdirAll(root, 0o755); err != nil {
		return nil, fmt.Errorf("无法创建提示音缓存：%v", err)
	}
	var next atomic.Uint32
	for sequence := 0; sequence < 1000; sequence++ {
		path := filepath.Join(root, fmt.Sprintf("notification-%d-%d-%d.mp3",
			os.Getpid(), time.Now().UnixNano(), next.Add(1)))
		file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
		if err != nil {
			if os.IsExist(err) {
				continue
			}
			return nil, err
		}
		_, writeErr := file.Write(audio)
		if closeErr := file.Close(); writeErr == nil {
			writeErr = closeErr
		}
		if writeErr != nil {
			_ = os.Remove(path)
			return nil, writeErr
		}
		return &soundFile{path: path}, nil
	}
	return nil, fmt.Errorf("无法分配提示音缓存文件")
}

func (s *soundFile) Discard() {
	if s != nil {
		_ = os.Remove(s.path)
	}
}

type nativePlayer struct {
	root   string
	sound  *soundFile
	opened bool
}

func newNativePlayer(root string) *nativePlayer {
	return &nativePlayer{root: root}
}

func (p *nativePlayer) Play(cancelled func() bool) (time.Duration, error) {
	if cancelled() {
		return 0, fmt.Errorf("提示音请求已取消")
	}
	if p.sound == nil {
		file, err := createSoundFile(p.root)
		if err != nil {
			return 0, err
		}
		p.sound = file
	}
	open := func(typed bool) error {
		suffix := "\" alias speedipmsg_notify"
		if typed {
			suffix = "\" type mpegvideo alias speedipmsg_notify"
		}
		return mciCommand(`open "` + p.sound.path + suffix)
	}
	if cancelled() {
		return 0, fmt.Errorf("提示音请求已取消")
	}
	if err := open(true); err != nil {
		if err2 := open(false); err2 != nil {
			return 0, err2
		}
	}
	p.opened = true
	if err := mciCommand("set speedipmsg_notify time format milliseconds"); err != nil {
		return 0, err
	}
	length, err := mciStatusLength()
	if err != nil {
		return 0, err
	}
	if length == 0 || length > 15000 {
		return 0, fmt.Errorf("提示音时长无效")
	}
	if cancelled() {
		return 0, fmt.Errorf("提示音请求已取消")
	}
	if err := mciCommand("play speedipmsg_notify from 0"); err != nil {
		return 0, err
	}
	return time.Duration(length+100) * time.Millisecond, nil
}

func mciStatusLength() (uint64, error) {
	output := make([]uint16, 32)
	wide, err := syscall.UTF16PtrFromString("status speedipmsg_notify length")
	if err != nil {
		return 0, err
	}
	code, _, _ := procMciSendStringW.Call(uintptr(unsafe.Pointer(wide)),
		uintptr(unsafe.Pointer(&output[0])), uintptr(len(output)), 0)
	if code != 0 {
		return 0, fmt.Errorf("提示音设备错误 %d", code)
	}
	end := 0
	for end < len(output) && output[end] != 0 {
		end++
	}
	var length uint64
	text := syscall.UTF16ToString(output[:end])
	if _, err := fmt.Sscanf(text, "%d", &length); err != nil {
		return 0, fmt.Errorf("无法读取提示音时长")
	}
	return length, nil
}

func (p *nativePlayer) Stop() error {
	if p.opened {
		_ = mciCommand("stop speedipmsg_notify")
		if err := mciCommand("close speedipmsg_notify"); err != nil {
			p.opened = false
			return err
		}
		p.opened = false
	}
	return nil
}

// Cleanup 删除临时提示音缓存（工作线程退出时调用）。
func (p *nativePlayer) Cleanup() { p.sound.Discard() }
