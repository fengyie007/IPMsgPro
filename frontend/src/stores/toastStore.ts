// ============================================================================
// Toast Store - transient, non-blocking notifications (replaces window.alert)
// ============================================================================

import { create } from 'zustand';

export type ToastKind = 'info' | 'success' | 'error';

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

interface ToastStore {
  toasts: Toast[];
  show: (kind: ToastKind, message: string, durationMs?: number) => void;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToastStore = create<ToastStore>((set) => ({
  toasts: [],

  show: (kind, message, durationMs) => {
    const id = nextId++;
    const duration = durationMs ?? (kind === 'error' ? 6000 : 3000);
    set((state) => ({ toasts: [...state.toasts, { id, kind, message }] }));
    window.setTimeout(() => {
      set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) }));
    }, duration);
  },

  dismiss: (id) => {
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) }));
  },
}));

/** Convenience helpers usable outside React components. */
export const toast = {
  info: (message: string) => useToastStore.getState().show('info', message),
  success: (message: string) => useToastStore.getState().show('success', message),
  error: (message: string) => useToastStore.getState().show('error', message),
};
