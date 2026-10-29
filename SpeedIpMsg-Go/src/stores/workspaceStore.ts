import { create } from 'zustand';
import type { Message, User } from '../types';
import type { DraftPart } from '../utils/chatEditor';
import { useUserStore } from './userStore';

export type ViewMode = 'chat' | 'contacts' | 'settings';
export type SettingsCategory = 'profile' | 'notifications' | 'network' | 'storage' | 'general' | 'about';
type Destination = { view: ViewMode; user?: User };
export interface ChatView {
  scrollTop: number; nearBottom: boolean; searchOpen: boolean; query: string; results: Message[] | null;
}
interface WorkspaceState {
  view: ViewMode;
  returnView: 'chat' | 'contacts';
  contactId: string;
  searches: Record<'chat' | 'contacts', string>;
  collapsedGroups: Set<string>;
  settingsCategory: SettingsCategory;
  settingsDirty: boolean;
  settingsBusy: boolean;
  pendingNavigation: Destination | null;
  drafts: Map<string, DraftPart[]>;
  sending: Set<string>;
  chatViews: Map<string, ChatView>;
  scrollPositions: Map<string, number>;
  navigate: (view: ViewMode) => void;
  openChat: (user: User) => void;
  requestNavigation: (destination: Destination) => void;
  confirmNavigation: () => void;
  cancelNavigation: () => void;
  setSettingsDirty: (dirty: boolean) => void;
  setSettingsBusy: (busy: boolean) => void;
  saveDraft: (id: string, parts: DraftPart[]) => void;
  clearSentDraft: (id: string, expected: DraftPart[] | undefined) => void;
  beginSend: (id: string) => boolean;
  endSend: (id: string) => void;
  saveChatView: (id: string, view: ChatView) => void;
  saveScroll: (key: string, top: number) => void;
}

// Memory only: leaving a page never writes unfinished messages or settings to disk.
export const useWorkspaceStore = create<WorkspaceState>((set, get) => {
  const apply = ({ view, user }: Destination) => {
    if (user) {
      const users = useUserStore.getState();
      users.setCurrentUser(users.users.find((item) => item.id === user.id) || user);
    }
    set((state) => ({
      view, pendingNavigation: null, settingsDirty: false,
      returnView: view === 'settings' && state.view !== 'settings' ? state.view : state.returnView,
    }));
  };
  return {
    view: 'chat', returnView: 'chat', contactId: '', searches: { chat: '', contacts: '' },
    collapsedGroups: new Set(), settingsCategory: 'profile', settingsDirty: false, settingsBusy: false,
    pendingNavigation: null, drafts: new Map(), sending: new Set(), chatViews: new Map(), scrollPositions: new Map(),
    navigate: (view) => get().requestNavigation({ view }),
    openChat: (user) => get().requestNavigation({ view: 'chat', user }),
    requestNavigation: (destination) => {
      const state = get();
      if (destination.view === state.view && !destination.user) return;
      if (state.view === 'settings' && (state.settingsDirty || state.settingsBusy)) {
        set({ pendingNavigation: destination });
      } else apply(destination);
    },
    confirmNavigation: () => {
      if (!get().settingsBusy && get().pendingNavigation) apply(get().pendingNavigation!);
    },
    cancelNavigation: () => set({ pendingNavigation: null }),
    setSettingsDirty: (settingsDirty) => set({ settingsDirty }),
    setSettingsBusy: (settingsBusy) => {
      set({ settingsBusy });
      if (!settingsBusy && !get().settingsDirty && get().pendingNavigation) apply(get().pendingNavigation!);
    },
    saveDraft: (id, parts) => set((state) => {
      if (JSON.stringify(state.drafts.get(id) || []) === JSON.stringify(parts)) return state;
      const drafts = new Map(state.drafts);
      if (parts.length) drafts.set(id, parts); else drafts.delete(id);
      return { drafts };
    }),
    clearSentDraft: (id, expected) => set((state) => {
      if (state.drafts.get(id) !== expected) return state;
      const drafts = new Map(state.drafts); drafts.delete(id); return { drafts };
    }),
    beginSend: (id) => {
      if (get().sending.has(id)) return false;
      set((state) => ({ sending: new Set(state.sending).add(id) })); return true;
    },
    endSend: (id) => set((state) => { const sending = new Set(state.sending); sending.delete(id); return { sending }; }),
    saveChatView: (id, view) => set((state) => ({ chatViews: new Map(state.chatViews).set(id, view) })),
    saveScroll: (key, top) => set((state) => state.scrollPositions.get(key) === top ? state : ({ scrollPositions: new Map(state.scrollPositions).set(key, top) })),
  };
});
