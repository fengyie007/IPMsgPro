import React, { useRef, useEffect, useState, memo, useCallback } from 'react';
import { FiCamera, FiImage, FiFile, FiSmile, FiX, FiCheck, FiAlertCircle, FiDownload, FiFolder, FiMoreHorizontal, FiTrash2 } from 'react-icons/fi';
import { useUserStore } from '../stores/userStore';
import { useMessageStore, PendingFileReceive } from '../stores/messageStore';
import { Message } from '../types';
import { invoke, listen } from '../services/bridge';
import { EMOJIS, buildEmojiMessage, parseEmojiId, emojiStyle, EMOJI_TOKEN_RE } from '../emojiData';
import ScreenshotEditor from './ScreenshotEditor';

// ============================================================================
// Send Preview Modal - shown before sending a file
// ============================================================================

interface SendPreviewProps {
  fileName: string;
  fileSize: number;
  onConfirm: () => void;
  onCancel: () => void;
}

function SendPreview({ fileName, fileSize, onConfirm, onCancel }: SendPreviewProps) {
  return (
    <div className="fixed inset-0 bg-black/40 z-50 flex items-center justify-center"
      onClick={onCancel}>
      <div className="bg-white rounded-lg shadow-xl w-[400px] max-h-[80vh] flex flex-col"
        onClick={e => e.stopPropagation()}>
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b">
          <h3 className="text-sm font-medium text-gray-800">发送文件</h3>
          <button className="p-1 text-gray-400 hover:text-gray-600" onClick={onCancel}>
            <FiX size={16} />
          </button>
        </div>

        {/* Preview */}
        <div className="px-4 py-3 flex-1 overflow-auto">
          <div className="flex items-center gap-3 p-3 bg-gray-50 rounded-lg">
            <FiFile size={28} className="text-gray-400 shrink-0" />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-gray-800 truncate">{fileName}</p>
              <p className="text-xs text-gray-400">{formatFileSize(fileSize)}</p>
            </div>
          </div>
          <p className="text-xs text-gray-400 mt-2">文件将通过 TCP 传输发送给对方</p>
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-2 px-4 py-3 border-t">
          <button
            className="px-4 py-1.5 text-sm text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
            onClick={onCancel}
          >
            取消
          </button>
          <button
            className="px-4 py-1.5 text-sm text-white bg-primary-500 rounded hover:bg-primary-600 transition-colors"
            onClick={onConfirm}
          >
            发送
          </button>
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// ChatPanel - main chat component
// ============================================================================

export default function ChatPanel() {
  const currentUser = useUserStore((s) => s.currentUser);
  const sendMessage = useMessageStore((s) => s.sendMessage);
  const sendImage = useMessageStore((s) => s.sendImage);
  const sendFileByPath = useMessageStore((s) => s.sendFileByPath);
  const loadHistory = useMessageStore((s) => s.loadHistory);
  const clearHistory = useMessageStore((s) => s.clearHistory);
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
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [showMoreMenu, setShowMoreMenu] = useState(false);
  const [nativeDragging, setNativeDragging] = useState(false);

  const userId = currentUser?.id || '';

  // Get messages for current user
  const userMessages = useMessageStore((s) => s.messages.get(userId)) || [];

  // Get pending receives for current user
  const currentUserPendingReceives = pendingFileReceives.filter(
    r => r.fromUser === userId
  );

  // Load history when user changes
  useEffect(() => {
    if (userId) {
      loadHistory(userId);
    }
  }, [userId]);

  // Auto-scroll to bottom
  useEffect(() => {
    if (messageListRef.current) {
      messageListRef.current.scrollTop = messageListRef.current.scrollHeight;
    }
  }, [userMessages, currentUserPendingReceives]);

  // ---- Serialize the contentEditable input into the wire format ----
  // Text nodes are kept verbatim; inline emoji spans become WeChat-style XML;
  // <br>/block breaks become newlines.
  const serializeEditor = (): string => {
    const el = editorRef.current;
    if (!el) return '';
    let out = '';
    el.childNodes.forEach((node) => {
      if (node.nodeType === Node.TEXT_NODE) {
        out += node.textContent || '';
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        const elem = node as HTMLElement;
        const emojiId = elem.dataset.emojiId;
        if (emojiId) {
          out += buildEmojiMessage(emojiId);
        } else if (elem.tagName === 'BR') {
          out += '\n';
        } else {
          out += (elem.textContent || '') + (elem.tagName === 'DIV' || elem.tagName === 'P' ? '\n' : '');
        }
      }
    });
    return out;
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
    if (!currentUser) return;
    const content = serializeEditor().replace(/\s+$/g, '');
    if (!content.trim()) return;
    const success = await sendMessage(currentUser.id, content);
    if (success) {
      if (editorRef.current) editorRef.current.innerHTML = '';
      setHasInput(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.ctrlKey && !e.shiftKey) {
      e.preventDefault();
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
        alert('截图失败：' + ((res && res.error) || '未知错误'));
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
      alert('截图失败：' + err);
    }
  };

  const finishScreenshot = async (confirm: boolean, dataUrl?: string) => {
    if (confirm && dataUrl && currentUser) {
      const base64 = dataUrl.split(',')[1];
      const now = new Date();
      const ts = now.getFullYear().toString() +
        String(now.getMonth() + 1).padStart(2, '0') +
        String(now.getDate()).padStart(2, '0') +
        String(now.getHours()).padStart(2, '0') +
        String(now.getMinutes()).padStart(2, '0') +
        String(now.getSeconds()).padStart(2, '0');
      await sendImage(currentUser.id, base64, `Beixin_${ts}_screenshot.png`);
    }
    // Restore the window to its normal state and keep it on top.
    try {
      await invoke('window.restore');
      await invoke('window.set_always_on_top', { on_top: true });
    } catch { /* ignore */ }
    setScreenshot(null);
  };

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
                    if (window.confirm(`确定要清空与 ${currentUser.nickname} 的聊天记录吗？`)) {
                      clearHistory(userId);
                    }
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
      <div ref={messageListRef} className="flex-1 overflow-y-auto p-4 space-y-3 bg-[#F5F5F5]">
        {userMessages.length === 0 && currentUserPendingReceives.length === 0 ? (
          <div className="text-center text-gray-400 text-sm mt-10">
            暂无消息，发送一条消息开始聊天
          </div>
        ) : (
          userMessages.map((msg) => (
            <MessageBubble
              key={msg.id}
              message={msg}
              pendingReceives={pendingFileReceives}
              onAccept={handleAcceptFile}
              onReject={handleRejectFile}
            />
          ))
        )}
      </div>

      {/* Input area */}
      <div className="border-t border-gray-200 shrink-0 relative">
        {/* Emoji picker */}
        {showEmojiPicker && (
          <>
            <div
              className="fixed inset-0 z-10"
              onClick={() => setShowEmojiPicker(false)}
            />
            <div className="absolute bottom-full left-0 mb-2 w-[30rem] max-h-72 overflow-y-auto
                            bg-white border border-gray-200 rounded-lg shadow-lg p-2
                            grid grid-cols-10 gap-1 z-20">
              {EMOJIS.map((e) => (
                <button
                  key={e.id}
                  title={e.id}
                  className="p-1 hover:bg-gray-100 rounded transition-colors flex items-center justify-center"
                  onClick={() => handleSelectEmoji(e.id)}
                >
                  <EmojiSprite id={e.id} size={32} />
                </button>
              ))}
            </div>
          </>
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
            data-placeholder="输入消息，Enter发送，Ctrl+Enter换行"
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
        <SendPreview
          fileName={pendingFileName}
          fileSize={pendingFileSize ?? 0}
          onConfirm={handleFileSend}
          onCancel={resetPendingFile}
        />
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
    </div>
  );
}

// ============================================================================
// MessageBubble - render different types of messages
// ============================================================================

interface MessageBubbleProps {
  message: Message;
  pendingReceives: PendingFileReceive[];
  onAccept: (requestId: string) => void;
  onReject: (requestId: string) => void;
}

const MessageBubble = memo(function MessageBubble({ message, pendingReceives, onAccept, onReject }: MessageBubbleProps) {
  const isSelf = message.from === 'self';

  return (
    <div className={`flex ${isSelf ? 'justify-end' : 'justify-start'} message-in`}>
      <div className={`max-w-[70%] ${isSelf ? 'order-2' : ''}`}>
        <div
          className={`px-3 py-2 rounded-lg text-sm leading-relaxed break-words
            ${isSelf
              ? 'bg-chat-sent text-gray-800 rounded-tr-none'
              : 'bg-chat-received text-gray-800 rounded-tl-none shadow-sm'
            }`}
        >
          {renderContent(message, pendingReceives, onAccept, onReject)}
        </div>
        <div className={`text-[10px] text-gray-400 mt-0.5 ${isSelf ? 'text-right' : 'text-left'}`}>
          {formatTime(message.timestamp)}
        </div>
      </div>
    </div>
  );
});

// Split a message that mixes plain text and inline emoji XML tokens into an
// ordered list of segments for inline rendering.
function splitInlineContent(content: string): Array<{ type: 'text'; value: string } | { type: 'emoji'; id: string }> {
  const parts: Array<{ type: 'text'; value: string } | { type: 'emoji'; id: string }> = [];
  EMOJI_TOKEN_RE.lastIndex = 0;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = EMOJI_TOKEN_RE.exec(content)) !== null) {
    if (m.index > last) {
      parts.push({ type: 'text', value: content.slice(last, m.index) });
    }
    parts.push({ type: 'emoji', id: m[1] });
    last = m.index + m[0].length;
  }
  if (last < content.length) {
    parts.push({ type: 'text', value: content.slice(last) });
  }
  return parts;
}

function renderContent(
  message: Message,
  pendingReceives: PendingFileReceive[],
  onAccept: (requestId: string) => void,
  onReject: (requestId: string) => void
) {
  switch (message.type) {
    case 'image':
      return <ImageContent message={message} pendingReceives={pendingReceives} onAccept={onAccept} onReject={onReject} />;
    case 'file':
      return <FileContent message={message} pendingReceives={pendingReceives} onAccept={onAccept} onReject={onReject} />;
    default: {
      const emojiId = parseEmojiId(message.content);
      if (emojiId) {
        return <EmojiSprite id={emojiId} size={18} />;
      }
      // Mixed message: text with inline emoji tokens.
      const parts = splitInlineContent(message.content);
      if (parts.length === 1 && parts[0].type === 'text') {
        return <span className="whitespace-pre-wrap break-words">{parts[0].value}</span>;
      }
      return (
        <span className="inline-flex flex-wrap items-center whitespace-pre-wrap break-words">
          {parts.map((p, i) =>
            p.type === 'text' ? (
              <span key={i} className="whitespace-pre-wrap break-words">
                {p.value}
              </span>
            ) : (
              <EmojiSprite key={i} id={p.id} size={18} />
            )
          )}
        </span>
      );
    }
  }
}

// ---- Emoji drawn from the sprite sheet (emoji.png) via background-position ----
function EmojiSprite({ id, size }: { id: string; size: number }) {
  const style = emojiStyle(id, size);
  if (!style) {
    return <span className="text-gray-400">[emoji:{id}]</span>;
  }
  return <span style={style} />;
}

// ---- Image message with thumbnail + progress ----
function ImageContent({ message, pendingReceives, onAccept, onReject }: {
  message: Message;
  pendingReceives: PendingFileReceive[];
  onAccept: (requestId: string) => void;
  onReject: (requestId: string) => void;
}) {
  const progress = message.transferProgress;
  const isWaiting = progress === -1;  // waiting for acceptance
  const isTransferring = progress !== undefined && progress !== -1 && progress >= 0 && progress < 100;
  const isCompleted = progress === 100 || (message.status === 'delivered' && message.fileInfo?.filePath);
  const isFailed = message.status === 'failed';

  // Find the pending receive request for this message
  const pendingReq = pendingReceives.find(r => `recv_${r.packetNo}` === message.id);

  return (
    <div className="relative">
      {/* Image display */}
      {message.content.startsWith('data:') ? (
        <img
          src={message.content}
          alt="图片"
          className="max-w-full rounded cursor-pointer hover:opacity-90 transition-opacity"
          style={{ maxHeight: 200 }}
        />
      ) : (
        <div className="flex items-center gap-2 p-1">
          <FiImage size={20} className="text-primary-400 shrink-0" />
          <span className="text-sm text-gray-700">{message.fileInfo?.fileName || message.content}</span>
        </div>
      )}

      {/* File receive confirmation - embedded in message */}
      {isWaiting && pendingReq && (
        <div className="mt-2 p-2 bg-amber-50 rounded border border-amber-200">
          <div className="flex items-center gap-2 mb-2">
            <FiDownload size={14} className="text-amber-600" />
            <span className="text-xs text-amber-700 font-medium">
              对方发送了一张图片，是否接收？
            </span>
          </div>
          <div className="flex items-center gap-2 text-xs text-gray-500 mb-2">
            <span>{message.fileInfo?.fileName}</span>
            {message.fileInfo?.fileSize ? <span>({formatFileSize(message.fileInfo.fileSize)})</span> : null}
          </div>
          <div className="flex gap-2">
            <button
              className="px-3 py-1 text-xs text-white bg-primary-500 rounded hover:bg-primary-600 transition-colors"
              onClick={() => onAccept(pendingReq.id)}
            >
              接收
            </button>
            <button
              className="px-3 py-1 text-xs text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
              onClick={() => onReject(pendingReq.id)}
            >
              拒绝
            </button>
          </div>
        </div>
      )}

      {/* Transfer progress */}
      {isTransferring && (
        <div className="mt-1">
          <div className="w-full h-1 bg-gray-200 rounded-full overflow-hidden">
            <div
              className="h-full bg-primary-500 rounded-full transition-all duration-300"
              style={{ width: `${progress}%` }}
            />
          </div>
          <span className="text-xs text-gray-400">{progress}%</span>
        </div>
      )}

      {/* Transfer completed - show "open folder" link */}
      {isCompleted && message.fileInfo?.filePath && !isSelf(message) && (
        <div className="mt-1 flex items-center gap-1">
          <FiCheck size={12} className="text-green-500" />
          <span className="text-xs text-green-600">已保存</span>
          <button
            className="text-xs text-primary-500 hover:text-primary-600 flex items-center gap-0.5"
            onClick={() => openFolder(message.fileInfo!.filePath!)}
          >
            <FiFolder size={10} />
            打开文件夹
          </button>
        </div>
      )}

      {/* Sent successfully indicator for self-sent images */}
      {isCompleted && isSelf(message) && (
        <div className="mt-1 flex items-center gap-1">
          <FiCheck size={12} className="text-green-500" />
          <span className="text-xs text-green-600">发送成功</span>
        </div>
      )}

      {isFailed && (
        <div className="mt-1 flex items-center gap-1">
          <FiAlertCircle size={12} className="text-red-500" />
          <span className="text-xs text-red-500">传输失败</span>
        </div>
      )}
    </div>
  );
}

// ---- File message with card + progress ----
function FileContent({ message, pendingReceives, onAccept, onReject }: {
  message: Message;
  pendingReceives: PendingFileReceive[];
  onAccept: (requestId: string) => void;
  onReject: (requestId: string) => void;
}) {
  const progress = message.transferProgress;
  const isWaiting = progress === -1;  // waiting for acceptance
  const isTransferring = progress !== undefined && progress !== -1 && progress >= 0 && progress < 100;
  const isCompleted = progress === 100 || (message.status === 'delivered' && message.fileInfo?.filePath);
  const isFailed = message.status === 'failed';
  const isSentByMe = message.from === 'self';

  // Find the pending receive request for this message
  const pendingReq = pendingReceives.find(r => `recv_${r.packetNo}` === message.id);

  return (
    <div className="min-w-[200px]">
      <div className="flex items-center gap-3">
        <div className={`p-2 rounded ${isCompleted ? 'bg-green-50' : 'bg-gray-50'}`}>
          <FiFile size={20} className={isCompleted ? 'text-green-500' : 'text-gray-400'} />
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm text-gray-800 truncate font-medium">
            {message.fileInfo?.fileName || message.content}
          </p>
          <p className="text-xs text-gray-400">
            {message.fileInfo?.fileSize ? formatFileSize(message.fileInfo.fileSize) : ''}
          </p>
        </div>
        {isCompleted && isSentByMe && (
          <FiCheck size={14} className="text-green-500 shrink-0" />
        )}
      </div>

      {/* File receive confirmation - embedded in message */}
      {isWaiting && pendingReq && (
        <div className="mt-2 p-2 bg-amber-50 rounded border border-amber-200">
          <div className="flex items-center gap-2 mb-2">
            <FiDownload size={14} className="text-amber-600" />
            <span className="text-xs text-amber-700 font-medium">
              对方发送了一个文件，是否接收？
            </span>
          </div>
          <div className="flex items-center gap-2 text-xs text-gray-500 mb-2">
            <span>{message.fileInfo?.fileName}</span>
            {message.fileInfo?.fileSize ? <span>({formatFileSize(message.fileInfo.fileSize)})</span> : null}
          </div>
          <div className="flex gap-2">
            <button
              className="px-3 py-1 text-xs text-white bg-primary-500 rounded hover:bg-primary-600 transition-colors"
              onClick={() => onAccept(pendingReq.id)}
            >
              接收
            </button>
            <button
              className="px-3 py-1 text-xs text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
              onClick={() => onReject(pendingReq.id)}
            >
              拒绝
            </button>
          </div>
        </div>
      )}

      {/* Progress bar for transferring */}
      {isTransferring && (
        <div className="mt-2">
          <div className="w-full h-1.5 bg-gray-200 rounded-full overflow-hidden">
            <div
              className="h-full bg-primary-500 rounded-full transition-all duration-300"
              style={{ width: `${progress}%` }}
            />
          </div>
          <span className="text-xs text-gray-400">{progress}%</span>
        </div>
      )}

      {/* Transfer completed - show save path and "open folder" link */}
      {isCompleted && !isSentByMe && message.fileInfo?.filePath && (
        <div className="mt-2 space-y-1">
          <div className="flex items-center gap-1">
            <FiCheck size={12} className="text-green-500" />
            <span className="text-xs text-green-600">接收成功</span>
          </div>
          <div className="text-xs text-gray-400 truncate" title={message.fileInfo.filePath}>
            保存至：{message.fileInfo.filePath}
          </div>
          <button
            className="text-xs text-primary-500 hover:text-primary-600 flex items-center gap-0.5"
            onClick={() => openFolder(message.fileInfo!.filePath!)}
          >
            <FiFolder size={10} />
            打开文件夹
          </button>
        </div>
      )}

      {/* Sent successfully for self-sent files */}
      {isCompleted && isSentByMe && (
        <div className="mt-1 flex items-center gap-1">
          <FiCheck size={12} className="text-green-500" />
          <span className="text-xs text-green-600">发送成功</span>
        </div>
      )}

      {isFailed && (
        <div className="mt-1 flex items-center gap-1">
          <FiAlertCircle size={12} className="text-red-500" />
          <span className="text-xs text-red-500">传输失败</span>
        </div>
      )}
    </div>
  );
}

// Helper to check if message is from self (needed inside component functions)
function isSelf(message: Message): boolean {
  return message.from === 'self';
}

// ============================================================================
// Utility functions
// ============================================================================

function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();

  if (isToday) {
    return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  }
  return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' }) + ' ' +
    date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
}

/** Open the containing folder of a file */
function openFolder(filePath: string) {
  // Use shell command to open the folder and select the file
  // In Tauri, we'd need to invoke a backend command
  // For now, just copy path to clipboard and alert
  try {
    // Try to use the backend to open folder
    invoke('file.open_folder', { path: filePath }).catch(() => {
      // Fallback: copy to clipboard
      navigator.clipboard.writeText(filePath).then(() => {
        alert(`文件路径已复制到剪贴板：${filePath}`);
      }).catch(() => {
        alert(`文件保存位置：${filePath}`);
      });
    });
  } catch {
    alert(`文件保存位置：${filePath}`);
  }
}
