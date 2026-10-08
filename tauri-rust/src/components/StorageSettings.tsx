import React, { useEffect, useState } from 'react';
import { invoke } from '../services/bridge';
import { toast } from '../stores/toastStore';
import ConfirmDialog from './ConfirmDialog';
interface Info { active: string; pending: string | null; defaultDirectory: string; isDefault: boolean; error: string | null }
interface Choice { selectionId: string; source: string; target: string; cancelled?: boolean }
export default function StorageSettings({ disabled, onBusyChange }: { disabled: boolean; onBusyChange?: (busy: boolean) => void }) {
  const [info, setInfo] = useState<Info | null>(null), [choice, setChoice] = useState<Choice | null>(null), [busy, setBusy] = useState(false);
  useEffect(() => { onBusyChange?.(busy || !!choice); }, [busy, choice, onBusyChange]);
  useEffect(() => () => onBusyChange?.(false), [onBusyChange]);
  useEffect(() => { let stopped = false; void invoke<Info>('storage.info').then((value) => { if (!stopped) setInfo(value); }).catch((e) => toast.error('读取存储目录失败：' + String(e))); return () => { stopped = true; }; }, []);
  const choose = async (reset = false) => {
    if (busy) return; setBusy(true);
    try { const result = await invoke<Choice>(reset ? 'storage.default' : 'storage.select'); if (!result.cancelled) setChoice(result); }
    catch (error) { toast.error('选择目录失败：' + String(error)); } finally { setBusy(false); }
  };
  const apply = async () => {
    if (!choice || busy) return; setBusy(true);
    try { setInfo(await invoke<Info>('storage.apply', { selectionId: choice.selectionId })); setChoice(null); toast.success('目录选择已保存，下次启动复制数据并切换'); }
    catch (error) { toast.error('保存目录失败：' + String(error)); } finally { setBusy(false); }
  };
  const cancel = async () => { if (busy) return; setBusy(true); try { setInfo(await invoke<Info>('storage.cancel')); } catch (e) { toast.error(String(e)); } finally { setBusy(false); } };
  return <section className="space-y-3">
    <h3 className="text-sm font-semibold text-gray-700">数据存储目录</h3>
    <input readOnly aria-label="当前存储目录" value={info?.active || ''} className="input-field text-gray-500 bg-gray-50" />
    <div className="flex gap-4 text-sm text-primary-600"><button disabled={disabled || busy || !info} onClick={() => void choose()}>选择目录</button><button disabled={disabled || busy || !info || info.isDefault} onClick={() => void choose(true)}>恢复默认位置</button></div>
    <p className="text-xs text-gray-400">确认后已保存目录选择，重启生效。将复制当前 Rust 配置、聊天记录及图片，原目录保留备份；大量数据可能需要等待。普通接收文件仍在 Downloads，不读取旧 C++ 版数据。</p>
    {info?.pending && <div className="text-xs bg-primary-50 p-3 rounded break-all">下次启动切换到：{info.pending}<button disabled={busy} onClick={() => void cancel()} className="block mt-2 text-primary-700">取消待执行的切换</button></div>}
    {info?.error && <p className="text-xs text-red-500">上次切换失败，仍使用原目录：{info.error}</p>}
    {choice && <ConfirmDialog title="安排数据目录切换" message={`当前目录：${choice.source}\n目标目录：${choice.target}\n\n下次启动复制当前 Rust 数据后切换，原目录保留备份。是否确认？`} onConfirm={() => void apply()} onCancel={() => { if (!busy) setChoice(null); }} />}
  </section>;
}
