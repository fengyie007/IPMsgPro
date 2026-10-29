import React, { useEffect, useState } from 'react';
import { FiX } from 'react-icons/fi';
import { FEIQ_EMOJIS } from '../feiqEmojiData';
import EmojiSprite from './EmojiSprite';

interface EmojiPickerProps {
  onSelect: (id: string) => void;
  onClose: () => void;
  disabled?: boolean;
}

/**
 * Emoji grid anchored above the chat input. Selecting keeps the picker open so
 * several emojis can be inserted; clicking anywhere outside closes it.
 */
export default function EmojiPicker({ onSelect, onClose, disabled = false }: EmojiPickerProps) {
  const [hint, setHint] = useState(FEIQ_EMOJIS[0]);
  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); onClose(); }
    };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [onClose]);
  return (
    <>
      <div className="fixed inset-0 z-10" onClick={onClose} />
      <div role="region" aria-label="飞秋表情" className="absolute bottom-full left-2 right-2 max-w-96 mb-2 bg-white border border-gray-200 rounded-lg shadow-lg z-20">
        <div className="flex items-center justify-between px-3 py-2 border-b border-gray-100"><span className="text-sm text-gray-700">飞秋表情</span><button type="button" aria-label="关闭表情" className="p-1 text-gray-400 hover:text-gray-700" onClick={onClose}><FiX size={15} /></button></div>
        <div className="overflow-y-auto p-2 grid grid-cols-10 gap-1" style={{ maxHeight: 'min(15rem, calc(100vh - 17rem))' }}>
        {FEIQ_EMOJIS.map((e) => (
          <button
            key={e.id}
            type="button" disabled={disabled}
            title={`${e.name} ${e.code}`} aria-label={`${e.name} ${e.code}`}
            onMouseEnter={() => setHint(e)} onFocus={() => setHint(e)}
            onMouseDown={(event) => event.preventDefault()}
            className="p-0.5 hover:bg-primary-50 rounded transition-colors flex items-center justify-center disabled:opacity-40"
            onClick={() => onSelect(e.id)}
          >
            <EmojiSprite id={e.id} size={24} />
          </button>
        ))}
        </div>
        <div className="px-3 py-2 border-t border-gray-100 text-xs text-gray-500 flex items-center justify-between gap-3"><span>{hint.name}</span><code className="text-gray-600">{hint.code}</code></div>
      </div>
    </>
  );
}
