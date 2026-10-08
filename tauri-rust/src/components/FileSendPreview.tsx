import React, { useEffect, useRef } from 'react';
import { useFileSelectionStore } from '../stores/fileSelectionStore';
import { useUserStore } from '../stores/userStore';
import { formatFileSize } from '../utils/format';
export default function FileSendPreview() {
  const { target, files, busy, send, cancel } = useFileSelectionStore();
  const user = useUserStore((s) => s.users.find((u) => u.id === target));
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!files.length) return;
    const previous = document.activeElement; root.current?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if (event.key === 'Escape') { event.preventDefault(); if (!busy) void cancel(); }
      if (event.key === 'Tab') {
        const buttons = [...(root.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') || [])];
        event.preventDefault(); const index = buttons.findIndex((b) => b === document.activeElement);
        buttons[event.shiftKey ? (index <= 0 ? buttons.length - 1 : index - 1) : (index + 1) % buttons.length]?.focus();
      }
    };
    window.addEventListener('keydown', key);
    return () => { window.removeEventListener('keydown', key); if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, [files.length, busy, cancel]);
  if (!files.length) return null;
  return <div className="fixed inset-0 z-50 bg-black/50 flex items-center justify-center p-5">
    <div ref={root} tabIndex={-1} role="dialog" aria-modal="true" aria-label="确认发送文件" className="bg-white rounded-lg shadow-xl w-96 max-w-full outline-none">
      <h2 className="p-4 border-b font-medium truncate">发送文件给 {user?.nickname || target}</h2>
      <ul className="px-4 py-3 space-y-3 max-h-64 overflow-y-auto">{files.map((file) => <li key={file.selectionId}><p className="text-sm break-all">{file.fileName}</p><p className="text-xs text-gray-400">{formatFileSize(file.fileSize)}</p></li>)}</ul>
      <p className="px-4 pb-3 text-xs text-gray-500">{files.some((file) => file.isDirectory) ? '包含文件夹；对方需支持 IPMsg 目录传输。' : '对方确认接收后开始传输。'}</p>
      <div className="border-t p-3 flex justify-end gap-4"><button disabled={busy} onClick={() => void cancel()} className="text-sm text-gray-500">取消</button><button disabled={busy} onClick={() => void send()} className="bg-primary-500 text-white rounded px-4 py-1.5 text-sm disabled:opacity-50">{busy ? '正在发送…' : '发送'}</button></div>
    </div>
  </div>;
}
