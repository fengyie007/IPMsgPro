import { invoke, isTauri } from '@tauri-apps/api/core';
import type { ImageMetadata } from '../types';
export function clipboardImage(files: ArrayLike<File>): File | null {
  const images = Array.from(files).filter((file) => file.type.startsWith('image/'));
  if (images.length > 1) throw new Error('一次只能粘贴一张图片');
  return images[0] || null;
}
export async function importClipboardImage(file: Blob): Promise<ImageMetadata> {
  if (!file.size || file.size > 20 * 1024 * 1024) throw new Error('剪贴板图片为空或超过20 MiB');
  if (!['image/png','image/jpeg','image/bmp','image/x-ms-bmp'].includes(file.type)) throw new Error('剪贴板图片仅支持PNG/JPEG/BMP');
  if (!isTauri()) throw new Error('剪贴板图片导入需要桌面版');
  const bytes = new Uint8Array(await file.arrayBuffer());
  const result = await invoke<{ success: boolean; image: ImageMetadata }>('import_clipboard_image', bytes);
  if (!result.success || !result.image?.assetId) throw new Error('后端未返回有效图片');
  return result.image;
}
