import React, { useCallback, useEffect, useRef, useState } from 'react';
import { FiArrowLeft, FiBell, FiDatabase, FiInfo, FiMonitor, FiPlus, FiRotateCcw, FiTrash2, FiUser, FiWifi } from 'react-icons/fi';
import { useConfigStore } from '../stores/configStore';
import { useWorkspaceStore, type SettingsCategory } from '../stores/workspaceStore';
import { toast } from '../stores/toastStore';
import { APP_NAME, APP_VERSION, type Config } from '../types';
import { normalizeDirectUser } from '../utils/netValidation';
import { prepareSettings, settingsDirty } from '../utils/settingsDraft';
import { useScrollMemory } from '../utils/useScrollMemory';
import { invoke } from '../services/bridge';
import ConfirmDialog from './ConfirmDialog';
import ScanSettings from './ScanSettings';
import NotificationSettings from './NotificationSettings';
import StorageSettings from './StorageSettings';

const categories: { id: SettingsCategory; label: string; description: string; icon: React.ReactNode }[] = [
  { id: 'profile', label: '个人信息', description: '让局域网中的联系人认出你', icon: <FiUser /> },
  { id: 'notifications', label: '消息通知', description: '选择新消息到来时的提醒方式', icon: <FiBell /> },
  { id: 'network', label: '网络发现', description: '发现局域网和指定网段中的联系人', icon: <FiWifi /> },
  { id: 'storage', label: '数据存储', description: '管理聊天记录和图片的存储位置', icon: <FiDatabase /> },
  { id: 'general', label: '通用', description: '调整窗口关闭时的行为', icon: <FiMonitor /> },
  { id: 'about', label: '关于', description: '应用版本与运行信息', icon: <FiInfo /> },
];

export default function Settings({ onClose }: { onClose: () => void }) {
  const config = useConfigStore((state) => state.config);
  const info = useConfigStore((state) => state.info);
  const category = useWorkspaceStore((state) => state.settingsCategory);
  const returnView = useWorkspaceStore((state) => state.returnView);
  const dirty = useWorkspaceStore((state) => state.settingsDirty);
  const saved = useRef(config);
  const [draft, setDraftState] = useState(config);
  const draftRef = useRef(draft);
  const [newUser, setNewUser] = useState(''), [scanInput, setScanInput] = useState('');
  const inputs = useRef({ direct: '', scan: '' });
  const [inputError, setInputError] = useState('');
  const [busy, setBusy] = useState(false), [nativeBusy, setNativeBusy] = useState(false);
  const [resetOpen, setResetOpen] = useState(false);
  const locks = useRef({ saving: false, native: false, reset: false });
  const updateLock = () => useWorkspaceStore.getState().setSettingsBusy(Object.values(locks.current).some(Boolean));
  const lock = (value: boolean) => { locks.current.saving = value; setBusy(value); updateLock(); };
  const showReset = (value: boolean) => { locks.current.reset = value; setResetOpen(value); updateLock(); };
  const nativeLock = useCallback((value: boolean) => {
    locks.current.native = value; setNativeBusy(value);
    useWorkspaceStore.getState().setSettingsBusy(Object.values(locks.current).some(Boolean));
  }, []);
  useEffect(() => () => { useWorkspaceStore.getState().setSettingsBusy(false); }, []);
  const markDirty = () => useWorkspaceStore.getState().setSettingsDirty(
    settingsDirty(saved.current, draftRef.current, inputs.current.direct, inputs.current.scan),
  );
  const setDraft: React.Dispatch<React.SetStateAction<Config>> = (value) => {
    const next = typeof value === 'function' ? value(draftRef.current) : value;
    draftRef.current = next; setDraftState(next); markDirty();
  };
  const changeDirect = (value: string) => { inputs.current.direct = value; setNewUser(value); setInputError(''); markDirty(); };
  const changeScan = (value: string) => { inputs.current.scan = value; setScanInput(value); markDirty(); };
  const acceptSaved = () => {
    const value = useConfigStore.getState().config;
    saved.current = value; draftRef.current = value; setDraftState(value);
    inputs.current = { direct: '', scan: '' }; setNewUser(''); setScanInput(''); setInputError('');
    useWorkspaceStore.getState().setSettingsDirty(false);
  };
  const addUser = () => {
    if (busy) return;
    const parsed = normalizeDirectUser(newUser);
    if ('error' in parsed) { setInputError(parsed.error); return; }
    if (!draft.directUsers.includes(parsed.value)) setDraft({ ...draft, directUsers: [...draft.directUsers, parsed.value] });
    changeDirect('');
  };
  const save = async () => {
    if (Object.values(locks.current).some(Boolean)) return;
    lock(true);
    try {
      const next = prepareSettings(draftRef.current, inputs.current.direct, inputs.current.scan);
      await useConfigStore.getState().saveConfig(next);
      acceptSaved(); toast.success('设置已保存并应用');
    } catch (error) { toast.error('保存设置失败：' + String(error)); }
    finally { lock(false); }
  };
  const reset = async () => {
    if (locks.current.saving) return;
    lock(true);
    try {
      await useConfigStore.getState().resetConfig();
      acceptSaved(); showReset(false); toast.success('已恢复默认设置');
    } catch (error) { toast.error('恢复设置失败：' + String(error)); }
    finally { lock(false); }
  };
  const locked = busy || nativeBusy;
  const selected = categories.find((item) => item.id === category)!;

  return <section aria-label="设置工作区" className="flex-1 flex flex-col min-[840px]:flex-row min-w-0 min-h-0 bg-white">
    <aside className="min-[840px]:w-44 shrink-0 bg-list-bg border-b min-[840px]:border-b-0 min-[840px]:border-r border-gray-200">
      <h1 className="hidden min-[840px]:flex h-16 items-center px-5 text-lg font-semibold text-gray-800">设置</h1>
      <nav aria-label="设置分类" className="flex min-[840px]:flex-col gap-1 overflow-x-auto px-2 py-2 min-[840px]:py-3">
        {categories.map((item) => <button key={item.id} disabled={locked} aria-label={item.label} aria-current={category === item.id ? 'page' : undefined}
          onClick={() => useWorkspaceStore.setState({ settingsCategory: item.id })}
          className={`flex items-center gap-2.5 whitespace-nowrap px-3 py-2.5 rounded-md text-sm text-left disabled:opacity-50 ${category === item.id ? 'bg-primary-100/70 text-primary-700 font-medium' : 'text-gray-600 hover:bg-gray-200/60'}`}>
          <span className="hidden min-[840px]:inline-flex text-base">{item.icon}</span>{item.label}
        </button>)}
      </nav>
    </aside>
    <div className="flex-1 flex flex-col min-w-0 min-h-0">
      <header className="h-16 px-5 min-[840px]:px-7 flex items-center justify-between gap-3 shrink-0 border-b border-gray-200">
        <h2 className="font-semibold text-gray-800">{selected.label}</h2>
        <button onClick={onClose} disabled={locked} className="inline-flex items-center gap-1.5 text-sm text-gray-500 hover:text-gray-800 disabled:opacity-50"><FiArrowLeft size={15} />返回{returnView === 'contacts' ? '通讯录' : '聊天'}</button>
      </header>
      <SettingsBody key={category} category={category}>
        <p className="text-sm text-gray-500 mb-7">{selected.description}</p>
        {category === 'profile' && <Section title="在通讯录中显示的信息">
          <Field label="昵称"><input aria-label="昵称" disabled={locked} className="input-field" value={draft.nickname} maxLength={128} onChange={(e) => setDraft({ ...draft, nickname: e.target.value })} placeholder="留空使用默认昵称" /></Field>
          <Field label="分组"><input aria-label="分组" disabled={locked} className="input-field" value={draft.group} maxLength={128} onChange={(e) => setDraft({ ...draft, group: e.target.value })} placeholder="例如：研发部" /></Field>
          <p className="text-xs text-gray-500">保存后，其他联系人将看到更新后的昵称和分组。</p>
        </Section>}
        {category === 'notifications' && <div className="space-y-8">
          <NotificationSettings enabled={draft.notificationSound} supported={info?.capabilities.notificationSound ?? false} disabled={locked} onChange={(notificationSound) => setDraft({ ...draft, notificationSound })} />
          <Section title="系统通知">
            <label className="flex items-center gap-2 text-sm text-gray-700"><input type="checkbox" disabled={locked || !info?.systemNotifications} checked={draft.systemNotifications} onChange={(e) => setDraft({ ...draft, systemNotifications: e.target.checked })} />显示 Windows 系统通知</label>
            <label className="flex items-center gap-2 text-sm text-gray-700"><input type="checkbox" disabled={locked || !info?.systemNotifications} checked={draft.notificationPreview} onChange={(e) => setDraft({ ...draft, notificationPreview: e.target.checked })} />在通知中显示文字消息摘要</label>
            <button disabled={locked || !info?.systemNotifications} className="text-sm text-primary-700 disabled:opacity-40" onClick={() => void invoke('notification.test_system').then(() => toast.info('已交给 Windows 显示通知；未显示时请检查系统通知设置')).catch((e) => toast.error(String(e)))}>测试系统通知</button>
            <p className="text-xs text-gray-500 leading-relaxed">正在查看的聊天不重复提醒。浏览通讯录、设置或切换到后台时，仍可接收新消息提醒。点击通知打开对应会话。</p>
            {!info?.systemNotifications && <p className="text-xs text-gray-500">系统通知需要 Windows 桌面版。</p>}
          </Section>
        </div>}
        {category === 'network' && <div className="space-y-8">
          <Section title="直接添加用户">
            <p className="text-xs text-gray-500">填写对方的 IPv4 地址和端口，保存后发送发现请求。</p>
            <div className="flex gap-2"><input aria-label="直接用户地址" disabled={locked} className="input-field flex-1" value={newUser} onChange={(e) => changeDirect(e.target.value)} placeholder="例如 192.168.2.88:2425" onKeyDown={(e) => { if (e.key === 'Enter' && !e.nativeEvent.isComposing) { e.preventDefault(); addUser(); } }} />
              <button disabled={locked} onClick={addUser} className="px-3 rounded border border-gray-200 disabled:opacity-50" title="添加直接用户"><FiPlus /></button></div>
            {inputError && <p role="alert" className="text-xs text-red-500">{inputError}</p>}
            <div className="space-y-1">{draft.directUsers.map((address) => <div key={address} className="flex items-center justify-between gap-2 py-1 px-2 rounded bg-gray-50 text-sm text-gray-600"><span className="break-all">{address}</span><button disabled={locked} title={`移除 ${address}`} className="p-1 text-gray-400 hover:text-red-500" onClick={() => setDraft({ ...draft, directUsers: draft.directUsers.filter((item) => item !== address) })}><FiTrash2 size={14} /></button></div>)}</div>
          </Section>
          {info?.capabilities.scan && <ScanSettings draft={draft} setDraft={setDraft} disabled={locked} input={scanInput} onInputChange={changeScan} />}
        </div>}
        {category === 'storage' && <><StorageSettings disabled={busy} onBusyChange={nativeLock} /><p className="mt-6 text-xs text-gray-500 leading-relaxed">目录切换需要单独确认，重启后生效；不需要再点击下方“保存设置”。</p></>}
        {category === 'general' && <Section title="关闭主窗口时">
          <div className="space-y-3">{(['tray', 'taskbar'] as const).map((behavior) => <label key={behavior} className="flex items-start gap-3 text-sm text-gray-700"><input className="mt-1" type="radio" name="close-behavior" disabled={locked} checked={draft.minimizeBehavior === behavior} onChange={() => setDraft({ ...draft, minimizeBehavior: behavior })} /><span>{behavior === 'tray' ? '隐藏到系统托盘' : '退出程序'}<span className="block text-xs text-gray-500 mt-1">{behavior === 'tray' ? '继续接收消息，可从托盘恢复窗口。' : '结束运行，停止接收消息。'}</span></span></label>)}</div>
          <p className="text-xs text-gray-500">标题栏最小化按钮仍会将窗口最小化到任务栏。</p>
        </Section>}
        {category === 'about' && <Section title={APP_NAME}>
          <p className="text-sm text-gray-700">版本 {APP_VERSION}</p>
          <p className="text-sm text-gray-500">用于局域网中的消息、图片和文件交流。</p>
          <dl className="grid grid-cols-[5rem_minmax(0,1fr)] gap-3 text-sm"><dt className="text-gray-500">监听端口</dt><dd>{info?.port ?? '—'}</dd><dt className="text-gray-500">数据目录</dt><dd className="break-all text-gray-700">{info?.dataDir || '—'}</dd></dl>
        </Section>}
      </SettingsBody>
      <footer className="border-t px-5 min-[840px]:px-7 py-4 flex items-center justify-between gap-3 shrink-0">
        <button disabled={locked} onClick={() => showReset(true)} className="inline-flex items-center gap-1.5 text-xs text-gray-500 hover:text-gray-700 disabled:opacity-50"><FiRotateCcw size={14} />恢复默认</button>
        <div className="flex items-center gap-3"><span role="status" className={`text-xs ${dirty ? 'text-amber-700' : 'text-gray-400'}`}>{dirty ? '有未保存的更改' : '设置已保存'}</span><button disabled={locked || !dirty} onClick={() => void save()} className="px-4 py-2 bg-primary-600 text-white rounded-md text-sm hover:bg-primary-700 disabled:opacity-40">{busy ? '保存中…' : '保存设置'}</button></div>
      </footer>
    </div>
    {resetOpen && <ConfirmDialog title="恢复默认设置" message="将重置个人信息、网络发现、消息通知和关闭行为。聊天记录、数据目录及已经启动的扫描不变。" onConfirm={() => void reset()} onCancel={() => { if (!busy) showReset(false); }} />}
  </section>;
}

function SettingsBody({ category, children }: { category: SettingsCategory; children: React.ReactNode }) {
  const scroll = useScrollMemory(`settings:${category}`);
  return <div {...scroll} className="flex-1 overflow-y-auto p-5 min-[840px]:p-7"><div className="max-w-2xl">{children}</div></div>;
}
function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return <section><h3 className="text-sm font-semibold text-gray-800 mb-4">{title}</h3><div className="space-y-4">{children}</div></section>;
}
function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return <div><p className="text-sm text-gray-600 mb-2">{label}</p>{children}</div>;
}
