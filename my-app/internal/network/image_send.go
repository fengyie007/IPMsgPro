// image_send.go 对应 tauri-rust network/image_send.rs：有界发送队列、
// 分片窗口确认、全部确认后才发引用、引用重试同包号、取消与超时。
package network

import (
	"bytes"
	"fmt"
	"log"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/media"
	"my-app/internal/store"
)

const (
	imageSendMaxQueued = 4
	imageSendOverall   = 120 * time.Second
	fragmentSizeConst  = 512
)

type ImageSendResult struct {
	MessageID string              `json:"messageId"`
	ImageID   string              `json:"imageId"`
	Image     media.ImageMetadata `json:"image"`
}

type imageSendJob struct {
	result   ImageSendResult
	peer     User
	peerKey  string
	deadline time.Time

	mu                sync.Mutex
	sent, acked       []bool
	acknowledged      int
	referencePacket   uint32
	referenceSent     bool
	referenceAck      bool
	cancelled, sealed bool
	done              bool
	lastProgress      string
	lastEmit          time.Time
}

func (j *imageSendJob) check(now time.Time) error {
	j.mu.Lock()
	defer j.mu.Unlock()
	if j.cancelled {
		return fmt.Errorf("图片发送已取消")
	}
	if j.sealed || j.done {
		return fmt.Errorf("图片发送已经结束")
	}
	if now.After(j.deadline) || now.Equal(j.deadline) {
		return fmt.Errorf("图片发送超过120秒期限")
	}
	return nil
}

func (j *imageSendJob) cancel() bool {
	j.mu.Lock()
	defer j.mu.Unlock()
	if j.sealed || j.done {
		return false
	}
	j.cancelled = true
	return true
}

func (j *imageSendJob) shouldFinish(now time.Time) bool {
	j.mu.Lock()
	defer j.mu.Unlock()
	return !j.sealed && (j.cancelled || !now.Before(j.deadline))
}

// ImportImagePath 导入本地图片文件为待发送预览。
func (h *Hub) ImportImagePath(path string) (media.ImageMetadata, error) {
	info, err := os.Stat(path)
	if err != nil {
		return media.ImageMetadata{}, err
	}
	if !info.Mode().IsRegular() || info.Size() == 0 || info.Size() > media.MaxImportBytes {
		return media.ImageMetadata{}, fmt.Errorf("仅支持不超过20 MiB的图片文件")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return media.ImageMetadata{}, err
	}
	name := filepath.Base(strings.TrimSuffix(path, filepath.Ext(path)) + ".png")
	return h.importImageSource(data, name)
}

// ImportClipboard 导入粘贴的图片字节（上层已做 20 MiB 上限检查）。
func (h *Hub) ImportClipboard(data []byte) (media.ImageMetadata, error) {
	if len(data) == 0 || len(data) > media.MaxImportBytes {
		return media.ImageMetadata{}, fmt.Errorf("剪贴板图片为空或超过20 MiB")
	}
	return h.importImageSource(data, fmt.Sprintf("粘贴图片_%d.png", time.Now().Unix()))
}

// ImportScreenshot 导入编辑器产出的 PNG（M3 接入；不暴露文件系统路径）。
func (h *Hub) ImportScreenshot(png []byte) (media.ImageMetadata, error) {
	if len(png) > media.MaxImportBytes || !strings.HasPrefix(string(png), "\x89PNG\r\n\x1a\n") {
		return media.ImageMetadata{}, fmt.Errorf("截图必须是20 MiB以内的PNG")
	}
	return h.importImageSource(png, fmt.Sprintf("截图_%d.png", time.Now().Unix()))
}

func (h *Hub) importImageSource(data []byte, name string) (media.ImageMetadata, error) {
	if h.assets == nil || h.db == nil {
		return media.ImageMetadata{}, fmt.Errorf("图片服务未启用")
	}
	h.cleanImageImports(false)
	decoded, err := media.ImportImage(data)
	if err != nil {
		return media.ImageMetadata{}, err
	}
	pending, err := h.assets.Persist(decoded, "import")
	if err != nil {
		return media.ImageMetadata{}, err
	}
	pending.Metadata.FileName = name
	metadata := pending.Metadata
	if err := h.db.RegisterImport(metadata); err != nil {
		pending.Release()
		return media.ImageMetadata{}, err
	}
	pending.Keep()
	return metadata, nil
}

func (h *Hub) DiscardImage(id string) (bool, error) {
	if !media.ValidAssetID(id) {
		return false, fmt.Errorf("无效图片标识")
	}
	ids, err := h.db.DiscardImports(&id, int64(^uint64(0)>>1))
	if err != nil {
		return false, err
	}
	if err := h.removeImportFiles(ids); err != nil {
		return false, err
	}
	return len(ids) > 0, nil
}

func (h *Hub) removeImportFiles(ids []string) error {
	for _, id := range ids {
		if err := h.assets.RemoveUnreferenced(id); err != nil {
			return err
		}
		if err := h.db.ForgetDiscardedAsset(id); err != nil {
			return err
		}
	}
	return nil
}

// cleanImageImports 清理过期（或全部）的未引用预览。
func (h *Hub) cleanImageImports(all bool) {
	cutoff := time.Now().Unix() - 600
	if all {
		cutoff = int64(^uint64(0) >> 1)
	}
	ids, err := h.db.DiscardImports(nil, cutoff)
	if err != nil {
		log.Printf("预览清理失败: %v", err)
		return
	}
	if err := h.removeImportFiles(ids); err != nil {
		log.Printf("预览清理失败，将重试：%v", err)
	}
}

// SendImage 登记发送任务：先建消息（status 0）再入队；全部分片确认后才发引用。
func (h *Hub) SendImage(target, assetID string) (ImageSendResult, error) {
	h.imageSendGate.Lock()
	defer h.imageSendGate.Unlock()
	if h.assets == nil || h.db == nil {
		return ImageSendResult{}, fmt.Errorf("图片服务未启用")
	}
	h.imageSendsMu.Lock()
	if len(h.imageSends) >= imageSendMaxQueued {
		h.imageSendsMu.Unlock()
		return ImageSendResult{}, fmt.Errorf("图片发送队列已满，请稍后重试")
	}
	h.imageSendsMu.Unlock()

	var peer *User
	h.mu.Lock()
	for _, u := range h.users {
		if u.ID == target {
			peer = u
			break
		}
	}
	h.mu.Unlock()
	if peer == nil || !peer.Online {
		return ImageSendResult{}, fmt.Errorf("请先发现目标用户")
	}
	image, err := h.db.ImageAsset(assetID)
	if err != nil {
		return ImageSendResult{}, err
	}
	if image == nil {
		return ImageSendResult{}, fmt.Errorf("图片未登记或不存在")
	}
	imageID := fmt.Sprintf("%08x", h.imageSequence.Add(1))
	messageID := fmt.Sprintf("image-tx-%d-%d-%s", time.Now().Unix(), h.nextPktNo.Add(1), imageID)
	if err := h.db.InsertSentImage(store.Record{
		ID: messageID, FromID: h.LocalID(), ToID: target,
		Content: "[图片]", Kind: 1, Timestamp: time.Now().Unix(), Status: 0,
	}, *image); err != nil {
		return ImageSendResult{}, err
	}
	result := ImageSendResult{MessageID: messageID, ImageID: imageID, Image: *image}
	job := &imageSendJob{
		result:   result,
		peer:     *peer,
		peerKey:  peer.Key,
		deadline: time.Now().Add(imageSendOverall),
	}
	h.imageSendsMu.Lock()
	h.imageSends[messageID] = job
	h.imageSendsMu.Unlock()
	go h.transmitImage(job)
	return result, nil
}

// CancelImage 取消一个在途发送；已结束的任务返回 false。
func (h *Hub) CancelImage(messageID string) bool {
	h.imageSendsMu.Lock()
	job, ok := h.imageSends[messageID]
	h.imageSendsMu.Unlock()
	if !ok {
		return false
	}
	return job.cancel()
}

// interruptImageSends 退出时把所有在途发送置失败。
func (h *Hub) interruptImageSends() {
	h.imageSendsMu.Lock()
	jobs := make([]*imageSendJob, 0, len(h.imageSends))
	for _, job := range h.imageSends {
		jobs = append(jobs, job)
	}
	h.imageSends = map[string]*imageSendJob{}
	h.imageSendsMu.Unlock()
	for _, job := range jobs {
		_ = h.db.InterruptSending(job.result.MessageID)
	}
}

func (h *Hub) imageProgress(job *imageSendJob, progress int, stage string) {
	job.mu.Lock()
	if job.done || job.sealed {
		job.mu.Unlock()
		return
	}
	key := strconv.Itoa(progress) + ":" + stage
	if job.lastProgress == key {
		job.mu.Unlock()
		return
	}
	if strings.HasSuffix(job.lastProgress, ":"+stage) && time.Since(job.lastEmit) < 100*time.Millisecond {
		job.mu.Unlock()
		return
	}
	job.lastProgress = key
	job.lastEmit = time.Now()
	job.mu.Unlock()
	if h.ImageSendProgress != nil {
		h.ImageSendProgress(job.result.MessageID, job.peer.ID, progress, stage, job.result.Image)
	}
}

func (h *Hub) finishImageSend(job *imageSendJob, err error) {
	job.mu.Lock()
	if job.sealed || job.done {
		job.mu.Unlock()
		return
	}
	job.sealed = true
	cancelled := job.cancelled
	if err == nil && !time.Now().Before(job.deadline) {
		err = fmt.Errorf("图片发送超过120秒期限")
	}
	if cancelled {
		err = fmt.Errorf("图片发送已取消")
	}
	job.mu.Unlock()
	status := int64(1)
	if err != nil {
		status = 3
	}
	if dbErr := h.db.Status(job.result.MessageID, status); dbErr != nil {
		err = fmt.Errorf("图片状态保存失败：%w", dbErr)
	}
	job.mu.Lock()
	job.done = true
	job.mu.Unlock()
	if h.ImageSendFinished != nil {
		h.ImageSendFinished(job.result.MessageID, job.peer.ID, status == 1, err, cancelled)
	}
	h.imageSendsMu.Lock()
	delete(h.imageSends, job.result.MessageID)
	h.imageSendsMu.Unlock()
}

// waitImage 轮询等待 ready；返回 false 表示到达 until。
func (h *Hub) waitImage(job *imageSendJob, until time.Time, ready func(*imageSendJob) bool) (bool, error) {
	for {
		if err := job.check(time.Now()); err != nil {
			return false, err
		}
		if ready(job) {
			return true, nil
		}
		if !time.Now().Before(until) {
			return false, nil
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func (h *Hub) transmitImage(job *imageSendJob) {
	log.Printf("图片发送开始: message=%s image=%s target=%s", job.result.MessageID, job.result.Image.AssetID, job.peer.Key)
	overall := job.deadline
	if err := job.check(time.Now()); err != nil {
		h.finishImageSend(job, err)
		return
	}
	h.imageProgress(job, 0, "encoding")
	fullPNG, err := h.assets.Read(job.result.Image.AssetID, false)
	if err != nil {
		log.Printf("图片资产读取失败: %v", err)
	}
	if err == nil {
		var payload []byte
		payload, err = media.EncodePNGForWire(fullPNG)
		if err != nil {
			log.Printf("图片编码失败: %v", err)
		}
		if err == nil {
			err = job.check(time.Now())
			if err == nil {
				err = h.transmitPayload(job, payload, overall)
			}
		}
	}
	if err != nil {
		log.Printf("图片发送结束(失败): %v", err)
	}
	h.finishImageSend(job, err)
}

func (h *Hub) transmitPayload(job *imageSendJob, payload []byte, overall time.Time) error {
	log.Printf("transmitPayload 进入: payload=%d", len(payload))
	count := (len(payload) + fragmentSizeConst - 1) / fragmentSizeConst
	job.mu.Lock()
	job.sent = make([]bool, count)
	job.acked = make([]bool, count)
	job.mu.Unlock()
	h.imageProgress(job, 0, "transferring")

	addr, err := net.ResolveUDPAddr("udp4", job.peerKey)
	if err != nil {
		return err
	}
	for base := 0; base < count; base += 32 {
		end := base + 32
		if end > count {
			end = count
		}
		finalWindow := end == count
		rounds, intervalSecs, windowSecs := 4, 1, 4
		if finalWindow {
			rounds, intervalSecs, windowSecs = 15, 2, 30
		}
		windowDeadline := time.Now().Add(time.Duration(windowSecs) * time.Second)
		if windowDeadline.After(overall) {
			windowDeadline = overall
		}
		complete := false
		for round := 0; round < rounds; round++ {
			for index := base; index < end; index++ {
				if err := job.check(time.Now()); err != nil {
					return err
				}
				job.mu.Lock()
				if job.acked[index] {
					job.mu.Unlock()
					continue
				}
				job.sent[index] = true
				job.mu.Unlock()
				offset := index * fragmentSizeConst
				length := len(payload) - offset
				if length > fragmentSizeConst {
					length = fragmentSizeConst
				}
				body := fmt.Sprintf("%s|%d|%d|%d|%d|%d|0|1|0|00000000#",
					job.result.ImageID, len(payload), offset, count, index+1, length)
				wire, err := h.buildImageWire(payload[offset:offset+length], body)
				if err != nil {
					return err
				}
				if _, err := h.writeRaw(addr, wire); err != nil {
					return err
				}
				time.Sleep(2 * time.Millisecond)
			}
			ok, err := h.waitImage(job, minTime(time.Now().Add(time.Duration(intervalSecs)*time.Second), windowDeadline),
				func(job *imageSendJob) bool {
					job.mu.Lock()
					defer job.mu.Unlock()
					for _, v := range job.acked[base:end] {
						if !v {
							return false
						}
					}
					return true
				})
			if err != nil {
				return err
			}
			complete = ok
			job.mu.Lock()
			progress := job.acknowledged * 100 / count
			job.mu.Unlock()
			if progress > 99 {
				progress = 99
			}
			h.imageProgress(job, progress, "transferring")
			if complete || !time.Now().Before(windowDeadline) {
				break
			}
		}
		if !complete {
			return fmt.Errorf("图片分片确认超时，可尝试通过原版文件通道发送")
		}
	}
	// 全部数据确认后才发聊天引用
	h.imageProgress(job, 99, "waiting_reference")
	number := h.nextPktNo.Add(1)
	job.mu.Lock()
	job.referencePacket = number
	job.referenceSent = true
	job.mu.Unlock()
	reference := fmt.Sprintf("/~#>%s<B~{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}", job.result.ImageID)
	extra := ""
	wire, err := ipmsg.EncodePacket(h.WireVersion(), number, h.Nick(), h.Hostname(),
		ipmsg.CmdSendMsg|ipmsg.OptSendCheck, reference, &extra)
	if err != nil {
		return err
	}
	for attempt := 0; attempt < 4; attempt++ {
		if err := job.check(time.Now()); err != nil {
			return err
		}
		if _, err := h.writeRaw(addr, wire); err != nil {
			return err
		}
		ok, err := h.waitImage(job, minTime(time.Now().Add(time.Second), overall),
			func(job *imageSendJob) bool {
				job.mu.Lock()
				defer job.mu.Unlock()
				return job.referenceAck
			})
		if err != nil {
			return err
		}
		if ok {
			return nil
		}
	}
	return fmt.Errorf("图片已传输，但图片引用未确认")
}

// buildImageWire 编码一条图片分片报文（固定包号无重试需求，分片由窗口重发）。
func (h *Hub) buildImageWire(fragment []byte, header string) ([]byte, error) {
	pktNo := h.nextPktNo.Add(1)
	p := h.prof.Load()
	encUser := gbkOrRaw(0, p.nick)
	encHost := gbkOrRaw(0, p.hostname)
	var data bytes.Buffer
	fmt.Fprintf(&data, "%s:%d:", p.wireVer, pktNo)
	data.Write(encUser)
	data.WriteByte(':')
	data.Write(encHost)
	fmt.Fprintf(&data, ":%d:", ipmsg.CmdSendImage|ipmsg.OptFileAttach)
	data.WriteString(header)
	data.WriteByte(0)
	data.Write(fragment)
	return data.Bytes(), nil
}

// writeRaw 原样发送一段已编码报文。
func (h *Hub) writeRaw(addr *net.UDPAddr, wire []byte) (int, error) {
	return h.conn.WriteToUDP(wire, addr)
}

// imageSendAck 校验并处理图片链路的回执（RECVMSG 引用回执 / REPORT_RECVIMAGE 分片确认）。
// 返回该报文是否被某个发送任务认领。
func (h *Hub) imageSendAck(pkt ipmsg.Packet, key string) bool {
	valid := false
	h.imageSendsMu.Lock()
	jobs := make([]*imageSendJob, 0, len(h.imageSends))
	for _, job := range h.imageSends {
		jobs = append(jobs, job)
	}
	h.imageSendsMu.Unlock()
	for _, job := range jobs {
		// 严格来源匹配：用户名/主机名/IP/端口
		if job.peer.Nick != pkt.Username || job.peer.Host != pkt.Hostname || job.peer.Key != key {
			continue
		}
		job.mu.Lock()
		if job.sealed || job.done {
			job.mu.Unlock()
			continue
		}
		switch ipmsg.CommandMode(pkt.Command) {
		case ipmsg.CmdRecvMsg:
			number, err := strconv.ParseUint(strings.TrimSpace(pkt.Body), 10, 32)
			if err == nil && job.referenceSent && uint32(number) == job.referencePacket {
				job.referenceAck = true
				valid = true
			}
		case ipmsg.CmdReportImage:
			body := pkt.Body
			if !strings.HasSuffix(body, "#") {
				job.mu.Unlock()
				continue
			}
			id, part, ok := strings.Cut(strings.TrimSuffix(body, "#"), "|")
			if !ok || id != job.result.ImageID || part == "" || !ipmsg.AllDigitsASCII(part) {
				job.mu.Unlock()
				continue
			}
			index, err := strconv.Atoi(part)
			if err != nil || index <= 0 || index > len(job.sent) || !job.sent[index-1] {
				job.mu.Unlock()
				continue
			}
			valid = true
			if !job.acked[index-1] {
				job.acked[index-1] = true
				job.acknowledged++
			}
		}
		job.mu.Unlock()
	}
	return valid
}

// expireImageSends 完成已取消/超时的发送任务。
func (h *Hub) expireImageSends() {
	h.imageSendsMu.Lock()
	jobs := make([]*imageSendJob, 0, len(h.imageSends))
	for _, job := range h.imageSends {
		jobs = append(jobs, job)
	}
	h.imageSendsMu.Unlock()
	for _, job := range jobs {
		if job.shouldFinish(time.Now()) {
			h.finishImageSend(job, fmt.Errorf("图片发送已取消或超时"))
		}
	}
}

func minTime(a, b time.Time) time.Time {
	if a.Before(b) {
		return a
	}
	return b
}
