import { create } from 'zustand';
import { invoke, listen } from '../services/bridge';
import { useUserStore } from './userStore';
import { toast } from './toastStore';
import type { HistoryRecord, HistoryResult, Message, MessageReceivedEvent } from '../types';

type Delivery = { status: 'delivered' | 'failed'; error?: string; at: number };
const earlyDelivery = new Map<string, Delivery>();
const EARLY_LIMIT = 256;
const EARLY_TTL = 5 * 60 * 1000;
const PAGE_SIZE = 50;
const historyEpoch = new Map<string, number>();
const clearingUsers = new Set<string>();
const clearedIds = new Set<string>();
const unreadIds = new Map<string, Set<string>>();
const CLEARED_LIMIT = 4096;
let loadingRequests = 0;

function cacheDelivery(id: string, update: Delivery) {
  for (const [key, item] of earlyDelivery) {
    if (Date.now() - item.at > EARLY_TTL) earlyDelivery.delete(key);
  }
  if (earlyDelivery.has(id)) return;
  if (earlyDelivery.size >= EARLY_LIMIT) earlyDelivery.delete(earlyDelivery.keys().next().value!);
  earlyDelivery.set(id, update);
}

function fromHistory(row: HistoryRecord, localId: string): Message {
  return {
    id: row.id,
    from: row.fromId === localId ? 'self' : row.fromId,
    to: row.toId === localId ? 'self' : row.toId,
    content: row.content,
    type: row.type === 0 ? 'text' : row.type === 1 ? 'image' : 'file',
    timestamp: row.timestamp * 1000,
    status: row.status === 0 ? 'sending' : row.status === 1 || row.status === 2 ? 'delivered' : 'failed',
    image: row.image,
  };
}

function mergeHistory(existing: Message[], incoming: Message[]): Message[] {
  const byId = new Map(existing.filter((m) => !clearedIds.has(m.id)).map((m) => [m.id, m]));
  for (const message of incoming) {
    if (clearedIds.has(message.id)) continue;
    const current = byId.get(message.id);
    byId.set(message.id, current ? {
      ...message, ...current,
      status: current.status === 'sending' ? message.status : current.status,
      image: current.image ?? message.image,
    } : message);
  }
  return [...byId.values()].sort((a, b) => a.timestamp - b.timestamp || a.id.localeCompare(b.id));
}

interface MessageStore {
  messages: Map<string, Message[]>;
  unread: Map<string, number>;
  historyPages: Map<string, { offset: number; hasMore: boolean }>;
  localUserId: string;
  activeConversation: string;
  loading: boolean;
  error: string | null;
  loadLocalUserId: () => Promise<void>;
  sendMessage: (target: string, content: string) => Promise<boolean>;
  recvMessage: (message: Message) => void;
  updateDelivery: (id: string, status: Delivery['status'], error?: string) => void;
  clearUnread: (userId: string) => void;
  setActiveConversation: (userId: string) => void;
  loadHistory: (userId: string) => Promise<void>;
  loadMoreHistory: (userId: string) => Promise<void>;
  loadRecentConversations: () => Promise<void>;
  searchMessages: (keyword: string, userId?: string) => Promise<Message[]>;
  clearHistory: (userId?: string) => Promise<void>;
  initListeners: () => () => void;
}

export const useMessageStore = create<MessageStore>((set, get) => {
  const replay = (messages: Message[]) => {
    for (const message of messages) {
      const update = earlyDelivery.get(message.id);
      if (!update) continue;
      earlyDelivery.delete(message.id);
      if (Date.now() - update.at <= EARLY_TTL) get().updateDelivery(message.id, update.status, update.error);
    }
  };
  const readPage = async (userId: string, offset: number) => {
    if (clearingUsers.has(userId)) return;
    const epoch = historyEpoch.get(userId) || 0;
    ++loadingRequests;
    set({ loading: true });
    try {
      const result = await invoke<HistoryResult>('history.get', { userId, limit: PAGE_SIZE, offset });
      if ((historyEpoch.get(userId) || 0) !== epoch) return;
      if (!Array.isArray(result.messages) || !result.localUserId) throw new Error('历史记录格式错误');
      const incoming = result.messages.map((row) => fromHistory(row, result.localUserId));
      set((state) => {
        const messages = new Map(state.messages);
        messages.set(userId, mergeHistory(messages.get(userId) || [], incoming));
        const historyPages = new Map(state.historyPages);
        historyPages.set(userId, { offset: offset + incoming.length, hasMore: incoming.length === PAGE_SIZE });
        return { messages, historyPages, localUserId: result.localUserId };
      });
      replay(incoming);
    } finally {
      --loadingRequests;
      set({ loading: loadingRequests > 0 });
    }
  };
  return {
    messages: new Map(), unread: new Map(), historyPages: new Map(),
    localUserId: '', activeConversation: '', loading: false, error: null,
    loadLocalUserId: async () => {
      const result = await invoke<{ success: boolean; id: string }>('user.local');
      if (!result.id) throw new Error('后端未提供本机身份');
      set({ localUserId: result.id });
    },
    sendMessage: async (target, content) => {
      set({ error: null });
      try {
        const result = await invoke<{ success: boolean; messageId: string }>('message.send', { target, content });
        if (!result.messageId) throw new Error('后端未返回消息 ID');
        const message: Message = {
          id: result.messageId, from: 'self', to: target, content,
          type: 'text', timestamp: Date.now(), status: 'sending',
        };
        get().recvMessage(message);
        replay([message]);
        return true;
      } catch (error) {
        set({ error: error instanceof Error ? error.message : String(error) });
        return false;
      }
    },
    recvMessage: (message) => set((state) => {
      const partner = message.from === 'self' ? message.to : message.from;
      const existing = state.messages.get(partner) || [];
      if (clearedIds.has(message.id)) return {};
      const index = existing.findIndex((m) => m.id === message.id);
      if (index >= 0) {
        // A late complete image event may fill metadata omitted by a stale
        // history snapshot. It is still the same message, not another unread.
        if (!existing[index].image && message.image) {
          const updated = [...existing];
          updated[index] = { ...updated[index], image: message.image };
          const messages = new Map(state.messages); messages.set(partner, updated);
          return { messages };
        }
        return {};
      }
      const messages = new Map(state.messages);
      messages.set(partner, [...existing, message].sort((a, b) => a.timestamp - b.timestamp || a.id.localeCompare(b.id)));
      const unread = new Map(state.unread);
      const visible = state.activeConversation === partner && document.visibilityState === 'visible' && document.hasFocus();
      if (message.from !== 'self' && !visible) {
        const ids = unreadIds.get(partner) || new Set<string>();
        ids.add(message.id);
        unreadIds.set(partner, ids);
        unread.set(partner, ids.size);
      }
      return { messages, unread };
    }),
    updateDelivery: (id, status, error) => {
      if (!id || clearedIds.has(id)) return;
      let found = false, newlyFailed = false;
      set((state) => {
        for (const [partner, list] of state.messages) {
          const index = list.findIndex((m) => m.id === id && m.from === 'self');
          if (index < 0) continue;
          found = true;
          if (list[index].status !== 'sending') return {};
          const updated = [...list];
          updated[index] = { ...list[index], status };
          const messages = new Map(state.messages);
          messages.set(partner, updated);
          newlyFailed = status === 'failed';
          return { messages };
        }
        return {};
      });
      if (!found) cacheDelivery(id, { status, error, at: Date.now() });
      if (newlyFailed) toast.error('消息发送失败：' + (error || '对方未确认接收'));
    },
    clearUnread: (userId) => set((state) => {
      unreadIds.delete(userId);
      const unread = new Map(state.unread); unread.delete(userId); return { unread };
    }),
    setActiveConversation: (userId) => {
      set({ activeConversation: userId });
      if (userId && document.hasFocus()) get().clearUnread(userId);
    },
    loadHistory: async (userId) => {
      try { await readPage(userId, 0); }
      catch (error) { toast.error('加载历史失败：' + String(error)); }
    },
    loadMoreHistory: async (userId) => {
      if (get().loading) return;
      const page = get().historyPages.get(userId);
      if (!page?.hasMore) return;
      try { await readPage(userId, page.offset); }
      catch (error) { toast.error('加载更早消息失败：' + String(error)); }
    },
    loadRecentConversations: async () => {
      const startedAt = new Map(historyEpoch);
      const result = await invoke<HistoryResult>('history.get_recent', { limit: 100 });
      if (!Array.isArray(result.messages) || !result.localUserId) throw new Error('最近会话格式错误');
      const incoming = result.messages.map((row) => fromHistory(row, result.localUserId));
      set((state) => {
        const messages = new Map(state.messages);
        for (const message of incoming) {
          const partner = message.from === 'self' ? message.to : message.from;
          if ((historyEpoch.get(partner) || 0) !== (startedAt.get(partner) || 0)) continue;
          messages.set(partner, mergeHistory(messages.get(partner) || [], [message]));
        }
        return { messages, localUserId: result.localUserId };
      });
      replay(incoming);
    },
    searchMessages: async (keyword, userId) => {
      const result = await invoke<HistoryResult>('history.search', { keyword, ...(userId ? { userId } : {}) });
      return result.messages.filter((row) => !clearedIds.has(row.id)).map((row) => fromHistory(row, result.localUserId));
    },
    clearHistory: async (userId) => {
      const users = userId ? [userId] : [...get().messages.keys()];
      if (users.some((id) => clearingUsers.has(id))) throw new Error('正在清空记录，请稍候');
      for (const id of users) {
        clearingUsers.add(id);
        historyEpoch.set(id, (historyEpoch.get(id) || 0) + 1);
      }
      let deleted: Set<string>;
      try {
        const result = await invoke<{ success: boolean; deletedIds: string[] }>('history.clear', userId ? { userId } : {});
        if (!Array.isArray(result.deletedIds) || result.deletedIds.some((id) => typeof id !== 'string')) throw new Error('清空历史返回格式错误');
        deleted = new Set(result.deletedIds);
        for (const id of result.deletedIds) {
          clearedIds.add(id);
          earlyDelivery.delete(id);
          if (clearedIds.size > CLEARED_LIMIT) clearedIds.delete(clearedIds.values().next().value!);
        }
      } finally {
        for (const id of users) clearingUsers.delete(id);
      }
      const affected = userId ? [userId] : [...new Set([...users, ...get().messages.keys()])];
      for (const id of affected) historyEpoch.set(id, (historyEpoch.get(id) || 0) + 1);
      set((state) => {
        const messages = new Map(state.messages), unread = new Map(state.unread), historyPages = new Map(state.historyPages);
        for (const id of affected) {
          // Delete exactly the IDs removed in the backend transaction, not a client-side time boundary.
          const remaining = (messages.get(id) || []).filter((m) => !deleted.has(m.id));
          if (remaining.length) messages.set(id, remaining); else messages.delete(id);
          const ids = unreadIds.get(id);
          if (ids) for (const messageId of deleted) ids.delete(messageId);
          if (ids?.size) unread.set(id, ids.size);
          else { unread.delete(id); unreadIds.delete(id); }
          historyPages.delete(id);
        }
        return { messages, unread, historyPages };
      });
    },
    initListeners: () => {
      const unsubs = [
        listen('message.received', (data: MessageReceivedEvent) => {
          if (!data?.id || !data.from) return;
          if (data.fromUser && !useUserStore.getState().users.some((u) => u.id === data.fromUser!.id)) {
            useUserStore.getState().addUser(data.fromUser);
          }
          get().recvMessage({
            id: data.id, from: data.from, to: 'self',
            content: data.content, type: data.type === 'text' ? 'text' : data.type === 'image' ? 'image' : 'file',
            timestamp: data.timestamp * 1000, status: 'delivered', fromUser: data.fromUser, image: data.image,
          });
        }),
        listen('message.ack', (data: { messageId: string }) => get().updateDelivery(data.messageId, 'delivered')),
        listen('message.failed', (data: { messageId: string; error?: string }) => get().updateDelivery(data.messageId, 'failed', data.error)),
        listen('image.receive_failed', (data: { error?: string }) => toast.error('图片接收失败：' + (data.error || '图片未能完成校验或保存'))),
      ];
      return () => unsubs.forEach((unsub) => unsub());
    },
  };
});
