// Unit-test the real message store with an injected IPC boundary; no GUI/network.
// Default: use this project's installed dependencies. For an offline checkout:
// node tools/test-store.cjs ../frontend/package.json  (path relative to your cwd)
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const { createRequire } = require('node:module');
const deps = createRequire(path.resolve(process.argv[2] || path.join(__dirname, '../package.json')));
const ts = deps('typescript');
const zustand = deps('zustand');
let invoke = async () => { throw new Error('Unexpected IPC call'); };
const sourcePath = path.join(__dirname, '../src/stores/messageStore.ts');
const compiled = ts.transpileModule(fs.readFileSync(sourcePath, 'utf8'), {
  fileName: sourcePath,
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
}).outputText;
const exported = {};
const context = {
  exports: exported,
  console,
  document: { visibilityState: 'hidden', hasFocus: () => false },
  require(name) {
    if (name === 'zustand') return zustand;
    if (name === '../services/bridge') return { invoke: (...args) => invoke(...args), listen: () => () => {} };
    if (name === './userStore') return { useUserStore: { getState: () => ({ users: [], addUser() {} }) } };
    if (name === './toastStore') return { toast: { error() {} } };
    throw new Error(`Unexpected import ${name}`);
  },
};
vm.runInNewContext(compiled, context, { filename: sourcePath });
const store = exported.useMessageStore;
const incoming = (id, from = 'peer') => ({ id, from, to: 'self', content: id, type: 'text', timestamp: Date.now(), status: 'delivered' });
(async () => {
  invoke = async (command) => {
    assert.equal(command, 'message.send');
    store.getState().updateDelivery('early-ack', 'delivered');
    return { success: true, messageId: 'early-ack' };
  };
  assert.equal(await store.getState().sendMessage('peer', 'hello'), true);
  assert.equal(store.getState().messages.get('peer')[0].status, 'delivered');
  console.log('PASS early ACK before invoke result');

  store.setState({ messages: new Map(), unread: new Map() });
  store.getState().recvMessage(incoming('clear-A'));
  let finishClear;
  invoke = (command) => {
    assert.equal(command, 'history.clear');
    return new Promise((resolve) => { finishClear = resolve; });
  };
  const clearing = store.getState().clearHistory('peer');
  store.getState().recvMessage(incoming('clear-B')); // committed before server delete
  store.getState().recvMessage(incoming('keep-C')); // committed after server delete
  finishClear({ success: true, deletedIds: ['clear-A', 'clear-B'] });
  await clearing;
  assert.equal(store.getState().messages.get('peer').length, 1);
  assert.equal(store.getState().messages.get('peer')[0].id, 'keep-C');
  assert.equal(store.getState().unread.get('peer'), 1);
  store.getState().recvMessage(incoming('clear-B')); // delayed already-deleted event
  assert.equal(store.getState().messages.get('peer').length, 1);
  console.log('PASS exact clear IDs, late events and surviving unread count');

  invoke = async (command, args) => {
    assert.equal(command, 'history.search');
    assert.equal(args.userId, 'peer');
    return { success: true, messages: [], localUserId: 'me' };
  };
  await store.getState().searchMessages('needle', 'peer');
  console.log('PASS search scope sent to backend');

  invoke = async () => { throw new Error('database unavailable'); };
  await assert.rejects(store.getState().clearHistory('peer'));
  assert.equal(store.getState().messages.get('peer')[0].id, 'keep-C');
  console.log('PASS failed clear preserves messages');
})().catch((error) => { console.error(error); process.exitCode = 1; });
