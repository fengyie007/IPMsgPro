// files.go 对应 tauri-rust src-tauri/src/files.rs：文件选择对话框、发送/接受/
// 拒绝/暂停/取消命令、打开所在文件夹与原生拖放分发。
package app

import (
	"os"

	"github.com/egoist/mygo"

	"my-app/internal/filetransfer"
)

// beginSelection 与图片选择/截图共用互斥（对拍 Rust image_selecting 标志）。
func (r *Runtime) beginSelection() bool {
	return r.selecting.CompareAndSwap(false, true)
}

func (r *Runtime) endSelection() { r.selecting.Store(false) }

// SelectFileOrFolder 弹出原生选择框并把所选文件/文件夹登记为发送预览。
func (r *Runtime) SelectFileOrFolder(directory bool) map[string]any {
	if !r.beginSelection() {
		return fail("请先完成已有选择或截图")
	}
	defer r.endSelection()
	paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{
		Title: map[bool]string{true: "选择要发送的文件夹", false: "选择要发送的文件"}[directory],
		// 图片过滤器保留给文件选择（文件夹模式忽略）。
		Filters:   []mygo.FileFilter{{Name: "常用文件", Extensions: []string{"png", "jpg", "jpeg", "bmp", "zip", "doc", "docx", "xls", "xlsx", "pdf", "txt"}}},
		Directory: directory,
	})
	if err != nil {
		return fail(err.Error())
	}
	if len(paths) == 0 {
		return ok(map[string]any{"cancelled": true})
	}
	transfers, err := r.Hub.Files()
	if err != nil {
		return fail(err.Error())
	}
	selection, err := transfers.SelectPath(paths[0])
	if err != nil {
		return fail(err.Error())
	}
	return ok(map[string]any{"files": []any{selection}})
}

// HandleFileDrop 处理原生拖放：按当前会话直接登记预览并通知前端
// （对拍 Rust files::dropped）。
func (r *Runtime) HandleFileDrop(paths []string) {
	if !r.accepting.Load() {
		return
	}
	r.mu.Lock()
	target := r.activeConversation
	r.mu.Unlock()
	r.Emit("file.selected", r.dropSelection(target, paths))
}

func (r *Runtime) dropSelection(target string, paths []string) map[string]any {
	if target == "" {
		return map[string]any{"target": target, "error": "请先选择一个聊天对象"}
	}
	if len(paths) == 0 || len(paths) > 8 {
		return map[string]any{"target": target, "error": "一次最多拖入8个文件或文件夹"}
	}
	if !r.beginSelection() {
		return map[string]any{"target": target, "error": "请先完成已有选择或截图"}
	}
	defer r.endSelection()
	transfers, err := r.Hub.Files()
	if err != nil {
		return map[string]any{"target": target, "error": err.Error()}
	}
	selections := make([]filetransfer.Selection, 0, len(paths))
	for _, path := range paths {
		selection, err := transfers.SelectPath(path)
		if err != nil {
			for _, s := range selections {
				transfers.Discard(s.SelectionID)
			}
			return map[string]any{"target": target, "error": err.Error()}
		}
		selections = append(selections, selection)
	}
	return map[string]any{"target": target, "files": selections}
}

// OpenFileFolder 在资源管理器中定位已接收完成的文件。
func (r *Runtime) OpenFileFolder(messageID string) map[string]any {
	path, err := r.DB.FilePath(messageID)
	if err != nil {
		return fail(err.Error())
	}
	info, err := os.Stat(path)
	if err != nil || (!info.Mode().IsRegular() && !info.IsDir()) {
		return fail("本地文件已移动或删除")
	}
	if err := revealInExplorer(path); err != nil {
		return fail(err.Error())
	}
	return ok(nil)
}
