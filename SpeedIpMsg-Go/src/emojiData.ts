// Emoji definitions for the chat emoji picker.
//
// Emoji are rendered from a single sprite sheet
// (`frontend/src/assets/emoji.png`) using CSS background-position.
// The position data is generated from `emoji_positon.less`.
//
// The wire / history format follows the WeChat-style XML:
//
//   <msg><emoji type="1" id="<name>" /></msg>
//
// When sent or stored, only that XML string is transmitted/saved; the
// image is resolved on render from the `id` (the sprite class name).

import emojiPng from './assets/emoji.png';
import type { CSSProperties } from 'react';

export interface EmojiDef {
  id: string; // sprite class name, also used as the XML id
  x: number;  // background-position x (px)
  y: number;  // background-position y (px)
  w: number;  // native cell width (px)
  h: number;  // native cell height (px)
}

// The bundled sprite sheet (Vite handles the import).
export const SPRITE_URL = emojiPng;

export const EMOJIS: EmojiDef[] = [
  { id: 'e2_02', x: -66, y: 0, w: 64, h: 64 },
  { id: 'e2_04', x: -462, y: -396, w: 64, h: 64 },
  { id: 'e2_05', x: 0, y: -66, w: 64, h: 64 },
  { id: 'e2_06', x: -66, y: -66, w: 64, h: 64 },
  { id: 'e2_09', x: -132, y: 0, w: 64, h: 64 },
  { id: 'e2_11', x: -132, y: -66, w: 64, h: 64 },
  { id: 'e2_12', x: 0, y: -132, w: 64, h: 64 },
  { id: 'e2_14', x: -66, y: -132, w: 64, h: 64 },
  { id: 'smiley_0', x: -132, y: -132, w: 64, h: 64 },
  { id: 'smiley_1', x: -660, y: -594, w: 63, h: 64 },
  { id: 'smiley_10', x: -198, y: -66, w: 64, h: 64 },
  { id: 'smiley_11', x: -198, y: -132, w: 64, h: 64 },
  { id: 'smiley_12', x: 0, y: -198, w: 64, h: 64 },
  { id: 'smiley_13', x: -66, y: -198, w: 64, h: 64 },
  { id: 'smiley_14', x: -132, y: -198, w: 64, h: 64 },
  { id: 'smiley_15', x: -198, y: -198, w: 64, h: 64 },
  { id: 'smiley_17', x: -264, y: 0, w: 64, h: 64 },
  { id: 'smiley_18', x: -264, y: -66, w: 64, h: 64 },
  { id: 'smiley_19', x: -264, y: -132, w: 64, h: 64 },
  { id: 'smiley_2', x: -264, y: -198, w: 64, h: 64 },
  { id: 'smiley_20', x: 0, y: -264, w: 64, h: 64 },
  { id: 'smiley_21', x: -66, y: -264, w: 64, h: 64 },
  { id: 'smiley_22', x: -132, y: -264, w: 64, h: 64 },
  { id: 'smiley_23', x: -198, y: -264, w: 64, h: 64 },
  { id: 'smiley_25', x: -264, y: -264, w: 64, h: 64 },
  { id: 'smiley_26', x: -330, y: 0, w: 64, h: 64 },
  { id: 'smiley_27', x: -330, y: -66, w: 64, h: 64 },
  { id: 'smiley_28', x: -330, y: -132, w: 64, h: 64 },
  { id: 'smiley_29', x: -330, y: -198, w: 64, h: 64 },
  { id: 'smiley_3', x: -330, y: -264, w: 64, h: 64 },
  { id: 'smiley_30', x: 0, y: -330, w: 64, h: 64 },
  { id: 'smiley_31', x: -66, y: -330, w: 64, h: 64 },
  { id: 'smiley_313', x: -132, y: -330, w: 64, h: 64 },
  { id: 'smiley_314', x: -198, y: -330, w: 64, h: 64 },
  { id: 'smiley_315', x: -264, y: -330, w: 64, h: 64 },
  { id: 'smiley_316', x: -330, y: -330, w: 64, h: 64 },
  { id: 'smiley_317', x: -396, y: 0, w: 64, h: 64 },
  { id: 'smiley_318', x: -396, y: -66, w: 64, h: 64 },
  { id: 'smiley_319', x: -396, y: -132, w: 64, h: 64 },
  { id: 'smiley_32', x: -396, y: -198, w: 64, h: 64 },
  { id: 'smiley_320', x: -396, y: -264, w: 64, h: 64 },
  { id: 'smiley_321', x: -396, y: -330, w: 64, h: 64 },
  { id: 'smiley_322', x: 0, y: -396, w: 64, h: 64 },
  { id: 'smiley_33', x: -66, y: -396, w: 64, h: 64 },
  { id: 'smiley_34', x: -132, y: -396, w: 64, h: 64 },
  { id: 'smiley_36', x: -198, y: -396, w: 64, h: 64 },
  { id: 'smiley_37', x: -264, y: -396, w: 64, h: 64 },
  { id: 'smiley_38', x: -330, y: -396, w: 64, h: 64 },
  { id: 'smiley_39', x: -396, y: -396, w: 64, h: 64 },
  { id: 'smiley_4', x: -462, y: 0, w: 64, h: 64 },
  { id: 'smiley_40', x: -462, y: -66, w: 64, h: 64 },
  { id: 'smiley_41', x: -462, y: -132, w: 64, h: 64 },
  { id: 'smiley_42', x: -462, y: -198, w: 64, h: 64 },
  { id: 'smiley_44', x: -462, y: -264, w: 64, h: 64 },
  { id: 'smiley_45', x: -462, y: -330, w: 64, h: 64 },
  { id: 'smiley_46', x: 0, y: 0, w: 64, h: 64 },
  { id: 'smiley_47', x: 0, y: -462, w: 64, h: 64 },
  { id: 'smiley_48', x: -66, y: -462, w: 64, h: 64 },
  { id: 'smiley_49', x: -132, y: -462, w: 64, h: 64 },
  { id: 'smiley_5', x: -198, y: -462, w: 64, h: 64 },
  { id: 'smiley_50', x: -264, y: -462, w: 64, h: 64 },
  { id: 'smiley_51', x: -330, y: -462, w: 64, h: 64 },
  { id: 'smiley_52', x: -396, y: -462, w: 64, h: 64 },
  { id: 'smiley_54', x: -462, y: -462, w: 64, h: 64 },
  { id: 'smiley_55', x: -528, y: 0, w: 64, h: 64 },
  { id: 'smiley_56', x: -528, y: -66, w: 64, h: 64 },
  { id: 'smiley_57', x: -528, y: -132, w: 64, h: 64 },
  { id: 'smiley_6', x: -528, y: -198, w: 64, h: 64 },
  { id: 'smiley_60', x: -528, y: -264, w: 64, h: 64 },
  { id: 'smiley_61', x: -528, y: -330, w: 64, h: 64 },
  { id: 'smiley_62', x: -528, y: -396, w: 64, h: 64 },
  { id: 'smiley_63', x: -528, y: -462, w: 64, h: 64 },
  { id: 'smiley_64', x: 0, y: -528, w: 64, h: 64 },
  { id: 'smiley_65', x: -66, y: -528, w: 64, h: 64 },
  { id: 'smiley_66', x: -132, y: -528, w: 64, h: 64 },
  { id: 'smiley_67', x: -198, y: -528, w: 64, h: 64 },
  { id: 'smiley_68', x: -264, y: -528, w: 64, h: 64 },
  { id: 'smiley_7', x: -330, y: -528, w: 64, h: 64 },
  { id: 'smiley_70', x: -396, y: -528, w: 64, h: 64 },
  { id: 'smiley_74', x: -462, y: -528, w: 64, h: 64 },
  { id: 'smiley_75', x: -528, y: -528, w: 64, h: 64 },
  { id: 'smiley_76', x: -594, y: 0, w: 64, h: 64 },
  { id: 'smiley_78', x: -594, y: -66, w: 64, h: 64 },
  { id: 'smiley_79', x: -594, y: -132, w: 64, h: 64 },
  { id: 'smiley_8', x: -594, y: -198, w: 64, h: 64 },
  { id: 'smiley_80', x: -594, y: -264, w: 64, h: 64 },
  { id: 'smiley_81', x: -594, y: -330, w: 64, h: 64 },
  { id: 'smiley_82', x: -594, y: -396, w: 64, h: 64 },
  { id: 'smiley_83', x: -594, y: -462, w: 64, h: 64 },
  { id: 'smiley_84', x: -594, y: -528, w: 64, h: 64 },
  { id: 'smiley_85', x: 0, y: -594, w: 64, h: 64 },
  { id: 'smiley_89', x: -66, y: -594, w: 64, h: 64 },
  { id: 'smiley_9', x: -132, y: -594, w: 64, h: 64 },
  { id: 'smiley_92', x: -198, y: -594, w: 64, h: 64 },
  { id: 'smiley_93', x: -264, y: -594, w: 64, h: 64 },
  { id: 'smiley_94', x: -330, y: -594, w: 64, h: 64 },
  { id: 'smiley_95', x: -396, y: -594, w: 64, h: 64 },
  { id: 'u1F381', x: -462, y: -594, w: 64, h: 64 },
  { id: 'u1F389', x: -528, y: -594, w: 64, h: 64 },
  { id: 'u1F47B', x: -594, y: -594, w: 64, h: 64 },
  { id: 'u1F4AA', x: -660, y: 0, w: 64, h: 64 },
  { id: 'u1F602', x: -660, y: -66, w: 64, h: 64 },
  { id: 'u1F604', x: -660, y: -132, w: 64, h: 64 },
  { id: 'u1F612', x: -660, y: -198, w: 64, h: 64 },
  { id: 'u1F614', x: -660, y: -264, w: 64, h: 64 },
  { id: 'u1F61D', x: -660, y: -330, w: 64, h: 64 },
  { id: 'u1F631', x: -660, y: -396, w: 64, h: 64 },
  { id: 'u1F633', x: -660, y: -462, w: 64, h: 64 },
  { id: 'u1F637', x: -198, y: 0, w: 64, h: 64 },
  { id: 'u1F64F', x: -660, y: -528, w: 64, h: 64 },
];

// Total sprite sheet dimensions, derived from the extreme cell offsets.
export const SPRITE_WIDTH = Math.max(...EMOJIS.map((e) => -e.x + e.w));
export const SPRITE_HEIGHT = Math.max(...EMOJIS.map((e) => -e.y + e.h));

// Anchored matcher for a standalone emoji message.
const EMOJI_RE = /^<msg><emoji type="1" id="([^"]+)" \/><\/msg>$/;

// Global matcher used to split a message that mixes text and emoji tokens.
export const EMOJI_TOKEN_RE = /<msg><emoji type="1" id="([^"]+)" \/><\/msg>/g;

// Inline CSS to render one emoji cell from the sprite at the given pixel size.
export function emojiStyle(id: string, size: number): CSSProperties | undefined {
  const e = getEmoji(id);
  if (!e) return undefined;
  const scale = size / e.w;
  return {
    display: 'inline-block',
    width: `${size}px`,
    height: `${size}px`,
    backgroundImage: `url(${SPRITE_URL})`,
    backgroundRepeat: 'no-repeat',
    backgroundSize: `${SPRITE_WIDTH * scale}px ${SPRITE_HEIGHT * scale}px`,
    backgroundPosition: `${e.x * scale}px ${e.y * scale}px`,
    verticalAlign: 'middle',
    flexShrink: 0,
  };
}


export function buildEmojiMessage(id: string): string {
  return `<msg><emoji type="1" id="${id}" /></msg>`;
}

export function parseEmojiId(content: string): string | null {
  const m = content.trim().match(EMOJI_RE);
  return m ? m[1] : null;
}

export function isEmojiMessage(content: string): boolean {
  return EMOJI_RE.test(content.trim());
}

export function getEmoji(id: string): EmojiDef | undefined {
  return EMOJIS.find((e) => e.id === id);
}
