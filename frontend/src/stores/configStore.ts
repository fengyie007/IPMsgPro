// ============================================================================
// Config Store - Zustand state management for configuration
// Uses IndexedDB for persistent storage (not C++ backend)
// ============================================================================

import { create } from 'zustand';
import { Config, DEFAULT_CONFIG } from '../types';
import { configDB } from '../services/configDB';
import { invoke } from '../services/bridge';

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
      // reset it to the default so the correct "C:\Users\<user>\.ipmsgpro" is shown.
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

      // Apply saved nickname to backend immediately on startup
      if (config.nickname && config.nickname.length > 0) {
        console.log('[ConfigStore] Applying saved nickname to backend:', config.nickname);
        try {
          await invoke('config.set', { nickname: config.nickname });
        } catch (e) {
          console.error('[ConfigStore] Failed to set nickname on backend:', e);
        }
      }

      // Apply saved dataDir to backend immediately on startup
      if (config.dataDir) {
        console.log('[ConfigStore] Applying saved dataDir to backend:', config.dataDir);
        try {
          await invoke('config.set', { dataDir: config.dataDir });
        } catch (e) {
          console.error('[ConfigStore] Failed to set dataDir on backend:', e);
        }
      }

      // Apply saved minimizeBehavior to backend
      try {
        await invoke('config.set', { minimizeBehavior: config.minimizeBehavior });
      } catch (e) {
        console.error('[ConfigStore] Failed to set minimizeBehavior on backend:', e);
      }

      // Apply saved notificationSound to backend
      try {
        await invoke('config.set', { notificationSound: config.notificationSound });
      } catch (e) {
        console.error('[ConfigStore] Failed to set notificationSound on backend:', e);
      }

      // Apply saved group to backend
      if (config.group) {
        console.log('[ConfigStore] Applying saved group to backend:', config.group);
        try {
          await invoke('config.set', { group: config.group });
        } catch (e) {
          console.error('[ConfigStore] Failed to set group on backend:', e);
        }
      }

      // Apply saved segments to backend
      if (config.segments && config.segments.length > 0) {
        console.log('[ConfigStore] Applying saved segments to backend:', config.segments);
        try {
          await invoke('config.set', { segments: config.segments });
        } catch (e) {
          console.error('[ConfigStore] Failed to set segments on backend:', e);
        }
      }

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
      console.log('[ConfigStore] Config saved successfully');

      // Notify backend of nickname changes
      const nickname = partial.nickname;
      if (nickname !== undefined) {
        console.log('[ConfigStore] Notifying backend of nickname change:', nickname);
        await invoke('config.set', { nickname });
      }

      // Notify backend of dataDir changes
      const dataDir = partial.dataDir;
      if (dataDir !== undefined) {
        console.log('[ConfigStore] Notifying backend of dataDir change:', dataDir);
        await invoke('config.set', { dataDir });
      }

      // Notify backend of minimizeBehavior changes
      const minimizeBehavior = partial.minimizeBehavior;
      if (minimizeBehavior !== undefined) {
        console.log('[ConfigStore] Notifying backend of minimizeBehavior change:', minimizeBehavior);
        await invoke('config.set', { minimizeBehavior });
      }

      // Notify backend of notificationSound changes
      const notificationSound = partial.notificationSound;
      if (notificationSound !== undefined) {
        console.log('[ConfigStore] Notifying backend of notificationSound change:', notificationSound);
        await invoke('config.set', { notificationSound });
      }

      // Notify backend of group changes
      const group = partial.group;
      if (group !== undefined) {
        console.log('[ConfigStore] Notifying backend of group change:', group);
        await invoke('config.set', { group });
      }

      // Notify backend of segments changes
      const segments = partial.segments;
      if (segments !== undefined) {
        console.log('[ConfigStore] Notifying backend of segments change:', segments);
        await invoke('config.set', { segments });
      }
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
