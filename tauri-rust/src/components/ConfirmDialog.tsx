import React, { useEffect, useId, useRef } from 'react';

interface ConfirmDialogProps {
  title: string;
  message?: string;
  confirmText?: string;
  cancelText?: string;
  /** Style the confirm button as destructive (red) */
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/** Esc cancels; Enter activates the focused button. Keep focus inside the dialog. */
export default function ConfirmDialog({
  title, message, confirmText = '确定', cancelText = '取消', danger = false, onConfirm, onCancel,
}: ConfirmDialogProps) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const previousFocus = useRef(document.activeElement);
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); onCancel(); }
      if (e.key === 'Tab') {
        const buttons = panel.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)');
        if (!buttons?.length) return;
        const first = buttons[0], last = buttons[buttons.length - 1];
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [onConfirm, onCancel]);
  useEffect(() => () => {
    const previous = previousFocus.current;
    if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
  }, []);

  return (
    <div className="fixed inset-0 bg-black/40 z-50 flex items-center justify-center" onClick={onCancel}>
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-white rounded-lg shadow-xl w-[360px] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3 border-b">
          <h3 id={titleId} className="text-sm font-medium text-gray-800">{title}</h3>
        </div>
        {message && (
          <div className="px-4 py-3 text-sm text-gray-600 break-words whitespace-pre-wrap">{message}</div>
        )}
        <div className="flex justify-end gap-2 px-4 py-3 border-t">
          <button
            className="px-4 py-1.5 text-sm text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
            onClick={onCancel}
          >
            {cancelText}
          </button>
          <button
            autoFocus
            className={`px-4 py-1.5 text-sm text-white rounded transition-colors ${
              danger ? 'bg-red-500 hover:bg-red-600' : 'bg-primary-500 hover:bg-primary-600'
            }`}
            onClick={onConfirm}
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}
