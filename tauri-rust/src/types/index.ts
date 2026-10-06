export const APP_VERSION = '0.1.0';
export const APP_NAME = '迅秋 Rust 预览版';

export interface User {
  id: string;
  nickname: string;
  username: string;
  hostname: string;
  group: string;
  ip: string;
  port: number;
  status: 'online' | 'away' | 'offline';
  version: string;
}

export interface ImageMetadata {
  assetId: string;
  fileName: string;
  fileSize: number;
  mime: string;
  width: number;
  height: number;
}

export interface ImageReadResult {
  success: boolean;
  url: string;
}

export interface ImageSendEvent {
  messageId: string;
  target: string;
  progress?: number;
  stage?: string;
  error?: string;
  cancelled?: boolean;
}

export interface Message {
  id: string;
  from: string;
  to: string;
  content: string;
  type: 'text' | 'image' | 'file';
  timestamp: number; // UI uses milliseconds; the backend returns seconds.
  status: 'sending' | 'delivered' | 'failed';
  fromUser?: User;
  image?: ImageMetadata;
  imageProgress?: number;
  imageStage?: string;
  imageError?: string;
  fileInfo?: { fileName: string };
}

export interface HistoryRecord {
  id: string;
  fromId: string;
  toId: string;
  content: string;
  type: number;
  timestamp: number;
  status: number;
  image?: ImageMetadata;
}

export interface HistoryResult {
  success: boolean;
  messages: HistoryRecord[];
  localUserId: string;
}

// Rust is the only configuration authority. The unused fields retain the UI
// shape for migration, but are not editable or replayed as active features.
export interface Config {
  nickname: string;
  group: string;
  minimizeBehavior: 'taskbar' | 'tray';
  directUsers: string[];
  dataDir: string;
  notificationSound: boolean;
  segments: string[];
  ipScanRanges: string[];
}

export const DEFAULT_CONFIG: Config = {
  nickname: '', group: '', minimizeBehavior: 'tray', directUsers: [],
  dataDir: '', notificationSound: false, segments: [], ipScanRanges: [],
};

export interface Capabilities {
  images: boolean; // aggregate: both receiving and sending are available
  imageReceive: boolean;
  imageSend: boolean;
  files: boolean;
  screenshot: boolean;
  scan: boolean;
  notificationSound: boolean;
}

export const MVP_CAPABILITIES: Capabilities = {
  images: true, imageReceive: true, imageSend: true, files: false, screenshot: false, scan: false, notificationSound: false,
};

export interface AppInfo {
  success: boolean;
  version: string;
  dataDir: string;
  port: number;
  capabilities: Capabilities;
}

export interface MessageReceivedEvent {
  id: string;
  from: string;
  fromUser?: User;
  content: string;
  type: string;
  timestamp: number;
  command?: number;
  image?: ImageMetadata;
}
