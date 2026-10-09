import { FEIQ_EMOJIS, type FeiqEmojiDef } from '../feiqEmojiData';
import { buildEmojiMessage, EMOJI_TOKEN_RE, getEmoji, parseEmojiId } from '../emojiData';

const byId = new Map<string, FeiqEmojiDef>(FEIQ_EMOJIS.map((emoji) => [emoji.id, emoji]));
const byCode = new Map<string, FeiqEmojiDef>(FEIQ_EMOJIS.map((emoji) => [emoji.code, emoji]));
const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const facePattern = [...byCode.keys()].sort((a, b) => b.length - a.length).map(escaped).join('|');
// URLs win before emoji matching so a link path containing /:) remains readable.
const tokenPattern = `https?:\\/\\/[^\\s<>]+|${EMOJI_TOKEN_RE.source}|${facePattern}`;

export type EmoticonToken = { type: 'text'; text: string } | { type: 'emoji'; id: string; raw: string; name: string };
export const getFeiqEmoji = (id: string): FeiqEmojiDef | undefined => byId.get(id);

export function emojiWireText(id: string): string {
  return byId.get(id)?.code ?? (getEmoji(id) ? buildEmojiMessage(id) : `[表情:${id}]`);
}

export function parseEmoticons(content: string): EmoticonToken[] {
  const parts: EmoticonToken[] = [];
  const text = (value: string) => {
    if (!value) return;
    const last = parts[parts.length - 1];
    if (last?.type === 'text') last.text += value; else parts.push({ type: 'text', text: value });
  };
  let end = 0;
  for (const match of content.matchAll(new RegExp(tokenPattern, 'g'))) {
    text(content.slice(end, match.index));
    const raw = match[0], feiq = byCode.get(raw), legacyId = parseEmojiId(raw);
    if (feiq) parts.push({ type: 'emoji', id: feiq.id, raw, name: feiq.name });
    else if (legacyId && getEmoji(legacyId)) parts.push({ type: 'emoji', id: legacyId, raw, name: '表情' });
    else text(raw); // Unknown legacy IDs and URLs stay as literal text.
    end = match.index! + raw.length;
  }
  text(content.slice(end));
  return parts;
}

export function emoticonPreview(content: string): string {
  return parseEmoticons(content).map((part) => part.type === 'text' ? part.text : `[${part.name}]`).join('');
}
