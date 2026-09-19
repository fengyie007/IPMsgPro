import React from 'react';
import { emojiStyle } from '../emojiData';

/** One emoji drawn from the sprite sheet (emoji.png) via background-position. */
export default function EmojiSprite({ id, size }: { id: string; size: number }) {
  const style = emojiStyle(id, size);
  if (!style) {
    return <span className="text-gray-400">[emoji:{id}]</span>;
  }
  return <span style={style} />;
}
