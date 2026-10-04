import { create } from 'zustand';
import { invoke } from '../services/bridge';
import { DEFAULT_CONFIG, type AppInfo, type Config } from '../types';

function parseConfig(value: Partial<Config>): Config {
  if (!value || typeof value.nickname !== 'string' || typeof value.group !== 'string' ||
      !Array.isArray(value.directUsers) || value.directUsers.some((v) => typeof v !== 'string') ||
      (value.minimizeBehavior !== 'tray' && value.minimizeBehavior !== 'taskbar')) {
    throw new Error('后端返回了无效设置');
  }
  return { ...DEFAULT_CONFIG, ...value };
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
    for (const key of ['nickname', 'group', 'directUsers', 'minimizeBehavior'] as const) {
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
    });
  },
}));
