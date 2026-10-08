import React, { useEffect, useRef, useState } from 'react';
import { FiX, FiPlus, FiTrash2, FiRotateCcw } from 'react-icons/fi';
import { useConfigStore } from '../stores/configStore';
import { toast } from '../stores/toastStore';
import { APP_NAME, APP_VERSION, type Config } from '../types';
import { normalizeDirectUser, validateScanOptions } from '../utils/netValidation';
import ConfirmDialog from './ConfirmDialog';
import ScanSettings from './ScanSettings';
import NotificationSettings from './NotificationSettings';

export default function Settings({ onClose }: { onClose: () => void }) {
  const config = useConfigStore((s) => s.config);
  const info = useConfigStore((s) => s.info);
  const [draft, setDraft] = useState<Config>({ ...config, directUsers: [...config.directUsers] });
  const [newUser, setNewUser] = useState('');
  const [inputError, setInputError] = useState('');
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [resetOpen, setResetOpen] = useState(false);
  useEffect(() => setDraft({ ...config, directUsers: [...config.directUsers] }), [config]);

  const addUser = () => {
    if (busyRef.current) return;
    const parsed = normalizeDirectUser(newUser);
    if ('error' in parsed) { setInputError(parsed.error); return; }
    if (!draft.directUsers.includes(parsed.value)) setDraft({ ...draft, directUsers: [...draft.directUsers, parsed.value] });
    setNewUser(''); setInputError('');
  };
  const save = async () => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true);
    try {
      const scan = validateScanOptions(draft.ipScanRanges, draft.scanPort, draft.scanDelayMs);
      if ('error' in scan) throw new Error(scan.error);
      await useConfigStore.getState().saveConfig({
        nickname: draft.nickname, group: draft.group,
        minimizeBehavior: draft.minimizeBehavior, directUsers: draft.directUsers,
        ipScanRanges: scan.value.ranges, scanPort: draft.scanPort, scanDelayMs: draft.scanDelayMs, scanOnStartup: draft.scanOnStartup,
        notificationSound: draft.notificationSound,
      });
      toast.success('设置已保存并应用');
      onClose();
    } catch (error) { toast.error('保存设置失败：' + String(error)); }
    finally { busyRef.current = false; setBusy(false); }
  };
  const reset = async () => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true);
    try {
      await useConfigStore.getState().resetConfig();
      setResetOpen(false);
      toast.success('已恢复默认设置');
    } catch (error) { toast.error('恢复设置失败：' + String(error)); }
    finally { busyRef.current = false; setBusy(false); }
  };

  return (
    <div className="flex-1 min-w-0 flex flex-col bg-white">
      <div className="h-16 px-6 flex items-center justify-between border-b border-gray-200 shrink-0">
        <h2 className="font-semibold text-gray-800">设置</h2>
        <button onClick={onClose} disabled={busy} title="关闭设置" className="p-2 rounded hover:bg-gray-100 disabled:opacity-50"><FiX size={20} /></button>
      </div>
      <div className="flex-1 overflow-y-auto p-6 space-y-7">
        <div className="rounded border border-primary-100 bg-primary-50 px-4 py-3 text-sm text-gray-600">
          Rust 预览版：文本、图片、普通文件、Windows 截图与提示音、IP范围扫描、历史、通讯录和托盘已接入。文件夹暂不支持。
        </div>
        <Section title="个人信息">
          <Field label="昵称"><input disabled={busy} className="input-field" value={draft.nickname} maxLength={128} onChange={(e) => setDraft({ ...draft, nickname: e.target.value })} placeholder="留空使用默认昵称" /></Field>
          <Field label="组名"><input disabled={busy} className="input-field" value={draft.group} maxLength={128} onChange={(e) => setDraft({ ...draft, group: e.target.value })} placeholder="通讯录将按此组名展示" /></Field>
        </Section>
        <Section title="直接添加用户">
          <p className="text-xs text-gray-400 mb-3">配置目标 IPv4:端口，保存后发送实际发现请求。原 C++ 版2426或飞秋2425可在此添加。</p>
          <div className="flex gap-2">
            <input disabled={busy} className="input-field flex-1" value={newUser} onChange={(e) => { setNewUser(e.target.value); setInputError(''); }}
              placeholder="例如 192.168.2.88:2425" onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addUser(); } }} />
            <button disabled={busy} onClick={addUser} className="px-3 rounded border border-gray-200 hover:bg-gray-50 disabled:opacity-50" title="添加直接用户"><FiPlus /></button>
          </div>
          {inputError && <p className="text-xs text-red-500 mt-1">{inputError}</p>}
          <div className="mt-3 space-y-1">
            {draft.directUsers.map((address) => <div key={address} className="flex items-center justify-between py-1 px-2 rounded bg-gray-50 text-sm text-gray-600">
              <span>{address}</span>
              <button disabled={busy} title={`移除 ${address}`} className="p-1 text-gray-400 hover:text-red-500" onClick={() => setDraft({ ...draft, directUsers: draft.directUsers.filter((v) => v !== address) })}><FiTrash2 size={14} /></button>
            </div>)}
          </div>
        </Section>
        {info?.capabilities.scan && <ScanSettings draft={draft} setDraft={setDraft} disabled={busy} />}
        <NotificationSettings enabled={draft.notificationSound} supported={info?.capabilities.notificationSound ?? false} disabled={busy}
          onChange={(notificationSound) => setDraft({ ...draft, notificationSound })} />
        <Section title="窗口行为">
          <Field label="点击窗口关闭按钮时">
            <div className="flex flex-wrap gap-3">
              {(['tray', 'taskbar'] as const).map((behavior) => <button key={behavior} disabled={busy}
                onClick={() => setDraft({ ...draft, minimizeBehavior: behavior })}
                className={`px-3 py-2 text-sm rounded border ${draft.minimizeBehavior === behavior ? 'border-primary-500 bg-primary-50 text-primary-700' : 'border-gray-200 text-gray-500'}`}>
                {behavior === 'tray' ? '隐藏到系统托盘' : '退出程序'}
              </button>)}
            </div>
            <p className="text-xs text-gray-400 mt-2">隐藏到托盘时仍可收发消息；托盘菜单“退出”才结束程序。标题栏最小化按钮仍最小化到任务栏。</p>
          </Field>
        </Section>
        <Section title="独立数据目录">
          <input readOnly className="input-field text-gray-500 bg-gray-50" value={info?.dataDir || ''} aria-label="Rust 版独立数据目录" />
          <p className="text-xs text-gray-400 mt-2">Rust 版使用独立配置、SQLite与日志，不迁移或覆盖原版数据。本阶段目录不可修改。</p>
          <p className="text-xs text-gray-500 mt-2">当前监听端口：{info?.port ?? '—'}（通过 --port 指定）</p>
        </Section>
        <Section title="关于"><p className="text-sm text-gray-500">{APP_NAME} v{APP_VERSION} · Rust + Tauri 2</p></Section>
      </div>
      <div className="border-t px-6 py-4 flex items-center justify-between gap-3 shrink-0">
        <button disabled={busy} onClick={() => setResetOpen(true)} className="flex items-center gap-1 text-sm text-gray-500 hover:text-gray-700 disabled:opacity-50"><FiRotateCcw size={14} />恢复默认</button>
        <button disabled={busy} onClick={save} className="px-5 py-2 bg-primary-500 text-white rounded text-sm hover:bg-primary-600 disabled:opacity-50">{busy ? '保存中…' : '保存设置'}</button>
      </div>
      {resetOpen && <ConfirmDialog title="恢复默认设置" message="将重置昵称、组名、直接用户、扫描、提示音和关闭行为。不会删除聊天记录，也不会改变已经启动的扫描。" onConfirm={() => void reset()} onCancel={() => { if (!busyRef.current) setResetOpen(false); }} />}
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return <section><h3 className="text-sm font-semibold text-gray-700 mb-3">{title}</h3><div className="space-y-4">{children}</div></section>;
}
function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return <div><label className="block text-sm text-gray-500 mb-1.5">{label}</label>{children}</div>;
}
