import React, { useState } from 'react';
import { FiFile } from 'react-icons/fi';
import type { Message } from '../types';
import { invoke } from '../services/bridge';
import { toast } from '../stores/toastStore';
import { formatFileSize } from '../utils/format';
export default function FileBubble({ message }: { message: Message }) {
  const [busy, setBusy] = useState(false), file = message.file;
  if (!file) return <span>[文件信息缺失，无法接收]</span>;
  const action = async (command: string) => {
    if (busy) return; setBusy(true);
    try { const result = await invoke(command, { messageId: message.id }); if (result.cancelled === false) toast.info('传输已结束或正在保存最终状态'); }
    catch (error) { toast.error('文件操作失败：' + String(error)); }
    finally { setBusy(false); }
  };
  const received = file.state === 'transferring' ? Math.max(file.transferred, file.inFlightBytes || 0) : file.transferred;
  const percent = file.state === 'completed' ? 100 : file.fileSize ? Math.min(99, Math.floor(received * 100 / file.fileSize)) : 0;
  const active = file.state === 'transferring' || file.state === 'finalizing';
  const labels = { offered: file.incoming ? '待接收' : '等待对方接收', transferring: '传输中', finalizing: '正在保存', completed: file.incoming ? '已保存' : '已发送', cancelled: '已取消', rejected: '已拒绝', failed: '传输失败', paused: '已暂停，可继续接收' };
  return <div className="min-w-[220px] max-w-xs">
    <div className="flex gap-2 items-center"><FiFile size={26} className="shrink-0 text-gray-500"/><div className="min-w-0"><p className="break-all font-medium">{file.fileName}</p><p className="text-xs text-gray-500">{formatFileSize(file.fileSize)} · {labels[file.state]}</p></div></div>
    {active && <><div className="h-1.5 bg-gray-200 rounded mt-3 overflow-hidden"><div className="h-full bg-primary-500" style={{ width: `${percent}%` }} /></div><p className="text-xs text-gray-500 mt-1">{formatFileSize(received)} / {formatFileSize(file.fileSize)} · {percent}%</p></>}
    {file.error && <p className="text-xs text-red-600 mt-2 break-all">{file.error}</p>}
    <div className="flex gap-4 text-xs mt-2 text-primary-700">
      {file.incoming && !file.isDirectory && file.canResume && file.state === 'transferring' && <button disabled={busy} onClick={() => void action('file.pause')}>暂停</button>}
      {file.state === 'paused' && <><button disabled={busy || !file.canResume} onClick={() => void action('file.resume')}>继续接收</button><button disabled={busy} onClick={() => void action('file.cancel')}>取消并清理</button></>}
      {file.state === 'offered' && file.incoming && <><button disabled={busy} onClick={() => void action('file.accept')}>接收</button><button disabled={busy} onClick={() => void action('file.reject')}>拒绝</button></>}
      {(active || (file.state === 'offered' && !file.incoming)) && <button disabled={busy || file.state === 'finalizing'} onClick={() => void action('file.cancel')}>取消</button>}
      {file.state === 'completed' && file.hasLocalFile && <button disabled={busy} onClick={() => void action('file.open_folder')}>打开文件夹</button>}
    </div>
  </div>;
}
