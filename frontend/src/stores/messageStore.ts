// ============================================================================
// Message Store - Zustand state management for messages
// ============================================================================

import { create } from 'zustand';
import { Message, FileInfoAttachment, FileReceiveRequestEvent, User } from '../types';
import { invoke, listen } from '../services/bridge';
import { useUserStore } from './userStore';

// Module-level dedupe for FeiQ screenshots. Lives outside the store closure so it
// stays effective even if initMessageListeners is (accidentally) registered twice
// (e.g. React StrictMode in dev). Keyed by sender+size+dataUrl length, 2s window.
let lastFeiqSig = '';
let lastFeiqAt = 0;

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
          m.status === 'sending' &&
          (m.transferProgress === undefined || m.transferProgress < 100)
      );
      if (idx >= 0) return { userId, idx };
    }
  }
  return null;
}

interface MessageStore {
  /** Map of userId -> messages array */
  messages: Map<string, Message[]>;
  /** Monotonically increasing counter bumped on each message receive - used to force React re-render */
  messageVersion: number;
  loading: boolean;
  error: string | null;

  /** Local user ID (e.g. "Mason@DESKTOP-ABC") - used to distinguish sent vs received messages */
  localUserId: string;

  /** Pending file receive requests (waiting for user confirmation) */
  pendingFileReceives: PendingFileReceive[];

  /** Send a text message to a user */
  sendMessage: (target: string, content: string) => Promise<boolean>;

  /** Send an image to a user */
  sendImage: (target: string, base64Data: string, filename: string) => Promise<boolean>;

  /** Send a file to a user (via base64 + temp copy) */
  sendFile: (target: string, base64Data: string, filename: string) => Promise<boolean>;
  /** Send a file to a user using a real file path (no base64/temp copy, fast for large files) */
  sendFileByPath: (target: string, filePath: string) => Promise<boolean>;

  /** Accept a file receive request */
  acceptFileReceive: (requestId: string, savePath: string) => Promise<boolean>;

  /** Reject a file receive request */
  rejectFileReceive: (requestId: string) => void;

  /** Receive an incoming message */
  recvMessage: (message: Message) => void;

  /** Update transfer progress for a message */
  updateTransferProgress: (transferId: string, progress: number, isSending: boolean) => void;

  /** Load chat history for a user from the backend */
  loadHistory: (userId: string, limit?: number, offset?: number) => Promise<void>;

  /** Search messages by keyword */
  searchMessages: (keyword: string) => Promise<Message[]>;

  /** Clear chat history for a user */
  clearHistory: (userId?: string) => Promise<void>;

  /** Get messages for a specific user */
  getMessages: (userId: string) => Message[];

  /** Load local user ID from backend */
  loadLocalUserId: () => Promise<void>;

  /** Initialize event listeners */
  initListeners: () => () => void;
}

export const useMessageStore = create<MessageStore>((set, get) => ({
  messages: new Map(),
  messageVersion: 0,
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
      // 截图/图片统一走标准文件传输通道（TCP），对方以“接收/确认”方式接收，
      // 不再使用飞秋内联富文本（引用消息 + 分片）协议。
      const saveResult = await invoke<{ success: boolean; filePath?: string; error?: string }>(
        'file.save_temp',
        { data: base64Data, filename }
      );
      if (!saveResult.success || !saveResult.filePath) {
        console.error('[IMG_SEND] Failed to save temp file:', saveResult.error);
        return false;
      }
      const result = await invoke<{ success: boolean; transferId?: string; fileName?: string; error?: string }>(
        'file.send',
        { target, filePath: saveResult.filePath }
      );
      if (result.success) {
        const displayName = result.fileName || filename;
        const msg: Message = {
          id: result.transferId || Date.now().toString(),
          from: 'self',
          to: target,
          content: displayName,
          type: 'image',
          timestamp: Date.now(),
          status: 'sending',
          fileInfo: {
            fileName: displayName,
            fileSize: 0,
            filePath: saveResult.filePath,
            transferId: result.transferId,
          },
          transferProgress: 0,
        };
        get().recvMessage(msg);
        return true;
      }
      console.error('[IMG_SEND] file.send failed:', result.error);
      return false;
    } catch (err) {
      console.error('sendImage error:', err);
      return false;
    }
  },

  sendFile: async (target, base64Data, filename) => {
    console.log(`[FILE_SEND] target=${target}, filename=${filename}, dataSize=${base64Data.length}`);
    try {
      const saveResult = await invoke<{ success: boolean; filePath?: string; error?: string }>(
        'file.save_temp',
        { data: base64Data, filename }
      );

      if (!saveResult.success || !saveResult.filePath) {
        console.error('Failed to save temp file:', saveResult.error);
        return false;
      }

      const result = await invoke<{ success: boolean; transferId?: string; fileName?: string; error?: string }>(
        'file.send',
        { target, filePath: saveResult.filePath }
      );

      if (result.success) {
        // Use backend's fileName (timestamp prefix stripped) if available
        const displayName = result.fileName || filename;
        console.log(`[FILE_SEND] result: transferId=${result.transferId}, fileName=${result.fileName}, displayName=${displayName}`);
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
            fileSize: 0,
            filePath: saveResult.filePath,
            transferId: result.transferId,
          },
          transferProgress: 0,
        };
        get().recvMessage(msg);
        return true;
      }
      return false;
    } catch (err) {
      console.error('sendFile error:', err);
      return false;
    }
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
          return { messages: newMessages, messageVersion: state.messageVersion + 1 };
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
          return { messages: newMessages, messageVersion: state.messageVersion + 1 };
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
      return { messages: newMessages, messageVersion: state.messageVersion + 1 };
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
      return { messages: newMessages, messageVersion: state.messageVersion + 1 };
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

          // For file messages loaded from history:
          // - If sent by local user && status <= 1 => still pending transfer
          // - If received && status <= 1 => waiting for acceptance (need to re-trigger)
          // - If status >= 2 => completed
          let transferProgress: number | undefined = undefined;
          let msgStatus: 'sending' | 'sent' | 'delivered' | 'failed' = m.status === 0 ? 'sending' : m.status === 1 ? 'delivered' : m.status === 2 ? 'delivered' : 'failed';

          if (isFileMsg) {
            if (isSentByMe) {
              // Sent files: status 0/1 means still pending/sending, 2 means completed
              transferProgress = m.status >= 2 ? 100 : 0;
            } else {
              // Received files from history: status 1 = delivered = file notification received
              // but no actual transfer happened yet on this session
              // Mark as -1 to show waiting-for-acceptance UI (user needs to accept again)
              if (m.status < 2) {
                transferProgress = -1;
                msgStatus = 'sending';
              } else {
                transferProgress = 100;
              }
            }
          }

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
            return inFlight;
          });

          // Combine: history + realtime-only, sorted by timestamp
          const combined = [...msgs, ...realtimeOnly].sort((a, b) => a.timestamp - b.timestamp);
          newMessages.set(userId, combined);
          return { messages: newMessages };
        });
      }
    } catch (err: any) {
      set({ error: err.message });
    } finally {
      set({ loading: false });
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
          return { messages: newMessages };
        });
      } else {
        set({ messages: new Map() });
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

        // Auto-select the sender in ChatPanel so the message is visible immediately
        const currentUser = userStore.currentUser;
        if (!currentUser || currentUser.id !== data.from) {
          const sender = userStore.users.find(u => u.id === data.from);
          if (sender) {
            console.log('[MSGRECV] Auto-selecting user:', sender.id);
            userStore.setCurrentUser(sender);
          }
        }
      }
      // For file attachments, the message was already added by file.receive_request handler
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
          return { messages: newMessages, messageVersion: state.messageVersion + 1 };
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
        return { messages: newMessages, messageVersion: state.messageVersion + 1 };
      });
    }));

    // Listen for FeiQ inline screenshots, which are reassembled entirely on the
    // backend and delivered as a finished image (inline base64 data URL). This
    // bypasses the standard "accept file" UI used by normal file transfers.
    // Guard against duplicate delivery (UDP retransmit / accidental double listener).
    unsubs.push(listen('feiq.screenshot_received', (data: any) => {
      console.log(`[FEIQ_SHOT] from=${data.fromUser?.id}, fileName=${data.fileName}, size=${data.fileSize}`);

      // Dedupe: the same screenshot delivered twice (retransmit or double listener)
      // would otherwise create two preview records. Key by sender+size+dataUrl length.
      const sig = `${data.fromUser?.id}|${data.fileSize}|${(data.dataUrl || '').length}`;
      const now = Date.now();
      if (sig === lastFeiqSig && now - lastFeiqAt < 2000) {
        console.log('[FEIQ_SHOT] Duplicate delivery ignored (same sig within 2s)');
        return;
      }
      lastFeiqSig = sig;
      lastFeiqAt = now;

      // Auto-add the sender to the contact list if missing, and auto-select so
      // the incoming screenshot becomes visible immediately.
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
        const current = userStore.currentUser;
        if (!current || current.id !== data.fromUser.id) {
          const sender = userStore.users.find(u => u.id === data.fromUser.id);
          if (sender) userStore.setCurrentUser(sender);
        }
      }

      const msg: Message = {
        id: `feiq_${Date.now()}_${Math.random().toString(36).slice(2)}`,
        from: data.fromUser?.id || 'unknown',
        to: 'self',
        content: data.dataUrl,
        type: 'image',
        timestamp: Date.now(),
        status: 'delivered',
        fromUser: data.fromUser,
        fileInfo: { fileName: data.fileName, fileSize: data.fileSize || 0, filePath: data.savePath },
        transferProgress: 100,
        isFeiqShot: true,
      };
      get().recvMessage(msg);
    }));

    return () => {
      unsubs.forEach(unsub => unsub());
    };
  },
}));
