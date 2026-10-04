import React, { memo } from 'react';
import { FiAlertCircle, FiCheck, FiClock } from 'react-icons/fi';
import type { Message } from '../types';
import { parseEmojiId, EMOJI_TOKEN_RE } from '../emojiData';
import { formatTime } from '../utils/format';
import EmojiSprite from './EmojiSprite';

function renderText(content: string) {
  const emojiId = parseEmojiId(content);
  if (emojiId) return <EmojiSprite id={emojiId} size={18} />;
  const parts: React.ReactNode[] = [];
  let last = 0, match: RegExpExecArray | null;
  EMOJI_TOKEN_RE.lastIndex = 0;
  while ((match = EMOJI_TOKEN_RE.exec(content))) {
    if (match.index > last) parts.push(content.slice(last, match.index));
    parts.push(<EmojiSprite key={`${match.index}:${match[1]}`} id={match[1]} size={18} />);
    last = match.index + match[0].length;
  }
  if (last < content.length) parts.push(content.slice(last));
  return <span className="whitespace-pre-wrap break-words">{parts}</span>;
}

export default memo(function MessageBubble({ message }: { message: Message }) {
  const self = message.from === 'self';
  return (
    <div className={`flex ${self ? 'justify-end' : 'justify-start'} message-in`}>
      <div className="max-w-[70%] min-w-0">
        <div className={`px-3 py-2 rounded-lg text-sm leading-relaxed break-words ${self ? 'bg-chat-sent text-gray-800 rounded-tr-none' : 'bg-chat-received text-gray-800 rounded-tl-none shadow-sm'}`}>
          {message.type === 'text' ? renderText(message.content) : (
            <span className="text-gray-500">[{message.type === 'image' ? '图片' : '文件'}：Rust 核心版暂不支持，请使用原版查看]</span>
          )}
        </div>
        <div className={`flex items-center gap-1 text-[10px] text-gray-400 mt-0.5 ${self ? 'justify-end' : 'justify-start'}`}>
          <span>{formatTime(message.timestamp)}</span>
          {self && (message.status === 'sending'
            ? <FiClock size={10} title="等待对方确认" />
            : message.status === 'delivered'
            ? <FiCheck size={10} className="text-green-500" title="对方已收到" />
            : <FiAlertCircle size={10} className="text-red-500" title="发送失败或确认超时" />)}
        </div>
      </div>
    </div>
  );
});
