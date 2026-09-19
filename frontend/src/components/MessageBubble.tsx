import React, { memo, useEffect, useState } from 'react';
import { FiImage, FiFile, FiCheck, FiAlertCircle, FiDownload, FiFolder, FiClock } from 'react-icons/fi';
import { PendingFileReceive } from '../stores/messageStore';
import { toast } from '../stores/toastStore';
import { Message } from '../types';
import { invoke } from '../services/bridge';
import { parseEmojiId, EMOJI_TOKEN_RE } from '../emojiData';
import { formatTime, formatFileSize } from '../utils/format';
import EmojiSprite from './EmojiSprite';

// ============================================================================
// MessageBubble - one chat message (text / image / file)
// ============================================================================

export interface MessageBubbleProps {
  message: Message;
  /** The receive request that belongs to THIS message, if it is still pending. */
  pendingRequest?: PendingFileReceive;
  onAccept: (requestId: string) => void;
  onReject: (requestId: string) => void;
}

/**
 * Memoized: the parent passes only this bubble's own pending request (not the
 * whole pending list), so a progress tick or a new request elsewhere in the
 * conversation does not re-render every bubble.
 */
const MessageBubble = memo(function MessageBubble({ message, pendingRequest, onAccept, onReject }: MessageBubbleProps) {
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
          {renderContent(message, pendingRequest, onAccept, onReject)}
        </div>
        <div className={`flex items-center gap-1 text-[10px] text-gray-400 mt-0.5 ${isSelf ? 'justify-end' : 'justify-start'}`}>
          <span>{formatTime(message.timestamp)}</span>
          {isSelf && message.type === 'text' && <TextStatusIcon status={message.status} />}
        </div>
      </div>
    </div>
  );
});

export default MessageBubble;

// Delivery state of our own text messages (files have their own indicators).
function TextStatusIcon({ status }: { status: Message['status'] }) {
  switch (status) {
    case 'sending':
      return <FiClock size={10} className="text-gray-400" title="已发送，等待对方确认" />;
    case 'delivered':
      return <FiCheck size={10} className="text-green-500" title="对方已收到" />;
    case 'failed':
      return <FiAlertCircle size={10} className="text-red-500" title="发送失败" />;
    default:
      return null;
  }
}

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
  pendingRequest: PendingFileReceive | undefined,
  onAccept: (requestId: string) => void,
  onReject: (requestId: string) => void
) {
  switch (message.type) {
    case 'image':
      return <ImageContent message={message} pendingRequest={pendingRequest} onAccept={onAccept} onReject={onReject} />;
    case 'file':
      return <FileContent message={message} pendingRequest={pendingRequest} onAccept={onAccept} onReject={onReject} />;
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

// ---- Transfer state derived from a file/image message ----
interface TransferState {
  isWaiting: boolean;      // waiting for the local user to accept
  isTransferring: boolean; // 0..99 %
  isCompleted: boolean;
  isFailed: boolean;
  progress: number | undefined;
}

function transferState(message: Message): TransferState {
  const progress = message.transferProgress;
  return {
    progress,
    isWaiting: progress === -1,
    isTransferring: progress !== undefined && progress !== -1 && progress >= 0 && progress < 100,
    isCompleted: progress === 100 || (message.status === 'delivered' && !!message.fileInfo?.filePath),
    isFailed: message.status === 'failed',
  };
}

interface ContentProps {
  message: Message;
  pendingRequest?: PendingFileReceive;
  onAccept: (requestId: string) => void;
  onReject: (requestId: string) => void;
}

// ---- Accept / reject prompt embedded in an incoming file or image bubble ----
function FileReceivePrompt({ kind, message, request, onAccept, onReject }: {
  kind: 'image' | 'file';
  message: Message;
  request: PendingFileReceive;
  onAccept: (requestId: string) => void;
  onReject: (requestId: string) => void;
}) {
  return (
    <div className="mt-2 p-2 bg-amber-50 rounded border border-amber-200">
      <div className="flex items-center gap-2 mb-2">
        <FiDownload size={14} className="text-amber-600" />
        <span className="text-xs text-amber-700 font-medium">
          {kind === 'image' ? '对方发送了一张图片，是否接收？' : '对方发送了一个文件，是否接收？'}
        </span>
      </div>
      <div className="flex items-center gap-2 text-xs text-gray-500 mb-2">
        <span>{message.fileInfo?.fileName}</span>
        {message.fileInfo?.fileSize ? <span>({formatFileSize(message.fileInfo.fileSize)})</span> : null}
      </div>
      <div className="flex gap-2">
        <button
          className="px-3 py-1 text-xs text-white bg-primary-500 rounded hover:bg-primary-600 transition-colors"
          onClick={() => onAccept(request.id)}
        >
          接收
        </button>
        <button
          className="px-3 py-1 text-xs text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
          onClick={() => onReject(request.id)}
        >
          拒绝
        </button>
      </div>
    </div>
  );
}

function ProgressBar({ progress, thick = false }: { progress: number; thick?: boolean }) {
  return (
    <div className={thick ? 'mt-2' : 'mt-1'}>
      <div className={`w-full ${thick ? 'h-1.5' : 'h-1'} bg-gray-200 rounded-full overflow-hidden`}>
        <div
          className="h-full bg-primary-500 rounded-full transition-all duration-300"
          style={{ width: `${progress}%` }}
        />
      </div>
      <span className="text-xs text-gray-400">{progress}%</span>
    </div>
  );
}

function FailedBadge() {
  return (
    <div className="mt-1 flex items-center gap-1">
      <FiAlertCircle size={12} className="text-red-500" />
      <span className="text-xs text-red-500">传输失败</span>
    </div>
  );
}

function SentBadge() {
  return (
    <div className="mt-1 flex items-center gap-1">
      <FiCheck size={12} className="text-green-500" />
      <span className="text-xs text-green-600">发送成功</span>
    </div>
  );
}

// ---- Image message with thumbnail + progress ----
// Thumbnails are loaded lazily from the local file through the backend
// (file.read_image) and cached per path, so switching conversations does not
// re-read the same files. Bounded so the cache cannot grow without limit.
const thumbnailCache = new Map<string, string | null>();
const THUMBNAIL_CACHE_MAX = 200;

function useThumbnail(localPath: string | undefined, enabled: boolean): string | null {
  const [dataUrl, setDataUrl] = useState<string | null>(
    localPath ? thumbnailCache.get(localPath) ?? null : null
  );
  useEffect(() => {
    if (!localPath || !enabled) return;
    const cached = thumbnailCache.get(localPath);
    if (cached !== undefined) { setDataUrl(cached); return; }
    let cancelled = false;
    invoke<{ success?: boolean; dataUrl?: string }>('file.read_image', { filePath: localPath })
      .then((r) => {
        const url = r && r.success && r.dataUrl ? r.dataUrl : null;
        if (thumbnailCache.size >= THUMBNAIL_CACHE_MAX) {
          thumbnailCache.delete(thumbnailCache.keys().next().value as string);
        }
        thumbnailCache.set(localPath, url);
        if (!cancelled) setDataUrl(url);
      })
      .catch(() => { if (!cancelled) setDataUrl(null); });
    return () => { cancelled = true; };
  }, [localPath, enabled]);
  return dataUrl;
}

function ImageContent({ message, pendingRequest, onAccept, onReject }: ContentProps) {
  const { progress, isWaiting, isTransferring, isCompleted, isFailed } = transferState(message);
  const isSentByMe = message.from === 'self';

  // Inline data URL (FeiQ inline screenshots) renders directly; otherwise load
  // a thumbnail from the local file once it exists: immediately for our own
  // sends, after completion for received files (the file is partial before).
  const inlineUrl = message.content.startsWith('data:') ? message.content : null;
  const localPath = message.fileInfo?.filePath;
  const canLoad = !inlineUrl && !!localPath && (isSentByMe || isCompleted);
  const thumbnail = useThumbnail(localPath, canLoad);
  const imageUrl = inlineUrl ?? thumbnail;

  const openOriginal = () => {
    if (localPath) invoke('shell_open', { url: localPath }).catch(() => {});
  };

  return (
    <div className="relative">
      {imageUrl ? (
        <img
          src={imageUrl}
          alt={message.fileInfo?.fileName || '图片'}
          title={localPath ? '点击查看原图' : undefined}
          className="max-w-full rounded cursor-pointer hover:opacity-90 transition-opacity"
          style={{ maxHeight: 200 }}
          onClick={openOriginal}
        />
      ) : (
        <div className="flex items-center gap-2 p-1">
          <FiImage size={20} className="text-primary-400 shrink-0" />
          <span className="text-sm text-gray-700">{message.fileInfo?.fileName || message.content}</span>
        </div>
      )}

      {isWaiting && pendingRequest && (
        <FileReceivePrompt kind="image" message={message} request={pendingRequest} onAccept={onAccept} onReject={onReject} />
      )}

      {isTransferring && progress !== undefined && <ProgressBar progress={progress} />}

      {isCompleted && localPath && !isSentByMe && (
        <div className="mt-1 flex items-center gap-1">
          <FiCheck size={12} className="text-green-500" />
          <span className="text-xs text-green-600">已保存</span>
          <button
            className="text-xs text-primary-500 hover:text-primary-600 flex items-center gap-0.5"
            onClick={() => openFolder(localPath)}
          >
            <FiFolder size={10} />
            打开文件夹
          </button>
        </div>
      )}

      {isCompleted && isSentByMe && <SentBadge />}
      {isFailed && <FailedBadge />}
    </div>
  );
}

// ---- File message with card + progress ----
function FileContent({ message, pendingRequest, onAccept, onReject }: ContentProps) {
  const { progress, isWaiting, isTransferring, isCompleted, isFailed } = transferState(message);
  const isSentByMe = message.from === 'self';
  const localPath = message.fileInfo?.filePath;

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

      {isWaiting && pendingRequest && (
        <FileReceivePrompt kind="file" message={message} request={pendingRequest} onAccept={onAccept} onReject={onReject} />
      )}

      {isTransferring && progress !== undefined && <ProgressBar progress={progress} thick />}

      {isCompleted && !isSentByMe && localPath && (
        <div className="mt-2 space-y-1">
          <div className="flex items-center gap-1">
            <FiCheck size={12} className="text-green-500" />
            <span className="text-xs text-green-600">接收成功</span>
          </div>
          <div className="text-xs text-gray-400 truncate" title={localPath}>
            保存至：{localPath}
          </div>
          <button
            className="text-xs text-primary-500 hover:text-primary-600 flex items-center gap-0.5"
            onClick={() => openFolder(localPath)}
          >
            <FiFolder size={10} />
            打开文件夹
          </button>
        </div>
      )}

      {isCompleted && isSentByMe && <SentBadge />}
      {isFailed && <FailedBadge />}
    </div>
  );
}

/** Open the containing folder of a file (Explorer with the file selected). */
function openFolder(filePath: string) {
  invoke<{ success?: boolean }>('file.open_folder', { path: filePath })
    .then((r) => {
      if (r && r.success === false) toast.error(`无法打开文件夹：${filePath}`);
    })
    .catch(() => {
      // Fallback: put the path on the clipboard so the user can still find it
      navigator.clipboard.writeText(filePath)
        .then(() => toast.info(`文件路径已复制到剪贴板：${filePath}`))
        .catch(() => toast.info(`文件保存位置：${filePath}`));
    });
}
