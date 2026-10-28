import { create } from 'zustand';
import { invoke, listen } from '../services/bridge';
import { EMPTY_SCAN, type ScanOptions, type ScanStatus } from '../types';
import { toast } from './toastStore';

export const scanActive = (state: ScanStatus['state']) => state === 'running' || state === 'waiting' || state === 'cancelling';
function validScan(data: ScanStatus): boolean {
  return !!data && ['idle','running','waiting','cancelling','completed','cancelled','failed'].includes(data.state) &&
    [data.scanId, data.revision, data.current, data.total, data.found, data.failedSends, data.skipped].every((n) => Number.isSafeInteger(n) && n >= 0) &&
    data.current <= data.total && data.total <= 65536 && data.found <= data.current && data.failedSends + data.skipped <= data.current &&
    Array.isArray(data.ranges) && data.ranges.every((r) => typeof r === 'string');
}
interface ScanStore {
  status: ScanStatus; busy: boolean; refreshing: boolean; error: string | null;
  update: (status: ScanStatus) => void;
  refresh: () => Promise<void>;
  start: (options: ScanOptions) => Promise<boolean>;
  cancel: () => Promise<void>;
  initListeners: () => () => void;
}
export const useScanStore = create<ScanStore>((set, get) => ({
  status: { ...EMPTY_SCAN }, busy: false, refreshing: false, error: null,
  update: (status) => {
    if (!validScan(status)) return;
    const previous = get().status;
    if (status.scanId < previous.scanId || (status.scanId === previous.scanId &&
        (status.revision <= previous.revision || (!scanActive(previous.state) && previous.state !== 'idle')))) return;
    set({ status, error: null });
  },
  refresh: async () => {
    if (get().refreshing) return;
    set({ refreshing: true });
    try {
      const result = await invoke<{ scan: ScanStatus }>('network.scan_status');
      if (!validScan(result.scan)) throw new Error('后端返回了无效扫描状态');
      get().update(result.scan); set({ error: null });
    } catch (error) { set({ error: String(error) }); }
    finally { set({ refreshing: false }); }
  },
  start: async (options) => {
    if (get().busy || scanActive(get().status.state)) return false;
    set({ busy: true, error: null });
    try {
      const { ranges, port, delayMs } = options;
      const result = await invoke<{ scan: ScanStatus }>('network.scan_range', { ranges, port, delayMs });
      if (!validScan(result.scan)) throw new Error('后端未返回有效扫描任务');
      get().update(result.scan); return true;
    } catch (error) { set({ error: String(error) }); toast.error('无法开始扫描：' + String(error)); void get().refresh(); return false; }
    finally { set({ busy: false }); }
  },
  cancel: async () => {
    if (get().busy || !scanActive(get().status.state)) return;
    const scanId = get().status.scanId;
    set({ busy: true, error: null });
    try {
      const result = await invoke<{ scan: ScanStatus }>('network.scan_cancel', { scanId });
      if (!validScan(result.scan)) throw new Error('后端未返回有效取消结果');
      get().update(result.scan);
    } catch (error) { set({ error: String(error) }); toast.error('无法取消扫描：' + String(error)); void get().refresh(); }
    finally { set({ busy: false }); }
  },
  initListeners: () => {
    const unlisten = [listen('network.scan_progress', (data: ScanStatus) => useScanStore.getState().update(data)),
      listen('network.scan_complete', (data: ScanStatus) => useScanStore.getState().update(data))];
    return () => unlisten.forEach((stop) => stop());
  },
}));
