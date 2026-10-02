// ============================================================================
// Message Store - Zustand state management for messages
// ============================================================================

import { create } from 'zustand';
import { Message, FileInfoAttachment, FileReceiveRequestEvent, User, ImageSendResult, ImageSendEvent } from '../types';
import { invoke, listen } from '../services/bridge';
import { useUserStore } from './userStore';
import { toast } from './toastStore';

// Module-level dedupe for FeiQ screenshots. Lives outside the store closure so it
// stays effective even if initMessageListeners is (accidentally) registered twice
// (e.g. React StrictMode in dev). Keyed by sender+size+dataUrl length, 2s window.
let lastFeiqSig = '';
let lastFeiqAt = 0;

type ImageSendPhase = 'progress' | 'completed' | 'failed';
// Worker events can arrive before image.send returns its message id. Keep only
// the latest update per id, with terminal events taking precedence over progress.
const earlyImageEvents = new Map<string, { data: ImageSendEvent; phase: ImageSendPhase; at: number }>();
const imageEventKey = (data: Pick<ImageSendEvent, 'target' | 'messageId'>) => `${data.target}\0${data.messageId}`;
function bufferImageEvent(data: ImageSendEvent, phase: ImageSendPhase) {
  const now = Date.now();
  for (const [key, event] of earlyImageEvents) {
    if (now - event.at > 60_000) earlyImageEvents.delete(key);
  }
  const key = imageEventKey(data);
  const previous = earlyImageEvents.get(key);
  if (previous && previous.phase !== 'progress') return;
  if (previous && phase === 'progress' && previous.data.progress > data.progress) return;
  if (!previous && earlyImageEvents.size >= 128) {
    earlyImageEvents.delete(earlyImageEvents.keys().next().value!);
  }
  earlyImageEvents.set(key, { data, phase, at: now });
}

/** Pending file receive request */
export interface PendingFileReceive {
  id: string;            // unique ID for this request
  packetNo: number;
  fromUser: string;
  fromUserIp: string;
  fromUserPort: number;
  fileName: string;
  fileSize: number;
  fileId: number;
  transferId?: string;
  timestamp: number;
}

/**
 * Locate the message that a file-transfer event (progress/completion) belongs to.
 * First try exact transferId match (message id or fileInfo.transferId). As a
 * fallback for the SENDER side, match the locally-initiated file/image message
 * that is still in progress, so progress still updates even if transferId
 * mismatches for any reason.
 */
function locateTransferMessage(
  messages: Map<string, Message[]>,
  transferId: string | undefined,
  isSending?: boolean
): { userId: string; idx: number } | null {
  if (transferId) {
    for (const [userId, msgs] of messages) {
      const idx = msgs.findIndex(
        (m) => m.fileInfo?.transferId === transferId || m.id === transferId
      );
      if (idx >= 0) return { userId, idx };
    }
  }
  if (isSending) {
    for (const [userId, msgs] of messages) {
      const idx = msgs.findIndex(
        (m) =>
          m.from === 'self' &&
          (m.type === 'file' || m.type === 'image') &&
          !m.fileInfo?.imageId &&
          m.status === 'sending' &&
          (m.transferProgress === undefined || m.transferProgress < 100)
      );
      if (idx >= 0) return { userId, idx };
    }
  }
  return null;
}

/**
 * Map a persisted history record to in-memory message state.
 * Backend status codes (MessageStatus in src/database/message_db.h):
 *   text: 0 = sent, no receipt yet; 1 = delivered (RECVMSG received, or an
 *         incoming message); 3 = send failed (older builds wrote 2)
 *   file: 0/1 = transfer started but never finished; 2 = completed;
 *         3 = failed / rejected
 */
function historyState(m: any): { status: Message['status']; transferProgress?: number } {
  const isFileMsg = m.type !== 0;
  if (isFileMsg) {
    if (m.status === 2) return { status: 'delivered', transferProgress: 100 };
    if (m.status === 3) return { status: 'failed' };
    // Incomplete transfer from a previous session: show a plain card, never a
    // fake 0% bar or a "waiting to accept" prompt that cannot be answered.
    return { status: 'sent' };
  }
  return { status: m.status === 0 ? 'sending' : m.status === 1 ? 'delivered' : 'failed' };
}

interface MessageStore {
  /** Map of userId -> messages array */
  messages: Map<string, Message[]>;
  /** Map of userId -> number of incoming messages not yet viewed */
  unread: Map<string, number>;
  /** Map of userId -> how much history has been paged in from the backend */
  historyPages: Map<string, { offset: number; hasMore: boolean }>;
  loading: boolean;
  error: string | null;

  /** Local user ID (e.g. "Mason@DESKTOP-ABC") - used to distinguish sent vs received messages */
  localUserId: string;

  /** Pending file receive requests (waiting for user confirmation) */
  pendingFileReceives: PendingFileReceive[];

  /** Send a text message to a user */
  sendMessage: (target: string, content: string) => Promise<boolean>;

  /** Send an image through FeiQ's inline image channel */
  sendImage: (target: string, base64Data: string, filename: string) => Promise<boolean>;
  sendImageByPath: (target: string, filePath: string) => Promise<boolean>;
  updateImageSend: (data: ImageSendEvent, phase: ImageSendPhase) => void;

  /** Send a file to a user using a real file path (no base64/temp copy, fast for large files) */
  sendFileByPath: (target: string, filePath: string) => Promise<boolean>;

  /** Accept a file receive request */
  acceptFileReceive: (requestId: string, savePath: string) => Promise<boolean>;

  /** Reject a file receive request */
  rejectFileReceive: (requestId: string) => void;

  /** Receive an incoming message */
  recvMessage: (message: Message) => void;

  /** Mark a conversation as viewed (called when it is opened) */
  clearUnread: (userId: string) => void;

  /** Update transfer progress for a message */
  updateTransferProgress: (transferId: string, progress: number, isSending: boolean) => void;

  /** Load chat history for a user from the backend (newest page; resets paging) */
  loadHistory: (userId: string, limit?: number, offset?: number) => Promise<void>;

  /** Load the next older page of history for a user (prepends) */
  loadMoreHistory: (userId: string) => Promise<void>;

  /** Search messages by keyword */
  searchMessages: (keyword: string) => Promise<Message[]>;

  /** Clear chat history for a user */
  clearHistory: (userId?: string) => Promise<void>;

  /** Get messages for a specific user */
  getMessages: (userId: string) => Message[];

  /** Load recent conversations (latest message per user) for conversation list */
  loadRecentConversations: () => Promise<void>;

  /** Load local user ID from backend */
  loadLocalUserId: () => Promise<void>;

  /** Initialize event listeners */
  initListeners: () => () => void;
}

export const useMessageStore = create<MessageStore>((set, get) => ({
  messages: new Map(),
  unread: new Map(),
  historyPages: new Map(),
  loading: false,
  error: null,
  localUserId: '',
  pendingFileReceives: [],

  loadLocalUserId: async () => {
    try {
      const result = await invoke<{ success: boolean; id: string }>('user.local');
      if (result.success && result.id) {
        console.log('[MessageStore] Local user ID:', result.id);
        set({ localUserId: result.id });
      }
    } catch (err) {
      console.error('[MessageStore] Failed to load local user ID:', err);
    }
  },

  sendMessage: async (target, content) => {
    console.log(`[MSG_SEND] target=${target}, content="${content}"`);
    try {
      const result = await invoke<{ success: boolean; error?: string; messageId?: string }>(
        'message.send',
        { target, content }
      );
      if (result.success) {
        const msg: Message = {
          id: result.messageId || Date.now().toString(),
          from: 'self',
          to: target,
          content,
          type: 'text',
          timestamp: Date.now(),
          status: 'sending',
        };
        get().recvMessage(msg);
        return true;
      }
      return false;
    } catch {
      return false;
    }
  },

  sendImage: async (target, base64Data, filename) => {
    console.log(`[IMG_SEND] target=${target}, filename=${filename}, dataSize=${base64Data.length}`);
    try {
      const saveResult = await invoke<{ success: boolean; filePath?: string; error?: string }>(
        'file.save_temp',
        { data: base64Data, filename }
      );
      if (!saveResult.success || !saveResult.filePath) {
        throw new Error(saveResult.error || '无法保存截图');
      }
      return await get().sendImageByPath(target, saveResult.filePath);
    } catch (err) {
      console.error('sendImage error:', err);
      set({ error: err instanceof Error ? err.message : String(err) });
      return false;
    }
  },

  sendImageByPath: async (target, filePath) => {
    set({ error: null });
    try {
      const result = await invoke<ImageSendResult>('image.send', { target, filePath });
      if (!result.success || !result.messageId || !result.imageId || !result.filePath) {
        throw new Error(result.error || '图片发送任务未能创建');
      }
      const displayName = result.fileName || filePath.split(/[\\/]/).pop() || filePath;
      const msg: Message = {
        id: result.messageId,
        from: 'self',
        to: target,
        content: displayName,
        type: 'image',
        timestamp: Date.now(),
        status: 'sending',
        fileInfo: {
          fileName: displayName,
          fileSize: result.fileSize || 0,
          // The worker may still be copying into result.filePath. Preview the
          // existing source until its first event supplies the durable copy.
          filePath,
          imageId: result.imageId,
        },
        transferProgress: 0,
      };
      // A history request can also insert this id while image.send is pending.
      // Upsert metadata without duplicating it or downgrading a terminal event.
      set((state) => {
        const messages = [...(state.messages.get(target) || [])];
        const idx = messages.findIndex((m) => m.id === msg.id);
        if (idx < 0) messages.push(msg);
        else {
          const previous = messages[idx];
          const terminal = previous.status === 'delivered' || previous.status === 'failed';
          messages[idx] = {
            ...msg,
            ...(terminal ? { status: previous.status, transferProgress: previous.transferProgress } : {}),
            fileInfo: { ...msg.fileInfo!, ...(previous.transferProgress !== undefined || terminal ? previous.fileInfo : {}) },
          };
        }
        const all = new Map(state.messages);
        all.set(target, messages);
        return { messages: all };
      });
      const key = imageEventKey({ target, messageId: msg.id });
      const early = earlyImageEvents.get(key);
      earlyImageEvents.delete(key);
      if (early) get().updateImageSend(early.data, early.phase);
      return true;
    } catch (err) {
      console.error('[IMG_SEND] image.send failed:', err);
      set({ error: err instanceof Error ? err.message : String(err) });
      return false;
    }
  },

  updateImageSend: (data, phase) => {
    if (!data?.messageId || !data.target) return;
    let found = false;
    let notifyFailure = false;
    set((state) => {
      const messages = state.messages.get(data.target);
      const idx = messages?.findIndex((m) => m.id === data.messageId && m.from === 'self' && m.type === 'image') ?? -1;
      if (!messages || idx < 0) return {};
      found = true;
      const msg = messages[idx];
      // A delayed progress tick (or duplicate terminal event) cannot undo completion.
      if (msg.status === 'delivered' || msg.status === 'failed') return {};
      const progress = Number.isFinite(data.progress) ? Math.max(0, Math.min(100, data.progress)) : 0;
      const updated = [...messages];
      updated[idx] = {
        ...msg,
        status: phase === 'completed' ? 'delivered' : phase === 'failed' ? 'failed' : 'sending',
        transferProgress: phase === 'completed' ? 100 : phase === 'failed' ? undefined : Math.max(msg.transferProgress || 0, progress),
        fileInfo: {
          ...msg.fileInfo,
          fileName: data.fileName || msg.fileInfo?.fileName || '',
          fileSize: data.fileSize ?? msg.fileInfo?.fileSize ?? 0,
          filePath: data.filePath || msg.fileInfo?.filePath,
        },
      };
      const newMessages = new Map(state.messages);
      newMessages.set(data.target, updated);
      notifyFailure = phase === 'failed';
      return { messages: newMessages };
    });
    if (!found) bufferImageEvent(data, phase);
    else if (notifyFailure) toast.error('图片发送失败：' + (data.error || '对方可能不支持内嵌图片或已离线，可尝试通过文件按钮发送'));
  },

  sendFileByPath: async (target, filePath) => {
    console.log(`[FILE_SEND_PATH] target=${target}, filePath=${filePath}`);
    try {
      const result = await invoke<{ success: boolean; transferId?: string; fileName?: string; fileSize?: number; error?: string }>(
        'file.send',
        { target, filePath }
      );
      if (result.success) {
        const displayName = result.fileName || filePath.split(/[\\/]/).pop() || filePath;
        const msg: Message = {
          id: result.transferId || Date.now().toString(),
          from: 'self',
          to: target,
          content: displayName,
          type: 'file',
          timestamp: Date.now(),
          status: 'sending',
          fileInfo: {
            fileName: displayName,
            fileSize: result.fileSize || 0,
            filePath,
            transferId: result.transferId,
          },
          transferProgress: 0,
        };
        get().recvMessage(msg);
        return true;
      }
      console.error('[FILE_SEND_PATH] failed:', result.error);
      return false;
    } catch (err) {
      console.error('sendFileByPath error:', err);
      return false;
    }
  },

  acceptFileReceive: async (requestId, savePath) => {
    console.log(`[ACCEPT_FILE] requestId=${requestId}, savePath=${savePath}`);
    const request = get().pendingFileReceives.find(r => r.id === requestId);
    if (!request) {
      console.error(`[ACCEPT_FILE] Request not found for id=${requestId}`);
      return false;
    }
    console.log(`[ACCEPT_FILE] Found request: fromUser=${request.fromUser}, fileName=${request.fileName}, packetNo=${request.packetNo}, transferId=${request.transferId}`);

    // Remove from pending list
    set((state) => ({
      pendingFileReceives: state.pendingFileReceives.filter(r => r.id !== requestId),
    }));

    try {
      // Determine if image or file
      const ext = request.fileName.split('.').pop()?.toLowerCase() || '';
      const isImage = ['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp'].includes(ext);
      const msgType = isImage ? 'image' : 'file';

      // Update the existing placeholder message to show transferring
      set((state) => {
        const newMessages = new Map(state.messages);
        const userId = request.fromUser;
        const userMsgs = newMessages.get(userId);
        if (userMsgs) {
          const idx = userMsgs.findIndex(m => m.id === `recv_${request.packetNo}`);
          if (idx >= 0) {
            const updated = [...userMsgs];
            updated[idx] = {
              ...updated[idx],
              status: 'sending',
              transferProgress: 0,
              fileInfo: updated[idx].fileInfo
                ? { ...updated[idx].fileInfo! }
                : { fileName: request.fileName, fileSize: request.fileSize },
            };
            newMessages.set(userId, updated);
          }
        }
        return { messages: newMessages };
      });

      // Start receiving via file.accept (sends IPMSG_RECVMSG + starts TCP recv)
      console.log(`[ACCEPT_FILE] Calling file.accept with: target=${request.fromUserIp}, transferId=${request.transferId || request.packetNo.toString()}, fileName=${request.fileName}, fileSize=${request.fileSize}, packetNo=${request.packetNo}, fileId=${request.fileId}`);
      const result = await invoke<{ success: boolean; transferId?: string; error?: string }>(
        'file.accept',
        {
          target: request.fromUserIp,
          transferId: request.transferId || request.packetNo.toString(),
          fileName: request.fileName,
          fileSize: request.fileSize,
          savePath: savePath,
          packetNo: request.packetNo,   // Original SENDMSG packetNo for GETFILEDATA request
          fileId: request.fileId,       // File ID from attachment info for GETFILEDATA request
        }
      );
      console.log(`[ACCEPT_FILE] file.accept result: success=${result.success}, transferId=${result.transferId}, error=${result.error}`);

      if (!result.success) {
        // Update message status to failed
        set((state) => {
          const newMessages = new Map(state.messages);
          const userId = request.fromUser;
          const userMsgs = newMessages.get(userId);
          if (userMsgs) {
            const idx = userMsgs.findIndex(m => m.id === `recv_${request.packetNo}`);
            if (idx >= 0) {
              const updated = [...userMsgs];
              updated[idx] = { ...updated[idx], status: 'failed', transferProgress: undefined };
              newMessages.set(userId, updated);
            }
          }
          return { messages: newMessages };
        });
      } else if (result.transferId) {
        // Update message with the real transferId from backend for progress tracking
        set((state) => {
          const newMessages = new Map(state.messages);
          const userId = request.fromUser;
          const userMsgs = newMessages.get(userId);
          if (userMsgs) {
            const idx = userMsgs.findIndex(m => m.id === `recv_${request.packetNo}`);
            if (idx >= 0) {
              const updated = [...userMsgs];
              updated[idx] = {
                ...updated[idx],
                fileInfo: updated[idx].fileInfo
                  ? { ...updated[idx].fileInfo!, transferId: result.transferId }
                  : { fileName: request.fileName, fileSize: request.fileSize, transferId: result.transferId },
              };
              newMessages.set(userId, updated);
            }
          }
          return { messages: newMessages };
        });
      }
      return result.success;
    } catch (err) {
      console.error('acceptFileReceive error:', err);
      return false;
    }
  },

  rejectFileReceive: (requestId) => {
    const request = get().pendingFileReceives.find(r => r.id === requestId);

    set((state) => ({
      pendingFileReceives: state.pendingFileReceives.filter(r => r.id !== requestId),
    }));

    // Notify backend to send IPMSG_RELEASEFILES
    if (request) {
      invoke('file.reject', {
        target: request.fromUserIp,
        transferId: request.packetNo.toString(),
      }).catch(() => {});

      // Mark the corresponding message as rejected
      set((state) => {
        const newMessages = new Map(state.messages);
        for (const [userId, msgs] of newMessages) {
          const idx = msgs.findIndex(m => m.id === `recv_${request.packetNo}`);
          if (idx >= 0) {
            const updated = [...msgs];
            updated[idx] = { ...updated[idx], status: 'failed', transferProgress: undefined };
            newMessages.set(userId, updated);
            break;
          }
        }
        return { messages: newMessages };
      });
    }
  },

  recvMessage: (message) => {
    set((state) => {
      const newMessages = new Map(state.messages);
      const userId = message.from === 'self' ? message.to : message.from;
      const userMsgs = newMessages.get(userId) || [];
      // Deduplicate: skip if a message with the same id already exists
      // Also check by recv_ prefix since file.receive_request uses recv_{packetNo}
      // while message.received uses just {packetNo}
      const msgId = message.id;
      const packetNo = msgId.startsWith('recv_') ? msgId.substring(5) : msgId;
      const isDuplicate = userMsgs.some(m => {
        if (m.id === msgId) return true;
        // Cross-check: recv_1234 and 1234 refer to the same message
        const mPacketNo = m.id.startsWith('recv_') ? m.id.substring(5) : m.id;
        return mPacketNo === packetNo && packetNo.length > 0;
      });
      if (isDuplicate) {
        console.log(`[MessageStore] Duplicate message skipped: id=${msgId}`);
        return state;
      }
      newMessages.set(userId, [...userMsgs, message]);

      // Unread counter: an incoming message for a conversation that is not the
      // one currently open. The chat panel clears it when that user is opened.
      // We deliberately do NOT switch the active conversation here: stealing
      // focus destroyed the draft the user was typing.
      const currentId = useUserStore.getState().currentUser?.id;
      if (message.from !== 'self' && userId !== currentId) {
        const unread = new Map(state.unread);
        unread.set(userId, (unread.get(userId) ?? 0) + 1);
        return { messages: newMessages, unread };
      }
      return { messages: newMessages };
    });
  },

  clearUnread: (userId) => {
    set((state) => {
      if (!state.unread.has(userId)) return {};
      const unread = new Map(state.unread);
      unread.delete(userId);
      return { unread };
    });
  },

updateTransferProgress: (transferId, progress, isSending) => {
  set((state) => {
    const loc = locateTransferMessage(state.messages, transferId, isSending);
    if (!loc) return {};
    const msgs = state.messages.get(loc.userId)!;
    const current = msgs[loc.idx];
    // Once a transfer is finished, ignore any stale progress report (e.g. a
    // resume/GETFILEDATA chunk that restarts at 0%) so it can't downgrade an
    // already-delivered/received message back to 0%.
    if (((current.status as string) === 'delivered' || (current.status as string) === 'received' || (current.status as string) === 'read')
        && progress < 100) {
      return {};
    }
    const newMessages = new Map(state.messages);
    const oldProgress = msgs[loc.idx].transferProgress;
      // Only update if progress changed significantly (>1%) or reached 0/100
      const shouldUpdate = progress === 0 || progress === 100 ||
        (oldProgress === undefined || oldProgress === -1) ||
        Math.abs(progress - oldProgress) >= 1;
      if (!shouldUpdate) return {};
      const updated = [...msgs];
      updated[loc.idx] = {
        ...updated[loc.idx],
        transferProgress: progress,
        status: progress >= 100 ? 'delivered' : updated[loc.idx].status,
      };
      newMessages.set(loc.userId, updated);
      return { messages: newMessages };
    });
  },

  loadHistory: async (userId, limit = 50, offset = 0) => {
    set({ loading: true });
    try {
      const result = await invoke<{
        success: boolean;
        messages: any[];
        localUserId?: string;
      }>('history.get', { userId, limit, offset });

      // Update localUserId from backend if returned
      if (result.localUserId) {
        set({ localUserId: result.localUserId });
      }

      const localUserId = get().localUserId || result.localUserId || '';

      if (result.success && result.messages) {
        console.log(`[loadHistory] userId=${userId}, localUserId=${localUserId}, messages=${result.messages.length}`);

        const msgs: Message[] = result.messages.map((m: any) => {
          // Determine if this message was sent by the local user
          // fromId is the sender's Key, toId is the receiver's Key
          // If fromId === localUserId, this message was sent by us
          const isSentByMe = m.fromId === localUserId;

          const msgType = m.type === 0 ? 'text' : m.type === 1 ? 'image' : 'file';
          const isFileMsg = m.type !== 0;
          const { status: msgStatus, transferProgress } = historyState(m);

          return {
            id: m.id,
            from: isSentByMe ? 'self' : m.fromId,
            to: isSentByMe ? m.toId : 'self',
            content: m.content,
            type: msgType,
            timestamp: m.timestamp * 1000,
            status: msgStatus,
            transferProgress,
            fileInfo: isFileMsg ? {
              fileName: m.content.split(/[\\/]/).pop() || m.content,
              fileSize: 0,
              filePath: m.content,
              transferId: m.id,
            } : undefined,
          };
        });

        // Merge with existing real-time messages (don't lose in-flight messages)
        set((state) => {
          const newMessages = new Map(state.messages);
          const existingMsgs = newMessages.get(userId) || [];

          // Build a set of message IDs from history
          const historyIds = new Set(msgs.map(m => m.id));

          // Keep real-time messages that are not in history, plus any in-flight
          // transfer whose live progress must not be clobbered by the persisted
          // (possibly 0%) history entry.
          const realtimeOnly = existingMsgs.filter((m) => {
            if (!historyIds.has(m.id)) return true;
            const inFlight = (m.type === 'file' || m.type === 'image') &&
              m.status === 'sending' &&
              (m.transferProgress === undefined || m.transferProgress < 100);
            return inFlight || !!m.fileInfo?.imageId;
          });

          // Live state replaces the same historical id rather than duplicating it.
          const liveIds = new Set(realtimeOnly.map((m) => m.id));
          const combined = [...msgs.filter((m) => !liveIds.has(m.id)), ...realtimeOnly]
            .sort((a, b) => a.timestamp - b.timestamp);
          newMessages.set(userId, combined);

          // Paging bookkeeping: the backend returns the newest `limit` after
          // skipping `offset`, so the next older page starts at offset + count.
          const historyPages = new Map(state.historyPages);
          historyPages.set(userId, {
            offset: offset + result.messages.length,
            hasMore: result.messages.length >= limit,
          });
          return { messages: newMessages, historyPages };
        });
      }
    } catch (err: any) {
      set({ error: err.message });
    } finally {
      set({ loading: false });
    }
  },

  loadMoreHistory: async (userId) => {
    const page = get().historyPages.get(userId);
    if (!page || !page.hasMore || get().loading) return;
    await get().loadHistory(userId, 50, page.offset);
  },

  loadRecentConversations: async () => {
    try {
      console.log('[MessageStore] loadRecentConversations: calling history.get_recent');
      const result = await invoke<{
        success: boolean;
        messages: any[];
        localUserId?: string;
      }>('history.get_recent', { limit: 50 });  // Increase limit for 1-month filter

      console.log('[MessageStore] loadRecentConversations result:', result);

      if (result.success && result.messages) {
        const localUserId = get().localUserId || result.localUserId || '';

        // Filter to last 30 days (1 month)
        const oneMonthAgo = Date.now() - 30 * 24 * 60 * 60 * 1000;

        const msgs: Message[] = result.messages.map((m: any) => {
          const isSentByMe = m.fromId === localUserId;
          const msgType = m.type === 0 ? 'text' : m.type === 1 ? 'image' : 'file';

          return {
            id: m.id,
            from: isSentByMe ? 'self' : m.fromId,
            to: isSentByMe ? m.toId : 'self',
            content: m.content,
            type: msgType,
            timestamp: m.timestamp * 1000,
            status: historyState(m).status,
          };
        });

        console.log('[MessageStore] Parsed messages:', msgs);

        // Filter to last 1 month
        const recentMsgs = msgs.filter(m => m.timestamp >= oneMonthAgo);
        console.log('[MessageStore] Messages after 1-month filter:', recentMsgs);

        // Group by conversation partner
        const partnerMap = new Map<string, Message>();
        for (const msg of recentMsgs) {
          const partnerId = msg.from === 'self' ? msg.to : msg.from;
          const existing = partnerMap.get(partnerId);
          if (!existing || msg.timestamp > existing.timestamp) {
            partnerMap.set(partnerId, msg);
          }
        }

        console.log('[MessageStore] Partner map:', Array.from(partnerMap.entries()));

        // Update store with latest message per partner
        set((state) => {
          const newMessages = new Map(state.messages);
          for (const [partnerId, msg] of partnerMap) {
            const existing = newMessages.get(partnerId) || [];
            // Only add if not already present (avoid duplicates with real-time messages)
            const isDuplicate = existing.some(m => m.id === msg.id);
            if (!isDuplicate) {
              newMessages.set(partnerId, [msg, ...existing]);
            }
          }
          console.log('[MessageStore] Updated messages map keys:', Array.from(newMessages.keys()));
          return { messages: newMessages };
        });
      }
    } catch (err: any) {
      console.error('[MessageStore] loadRecentConversations failed:', err);
    }
  },

  searchMessages: async (keyword) => {
    try {
      const result = await invoke<{
        success: boolean;
        messages: any[];
      }>('history.search', { keyword });

      if (result.success && result.messages) {
        return result.messages.map((m: any) => ({
          id: m.id,
          from: m.fromId,
          to: m.toId,
          content: m.content,
          type: m.type === 0 ? 'text' : m.type === 1 ? 'image' : 'file',
          timestamp: m.timestamp * 1000,
          status: 'delivered',
        }));
      }
      return [];
    } catch {
      return [];
    }
  },

  clearHistory: async (userId) => {
    try {
      await invoke('history.clear', { userId });
      if (userId) {
        set((state) => {
          const newMessages = new Map(state.messages);
          newMessages.delete(userId);
          const historyPages = new Map(state.historyPages);
          historyPages.delete(userId);
          return { messages: newMessages, historyPages };
        });
      } else {
        set({ messages: new Map(), historyPages: new Map() });
      }
    } catch (err: any) {
      set({ error: err.message });
    }
  },

  getMessages: (userId) => {
    return get().messages.get(userId) || [];
  },

  initListeners: () => {
    const unsubs: (() => void)[] = [];

    console.log('[MessageStore] Registering event listeners...');

    // Debug: check if __tauricpp__ is available
    const hasTauricpp = typeof (window as any).__tauricpp__ !== 'undefined';
    const hasEmit = hasTauricpp && typeof (window as any).__tauricpp_internal_emit === 'function';
    console.log('[MessageStore] window.__tauricpp__:', hasTauricpp, 'window.__tauricpp_internal_emit:', hasEmit);

    // Inline images have their own lifecycle, independent from TCP file events.
    unsubs.push(listen('image.send_progress', (data: ImageSendEvent) => get().updateImageSend(data, 'progress')));
    unsubs.push(listen('image.send_completed', (data: ImageSendEvent) => get().updateImageSend(data, 'completed')));
    unsubs.push(listen('image.send_failed', (data: ImageSendEvent) => get().updateImageSend(data, 'failed')));

    // Listen for incoming messages
    unsubs.push(listen('message.received', (data: any) => {
      console.log(`[MSG_RECV] CALLBACK TRIGGERED! from=${data.from}, type=${data.type}, content="${data.content}"`);
      const isFileAttach = (data.command & 0x00200000) !== 0;  // IPMSG_FILEATTACHOPT
      let fileInfo: FileInfoAttachment | undefined;

      if (isFileAttach && data.extra) {
        // Parse extra in Feiq/IPMsg format: "fileId:filename:hexSize:hexMtime:hexFileAttr:\a"
        // Colons in filename are escaped as :: (:: represents a literal colon)
        // Reference: Feiq feiqengine.cpp RecvFile::createFileContent

        // First, split by ::-aware colon separator
        const raw = data.extra;
        const parts: string[] = [];
        let current = '';
        for (let i = 0; i < raw.length; i++) {
          if (raw[i] === ':' && i + 1 < raw.length && raw[i + 1] === ':') {
            // Escaped colon ::
            current += ':';
            i++; // skip the second colon
          } else if (raw[i] === ':') {
            // Field separator
            parts.push(current);
            current = '';
          } else {
            current += raw[i];
          }
        }
        if (current.length > 0) parts.push(current);

        if (parts.length >= 3) {
          const fileName = parts[1];
          // fileSize is in hexadecimal per Feiq protocol
          const fileSize = parseInt(parts[2], 16) || 0;

          if (data.type === 'image' || data.type === 'file') {
            fileInfo = {
              fileName,
              fileSize,
              fileId: parseInt(parts[0]) || undefined,
            };
          }
        }
      }

      const msg: Message = {
        id: data.id,
        from: data.from,
        to: 'self',
        content: data.content,
        type: (data.type as any) || 'text',
        timestamp: data.timestamp * 1000,
        status: 'delivered',
        fromUser: data.fromUser,
        fileInfo,
      };

      // If it's a text message (not file attachment), add to message list
      if (!isFileAttach) {
        console.log(`[MSG_RECV_CB] Calling recvMessage for ${data.from}, content="${data.content?.substring(0,30)}"`);
        get().recvMessage(msg);
      } else {
        console.log(`[MSG_RECV_CB] Skipping recvMessage for file attachment, id=${data.id}, command=0x${(data.command||0).toString(16)}`);
      }

      // Auto-add the sender to user list if not already present
      // This ensures the sender appears in the contacts view immediately
      if (data.fromUser) {
        const userStore = useUserStore.getState();
        const exists = userStore.users.some(u => u.id === data.from);
        if (!exists) {
          const newUser: User = {
            id: data.from,
            nickname: data.fromUser.nickname || data.fromUser.username || data.from.split('@')[0],
            username: data.fromUser.username || '',
            hostname: data.fromUser.hostname || '',
            group: data.fromUser.group || '',
            ip: data.fromUser.ip || '',
            port: data.fromUser.port || 0,
            status: 'online',
            version: data.fromUser.version || '',
          };
          console.log('[MSGRECV] Auto-adding user to list:', newUser.id);
          userStore.addUser(newUser);
        }
        // The sender is NOT auto-selected: the conversation list shows an
        // unread badge instead, so the user's current draft is preserved.
      }
      // For file attachments, the message was already added by file.receive_request handler
    }));

    // Listen for delivery receipts (IPMSG_RECVMSG) of our own text messages.
    // The backend resolves the acknowledged packetNo to the message id it
    // returned from message.send, so we can match by id.
    unsubs.push(listen('message.ack', (data: any) => {
      const messageId: string | undefined = data?.messageId;
      if (!messageId) return;
      set((state) => {
        for (const [userId, msgs] of state.messages) {
          const idx = msgs.findIndex((m) => m.id === messageId && m.from === 'self');
          if (idx < 0) continue;
          if (msgs[idx].status === 'delivered') return {};
          const newMessages = new Map(state.messages);
          const updated = [...msgs];
          updated[idx] = { ...updated[idx], status: 'delivered' };
          newMessages.set(userId, updated);
          return { messages: newMessages };
        }
        return {};
      });
    }));

    // Listen for file receive requests - add to pending list
    unsubs.push(listen('file.receive_request', (data: FileReceiveRequestEvent) => {
      console.log(`[FILE_RECV_REQ] from=${data.fromUser}(${data.fromUserIp}:${data.fromUserPort}), fileName=${data.fileName}, fileSize=${data.fileSize}, fileId=${data.fileId}, packetNo=${data.packetNo}, transferId=${data.transferId}`);
      
      // Write to console for debugging
      const pendingReq: PendingFileReceive = {
        id: `req_${data.packetNo}`,
        packetNo: data.packetNo,
        fromUser: data.fromUser,
        fromUserIp: data.fromUserIp,
        fromUserPort: data.fromUserPort,
        fileName: data.fileName,
        fileSize: data.fileSize,
        fileId: data.fileId,
        transferId: data.transferId,
        timestamp: Date.now(),
      };

      // Add a message with "waiting for acceptance" status (transferProgress = -1)
      const ext = data.fileName.split('.').pop()?.toLowerCase() || '';
      const isImage = ['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp'].includes(ext);

      const msg: Message = {
        id: `recv_${data.packetNo}`,
        from: data.fromUser,
        to: 'self',
        content: data.fileName,
        type: isImage ? 'image' : 'file',
        timestamp: Date.now(),
        status: 'sending',
        fileInfo: {
          fileName: data.fileName,
          fileSize: data.fileSize,
          fileId: data.fileId,
        },
        transferProgress: -1,  // -1 means waiting for acceptance
      };
      get().recvMessage(msg);

      // Add to pending list for user confirmation
      set((state) => ({
        pendingFileReceives: [...state.pendingFileReceives, pendingReq],
      }));
    }));

    // Listen for file transfer progress
    unsubs.push(listen('file.transfer_progress', (data: any) => {
      const progress = data.fileSize > 0
        ? Math.round((data.transferred * 100) / data.fileSize)
        : 0;
      // 节流日志：仅在整数百分比或每 5% 打印，避免大文件海量日志拖垮 UI（整框闪烁的根因）
      if (progress === 0 || progress === 100 || progress % 5 === 0) {
        console.log(`[FILE_PROGRESS] transferId=${data.transferId}, ${data.transferred}/${data.fileSize} (${progress}%)`);
      }
      get().updateTransferProgress(data.transferId, progress, data.isSending);
    }));

    // Listen for file transfer completion
    unsubs.push(listen('file.transfer_completed', (data: any) => {
      console.log(`[FILE_COMPLETE] transferId=${data.transferId}, isSending=${data.isSending}, savePath=${data.savePath || 'N/A'}`);
      get().updateTransferProgress(data.transferId, 100, data.isSending);

      // Update the message with save path for received files
      if (!data.isSending && data.savePath) {
        set((state) => {
          const loc = locateTransferMessage(state.messages, data.transferId, data.isSending);
          if (!loc) return {};
          const newMessages = new Map(state.messages);
          const msgs = newMessages.get(loc.userId)!;
          const updated = [...msgs];
          updated[loc.idx] = {
            ...updated[loc.idx],
            status: 'delivered',
            transferProgress: 100,
            fileInfo: updated[loc.idx].fileInfo
              ? { ...updated[loc.idx].fileInfo!, filePath: data.savePath }
              : { fileName: data.filename || '', fileSize: 0, filePath: data.savePath },
          };
          newMessages.set(loc.userId, updated);
          return { messages: newMessages };
        });
      }
    }));

    // Listen for file transfer failure
    unsubs.push(listen('file.transfer_failed', (data: any) => {
      set((state) => {
        const newMessages = new Map(state.messages);
        for (const [userId, msgs] of newMessages) {
          const idx = msgs.findIndex(
            m => m.fileInfo?.transferId === data.transferId
          );
          if (idx >= 0) {
            const updated = [...msgs];
            updated[idx] = { ...updated[idx], status: 'failed', transferProgress: undefined };
            newMessages.set(userId, updated);
            break;
          }
        }
        return { messages: newMessages };
      });
    }));

    // Listen for FeiQ inline screenshots, which are reassembled entirely on the
    // backend and delivered as a finished image (inline base64 data URL). This
    // bypasses the standard "accept file" UI used by normal file transfers.
    // Guard against duplicate delivery (UDP retransmit / accidental double listener).
    unsubs.push(listen('feiq.screenshot_received', (data: any) => {
      console.log(`[FEIQ_SHOT] from=${data.fromUser?.id}, fileName=${data.fileName}, size=${data.fileSize}`);

      const messageId = typeof data.messageId === 'string' && data.messageId ? data.messageId : undefined;
      // New backends provide a stable id (also used in history). Same-sized
      // images are distinct messages; only legacy events need the old heuristic.
      if (!messageId) {
        const sig = `${data.fromUser?.id}|${data.fileSize}|${(data.dataUrl || '').length}`;
        const now = Date.now();
        if (sig === lastFeiqSig && now - lastFeiqAt < 2000) {
          console.log('[FEIQ_SHOT] Duplicate delivery ignored (same sig within 2s)');
          return;
        }
        lastFeiqSig = sig;
        lastFeiqAt = now;
      }

      // Auto-add the sender to the contact list if missing. Like normal
      // messages, the conversation is not auto-selected (unread badge instead).
      if (data.fromUser) {
        const userStore = useUserStore.getState();
        const exists = userStore.users.some(u => u.id === data.fromUser.id);
        if (!exists) {
          userStore.addUser({
            id: data.fromUser.id,
            nickname: data.fromUser.nickname || data.fromUser.username || data.fromUser.id.split('@')[0],
            username: data.fromUser.username || '',
            hostname: data.fromUser.hostname || '',
            group: data.fromUser.group || '',
            ip: data.fromUser.ip || '',
            port: data.fromUser.port || 0,
            status: 'online',
            version: data.fromUser.version || '',
          });
        }
      }

      const msg: Message = {
        id: messageId || `feiq_${Date.now()}_${Math.random().toString(36).slice(2)}`,
        from: data.fromUser?.id || 'unknown',
        to: 'self',
        content: data.dataUrl,
        type: 'image',
        timestamp: typeof data.timestamp === 'number' && Number.isFinite(data.timestamp) ? data.timestamp * 1000 : Date.now(),
        status: 'delivered',
        fromUser: data.fromUser,
        fileInfo: { fileName: data.fileName, fileSize: data.fileSize || 0, filePath: data.savePath, imageId: data.imageId },
        transferProgress: 100,
      };
      get().recvMessage(msg);
    }));

    return () => {
      unsubs.forEach(unsub => unsub());
    };
  },
}));
