import React, { useEffect, useState } from 'react';
import LeftSidebar, { type ViewMode } from './components/LeftSidebar';
import UserListPanel from './components/UserListPanel';
import ChatPanel from './components/ChatPanel';
import Settings from './components/Settings';
import ToastHost from './components/Toast';
import FileSendPreview from './components/FileSendPreview';
import { useFileSelectionStore } from './stores/fileSelectionStore';
import type { FileSelection } from './types';
import { useUserStore } from './stores/userStore';
import { useMessageStore } from './stores/messageStore';
import { useConfigStore } from './stores/configStore';
import { useScanStore } from './stores/scanStore';
import { toast } from './stores/toastStore';
import { bridgeReady, invoke, isMockMode, listen } from './services/bridge';
import { watchNotificationActivation } from './services/notificationActivation';

export default function App() {
  const [viewMode, setViewMode] = useState<ViewMode>('chat');
  const [ready, setReady] = useState(false);
  const [startupError, setStartupError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const currentUser = useUserStore((s) => s.currentUser);
  useEffect(() => {
    if (!ready) return;
    return watchNotificationActivation((user) => { useUserStore.getState().setCurrentUser(user); setViewMode('chat'); });
  }, [ready]);
  useEffect(() => listen('notification.system_failed', (data: { error?: string }) => toast.error('系统通知失败：' + (data.error || '请检查 Windows 通知设置'))), []);

  useEffect(() => {
    const prevent = (event: DragEvent) => event.preventDefault();
    const drop = (event: DragEvent) => {
      event.preventDefault();
      if (isMockMode) toast.info('拖放文件需要桌面版');
    };
    window.addEventListener('dragover', prevent);
    window.addEventListener('drop', drop);
    return () => { window.removeEventListener('dragover', prevent); window.removeEventListener('drop', drop); };
  }, []);
  useEffect(() => listen('file.selected', (data: { target: string; files?: FileSelection[]; error?: string }) => {
    if (data.error) toast.error(data.error);
    else if (data.files?.length) useFileSelectionStore.getState().picked(data.target, data.files);
  }), []);
  useEffect(() => listen('notification.sound_failed', (data: { error?: string }) => {
    toast.error('提示音播放失败：' + (data.error || '请在设置中试听并检查音频设备'));
  }), []);

  useEffect(() => {
    let cancelled = false;
    setReady(false);
    setStartupError('');
    const unlistenUsers = useUserStore.getState().initListeners();
    const unlistenMessages = useMessageStore.getState().initListeners();
    const unlistenScans = useScanStore.getState().initListeners();
    const start = async () => {
      try {
        await bridgeReady();
        if (cancelled) return;
        await useConfigStore.getState().loadConfig();
        const storageError = useConfigStore.getState().info?.storageError;
        if (storageError) toast.error('数据目录切换失败，仍使用原目录：' + storageError);
        if (cancelled) return;
        await useMessageStore.getState().loadLocalUserId();
        if (cancelled) return;
        await useUserStore.getState().loadUsers();
        if (cancelled) return;
        // Rust treats this as idempotent. Discovery cannot run before listeners exist.
        await invoke('config.loaded');
        void useScanStore.getState().refresh();
        if (cancelled) return;
        await useMessageStore.getState().loadRecentConversations();
        if (!cancelled) setReady(true);
      } catch (error) {
        console.error('[App] Rust startup failed', error);
        if (!cancelled) setStartupError(error instanceof Error ? error.message : String(error));
      }
    };
    void start();
    return () => { cancelled = true; unlistenUsers(); unlistenMessages(); unlistenScans(); };
  }, [attempt]);

  const activeConversation = ready && viewMode !== 'settings' ? currentUser?.id || '' : '';
  useEffect(() => {
    useMessageStore.getState().setActiveConversation(activeConversation);
    if (!ready) return;
    void invoke('window.set_active_conversation', { userId: activeConversation })
      .catch((error) => console.error('[App] Active conversation update failed', error));
    const onFocus = () => {
      if (activeConversation) useMessageStore.getState().clearUnread(activeConversation);
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [ready, activeConversation]);

  if (!ready) {
    return (
      <div className="h-screen w-screen flex items-center justify-center bg-list-bg px-8">
        <div className="max-w-xl text-center">
          <h1 className="text-lg font-semibold text-gray-800">迅秋 Rust 预览版</h1>
          <p className="text-sm mt-4 text-gray-500 whitespace-pre-wrap break-words">
            {startupError || '正在连接 Rust 后端并加载独立数据…'}
          </p>
          {startupError && <button className="mt-5 px-4 py-2 rounded bg-primary-500 text-white text-sm" onClick={() => setAttempt((n) => n + 1)}>重试连接</button>}
        </div>
        <ToastHost />
      </div>
    );
  }
  return (
    <div className="flex flex-col h-screen w-screen bg-gray-100">
      {isMockMode && <div className="shrink-0 bg-amber-100 text-amber-900 text-xs px-4 py-1">浏览器演示模式：不进行网络收发，不保存到磁盘</div>}
      <div className="flex flex-1 min-h-0">
        <LeftSidebar viewMode={viewMode} onViewChange={setViewMode} />
        <UserListPanel viewMode={viewMode} onViewChange={setViewMode} />
        {viewMode === 'settings' ? <Settings onClose={() => setViewMode('chat')} /> : currentUser ? (
          <ChatPanel key={currentUser.id} />
        ) : (
          <div className="flex-1 flex items-center justify-center bg-chat-bg">
            <div className="text-center text-gray-400">
              <p className="text-lg">选择一个用户开始聊天</p>
              <p className="text-xs mt-3">Rust 核心版支持文本、表情、历史与托盘</p>
            </div>
          </div>
        )}
      </div>
      <ToastHost />
      <FileSendPreview />
    </div>
  );
}
