package notification

import (
	"strings"
	"testing"
)

func TestCleanFiltersControlsAndLimits(t *testing.T) {
	if got := Clean("a\x00b\x07c", 10); got != "abc" {
		t.Fatalf("控制字符未过滤: %q", got)
	}
	if got := Clean("多字节截断测试字符串", 2); got != "多字" {
		t.Fatalf("按 rune 截断异常: %q", got)
	}
	if got := Clean("a\nb\tc", 10); got != "a\nb\tc" {
		t.Fatalf("换行制表应保留: %q", got)
	}
	if !strings.EqualFold(Clean("OK", 2), "OK") {
		t.Fatal("普通文本应原样")
	}
}
