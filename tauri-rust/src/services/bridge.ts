import { invoke as nativeInvoke, isTauri } from '@tauri-apps/api/core';
import { listen as nativeListen, type UnlistenFn } from '@tauri-apps/api/event';
import { APP_VERSION, DEFAULT_CONFIG, MVP_CAPABILITIES, type Config, type HistoryRecord, type User } from '../types';

type Callback = (payload: any) => void;
interface Envelope { event: string; payload: unknown }
const listeners = new Map<string, Set<Callback>>();
let ready: Promise<void> | undefined;
let nativeUnlisten: UnlistenFn | undefined;
let disposed = false;

export const isMockMode = !isTauri() && import.meta.env.VITE_MOCK_BRIDGE === '1';
const supported = new Set([
  'app.info', 'config.get', 'config.set', 'config.loaded',
  'user.local', 'user.list', 'user.discover', 'message.send',
  'history.get', 'history.get_recent', 'history.search', 'history.clear', 'image.read',
  'image.select', 'image.send', 'image.cancel', 'image.discard',
  'window.set_active_conversation', 'frontend.error',
]);

function dispatch(envelope: Envelope) {
  if (disposed || !envelope || typeof envelope.event !== 'string') return;
  for (const callback of [...(listeners.get(envelope.event) || [])]) {
    try { callback(envelope.payload); }
    catch (error) { console.error('[Bridge] Event callback failed', envelope.event, error); }
  }
}

// One native subscription wraps all legacy domain.action event names, because
// Tauri event names cannot contain dots. All startup network work awaits this.
export function bridgeReady(): Promise<void> {
  if (disposed) return Promise.reject(new Error('桥接已关闭'));
  if (isMockMode) return Promise.resolve();
  if (!isTauri()) return Promise.reject(new Error('未连接 Rust 后端，请通过 Tauri 启动；浏览器演示须显式设置 VITE_MOCK_BRIDGE=1'));
  if (!ready) {
    ready = nativeListen<Envelope>('ipmsg-event', (event) => dispatch(event.payload))
      .then((unlisten) => {
        if (disposed) {
          unlisten();
          throw new Error('桥接已关闭');
        }
        nativeUnlisten = unlisten;
      })
      .catch((error) => {
        ready = undefined;
        throw error;
      });
  }
  return ready;
}

// Remains synchronous for Zustand/React cleanup. StrictMode can remove callbacks
// before the asynchronous native subscription completes without leaking them.
export function listen(event: string, callback: Callback): () => void {
  const callbacks = listeners.get(event) || new Set<Callback>();
  callbacks.add(callback);
  listeners.set(event, callbacks);
  return () => {
    callbacks.delete(callback);
    if (!callbacks.size) listeners.delete(event);
  };
}

export async function invoke<T = any>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!supported.has(command)) throw new Error(`Rust 核心版暂不支持：${command}`);
  await bridgeReady();
  const result = isMockMode
    ? mockResponse(command, args)
    : await nativeInvoke<T>('ipmsg_command', { command, args });
  if (result && typeof result === 'object' && 'success' in result && result.success === false) {
    throw new Error('error' in result ? String(result.error) : `${command} 执行失败`);
  }
  return result as T;
}

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    disposed = true;
    nativeUnlisten?.();
    nativeUnlisten = undefined;
    listeners.clear();
  });
}

// Explicit browser-only demonstration; never masks a missing Tauri runtime.
const mockLocalId = 'me-rust-2427@mock';
const mockUsers: User[] = [
  { id: 'demo@localhost', nickname: '演示联系人', username: 'demo', hostname: 'localhost', group: '测试组', ip: '127.0.0.1', port: 2425, status: 'online', version: '' },
  { id: 'guest@localhost', nickname: '访客', username: 'guest', hostname: 'localhost', group: '', ip: '127.0.0.1', port: 2426, status: 'away', version: '' },
];
let mockConfig: Config = { ...DEFAULT_CONFIG };
const mockImageSvg = '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180"><rect width="320" height="180" fill="#dcfce7"/><text x="160" y="95" text-anchor="middle" font-size="22" fill="#166534">Rust Image Preview</text></svg>';
const mockImageUrl = 'data:image/svg+xml,' + encodeURIComponent(mockImageSvg);
let mockRows: HistoryRecord[] = [{
  id: 'mock-received-image', fromId: mockUsers[0].id, toId: mockLocalId,
  content: '[图片]', type: 1, timestamp: Math.floor(Date.now() / 1000), status: 1,
  image: { assetId: 'mock-image', fileName: '演示图片.svg', fileSize: mockImageSvg.length, mime: 'image/svg+xml', width: 320, height: 180 },
}];
let mockSequence = 0;
const mockAssets = new Map([['mock-image', mockRows[0].image!]]);
const mockTemporary = new Set<string>();
const mockTasks = new Map<string, { target: string; timers: ReturnType<typeof setTimeout>[] }>();

function mockResponse(command: string, args: Record<string, unknown>): unknown {
  const localUser = {
    id: mockLocalId, nickname: mockConfig.nickname || 'Rust 演示用户', username: 'me-rust-2427',
    hostname: 'mock', group: mockConfig.group, ip: '127.0.0.1', port: 2427, status: 'online', version: APP_VERSION,
  };
  switch (command) {
    case 'app.info': return { success: true, version: APP_VERSION, dataDir: '浏览器演示：不写入磁盘', port: 2427, capabilities: MVP_CAPABILITIES };
    case 'config.get': return { success: true, config: mockConfig };
    case 'image.read':
      return mockAssets.has(String(args.assetId))
        ? { success: true, url: mockImageUrl }
        : { success: false, error: '浏览器演示中没有该图片' };
    case 'image.select': {
      if (mockAssets.size >= 64) return { success: false, error: '演示图片数量已达上限' };
      const assetId = `mock-import-${++mockSequence}`;
      const image = { ...mockRows[0]?.image, assetId, fileName: '演示图片.png', fileSize: mockImageSvg.length, mime: 'image/png', width: 320, height: 180 };
      mockAssets.set(assetId, image); mockTemporary.add(assetId);
      return { success: true, image };
    }
    case 'image.send': {
      const image = mockAssets.get(String(args.assetId)), target = String(args.target);
      if (!image || !mockUsers.some((user) => user.id === target)) return { success: false, error: '演示图片或目标不存在' };
      if (mockTasks.size >= 4) return { success: false, error: '演示队列已满' };
      const messageId = `mock-image-send-${Date.now()}-${++mockSequence}`, imageId = mockSequence.toString(16).padStart(8, '0');
      mockTemporary.delete(image.assetId);
      mockRows.push({ id: messageId, fromId: mockLocalId, toId: target, content: '[图片]', type: 1, timestamp: Date.now()/1000, status: 0, image });
      const data = { messageId, target };
      const timers = [
        setTimeout(() => { if (mockTasks.has(messageId)) dispatch({ event: 'image.send_progress', payload: { ...data, progress: 60, stage: 'transferring' } }); }, 150),
        setTimeout(() => {
          if (!mockTasks.delete(messageId)) return;
          const row = mockRows.find((row) => row.id === messageId); if (row) row.status = 1;
          dispatch({ event: 'image.send_completed', payload: { ...data, progress: 100 } });
        }, 400),
      ];
      mockTasks.set(messageId, { target, timers });
      dispatch({ event: 'image.send_progress', payload: { ...data, progress: 0, stage: 'queued' } });
      return { success: true, messageId, imageId, image };
    }
    case 'image.cancel': {
      const messageId = String(args.messageId), task = mockTasks.get(messageId);
      if (!task) return { success: true, cancelled: false };
      task.timers.forEach(clearTimeout); mockTasks.delete(messageId);
      const row = mockRows.find((row) => row.id === messageId); if (row) row.status = 3;
      dispatch({ event: 'image.send_failed', payload: { messageId, target: task.target, error: '图片发送已取消', cancelled: true } });
      return { success: true, cancelled: true };
    }
    case 'image.discard': {
      const id = String(args.assetId), discarded = mockTemporary.delete(id);
      if (discarded) mockAssets.delete(id);
      return { success: true, discarded };
    }
    case 'config.set':
      mockConfig = { ...mockConfig, ...args } as Config;
      return { success: true, config: mockConfig };
    case 'user.local': return { success: true, ...localUser };
    case 'user.list': return { success: true, users: mockUsers, count: mockUsers.length };
    case 'user.discover':
      for (const user of mockUsers) dispatch({ event: 'user.discovered', payload: user });
      return { success: true };
    case 'config.loaded':
    case 'window.set_active_conversation':
    case 'frontend.error': return { success: true };
    case 'message.send': {
      const id = `mock_${Date.now()}_${++mockSequence}`;
      mockRows.push({ id, fromId: mockLocalId, toId: String(args.target), content: String(args.content), type: 0, timestamp: Date.now() / 1000, status: 1 });
      // Exercise a receipt arriving before invoke returns to the message store.
      dispatch({ event: 'message.ack', payload: { messageId: id } });
      return { success: true, messageId: id };
    }
    case 'history.get': {
      const user = String(args.userId);
      const rows = mockRows.filter((r) => r.fromId === user || r.toId === user);
      const offset = Math.max(0, Number(args.offset) || 0), limit = Math.max(1, Number(args.limit) || 50);
      return { success: true, messages: rows.slice().reverse().slice(offset, offset + limit).reverse(), localUserId: mockLocalId };
    }
    case 'history.get_recent': {
      const latest = new Map<string, HistoryRecord>();
      for (const row of mockRows) latest.set(row.fromId === mockLocalId ? row.toId : row.fromId, row);
      return { success: true, messages: [...latest.values()], localUserId: mockLocalId };
    }
    case 'history.search': return {
      success: true,
      messages: mockRows.filter((r) => (!args.userId || r.fromId === args.userId || r.toId === args.userId) && r.content.toLowerCase().includes(String(args.keyword || '').toLowerCase())).slice(-200).reverse(),
      localUserId: mockLocalId,
    };
    case 'history.clear': {
      const deletedIds = mockRows.filter((r) => !args.userId || r.fromId === args.userId || r.toId === args.userId).map((r) => r.id);
      const deleted = new Set(deletedIds);
      mockRows = mockRows.filter((r) => !deleted.has(r.id));
      return { success: true, deletedIds };
    }
    default: throw new Error(`未实现命令：${command}`);
  }
}
