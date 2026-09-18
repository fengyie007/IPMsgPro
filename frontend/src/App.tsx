import React, { useEffect, useState } from 'react';
import LeftSidebar, { ViewMode } from './components/LeftSidebar';
import UserListPanel from './components/UserListPanel';
import ChatPanel from './components/ChatPanel';
import Settings from './components/Settings';
import { useUserStore } from './stores/userStore';
import { useMessageStore } from './stores/messageStore';
import { useConfigStore } from './stores/configStore';
import { invoke, listen } from './services/bridge';

function App() {
  const [viewMode, setViewMode] = useState<ViewMode>('chat');
  const loadUsers = useUserStore((s) => s.loadUsers);
  const discoverUsers = useUserStore((s) => s.discoverUsers);
  const initUserListeners = useUserStore((s) => s.initListeners);
  const initMessageListeners = useMessageStore((s) => s.initListeners);
  const loadConfig = useConfigStore((s) => s.loadConfig);
  const currentUser = useUserStore((s) => s.currentUser);
  const loadLocalUserId = useMessageStore((s) => s.loadLocalUserId);
  const loadRecentConversations = useMessageStore((s) => s.loadRecentConversations);

  console.log('[App] Rendering, initMessageListeners type:', typeof initMessageListeners);

  // Debug: log effect registration
  useEffect(() => {
    console.log('[App] useEffect START: registering listeners...');
    console.log('[App] initMessageListeners defined');
    // Register event listeners FIRST - these must be outside async init
    if (typeof initUserListeners !== 'function') {
      console.error('[App] initUserListeners is NOT a function!');
    }
    if (typeof initMessageListeners !== 'function') {
      console.error('[App] initMessageListeners is NOT a function!');
    }
    const unsubUsers = initUserListeners();
    const unsubMessages = initMessageListeners();
    console.log('[App] Listeners registered successfully');

    // Register scanner event listeners
    const unsubScanProgress = listen('network.scan_progress', (data: any) => {
      console.log('[App] Scan progress:', data);
      // Could show toast or update a global scan state
    });
    const unsubScanComplete = listen('network.scan_complete', (data: any) => {
      console.log('[App] Scan complete:', data);
      // Could show toast notification
    });

    // Then do async init: load config, users, discover
    const init = async () => {
      try {
        await loadConfig();
        // Notify backend that config is loaded, so it can send BR_ENTRY to direct users
        await invoke('config.loaded');
        
        // Auto-scan IP ranges if configured
        const config = useConfigStore.getState().config;
        if (config.ipScanRanges && config.ipScanRanges.length > 0) {
          try {
            await invoke('network.scan_range', { ranges: config.ipScanRanges });
          } catch (e) {
            console.error('[App] Auto-scan failed:', e);
          }
        }
        
        await loadUsers();
        await discoverUsers();
        await loadLocalUserId();
        await loadRecentConversations();  // Load recent conversations for chat list (after localUserId is ready)
      } catch (err: any) {
        console.error('[App] Init failed:', err);
        // Report error to backend for debugging
        try {
          await invoke('frontend.error', { message: err.message, stack: err.stack });
        } catch {}
      }
    };

    init();

    return () => {
      unsubUsers();
      unsubMessages();
      unsubScanProgress();
      unsubScanComplete();
      unsubUsers();
      unsubMessages();
    };
  }, []);

  // Only remount ChatPanel when the active conversation (user) changes.
  // Progress updates bump messageVersion, but we must NOT remount here,
  // otherwise useEffect(loadHistory) re-runs and replaces in-memory transfer
  // progress (causing the whole chat to flicker and progress to reset to 0%).
  const chatKey = currentUser ? currentUser.id : 'empty';

  return (
    <div className="flex h-screen w-screen bg-gray-100">
      <LeftSidebar viewMode={viewMode} onViewChange={setViewMode} />
      <UserListPanel viewMode={viewMode} onViewChange={setViewMode} />
      {viewMode === 'settings' ? (
        <Settings onClose={() => setViewMode('chat')} />
      ) : currentUser ? (
        <ChatPanel key={chatKey} />
      ) : (
        <EmptyChatView />
      )}
    </div>
  );
}

function EmptyChatView() {
  return (
    <div className="flex-1 flex items-center justify-center bg-white">
      <div className="text-center text-gray-400">
        <svg className="mx-auto mb-4 w-16 h-16" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
            d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
        </svg>
        <p className="text-lg">选择一个用户开始聊天</p>
      </div>
    </div>
  );
}

export default App;
