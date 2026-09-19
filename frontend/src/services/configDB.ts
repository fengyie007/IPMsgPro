// ============================================================================
// IndexedDB Configuration Storage Service
// Manages user preferences and settings in the browser's IndexedDB
// ============================================================================

import { Config, DEFAULT_CONFIG } from '../types';

const DB_NAME = 'ipmsg-config';
const DB_VERSION = 1;
const STORE_NAME = 'config';

/** True when `value` has the same runtime shape as `reference` (the default). */
function isCompatible(value: unknown, reference: unknown): boolean {
  if (Array.isArray(reference)) {
    return Array.isArray(value) && value.every((v) => typeof v === 'string');
  }
  return typeof value === typeof reference;
}

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

  /**
   * Load the full config object. Every key of DEFAULT_CONFIG is read from the
   * store; a stored value replaces the default only when it has the expected
   * type, so a corrupted or legacy entry cannot poison the config.
   */
  async loadConfig(): Promise<Config> {
    const config: Config = { ...DEFAULT_CONFIG };
    for (const key of Object.keys(DEFAULT_CONFIG) as (keyof Config)[]) {
      const stored = await this.get<unknown>(key);
      if (stored === null || stored === undefined) continue;
      if (isCompatible(stored, DEFAULT_CONFIG[key])) {
        (config as any)[key] = stored;
      } else {
        console.warn(`[ConfigDB] Ignoring stored "${key}" with unexpected type:`, stored);
      }
    }
    if (config.minimizeBehavior !== 'taskbar' && config.minimizeBehavior !== 'tray') {
      config.minimizeBehavior = DEFAULT_CONFIG.minimizeBehavior;
    }
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
