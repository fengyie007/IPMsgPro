// One-off generator: convert frontend/src/assets/emoji_positon.less into a
// self-contained TS sprite map (frontend/src/emojiData.ts). Run with:
//   node scripts/gen_emoji_ts.cjs
const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..');
const lessPath = path.join(root, 'src', 'assets', 'emoji_positon.less');
const outPath = path.join(root, 'src', 'emojiData.ts');

const src = fs.readFileSync(lessPath, 'utf8');

// Match individual rules like: .smiley_0 { width: 64px; height: 64px; background-position: -132px -132px }
const re = /\.([A-Za-z0-9_]+)\s*\{\s*width:\s*(\d+)px;\s*height:\s*(\d+)px;\s*background-position:\s*(-?\d+)(?:px)?\s+(-?\d+)(?:px)?\s*\}/g;

const emojis = [];
let m;
while ((m = re.exec(src)) !== null) {
  emojis.push({
    id: m[1],
    w: parseInt(m[2], 10),
    h: parseInt(m[3], 10),
    x: parseInt(m[4], 10),
    y: parseInt(m[5], 10),
  });
}

emojis.sort((a, b) => a.id.localeCompare(b.id));

const lines = [];
lines.push('// Emoji definitions for the chat emoji picker.');
lines.push('//');
lines.push('// Emoji are rendered from a single sprite sheet');
lines.push('// (`frontend/src/assets/emoji.png`) using CSS background-position.');
lines.push('// The position data is generated from `emoji_positon.less`.');
lines.push('//');
lines.push('// The wire / history format follows the WeChat-style XML:');
lines.push('//');
lines.push('//   <msg><emoji type="1" id="<name>" /></msg>');
lines.push('//');
lines.push('// When sent or stored, only that XML string is transmitted/saved; the');
lines.push('// image is resolved on render from the `id` (the sprite class name).');
lines.push('');
lines.push("import emojiPng from './assets/emoji.png';");
lines.push("import type { CSSProperties } from 'react';");
lines.push('');
lines.push('export interface EmojiDef {');
lines.push('  id: string; // sprite class name, also used as the XML id');
lines.push('  x: number;  // background-position x (px)');
lines.push('  y: number;  // background-position y (px)');
lines.push('  w: number;  // native cell width (px)');
lines.push('  h: number;  // native cell height (px)');
lines.push('}');
lines.push('');
lines.push('// The bundled sprite sheet (Vite handles the import).');
lines.push('export const SPRITE_URL = emojiPng;');
lines.push('');
lines.push('export const EMOJIS: EmojiDef[] = [');
for (const e of emojis) {
  lines.push(`  { id: '${e.id}', x: ${e.x}, y: ${e.y}, w: ${e.w}, h: ${e.h} },`);
}
lines.push('];');
lines.push('');
lines.push('// Total sprite sheet dimensions, derived from the extreme cell offsets.');
lines.push('export const SPRITE_WIDTH = Math.max(...EMOJIS.map((e) => -e.x + e.w));');
lines.push('export const SPRITE_HEIGHT = Math.max(...EMOJIS.map((e) => -e.y + e.h));');
lines.push('');
lines.push('// Anchored matcher for a standalone emoji message.');
lines.push('const EMOJI_RE = /^<msg><emoji type="1" id="([^"]+)" \\/><\\/msg>$/;');
lines.push('');
lines.push('// Global matcher used to split a message that mixes text and emoji tokens.');
lines.push('export const EMOJI_TOKEN_RE = /<msg><emoji type="1" id="([^"]+)" \\/><\\/msg>/g;');
lines.push('');
lines.push('// Inline CSS to render one emoji cell from the sprite at the given pixel size.');
lines.push('export function emojiStyle(id: string, size: number): CSSProperties | undefined {');
lines.push('  const e = getEmoji(id);');
lines.push('  if (!e) return undefined;');
lines.push('  const scale = size / e.w;');
lines.push('  return {');
lines.push('    display: \'inline-block\',');
lines.push('    width: `${size}px`,');
lines.push('    height: `${size}px`,');
lines.push('    backgroundImage: `url(${SPRITE_URL})`,');
lines.push('    backgroundRepeat: \'no-repeat\',');
lines.push('    backgroundSize: `${SPRITE_WIDTH * scale}px ${SPRITE_HEIGHT * scale}px`,');
lines.push('    backgroundPosition: `${e.x * scale}px ${e.y * scale}px`,');
lines.push('    verticalAlign: \'middle\',');
lines.push('    flexShrink: 0,');
lines.push('  };');
lines.push('}');
lines.push('');
lines.push('');
lines.push('export function buildEmojiMessage(id: string): string {');
lines.push('  return `<msg><emoji type="1" id="${id}" /></msg>`;');
lines.push('}');
lines.push('');
lines.push('export function parseEmojiId(content: string): string | null {');
lines.push('  const m = content.trim().match(EMOJI_RE);');
lines.push('  return m ? m[1] : null;');
lines.push('}');
lines.push('');
lines.push('export function isEmojiMessage(content: string): boolean {');
lines.push('  return EMOJI_RE.test(content.trim());');
lines.push('}');
lines.push('');
lines.push('export function getEmoji(id: string): EmojiDef | undefined {');
lines.push('  return EMOJIS.find((e) => e.id === id);');
lines.push('}');
lines.push('');

fs.writeFileSync(outPath, lines.join('\n'), 'utf8');
console.log(`Wrote ${emojis.length} emojis to ${outPath}`);
