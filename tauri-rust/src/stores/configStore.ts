import { create } from 'zustand';
import { invoke } from '../services/bridge';
import { DEFAULT_CONFIG, type AppInfo, type Config } from '../types';
import { validateScanOptions } from '../utils/netValidation';

function parseConfig(value: Partial<Config>): Config {
  if (!value || typeof value.nickname !== 'string' || typeof value.group !== 'string' ||
      !Array.isArray(value.directUsers) || value.directUsers.some((v) => typeof v !== 'string') ||
      (value.minimizeBehavior !== 'tray' && value.minimizeBehavior !== 'taskbar')) {
    throw new Error('后端返回了无效设置');
  }
  const config = { ...DEFAULT_CONFIG, ...value };
  if (typeof config.notificationSound !== 'boolean') throw new Error('后端返回了无效提示音设置');
  if (typeof config.systemNotifications !== 'boolean' || typeof config.notificationPreview !== 'boolean') throw new Error('后端返回了无效系统通知设置');
  if (!Array.isArray(config.ipScanRanges) || config.ipScanRanges.some((r) => typeof r !== 'string') ||
      typeof config.scanOnStartup !== 'boolean' || 'error' in validateScanOptions(config.ipScanRanges, config.scanPort, config.scanDelayMs)) {
    throw new Error('后端返回了无效扫描设置');
  }
  return config;
}

interface ConfigStore {
  config: Config;
  info: AppInfo | null;
  loaded: boolean;
  loadConfig: () => Promise<void>;
  saveConfig: (partial: Partial<Config>) => Promise<void>;
  resetConfig: () => Promise<void>;
}

export const useConfigStore = create<ConfigStore>((set, get) => ({
  config: { ...DEFAULT_CONFIG }, info: null, loaded: false,
  loadConfig: async () => {
    const [info, result] = await Promise.all([
      invoke<AppInfo>('app.info'),
      invoke<{ success: boolean; config: Config }>('config.get'),
    ]);
    if (!info.success || typeof info.dataDir !== 'string' || !Number.isInteger(info.port)) {
      throw new Error('无法读取 Rust 运行环境');
    }
    set({ info, config: parseConfig(result.config), loaded: true });
  },
  saveConfig: async (partial) => {
    const payload: Record<string, unknown> = {};
    for (const key of ['nickname', 'group', 'directUsers', 'minimizeBehavior', 'ipScanRanges', 'scanPort', 'scanDelayMs', 'scanOnStartup', 'notificationSound', 'systemNotifications', 'notificationPreview'] as const) {
      if (partial[key] !== undefined) payload[key] = partial[key];
    }
    const result = await invoke<{ success: boolean; config: Config }>('config.set', payload);
    // Rust validates, persists and applies the settings before publishing success.
    set({ config: parseConfig(result.config) });
  },
  resetConfig: async () => {
    await get().saveConfig({
      nickname: DEFAULT_CONFIG.nickname, group: DEFAULT_CONFIG.group,
      directUsers: [], minimizeBehavior: DEFAULT_CONFIG.minimizeBehavior,
      ipScanRanges: [], scanPort: DEFAULT_CONFIG.scanPort, scanDelayMs: DEFAULT_CONFIG.scanDelayMs, scanOnStartup: DEFAULT_CONFIG.scanOnStartup,
      notificationSound: DEFAULT_CONFIG.notificationSound,
      systemNotifications: false, notificationPreview: false,
    });
  },
}));
