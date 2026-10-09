import { AVATAR_COUNT, type Avatar } from '../types';

export function validAvatarId(id: unknown): id is number {
  return typeof id === 'number' && Number.isInteger(id) && id >= 0 && id < AVATAR_COUNT;
}
export function validAvatar(value: unknown): value is Avatar {
  if (!value || typeof value !== 'object') return false;
  const avatar = value as Avatar;
  return avatar.kind === 'custom' || avatar.kind === 'unknown' || (avatar.kind === 'builtin' && validAvatarId(avatar.id));
}
export function displayAvatar(primary: unknown, cached?: unknown): Avatar | undefined {
  return validAvatar(primary) ? primary : validAvatar(cached) ? cached : undefined;
}
