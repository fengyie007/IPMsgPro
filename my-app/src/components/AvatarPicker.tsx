import React, { useState } from 'react';
import { AVATAR_COUNT } from '../types';
import UserAvatar from './UserAvatar';

export default function AvatarPicker({ value, onChange, disabled }: {
  value: number; onChange: (id: number) => void; disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  return <section className="space-y-3" aria-label="选择内置头像">
    <div className="flex items-center gap-4">
      <UserAvatar avatar={{ kind: 'builtin', id: value }} size={64} className="rounded-xl" />
      <div><p className="text-sm text-gray-700">头像 {value + 1}</p><button type="button" disabled={disabled} aria-expanded={open}
        onClick={() => setOpen(!open)} className="mt-2 text-sm text-primary-700 disabled:opacity-50">{open ? '收起头像' : '选择头像'}</button></div>
    </div>
    {open && <div className="grid grid-cols-6 min-[840px]:grid-cols-8 gap-2 p-2 border border-gray-200 rounded-lg max-h-64 overflow-y-auto">
      {Array.from({ length: AVATAR_COUNT }, (_, id) => <button key={id} type="button" disabled={disabled}
        aria-label={`头像 ${id + 1}`} aria-pressed={id === value} title={`头像 ${id + 1}`}
        onClick={() => onChange(id)} className={`flex justify-center p-1 rounded border disabled:opacity-50 ${id === value ? 'border-primary-600 bg-primary-50' : 'border-transparent hover:bg-gray-100'}`}>
        <UserAvatar avatar={{ kind: 'builtin', id }} size={36} />
      </button>)}
    </div>}
    <p className="text-xs text-gray-500">选择后点击“保存设置”，联系人将收到头像更新。</p>
  </section>;
}
