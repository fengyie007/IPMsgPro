// fragments.go 对应 tauri-rust image/fragments.rs：有界的飞秋图片重组状态机。
// 本模块不做 I/O：调用方发送 accept/finish 返回的 ACK，载荷在受限的工作队列上解码。
package media

import (
	"strconv"
	"strings"
	"time"

	"speedipmsg-go/internal/ipmsg"
)

const (
	fragmentBytes = 512
	// MaxPayloadBytes/MAX_PAYLOAD_BYTES 在 lzw.go 定义
	maxReservedBytes  = 32 * 1024 * 1024
	maxActive         = 4
	maxRecords        = 1024
	maxPeerIDBytes    = 4096
	activityTimeout   = 120 * time.Second
	finalizingTimeout = 25 * time.Second
	terminalRetention = 600 * time.Second
)

type TransferKey struct {
	PeerID  string
	ImageID string
}

type Fragment struct {
	ImageID string
	Total   int
	Count   uint32
	Index   uint32
	Data    []byte
}

type ActionKind int

const (
	ActionAck ActionKind = iota
	ActionFinalize
	ActionPending
)

type Action struct {
	Kind    ActionKind
	Key     TransferKey
	Index   uint32
	Payload []byte
}

func validImageID(id string) bool {
	if len(id) != 8 {
		return false
	}
	for i := 0; i < len(id); i++ {
		c := id[i]
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f' || c >= 'A' && c <= 'F') {
			return false
		}
	}
	return true
}

func decimalField(text string) (int, error) {
	if text == "" || len(text) > 10 || !allDigitsASCII(text) {
		return 0, errFragment("图片分片数字字段无效")
	}
	value, err := strconv.Atoi(text)
	if err != nil {
		return 0, errFragment("图片分片数字溢出")
	}
	return value, nil
}

func allDigitsASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return true
}

type fragmentError string

func (e fragmentError) Error() string { return string(e) }

func errFragment(msg string) error { return fragmentError(msg) }

func validateGeometry(total, count, index, length int) (int, error) {
	if total == 0 || total > MaxPayloadBytes {
		return 0, errFragment("图片传输载荷为空或超过16MiB")
	}
	expectedCount := (total-1)/fragmentBytes + 1
	if count != expectedCount || index == 0 || index > count {
		return 0, errFragment("图片分片总数或片号无效")
	}
	offset := (index - 1) * fragmentBytes
	if length != min(fragmentBytes, total-offset) {
		return 0, errFragment("图片分片长度与位置不匹配")
	}
	return offset, nil
}

// ParseFragment 只解析有界的文本头；#\0 之后的每个字节都是数据，
// 包括后续的 NUL、冒号、# 或非 UTF-8 字节。
func ParseFragment(body []byte) (Fragment, error) {
	hash := -1
	for i, b := range body {
		if b == '#' {
			hash = i
			break
		}
	}
	if hash < 0 {
		return Fragment{}, errFragment("图片分片缺少#分隔符")
	}
	if hash == 0 || hash > 256 || hash+1 >= len(body) || body[hash+1] != 0 {
		return Fragment{}, errFragment("图片分片头或二进制分隔符无效")
	}
	header := string(body[:hash])
	for i := 0; i < len(header); i++ {
		if header[i] < 0x20 || header[i] > 0x7e {
			return Fragment{}, errFragment("图片分片头不是ASCII文本")
		}
	}
	fields := strings.Split(header, "|")
	if len(fields) != 10 || !validImageID(fields[0]) {
		return Fragment{}, errFragment("图片ID或分片头字段数量无效")
	}
	total, err := decimalField(fields[1])
	if err != nil {
		return Fragment{}, err
	}
	offsetField, err := decimalField(fields[2])
	if err != nil {
		return Fragment{}, err
	}
	count, err := decimalField(fields[3])
	if err != nil {
		return Fragment{}, err
	}
	index, err := decimalField(fields[4])
	if err != nil {
		return Fragment{}, err
	}
	length, err := decimalField(fields[5])
	if err != nil {
		return Fragment{}, err
	}
	for _, value := range fields[6:9] {
		reserved, err := decimalField(value)
		if err != nil || reserved < 0 || reserved > 0xFFFFFFFF {
			return Fragment{}, errFragment("图片保留字段溢出")
		}
	}
	if !validImageID(fields[9]) {
		return Fragment{}, errFragment("图片尾部保留字段无效")
	}
	geometryOffset, err := validateGeometry(total, count, index, length)
	if err != nil {
		return Fragment{}, err
	}
	if geometryOffset != offsetField {
		return Fragment{}, errFragment("图片分片偏移与片号不匹配")
	}
	data := body[hash+2:]
	if len(data) != length {
		return Fragment{}, errFragment("图片分片实际数据长度不匹配")
	}
	return Fragment{ImageID: fields[0], Total: total, Count: uint32(count), Index: uint32(index), Data: data}, nil
}

// ParseReference 识别一条完整的图片引用（可带飞秋已验证的字体尾标）；
// 引用周围的文本不会被静默丢弃（返回空表示不是纯引用）。
func ParseReference(body string) string {
	if len(body) > 1024 {
		return ""
	}
	plain := ipmsg.StripFeiqFontSuffix(body)
	after, ok := strings.CutPrefix(plain, "/~#>")
	if !ok {
		return ""
	}
	id, ok := strings.CutSuffix(after, "<B~")
	if !ok || !validImageID(id) {
		return ""
	}
	return id
}

type geometry struct {
	total int
	count uint32
}

type stateKind int

const (
	stateReferenceOnly stateKind = iota
	stateCollecting
	stateFinalizing
	stateCompleted
	stateRejected
)

type transfer struct {
	state        stateKind
	payload      []byte
	present      []bool
	fingerprints []uint32
	received     int
	heldIndex    uint32
	geo          *geometry
	deadline     time.Time
	reserved     int
}

func (t *transfer) active() bool {
	return t.state == stateReferenceOnly || t.state == stateCollecting || t.state == stateFinalizing
}

func (t *transfer) timeoutMessage() string {
	switch t.state {
	case stateReferenceOnly:
		return "图片引用等待数据超时"
	case stateCollecting:
		return "图片分片接收超时"
	case stateFinalizing:
		return "图片解码或保存超时"
	default:
		return "图片传输已结束"
	}
}

// Assembler 是有界的重组器；所有方法以调用方传入的 now 为时钟，可离线测试。
type Assembler struct {
	transfers       map[TransferKey]*transfer
	reservedBytes   int
	pendingFailures map[TransferKey]string
}

func NewAssembler() *Assembler {
	return &Assembler{
		transfers:       map[TransferKey]*transfer{},
		pendingFailures: map[TransferKey]string{},
	}
}

func makeKey(peerID, imageID string) (TransferKey, error) {
	if peerID == "" || len(peerID) > maxPeerIDBytes || !validImageID(imageID) {
		return TransferKey{}, errFragment("图片发送方或ID无效")
	}
	return TransferKey{PeerID: peerID, ImageID: imageID}, nil
}

func (a *Assembler) pruneTerminal(now time.Time) {
	for key, t := range a.transfers {
		_, pending := a.pendingFailures[key]
		if !t.active() && !t.deadline.After(now) && !pending {
			delete(a.transfers, key)
		}
	}
}

func (a *Assembler) canAdmit() error {
	if len(a.transfers) >= maxRecords {
		return errFragment("图片接收记录已达上限，请稍后重试")
	}
	active := 0
	for _, t := range a.transfers {
		if t.active() {
			active++
		}
	}
	if active >= maxActive {
		return errFragment("同时接收的图片过多")
	}
	return nil
}

func (a *Assembler) reject(key TransferKey, t *transfer, now time.Time, reason string) {
	if !t.active() {
		return
	}
	a.reservedBytes -= t.reserved
	t.reserved = 0
	t.state = stateRejected
	t.deadline = now.Add(terminalRetention)
	if _, exists := a.pendingFailures[key]; !exists {
		a.pendingFailures[key] = reason
	}
}

func (a *Assembler) Reference(peerID, imageID string, now time.Time) error {
	key, err := makeKey(peerID, imageID)
	if err != nil {
		return err
	}
	a.pruneTerminal(now)
	t, exists := a.transfers[key]
	if exists {
		var result error
		if t.active() && !t.deadline.After(now) {
			reason := t.timeoutMessage()
			a.reject(key, t, now, reason)
			result = errFragment(reason)
		} else if t.state == stateRejected {
			result = errFragment("该图片已拒收")
		}
		// 重复引用或完成后的引用不延长时限、不产生新记录
		return result
	}
	if err := a.canAdmit(); err != nil {
		return err
	}
	a.transfers[key] = &transfer{state: stateReferenceOnly, deadline: now.Add(activityTimeout)}
	return nil
}

func (a *Assembler) Accept(peerID string, fragment Fragment, now time.Time) (Action, error) {
	key, err := makeKey(peerID, fragment.ImageID)
	if err != nil {
		return Action{}, err
	}
	if _, err := validateGeometry(fragment.Total, int(fragment.Count), int(fragment.Index), len(fragment.Data)); err != nil {
		return Action{}, err
	}
	a.pruneTerminal(now)
	t, exists := a.transfers[key]
	if !exists {
		if err := a.canAdmit(); err != nil {
			return Action{}, err
		}
		// 准入即按声明总量预留额度；稀疏流无法超出预算
		if fragment.Total > maxReservedBytes-a.reservedBytes {
			return Action{}, errFragment("图片接收缓存额度不足")
		}
		t = &transfer{state: stateReferenceOnly, deadline: now.Add(activityTimeout)}
		a.transfers[key] = t
	}
	return a.acceptExisting(key, t, fragment, now)
}

func (a *Assembler) acceptExisting(key TransferKey, t *transfer, fragment Fragment, now time.Time) (Action, error) {
	if t.state == stateRejected {
		return Action{Kind: ActionPending, Key: key}, nil
	}
	if t.active() && !t.deadline.After(now) {
		reason := t.timeoutMessage()
		a.reject(key, t, now, reason)
		return Action{}, errFragment(reason)
	}
	geo := geometry{total: fragment.Total, count: fragment.Count}
	if t.geo != nil {
		if *t.geo != geo {
			if t.state != stateCompleted {
				a.reject(key, t, now, "同一图片的总长度或分片总数发生变化")
			}
			return Action{}, errFragment("同一图片的总长度或分片总数发生变化")
		}
	} else {
		if fragment.Total > maxReservedBytes-a.reservedBytes {
			a.reject(key, t, now, "图片接收缓存额度不足")
			return Action{}, errFragment("图片接收缓存额度不足")
		}
		a.reservedBytes += fragment.Total
		t.reserved = fragment.Total
		t.geo = &geo
		t.deadline = now.Add(activityTimeout)
		t.state = stateCollecting
		t.payload = make([]byte, fragment.Total)
		t.present = make([]bool, fragment.Count)
		t.fingerprints = make([]uint32, fragment.Count)
		t.received = 0
	}

	index := fragment.Index
	slot := int(index) - 1
	switch t.state {
	case stateCollecting:
		offset := slot * fragmentBytes
		end := offset + len(fragment.Data)
		if t.present[slot] {
			if !bytesEqual(t.payload[offset:end], fragment.Data) {
				a.reject(key, t, now, "同片号重复数据冲突，图片已拒收")
				return Action{}, errFragment("同片号重复数据冲突，图片已拒收")
			}
			return Action{Kind: ActionAck, Key: key, Index: index}, nil
		}
		copy(t.payload[offset:end], fragment.Data)
		t.fingerprints[slot] = CRC32(fragment.Data)
		t.present[slot] = true
		t.received++
		if t.received < int(geo.count) {
			return Action{Kind: ActionAck, Key: key, Index: index}, nil
		}
		payload := t.payload
		t.payload = nil
		t.present = nil
		t.state = stateFinalizing
		t.heldIndex = index
		t.deadline = now.Add(finalizingTimeout)
		return Action{Kind: ActionFinalize, Key: key, Index: index, Payload: payload}, nil
	case stateFinalizing:
		if t.fingerprints[slot] != CRC32(fragment.Data) {
			a.reject(key, t, now, "完成处理期间出现冲突分片，图片已拒收")
			return Action{}, errFragment("完成处理期间出现冲突分片，图片已拒收")
		}
		if index == t.heldIndex {
			return Action{Kind: ActionPending, Key: key}, nil
		}
		return Action{Kind: ActionAck, Key: key, Index: index}, nil
	case stateCompleted:
		if t.fingerprints[slot] != CRC32(fragment.Data) {
			return Action{}, errFragment("已完成图片的重发分片内容不一致")
		}
		return Action{Kind: ActionAck, Key: key, Index: index}, nil
	default: // stateRejected
		return Action{Kind: ActionPending, Key: key}, nil
	}
}

func (a *Assembler) IsFinalizing(key TransferKey, now time.Time) bool {
	t, ok := a.transfers[key]
	return ok && t.state == stateFinalizing && now.Before(t.deadline)
}

// Finish 在调用方解码、保存并提交图片后才释放完成分片；
// 迟到/重复回调不会复活拒收。
func (a *Assembler) Finish(key TransferKey, success bool, now time.Time) (uint32, bool) {
	t, ok := a.transfers[key]
	if !ok {
		return 0, false
	}
	var result uint32
	held := false
	if t.state == stateFinalizing {
		if success && now.Before(t.deadline) {
			a.reservedBytes -= t.reserved
			t.reserved = 0
			t.payload = nil
			t.present = nil
			t.state = stateCompleted
			t.deadline = now.Add(terminalRetention)
			result, held = t.heldIndex, true
		} else {
			reason := "图片解码或保存失败"
			if !now.Before(t.deadline) {
				reason = "图片解码或保存超时"
			}
			a.reject(key, t, now, reason)
		}
	}
	return result, held
}

// FailureReport 是一次待通知的接收失败。
type FailureReport struct {
	Key    TransferKey
	Reason string
}

// Expire 由宿主定期调用；一次性排空每条拒收/超时通知
// （包括 accept/reference/finish 提前发现并写入 pendingFailures 的失败）。
func (a *Assembler) Expire(now time.Time) []FailureReport {
	a.pruneTerminal(now)
	var expired []TransferKey
	for key, t := range a.transfers {
		if t.active() && !t.deadline.After(now) {
			expired = append(expired, key)
		}
	}
	for _, key := range expired {
		if t, ok := a.transfers[key]; ok {
			a.reject(key, t, now, t.timeoutMessage())
		}
	}
	failures := make([]FailureReport, 0, len(a.pendingFailures))
	for key, reason := range a.pendingFailures {
		failures = append(failures, FailureReport{key, reason})
	}
	a.pendingFailures = map[TransferKey]string{}
	// 过期的终态记录只保留到通知被取走为止
	a.pruneTerminal(now)
	return failures
}

func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
