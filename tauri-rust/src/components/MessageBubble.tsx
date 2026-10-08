import React, { memo, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { FiAlertCircle, FiCheck, FiClock, FiImage, FiX } from 'react-icons/fi';
import type { ImageMetadata, ImageReadResult, Message } from '../types';
import { parseEmojiId, EMOJI_TOKEN_RE } from '../emojiData';
import { formatTime, formatFileSize } from '../utils/format';
import { invoke } from '../services/bridge';
import EmojiSprite from './EmojiSprite';
import { useMessageStore } from '../stores/messageStore';
import { toast } from '../stores/toastStore';
import FileBubble from './FileBubble';

function ImageSendState({ message }: { message: Message }) {
  const [busy, setBusy] = useState(false);
  if (message.status === 'failed') return <p className="text-xs text-red-600 mt-1">{message.imageError || '图片发送失败或已中断'}</p>;
  if (message.status !== 'sending') return null;
  const labels: Record<string, string> = { queued: '排队中', encoding: '正在编码', transferring: '传输中', waiting_reference: '等待引用确认', cancelling: '正在取消' };
  const cancel = async () => {
    if (busy) return; setBusy(true);
    try {
      if (!await useMessageStore.getState().cancelImage(message.id)) toast.info('图片已结束或正在保存最终状态，不能再取消');
    } catch (error) { toast.error('取消图片失败：' + String(error)); }
    finally { setBusy(false); }
  };
  return <div className="flex items-center gap-3 text-xs text-gray-500 mt-1">
    <span>{labels[message.imageStage || ''] || '发送中'} · {message.imageProgress || 0}%</span>
    <button disabled={busy || message.imageStage === 'cancelling'} className="text-primary-600 disabled:opacity-50" onClick={cancel}>取消</button>
  </div>;
}


function renderText(content: string) {
  const emojiId = parseEmojiId(content);
  if (emojiId) return <EmojiSprite id={emojiId} size={18} />;
  const parts: React.ReactNode[] = [];
  let last = 0, match: RegExpExecArray | null;
  EMOJI_TOKEN_RE.lastIndex = 0;
  while ((match = EMOJI_TOKEN_RE.exec(content))) {
    if (match.index > last) parts.push(content.slice(last, match.index));
    parts.push(<EmojiSprite key={`${match.index}:${match[1]}`} id={match[1]} size={18} />);
    last = match.index + match[0].length;
  }
  if (last < content.length) parts.push(content.slice(last));
  return <span className="whitespace-pre-wrap break-words">{parts}</span>;
}

// Fetch only an application-owned asset URL. Do not cache failures or arbitrary
// peer paths; retries and switching messages must be able to load a fresh URL.
function useImageUrl(assetId: string, thumbnail: boolean) {
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{ key: string; url: string | null; error: string | null }>({ key: '', url: null, error: null });
  const key = `${assetId}:${thumbnail}`;
  useEffect(() => {
    let cancelled = false;
    setState({ key, url: null, error: null });
    invoke<ImageReadResult>('image.read', { assetId, thumbnail })
      .then((result) => {
        if (!result.success || typeof result.url !== 'string' || !result.url) throw new Error('后端未返回图片地址');
        if (!cancelled) setState({ key, url: result.url, error: null });
      })
      .catch((error) => {
        if (!cancelled) setState({ key, url: null, error: error instanceof Error ? error.message : String(error) });
      });
    return () => { cancelled = true; };
  }, [assetId, thumbnail, key, attempt]);
  return {
    url: state.key === key ? state.url : null,
    error: state.key === key ? state.error : null,
    retry: () => setAttempt((value) => value + 1),
    failed: () => setState({ key, url: null, error: '图片加载失败，文件可能已被移动或删除' }),
  };
}

function ImageViewer({ image, onClose }: { image: ImageMetadata; onClose: () => void }) {
  const { url, error, retry, failed } = useImageUrl(image.assetId, false);
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement;
    dialogRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault(); event.stopPropagation(); onClose();
      } else if (event.key === 'Tab') {
        const buttons = [...(dialogRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') || [])];
        if (!buttons.length) return;
        const index = buttons.findIndex((button) => button === document.activeElement);
        const next = event.shiftKey ? (index <= 0 ? buttons.length - 1 : index - 1) : (index + 1) % buttons.length;
        event.preventDefault(); event.stopPropagation(); buttons[next].focus();
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [onClose]);
  return createPortal(
    <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4" onClick={onClose}>
      <div ref={dialogRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label={`查看图片：${image.fileName}`}
        className="relative max-w-[95vw] max-h-[95vh] bg-white rounded-lg shadow-xl flex flex-col outline-none"
        onClick={(event) => event.stopPropagation()}>
        <div className="flex items-center gap-4 px-4 py-2 border-b min-w-0">
          <span className="text-sm text-gray-700 truncate flex-1">{image.fileName}</span>
          <button type="button" title="关闭（Esc）" aria-label="关闭图片" className="p-1 text-gray-500 hover:text-gray-800" onClick={onClose}><FiX size={20} /></button>
        </div>
        <div className="overflow-auto min-h-[120px] min-w-[200px] flex items-center justify-center p-2">
          {url ? <img src={url} alt={image.fileName} className="max-w-[90vw] max-h-[78vh] object-contain" onError={failed} /> :
            error ? <div className="p-6 text-center text-sm text-gray-500" role="alert">
              <p>{error}</p><button type="button" className="mt-3 text-primary-600 hover:underline" onClick={retry}>重新加载原图</button>
            </div> : <p className="p-6 text-sm text-gray-400">正在读取原图…</p>}
        </div>
        <div className="px-4 py-2 text-xs text-gray-400 border-t">{image.width} × {image.height} · {formatFileSize(image.fileSize)}</div>
      </div>
    </div>, document.body,
  );
}

function ReceivedImage({ image }: { image: ImageMetadata }) {
  const { url, error, retry, failed } = useImageUrl(image.assetId, true);
  const [open, setOpen] = useState(false);
  // A stable callback avoids reinstalling the viewer's focus/keyboard effect on
  // unrelated message-store renders.
  const close = React.useCallback(() => setOpen(false), []);
  return (
    <div className="min-w-0">
      {url ? <button type="button" onClick={() => setOpen(true)} title="查看原图" className="block max-w-full rounded overflow-hidden hover:opacity-90">
        <img src={url} alt={image.fileName} loading="lazy" className="block max-w-full max-h-60 object-contain" onError={failed} />
      </button> : error ? <div className="text-sm text-gray-500" role="alert">
        <p><FiAlertCircle className="inline mr-1" />图片预览加载失败</p>
        <p className="text-xs mt-1 break-words">{error}</p>
        <div className="flex gap-3 mt-2">
          <button type="button" className="text-primary-600 hover:underline" onClick={retry}>重试缩略图</button>
          <button type="button" className="text-primary-600 hover:underline" onClick={() => setOpen(true)}>查看原图</button>
        </div>
      </div> : <p className="text-gray-400 py-3"><FiImage className="inline mr-1" />正在读取图片…</p>}
      <p className="text-[11px] text-gray-400 mt-1 truncate" title={image.fileName}>{image.width} × {image.height} · {formatFileSize(image.fileSize)}</p>
      {open && <ImageViewer image={image} onClose={close} />}
    </div>
  );
}

export default memo(function MessageBubble({ message }: { message: Message }) {
  const self = message.from === 'self';
  return (
    <div className={`flex ${self ? 'justify-end' : 'justify-start'} message-in`}>
      <div className="max-w-[70%] min-w-0">
        <div className={`px-3 py-2 rounded-lg text-sm leading-relaxed break-words ${self ? 'bg-chat-sent text-gray-800 rounded-tr-none' : 'bg-chat-received text-gray-800 rounded-tl-none shadow-sm'}`}>
          {message.type === 'text' ? renderText(message.content) : message.type === 'image' ? (
            message.image?.assetId ? <ReceivedImage image={message.image} /> : <span className="text-gray-500">[图片信息缺失，无法预览]</span>
          ) : <FileBubble message={message} />}
        </div>
        {self && message.type === 'image' && <ImageSendState message={message} />}
        <div className={`flex items-center gap-1 text-[10px] text-gray-400 mt-0.5 ${self ? 'justify-end' : 'justify-start'}`}>
          <span>{formatTime(message.timestamp)}</span>
          {self && message.type !== 'file' && (message.status === 'sending'
            ? <FiClock size={10} title="等待对方确认" />
            : message.status === 'delivered'
            ? <FiCheck size={10} className="text-green-500" title="对方已收到" />
            : <FiAlertCircle size={10} className="text-red-500" title="发送失败或确认超时" />)}
        </div>
      </div>
    </div>
  );
});
