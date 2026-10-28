// image.go 对应 tauri-rust network/image_receive.rs：图片分片接收、
// 单并发解码工作协程、资产落盘与数据库提交、ACK 与失败通知。
package network

import (
	"fmt"
	"log"
	"net"
	"time"

	"my-app/internal/ipmsg"
	"my-app/internal/media"
	"my-app/internal/store"
)

type imageJob struct {
	key      media.TransferKey
	peer     User
	payload  []byte
	deadline time.Time
}

// ImageFailed / ImageReceived 是图片链路的前端事件回调。
// Hub 持有 assets（可选；未附加时不启用收图）。

func (h *Hub) AttachAssets(assets *media.AssetStore) {
	h.assets = assets
	h.images = media.NewAssembler()
}

func (h *Hub) ImageRead(id string, thumbnail bool) (media.ImageMetadata, []byte, error) {
	if !media.ValidAssetID(id) {
		return media.ImageMetadata{}, nil, fmt.Errorf("无效图片标识")
	}
	if h.db == nil || h.assets == nil {
		return media.ImageMetadata{}, nil, fmt.Errorf("图片服务未启用")
	}
	metadata, err := h.db.ImageAsset(id)
	if err != nil {
		return media.ImageMetadata{}, nil, err
	}
	if metadata == nil {
		return media.ImageMetadata{}, nil, fmt.Errorf("图片未登记或不存在")
	}
	data, err := h.assets.Read(id, thumbnail)
	if err != nil {
		return media.ImageMetadata{}, nil, err
	}
	return *metadata, data, nil
}

// imageAck 回图片分片确认：body 为 "<imageID>|<index>#".
func (h *Hub) imageAck(peerKey string, key media.TransferKey, index uint32) error {
	addr, err := net.ResolveUDPAddr("udp4", peerKey)
	if err != nil {
		return err
	}
	_, err = h.buildWireAndSend(addr, ipmsg.CmdReportImage, fmt.Sprintf("%s|%d#", key.ImageID, index), nil)
	return err
}

func (h *Hub) imageFailure(key media.TransferKey, reason string) {
	log.Printf("图片接收失败 peer=%s image=%s: %s", key.PeerID, key.ImageID, reason)
	if h.ImageFailed != nil {
		h.ImageFailed(key.PeerID, key.ImageID, reason)
	}
}

// lockImages 尝试锁定重组器；数据库提交只持锁做元数据 SQL，
// 丢弃可重传的数据报优于阻塞 UDP 接收。
func (h *Hub) lockImages() bool { return h.imagesMu.TryLock() }

func (h *Hub) unlockImages() { h.imagesMu.Unlock() }

// registerImageReference 记录文本引用（引用可先于/后于分片到达）。
func (h *Hub) registerImageReference(peerID, imageID string) error {
	if h.images == nil {
		return nil
	}
	now := time.Now()
	if !h.lockImages() {
		return fmt.Errorf("图片提交处理中，请重试引用")
	}
	defer h.unlockImages()
	result := h.images.Reference(peerID, imageID, now)
	for _, failure := range h.images.Expire(now) {
		h.imageFailure(failure.Key, failure.Reason)
	}
	return result
}

func (h *Hub) rejectImageJob(job imageJob, reason string) {
	now := time.Now()
	if !h.lockImages() {
		return
	}
	failures := h.images.Expire(now)
	if now.Before(job.deadline) && h.images.IsFinalizing(job.key, now) {
		h.images.Finish(job.key, false, now)
		for _, failure := range h.images.Expire(now) {
			if failure.Key == job.key {
				failure.Reason = reason
			}
			failures = append(failures, failure)
		}
	}
	h.unlockImages()
	for _, failure := range failures {
		h.imageFailure(failure.Key, failure.Reason)
	}
}

func (h *Hub) handleImageFragment(pkt ipmsg.Packet, src *net.UDPAddr, key string) error {
	if pkt.Command&ipmsg.OptFileAttach == 0 {
		return fmt.Errorf("图片分片缺少附件选项")
	}
	if h.images == nil || h.assets == nil {
		return fmt.Errorf("图片服务未启用")
	}
	fragment, err := media.ParseFragment([]byte(pkt.Body))
	if err != nil {
		return err
	}
	h.upsertUser(key, pkt, src)
	h.mu.Lock()
	peer, ok := h.users[key]
	h.mu.Unlock()
	if !ok {
		return fmt.Errorf("图片发送方未知")
	}
	now := time.Now()
	if !h.lockImages() {
		return nil // 可重传，静默丢弃
	}
	action, acceptErr := h.images.Accept(peer.ID, fragment, now)
	for _, failure := range h.images.Expire(now) {
		h.imageFailure(failure.Key, failure.Reason)
	}
	h.unlockImages()
	if acceptErr != nil {
		return acceptErr
	}
	switch action.Kind {
	case media.ActionAck:
		return h.imageAck(key, action.Key, action.Index)
	case media.ActionFinalize:
		job := imageJob{key: action.Key, peer: *peer, payload: action.Payload, deadline: now.Add(25 * time.Second)}
		select {
		case h.imageJobs <- job:
		default:
			h.rejectImageJob(job, "图片处理队列已满")
		}
	case media.ActionPending:
	}
	return nil
}

// imageWorker 单并发处理解码与提交；解码超时不会堆积并发。
func (h *Hub) imageWorker() {
	for job := range h.imageJobs {
		h.processImageJob(job)
	}
}

func (h *Hub) processImageJob(job imageJob) {
	now := time.Now()
	h.imagesMu.Lock()
	if !h.images.IsFinalizing(job.key, now) {
		h.imagesMu.Unlock()
		h.rejectImageJob(job, "图片排队超时")
		return
	}
	if now.After(job.deadline) {
		h.imagesMu.Unlock()
		h.rejectImageJob(job, "图片解码排队超时")
		return
	}
	h.imagesMu.Unlock()

	// 单个工作协程天然一次一个解码
	decoded, err := media.DecodeImage(job.payload)
	if err != nil {
		h.rejectImageJob(job, err.Error())
		return
	}

	h.imagesMu.Lock()
	if !h.images.IsFinalizing(job.key, time.Now()) || time.Now().After(job.deadline) {
		h.imagesMu.Unlock()
		h.rejectImageJob(job, "图片处理已取消或超时")
		return
	}
	pending, err := h.assets.Persist(decoded, job.key.ImageID)
	if err != nil {
		h.imagesMu.Unlock()
		h.rejectImageJob(job, err.Error())
		return
	}
	metadata := pending.Metadata
	messageID := fmt.Sprintf("image-rx:%s:%s", job.key.PeerID, job.key.ImageID)
	timestamp := time.Now().Unix()
	record := store.Record{
		ID: messageID, FromID: job.peer.ID, ToID: h.LocalID(),
		Content: "[图片]", Kind: 1, Timestamp: timestamp, Status: 1,
	}
	// 持重组器锁做元数据提交，提交前后校验接收状态（对应 Rust 的提交闸门）
	inserted, err := h.db.CommitReceivedImage(record, metadata)
	if err != nil {
		h.unlockImages()
		pending.Release()
		h.rejectImageJob(job, err.Error())
		return
	}
	ackIndex, finished := h.images.Finish(job.key, true, time.Now())
	h.unlockImages()
	if !finished {
		pending.Release()
		return
	}
	pending.Keep()
	if inserted && h.ImageReceived != nil {
		h.ImageReceived(ReceivedEvent{
			Key: job.peer.Key, PeerID: job.peer.ID, User: &job.peer,
			ID: messageID, Content: "[图片]", Timestamp: timestamp,
		}, metadata)
	}
	if err := h.imageAck(job.peer.Key, job.key, ackIndex); err != nil {
		log.Printf("图片 ACK 发送失败: %v", err)
	}
}

// expireImageLoop 定期排空引用-only/收集超时，并完成已取消/超时的发送任务。
func (h *Hub) expireImageLoop() {
	ticker := time.NewTicker(5 * time.Second)
	defer ticker.Stop()
	for range ticker.C {
		if h.images == nil {
			return
		}
		h.imagesMu.Lock()
		failures := h.images.Expire(time.Now())
		h.imagesMu.Unlock()
		for _, failure := range failures {
			h.imageFailure(failure.Key, failure.Reason)
		}
		h.expireImageSends()
	}
}
