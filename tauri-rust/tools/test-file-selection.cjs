const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm'), assert = require('node:assert/strict');
const ts = require('typescript'), zustand = require('zustand');
let invoke, sendFile;
const errors = [], exportsObject = {};
const source = path.join(__dirname, '../src/stores/fileSelectionStore.ts');
const compiled = ts.transpileModule(fs.readFileSync(source, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS } }).outputText;
vm.runInNewContext(compiled, { exports: exportsObject, console, require(name) {
  if (name === 'zustand') return zustand;
  if (name === '../services/bridge') return { invoke: (...args) => invoke(...args) };
  if (name === './messageStore') return { useMessageStore: { getState: () => ({ sendFile: (...args) => sendFile(...args), error: 'test error' }) } };
  if (name === './toastStore') return { toast: { error: (text) => errors.push(text), info() {} } };
  throw new Error(name);
} }, { filename: source });
const store = exportsObject.useFileSelectionStore;
const file = { selectionId: 'selected', fileName: '文件.txt', fileSize: 8 };
(async () => {
  let sends = [], discarded = [];
  invoke = async (name, args) => { if (name === 'file.discard') discarded.push(args.selectionId); return { success: true }; };
  sendFile = async (...args) => { sends.push(args); return true; };
  store.getState().picked('original', [file]);
  assert.equal(sends.length, 0);
  await store.getState().cancel();
  assert.deepEqual(discarded, ['selected']); assert.equal(sends.length, 0);
  console.log('PASS drop only previews; cancelling releases selection without sending');

  store.getState().picked('original', [file]);
  store.getState().picked('other', [{ ...file, selectionId: 'extra' }]);
  await Promise.resolve();
  assert.equal(store.getState().target, 'original'); assert.ok(discarded.includes('extra'));
  await store.getState().send();
  assert.deepEqual(sends, [['original', 'selected']]); assert.equal(store.getState().files.length, 0);
  console.log('PASS concurrent drops cannot replace confirmed recipient or existing preview');

  let finish;
  invoke = (name) => name === 'file.select' ? new Promise((resolve) => { finish = resolve; }) : Promise.resolve({ success: true });
  const picking = store.getState().select('picker-target');
  await store.getState().select('ignored');
  finish({ success: true, files: [file] }); await picking;
  assert.equal(store.getState().target, 'picker-target');
  let resolveSend;
  sendFile = () => new Promise((resolve) => { resolveSend = resolve; });
  const sending = store.getState().send(); await store.getState().send();
  resolveSend(true); await sending; assert.equal(store.getState().busy, false);
  console.log('PASS duplicate picker and send clicks are gated');

  store.getState().picked('failed-target', [file]);
  sendFile = async () => false;
  discarded = [];
  invoke = async (name, args) => { if (name === 'file.discard') discarded.push(args.selectionId); return { success: true }; };
  await store.getState().send();
  assert.deepEqual(discarded, ['selected']); assert.equal(store.getState().files.length, 0); assert.ok(errors.at(-1).includes('test error'));
  console.log('PASS rejected send reports failure and clears unused preview');
})().catch((error) => { console.error(error); process.exitCode = 1; });
