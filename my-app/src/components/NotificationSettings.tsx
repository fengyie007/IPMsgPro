import React, { useEffect, useRef, useState } from 'react';
import { previewNotificationSound } from '../services/notification';
import { toast } from '../stores/toastStore';

export default function NotificationSettings({ enabled, supported, disabled, onChange }: {
  enabled: boolean; supported: boolean; disabled: boolean; onChange: (enabled: boolean) => void;
}) {
  const [testing, setTesting] = useState(false);
  const busy = useRef(false), mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const preview = async () => {
    if (busy.current || disabled || !supported) return;
    busy.current = true; setTesting(true);
    try { await previewNotificationSound(); }
    catch (error) { if (mounted.current) toast.error('试听失败：' + String(error)); }
    finally { busy.current = false; if (mounted.current) setTesting(false); }
  };
  return <section className="space-y-3">
    <h3 className="text-sm font-semibold text-gray-700">新消息提示音</h3>
    <div className="flex items-center gap-4">
      <label className="flex items-center gap-2 text-sm text-gray-600"><input type="checkbox" checked={enabled} disabled={disabled || !supported}
        onChange={(event) => onChange(event.target.checked)} />播放新消息提示音</label>
      <button disabled={disabled || !supported || testing} onClick={() => void preview()} className="px-3 py-1.5 text-sm rounded border border-gray-200 text-gray-600 hover:bg-gray-50 disabled:opacity-40">{testing ? '准备试听…' : '试听'}</button>
    </div>
    <p className="text-xs text-gray-400">保存设置后生效。前台正在查看当前会话时静音；其他新文字、图片或文件邀请会提示。连续消息会合并提示，不叠加播放。</p>
    <p className="text-xs text-gray-400">试听不会修改开关或保存设置。{!supported && '当前环境不提供原生提示音，请使用 Windows 桌面版。'}</p>
  </section>;
}
