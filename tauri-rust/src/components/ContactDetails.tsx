import React from 'react';
import { FiCopy, FiMessageSquare, FiUsers } from 'react-icons/fi';
import { useUserStore } from '../stores/userStore';
import { useWorkspaceStore } from '../stores/workspaceStore';
import { toast } from '../stores/toastStore';
import { useScrollMemory } from '../utils/useScrollMemory';

export default function ContactDetails() {
  const contactId = useWorkspaceStore((state) => state.contactId);
  const scroll = useScrollMemory(`contact:${contactId}`);
  const user = useUserStore((state) => state.users.find((item) => item.id === contactId));
  if (!user) return <section className="flex-1 min-w-0 flex items-center justify-center bg-white p-8" aria-label="联系人资料">
    <div className="text-center text-gray-400"><FiUsers size={40} className="mx-auto mb-4" /><h2 className="text-base text-gray-600">选择一位联系人</h2><p className="mt-2 text-sm">查看资料，或双击联系人开始聊天</p></div>
  </section>;
  const status = { online: '在线', away: '离开', offline: '离线' }[user.status];
  const address = user.ip ? `${user.ip}:${user.port}` : '';
  const copyAddress = async () => {
    try { await navigator.clipboard.writeText(address); toast.success('已复制联系地址'); }
    catch { toast.error('复制失败，请选择地址后手动复制'); }
  };
  return <section className="flex-1 min-w-0 flex flex-col bg-white" aria-label="联系人资料">
    <header className="h-16 border-b border-gray-200 px-6 flex items-center shrink-0"><h2 className="font-semibold text-gray-800">联系人资料</h2></header>
    <div {...scroll} className="flex-1 overflow-y-auto px-6 py-8 min-[840px]:px-10">
      <div className="max-w-lg mx-auto">
        <div className="flex items-center gap-4 pb-7 border-b border-gray-100">
          <div className="w-16 h-16 shrink-0 rounded-xl bg-primary-100 text-primary-700 flex items-center justify-center text-2xl font-medium">{(user.nickname || user.username).slice(0, 1).toUpperCase()}</div>
          <div className="min-w-0"><h3 className="text-xl font-semibold text-gray-800 break-words">{user.nickname || user.username}</h3><p className="flex items-center gap-2 mt-2 text-sm text-gray-500"><span className={`h-2 w-2 rounded-full ${user.status === 'online' ? 'bg-primary-500' : user.status === 'away' ? 'bg-amber-400' : 'bg-gray-400'}`} />{status}</p></div>
        </div>
        <dl className="grid grid-cols-[5rem_minmax(0,1fr)] gap-y-5 gap-x-4 py-7 text-sm">
          <dt className="text-gray-500">分组</dt><dd className="text-gray-800 break-words">{user.group || '未分组'}</dd>
          <dt className="text-gray-500">用户名</dt><dd className="text-gray-800 break-all">{user.username || '—'}</dd>
          <dt className="text-gray-500">计算机</dt><dd className="text-gray-800 break-all">{user.hostname || '—'}</dd>
          <dt className="text-gray-500">联系地址</dt><dd className="text-gray-800 flex items-start gap-2 min-w-0"><span className="break-all select-text">{address || '暂无地址'}</span>{address && <button className="p-1 shrink-0 text-gray-400 hover:text-primary-700" title="复制联系地址" onClick={() => void copyAddress()}><FiCopy size={14} /></button>}</dd>
        </dl>
        {user.status === 'offline' && <p className="mb-4 text-xs text-gray-500">联系人当前离线，发送前请确认对方已启动应用。</p>}
        <button className="inline-flex items-center gap-2 px-6 py-2.5 rounded-md bg-primary-600 hover:bg-primary-700 text-white text-sm" onClick={() => useWorkspaceStore.getState().openChat(user)}><FiMessageSquare size={17} />发消息</button>
      </div>
    </div>
  </section>;
}
