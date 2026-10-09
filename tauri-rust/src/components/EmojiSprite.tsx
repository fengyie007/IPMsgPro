import React, { useState } from 'react';
import { emojiStyle } from '../emojiData';
import { FEIQ_EMOJI_URLS } from '../feiqEmojiAssets';
import { getFeiqEmoji } from '../utils/emoticons';

/** Animated FeiQ assets, with the original sprite retained for old history. */
export default function EmojiSprite({ id, size }: { id: string; size: number }) {
  const [failedId, setFailedId] = useState<string | null>(null);
  const feiq = getFeiqEmoji(id);
  if (feiq) {
    if (failedId === id) return <span>{feiq.code}</span>;
    return <img src={FEIQ_EMOJI_URLS[id]} alt={feiq.code} title={`${feiq.name} ${feiq.code}`}
      data-emoji-id={id} width={size} height={size} draggable={false} loading="lazy"
      className="inline-block align-middle object-contain" style={{ width: size, height: size }} onError={() => setFailedId(id)} />;
  }
  const style = emojiStyle(id, size);
  if (!style) {
    return <span className="text-gray-400">[emoji:{id}]</span>;
  }
  return <span style={style} data-emoji-id={id} role="img" aria-label="表情" />;
}
