// scan.go 对拍 tauri-rust network/scan.rs：IP 段定向探测扫描。
// 状态机 idle/running/waiting/cancelling/completed/cancelled/failed；
// 进度事件 100ms 限频；scanId 互斥；取消随时打断探测与等待。
package network

import (
	"fmt"
	"net"
	"strconv"
	"sync"
	"sync/atomic"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/store"
)

type scanPhase string

const (
	scanIdle       scanPhase = "idle"
	scanRunning    scanPhase = "running"
	scanWaiting    scanPhase = "waiting"
	scanCancelling scanPhase = "cancelling"
	scanCompleted  scanPhase = "completed"
	scanCancelled  scanPhase = "cancelled"
	scanFailed     scanPhase = "failed"
)

func (p scanPhase) active() bool {
	return p == scanRunning || p == scanWaiting || p == scanCancelling
}

// ScanStatus 对齐前端 types 的 ScanStatus（camelCase）。
type ScanStatus struct {
	ScanID      uint64   `json:"scanId"`
	Revision    uint64   `json:"revision"`
	State       scanPhase `json:"state"`
	Current     uint32   `json:"current"`
	Total       uint32   `json:"total"`
	Found       uint32   `json:"found"`
	FailedSends uint32   `json:"failedSends"`
	Skipped     uint32   `json:"skipped"`
	Port        uint16   `json:"port"`
	DelayMs     uint32   `json:"delayMs"`
	Ranges      []string `json:"ranges"`
	Error       *string  `json:"error"`
}

func defaultScanStatus() ScanStatus {
	return ScanStatus{State: scanIdle, Port: 2425, DelayMs: 20, Ranges: []string{}}
}

type scanData struct {
	status    ScanStatus
	probed    map[uint32]bool
	found     map[string]bool
	lastEvent time.Time
}

type scanner struct {
	mu   sync.Mutex
	data scanData

	controlMu   sync.Mutex
	cancel      chan struct{}
	cancelState bool
	workerDone  chan struct{}
	next        atomic.Uint64
}

func newScanner() *scanner {
	return &scanner{
		data: scanData{
			status: defaultScanStatus(),
			probed: map[uint32]bool{},
			found:  map[string]bool{},
		},
	}
}

// ScanStatus 返回当前扫描状态快照。
func (h *Hub) ScanStatus() ScanStatus {
	return h.scan.snapshot()
}

func (s *scanner) snapshot() ScanStatus {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.data.status
}

// ScanEvent 输出扫描进度/完成事件（network.scan_progress / network.scan_complete）。
type ScanEvent func(event string, payload ScanStatus)

func (h *Hub) scanEvent(force bool) {
	status := func() ScanStatus {
		h.scan.mu.Lock()
		defer h.scan.mu.Unlock()
		if !force && time.Since(h.scan.data.lastEvent) < 100*time.Millisecond {
			return ScanStatus{}
		}
		h.scan.data.lastEvent = time.Now()
		return h.scan.data.status
	}()
	if status.State == "" {
		return // 限频窗口内丢弃
	}
	name := "network.scan_progress"
	if !status.State.active() {
		name = "network.scan_complete"
	}
	if h.ScanEvent != nil {
		h.ScanEvent(name, status)
	}
}

// scanResponse 扫描期间收到的 BR_ENTRY/ANSENTRY 记为发现（对拍 network.rs）。
func (h *Hub) scanResponse(addr *net.UDPAddr) {
	if addr.IP == nil {
		return
	}
	ip4 := addr.IP.To4()
	if ip4 == nil {
		return
	}
	changed := func() bool {
		h.scan.mu.Lock()
		defer h.scan.mu.Unlock()
		if h.scan.data.status.State != scanRunning && h.scan.data.status.State != scanWaiting {
			return false
		}
		if addr.Port != int(h.scan.data.status.Port) || !h.scan.data.probed[be32(ip4)] {
			return false
		}
		key := addr.String()
		if h.scan.data.found[key] {
			return false
		}
		h.scan.data.found[key] = true
		h.scan.data.status.Found = uint32(len(h.scan.data.found))
		h.scan.data.status.Revision++
		return true
	}()
	if changed {
		h.scanEvent(false)
	}
}

func be32(ip net.IP) uint32 {
	return uint32(ip[0])<<24 | uint32(ip[1])<<16 | uint32(ip[2])<<8 | uint32(ip[3])
}

// StartScan 启动一次扫描；skipExisting=true 时已有活动扫描直接返回现状
// （启动期自动扫描用，对拍 start_scan_inner）。
func (h *Hub) StartScan(options store.ScanOptions, skipExisting bool) (ScanStatus, error) {
	plan, err := options.Plan()
	if err != nil {
		return ScanStatus{}, err
	}
	if plan.Total == 0 {
		return ScanStatus{}, fmt.Errorf("请至少配置一个扫描范围")
	}
	h.scan.controlMu.Lock()
	defer h.scan.controlMu.Unlock()
	if h.stopped.Load() {
		return ScanStatus{}, fmt.Errorf("程序正在退出")
	}
	previous := h.ScanStatus()
	if previous.State.active() {
		if skipExisting {
			return previous, nil
		}
		return ScanStatus{}, fmt.Errorf("已有扫描正在进行，请先取消")
	}
	h.waitWorker()

	id := h.scan.next.Add(1)
	rangeTexts := make([]string, 0, len(plan.Ranges))
	for _, r := range plan.Ranges {
		rangeTexts = append(rangeTexts, fmt.Sprintf("%d.%d.%d.%d-%d.%d.%d.%d",
			byte(r[0]>>24), byte(r[0]>>16), byte(r[0]>>8), byte(r[0]),
			byte(r[1]>>24), byte(r[1]>>16), byte(r[1]>>8), byte(r[1])))
	}
	status := ScanStatus{
		ScanID:   id,
		Revision: 1,
		State:    scanRunning,
		Total:    plan.Total,
		Port:     plan.Port,
		DelayMs:  plan.DelayMs,
		Ranges:   rangeTexts,
	}
	h.scan.mu.Lock()
	h.scan.data.status = status
	h.scan.data.probed = map[uint32]bool{}
	h.scan.data.found = map[string]bool{}
	h.scan.mu.Unlock()
	h.scan.cancel = make(chan struct{})
	h.scan.cancelState = false
	h.scanEvent(true)

	cancel := h.scan.cancel
	done := make(chan struct{})
	h.scan.workerDone = done
	go func() {
		defer close(done)
		h.scanWorker(id, plan, cancel)
	}()
	return status, nil
}

func (h *Hub) waitWorker() {
	if h.scan.workerDone != nil {
		select {
		case <-h.scan.workerDone:
		case <-time.After(2 * time.Second):
		}
		h.scan.workerDone = nil
	}
}

func (h *Hub) finishScan(id uint64, phase scanPhase, errorMsg string) {
	{
		h.scan.mu.Lock()
		if h.scan.data.status.ScanID != id || !h.scan.data.status.State.active() {
			h.scan.mu.Unlock()
			return
		}
		if h.scan.data.status.State == scanCancelling {
			h.scan.data.status.State = scanCancelled
		} else {
			h.scan.data.status.State = phase
		}
		if errorMsg != "" {
			text := errorMsg
			h.scan.data.status.Error = &text
		}
		h.scan.data.status.Revision++
		h.scan.mu.Unlock()
	}
	h.scanEvent(true)
}

func (h *Hub) scanWorker(id uint64, plan store.ScanPlan, cancel chan struct{}) {
	lastEvent := time.Now()
	for _, r := range plan.Ranges {
		for ip := r[0]; ; ip++ {
			select {
			case <-cancel:
				h.finishScan(id, scanCancelled, "")
				return
			default:
			}
			if h.stopped.Load() {
				h.finishScan(id, scanCancelled, "")
				return
			}
			localIP := h.localIPString()
			skip := int(h.port) == int(plan.Port) && localIP == ipv4Text(ip)
			{
				h.scan.mu.Lock()
				if h.scan.data.status.ScanID != id {
					h.scan.mu.Unlock()
					return
				}
				h.scan.data.status.Current++
				h.scan.data.status.Revision++
				if skip {
					h.scan.data.status.Skipped++
				} else {
					h.scan.data.probed[ip] = true
				}
				h.scan.mu.Unlock()
			}
			if !skip {
				wire, err := h.buildWire(ipmsg.CmdBrEntry|ipmsg.OptCapUTF8, h.nextPktNo.Add(1), h.Nick(), &h.prof.Load().group)
				if err != nil {
					h.finishScan(id, scanFailed, err.Error())
					return
				}
				target := &net.UDPAddr{IP: net.IPv4(byte(ip>>24), byte(ip>>16), byte(ip>>8), byte(ip)), Port: int(plan.Port)}
				if err := h.sendProbe(target, wire, cancel); err != nil {
					h.scan.mu.Lock()
					h.scan.data.status.FailedSends++
					text := err.Error()
					h.scan.data.status.Error = &text
					h.scan.data.status.Revision++
					h.scan.mu.Unlock()
				}
			}
			if time.Since(lastEvent) >= 100*time.Millisecond {
				h.scanEvent(false)
				lastEvent = time.Now()
			}
			if h.ScanStatus().Current < plan.Total {
				select {
				case <-cancel:
					h.finishScan(id, scanCancelled, "")
					return
				case <-time.After(time.Duration(plan.DelayMs) * time.Millisecond):
				}
			}
			if ip == r[1] {
				break
			}
		}
	}
	select {
	case <-cancel:
		h.finishScan(id, scanCancelled, "")
		return
	default:
	}
	{
		h.scan.mu.Lock()
		if h.scan.data.status.State == scanCancelling {
			h.scan.mu.Unlock()
			h.finishScan(id, scanCancelled, "")
			return
		}
		h.scan.data.status.State = scanWaiting
		h.scan.data.status.Revision++
		h.scan.mu.Unlock()
	}
	h.scanEvent(true)
	select {
	case <-cancel:
		h.finishScan(id, scanCancelled, "")
		return
	case <-time.After(time.Second):
	}
	status := h.ScanStatus()
	failed := status.FailedSends > 0 && status.FailedSends+status.Skipped == status.Current
	phase := scanCompleted
	if failed {
		phase = scanFailed
	}
	h.finishScan(id, phase, "")
}

// sendProbe 单次 UDP 探测：1 秒写超时，取消随时打断。
func (h *Hub) sendProbe(target *net.UDPAddr, wire []byte, cancel chan struct{}) error {
	done := make(chan error, 1)
	go func() {
		_ = h.conn.SetWriteDeadline(time.Now().Add(time.Second))
		_, err := h.conn.WriteToUDP(wire, target)
		done <- err
	}()
	select {
	case err := <-done:
		return err
	case <-cancel:
		return fmt.Errorf("扫描已取消")
	case <-time.After(time.Second):
		return fmt.Errorf("UDP探测发送超时")
	}
}

// CancelScan 取消指定扫描；id 不匹配报错（对拍 stop_scan）。
func (h *Hub) CancelScan(id uint64) (ScanStatus, error) {
	return h.stopScan(&id)
}

// ShutdownScan 退出时停扫（不校验 id）。
func (h *Hub) ShutdownScan() {
	_, _ = h.stopScan(nil)
}

func (h *Hub) stopScan(expected *uint64) (ScanStatus, error) {
	h.scan.controlMu.Lock()
	defer h.scan.controlMu.Unlock()
	status := h.ScanStatus()
	if expected != nil && *expected != status.ScanID {
		return ScanStatus{}, fmt.Errorf("扫描任务已变更，请刷新状态")
	}
	if status.State.active() {
		h.scan.mu.Lock()
		// 完成可能先抢到状态锁。
		if h.scan.data.status.State.active() {
			h.scan.data.status.State = scanCancelling
			h.scan.data.status.Revision++
		}
		h.scan.mu.Unlock()
		if h.scan.cancel != nil && !h.scan.cancelState {
			h.scan.cancelState = true
			close(h.scan.cancel)
		}
		h.scanEvent(true)
	}
	h.waitWorker()
	h.finishScan(status.ScanID, scanCancelled, "")
	return h.ScanStatus(), nil
}

func ipv4Text(ip uint32) string {
	return strconv.Itoa(int(ip>>24)) + "." + strconv.Itoa(int(ip>>16&0xff)) + "." +
		strconv.Itoa(int(ip>>8&0xff)) + "." + strconv.Itoa(int(ip&0xff))
}
