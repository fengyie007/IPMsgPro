import { invoke } from './bridge';

let previewing = false;
// Explicit user action only. A backend error must not become a successful preview.
export async function previewNotificationSound(): Promise<boolean> {
  if (previewing) return false;
  previewing = true;
  try {
    const result = await invoke<{ success: boolean; durationMs: number }>('notification.test_sound');
    if (!result.success || !Number.isFinite(result.durationMs) || result.durationMs <= 0 || result.durationMs > 20000) {
      throw new Error('后端未确认提示音播放');
    }
    return true;
  } finally { previewing = false; }
}
