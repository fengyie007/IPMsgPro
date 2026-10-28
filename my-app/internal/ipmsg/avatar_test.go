package ipmsg

// 对拍 tauri-rust avatar.rs 与 protocol.rs 的头像/MAC 测试用例。

import "testing"

func TestBuiltinCustomAndInvalidProfileFields(t *testing.T) {
	for dialect := 4; dialect <= 6; dialect += 2 {
		for id := 0; id < 84; id++ {
			version := "1_lbt" + string(rune('0'+dialect)) + "_" + itoa(id) + "#128#mac#0#0#0#4001#9"
			kind, got := ParseAvatar(version)
			if kind != AvatarBuiltin || got != id {
				t.Fatalf("dialect=%d id=%d 解析异常: %v %d", dialect, id, kind, got)
			}
		}
	}
	if kind, _ := ParseAvatar("1_lbt6_0#128#mac#123#0#0#4001#9"); kind != AvatarCustom {
		t.Fatalf("自定义头像应识别为 custom")
	}
	if kind, _ := ParseAvatar("1_lbt6_84#128#mac#0#0"); kind != AvatarUnknown {
		t.Fatalf("越界编号应识别为 unknown")
	}
	for _, invalid := range []string{
		"1",
		"1_lbt6_3",
		"1_lbt6_-1#128#mac#0#0",
		"1_lbt6_999999999999#128#mac#0#0",
		"1_lbt6_1#128#mac#bad#0",
		"1_lbt6_1#128#mac#0#",
		"1_lbt99_1#128#mac#0#0",
	} {
		if kind, _ := ParseAvatar(invalid); kind != AvatarNone {
			t.Fatalf("%q 应解析失败", invalid)
		}
	}
	if ValidateAvatarID(83) != nil || ValidateAvatarID(84) == nil || ValidateAvatarID(-1) == nil {
		t.Fatalf("头像编号校验异常")
	}
}

func TestFeiqMacNormalizesValidAndRejectsInvalid(t *testing.T) {
	// 对拍 protocol.rs feiq_mac 测试：02 开头的本机生成 ID 与合法硬件地址。
	if mac := FeiqMac(FeiqCompatVersion("local", 2425, 0)); mac == "" {
		t.Fatalf("本机生成的版本头应能解析出 MAC")
	}
	valid := []struct{ raw, want string }{
		{"02ABCDEF1234", "02-AB-CD-EF-12-34"},
		{"02abcdef1234", "02-AB-CD-EF-12-34"},
		{"02-AB-CD-EF-12-34", "02-AB-CD-EF-12-34"},
	}
	for _, v := range valid {
		version := "1_lbt6_0#128#" + v.raw + "#0#0#0#4001#9"
		if mac := FeiqMac(version); mac != v.want {
			t.Fatalf("%s 归一化异常: %q", v.raw, mac)
		}
	}
	for _, invalid := range []string{
		"not-a-mac-id",
		"03ABCDEF1234",  // 组播位
		"000000000000",  // 全零
		"02ABCDEF123",   // 长度不对
		"02-AB-CD-EF-12", // 分段不全
	} {
		version := "1_lbt6_0#128#" + invalid + "#0#0#0#4001#9"
		if mac := FeiqMac(version); mac != "" {
			t.Fatalf("%q 应拒绝: %q", invalid, mac)
		}
	}
}

func itoa(v int) string {
	if v == 0 {
		return "0"
	}
	var digits []byte
	for v > 0 {
		digits = append([]byte{byte('0' + v%10)}, digits...)
		v /= 10
	}
	return string(digits)
}
