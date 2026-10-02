// ============================================================================
// TypeScript Type Definitions for SpeedIPMsg
// ============================================================================

/** User type */
export interface User {
  id: string;           // userName@hostName
  nickname: string;
  username: string;
  hostname: string;
  group: string;
  ip: string;
  port: number;
  status: 'online' | 'away' | 'offline';
  version: string;
}

/** Message type */
export interface Message {
  id: string;
  from: string;         // sender user id
  to: string;           // receiver user id
  content: string;
  type: 'text' | 'image' | 'file';
  timestamp: number;    // unix timestamp in ms
  status: 'sending' | 'sent' | 'delivered' | 'failed';
  fromUser?: User;      // optional: sender user info
  /** File attachment info (for image/file type) */
  fileInfo?: FileInfoAttachment;
  /** Transfer progress (0-100), only for file/image in transit */
  transferProgress?: number;
}

/** File attachment info attached to a message */
export interface FileInfoAttachment {
  fileName: string;
  fileSize: number;
  fileId?: number;
  /** Local file path (for sent files, the source path; for received files, the save path) */
  filePath?: string;
  /** Transfer ID for tracking progress */
  transferId?: string;
  /** FeiQ inline image ID; these messages are not TCP file transfers. */
  imageId?: string;
}

export interface ImageSendResult {
  success: boolean;
  messageId?: string;
  imageId?: string;
  filePath?: string;
  fileName?: string;
  fileSize?: number;
  error?: string;
}

export interface ImageSendEvent {
  messageId: string;
  target: string;
  filePath: string;
  fileName: string;
  fileSize: number;
  progress: number;
  error?: string;
}

/** Config type. Persisted in IndexedDB; the backend receives the keys listed
 *  in configStore's BACKEND_KEYS via config.set. The listening port is a
 *  process-level setting (--port=N) and is therefore not part of the config. */
export interface Config {
  nickname: string;
  group: string;        // user group name shown to peers
  segments: string[];   // multi-segment broadcast addresses
  dataDir: string;      // chat history & data directory (debug dir follows this)
  minimizeBehavior: 'taskbar' | 'tray';  // minimize to taskbar or system tray
  notificationSound: boolean;  // play notification sound on new messages
  directUsers: string[]; // cross-subnet users to add directly (format: "ip:port")
  ipScanRanges: string[]; // IP ranges for auto-scan (format: "10.8.33.1-254")
}

/** Application version */
export const APP_VERSION = '1.5.0';

/** File transfer type */
export interface FileTransfer {
  id: string;
  filename: string;
  size: number;
  progress: number;     // 0-100
  status: 'pending' | 'transferring' | 'completed' | 'failed';
}

/** Bridge event data types */
export interface UserDiscoveredEvent {
  id: string;
  nickname: string;
  username: string;
  hostname: string;
  group: string;
  ip: string;
  port: number;
  status: string;
  version: string;
}

export interface UserStatusChangedEvent {
  user: UserDiscoveredEvent;
  status: 'online' | 'away' | 'offline';
}

export interface MessageReceivedEvent {
  id: string;
  from: string;
  fromUser?: User;
  content: string;
  type: string;
  timestamp: number;
  command?: number;
}

export interface FileTransferProgressEvent {
  transferId: string;
  filename: string;
  fileSize: number;
  transferred: number;
  status: number;
}

/** File receive request event from backend */
export interface FileReceiveRequestEvent {
  packetNo: number;
  fromUser: string;
  fromUserIp: string;
  fromUserPort: number;
  fileName: string;
  fileSize: number;
  fileId: number;
  transferId?: string;
}

/** Default data directory under user home (matches backend default) */
const getDefaultDataDir = (): string => {
  return (typeof window !== 'undefined' && (window as any).__tauricpp__?.defaultDataDir) || '';
};

/** Default config */
export const DEFAULT_CONFIG: Config = {
  nickname: '',
  group: '',
  segments: [],
  dataDir: '',
  minimizeBehavior: 'tray',
  notificationSound: true,
  directUsers: [],
  ipScanRanges: [],
};
