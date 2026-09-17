// ============================================================================
// User Store - Zustand state management for users
// ============================================================================

import { create } from 'zustand';
import { User } from '../types';
import { invoke, listen } from '../services/bridge';

interface UserStore {
  users: User[];
  currentUser: User | null;
  loading: boolean;
  error: string | null;

  /** Discover users on the network (broadcast BR_ENTRY) */
  discoverUsers: () => Promise<void>;

  /** Load the current user list from the backend */
  loadUsers: () => Promise<void>;

  /** Set the currently selected user for chat */
  setCurrentUser: (user: User | null) => void;

  /** Update a user's status */
  updateUserStatus: (userId: string, status: 'online' | 'away' | 'offline') => void;

  /** Add a discovered user */
  addUser: (user: User) => void;

  /** Initialize event listeners */
  initListeners: () => () => void;
}

export const useUserStore = create<UserStore>((set, get) => ({
  users: [],
  currentUser: null,
  loading: false,
  error: null,

  discoverUsers: async () => {
    set({ loading: true, error: null });
    try {
      await invoke('user.discover');
      // After broadcasting, reload the user list
      await get().loadUsers();
    } catch (err: any) {
      set({ error: err.message });
    } finally {
      set({ loading: false });
    }
  },

  loadUsers: async () => {
    try {
      console.log('[UserStore] Loading users from backend...');
      const result = await invoke<{ users: User[]; count: number }>('user.list');
      console.log('[UserStore] Users loaded:', result.count, 'users');
      result.users?.forEach(u => console.log(`  - ${u.id} (${u.nickname} @ ${u.ip}:${u.port})`));
      set({ users: result.users || [] });
    } catch (err: any) {
      console.error('[UserStore] Failed to load users:', err);
      set({ error: err.message });
    }
  },

  setCurrentUser: (user) => {
    set({ currentUser: user });
  },

  updateUserStatus: (userId, status) => {
    set((state) => ({
      users: state.users.map((u) =>
        u.id === userId ? { ...u, status } : u
      ),
    }));
  },

  addUser: (user) => {
    set((state) => {
      const exists = state.users.some((u) => u.id === user.id);
      if (exists) {
        return {
          users: state.users.map((u) => (u.id === user.id ? user : u)),
        };
      }
      return { users: [...state.users, user] };
    });
  },

  initListeners: () => {
    console.log('[UserStore] Registering event listeners...');
    const unsub1 = listen('user.discovered', (data: any) => {
      const user: User = {
        id: data.id,
        nickname: data.nickname,
        username: data.username,
        hostname: data.hostname,
        group: data.group || '',
        ip: data.ip,
        port: data.port,
        status: (data.status as any) || 'online',
        version: data.version || '',
      };
      console.log('[UserStore] User discovered:', user.id, '(', user.nickname, '@', user.ip, ')');
      get().addUser(user);
    });

    const unsub2 = listen('user.status_changed', (data: any) => {
      if (data.user) {
        console.log('[UserStore] User status changed:', data.user.id, '->', data.status);
        get().updateUserStatus(data.user.id, data.status);
      }
    });

    // Return cleanup function
    return () => {
      unsub1();
      unsub2();
    };
  },
}));
