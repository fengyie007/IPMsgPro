package store

// 扫描范围校验测试对拍 tauri-rust scan.rs 的同名用例。

import (
	"testing"
)

func mustParseRange(t *testing.T, text string) [2]uint32 {
	t.Helper()
	r, err := ParseRange(text)
	if err != nil {
		t.Fatalf("ParseRange(%q) 失败: %v", text, err)
	}
	return r
}

func TestRangeFormsAndBoundaries(t *testing.T) {
	cases := []struct {
		text  string
		start string
		end   string
	}{
		{"10.8.33.1-254", "10.8.33.1", "10.8.33.254"},
		{" 10.8.33.9/24 ", "10.8.33.1", "10.8.33.254"},
		{"10.8.33.0/31", "10.8.33.0", "10.8.33.1"},
		{"127.0.0.1/32", "127.0.0.1", "127.0.0.1"},
		{"127.0.0.1", "127.0.0.1", "127.0.0.1"},
		{"10.0.0.255-10.0.1.0", "10.0.0.255", "10.0.1.0"},
	}
	for _, c := range cases {
		got := mustParseRange(t, c.text)
		want := [2]uint32{mustParseRange(t, c.start)[0], mustParseRange(t, c.end)[1]}
		if got != want {
			t.Fatalf("ParseRange(%q) = %v, want %v", c.text, got, want)
		}
	}
	for _, text := range []string{
		"", "10.0.0.2-1", "10.0.0.1-256", "10.0.0.1/33", "10.0.0.1/", "10.0.0.1/-1",
		"0.0.0.1", "224.0.0.1", "255.255.255.255", "10.0.0.0/15", "01.2.3.4", "10.0.0.1-2-3",
	} {
		if _, err := ParseRange(text); err == nil {
			t.Errorf("应拒绝 %q", text)
		}
	}
}

func TestMergeDeduplicatesOverlapsAndCapsCombinedTotal(t *testing.T) {
	plan, err := ScanOptions{Ranges: []string{"10.0.0.1-5", "10.0.0.3-7", "10.0.0.7-9"}, Port: 2425, DelayMs: 20}.Plan()
	if err != nil {
		t.Fatal(err)
	}
	if plan.Total != 9 || len(plan.Ranges) != 1 {
		t.Fatalf("重叠范围应合并: %+v", plan)
	}
	options := ScanOptions{Ranges: []string{"10.0.0.0-10.0.255.255"}, Port: 2425, DelayMs: 10}
	plan, err = options.Plan()
	if err != nil || plan.Total != 65536 {
		t.Fatalf("单范围 65536 应可行: %v %v", plan, err)
	}
	options.Ranges = append(options.Ranges, "10.1.0.1")
	if _, err := options.Plan(); err == nil {
		t.Fatal("合并后超 65536 应报错")
	}
	options.Ranges = nil
	if plan, err = options.Plan(); err != nil || plan.Total != 0 {
		t.Fatalf("空范围应可行: %v %v", plan, err)
	}
	options.DelayMs = 0
	if _, err := options.Plan(); err == nil {
		t.Fatal("delay=0 应报错")
	}
	options.DelayMs = 20
	options.Port = 0
	if _, err := options.Plan(); err == nil {
		t.Fatal("port=0 应报错")
	}
}

func TestDirectAddressValidation(t *testing.T) {
	for _, text := range []string{"127.0.0.1:2426", "192.168.2.88:2425"} {
		if err := ParseDirectAddress(text); err != nil {
			t.Errorf("%s 应合法: %v", text, err)
		}
	}
	for _, text := range []string{"", "127.0.0.1:0", "0.0.0.0:2425", "224.0.0.1:2425", "255.255.255.255:2425", "localhost:2425", "127.0.0.1"} {
		if err := ParseDirectAddress(text); err == nil {
			t.Errorf("应拒绝 %q", text)
		}
	}
}
