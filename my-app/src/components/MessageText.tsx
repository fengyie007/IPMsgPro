import React from 'react';
import { parseEmoticons } from '../utils/emoticons';
import { copyEditorSelection } from '../utils/chatEditor';
import EmojiSprite from './EmojiSprite';

export default function MessageText({ content }: { content: string }) {
  return <span className="whitespace-pre-wrap break-words" onCopy={(event) => {
    if (copyEditorSelection(event.currentTarget, event.clipboardData)) event.preventDefault();
  }}>
    {parseEmoticons(content).map((part, index) => part.type === 'text' ? part.text :
      <EmojiSprite key={`${index}:${part.id}`} id={part.id} size={part.id.startsWith('feiq:') ? 24 : 18} />)}
  </span>;
}
