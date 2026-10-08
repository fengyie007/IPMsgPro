// Real validation/stores/explicit mock adapter, with only IPC and timers replaced.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm'), assert = require('node:assert/strict');
const ts = require('typescript'), zustand = require('zustand');
function load(relative, dependencies = {}, globals = {}, transform = (text) => text) {
  const filename = path.join(__dirname, '..', relative), exported = {};
  const compiled = ts.transpileModule(transform(fs.readFileSync(filename, 'utf8')), {
    fileName: filename, compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  }).outputText;
  vm.runInNewContext(compiled, { exports: exported, console, ...globals, require(name) {
    if (name in dependencies) return dependencies[name];
    throw new Error(`Unexpected import ${name}`);
  } }, { filename });
  return exported;
}
const types = load('src/types/index.ts');
const validation = load('src/utils/netValidation.ts');
for (const [input, expected] of [
  ['10.8.33.1-254', '10.8.33.1-10.8.33.254'], ['10.8.33.9/24', '10.8.33.1-10.8.33.254'],
  ['10.8.33.0/31', '10.8.33.0-10.8.33.1'], ['127.0.0.1/32', '127.0.0.1-127.0.0.1'],
  ['127.0.0.1', '127.0.0.1-127.0.0.1'], ['10.0.0.255-10.0.1.0', '10.0.0.255-10.0.1.0'],
]) assert.equal(validation.normalizeScanRange(input).value, expected);
for (const input of ['', '10.0.0.2-1', '10.0.0.1-256', '10.0.0.1/33', '10.0.0.1/', '10.0.0.1/-1',
  '0.0.0.1', '224.0.0.1', '255.255.255.255', '10.0.0.0/15', '01.2.3.4', '10.0.0.1-2-3']) assert.ok(validation.normalizeScanRange(input).error, input);
console.log('PASS scan range/CIDR normalization and unicast boundaries');
assert.equal(validation.validateScanOptions(['10.0.0.1-5', '10.0.0.3-7', '10.0.0.7-9'], 2425, 20).value.total, 9);
assert.equal(validation.validateScanOptions(['10.0.0.0-10.0.255.255'], 65535, 1000).value.total, 65536);
assert.ok(validation.validateScanOptions(['10.0.0.0-10.0.255.255', '10.1.0.1'], 2425, 10).error);
assert.ok(validation.validateScanOptions(Array(33).fill('127.0.0.1'), 2425, 10).error);
for (const [port, delay] of [[0, 20], [65536, 20], [2425, 9], [2425, 1001], [2.5, 20]]) assert.ok(validation.validateScanOptions([], port, delay).error);
console.log('PASS merged address quota, range count, port and pacing validation');

let invoke;
const listeners = new Map(), errors = [];
const bridge = { invoke: (...args) => invoke(...args), listen(event, callback) {
  const callbacks = listeners.get(event) || new Set(); callbacks.add(callback); listeners.set(event, callbacks);
  return () => { callbacks.delete(callback); if (!callbacks.size) listeners.delete(event); };
} };
function emit(event, data) { for (const callback of listeners.get(event) || []) callback(data); }
const dependencies = { zustand, '../services/bridge': bridge, '../types': types, '../utils/netValidation': validation,
  './toastStore': { toast: { error: (message) => errors.push(message) } } };
const store = load('src/stores/scanStore.ts', dependencies).useScanStore;
const configStore = load('src/stores/configStore.ts', dependencies).useConfigStore;
const snapshot = (scanId, revision, state = 'running', current = 0) => ({ ...types.EMPTY_SCAN, scanId, revision, state, current, total: 10, ranges: ['127.0.0.1-127.0.0.10'] });

(async () => {
  let saved = { nickname: 'legacy', group: '', directUsers: [], minimizeBehavior: 'tray', ipScanRanges: [] }, writes = 0;
  invoke = async (command, args) => {
    if (command === 'app.info') return { success: true, dataDir: 'test-only', port: 2427 };
    if (command === 'config.set') { ++writes; saved = { ...saved, ...args }; }
    return { success: true, config: saved };
  };
  await configStore.getState().loadConfig();
  assert.equal(configStore.getState().config.scanPort, 2425);
  assert.equal(configStore.getState().config.scanOnStartup, true);
  await configStore.getState().saveConfig({ ipScanRanges: ['10.8.33.1-254'], scanPort: 2426, scanDelayMs: 40, scanOnStartup: false });
  assert.equal(writes, 1); assert.equal(saved.scanPort, 2426); assert.equal(saved.scanOnStartup, false);
  await configStore.getState().resetConfig();
  assert.equal(writes, 2); assert.equal(saved.ipScanRanges.length, 0); assert.equal(saved.scanDelayMs, 20);
  console.log('PASS legacy configuration defaults, single config.set and scan reset');

  const stop = store.getState().initListeners();
  invoke = async (command) => {
    assert.equal(command, 'network.scan_range');
    emit('network.scan_complete', snapshot(1, 5, 'completed', 10));
    return { scan: snapshot(1, 1) };
  };
  assert.equal(await store.getState().start({ ranges: ['127.0.0.1-10'], port: 2425, delayMs: 20 }), true);
  emit('network.scan_progress', snapshot(1, 99, 'running', 1));
  assert.equal(store.getState().status.state, 'completed'); assert.equal(store.getState().status.revision, 5);
  console.log('PASS early completion survives delayed start response and late progress');

  let resolvePoll;
  invoke = () => new Promise((resolve) => { resolvePoll = resolve; });
  const polling = store.getState().refresh();
  emit('network.scan_progress', snapshot(2, 2, 'running', 2));
  resolvePoll({ scan: snapshot(1, 100, 'completed', 10) }); await polling;
  emit('network.scan_progress', snapshot(2, 1, 'running', 0));
  assert.equal(store.getState().status.scanId, 2); assert.equal(store.getState().status.current, 2);
  console.log('PASS old status queries and out-of-order revisions cannot replace a newer scan');

  invoke = async (command, args) => {
    assert.equal(command, 'network.scan_cancel'); assert.equal(args.scanId, 2);
    emit('network.scan_complete', snapshot(2, 4, 'cancelled', 2));
    return { scan: snapshot(2, 3, 'cancelling', 2) };
  };
  await store.getState().cancel(); assert.equal(store.getState().status.state, 'cancelled');
  let resolveStart, count = 0;
  invoke = () => { ++count; return new Promise((resolve) => { resolveStart = resolve; }); };
  const starting = store.getState().start({ ranges: ['127.0.0.1'], port: 2425, delayMs: 20 });
  assert.equal(await store.getState().start({ ranges: ['127.0.0.1'], port: 2425, delayMs: 20 }), false);
  resolveStart({ scan: snapshot(3, 1) }); await starting; assert.equal(count, 1);
  emit('network.scan_complete', snapshot(2, 20, 'cancelled', 2));
  assert.equal(store.getState().status.scanId, 3); assert.equal(store.getState().status.state, 'running');
  console.log('PASS cancellation targets its scan ID and repeated start clicks are gated');

  invoke = async () => { throw new Error('connection lost'); };
  await store.getState().refresh();
  assert.equal(store.getState().status.state, 'running'); assert.ok(store.getState().error.includes('connection lost'));
  emit('network.scan_progress', snapshot(3, 2, 'running', 50));
  assert.equal(store.getState().status.current, 0);
  emit('network.scan_progress', snapshot(3, 3, 'running', 3));
  assert.equal(store.getState().error, null);
  stop(); assert.equal(listeners.size, 0);
  console.log('PASS failed polls and malformed events do not fabricate completion; listeners clean up');

  const timers = new Map(); let timerId = 0;
  const mock = load('src/services/bridge.ts', {
    '@tauri-apps/api/core': { isTauri: () => false, invoke: () => { throw new Error('Unexpected native IPC'); } },
    '@tauri-apps/api/event': { listen: () => { throw new Error('Unexpected native listener'); } },
    '../types': types, '../utils/netValidation': validation,
  }, { setInterval: (callback) => { timers.set(++timerId, callback); return timerId; }, clearInterval: (id) => timers.delete(id), setTimeout, clearTimeout },
  (source) => source.replaceAll('import.meta.hot', 'undefined').replaceAll('import.meta.env.VITE_MOCK_BRIDGE', "'1'"));
  let result = await mock.invoke('network.scan_range', { ranges: ['127.0.0.1'], port: 2425, delayMs: 20 });
  await assert.rejects(mock.invoke('network.scan_cancel', { scanId: result.scan.scanId + 1 }));
  await mock.invoke('network.scan_cancel', { scanId: result.scan.scanId }); assert.equal(timers.size, 0);
  await mock.invoke('network.scan_range', { ranges: ['127.0.0.1'], port: 2425, delayMs: 20 });
  for (let n = 0; n < 3; n++) for (const callback of [...timers.values()]) callback();
  result = await mock.invoke('network.scan_status'); assert.equal(result.scan.state, 'completed'); assert.equal(result.scan.found, 1); assert.equal(timers.size, 0);
  console.log('PASS explicit browser mock scan start/status/cancel and bounded timer lifecycle');
})().catch((error) => { console.error(error); process.exitCode = 1; });
