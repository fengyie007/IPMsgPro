import React, { useRef, useEffect, useState, useMemo, useCallback } from 'react';
import { FiCamera, FiImage, FiX, FiFile, FiSmile, FiMoreHorizontal, FiTrash2, FiChevronUp } from 'react-icons/fi';
import { useUserStore } from '../stores/userStore';
import { useMessageStore, PendingFileReceive } from '../stores/messageStore';
import { toast } from '../stores/toastStore';
import { invoke, listen } from '../services/bridge';
import { buildEmojiMessage, emojiStyle } from '../emojiData';
import { isSameDay, formatDateSeparator, formatFileSize } from '../utils/format';
import ScreenshotEditor from './ScreenshotEditor';
import ConfirmDialog from './ConfirmDialog';
import MessageBubble from './MessageBubble';
import EmojiPicker from './EmojiPicker';
import SendPreviewModal from './SendPreviewModal';

const BLOCK_TAG = /^(DIV|P|LI|TR|PRE|BLOCKQUOTE|H[1-6])$/;

// Chromium wraps lines 2+ in <div>s, so a block breaks the line unless already at a line start.
function serializeEditor(root: HTMLElement): string {
  let out = '';
  const lineBreak = () => {
    if (out && !out.endsWith('\n')) out += '\n';
  };
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      out += node.textContent || '';
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const elem = node as HTMLElement;
    const emojiId = elem.dataset.emojiId;
    if (emojiId) {
      out += buildEmojiMessage(emojiId);
      return;
    }
    if (elem.tagName === 'BR') {
      out += '\n';
      return;
    }
    const block = BLOCK_TAG.test(elem.tagName);
    if (block) lineBreak();
    elem.childNodes.forEach(walk);
    if (block) lineBreak();
  };
  root.childNodes.forEach(walk);
  return out;
}

// ============================================================================
// ChatPanel - main chat component
// ============================================================================

export default function ChatPanel() {
  const currentUser = useUserStore((s) => s.currentUser);
  const sendMessage = useMessageStore((s) => s.sendMessage);
  const sendImage = useMessageStore((s) => s.sendImage);
  const sendImageByPath = useMessageStore((s) => s.sendImageByPath);
  const sendFileByPath = useMessageStore((s) => s.sendFileByPath);
  const loadHistory = useMessageStore((s) => s.loadHistory);
  const loadMoreHistory = useMessageStore((s) => s.loadMoreHistory);
  const clearHistory = useMessageStore((s) => s.clearHistory);
  const clearUnread = useMessageStore((s) => s.clearUnread);
  const pendingFileReceives = useMessageStore((s) => s.pendingFileReceives);
  const acceptFileReceive = useMessageStore((s) => s.acceptFileReceive);
  const rejectFileReceive = useMessageStore((s) => s.rejectFileReceive);

  const [hasInput, setHasInput] = useState(false);
  const messageListRef = useRef<HTMLDivElement>(null);

  // ---- Screenshot state ----
  const [screenshot, setScreenshot] = useState<{ image: string; screenCount?: number } | null>(null);
  const editorRef = useRef<HTMLDivElement>(null);
  // Last caret position inside the editor, so an emoji can be inserted there
  // even after the editor loses focus to the picker button.
  const savedRange = useRef<Range | null>(null);

  // Send-confirm modal state: the file picked via the native dialog (real path)
  const [pendingFilePath, setPendingFilePath] = useState<string | null>(null);
  const [pendingFileName, setPendingFileName] = useState<string>('');
  // Real file size (bytes) for the send-confirm modal, queried from backend
  const [pendingFileSize, setPendingFileSize] = useState<number | null>(null);
  const [pendingImage, setPendingImage] = useState<{
    target: string; filePath: string; fileName: string; fileSize: number; dataUrl: string;
  } | null>(null);
  const [imageBusy, setImageBusy] = useState(false);
  const imageBusyRef = useRef(false);
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [showMoreMenu, setShowMoreMenu] = useState(false);
  const [showClearConfirm, setShowClearConfirm] = useState(false);
  const [nativeDragging, setNativeDragging] = useState(false);

  const userId = currentUser?.id || '';

  // Get messages for current user
  const userMessages = useMessageStore((s) => s.messages.get(userId)) || [];
  const hasMoreHistory = useMessageStore((s) => s.historyPages.get(userId)?.hasMore ?? false);
  const historyLoading = useMessageStore((s) => s.loading);

  // Pending receive requests of this conversation, indexed by the id of the
  // placeholder message they belong to ("recv_<packetNo>"). Each bubble gets
  // only its own request, so memoized bubbles stay untouched when the list
  // changes elsewhere.
  const pendingByMessageId = useMemo(() => {
    const map = new Map<string, PendingFileReceive>();
    for (const r of pendingFileReceives) {
      if (r.fromUser === userId) map.set(`recv_${r.packetNo}`, r);
    }
    return map;
  }, [pendingFileReceives, userId]);

  // Load history when user changes; opening a conversation marks it as read
  useEffect(() => {
    if (userId) {
      loadHistory(userId);
      clearUnread(userId);
    }
  }, [userId]);

  // Auto-scroll only when the user is already at the bottom, or the newest
  // message is our own. A transfer progress tick or a history reload must not
  // yank the view away from older messages the user is reading.
  const lastMessage = userMessages[userMessages.length - 1];
  const lastMessageKey = lastMessage ? `${lastMessage.id}|${userMessages.length}` : '';
  const nearBottomRef = useRef(true);
  const handleListScroll = () => {
    const el = messageListRef.current;
    if (!el) return;
    nearBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };
  useEffect(() => {
    const el = messageListRef.current;
    if (!el) return;
    if (prependRef.current !== null) {
      // Older messages were prepended: keep the viewport on the same message
      // instead of jumping, by compensating for the added height.
      el.scrollTop += el.scrollHeight - prependRef.current;
      prependRef.current = null;
      return;
    }
    if (nearBottomRef.current || lastMessage?.from === 'self') {
      el.scrollTop = el.scrollHeight;
    }
  }, [lastMessageKey]);

  // "Load earlier messages": remember the scroll height so the effect above
  // can restore the position after the older page is merged in.
  const prependRef = useRef<number | null>(null);
  const handleLoadMore = async () => {
    const el = messageListRef.current;
    prependRef.current = el ? el.scrollHeight : null;
    await loadMoreHistory(userId);
  };

  const syncHasInput = () => {
    const el = editorRef.current;
    if (!el) return setHasInput(false);
    const hasText = (el.textContent || '').replace(/\s/g, '').length > 0;
    const hasEmoji = !!el.querySelector('[data-emoji-id]');
    setHasInput(hasText || hasEmoji);
  };

  // ---- Text send ----
  const handleSend = async () => {
    if (!currentUser || !editorRef.current) return;
    const content = serializeEditor(editorRef.current).replace(/\s+$/g, '');
    if (!content.trim()) return;
    const success = await sendMessage(currentUser.id, content);
    if (success) {
      if (editorRef.current) editorRef.current.innerHTML = '';
      setHasInput(false);
    } else {
      toast.error('发送失败，对方可能已离线');
    }
  };

  // Insert the <br> ourselves: Chromium's default differs per modifier (<br> vs. a new <div>).
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    if (e.shiftKey || e.ctrlKey) {
      document.execCommand('insertLineBreak');
    } else {
      handleSend();
    }
  };

  // Remember caret position inside the editor for later emoji insertion.
  const saveSelection = () => {
    const sel = window.getSelection();
    if (sel && sel.rangeCount > 0) {
      const r = sel.getRangeAt(0);
      if (editorRef.current?.contains(r.commonAncestorContainer)) {
        savedRange.current = r;
      }
    }
  };

  // ---- Emoji: insert inline into the input, not send immediately ----
  const insertEmoji = (id: string) => {
    const el = editorRef.current;
    if (!el) return;
    el.focus();
    const sel = window.getSelection();
    let range: Range;
    if (savedRange.current && el.contains(savedRange.current.commonAncestorContainer)) {
      range = savedRange.current;
    } else {
      range = document.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
    }
    range.deleteContents();
    const span = document.createElement('span');
    span.setAttribute('data-emoji-id', id);
    span.setAttribute('contenteditable', 'false');
    Object.assign(span.style, emojiStyle(id, 18) as CSSStyleDeclaration);
    range.insertNode(span);
    const after = document.createRange();
    after.setStartAfter(span);
    after.collapse(true);
    sel?.removeAllRanges();
    sel?.addRange(after);
    savedRange.current = after;
    syncHasInput();
  };

  const handleSelectEmoji = (id: string) => {
    insertEmoji(id);
    // Keep the picker open so multiple emojis can be added; click outside to close.
  };

  // ---- Screenshot feature ----
  // Clicking the screenshot button hides the window (handled in C++), captures the
  // current monitor, then returns here so the full-screen editor can be shown.
  const startScreenshot = async () => {
    if (!currentUser) return;
    try {
      const res: any = await invoke('screenshot.capture');
      if (!res || !res.success || !res.image) {
        toast.error('截图失败：' + ((res && res.error) || '未知错误'));
        return;
      }
      setScreenshot({
        image: res.image,
        screenCount: res.screenCount,
      });
      // 截图编辑器以全屏浮层呈现，选区阶段就把主窗口置顶，
      // 否则会被其它窗口盖住、无法覆盖到要截取的内容。
      try {
        await invoke('window.set_always_on_top', { on_top: true });
      } catch { /* ignore */ }
    } catch (err) {
      toast.error('截图失败：' + err);
    }
  };

  const finishScreenshot = async (confirm: boolean, dataUrl?: string) => {
    try {
      if (confirm && dataUrl && currentUser) {
        const base64 = dataUrl.split(',')[1];
        const now = new Date();
        const ts = now.getFullYear().toString() +
          String(now.getMonth() + 1).padStart(2, '0') +
          String(now.getDate()).padStart(2, '0') +
          String(now.getHours()).padStart(2, '0') +
          String(now.getMinutes()).padStart(2, '0') +
          String(now.getSeconds()).padStart(2, '0');
        const sent = await sendImage(currentUser.id, base64, `Beixin_${ts}_screenshot.png`);
        if (!sent) toast.error('截图发送失败：' + (useMessageStore.getState().error || '无法创建发送任务'));
      }
    } catch (err) {
      toast.error('截图发送失败：' + String(err));
    } finally {
      // Always release the editor's temporary topmost state, even if sending fails.
      // Restore and unpin independently so a restore error cannot skip unpinning.
      try {
        await invoke('window.restore');
      } catch (err) {
        console.error('[ChatPanel] Failed to restore screenshot window', err);
      }
      try {
        const res = await invoke<{ success: boolean; error?: string }>(
          'window.set_always_on_top', { on_top: false });
        if (!res?.success) throw new Error(res?.error || '取消置顶失败');
      } catch (err) {
        toast.error('取消截图窗口置顶失败：' + String(err));
      }
      setScreenshot(null);
    }
  };

  // ---- Inline image selection & preview ----
  const handleImageClick = async () => {
    if (!currentUser || imageBusyRef.current || pendingImage) return;
    const target = currentUser.id;
    imageBusyRef.current = true;
    setImageBusy(true);
    try {
      const result = await invoke<{ success: boolean; files?: string[]; error?: string }>('dialog.open', {
        title: '选择要发送的图片',
        multi_select: false,
        filters: [{ name: '图片 (PNG/JPEG/BMP)', pattern: '*.png;*.jpg;*.jpeg;*.bmp' }],
      });
      if (!result.success) throw new Error(result.error || '无法打开图片选择器');
      const filePath = result.files?.[0];
      if (!filePath) return;
      if (!/\.(png|jpe?g|bmp)$/i.test(filePath)) throw new Error('请选择 PNG、JPEG 或 BMP 图片');
      const preview = await invoke<{ success: boolean; dataUrl?: string; error?: string }>(
        'file.read_image', { filePath });
      if (!preview.success || !preview.dataUrl) throw new Error(preview.error || '无法读取图片预览');
      const info = await invoke<{ success: boolean; fileSize?: number }>('file.info', { filePath });
      setPendingImage({
        target, filePath,
        fileName: filePath.split(/[\\/]/).pop() || filePath,
        fileSize: info.success ? info.fileSize || 0 : 0,
        dataUrl: preview.dataUrl,
      });
    } catch (err) {
      toast.error('选择图片失败：' + (err instanceof Error ? err.message : String(err)));
    } finally {
      imageBusyRef.current = false;
      setImageBusy(false);
    }
  };

  const cancelPendingImage = () => {
    if (!imageBusyRef.current) setPendingImage(null);
  };
  const handleImageSend = async () => {
    if (!pendingImage || imageBusyRef.current) return;
    imageBusyRef.current = true;
    setImageBusy(true);
    try {
      const sent = await sendImageByPath(pendingImage.target, pendingImage.filePath);
      if (sent) setPendingImage(null);
      else toast.error('图片发送失败：' + (useMessageStore.getState().error || '无法创建发送任务'));
    } finally {
      imageBusyRef.current = false;
      setImageBusy(false);
    }
  };
  useEffect(() => {
    if (!pendingImage) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' && event.key !== 'Enter') return;
      event.preventDefault();
      event.stopPropagation();
      if (event.key === 'Escape') cancelPendingImage();
      else void handleImageSend();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [pendingImage, sendImageByPath]);

  // ---- File select & preview ----
  // 使用原生文件对话框直接拿到真实路径，避免前端 base64 编码 + 复制到临时文件夹（大文件极慢）
  const handleFileClick = async () => {
    if (!currentUser) return;
    try {
      const res = await invoke<{ success?: boolean; files?: string[] }>('dialog.open', {
        title: '选择要发送的文件',
        multi_select: false,
      });
      if (res && res.success && res.files && res.files.length > 0) {
        const fp = res.files[0];
        const name = fp.split(/[\\/]/).pop() || fp;
        setPendingFilePath(fp);
        setPendingFileName(name);
        setPendingFileSize(null);
        // Query real file size from backend for the confirm modal
        invoke<{ success?: boolean; fileSize?: number }>('file.info', { filePath: fp })
          .then((r) => setPendingFileSize(r && r.success ? (r.fileSize ?? 0) : 0))
          .catch(() => setPendingFileSize(0));
      }
    } catch (e) {
      console.error('[ChatPanel] dialog.open failed', e);
    }
  };

  const resetPendingFile = () => {
    setPendingFilePath(null);
    setPendingFileName('');
    setPendingFileSize(null);
  };

  const handleFileSend = async () => {
    if (!currentUser || !pendingFilePath) return;
    await sendFileByPath(currentUser.id, pendingFilePath);
    resetPendingFile();
  };

  // ---- Native file drag & drop ----
  // The window host (TauriCPP) captures dropped files at the OS level and emits
  // their real filesystem paths via the `window.files_dropped` event, so we can
  // send them through the real path (file.send) without base64 + save_temp.
  // `window.files_dragging` toggles the drop overlay.
  const currentUserRef = useRef(currentUser);
  currentUserRef.current = currentUser;

  useEffect(() => {
    const offDrop = listen('window.files_dropped', (data: any) => {
      const user = currentUserRef.current;
      const paths: string[] = (data && data.paths) || [];
      if (!user || paths.length === 0) return;
      paths.forEach((p: string) => sendFileByPath(user.id, p));
    });
    const offDrag = listen('window.files_dragging', (data: any) => {
      setNativeDragging(!!(data && data.dragging));
    });
    return () => {
      offDrop();
      offDrag();
    };
  }, [sendFileByPath]);

  // ---- File receive handlers ----
  // Stable references so memoized MessageBubble instances don't re-render
  // on unrelated progress updates.
  const handleAcceptFile = useCallback((requestId: string) => {
    // Pass empty savePath, backend will auto-generate using Downloads folder
    acceptFileReceive(requestId, '');
  }, [acceptFileReceive]);

  const handleRejectFile = useCallback((requestId: string) => {
    rejectFileReceive(requestId);
  }, [rejectFileReceive]);

  if (!currentUser) return null;

  const statusText = currentUser.status === 'online' ? '在线'
    : currentUser.status === 'away' ? '离开'
    : '离线';

  const groupText = currentUser.group ? ` · ${currentUser.group}` : '';

  return (
    <div
      className={`flex-1 flex flex-col bg-white relative ${nativeDragging ? 'ring-2 ring-inset ring-primary-400' : ''}`}
    >
      {nativeDragging && (
        <div className="absolute inset-0 z-40 flex items-center justify-center bg-primary-50/70 pointer-events-none">
          <div className="px-6 py-4 rounded-lg border-2 border-dashed border-primary-500 text-primary-600 text-sm font-medium">
            释放以发送文件{currentUser ? `给 ${currentUser.nickname}` : ''}
          </div>
        </div>
      )}
      {/* Chat header */}
      <div className="h-14 border-b border-gray-200 flex items-center px-4 shrink-0">
        <div>
          <h3 className="text-sm font-medium text-gray-800">{currentUser.nickname}</h3>
          <p className="text-xs text-gray-400">
            {statusText}{groupText} · {currentUser.ip}:{currentUser.port}
          </p>
        </div>
        <div className="ml-auto relative">
          <button
            type="button"
            title="更多"
            onClick={() => setShowMoreMenu((v) => !v)}
            className="w-8 h-8 flex items-center justify-center rounded hover:bg-gray-100 text-gray-600"
          >
            <FiMoreHorizontal size={18} />
          </button>
          {showMoreMenu && (
            <>
              <div className="fixed inset-0 z-10" onClick={() => setShowMoreMenu(false)} />
              <div className="absolute right-0 mt-1 w-40 bg-white rounded-md shadow-lg border border-gray-200 py-1 z-20">
                <button
                  type="button"
                  onClick={() => {
                    setShowMoreMenu(false);
                    setShowClearConfirm(true);
                  }}
                  className="w-full flex items-center gap-2 px-3 py-2 text-sm text-left text-red-600 hover:bg-gray-100"
                >
                  <FiTrash2 size={14} />
                  清空聊天记录
                </button>
              </div>
            </>
          )}
        </div>
      </div>

      {/* Message list */}
      <div ref={messageListRef} onScroll={handleListScroll} className="flex-1 overflow-y-auto p-4 space-y-3 bg-chat-bg">
        {hasMoreHistory && (
          <div className="flex justify-center">
            <button
              type="button"
              disabled={historyLoading}
              onClick={handleLoadMore}
              className="flex items-center gap-1 px-3 py-1 text-xs text-gray-500 bg-white/70 rounded-full hover:bg-white hover:text-gray-700 disabled:opacity-50 transition-colors"
            >
              <FiChevronUp size={12} />
              {historyLoading ? '加载中…' : '加载更早的消息'}
            </button>
          </div>
        )}
        {userMessages.length === 0 && pendingByMessageId.size === 0 ? (
          <div className="text-center text-gray-400 text-sm mt-10">
            暂无消息，发送一条消息开始聊天
          </div>
        ) : (
          userMessages.map((msg, i) => {
            const prev = userMessages[i - 1];
            const showDate = !prev || !isSameDay(prev.timestamp, msg.timestamp);
            return (
              <React.Fragment key={msg.id}>
                {showDate && (
                  <div className="flex justify-center">
                    <span className="px-2 py-0.5 text-[11px] text-gray-500 bg-gray-200/70 rounded">
                      {formatDateSeparator(msg.timestamp)}
                    </span>
                  </div>
                )}
                <MessageBubble
                  message={msg}
                  pendingRequest={pendingByMessageId.get(msg.id)}
                  onAccept={handleAcceptFile}
                  onReject={handleRejectFile}
                />
              </React.Fragment>
            );
          })
        )}
      </div>

      {/* Input area */}
      <div className="border-t border-gray-200 shrink-0 relative">
        {/* Emoji picker */}
        {showEmojiPicker && (
          <EmojiPicker onSelect={handleSelectEmoji} onClose={() => setShowEmojiPicker(false)} />
        )}

        {/* Toolbar */}
        <div className="flex items-center gap-2 px-4 pt-2">
          <button
            className={`p-1.5 rounded transition-colors ${showEmojiPicker ? 'text-primary-500 bg-gray-100' : 'text-gray-400 hover:text-gray-600'}`}
            title="表情"
            onClick={() => setShowEmojiPicker((v) => !v)}
          >
            <FiSmile size={18} />
          </button>
          <button
            className="p-1.5 text-gray-400 hover:text-gray-600 rounded transition-colors"
            title="截图"
            onClick={startScreenshot}
          >
            <FiCamera size={18} />
          </button>
          <button
            className="p-1.5 text-gray-400 hover:text-gray-600 rounded transition-colors disabled:opacity-50"
            title="发送图片"
            disabled={imageBusy || pendingImage !== null}
            onClick={handleImageClick}
          >
            <FiImage size={18} />
          </button>
          <button
            className="p-1.5 text-gray-400 hover:text-gray-600 rounded transition-colors"
            title="文件"
            onClick={handleFileClick}
          >
            <FiFile size={18} />
          </button>
        </div>

        {/* Text input (contentEditable so emoji can be inserted inline at text height) */}
        <div className="px-4 pb-3 pt-1">
          <div
            ref={editorRef}
            contentEditable
            suppressContentEditableWarning
            onKeyDown={handleKeyDown}
            onKeyUp={saveSelection}
            onMouseUp={saveSelection}
            onBlur={saveSelection}
            onInput={syncHasInput}
            data-placeholder="输入消息，Enter 发送，Shift+Enter 或 Ctrl+Enter 换行"
            className="chat-editor w-full min-h-[4.5rem] max-h-40 overflow-y-auto text-sm leading-relaxed p-2 rounded border border-gray-200
                       focus:outline-none focus:ring-1 focus:ring-primary-400
                       whitespace-pre-wrap break-words"
          />
          <div className="flex justify-end mt-1">
            <button
              className="px-4 py-1.5 text-sm text-white bg-primary-500 rounded
                         hover:bg-primary-600 transition-colors disabled:opacity-50
                         disabled:cursor-not-allowed"
              onClick={handleSend}
              disabled={!hasInput}
            >
              发送
            </button>
          </div>
        </div>
      </div>

      {/* Send confirm modal */}
      {pendingFilePath !== null && (
        <SendPreviewModal
          fileName={pendingFileName}
          fileSize={pendingFileSize ?? 0}
          onConfirm={handleFileSend}
          onCancel={resetPendingFile}
        />
      )}

      {/* Inline image confirmation; normal files keep their existing modal. */}
      {pendingImage && (
        <div className="fixed inset-0 bg-black/40 z-50 flex items-center justify-center" onClick={cancelPendingImage}>
          <div
            role="dialog"
            aria-modal="true"
            aria-label="发送图片"
            className="bg-white rounded-lg shadow-xl w-[400px] max-h-[80vh] flex flex-col"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between px-4 py-3 border-b">
              <h3 className="text-sm font-medium text-gray-800">发送图片</h3>
              <button className="p-1 text-gray-400 hover:text-gray-600 disabled:opacity-50" title="取消" disabled={imageBusy} onClick={cancelPendingImage}>
                <FiX size={16} />
              </button>
            </div>
            <div className="px-4 py-3 flex-1 overflow-auto">
              <img src={pendingImage.dataUrl} alt={pendingImage.fileName} className="mx-auto max-w-full max-h-[300px] rounded object-contain" />
              <p className="text-sm text-gray-700 truncate mt-2" title={pendingImage.fileName}>{pendingImage.fileName}</p>
              <p className="text-xs text-gray-400 mt-1">{formatFileSize(pendingImage.fileSize)} · 以飞秋内嵌图片发送，对方无需确认文件下载</p>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t">
              <button className="px-4 py-1.5 text-sm text-gray-600 bg-gray-100 rounded hover:bg-gray-200 disabled:opacity-50" disabled={imageBusy} onClick={cancelPendingImage}>取消</button>
              <button autoFocus className="px-4 py-1.5 text-sm text-white bg-primary-500 rounded hover:bg-primary-600 disabled:opacity-50" disabled={imageBusy} onClick={handleImageSend}>
                {imageBusy ? '准备发送…' : '发送'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Full-screen screenshot editor */}
      {screenshot && (
        <ScreenshotEditor
          image={screenshot.image}
          screenCount={screenshot.screenCount}
          onCancel={() => finishScreenshot(false)}
          onConfirm={(dataUrl) => finishScreenshot(true, dataUrl)}
        />
      )}

      {/* Clear-history confirmation */}
      {showClearConfirm && (
        <ConfirmDialog
          title="清空聊天记录"
          message={`确定要清空与 ${currentUser.nickname} 的全部聊天记录吗？此操作不可恢复。`}
          confirmText="清空"
          danger
          onConfirm={() => {
            setShowClearConfirm(false);
            clearHistory(userId);
            toast.success('聊天记录已清空');
          }}
          onCancel={() => setShowClearConfirm(false)}
        />
      )}
    </div>
  );
}
