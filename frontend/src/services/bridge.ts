// ============================================================================
// Bridge Communication Service
// Type-safe wrapper around TauriCPP's __tauricpp__ API
// ============================================================================

/**
 * Invoke a C++ backend command and return the result.
 * The C++ HandleInvoke returns a JSON string, which window.cpp parses
 * into a JSON object before sending back via PostWebMessageAsJson.
 * So msg.result is already a parsed JSON object — no need to JSON.parse again.
 *
 * Falls back to mock data in development mode (when __tauricpp__ is not available).
 */
export async function invoke<T = any>(command: string, args?: Record<string, any>): Promise<T> {
  console.log(`[Bridge invoke] command=${command}, args=${JSON.stringify(args)}, __tauricpp__=${!!window.__tauricpp__}`);
  if (window.__tauricpp__) {
    // msg.result is already a parsed JSON object (not a string)
    const result = await window.__tauricpp__.invoke(command, args ?? {});
    console.log(`[Bridge invoke] result=${JSON.stringify(result)}`);
    return result as T;
  }

  // Dev mode: return mock data
  console.log(`[Bridge Dev] invoke("${command}",`, args, ')');
  return getMockResponse<T>(command, args);
}

const mockListeners = new Map<string, Set<(data: any) => void>>();
const mockImages = new Map<string, { dataUrl: string; fileSize: number }>();
let mockImageSequence = 0;
function emitMock(event: string, data: unknown) {
  mockListeners.get(event)?.forEach((callback) => callback(data));
}
function rememberMockImage(filePath: string, image: { dataUrl: string; fileSize: number }) {
  if (mockImages.size >= 64) mockImages.delete(mockImages.keys().next().value!);
  mockImages.set(filePath, image);
}

/**
 * Listen to a C++ backend event.
 * Returns an unsubscribe function.
 */
export function listen(event: string, callback: (data: any) => void): () => void {
  if (window.__tauricpp__) {
    return window.__tauricpp__.listen(event, callback);
  }

  console.log(`[Bridge Dev] listen("${event}")`);
  const callbacks = mockListeners.get(event) || new Set<(data: any) => void>();
  callbacks.add(callback);
  mockListeners.set(event, callbacks);
  return () => {
    callbacks.delete(callback);
    if (callbacks.size === 0) mockListeners.delete(event);
  };
}

// ---------- Mock responses for development ----------
// Keep this list aligned with the commands the UI actually invokes (grep for
// invoke(' in frontend/src). Unknown commands fall through to an error so a
// missing mock is visible in the dev console instead of silently "working".

function getMockResponse<T>(command: string, args?: Record<string, any>): T {
  switch (command) {
    case 'user.list':
      return {
        users: [
          { id: 'test1@localhost', nickname: '测试用户1', username: 'test1', hostname: 'localhost', group: '测试组', ip: '127.0.0.1', port: 2425, status: 'online', version: '' },
          { id: 'test2@localhost', nickname: '测试用户2', username: 'test2', hostname: 'localhost', group: '测试组', ip: '127.0.0.1', port: 2425, status: 'away', version: '' },
          { id: 'dev1@devhost', nickname: '研发用户', username: 'dev1', hostname: 'devhost', group: '研发部', ip: '192.168.1.20', port: 2425, status: 'online', version: '' },
          { id: 'guest@guesthost', nickname: '访客', username: 'guest', hostname: 'guesthost', group: '', ip: '192.168.1.30', port: 2425, status: 'online', version: '' },
        ],
        count: 4,
      } as T;

    case 'user.discover':
    case 'config.set':
    case 'config.loaded':
    case 'frontend.error':
    case 'file.reject':
    case 'file.accept':
    case 'history.clear':
    case 'network.scan_range':
    case 'network.scan_cancel':
    case 'window.restore':
    case 'window.set_always_on_top':
    case 'window.set_active_conversation':
    case 'shell_open':
    case 'file.open_folder':
      return { success: true } as T;

    case 'user.local':
      return { success: true, id: 'me@localhost', nickname: '我', username: 'me', hostname: 'localhost', group: '', ip: '127.0.0.1', port: 2425 } as T;

    case 'message.send':
      return { success: true, messageId: `mock_${Date.now()}` } as T;

    case 'file.send':
      return { success: true, transferId: `mock_${Date.now()}`, fileName: args?.filePath?.split(/[\\/]/).pop() ?? 'file' } as T;

    case 'image.send': {
      const source = mockImages.get(args?.filePath);
      if (!source) return { success: false, error: '开发模式下请选择模拟图片' } as T;
      const imageId = (++mockImageSequence).toString(16).padStart(8, '0');
      const messageId = `mock_image_${Date.now()}_${imageId}`;
      const fileName = args?.filePath?.split(/[\\/]/).pop() || 'image.png';
      const filePath = `C:\\Mock\\Sent\\${imageId}_${fileName}`;
      rememberMockImage(filePath, source);
      const data = { messageId, target: args?.target, filePath, fileName, fileSize: source.fileSize, progress: 0 };
      // Exercise the real race: the initial event arrives before invoke resolves.
      emitMock('image.send_progress', data);
      setTimeout(() => emitMock('image.send_progress', { ...data, progress: 50 }), 200);
      setTimeout(() => emitMock('image.send_completed', { ...data, progress: 100 }), 500);
      return { success: true, messageId, imageId, filePath, fileName, fileSize: source.fileSize } as T;
    }

    case 'file.save_temp': {
      const filePath = `C:\\Temp\\${args?.filename ?? 'temp'}`;
      rememberMockImage(filePath, { dataUrl: `data:image/png;base64,${args?.data || ''}`, fileSize: Math.floor((args?.data?.length || 0) * 3 / 4) });
      return { success: true, filePath } as T;
    }

    case 'file.info':
      return { success: true, fileSize: mockImages.get(args?.filePath)?.fileSize || 0, fileName: args?.filePath?.split(/[\\/]/).pop() ?? '' } as T;

    case 'file.read_image': {
      const image = mockImages.get(args?.filePath);
      return (image ? { success: true, ...image } : { success: false, error: 'not available in dev mode' }) as T;
    }

    case 'history.get':
    case 'history.get_recent':
    case 'history.search':
      return { success: true, messages: [], localUserId: 'me@localhost' } as T;

    case 'dialog.pick_folder':
      return { success: true, folder: '' } as T;

    case 'dialog.open': {
      if (args?.filters?.some((filter: { pattern?: string }) => filter.pattern?.includes('*.png'))) {
        const filePath = 'C:\\Mock\\示例图片.png';
        const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180"><rect width="320" height="180" fill="#dcfce7"/><text x="160" y="95" text-anchor="middle" font-size="24" fill="#166534">Image preview</text></svg>';
        rememberMockImage(filePath, { dataUrl: `data:image/svg+xml,${encodeURIComponent(svg)}`, fileSize: svg.length });
        return { success: true, files: [filePath] } as T;
      }
      return { success: true, files: [] } as T;
    }

    case 'dialog.save':
      return { success: false, cancelled: true } as T;

    case 'file.save_data':
      return { success: false, error: 'not available in dev mode' } as T;

    case 'screenshot.capture':
      return { success: false, error: 'not available in dev mode' } as T;

    default:
      return { success: false, error: 'Unknown command' } as T;
  }
}

// ---------- Type declarations for window.__tauricpp__ ----------

declare global {
  interface Window {
    __tauricpp__?: {
      invoke: (cmd: string, args: Record<string, any>) => Promise<any>;
      listen: (event: string, callback: (data: any) => void) => () => void;
      homeDir?: string;
      defaultDataDir?: string;
    };
  }
}
