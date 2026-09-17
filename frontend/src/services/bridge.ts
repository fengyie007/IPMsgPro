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

/**
 * Listen to a C++ backend event.
 * Returns an unsubscribe function.
 */
export function listen(event: string, callback: (data: any) => void): () => void {
  if (window.__tauricpp__) {
    return window.__tauricpp__.listen(event, callback);
  }

  // Dev mode: no-op
  console.log(`[Bridge Dev] listen("${event}")`);
  return () => {};
}

// ---------- Mock responses for development ----------

function getMockResponse<T>(command: string, args?: Record<string, any>): T {
  switch (command) {
    case 'user.list':
      return {
        users: [
          { id: 'test1@localhost', nickname: '测试用户1', username: 'test1', hostname: 'localhost', group: '测试组', ip: '127.0.0.1', port: 2425, status: 'online', version: '' },
          { id: 'test2@localhost', nickname: '测试用户2', username: 'test2', hostname: 'localhost', group: '测试组', ip: '127.0.0.1', port: 2425, status: 'away', version: '' },
        ],
        count: 2,
      } as T;

    case 'user.discover':
      return { success: true } as T;

    case 'user.status':
      return { success: true, status: args?.status ?? 'online' } as T;

    case 'message.send':
      return { success: true } as T;

    case 'message.send_image':
      return { success: true } as T;

    case 'file.send':
      return { success: true } as T;

    case 'file.recv':
      return { success: true } as T;

    case 'file.accept':
      return { success: true } as T;

    case 'file.reject':
      return { success: true } as T;

    case 'history.get':
      return { success: true, messages: [] } as T;

    case 'history.search':
      return { success: true, messages: [] } as T;

    case 'history.clear':
      return { success: true } as T;

    case 'network.scan':
      return { success: true } as T;

    case 'config.set':
      return { success: true } as T;

    case 'dialog.pick_folder':
      return { success: true, folder: '' } as T;

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
