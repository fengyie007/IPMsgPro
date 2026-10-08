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
const listeners = new Map();
const toastErrors = [];
function listen(event, callback) {
  const callbacks = listeners.get(event) || new Set();
  callbacks.add(callback); listeners.set(event, callbacks);
  return () => { callbacks.delete(callback); if (!callbacks.size) listeners.delete(event); };
}
function emit(event, payload) { for (const callback of listeners.get(event) || []) callback(payload); }
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
    if (name === '../services/bridge') return { invoke: (...args) => invoke(...args), listen };
    if (name === './userStore') return { useUserStore: { getState: () => ({ users: [], addUser() {} }) } };
    if (name === './toastStore') return { toast: { error(message) { toastErrors.push(message); } } };
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

  const stopListeners = store.getState().initListeners();
  const image = { assetId: 'received-asset-a', fileName: '截图.png', fileSize: 1234, mime: 'image/png', width: 394, height: 198 };
  const receivedImage = { id: 'image-message-a', from: 'image-peer', content: '[图片]', type: 'image', timestamp: 1700000000, image };
  emit('message.received', receivedImage);
  emit('message.received', receivedImage);
  assert.equal(store.getState().messages.get('image-peer').length, 1);
  assert.equal(store.getState().messages.get('image-peer')[0].image.assetId, image.assetId);
  assert.equal(store.getState().unread.get('image-peer'), 1);
  emit('message.received', { ...receivedImage, id: 'image-message-b', image: { ...image, assetId: 'received-asset-b' } });
  assert.equal(store.getState().messages.get('image-peer').length, 2);
  assert.equal(store.getState().unread.get('image-peer'), 2);
  console.log('PASS image metadata and stable-ID dedupe, equal-size images remain distinct');
  emit('message.received', { ...receivedImage, id: 'late-metadata', from: 'late-image-peer', image: undefined });
  emit('message.received', { ...receivedImage, id: 'late-metadata', from: 'late-image-peer' });
  assert.equal(store.getState().messages.get('late-image-peer')[0].image.assetId, image.assetId);
  assert.equal(store.getState().messages.get('late-image-peer').length, 1);
  assert.equal(store.getState().unread.get('late-image-peer'), 1);
  console.log('PASS late image metadata fills the same message without another unread');

  const historyImage = { id: 'history-image', fromId: 'history-peer', toId: 'me', content: '[图片]', type: 1, timestamp: 1700000001, status: 1, image };
  // A message from an older/stale source may omit metadata; history must fill it.
  store.getState().recvMessage({ ...incoming('history-image', 'history-peer'), type: 'image', image: undefined });
  invoke = async (command) => {
    assert.ok(['history.get', 'history.get_recent', 'history.search'].includes(command));
    return { success: true, messages: [historyImage], localUserId: 'me' };
  };
  await store.getState().loadHistory('history-peer');
  assert.equal(store.getState().messages.get('history-peer')[0].image.assetId, image.assetId);
  await store.getState().loadRecentConversations();
  assert.equal(store.getState().messages.get('history-peer').length, 1);
  assert.equal((await store.getState().searchMessages('图片', 'history-peer'))[0].image.fileName, image.fileName);
  console.log('PASS image metadata survives history, recent conversations and search');

  invoke = async (command) => {
    assert.equal(command, 'history.clear');
    return { success: true, deletedIds: ['image-message-a', 'image-message-b'] };
  };
  await store.getState().clearHistory('image-peer');
  emit('message.received', receivedImage);
  assert.equal(store.getState().messages.has('image-peer'), false);
  assert.equal(store.getState().unread.has('image-peer'), false);
  console.log('PASS cleared images cannot return through a delayed image event');

  const beforeMessages = store.getState().messages.size;
  emit('image.receive_failed', { imageId: 'bad-image', error: 'CRC校验失败' });
  assert.equal(store.getState().messages.size, beforeMessages);
  assert.ok(toastErrors.at(-1).includes('CRC校验失败'));
  invoke = async (command) => {
    assert.equal(command, 'image.send');
    emit('image.send_completed', { messageId: 'tx-early', target: 'tx-peer', progress: 100 });
    emit('image.send_progress', { messageId: 'tx-early', target: 'tx-peer', progress: 20, stage: 'transferring' });
    return { success: true, messageId: 'tx-early', imageId: '12345678', image };
  };
  assert.equal(await store.getState().sendImage('tx-peer', image.assetId), true);
  assert.equal(store.getState().messages.get('tx-peer')[0].status, 'delivered');
  assert.equal(store.getState().messages.get('tx-peer')[0].imageProgress, 100);
  emit('image.send_failed', { messageId: 'tx-early', target: 'tx-peer', error: 'late failure' });
  assert.equal(store.getState().messages.get('tx-peer')[0].status, 'delivered');
  assert.equal(store.getState().messages.get('tx-peer').length, 1);
  console.log('PASS early image completion wins over late progress/terminal events');

  invoke = async () => {
    emit('image.send_failed', { messageId: 'tx-failed', target: 'tx-peer', error: 'early failure' });
    return { success: true, messageId: 'tx-failed', imageId: '12345679', image };
  };
  await store.getState().sendImage('tx-peer', image.assetId);
  assert.equal(store.getState().messages.get('tx-peer').find((m) => m.id === 'tx-failed').status, 'failed');
  assert.ok(toastErrors.at(-1).includes('early failure'));
  console.log('PASS early image failure is replayed after message insertion');

  invoke = async (command) => command === 'image.send'
    ? { success: true, messageId: 'tx-cancel', imageId: '12345680', image }
    : { success: true, cancelled: true };
  await store.getState().sendImage('tx-peer', image.assetId);
  emit('message.ack', { messageId: 'tx-cancel' });
  assert.equal(store.getState().messages.get('tx-peer').find((m) => m.id === 'tx-cancel').status, 'sending');
  await store.getState().cancelImage('tx-cancel');
  emit('image.send_progress', { messageId: 'tx-cancel', target: 'tx-peer', progress: 90, stage: 'transferring' });
  assert.equal(store.getState().messages.get('tx-peer').find((m) => m.id === 'tx-cancel').imageStage, 'cancelling');
  const errorsBeforeCancel = toastErrors.length;
  emit('image.send_failed', { messageId: 'tx-cancel', target: 'tx-peer', cancelled: true, error: '图片发送已取消' });
  assert.equal(store.getState().messages.get('tx-peer').find((m) => m.id === 'tx-cancel').imageStage, 'cancelled');
  assert.equal(toastErrors.length, errorsBeforeCancel);
  console.log('PASS image cancellation does not become delivery or revert on late progress');

  invoke = async () => ({ success: true, deletedIds: ['tx-early', 'tx-failed', 'tx-cancel'] });
  await store.getState().clearHistory('tx-peer');
  emit('image.send_progress', { messageId: 'tx-cancel', target: 'tx-peer', progress: 99 });
  emit('image.send_completed', { messageId: 'tx-cancel', target: 'tx-peer', progress: 100 });
  assert.equal(store.getState().messages.has('tx-peer'), false);
  console.log('PASS late outbound image events do not resurrect cleared history');

  const file = { fileName: '中文.txt', fileSize: 100, state: 'offered', transferred: 0, incoming: false, hasLocalFile: false };
  invoke = async (command) => {
    assert.equal(command, 'file.send');
    emit('file.updated', { messageId: 'file-early', target: 'file-peer', file: { ...file, state: 'completed', transferred: 100 } });
    emit('file.updated', { messageId: 'file-early', target: 'file-peer', file: { ...file, state: 'transferring', transferred: 20 } });
    return { success: true, message: { id: 'file-early', fromId: 'local', toId: 'file-peer', type: 2, content: '[文件]', timestamp: 1, status: 0, file } };
  };
  await store.getState().sendFile('file-peer', 'selection');
  assert.equal(store.getState().messages.get('file-peer')[0].file.state, 'completed');
  assert.equal(store.getState().messages.get('file-peer')[0].file.transferred, 100);
  emit('file.updated', { messageId: 'file-early', target: 'file-peer', file: { ...file, state: 'failed' } });
  assert.equal(store.getState().messages.get('file-peer')[0].file.state, 'completed');
  console.log('PASS file completion before invoke and late progress cannot reverse terminal status');

  emit('message.received', { id: 'file-receive', from: 'file-peer', type: 'file', content: '[文件]', timestamp: 2, file: { ...file, incoming: true } });
  emit('file.updated', { messageId: 'file-receive', target: 'file-peer', file: { ...file, incoming: true, state: 'transferring', transferred: 60 } });
  emit('file.updated', { messageId: 'file-receive', target: 'file-peer', file: { ...file, incoming: true, state: 'transferring', transferred: 20 } });
  assert.equal(store.getState().messages.get('file-peer')[1].file.transferred, 60);
  emit('file.updated', { messageId: 'file-receive', target: 'file-peer', file: { ...file, incoming: true, state: 'cancelled' } });
  emit('file.updated', { messageId: 'file-receive', target: 'file-peer', file: { ...file, incoming: true, state: 'completed' } });
  assert.equal(store.getState().messages.get('file-peer')[1].file.state, 'cancelled');
  console.log('PASS file receive progress is monotonic and cancelled transfer stays cancelled');

  invoke = async () => ({ success: true, localUserId: 'local', messages: [{ id: 'file-history', fromId: 'file-peer', toId: 'local', type: 2, content: '[文件]', timestamp: 3, status: 2, file: { ...file, incoming: true, state: 'completed', hasLocalFile: true, transferred: 100 } }] });
  await store.getState().loadHistory('file-peer');
  assert.equal(store.getState().messages.get('file-peer').find((m) => m.id === 'file-history').file.hasLocalFile, true);
  console.log('PASS completed file metadata survives history reload');

  invoke = async () => ({ success: true, deletedIds: ['file-early', 'file-receive', 'file-history'] });
  await store.getState().clearHistory('file-peer');
  emit('file.updated', { messageId: 'file-receive', target: 'file-peer', file: { ...file, state: 'completed' } });
  emit('message.received', { id: 'file-receive', from: 'file-peer', type: 'file', timestamp: 2, file });
  assert.equal(store.getState().messages.has('file-peer'), false);
  console.log('PASS cleared file tasks cannot reappear through late events');

  stopListeners();
  assert.equal(listeners.size, 0);
  console.log('PASS receive failure is reported without a phantom chat message; listeners cleaned');
})().catch((error) => { console.error(error); process.exitCode = 1; });
