// ============================================================================
// IndexedDB Configuration Storage Service
// Manages user preferences and settings in the browser's IndexedDB
// ============================================================================

import { Config, DEFAULT_CONFIG } from '../types';

const DB_NAME = 'ipmsg-config';
const DB_VERSION = 1;
const STORE_NAME = 'config';

class ConfigDB {
  private db: IDBDatabase | null = null;

  /** Initialize the IndexedDB database */
  async init(): Promise<void> {
    return new Promise((resolve, reject) => {
      const request = indexedDB.open(DB_NAME, DB_VERSION);

      request.onupgradeneeded = () => {
        const db = request.result;
        if (!db.objectStoreNames.contains(STORE_NAME)) {
          db.createObjectStore(STORE_NAME);
        }
      };

      request.onsuccess = () => {
        this.db = request.result;
        resolve();
      };

      request.onerror = () => {
        reject(request.error);
      };
    });
  }

  /** Get a config value by key */
  async get<T = any>(key: string): Promise<T | null> {
    if (!this.db) await this.init();

    return new Promise((resolve, reject) => {
      const tx = this.db!.transaction(STORE_NAME, 'readonly');
      const store = tx.objectStore(STORE_NAME);
      const request = store.get(key);

      request.onsuccess = () => {
        resolve(request.result ?? null);
      };

      request.onerror = () => {
        reject(request.error);
      };
    });
  }

  /** Set a config value by key */
  async set(key: string, value: any): Promise<void> {
    if (!this.db) await this.init();

    return new Promise((resolve, reject) => {
      const tx = this.db!.transaction(STORE_NAME, 'readwrite');
      const store = tx.objectStore(STORE_NAME);
      const request = store.put(value, key);

      request.onsuccess = () => resolve();
      request.onerror = () => reject(request.error);
    });
  }

  /** Remove a config key */
  async remove(key: string): Promise<void> {
    if (!this.db) await this.init();

    return new Promise((resolve, reject) => {
      const tx = this.db!.transaction(STORE_NAME, 'readwrite');
      const store = tx.objectStore(STORE_NAME);
      const request = store.delete(key);

      request.onsuccess = () => resolve();
      request.onerror = () => reject(request.error);
    });
  }

  /** Clear all config data */
  async clear(): Promise<void> {
    if (!this.db) await this.init();

    return new Promise((resolve, reject) => {
      const tx = this.db!.transaction(STORE_NAME, 'readwrite');
      const store = tx.objectStore(STORE_NAME);
      const request = store.clear();

      request.onsuccess = () => resolve();
      request.onerror = () => reject(request.error);
    });
  }

  /** Load the full config object */
  async loadConfig(): Promise<Config> {
    const config: Config = { ...DEFAULT_CONFIG };

    const nickname = await this.get<string>('nickname');
    if (nickname) config.nickname = nickname;

    const password = await this.get<string>('password');
    if (password) config.password = password;

    const segments = await this.get<string[]>('segments');
    if (segments) config.segments = segments;

    const port = await this.get<number>('port');
    if (port) config.port = port;

    const autoDiscovery = await this.get<boolean>('autoDiscovery');
    if (autoDiscovery !== null) config.autoDiscovery = autoDiscovery;

    const dataDir = await this.get<string>('dataDir');
    if (dataDir) config.dataDir = dataDir;

    const group = await this.get<string>('group');
    if (group) config.group = group;

    const minimizeBehavior = await this.get<string>('minimizeBehavior');
    if (minimizeBehavior === 'taskbar' || minimizeBehavior === 'tray') config.minimizeBehavior = minimizeBehavior;

    const notificationSound = await this.get<boolean>('notificationSound');
    if (notificationSound !== null) config.notificationSound = notificationSound;

    const directUsers = await this.get<string[]>('directUsers');
    if (directUsers) config.directUsers = directUsers;

    const ipScanRanges = await this.get<string[]>('ipScanRanges');
    if (ipScanRanges) config.ipScanRanges = ipScanRanges;

    return config;
  }

  /** Save the full config object */
  async saveConfig(config: Partial<Config>): Promise<void> {
    const entries = Object.entries(config);
    for (const [key, value] of entries) {
      if (value !== undefined) {
        await this.set(key, value);
      }
    }
  }
}

// Singleton instance
export const configDB = new ConfigDB();
