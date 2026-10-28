// assets.go 对应 tauri-rust image/assets.rs：磁盘上的受管图片资产。
// Go 无 Drop：PendingAsset 的生命周期由持有方显式 Keep()/Release() 管理，
// 未 Keep 的资产必须调用 Release 回收（收图工作协程保证这一点）。
package media

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"time"
)

const (
	maxAssetPNGBytes  = int64(64 * 1024 * 1024)
	maxAssetThumbByte = int64(2 * 1024 * 1024)
)

// ImageMetadata 与 Rust 版字段一致（camelCase JSON）。
type ImageMetadata struct {
	AssetID  string `json:"assetId"`
	FileName string `json:"fileName"`
	FileSize uint64 `json:"fileSize"`
	Mime     string `json:"mime"`
	Width    uint32 `json:"width"`
	Height   uint32 `json:"height"`
}

func ValidAssetID(id string) bool {
	if id == "" || len(id) > 96 {
		return false
	}
	for i := 0; i < len(id); i++ {
		c := id[i]
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c == '-') {
			return false
		}
	}
	return true
}

// PendingAsset 持有一个图片的目录与元数据，直到数据库接受它。
type PendingAsset struct {
	Metadata  ImageMetadata
	directory string
	kept      bool
}

func (p *PendingAsset) Keep() { p.kept = true }

func (p *PendingAsset) MetadataMatchesDirectory() bool {
	return filepath.Base(p.directory) == p.Metadata.AssetID
}

// Release 删除未保留的目录；已 Keep 的资产不删。
func (p *PendingAsset) Release() {
	if p.kept {
		return
	}
	if canonical, err := filepath.EvalSymlinks(p.directory); err == nil && canonical == p.directory {
		_ = os.RemoveAll(p.directory)
	}
}

type AssetStore struct {
	root string
}

func NewAssetStore(root string) (*AssetStore, error) {
	if err := os.MkdirAll(root, 0o755); err != nil {
		return nil, err
	}
	canonical, err := filepath.EvalSymlinks(root)
	if err != nil {
		return nil, err
	}
	return &AssetStore{root: canonical}, nil
}

var assetSequence atomic.Uint64

// Persist 独占创建资产目录并写入 full.png / thumb.png；
// 失败时只回收本次独占创建的目录。
func (s *AssetStore) Persist(decoded *DecodedImage, wireID string) (*PendingAsset, error) {
	if len(decoded.PNG) == 0 || int64(len(decoded.PNG)) > maxAssetPNGBytes ||
		len(decoded.ThumbnailPNG) == 0 || int64(len(decoded.ThumbnailPNG)) > maxAssetThumbByte {
		return nil, fmt.Errorf("图片输出超出大小限制")
	}
	now := time.Now().UnixNano()
	id := fmt.Sprintf("rx-%x-%x-%x", os.Getpid(), now, assetSequence.Add(1))
	directory := filepath.Join(s.root, id)
	// create_dir 独占：绝不覆盖或清理别的资产
	if err := os.Mkdir(directory, 0o755); err != nil {
		return nil, err
	}
	pending := &PendingAsset{
		Metadata: ImageMetadata{
			AssetID:  id,
			FileName: fmt.Sprintf("飞秋图片_%s.png", wireID),
			FileSize: uint64(len(decoded.PNG)),
			Mime:     "image/png",
			Width:    decoded.Width,
			Height:   decoded.Height,
		},
		directory: directory,
	}
	for name, data := range map[string][]byte{"full.png": decoded.PNG, "thumb.png": decoded.ThumbnailPNG} {
		target := filepath.Join(directory, name)
		file, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if err != nil {
			pending.Release()
			return nil, err
		}
		if _, err := file.Write(data); err != nil {
			file.Close()
			pending.Release()
			return nil, err
		}
		if err := file.Sync(); err != nil {
			file.Close()
			pending.Release()
			return nil, err
		}
		file.Close()
	}
	return pending, nil
}

// Read 读取已登记的资产；调用方必须先确认 ID 已登记在 image_assets。
func (s *AssetStore) Read(id string, thumbnail bool) ([]byte, error) {
	if !ValidAssetID(id) {
		return nil, fmt.Errorf("无效图片标识")
	}
	path := filepath.Join(s.root, id, map[bool]string{true: "thumb.png", false: "full.png"}[thumbnail])
	canonical, err := filepath.EvalSymlinks(path)
	if err != nil {
		return nil, fmt.Errorf("图片文件不存在")
	}
	if !strings.HasPrefix(canonical, s.root+string(filepath.Separator)) && canonical != s.root {
		return nil, fmt.Errorf("图片路径超出应用存储目录")
	}
	limit := maxAssetPNGBytes
	if thumbnail {
		limit = maxAssetThumbByte
	}
	info, err := os.Stat(canonical)
	if err != nil {
		return nil, err
	}
	if info.Size() > limit {
		return nil, fmt.Errorf("图片文件过大")
	}
	data, err := os.ReadFile(canonical)
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit || !strings.HasPrefix(string(data), "\x89PNG\r\n\x1a\n") {
		return nil, fmt.Errorf("图片文件格式或大小无效")
	}
	return data, nil
}

// RemoveUnreferenced 删除数据库里已标记废弃的未引用资产；
// 删除失败必须上报，让墓碑可以重试而不是丢失。
func (s *AssetStore) RemoveUnreferenced(id string) error {
	if !ValidAssetID(id) {
		return fmt.Errorf("无效资产标识")
	}
	path := filepath.Join(s.root, id)
	canonical, err := filepath.EvalSymlinks(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	if canonical != path {
		return fmt.Errorf("拒绝删除重定向的资产目录")
	}
	return os.RemoveAll(path)
}

// DiscardUncommitted 只用于新写入、未提交的资产；绝不用于历史 GC。
func (s *AssetStore) DiscardUncommitted(id string) {
	if !ValidAssetID(id) {
		return
	}
	path := filepath.Join(s.root, id)
	if canonical, err := filepath.EvalSymlinks(path); err == nil && canonical == path {
		_ = os.RemoveAll(path)
	}
}
