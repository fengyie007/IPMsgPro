import { create } from 'zustand';
import { invoke } from '../services/bridge';
import { useMessageStore } from './messageStore';
import { toast } from './toastStore';
import type { FileSelection } from '../types';
interface SelectionState {
  target: string; files: FileSelection[]; busy: boolean;
  picked: (target: string, files: FileSelection[]) => void;
  select: (target: string) => Promise<void>;
  cancel: () => Promise<void>;
  send: () => Promise<void>;
}
async function discard(files: FileSelection[]) {
  for (const file of files) await invoke('file.discard', { selectionId: file.selectionId });
}
export const useFileSelectionStore = create<SelectionState>((set, get) => ({
  target: '', files: [], busy: false,
  picked: (target, files) => {
    if (!target || get().busy || get().files.length) {
      void discard(files).catch((e) => toast.error('文件预览清理失败：' + String(e)));
      toast.info('请先完成当前文件预览'); return;
    }
    set({ target, files });
  },
  select: async (target) => {
    if (!target || get().busy || get().files.length) return;
    set({ busy: true });
    try {
      const result = await invoke<{ cancelled?: boolean; files?: FileSelection[] }>('file.select');
      if (!result.cancelled && result.files?.length) set({ target, files: result.files });
    } catch (error) { toast.error('选择文件失败：' + String(error)); }
    finally { set({ busy: false }); }
  },
  cancel: async () => {
    if (get().busy) return;
    const files = get().files; set({ files: [], target: '' });
    try { await discard(files); } catch (error) { toast.error('文件预览清理失败：' + String(error)); }
  },
  send: async () => {
    if (get().busy || !get().files.length) return;
    const { target, files } = get(); set({ busy: true });
    try {
      for (const file of files) {
        if (!await useMessageStore.getState().sendFile(target, file.selectionId)) toast.error(`发送 ${file.fileName} 失败：${useMessageStore.getState().error || '未知错误'}`);
      }
    } finally {
      try { await discard(files); } catch (error) { console.error('File selection cleanup failed', error); }
      set({ files: [], target: '', busy: false });
    }
  },
}));
