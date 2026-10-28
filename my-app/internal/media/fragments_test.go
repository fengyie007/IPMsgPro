package media

// 分片状态机测试对拍 tauri-rust fragments.rs 的同名用例。

import (
	"bytes"
	"fmt"
	"testing"
	"time"
)

var now = time.Now()

func fragment(id string, data []byte, index uint32) Fragment {
	offset := int(index-1) * fragmentBytes
	return Fragment{
		ImageID: id,
		Total:   len(data),
		Count:   uint32((len(data)-1)/fragmentBytes + 1),
		Index:   index,
		Data:    data[offset:min(int(offset)+fragmentBytes, len(data))],
	}
}

func key(peer, id string) TransferKey { return TransferKey{PeerID: peer, ImageID: id} }

func finalizeAction(action Action, err error) (TransferKey, uint32, []byte, error) {
	if err != nil {
		return TransferKey{}, 0, nil, err
	}
	if action.Kind != ActionFinalize {
		panic(fmt.Sprintf("期望 Finalize，得到 %v", action.Kind))
	}
	return action.Key, action.Index, action.Payload, nil
}

func TestParserPreservesBinaryAndWireIDCase(t *testing.T) {
	packet := []byte("Ab12CD34|5|0|1|1|5|0|1|0|00000000#\x00")
	packet = append(packet, 0, '#', ':', 0xff, 0)
	parsed, err := ParseFragment(packet)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.ImageID != "Ab12CD34" {
		t.Fatalf("wire ID 大小写应保留: %s", parsed.ImageID)
	}
	if !bytes.Equal(parsed.Data, []byte{0, '#', ':', 0xff, 0}) {
		t.Fatalf("二进制数据应原样保留: %v", parsed.Data)
	}
}

func TestParserRejectsBadGeometryAndFraming(t *testing.T) {
	for _, body := range [][]byte{
		[]byte("abc|1|0|1|1|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|0|0|0|1|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|1|1|1|1|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|1|0|1|0|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|1|0|2|1|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|1|0|1|2|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|1|0|1|1|2|0|1|0|00000000#\x00xy"),
		[]byte("abcdef01|1|0|1|1|1|0|1|0|00000000#x"),
		[]byte("abcdef01|1|0|1|1|1|0|1|0|00000000#\x00xy"),
		[]byte("abcdef01|16777217|0|32769|1|512|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|99999999999999999999|0|1|1|1|0|1|0|00000000#\x00x"),
		[]byte("abcdef01|-1|0|1|1|1|0|1|0|00000000#\x00x"),
	} {
		if _, err := ParseFragment(body); err == nil {
			t.Errorf("应拒绝 %q", body)
		}
	}
	huge := append(bytes.Repeat([]byte{'x'}, 257), []byte("#\x00x")...)
	if _, err := ParseFragment(huge); err == nil {
		t.Error("超长头应被拒绝")
	}
}

func TestExactFullAndShortLastFragmentGeometry(t *testing.T) {
	full := append([]byte("abcdef01|512|0|1|1|512|0|2|0|00000000#\x00"), bytes.Repeat([]byte{7}, 512)...)
	parsed, err := ParseFragment(full)
	if err != nil || len(parsed.Data) != 512 {
		t.Fatalf("整片长度异常: %v %v", parsed, err)
	}
	last, err := ParseFragment([]byte("abcdef01|513|512|2|2|1|0|1|0|00000000#\x00x"))
	if err != nil || last.Index != 2 {
		t.Fatalf("短末片异常: %+v %v", last, err)
	}
}

func TestReferenceIsExactAndMayHaveAFontSuffix(t *testing.T) {
	if got := ParseReference("/~#>Ab12CD34<B~"); got != "Ab12CD34" {
		t.Fatalf("引用解析异常: %q", got)
	}
	if got := ParseReference("/~#>abcdef01<B~{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}"); got != "abcdef01" {
		t.Fatalf("字体尾标应被剥离: %q", got)
	}
	for _, text := range []string{
		"prefix/~#>abcdef01<B~",
		"/~#>abcdef01<B~suffix",
		"/~#>abc<B~",
		"/~#>abcdefgh<B~",
		"/~#>abcdef01<B~ ",
		"/~#>abcdef01<B~{/font;broken;}",
	} {
		if got := ParseReference(text); got != "" {
			t.Errorf("%q 应解析为空，得到 %q", text, got)
		}
	}
}

func TestOutOfOrderDuplicatesAndCompletionAck(t *testing.T) {
	a := NewAssembler()
	data := make([]byte, 1100)
	for i := range data {
		data[i] = byte(i % 251)
	}
	k := key("peer", "abcdef01")
	action, err := a.Accept("peer", fragment("abcdef01", data, 3), now)
	if err != nil || action.Kind != ActionAck || action.Index != 3 || action.Key != k {
		t.Fatalf("片 3 应 Ack: %+v %v", action, err)
	}
	action, err = a.Accept("peer", fragment("abcdef01", data, 3), now)
	if err != nil || action.Kind != ActionAck {
		t.Fatalf("重复片应 Ack: %+v %v", action, err)
	}
	if _, err := a.Accept("peer", fragment("abcdef01", data, 1), now); err != nil {
		t.Fatal(err)
	}
	gotKey, held, payload, ferr := finalizeAction(a.Accept("peer", fragment("abcdef01", data, 2), now))
	if ferr != nil {
		t.Fatal(ferr)
	}
	if ferr != nil || gotKey != k || held != 2 || !bytes.Equal(payload, data) {
		t.Fatalf("完成载荷异常: %v %d", gotKey, held)
	}
	if !a.IsFinalizing(k, now) {
		t.Fatal("应处于 finalizing")
	}
	action, _ = a.Accept("peer", fragment("abcdef01", data, 2), now)
	if action.Kind != ActionPending {
		t.Fatal("finalizing 中重复片应 Pending")
	}
	action, err = a.Accept("peer", fragment("abcdef01", data, 1), now)
	if err != nil || action.Kind != ActionAck || action.Index != 1 {
		t.Fatalf("finalizing 中其他片应 Ack: %+v %v", action, err)
	}
	if held, ok := a.Finish(k, true, now); !ok || held != 2 {
		t.Fatalf("finish 应返回 held=2: %d %v", held, ok)
	}
	if a.reservedBytes != 0 {
		t.Fatalf("预留额度应清零: %d", a.reservedBytes)
	}
	if _, ok := a.Finish(k, true, now); ok {
		t.Fatal("重复 finish 应无效")
	}
	action, err = a.Accept("peer", fragment("abcdef01", data, 2), now)
	if err != nil || action.Kind != ActionAck || action.Index != 2 {
		t.Fatalf("完成后重发应 Ack: %+v %v", action, err)
	}
}

func TestSameWireIDFromDifferentPeersIsIndependent(t *testing.T) {
	a := NewAssembler()
	one, _, p, err1 := finalizeAction(a.Accept("peer-a", fragment("abcdef01", []byte("one"), 1), now))
	two, _, q, err2 := finalizeAction(a.Accept("peer-b", fragment("abcdef01", []byte("two"), 1), now))
	if err1 != nil || err2 != nil {
		t.Fatal(err1)
	}

	if one == two {
		t.Fatal("不同对端应独立")
	}
	if string(p) != "one" || string(q) != "two" {
		t.Fatal("载荷串线")
	}
	if _, ok := a.Finish(one, true, now); !ok {
		t.Fatal("第一个应完成")
	}
	if !a.IsFinalizing(two, now) {
		t.Fatal("第二个应仍在 finalizing")
	}
}

func TestDecodeFailureCannotBeReacknowledgedAsANewPartialImage(t *testing.T) {
	a := NewAssembler()
	data := bytes.Repeat([]byte{4}, 513)
	if _, err := a.Accept("p", fragment("abcdef01", data, 1), now); err != nil {
		t.Fatal(err)
	}
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", data, 2), now))
	if _, ok := a.Finish(k, false, now); ok {
		t.Fatal("失败 finish 不应返回 held")
	}
	for _, index := range []uint32{2, 1, 2} {
		action, err := a.Accept("p", fragment("abcdef01", data, index), now)
		if err != nil || action.Kind != ActionPending {
			t.Fatalf("拒收后应 Pending: %+v %v", action, err)
		}
	}
	if _, ok := a.Finish(k, true, now); ok {
		t.Fatal("迟到的成功不应生效")
	}
	if err := a.Reference("p", "abcdef01", now); err == nil {
		t.Fatal("已拒收的引用应报错")
	}
	if a.reservedBytes != 0 {
		t.Fatalf("预留应清零: %d", a.reservedBytes)
	}
}

func TestConflictingChunksAndMetadataAreRejected(t *testing.T) {
	data := bytes.Repeat([]byte{1}, 513)
	a := NewAssembler()
	if _, err := a.Accept("p", fragment("abcdef01", data, 1), now); err != nil {
		t.Fatal(err)
	}
	conflict := fragment("abcdef01", data, 1)
	conflict.Data[0] = 9
	if _, err := a.Accept("p", conflict, now); err == nil {
		t.Fatal("冲突数据应报错")
	}
	action, _ := a.Accept("p", fragment("abcdef01", data, 2), now)
	if action.Kind != ActionPending {
		t.Fatal("拒收后应 Pending")
	}
	b := NewAssembler()
	if _, err := b.Accept("p", fragment("abcdef02", data, 1), now); err != nil {
		t.Fatal(err)
	}
	if _, err := b.Accept("p", fragment("abcdef02", bytes.Repeat([]byte{1}, 514), 2), now); err == nil {
		t.Fatal("几何变化应报错")
	}
	if b.reservedBytes != 0 {
		t.Fatalf("预留应清零: %d", b.reservedBytes)
	}
}

func TestFinalizingConflictRejectsButCompletedConflictDoesNotDestroySuccess(t *testing.T) {
	a := NewAssembler()
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("yes"), 1), now))
	if _, err := a.Accept("p", fragment("abcdef01", []byte("bad"), 1), now); err == nil {
		t.Fatal("finalizing 冲突应报错")
	}
	if _, ok := a.Finish(k, true, now); ok {
		t.Fatal("冲突后 finish 不应成功")
	}
	b := NewAssembler()
	k2, _, _, _ := finalizeAction(b.Accept("p", fragment("abcdef01", []byte("yes"), 1), now))
	b.Finish(k2, true, now)
	if _, err := b.Accept("p", fragment("abcdef01", []byte("bad"), 1), now); err == nil {
		t.Fatal("完成后冲突内容应报错")
	}
	action, err := b.Accept("p", fragment("abcdef01", []byte("yes"), 1), now)
	if err != nil || action.Kind != ActionAck {
		t.Fatalf("完成后原内容重发应 Ack: %+v %v", action, err)
	}
}

func TestReferencesBeforeOrAfterDataNeverRequireAnotherHistoryMessage(t *testing.T) {
	a := NewAssembler()
	if err := a.Reference("p", "abcdef01", now); err != nil {
		t.Fatal(err)
	}
	if err := a.Reference("p", "abcdef01", now.Add(time.Second)); err != nil {
		t.Fatal(err)
	}
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now))
	if err := a.Reference("p", "abcdef01", now); err != nil {
		t.Fatal(err)
	}
	if !a.IsFinalizing(k, now) {
		t.Fatal("引用不应打断 finalizing")
	}
	if held, ok := a.Finish(k, true, now); !ok || held != 1 {
		t.Fatal("finish 应成功")
	}
	if err := a.Reference("p", "abcdef01", now); err != nil {
		t.Fatal(err)
	}
	if len(a.transfers) != 1 {
		t.Fatalf("记录应仍为 1: %d", len(a.transfers))
	}
	action, err := a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now)
	if err != nil || action.Kind != ActionAck {
		t.Fatalf("完成后重发应 Ack: %+v %v", action, err)
	}
}

func TestTimeoutsAreOnceOnlyAndLateFinalizationCannotSucceed(t *testing.T) {
	a := NewAssembler()
	if err := a.Reference("reference", "abcdef01", now); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Accept("collecting", fragment("abcdef01", bytes.Repeat([]byte{0}, 513), 1), now); err != nil {
		t.Fatal(err)
	}
	k, _, _, _ := finalizeAction(a.Accept("finalizing", fragment("abcdef01", []byte("ok"), 1), now))
	if a.IsFinalizing(k, now.Add(finalizingTimeout)) {
		t.Fatal("超时后不应再是 finalizing")
	}
	if got := len(a.Expire(now.Add(finalizingTimeout))); got != 1 {
		t.Fatalf("应有一条超时: %d", got)
	}
	if _, ok := a.Finish(k, true, now.Add(finalizingTimeout)); ok {
		t.Fatal("超时后 finish 不应成功")
	}
	if got := len(a.Expire(now.Add(finalizingTimeout))); got != 0 {
		t.Fatalf("通知应只出现一次: %d", got)
	}
	if got := len(a.Expire(now.Add(activityTimeout))); got != 2 {
		t.Fatalf("另两条应超时: %d", got)
	}
	if a.reservedBytes != 0 {
		t.Fatalf("预留应清零: %d", a.reservedBytes)
	}
	if got := len(a.Expire(now.Add(activityTimeout))); got != 0 {
		t.Fatalf("不应重复通知: %d", got)
	}
}

func TestFinishChecksDeadlineWithoutWaitingForExpireTick(t *testing.T) {
	a := NewAssembler()
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now))
	if _, ok := a.Finish(k, true, now.Add(finalizingTimeout)); ok {
		t.Fatal("超时 finish 应失败")
	}
	action, err := a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now.Add(finalizingTimeout))
	if err != nil || action.Kind != ActionPending {
		t.Fatalf("超时后应 Pending: %+v %v", action, err)
	}
}

func TestActivityDeadlinesDoNotSlideOnDuplicateTraffic(t *testing.T) {
	a := NewAssembler()
	data := bytes.Repeat([]byte{0}, 513)
	if _, err := a.Accept("p", fragment("abcdef01", data, 1), now); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Accept("p", fragment("abcdef01", data, 1), now.Add(119*time.Second)); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Accept("p", fragment("abcdef01", data, 2), now.Add(activityTimeout)); err == nil {
		t.Fatal("超时应报错")
	}
	if a.reservedBytes != 0 {
		t.Fatalf("预留应清零: %d", a.reservedBytes)
	}
}

func TestActiveAndByteLimitsAreCheckedBeforeAllocation(t *testing.T) {
	refs := NewAssembler()
	for n := 0; n < maxActive; n++ {
		if err := refs.Reference("p", fmt.Sprintf("%08x", n), now); err != nil {
			t.Fatal(err)
		}
	}
	if err := refs.Reference("p", "ffffffff", now); err == nil {
		t.Fatal("超过活跃上限应报错")
	}
	large := func(id string) Fragment {
		return Fragment{ImageID: id, Total: MaxPayloadBytes, Count: MaxPayloadBytes / 512, Index: 1, Data: make([]byte, 512)}
	}
	a := NewAssembler()
	if _, err := a.Accept("p", large("00000001"), now); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Accept("p", large("00000002"), now); err != nil {
		t.Fatal(err)
	}
	if a.reservedBytes != maxReservedBytes {
		t.Fatalf("预留应为上限: %d", a.reservedBytes)
	}
	if _, err := a.Accept("p", large("00000003"), now); err == nil {
		t.Fatal("额度不足应报错")
	}
	if len(a.transfers) != 2 {
		t.Fatalf("应只有两条记录: %d", len(a.transfers))
	}
	invalid := Fragment{ImageID: "00000004", Total: int(^uint(0) >> 1), Count: 0xFFFFFFFF, Index: 0xFFFFFFFF, Data: []byte{0}}
	if _, err := a.Accept("p", invalid, now); err == nil {
		t.Fatal("非法几何应报错")
	}
	if err := a.Reference("p", "00000003", now); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Accept("p", large("00000003"), now); err == nil {
		t.Fatal("额度不足应报错")
	}
	failures := a.Expire(now)
	if len(failures) != 1 || failures[0].Key != key("p", "00000003") || failures[0].Reason != "图片接收缓存额度不足" {
		t.Fatalf("失败通知异常: %+v", failures)
	}
	if len(a.Expire(now)) != 0 {
		t.Fatal("不应重复通知")
	}
}

func assertFailureOnce(t *testing.T, a *Assembler, expected TransferKey, at time.Time, reason string) {
	t.Helper()
	failures := a.Expire(at)
	if len(failures) != 1 {
		t.Fatalf("应恰好一条通知: %+v", failures)
	}
	if failures[0].Key != expected {
		t.Fatalf("通知键不匹配: %+v", failures[0])
	}
	if !bytes.Contains([]byte(failures[0].Reason), []byte(reason)) {
		t.Fatalf("原因应包含 %q: %s", reason, failures[0].Reason)
	}
	if len(a.Expire(at)) != 0 {
		t.Fatal("不应重复通知")
	}
}

func TestAcceptConflictsAreReportedOnceWithoutWaitingForTimeout(t *testing.T) {
	data := bytes.Repeat([]byte{1}, 513)
	a := NewAssembler()
	if _, err := a.Accept("p", fragment("abcdef01", data, 1), now); err != nil {
		t.Fatal(err)
	}
	conflict := fragment("abcdef01", data, 1)
	conflict.Data[0] = 9
	if _, err := a.Accept("p", conflict, now); err == nil {
		t.Fatal("冲突应报错")
	}
	action, _ := a.Accept("p", fragment("abcdef01", data, 2), now)
	if action.Kind != ActionPending {
		t.Fatal("拒收后应 Pending")
	}
	assertFailureOnce(t, a, key("p", "abcdef01"), now, "重复数据冲突")
	action, _ = a.Accept("p", fragment("abcdef01", data, 2), now)
	if action.Kind != ActionPending {
		t.Fatal("拒收后应 Pending")
	}
	if len(a.Expire(now)) != 0 {
		t.Fatal("不应重复通知")
	}

	b := NewAssembler()
	if _, err := b.Accept("p", fragment("abcdef02", data, 1), now); err != nil {
		t.Fatal(err)
	}
	if _, err := b.Accept("p", fragment("abcdef02", bytes.Repeat([]byte{1}, 514), 2), now); err == nil {
		t.Fatal("几何变化应报错")
	}
	assertFailureOnce(t, b, key("p", "abcdef02"), now, "总长度或分片总数发生变化")
}

func TestReferenceAndAcceptDetectedTimeoutsRemainObservable(t *testing.T) {
	expired := now.Add(activityTimeout)
	a := NewAssembler()
	if err := a.Reference("p", "abcdef01", now); err != nil {
		t.Fatal(err)
	}
	if err := a.Reference("p", "abcdef01", expired); err == nil {
		t.Fatal("超时引用应报错")
	}
	if err := a.Reference("p", "abcdef01", expired); err == nil {
		t.Fatal("超时引用应报错")
	}
	assertFailureOnce(t, a, key("p", "abcdef01"), expired, "引用等待数据超时")

	b := NewAssembler()
	data := bytes.Repeat([]byte{1}, 513)
	if _, err := b.Accept("p", fragment("abcdef02", data, 1), now); err != nil {
		t.Fatal(err)
	}
	if _, err := b.Accept("p", fragment("abcdef02", data, 2), expired); err == nil {
		t.Fatal("超时应报错")
	}
	assertFailureOnce(t, b, key("p", "abcdef02"), expired, "分片接收超时")
}

func TestFinishFailureIsReportedOnceAndLateSuccessCannotReplaceIt(t *testing.T) {
	a := NewAssembler()
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now))
	if _, ok := a.Finish(k, false, now); ok {
		t.Fatal("失败 finish 不应返回 held")
	}
	if _, ok := a.Finish(k, false, now); ok {
		t.Fatal("重复失败 finish 不应返回 held")
	}
	if _, ok := a.Finish(k, true, now); ok {
		t.Fatal("迟到成功不应生效")
	}
	action, _ := a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now)
	if action.Kind != ActionPending {
		t.Fatal("拒收后应 Pending")
	}
	assertFailureOnce(t, a, k, now, "解码或保存失败")
}

func TestFinalizingConflictReportsFailureButCompletedConflictDoesNot(t *testing.T) {
	a := NewAssembler()
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("yes"), 1), now))
	if _, err := a.Accept("p", fragment("abcdef01", []byte("bad"), 1), now); err == nil {
		t.Fatal("冲突应报错")
	}
	if _, ok := a.Finish(k, true, now); ok {
		t.Fatal("冲突后 finish 不应成功")
	}
	assertFailureOnce(t, a, k, now, "完成处理期间出现冲突分片")

	b := NewAssembler()
	k2, _, _, _ := finalizeAction(b.Accept("p", fragment("abcdef01", []byte("yes"), 1), now))
	if held, ok := b.Finish(k2, true, now); !ok || held != 1 {
		t.Fatal("finish 应成功")
	}
	if _, err := b.Accept("p", fragment("abcdef01", []byte("bad"), 1), now); err == nil {
		t.Fatal("完成后冲突应报错")
	}
	if len(b.Expire(now)) != 0 {
		t.Fatal("完成后的冲突不应产生失败通知")
	}
}

func TestExpireMergesQueuedRejectionsWithNewTimeouts(t *testing.T) {
	a := NewAssembler()
	failed, _, _, ferr0 := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("bad"), 1), now))
	if ferr0 != nil {
		t.Fatal(ferr0)
	}
	a.Finish(failed, false, now)
	if err := a.Reference("p", "abcdef02", now); err != nil {
		t.Fatal(err)
	}
	got := map[TransferKey]string{}
	for _, f := range a.Expire(now.Add(activityTimeout)) {
		got[f.Key] = f.Reason
	}
	if len(got) != 2 {
		t.Fatalf("应两条通知: %+v", got)
	}
	if got[failed] != "图片解码或保存失败" || got[key("p", "abcdef02")] != "图片引用等待数据超时" {
		t.Fatalf("原因不匹配: %+v", got)
	}
	if len(a.Expire(now.Add(activityTimeout))) != 0 {
		t.Fatal("不应重复通知")
	}
}

func TestALateFinishReportsTimeoutNotSuccess(t *testing.T) {
	a := NewAssembler()
	k, _, _, _ := finalizeAction(a.Accept("p", fragment("abcdef01", []byte("ok"), 1), now))
	if _, ok := a.Finish(k, true, now.Add(finalizingTimeout)); ok {
		t.Fatal("超时 finish 不应成功")
	}
	assertFailureOnce(t, a, k, now.Add(finalizingTimeout), "解码或保存超时")
}

func TestUnexpiredTerminalRecordsAreNotEvictedForNewImages(t *testing.T) {
	a := NewAssembler()
	for n := 0; n < maxRecords; n++ {
		id := fmt.Sprintf("%08x", n)
		k, _, _, _ := finalizeAction(a.Accept("p", fragment(id, []byte("x"), 1), now))
		a.Finish(k, false, now)
	}
	if _, err := a.Accept("p", fragment("ffffffff", []byte("x"), 1), now); err == nil {
		t.Fatal("记录满应报错")
	}
	action, _ := a.Accept("p", fragment("00000000", []byte("x"), 1), now)
	if action.Kind != ActionPending {
		t.Fatal("已有失败记录的传输应 Pending")
	}
	if len(a.transfers) != maxRecords || len(a.pendingFailures) != maxRecords {
		t.Fatalf("记录数异常: %d/%d", len(a.transfers), len(a.pendingFailures))
	}
	// 名义上过期的失败也保持钉住，直到有人观察
	if _, err := a.Accept("p", fragment("ffffffff", []byte("x"), 1), now.Add(terminalRetention)); err == nil {
		t.Fatal("钉住记录不应被驱逐")
	}
	if len(a.pendingFailures) != maxRecords {
		t.Fatalf("失败通知应保持: %d", len(a.pendingFailures))
	}
	failures := a.Expire(now.Add(terminalRetention))
	if len(failures) != maxRecords {
		t.Fatalf("应排空全部通知: %d", len(failures))
	}
	if len(a.pendingFailures) != 0 || len(a.Expire(now.Add(terminalRetention))) != 0 || len(a.transfers) != 0 {
		t.Fatal("排空后应清理干净")
	}
	action, err := a.Accept("p", fragment("ffffffff", []byte("x"), 1), now.Add(terminalRetention))
	if err != nil || action.Kind != ActionFinalize {
		t.Fatalf("清理后应可重新接收: %+v %v", action, err)
	}
}
