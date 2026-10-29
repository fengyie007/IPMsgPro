import { invoke, isMockMode } from './bridge';
import { isMyGo } from 'mygo-runtime';
import type { ImageMetadata } from '../types';
export function clipboardImage(files: ArrayLike<File>): File | null {
  const images = Array.from(files).filter((file) => file.type.startsWith('image/'));
  if (images.length > 1) throw new Error('一次只能粘贴一张图片');
  return images[0] || null;
}
export async function importClipboardImage(file: Blob): Promise<ImageMetadata> {
  if (!file.size || file.size > 20 * 1024 * 1024) throw new Error('剪贴板图片为空或超过20 MiB');
  if (!['image/png','image/jpeg','image/bmp','image/x-ms-bmp'].includes(file.type)) throw new Error('剪贴板图片仅支持PNG/JPEG/BMP');
  if (isMockMode || !isMyGo()) throw new Error('剪贴板图片导入需要桌面版');
  // MyGo 绑定为 JSON 参数，raw body 计划在 M2 改为 base64/临时文件传输。
  const bytes = new Uint8Array(await file.arrayBuffer());
  const result = await invoke<{ success: boolean; image: ImageMetadata; error?: string }>('image.import_clipboard', { base64: btoa(String.fromCharCode(...bytes)) });
  if (!result.success || !result.image?.assetId) throw new Error(result.error || '后端未返回有效图片');
  return result.image;
}
