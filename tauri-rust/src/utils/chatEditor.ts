import { buildEmojiMessage, emojiStyle } from '../emojiData';

export type DraftPart = { text: string } | { emoji: string };
const BLOCK_TAG = /^(DIV|P|LI|TR|PRE|BLOCKQUOTE|H[1-6])$/;

export function readEditor(root: HTMLElement): DraftPart[] {
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
    if (element.dataset.emojiId) { parts.push({ emoji: element.dataset.emojiId }); return; }
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
  return parts.map((part) => 'text' in part ? part.text : buildEmojiMessage(part.emoji)).join('');
}

export function restoreEditor(root: HTMLElement, parts: DraftPart[]): void {
  // Rebuild only text and our own emoji elements; never restore arbitrary HTML.
  root.replaceChildren(...parts.map((part) => {
    if ('text' in part) return document.createTextNode(part.text);
    const span = document.createElement('span');
    span.dataset.emojiId = part.emoji; span.contentEditable = 'false';
    Object.assign(span.style, emojiStyle(part.emoji, 18));
    return span;
  }));
}
