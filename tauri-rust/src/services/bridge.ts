import { invoke as nativeInvoke, isTauri } from '@tauri-apps/api/core';
import { listen as nativeListen, type UnlistenFn } from '@tauri-apps/api/event';
import { APP_VERSION, DEFAULT_CONFIG, MVP_CAPABILITIES, EMPTY_SCAN, type Config, type HistoryRecord, type User, type ScanStatus } from '../types';
import { validateScanOptions } from '../utils/netValidation';

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
  'screenshot.start',
  'notification.test_sound',
  'notification.test_system', 'notification.take_activation',
  'storage.info', 'storage.select', 'storage.default', 'storage.apply', 'storage.cancel',
  'network.scan_range', 'network.scan_cancel', 'network.scan_status',
  'file.select', 'file.send', 'file.discard', 'file.accept', 'file.reject', 'file.cancel', 'file.open_folder',
  'file.select_folder', 'file.resume', 'file.pause',
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
    if (mockScanTimer) clearInterval(mockScanTimer);
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
let mockScan: ScanStatus = { ...EMPTY_SCAN };
let mockScanTimer: ReturnType<typeof setInterval> | undefined;
let mockStartupHandled = false;
const mockScanActive = () => ['running', 'waiting', 'cancelling'].includes(mockScan.state);
function startMockScan(args: Record<string, unknown>) {
  if (mockScanActive()) return { success: false, error: '已有扫描正在进行，请先取消' };
  if (!Array.isArray(args.ranges) || args.ranges.some((r) => typeof r !== 'string')) return { success: false, error: '扫描范围无效' };
  const parsed = validateScanOptions(args.ranges, args.port as number, args.delayMs as number);
  if ('error' in parsed) return { success: false, error: parsed.error };
  if (!parsed.value.total) return { success: false, error: '请至少配置一个扫描范围' };
  const options = parsed.value;
  mockScan = { ...EMPTY_SCAN, scanId: mockScan.scanId + 1, revision: 1, state: 'running', total: options.total,
    ranges: options.ranges, port: options.port, delayMs: options.delayMs };
  dispatch({ event: 'network.scan_progress', payload: { ...mockScan } });
  mockScanTimer = setInterval(() => {
    const current = Math.min(mockScan.total, mockScan.current + Math.max(1, Math.ceil(mockScan.total / 10)));
    if (mockScan.state === 'waiting') {
      const ipNumber = (ip: string) => ip.split('.').reduce((n, part) => n * 256 + Number(part), 0);
      const responding = new Set(mockUsers.filter((user) => user.port === options.port && options.ranges.some((range) => {
        const [start, end] = range.split('-').map(ipNumber); return ipNumber(user.ip) >= start && ipNumber(user.ip) <= end;
      })).map((user) => `${user.ip}:${user.port}`));
      mockScan = { ...mockScan, revision: mockScan.revision + 1, state: 'completed', found: responding.size };
      clearInterval(mockScanTimer); mockScanTimer = undefined;
      dispatch({ event: 'network.scan_complete', payload: { ...mockScan } });
    } else {
      mockScan = { ...mockScan, revision: mockScan.revision + 1, current, state: current === mockScan.total ? 'waiting' : 'running' };
      dispatch({ event: 'network.scan_progress', payload: { ...mockScan } });
    }
  }, 100);
  return { success: true, scan: { ...mockScan } };
}

function mockResponse(command: string, args: Record<string, unknown>): unknown {
  const localUser = {
    id: mockLocalId, nickname: mockConfig.nickname || 'Rust 演示用户', username: 'me-rust-2427',
    hostname: 'mock', group: mockConfig.group, ip: '127.0.0.1', port: 2427, status: 'online', version: APP_VERSION,
  };
  switch (command) {
    case 'storage.info': return { success: true, active: '浏览器演示，不写磁盘', pending: null, defaultDirectory: '', isDefault: true, error: null };
    case 'storage.select': case 'storage.default': case 'storage.apply': case 'storage.cancel': return { success: false, error: '目录切换需要桌面版' };
    case 'notification.test_system': return { success: false, error: '系统通知需要 Windows 桌面版' };
    case 'notification.take_activation': return { success: true, userId: null };
    case 'notification.test_sound': return { success: false, error: '提示音试听需要 Windows 桌面版，浏览器演示不播放原生音效' };
    case 'network.scan_range': return startMockScan(args);
    case 'network.scan_status': return { success: true, scan: { ...mockScan } };
    case 'network.scan_cancel': {
      if (args.scanId !== mockScan.scanId) return { success: false, error: '扫描任务已变更，请刷新状态' };
      if (mockScanActive()) {
        if (mockScanTimer) clearInterval(mockScanTimer); mockScanTimer = undefined;
        mockScan = { ...mockScan, revision: mockScan.revision + 1, state: 'cancelled' };
        dispatch({ event: 'network.scan_complete', payload: { ...mockScan } });
      }
      return { success: true, scan: { ...mockScan } };
    }
    case 'file.select':
    case 'file.select_folder':
    case 'file.pause':
    case 'file.resume':
    case 'file.send':
    case 'file.accept':
    case 'file.reject':
    case 'file.cancel':
    case 'file.open_folder': return { success: false, error: '文件传输需要桌面版，浏览器演示未连接文件服务' };
    case 'file.discard': return { success: true };
    case 'screenshot.start': return { success: false, error: '截图需要在 Windows 桌面版中使用' };
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
    case 'config.set': {
      const next = { ...mockConfig, ...args } as Config;
      if (typeof next.notificationSound !== 'boolean') return { success: false, error: '提示音开关必须是布尔值' };
      if (!Array.isArray(next.ipScanRanges) || next.ipScanRanges.some((r) => typeof r !== 'string') || typeof next.scanOnStartup !== 'boolean') return { success: false, error: '扫描设置无效' };
      const parsed = validateScanOptions(next.ipScanRanges, next.scanPort, next.scanDelayMs);
      if ('error' in parsed) return { success: false, error: parsed.error };
      mockConfig = next;
      return { success: true, config: mockConfig };
    }
    case 'user.local': return { success: true, ...localUser };
    case 'user.list': return { success: true, users: mockUsers, count: mockUsers.length };
    case 'user.discover':
      for (const user of mockUsers) dispatch({ event: 'user.discovered', payload: user });
      return { success: true };
    case 'config.loaded':
      if (!mockStartupHandled) {
        mockStartupHandled = true;
        if (mockConfig.scanOnStartup && mockConfig.ipScanRanges.length) startMockScan({ ranges: mockConfig.ipScanRanges, port: mockConfig.scanPort, delayMs: mockConfig.scanDelayMs });
      }
      return { success: true };
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
