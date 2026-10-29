//go:build windows

package notification

import (
	"testing"
	"time"
)

// 回归：连续试听曾报 263（设备未打开）/ 289（别名被占用）——播放工作协程被 Go
// 调度器搬到了别的 OS 线程，而 MCI 设备绑定在打开它的线程上。
// 无声卡的环境直接跳过（那不是本用例要测的路径）。
func TestRepeatedPreviewKeepsWorking(t *testing.T) {
	s, err := New(t.TempDir(), true, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = s.Shutdown() }()

	for i := 0; i < 3; i++ {
		duration, err := s.Preview()
		if err != nil {
			if i == 0 {
				t.Skipf("本机无法播放提示音，跳过：%v", err)
			}
			t.Fatalf("第 %d 次试听失败：%v", i+1, err)
		}
		if duration == 0 {
			t.Fatalf("第 %d 次试听时长为 0", i+1)
		}
		time.Sleep(300 * time.Millisecond)
	}
}
