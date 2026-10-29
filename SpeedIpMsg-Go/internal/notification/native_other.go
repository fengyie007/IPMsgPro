//go:build !windows

package notification

import (
	"fmt"
	"time"
)

type nativePlayer struct{ root string }

func newNativePlayer(root string) *nativePlayer { return &nativePlayer{root: root} }

func (p *nativePlayer) Play(cancelled func() bool) (time.Duration, error) {
	return 0, fmt.Errorf("当前平台暂不支持提示音")
}

func (p *nativePlayer) Stop() error { return nil }

func (p *nativePlayer) Cleanup() {}
