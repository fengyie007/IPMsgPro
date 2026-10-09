import React, { useState } from 'react';
import type { Avatar, User } from '../types';
import { FEIQ_AVATAR_URLS } from '../feiqAvatarAssets';
import { useUserStore } from '../stores/userStore';
import { displayAvatar } from '../utils/avatar';

export default function UserAvatar({ user, avatar, size = 40, className = '' }: {
  user?: Pick<User, 'id' | 'nickname' | 'username' | 'avatar'>;
  avatar?: Avatar;
  size?: number;
  className?: string;
}) {
  const cached = useUserStore((state) => state.avatars.get(user?.id || ''));
  const selected = avatar === undefined ? displayAvatar(user?.avatar, cached) : displayAvatar(avatar);
  const id = selected?.kind === 'builtin' ? selected.id : undefined;
  const [failed, setFailed] = useState<number | null>(null);
  const name = user?.nickname || user?.username || '';
  const label = name ? `${name}的头像` : '内置头像';
  const classes = `shrink-0 rounded-lg bg-primary-100 inline-flex items-center justify-center overflow-hidden ${className}`;
  if (id !== undefined && failed !== id && FEIQ_AVATAR_URLS[id]) {
    return <img src={FEIQ_AVATAR_URLS[id]} alt={label} width={size} height={size} loading="lazy" draggable={false}
      className={`${classes} object-contain`} style={{ width: size, height: size }} onError={() => setFailed(id)} />;
  }
  return <span role="img" aria-label={name ? `${name}的默认头像` : '默认头像'} className={`${classes} text-primary-700 font-medium`}
    style={{ width: size, height: size, fontSize: Math.max(14, Math.round(size * 0.38)) }}>
    {Array.from(name.trim())[0]?.toUpperCase() || '?'}
  </span>;
}
