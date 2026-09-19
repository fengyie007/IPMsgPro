// ============================================================================
// Config Store - Zustand state management for configuration
// Uses IndexedDB for persistent storage (not C++ backend)
// ============================================================================

import { create } from 'zustand';
import { Config, DEFAULT_CONFIG } from '../types';
import { configDB } from '../services/configDB';
import { invoke } from '../services/bridge';

/**
 * Config keys the C++ backend consumes (HandleConfigSet). One `config.set`
 * call carries every key present in the payload; the backend only touches the
 * keys it receives.
 */
const BACKEND_KEYS = [
  'nickname', 'group', 'dataDir', 'minimizeBehavior', 'notificationSound',
  'segments', 'directUsers', 'ipScanRanges',
] as const satisfies readonly (keyof Config)[];

function backendPayload(partial: Partial<Config>): Partial<Config> {
  const payload: Partial<Config> = {};
  for (const key of BACKEND_KEYS) {
    if (partial[key] !== undefined) (payload as any)[key] = partial[key];
  }
  return payload;
}

async function pushToBackend(payload: Partial<Config>, context: string) {
  if (Object.keys(payload).length === 0) return;
  try {
    await invoke('config.set', payload);
  } catch (e) {
    console.error(`[ConfigStore] config.set failed (${context}):`, e);
  }
}

interface ConfigStore {
  config: Config;
  loaded: boolean;

  /** Load config from IndexedDB */
  loadConfig: () => Promise<void>;

  /** Save config to IndexedDB (and optionally notify backend) */
  saveConfig: (partial: Partial<Config>) => Promise<void>;

  /** Reset config to defaults */
  resetConfig: () => Promise<void>;
}

export const useConfigStore = create<ConfigStore>((set, get) => ({
  config: { ...DEFAULT_CONFIG },
  loaded: false,

  loadConfig: async () => {
    try {
      console.log('[ConfigStore] Loading config from IndexedDB...');
      await configDB.init();
      const config = await configDB.loadConfig();
      console.log('[ConfigStore] Config loaded successfully:', JSON.stringify(config));

      // Repair: if a previously saved dataDir is exactly the user's home directory
      // (mistakenly persisted when the folder picker opened at home and OK was clicked),
      // reset it to the default so the correct "C:\Users\<user>\.speedipmsg" is shown.
      const home = (typeof window !== 'undefined' && (window as any).__tauricpp__?.homeDir) || '';
      const normalizePath = (p: string) => p.replace(/\//g, '\\').toLowerCase();
      if (config.dataDir && home && normalizePath(config.dataDir) === normalizePath(home)) {
        console.log('[ConfigStore] Repairing mistyped dataDir (== home):', config.dataDir);
        config.dataDir = '';
        try {
          await configDB.saveConfig({ dataDir: '' });
          await invoke('config.set', { dataDir: '' });
        } catch (e) {
          console.error('[ConfigStore] Failed to repair dataDir:', e);
        }
      }

      set({ config, loaded: true });

      // Replay the persisted config to the backend in ONE call. An empty
      // dataDir is omitted: the backend already runs on the default directory,
      // and sending '' would make it re-open the log and database for nothing.
      const payload = backendPayload(config);
      if (!config.dataDir) delete payload.dataDir;
      await pushToBackend(payload, 'startup');
    } catch (err) {
      console.error('[ConfigStore] Failed to load config:', err);
      set({ loaded: true });
    }
  },

  saveConfig: async (partial) => {
    const newConfig = { ...get().config, ...partial };
    set({ config: newConfig });

    try {
      console.log('[ConfigStore] Saving config to IndexedDB:', JSON.stringify(partial));
      await configDB.saveConfig(partial);
      // Forward only the changed backend-relevant keys, in one call.
      await pushToBackend(backendPayload(partial), 'save');
    } catch (err) {
      console.error('[ConfigStore] Failed to save config:', err);
    }
  },

  resetConfig: async () => {
    set({ config: { ...DEFAULT_CONFIG } });
    try {
      await configDB.clear();
    } catch (err) {
      console.error('Failed to reset config:', err);
    }
  },
}));
