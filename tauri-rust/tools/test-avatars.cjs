// Actual stores/components with IPC and hook-state substitutes; no browser or LAN.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const assert = require('node:assert/strict'), ts = require('typescript');
const React = require('react'), { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '../src'), cache = new Map(), listeners = new Map();
let ipc, hookState = null, hookIndex = 0;
function load(filename) {
  if (cache.has(filename)) return cache.get(filename);
  const exports = {}; cache.set(filename, exports);
  const code = ts.transpileModule(fs.readFileSync(filename, 'utf8'), { fileName: filename,
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true } }).outputText;
  vm.runInNewContext(code, { exports, console, require(name) {
    if (name === '../services/bridge') return { invoke: (...args) => ipc(...args), listen: (name, fn) => { listeners.set(name, fn); return () => listeners.delete(name); } };
    if (name === './toastStore') return { toast: { error() {} } };
    if (name === 'react') return { ...React, useState(initial) {
      if (!hookState) return React.useState(initial);
      const state = hookState, index = hookIndex++;
      if (!(index in state)) state[index] = initial;
      return [state[index], value => { state[index] = value; }];
    } };
    if (name === '../stores/userStore') return { useUserStore: selector => selector(users.getState()) };
    if (!name.startsWith('.')) return require(name);
    const target = path.resolve(path.dirname(filename), name), asset = target.split('?')[0];
    if (asset.endsWith('.png')) { assert(fs.existsSync(asset)); return '/assets/' + path.basename(asset); }
    return load(fs.existsSync(target + '.ts') ? target + '.ts' : fs.existsSync(target + '.tsx') ? target + '.tsx' : path.join(target, 'index.ts'));
  } }, { filename });
  return exports;
}
const from = file => load(path.join(root, file));
const json = value => JSON.parse(JSON.stringify(value));
const { DEFAULT_CONFIG, AVATAR_COUNT } = from('types/index.ts');
const { validAvatarId, displayAvatar } = from('utils/avatar.ts');
const { FEIQ_AVATAR_URLS } = from('feiqAvatarAssets.ts');
const users = from('stores/userStore.ts').useUserStore;
const config = from('stores/configStore.ts').useConfigStore;
const { prepareSettings, settingsDirty } = from('utils/settingsDraft.ts');
const UserAvatar = from('components/UserAvatar.tsx').default;
const AvatarPicker = from('components/AvatarPicker.tsx').default;
const builtin = id => ({ kind: 'builtin', id });
const alice = { id: 'peer@host#127.0.0.1:2425', nickname: 'Alice', username: 'peer', status: 'online' };
function renderFunction(Component, props, state) {
  hookState = state; hookIndex = 0;
  try { return Component(props); } finally { hookState = null; }
}
function elements(tree) {
  if (Array.isArray(tree)) return tree.flatMap(elements);
  if (!React.isValidElement(tree)) return [];
  return [tree, ...elements(tree.props.children)];
}
function deferred() { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; }

(async () => {
  assert.equal(AVATAR_COUNT, 84); assert.equal(Object.keys(FEIQ_AVATAR_URLS).length, 84);
  for (let id = 0; id < 84; id++) {
    assert(validAvatarId(id)); assert.equal(FEIQ_AVATAR_URLS[id], `/assets/${id}.png`);
    const png = fs.readFileSync(path.join(root, 'assets/feiq-avatars', `${id}.png`));
    assert.equal(png.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
    assert(png.readUInt32BE(16) > 0); assert(png.readUInt32BE(20) > 0);
  }
  for (const id of [-1, 84, 1.5, '5', NaN, Infinity, null, undefined]) assert(!validAvatarId(id));
  console.log('PASS all 84 protocol IDs resolve to bundled PNGs; invalid IDs are rejected');

  const legacy = { ...DEFAULT_CONFIG }; delete legacy.avatarId;
  let saved = legacy, requests = [];
  ipc = async (command, args) => {
    requests.push([command, args]);
    if (command === 'app.info') return { success: true, dataDir: 'fixture', port: 2427 };
    if (command === 'config.set') saved = { ...saved, ...args };
    return { success: true, config: saved };
  };
  await config.getState().loadConfig(); assert.equal(config.getState().config.avatarId, 0);
  const draft = { ...config.getState().config, avatarId: 83 };
  assert(settingsDirty(config.getState().config, draft));
  const prepared = prepareSettings(draft, '', '');
  assert.equal(config.getState().config.avatarId, 0);
  assert.throws(() => prepareSettings({ ...draft, avatarId: 84 }, '', ''));
  requests = [];
  await config.getState().saveConfig(prepared);
  assert.equal(requests.length, 1); assert.equal(requests[0][0], 'config.set'); assert.equal(requests[0][1].avatarId, 83);
  assert.equal(config.getState().config.avatarId, 83);
  await config.getState().resetConfig(); assert.equal(config.getState().config.avatarId, 0);
  const previous = config.getState().config;
  ipc = async () => { throw new Error('write failed'); };
  await assert.rejects(config.getState().saveConfig({ avatarId: 7 }));
  for (const invalid of [84, -1, 1.5, '3', null]) {
    ipc = async () => ({ success: true, config: { ...previous, avatarId: invalid } });
    await assert.rejects(config.getState().saveConfig({ avatarId: 7 }));
    assert.equal(config.getState().config, previous);
  }
  console.log('PASS legacy config, draft/save/reset and failed or invalid saves preserve valid settings');

  const offline = 'offline@host#127.0.0.1:2425';
  ipc = async () => ({ users: [alice], count: 1, avatars: { [alice.id]: builtin(5), [offline]: builtin(83), invalid: builtin(84) } });
  await users.getState().loadUsers();
  assert.equal(users.getState().users.length, 1); assert(!users.getState().avatars.has('invalid'));
  assert.equal(users.getState().avatars.get(offline).id, 83);
  assert.equal(users.getState().users[0].avatar.id, 5);
  const markup = renderToStaticMarkup(React.createElement(UserAvatar, { user: { ...alice, id: offline, status: 'offline' } }));
  assert(markup.includes('src="/assets/83.png"'));
  const other = { ...alice, id: 'peer@host#127.0.0.1:2426', avatar: builtin(7) };
  users.getState().addUser(other); users.getState().addUser({ ...alice, avatar: undefined });
  assert.equal(users.getState().avatars.get(alice.id).id, 5);
  assert.equal(users.getState().avatars.get(other.id).id, 7);
  for (const avatar of [{ kind: 'custom' }, { kind: 'unknown' }]) {
    users.getState().addUser({ ...alice, avatar });
    assert.deepEqual(json(displayAvatar(avatar, builtin(5))), avatar);
    assert.equal(users.getState().avatars.get(alice.id).kind, avatar.kind);
  }
  console.log('PASS offline avatars use the endpoint cache without inventing online peers; profile markers replace old images');

  const stop = users.getState().initListeners();
  const pending = deferred(); ipc = () => pending.promise;
  const loading = users.getState().loadUsers();
  listeners.get('user.discovered')({ ...alice, avatar: builtin(12) });
  pending.resolve({ users: [{ ...alice, avatar: builtin(1) }], avatars: { [alice.id]: builtin(1) } });
  await loading;
  assert.equal(users.getState().users.find(u => u.id === alice.id).avatar.id, 12);
  assert.equal(users.getState().avatars.get(alice.id).id, 12);
  listeners.get('user.status_changed')({ user: { id: alice.id }, status: 'offline' });
  assert.equal(users.getState().avatars.get(alice.id).id, 12);
  const old = deferred(), newer = deferred(); let n = 0;
  ipc = () => (n++ === 0 ? old : newer).promise;
  const a = users.getState().loadUsers(), b = users.getState().loadUsers();
  newer.resolve({ users: [{ ...alice, avatar: builtin(33) }], avatars: {} }); await b;
  old.resolve({ users: [{ ...alice, avatar: builtin(4) }], avatars: {} }); await a;
  assert.equal(users.getState().avatars.get(alice.id).id, 33);
  stop(); assert.equal(listeners.size, 0);
  console.log('PASS discoveries and newer list responses win over stale snapshots; status changes retain avatars');

  for (const avatar of [{ kind: 'custom' }, { kind: 'unknown' }, builtin(84), { kind: 'url', url: 'https://example.test' }]) {
    const markup = renderToStaticMarkup(React.createElement(UserAvatar, { user: { ...alice, id: 'uncached', nickname: '<Alice>' }, avatar }));
    assert(!markup.includes('<img')); assert(!markup.includes('<Alice>')); assert(markup.includes('&lt;'));
  }
  assert.equal(displayAvatar(null, builtin(3)).id, 3);
  const state = [], props = { user: alice, avatar: builtin(0) };
  let element = renderFunction(UserAvatar, props, state);
  assert.equal(element.type, 'img'); element.props.onError();
  element = renderFunction(UserAvatar, props, state);
  assert.equal(element.type, 'span'); assert.equal(element.props.children, 'A');
  assert.equal(renderFunction(UserAvatar, { ...props, avatar: builtin(83) }, state).type, 'img');
  console.log('PASS unsupported/missing/broken images fall back safely and a different avatar can render again');

  const statePicker = [], changes = [], propsPicker = { value: 83, disabled: false, onChange: id => changes.push(id) };
  let tree = renderFunction(AvatarPicker, propsPicker, statePicker);
  const toggle = elements(tree).find(e => e.type === 'button');
  assert.equal(toggle.props['aria-expanded'], false); toggle.props.onClick();
  tree = renderFunction(AvatarPicker, propsPicker, statePicker);
  const options = elements(tree).filter(e => e.type === 'button' && 'aria-pressed' in e.props);
  assert.equal(options.length, 84); assert.equal(options.filter(e => e.props['aria-pressed']).length, 1);
  assert.equal(options[83].props['aria-pressed'], true); options[0].props.onClick();
  assert.deepEqual(changes, [0]); assert.equal(propsPicker.value, 83);
  const disabled = elements(renderFunction(AvatarPicker, { ...propsPicker, disabled: true }, statePicker)).filter(e => e.type === 'button');
  assert(disabled.every(e => e.props.disabled));
  console.log('PASS picker exposes all avatars, tracks selection, changes only the draft and disables edits while saving');
})().catch(error => { console.error(error); process.exitCode = 1; });
