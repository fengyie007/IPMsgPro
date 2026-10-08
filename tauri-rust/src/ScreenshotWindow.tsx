import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ScreenshotEditor from './components/ScreenshotEditor';

// This window intentionally never initializes App, message stores, or the main bridge.
export default function ScreenshotWindow({ sessionId }: { sessionId: string }) {
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const cancelled = useRef(false);
  const cancel = useCallback((reason?: string) => {
    cancelled.current = true;
    void invoke('screenshot_command', { sessionId, command: 'cancel', error: reason || null })
      .catch((failure) => setError('关闭截图失败：' + String(failure)));
  }, [sessionId]);
  const failure = useCallback((reason: string) => cancel(reason), [cancel]);
  const ready = useCallback(() => {
    if (!cancelled.current) void invoke('screenshot_command', { sessionId, command: 'ready' }).catch((e) => failure(String(e)));
  }, [sessionId, failure]);
  useEffect(() => {
    let disposed = false;
    invoke<{ url: string }>('screenshot_command', { sessionId, command: 'read' })
      .then((result) => { if (!disposed) setUrl(result.url); })
      .catch((e) => { if (!disposed) failure(String(e)); });
    return () => { disposed = true; };
  }, [sessionId, failure]);
  const confirm = useCallback(async (blob: Blob) => {
    if (cancelled.current) return;
    setBusy(true);
    try {
      const bytes = new Uint8Array(await blob.arrayBuffer());
      if (!cancelled.current) await invoke('screenshot_confirm', bytes, { headers: { 'x-capture-session': sessionId } });
    } finally { setBusy(false); }
  }, [sessionId]);
  const onCancel = useCallback(() => cancel(), [cancel]);
  if (error) return <div className="h-screen bg-neutral-900 text-white p-8"><p role="alert">{error}</p><button onClick={onCancel} className="mt-4">重试关闭</button></div>;
  return url ? <ScreenshotEditor image={url} busy={busy} onReady={ready} onCancel={onCancel} onConfirm={confirm} onFailure={failure} />
    : <div className="h-screen bg-neutral-900 text-white p-8">正在准备截图…</div>;
}
