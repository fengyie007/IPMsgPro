import { invoke } from './bridge';
import { useMessageStore } from '../stores/messageStore';
import type { ImageMetadata } from '../types';

// Captures the original recipient even if the active conversation changes later.
export async function captureAndSend(target: string): Promise<void> {
  const result = await invoke<{ success: boolean; cancelled?: boolean; image?: ImageMetadata }>('screenshot.start');
  if (result.cancelled) return;
  const image = result.image;
  if (!image?.assetId) throw new Error('截图未返回有效图片');
  let accepted = false;
  try {
    accepted = await useMessageStore.getState().sendImage(target, image.assetId);
    if (!accepted) throw new Error(useMessageStore.getState().error || '后端未接受截图发送');
  } finally {
    if (!accepted) await invoke('image.discard', { assetId: image.assetId });
  }
}
