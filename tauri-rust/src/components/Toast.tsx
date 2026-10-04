import React from 'react';
import { FiAlertCircle, FiCheckCircle, FiInfo, FiX } from 'react-icons/fi';
import { useToastStore, ToastKind } from '../stores/toastStore';

const STYLES: Record<ToastKind, { box: string; icon: React.ReactNode }> = {
  info: { box: 'bg-gray-800 text-white', icon: <FiInfo size={16} /> },
  success: { box: 'bg-green-600 text-white', icon: <FiCheckCircle size={16} /> },
  error: { box: 'bg-red-600 text-white', icon: <FiAlertCircle size={16} /> },
};

/** Renders the toast stack. Mount once, near the app root. */
export default function ToastHost() {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);
  if (toasts.length === 0) return null;

  return (
    <div className="fixed top-4 left-1/2 -translate-x-1/2 z-[100] flex flex-col gap-2 pointer-events-none">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === 'error' ? 'alert' : 'status'}
          className={`pointer-events-auto flex items-center gap-2 px-4 py-2 rounded-lg shadow-lg text-sm max-w-[480px] message-in ${STYLES[t.kind].box}`}
        >
          <span className="shrink-0">{STYLES[t.kind].icon}</span>
          <span className="break-all">{t.message}</span>
          <button
            className="ml-1 shrink-0 opacity-70 hover:opacity-100"
            onClick={() => dismiss(t.id)}
            title="关闭"
          >
            <FiX size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
