import React, { useEffect, useRef, useState } from 'react';
import { FiX } from 'react-icons/fi';
import type { ImageMetadata } from '../types';
import { formatFileSize } from '../utils/format';

export default function ImageSendPreview({ image, url, busy, onConfirm, onCancel }: {
  image: ImageMetadata; url: string; busy: boolean; onConfirm: () => void; onCancel: () => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const previous = document.activeElement;
    root.current?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if (event.key === 'Escape' || event.key === 'Enter') {
        event.preventDefault(); event.stopPropagation();
        if (!busy && event.key === 'Escape') onCancel();
        else if (!busy && loaded && !failed && event.key === 'Enter') onConfirm();
      } else if (event.key === 'Tab') {
        const buttons = [...(root.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') || [])];
        if (!buttons.length) { event.preventDefault(); return; }
        const index = buttons.findIndex((button) => button === document.activeElement);
        event.preventDefault(); buttons[event.shiftKey ? (index <= 0 ? buttons.length - 1 : index - 1) : (index + 1) % buttons.length].focus();
      }
    };
    window.addEventListener('keydown', key, true);
    return () => { window.removeEventListener('keydown', key, true); if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, [busy, loaded, failed, onConfirm, onCancel]);
  return <div className="fixed inset-0 z-50 bg-black/50 flex items-center justify-center p-4" onClick={onCancel}>
    <div ref={root} tabIndex={-1} role="dialog" aria-modal="true" aria-label="确认发送图片" className="w-[400px] max-w-full bg-white rounded-lg shadow-xl outline-none" onClick={(event) => event.stopPropagation()}>
      <div className="flex justify-between items-center border-b px-4 py-3"><span className="font-medium">发送图片</span><button disabled={busy} title="取消" onClick={onCancel}><FiX /></button></div>
      <div className="p-4">{failed ? <p role="alert" className="text-sm text-red-600">无法显示预览，请取消后重新选择图片。</p> : <img src={url} alt={image.fileName} onLoad={() => setLoaded(true)} onError={() => setFailed(true)} className="max-h-64 max-w-full mx-auto object-contain" />}<p className="text-sm truncate mt-3" title={image.fileName}>{image.fileName}</p><p className="text-xs text-gray-400">{image.width} × {image.height} · {formatFileSize(image.fileSize)} · 飞秋内嵌图片</p></div>
      <div className="flex justify-end gap-3 border-t px-4 py-3"><button disabled={busy} onClick={onCancel} className="text-sm text-gray-500">取消</button><button disabled={busy || !loaded || failed} onClick={onConfirm} className="text-sm px-4 py-1.5 bg-primary-500 text-white rounded disabled:opacity-50">{busy ? '创建任务…' : '发送'}</button></div>
    </div>
  </div>;
}
