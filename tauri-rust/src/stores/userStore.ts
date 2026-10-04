import { create } from 'zustand';
import { invoke, listen } from '../services/bridge';
import { toast } from './toastStore';
import type { User } from '../types';

interface UserStore {
  users: User[];
  currentUser: User | null;
  loading: boolean;
  error: string | null;
  revision: number;
  revisions: Map<string, number>;
  loadUsers: () => Promise<void>;
  discoverUsers: () => Promise<void>;
  setCurrentUser: (user: User | null) => void;
  addUser: (user: User) => void;
  initListeners: () => () => void;
}

export const useUserStore = create<UserStore>((set, get) => ({
  users: [], currentUser: null, loading: false, error: null, revision: 0, revisions: new Map(),
  loadUsers: async () => {
    const startedAt = get().revision;
    const result = await invoke<{ users: User[]; count: number }>('user.list');
    if (!Array.isArray(result.users)) throw new Error('用户列表返回格式错误');
    set((state) => {
      const users = new Map(result.users.map((user) => [user.id, user]));
      // Do not lose discoveries/status changes that raced with the snapshot.
      for (const user of state.users) {
        if ((state.revisions.get(user.id) || 0) > startedAt) users.set(user.id, user);
      }
      return {
        users: [...users.values()],
        currentUser: state.currentUser ? users.get(state.currentUser.id) || state.currentUser : null,
      };
    });
  },
  discoverUsers: async () => {
    if (get().loading) return;
    set({ loading: true, error: null });
    try {
      await invoke('user.discover');
      await get().loadUsers();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      set({ error: message });
      toast.error('刷新用户失败：' + message);
    } finally { set({ loading: false }); }
  },
  setCurrentUser: (user) => set({ currentUser: user }),
  addUser: (user) => set((state) => {
    const revision = state.revision + 1;
    const users = new Map(state.users.map((u) => [u.id, u]));
    users.set(user.id, user);
    const revisions = new Map(state.revisions);
    revisions.set(user.id, revision);
    return { users: [...users.values()], revision, revisions, currentUser: state.currentUser?.id === user.id ? user : state.currentUser };
  }),
  initListeners: () => {
    const unlistenUsers = listen('user.discovered', (user: User) => {
      if (user?.id) get().addUser(user);
    });
    const unlistenStatus = listen('user.status_changed', (data: { user?: Partial<User>; status?: User['status'] }) => {
      if (!data?.user?.id || !data.status) return;
      const existing = get().users.find((u) => u.id === data.user!.id);
      if (existing) get().addUser({ ...existing, ...data.user, status: data.status });
    });
    return () => { unlistenUsers(); unlistenStatus(); };
  },
}));
