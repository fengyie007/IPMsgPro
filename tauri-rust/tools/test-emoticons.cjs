// Pure codec/editor-tree and React server-render tests. No browser, clipboard,
// native notification, LAN socket or user database is accessed.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const assert = require('node:assert/strict'), ts = require('typescript');
const React = require('react'), { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '../src'), cache = new Map();
let selection = null, ipc;
class Element {
  constructor(tag, text) { this.nodeType = text === undefined ? 1 : 3; this.tagName = tag?.toUpperCase(); this.value = text; this.dataset = {}; this.style = {}; this.childNodes = []; }
  append(...nodes) { this.childNodes.push(...nodes); }
  replaceChildren(...nodes) { this.childNodes = nodes; }
  contains(node) { return node === this || this.childNodes.some(child => child.contains(node)); }
  get textContent() { return this.nodeType === 3 ? this.value : this.childNodes.map(child => child.textContent).join(''); }
  querySelector(selector) { assert.equal(selector, '[data-emoji-id]'); for (const child of this.childNodes) { if (child.dataset.emojiId) return child; const found = child.querySelector(selector); if (found) return found; } return null; }
}
const document = { createElement: tag => new Element(tag), createTextNode: text => new Element(undefined, text), hasFocus: () => false, visibilityState: 'hidden' };
function load(filename) {
  if (cache.has(filename)) return cache.get(filename);
  const exports = {}; cache.set(filename, exports);
  const compiled = ts.transpileModule(fs.readFileSync(filename, 'utf8'), { fileName: filename,
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true } }).outputText;
  vm.runInNewContext(compiled, { exports, console, document, window: { getSelection: () => selection }, Node: { TEXT_NODE: 3, ELEMENT_NODE: 1 }, require(name) {
    if (name === '../services/bridge') return { invoke: (...args) => ipc(...args), listen: () => () => {} };
    if (name === './userStore') return { useUserStore: { getState: () => ({ users: [], addUser() {} }) } };
    if (name === './toastStore') return { toast: { error() {} } };
    if (!name.startsWith('.')) return require(name);
    const target = path.resolve(path.dirname(filename), name);
    const asset = target.split('?')[0];
    if (/\.(gif|png)$/.test(asset)) { assert(fs.existsSync(asset)); return '/assets/' + path.basename(asset); }
    return load(fs.existsSync(target + '.ts') ? target + '.ts' : target + '.tsx');
  } }, { filename });
  return exports;
}
const { FEIQ_EMOJIS } = load(path.join(root, 'feiqEmojiData.ts'));
const legacy = load(path.join(root, 'emojiData.ts'));
const codec = load(path.join(root, 'utils/emoticons.ts'));
const editor = load(path.join(root, 'utils/chatEditor.ts'));
const { formatPreview } = load(path.join(root, 'utils/format.ts'));
const MessageText = load(path.join(root, 'components/MessageText.tsx')).default;
const json = value => JSON.parse(JSON.stringify(value));

assert.equal(FEIQ_EMOJIS.length, 96);
assert.equal(new Set(FEIQ_EMOJIS.map(emoji => emoji.id)).size, 96);
assert.equal(new Set(FEIQ_EMOJIS.map(emoji => emoji.code)).size, 96);
for (const [i, emoji] of FEIQ_EMOJIS.entries()) {
  assert.equal(emoji.id, `feiq:${i + 1}`); assert.equal(emoji.file, `${i + 1}.gif`);
  const bytes = fs.readFileSync(path.join(root, 'assets/feiq', emoji.file));
  assert(['GIF87a', 'GIF89a'].includes(bytes.subarray(0, 6).toString('ascii')));
  assert.equal(bytes.readUInt16LE(6), emoji.width); assert.equal(bytes.readUInt16LE(8), emoji.height);
  assert.equal(codec.emojiWireText(emoji.id), emoji.code);
  const parsed = codec.parseEmoticons(emoji.code);
  assert.equal(parsed.length, 1); assert.equal(parsed[0].id, emoji.id);
}
assert.equal(FEIQ_EMOJIS[0].code, '/:)'); assert.equal(FEIQ_EMOJIS[0].name, '微笑');
assert.equal(FEIQ_EMOJIS[4].code, '/8-)'); assert.equal(FEIQ_EMOJIS[13].code, '/:D');
assert.equal(FEIQ_EMOJIS[70].code, '/:heart'); assert.equal(FEIQ_EMOJIS[84].code, '/:strong');
console.log('PASS all 96 codes map uniquely to the bundled original GIFs');

const combined = FEIQ_EMOJIS.map(emoji => emoji.code).join('');
assert.equal(codec.parseEmoticons(combined).length, 96);
const mixed = '你好/:)/:D\n/<rotate>/:heart /:not_registered /:d';
const tokens = codec.parseEmoticons(mixed);
assert.equal(tokens.filter(token => token.type === 'emoji').length, 4);
assert.equal(tokens.map(token => token.type === 'text' ? token.text : token.raw).join(''), mixed);
assert.deepEqual(json(codec.parseEmoticons(mixed)), json(tokens));
assert.equal(codec.parseEmoticons('https://example.test/:)')[0].type, 'text');
assert.equal(codec.parseEmoticons('https://example.test/:) /:)').filter(token => token.type === 'emoji').length, 1);
console.log('PASS mixed/adjacent tokens, punctuation, case sensitivity, unknown codes and URLs');

const oldId = legacy.EMOJIS[0].id;
const oldText = legacy.buildEmojiMessage(oldId);
assert.equal(codec.parseEmoticons(oldText)[0].id, oldId);
assert.equal(codec.emojiWireText(oldId), oldText);
const unknownXml = '<msg><emoji type="1" id="not-a-real-id" /></msg>';
assert.deepEqual(json(codec.parseEmoticons(unknownXml)), [{ type: 'text', text: unknownXml }]);
assert.equal(codec.parseEmoticons(oldText + '/:)').filter(token => token.type === 'emoji').length, 2);
console.log('PASS existing XML history remains readable and unknown IDs remain literal text');

const markup = renderToStaticMarkup(React.createElement(MessageText, { content: '<script>alert(1)</script>/:)\n/<rotate>' + oldText }));
assert(!markup.includes('<script>')); assert(markup.includes('&lt;script&gt;'));
assert(markup.includes('src="/assets/1.gif"')); assert(markup.includes('src="/assets/16.gif"'));
assert(markup.includes('alt="/:)"')); assert(markup.includes('data-emoji-id="' + oldId + '"'));
assert(markup.includes('background-image'));
console.log('PASS React renders GIFs and legacy sprites while escaping peer-provided markup');

const original = document.createElement('div');
original.append(document.createTextNode('你好'), editor.createEmojiElement('feiq:1'));
const line = document.createElement('div'); line.append(document.createTextNode('第二行'), editor.createEmojiElement('feiq:16'));
original.append(line);
const parts = editor.readEditor(original), wire = editor.draftText(parts);
assert.equal(wire, '你好/:)\n第二行/<rotate>\n');
const restored = document.createElement('div'); editor.restoreEditor(restored, parts);
assert.deepEqual(json(editor.readEditor(restored)), json(parts));
assert.equal(restored.childNodes[1].contentEditable, 'false');
assert.equal(restored.childNodes[1].childNodes[0].src, '/assets/1.gif');
assert.equal(restored.childNodes[1].childNodes[0].draggable, false);
const unknown = editor.createEmojiElement('not-real'); assert.equal(unknown.nodeType, 3);
console.log('PASS text/emoji drafts round-trip with atomic elements and no arbitrary HTML restoration');

const writes = [], clipboard = { setData: (type, value) => writes.push({ type, value }) };
const range = { collapsed: false, startContainer: original, endContainer: original, cloneContents: () => restored };
selection = { rangeCount: 1, getRangeAt: () => range };
assert.equal(editor.copyEditorSelection(original, clipboard), range);
assert.deepEqual(writes, [{ type: 'text/plain', value: wire }]);
writes.length = 0;
range.endContainer = document.createElement('div');
assert.equal(editor.copyEditorSelection(original, clipboard), null); assert.equal(writes.length, 0);
range.endContainer = original; range.collapsed = true;
assert.equal(editor.copyEditorSelection(original, clipboard), null);
range.collapsed = false;
const plain = document.createElement('div'); plain.append(document.createTextNode('纯文本'));
range.cloneContents = () => plain;
assert.equal(editor.copyEditorSelection(original, clipboard), null);
console.log('PASS copying managed emoji yields protocol text and leaves other selections untouched');

assert.equal(formatPreview({ type: 'text', content: '你好/:)\n/:heart' + oldText }), '你好[微笑] [爱心][表情]');
assert.equal(formatPreview({ type: 'image', content: '/:)' }), '[图片]');
console.log('PASS conversation previews show meaningful emoji names without changing stored text');

(async () => {
  const store = load(path.join(root, 'stores/messageStore.ts')).useMessageStore;
  ipc = async (command, args) => {
    assert.equal(command, 'message.send'); assert.equal(args.target, 'peer'); assert.equal(args.content, wire);
    return { success: true, messageId: 'feiq-emoji-send' };
  };
  assert.equal(await store.getState().sendMessage('peer', wire), true);
  assert.equal(store.getState().messages.get('peer')[0].type, 'text');
  assert.equal(store.getState().messages.get('peer')[0].content, wire);
  console.log('PASS emoji messages use the existing text IPC and retain raw symbols in message history');
})().catch(error => { console.error(error); process.exitCode = 1; });
