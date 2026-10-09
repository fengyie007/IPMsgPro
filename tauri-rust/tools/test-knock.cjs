// Real store/config/effect code with synthetic IPC, clock and DOM. No GUI or LAN.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const assert = require('node:assert/strict'), ts = require('typescript');
const React = require('react'), { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '../src'), modules = new Map(), listeners = new Map(), timers = new Map();
let now = 1700000000000, nextTimer = 0, ipc, focused = true;
class Clock extends Date { static now() { return now; } }
const events = () => ({ callbacks: new Map(), addEventListener(name, fn) { this.callbacks.set(name, fn); },
  removeEventListener(name, fn) { if (this.callbacks.get(name) === fn) this.callbacks.delete(name); } });
const document = { ...events(), visibilityState: 'visible', hasFocus: () => focused };
const window = { ...events() };
const listen = (name, fn) => {
  const handlers = listeners.get(name) || new Set(); handlers.add(fn); listeners.set(name, handlers);
  return () => { handlers.delete(fn); if (!handlers.size) listeners.delete(name); };
};
const emit = (name, data) => { for (const fn of listeners.get(name) || []) fn(data); };
const fail = () => { throw new Error('Unexpected native operation'); };
function load(file) {
  if (modules.has(file)) return modules.get(file);
  const exports = {}; modules.set(file, exports);
  const code = ts.transpileModule(fs.readFileSync(file, 'utf8').replaceAll('import.meta.hot', 'undefined')
    .replaceAll('import.meta.env.VITE_MOCK_BRIDGE', "'1'"), { fileName: file,
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true } }).outputText;
  vm.runInNewContext(code, { exports, console, Date: Clock, document, window,
    setTimeout: fn => { timers.set(++nextTimer, fn); return nextTimer; }, clearTimeout: id => timers.delete(id),
    setInterval: fail, clearInterval: fail, require(name) {
      if (name === '../services/bridge' || name === './bridge') return { invoke: (...args) => ipc(...args), listen };
      if (name === './userStore') return { useUserStore: { getState: () => ({ users: [], addUser() {} }) } };
      if (name.endsWith('/toastStore')) return { toast: { error() {} } };
      if (name === './FileBubble') return { default: () => { throw new Error('Knock rendered as file'); } };
      if (name === './MessageText') return { default: () => { throw new Error('Knock rendered as text'); } };
      if (name === './emoticons') return { emoticonPreview: text => text };
      if (name === '@tauri-apps/api/core') return { isTauri: () => false, invoke: fail };
      if (name === '@tauri-apps/api/event') return { listen: fail };
      if (!name.startsWith('.')) return require(name);
      const target = path.resolve(path.dirname(file), name);
      return load(fs.existsSync(target + '.ts') ? target + '.ts' : fs.existsSync(target + '.tsx') ? target + '.tsx' : path.join(target, 'index.ts'));
    } }, { filename: file });
  return exports;
}
const from = file => load(path.join(root, file));
const { DEFAULT_CONFIG } = from('types/index.ts');
const config = from('stores/configStore.ts').useConfigStore;
const store = from('stores/messageStore.ts').useMessageStore;
const { KnockEffectGate, watchKnockEffects } = from('services/knockEffects.ts');
const { formatPreview } = from('utils/format.ts');
const MessageBubble = from('components/MessageBubble.tsx').default;
const event = (id, from = 'peer') => ({ id, from, content: '[窗口振动]', type: 'knock', timestamp: now / 1000 });
const row = (id, target = 'peer') => ({ id, fromId: 'me', toId: target, content: '[窗口振动]', type: 3, timestamp: now / 1000, status: 1 });

(async () => {
  let saved = { nickname: 'legacy', group: '', directUsers: [], minimizeBehavior: 'tray' };
  ipc = async (command, args) => {
    if (command === 'app.info') return { success: true, dataDir: 'fixture', port: 2427 };
    if (command === 'config.set') saved = { ...saved, ...args };
    return { success: true, config: saved };
  };
  await config.getState().loadConfig();
  for (const key of ['announceOnStartup', 'announceOnExit', 'knockEffects']) assert.equal(config.getState().config[key], true);
  await config.getState().saveConfig({ announceOnStartup: false, announceOnExit: false, knockEffects: false });
  assert.equal(config.getState().config.announceOnStartup, false); assert.equal(config.getState().config.announceOnExit, false);
  assert.equal(config.getState().config.knockEffects, false); assert.equal(config.getState().config.scanOnStartup, true);
  await config.getState().resetConfig();
  for (const key of ['announceOnStartup', 'announceOnExit', 'knockEffects']) {
    assert.equal(config.getState().config[key], true);
    for (const value of [1, 'true', null]) {
      const before = config.getState().config;
      ipc = async () => ({ success: true, config: { ...before, [key]: value } });
      await assert.rejects(config.getState().saveConfig({ [key]: false })); assert.equal(config.getState().config, before);
    }
  }
  console.log('PASS lifecycle/effect switches default on, save/reset independently and reject invalid responses without disabling scans');

  let calls = 0, resolve;
  ipc = (command, args) => {
    assert.equal(command, 'message.knock'); assert.equal(args.target, 'peer'); ++calls;
    return new Promise(r => { resolve = r; });
  };
  const pending = store.getState().sendKnock('peer');
  assert.equal(await store.getState().sendKnock('peer'), false); assert.equal(calls, 1);
  resolve({ success: true, message: row('sent') }); assert.equal(await pending, true);
  assert.equal(store.getState().messages.get('peer')[0].status, 'sent');
  assert.equal(store.getState().messages.get('peer')[0].type, 'knock');
  assert.equal(await store.getState().sendKnock('peer'), false); assert.equal(calls, 1);
  now += 10000;
  ipc = async () => ({ success: true, message: row('after-cooldown') });
  assert.equal(await store.getState().sendKnock('peer'), true);
  ipc = async () => { throw new Error('send failed'); };
  assert.equal(await store.getState().sendKnock('other'), false); assert.equal(store.getState().knockPending.size, 0);
  assert(!store.getState().messages.has('other'));
  console.log('PASS one IPC call per send, concurrent clicks/cooldown blocked, failures remain failures and success is sent rather than delivered');

  const stopMessages = store.getState().initListeners();
  focused = false;
  emit('message.received', event('received')); emit('message.received', event('received'));
  assert.equal(store.getState().unread.get('peer'), 1);
  assert.equal(store.getState().messages.get('peer').filter(m => m.id === 'received').length, 1);
  const messages = [row('history'), { ...row('incoming-history'), fromId: 'peer', toId: 'me' }];
  ipc = async () => ({ success: true, localUserId: 'me', messages });
  await store.getState().loadHistory('peer');
  const history = store.getState().messages.get('peer').find(m => m.id === 'history');
  assert.equal(history.type, 'knock'); assert.equal(history.status, 'sent');
  assert.equal(formatPreview(history), '[窗口振动]');
  assert.equal((await store.getState().searchMessages('振动', 'peer'))[0].type, 'knock');
  ipc = async () => ({ success: true, deletedIds: ['received'] });
  await store.getState().clearHistory('peer'); emit('message.received', event('received'));
  assert(!store.getState().messages.get('peer')?.some(m => m.id === 'received'));
  stopMessages(); focused = true;
  console.log('PASS receive dedupe/unread, history/search conversion and cleared-message protection retain the knock message type');

  const gate = new KnockEffectGate();
  assert(gate.accept(event('one'), now, true)); assert(!gate.accept(event('one'), now, true));
  now += 1100; assert(!gate.accept(event('rate'), now, true));
  assert(gate.accept(event('other', 'other'), now, true));
  now += 10000; assert(!gate.accept(event('muted'), now, false)); assert(!gate.accept(event('muted'), now, true));
  assert(gate.accept(event('later'), now, true));
  assert(!gate.accept({ ...event('old'), timestamp: now / 1000 - 20 }, now, true));
  assert(!gate.accept({ ...event('text'), type: 'text' }, now, true));
  console.log('PASS effect gate suppresses duplicate, muted, stale and excessive events without preventing later valid knocks');

  const classes = new Set(), panel = { classList: { add: name => classes.add(name), remove: name => classes.delete(name) } };
  const stopLiveMessages = store.getState().initListeners();
  const stop = watchKnockEffects('peer', panel);
  emit('message.received', event('effect')); assert(classes.has('knock-effect'));
  window.callbacks.get('blur')(); assert.equal(classes.size, 0); assert.equal(timers.size, 0);
  now += 10000; focused = false; emit('message.received', event('background')); assert.equal(classes.size, 0);
  focused = true; emit('message.received', event('background')); assert.equal(classes.size, 0);
  now += 10000; emit('message.received', event('effect2')); assert(classes.has('knock-effect'));
  config.setState({ config: { ...DEFAULT_CONFIG, knockEffects: false } }); assert.equal(classes.size, 0);
  config.setState({ config: { ...DEFAULT_CONFIG } });
  now += 10000; emit('message.received', event('foreign', 'other')); assert.equal(classes.size, 0);
  ipc = async () => ({ success: true, deletedIds: ['cleared-effect'] });
  await store.getState().clearHistory('peer');
  emit('message.received', event('cleared-effect')); assert.equal(classes.size, 0);
  emit('message.received', event('effect3')); assert(classes.has('knock-effect'));
  stop(); stopLiveMessages(); assert.equal(classes.size, 0); assert.equal(timers.size, 0); assert.equal(listeners.size, 0);
  assert.equal(window.callbacks.size, 0); assert.equal(document.callbacks.size, 0);
  console.log('PASS focus/visibility/setting gates and listener cleanup never replay background events or leave an animation running');

  const sentMarkup = renderToStaticMarkup(React.createElement(MessageBubble, { message: history }));
  assert(sentMarkup.includes('你发送了窗口振动')); assert(sentMarkup.includes('已发送')); assert(!sentMarkup.includes('对方已收到'));
  const receivedMarkup = renderToStaticMarkup(React.createElement(MessageBubble, { message: { ...history, from: 'peer' } }));
  assert(receivedMarkup.includes('对方发送了窗口振动'));
  console.log('PASS knock bubbles render as their own message and never claim remote delivery');

  const mock = from('services/bridge.ts');
  const result = await mock.invoke('message.knock', { target: 'demo@localhost' });
  assert.equal(result.message.type, 3); assert.equal(result.message.status, 1);
  await assert.rejects(mock.invoke('message.knock', { target: 'demo@localhost' }));
  for (const key of ['announceOnStartup', 'announceOnExit', 'knockEffects']) {
    await assert.rejects(mock.invoke('config.set', { [key]: 1 }));
    assert.equal((await mock.invoke('config.set', { [key]: false })).config[key], false);
  }
  console.log('PASS explicit browser mock allows the new command and validates cooldown and boolean configuration');
})().catch(error => { console.error(error); process.exitCode = 1; });
