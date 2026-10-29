import { emojiStyle, getEmoji } from '../emojiData';
import { FEIQ_EMOJI_URLS } from '../feiqEmojiAssets';
import { emojiWireText, getFeiqEmoji } from './emoticons';

export type DraftPart = { text: string } | { emoji: string };
const BLOCK_TAG = /^(DIV|P|LI|TR|PRE|BLOCKQUOTE|H[1-6])$/;

export function readEditor(root: ParentNode): DraftPart[] {
  const parts: DraftPart[] = [];
  const text = (value: string) => {
    if (!value) return;
    const last = parts[parts.length - 1];
    if (last && 'text' in last) last.text += value; else parts.push({ text: value });
  };
  const lineBreak = () => {
    const last = parts[parts.length - 1];
    if (last && !('text' in last && last.text.endsWith('\n'))) text('\n');
  };
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) { text(node.textContent || ''); return; }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const element = node as HTMLElement;
    const id = element.dataset.emojiId;
    if (id && (getFeiqEmoji(id) || getEmoji(id))) { parts.push({ emoji: id }); return; }
    if (element.tagName === 'BR') { text('\n'); return; }
    const block = BLOCK_TAG.test(element.tagName);
    if (block) lineBreak();
    element.childNodes.forEach(walk);
    if (block) lineBreak();
  };
  root.childNodes.forEach(walk);
  return parts;
}

export function draftText(parts: DraftPart[]): string {
  return parts.map((part) => 'text' in part ? part.text : emojiWireText(part.emoji)).join('');
}

export function createEmojiElement(id: string): Node {
  const feiq = getFeiqEmoji(id);
  if (!feiq && !getEmoji(id)) return document.createTextNode(emojiWireText(id));
  const span = document.createElement('span');
  span.dataset.emojiId = id; span.contentEditable = 'false';
  if (feiq) {
    span.title = `${feiq.name} ${feiq.code}`;
    span.style.display = 'inline-block'; span.style.verticalAlign = 'middle';
    const image = document.createElement('img');
    image.src = FEIQ_EMOJI_URLS[id]; image.alt = feiq.code; image.draggable = false;
    image.width = 24; image.height = 24; image.style.objectFit = 'contain';
    image.dataset.emojiId = id;
    span.append(image);
  } else {
    Object.assign(span.style, emojiStyle(id, 18));
  }
  return span;
}

export function restoreEditor(root: HTMLElement, parts: DraftPart[]): void {
  // Rebuild only text and our own emoji elements; never restore arbitrary HTML.
  root.replaceChildren(...parts.map((part) => {
    if ('text' in part) return document.createTextNode(part.text);
    return createEmojiElement(part.emoji);
  }));
}

// Copy the protocol text rather than empty sprite spans, object-replacement
// characters, or an installed-machine image path. Leave unrelated selections alone.
export function copyEditorSelection(root: HTMLElement, clipboard: DataTransfer): Range | null {
  const selection = window.getSelection();
  if (!selection?.rangeCount) return null;
  const range = selection.getRangeAt(0);
  if (range.collapsed || !root.contains(range.startContainer) || !root.contains(range.endContainer)) return null;
  const fragment = range.cloneContents();
  if (!fragment.querySelector('[data-emoji-id]')) return null;
  clipboard.setData('text/plain', draftText(readEditor(fragment)));
  return range;
}
