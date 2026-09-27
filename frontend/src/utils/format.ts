// ============================================================================
// Formatting helpers shared by the chat components
// ============================================================================

import type { Message } from '../types';
import { EMOJI_TOKEN_RE } from '../emojiData';

/** "HH:MM" for today, "MM/DD HH:MM" otherwise (zh-CN locale). */
export function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();

  if (isToday) {
    return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  }
  return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' }) + ' ' +
    date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

/** Conversation-list time: "HH:MM" today, "昨天", "MM/DD" this year, "YYYY/MM/DD" before. */
export function formatListTime(timestamp: number): string {
  const d = new Date(timestamp);
  const today = new Date();
  if (isSameDay(timestamp, today.getTime())) {
    return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  }
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (isSameDay(timestamp, yesterday.getTime())) return '昨天';
  return d.toLocaleDateString('zh-CN', d.getFullYear() === today.getFullYear()
    ? { month: '2-digit', day: '2-digit' }
    : { year: 'numeric', month: '2-digit', day: '2-digit' });
}

/** One-line conversation-list preview: placeholders instead of emoji XML, paths and data URLs. */
export function formatPreview(msg: Message): string {
  if (msg.type === 'image' || msg.content.startsWith('data:image/')) return '[图片]';
  if (msg.type === 'file') {
    return `[文件] ${msg.fileInfo?.fileName || msg.content.split(/[\\/]/).pop() || ''}`;
  }
  return msg.content.replace(EMOJI_TOKEN_RE, '[表情]').replace(/\s*\n\s*/g, ' ');
}

/** "1.5 MB" style, 1024-based. */
export function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1);
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
}

export function isSameDay(a: number, b: number): boolean {
  const da = new Date(a), db = new Date(b);
  return da.getFullYear() === db.getFullYear() && da.getMonth() === db.getMonth() && da.getDate() === db.getDate();
}

/** "今天" / "昨天" / "9月18日 周四" / "2025年12月1日" for the date separators. */
export function formatDateSeparator(timestamp: number): string {
  const d = new Date(timestamp);
  const today = new Date();
  if (isSameDay(timestamp, today.getTime())) return '今天';
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (isSameDay(timestamp, yesterday.getTime())) return '昨天';
  const sameYear = d.getFullYear() === today.getFullYear();
  return d.toLocaleDateString('zh-CN', sameYear
    ? { month: 'long', day: 'numeric', weekday: 'short' }
    : { year: 'numeric', month: 'long', day: 'numeric' });
}
