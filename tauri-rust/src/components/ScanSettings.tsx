import React, { useEffect, useState } from 'react';
import { FiPlus, FiTrash2 } from 'react-icons/fi';
import type { Config } from '../types';
import { normalizeScanRange, validateScanOptions } from '../utils/netValidation';
import { scanActive, useScanStore } from '../stores/scanStore';

export default function ScanSettings({ draft, setDraft, disabled, input, onInputChange: setInput }: {
  draft: Config; setDraft: React.Dispatch<React.SetStateAction<Config>>; disabled: boolean;
  input: string; onInputChange: (value: string) => void;
}) {
  const { status, busy, error, refresh, start, cancel } = useScanStore();
  const [inputError, setInputError] = useState('');
  const active = scanActive(status.state), locked = disabled || busy || active;
  const validation = validateScanOptions(draft.ipScanRanges, draft.scanPort, draft.scanDelayMs);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [active, refresh]);
  const add = () => {
    if (locked) return;
    const parsed = normalizeScanRange(input);
    if ('error' in parsed) { setInputError(parsed.error); return; }
    const ranges = draft.ipScanRanges.includes(parsed.value) ? draft.ipScanRanges : [...draft.ipScanRanges, parsed.value];
    const checked = validateScanOptions(ranges, draft.scanPort, draft.scanDelayMs);
    if ('error' in checked) { setInputError(checked.error); return; }
    setDraft({ ...draft, ipScanRanges: ranges }); setInput(''); setInputError('');
  };
  const labels = { idle: '尚未扫描', running: '正在探测', waiting: '正在等候回复', cancelling: '正在取消', completed: '扫描完成', cancelled: '已取消扫描', failed: '扫描失败' };
  const percent = status.total ? Math.floor(status.current * 100 / status.total) : 0;
  return <section className="space-y-3">
    <h3 className="text-sm font-semibold text-gray-700">IP 范围扫描（跨网段发现）</h3>
    <p className="text-xs text-gray-500">逐个探测地址，适用于广播无法到达的网段。支持完整范围、末段简写、CIDR 和单个 IPv4；重叠地址仅探测一次。</p>
    <div className="flex gap-2">
      <input aria-label="扫描范围" disabled={locked} className="input-field flex-1" value={input} placeholder="例如 10.8.33.1-254 或 10.8.33.0/24"
        onChange={(event) => { setInput(event.target.value); setInputError(''); }} onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); add(); } }} />
      <button title="添加扫描范围" disabled={locked} onClick={add} className="px-3 rounded border border-gray-200 disabled:opacity-50"><FiPlus /></button>
    </div>
    {inputError && <p role="alert" className="text-xs text-red-500">{inputError}</p>}
    <div className="space-y-1">
      {draft.ipScanRanges.map((range, index) => <div key={`${index}:${range}`} className="flex items-center justify-between gap-2 py-1 px-2 rounded bg-gray-50 text-sm text-gray-600">
        <span className="break-all">{range}</span><button title={`移除 ${range}`} disabled={locked} className="p-1 text-gray-400 hover:text-red-500 disabled:opacity-40"
          onClick={() => setDraft({ ...draft, ipScanRanges: draft.ipScanRanges.filter((_, i) => i !== index) })}><FiTrash2 size={14} /></button>
      </div>)}
    </div>
    <div className="flex gap-4 flex-wrap">
      <label className="text-xs text-gray-500">目标端口<input aria-label="扫描目标端口" type="number" min={1} max={65535} disabled={locked} value={draft.scanPort || ''}
        onChange={(event) => setDraft({ ...draft, scanPort: Number(event.target.value) })} className="input-field mt-1 w-28 block" /></label>
      <label className="text-xs text-gray-500">探测间隔（毫秒）<input aria-label="扫描间隔" type="number" min={10} max={1000} disabled={locked} value={draft.scanDelayMs || ''}
        onChange={(event) => setDraft({ ...draft, scanDelayMs: Number(event.target.value) })} className="input-field mt-1 w-28 block" /></label>
    </div>
    <p className="text-xs text-gray-400">飞秋默认端口为2425；目标端口不改变本程序监听端口。CIDR（/30及以下）会排除网络地址和广播地址。</p>
    {'error' in validation ? <p role="alert" className="text-xs text-red-500">{validation.error}</p> : <p className="text-xs text-gray-400">本次范围去重后共 {validation.value.total} 个地址，最多65536个。</p>}
    <label className="flex items-center gap-2 text-sm text-gray-600"><input type="checkbox" disabled={disabled} checked={draft.scanOnStartup}
      onChange={(event) => setDraft({ ...draft, scanOnStartup: event.target.checked })} />启动时自动扫描已保存范围</label>
    <div className="flex items-center gap-3">
      <button disabled={disabled || busy || active || 'error' in validation || !draft.ipScanRanges.length} onClick={() => void start({ ranges: draft.ipScanRanges, port: draft.scanPort, delayMs: draft.scanDelayMs })}
        className="px-4 py-1.5 rounded bg-primary-500 text-white text-sm disabled:opacity-40">开始扫描</button>
      {active && <button disabled={busy || status.state === 'cancelling'} onClick={() => void cancel()} className="text-sm text-gray-600 disabled:opacity-40">取消扫描</button>}
    </div>
    <p className="text-xs text-gray-400">“开始扫描”使用当前输入；“保存设置”用于下次启动，不会重复启动当前扫描。</p>
    {status.scanId > 0 && <div className="bg-gray-50 rounded p-3 space-y-2 text-xs text-gray-600" aria-live="polite">
      <div className="flex justify-between"><span>{labels[status.state]}</span><span>{status.current} / {status.total} · {percent}%</span></div>
      <div className="h-1.5 bg-gray-200 rounded overflow-hidden"><div className="h-full bg-primary-500" style={{ width: `${percent}%` }} /></div>
      <p>响应地址 {status.found} · 发送失败 {status.failedSends} · 跳过本机 {status.skipped}</p>
      <p className="text-gray-400">当前任务目标端口 {status.port} · 间隔 {status.delayMs} ms</p>
      <p className="text-gray-400 break-all">当前任务范围：{status.ranges.join('，')}</p>
      {status.error && <p className="text-red-500">{status.error}</p>}
    </div>}
    {error && <p role="alert" className="text-xs text-red-500">{error}</p>}
  </section>;
}
