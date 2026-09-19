import React from 'react';
import { EMOJIS } from '../emojiData';
import EmojiSprite from './EmojiSprite';

interface EmojiPickerProps {
  onSelect: (id: string) => void;
  onClose: () => void;
}

/**
 * Emoji grid anchored above the chat input. Selecting keeps the picker open so
 * several emojis can be inserted; clicking anywhere outside closes it.
 */
export default function EmojiPicker({ onSelect, onClose }: EmojiPickerProps) {
  return (
    <>
      <div className="fixed inset-0 z-10" onClick={onClose} />
      <div className="absolute bottom-full left-0 mb-2 w-[30rem] max-h-72 overflow-y-auto
                      bg-white border border-gray-200 rounded-lg shadow-lg p-2
                      grid grid-cols-10 gap-1 z-20">
        {EMOJIS.map((e) => (
          <button
            key={e.id}
            title={e.id}
            className="p-1 hover:bg-gray-100 rounded transition-colors flex items-center justify-center"
            onClick={() => onSelect(e.id)}
          >
            <EmojiSprite id={e.id} size={32} />
          </button>
        ))}
      </div>
    </>
  );
}
