// No audio device is opened: validate actual configuration and preview IPC code.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm'), assert = require('node:assert/strict');
const ts = require('typescript'), zustand = require('zustand');
function load(relative, dependencies = {}, transform = (source) => source) {
  const filename = path.join(__dirname, '..', relative), exports = {};
  const compiled = ts.transpileModule(transform(fs.readFileSync(filename, 'utf8')), {
    fileName: filename, compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  }).outputText;
  vm.runInNewContext(compiled, { exports, console, setTimeout, clearTimeout, setInterval, clearInterval, require(name) {
    if (name in dependencies) return dependencies[name];
    throw new Error(`Unexpected import ${name}`);
  } }, { filename });
  return exports;
}
const types = load('src/types/index.ts'), validation = load('src/utils/netValidation.ts');
let invoke;
const config = load('src/stores/configStore.ts', { zustand, '../types': types, '../utils/netValidation': validation,
  '../services/bridge': { invoke: (...args) => invoke(...args) } }).useConfigStore;
const preview = load('src/services/notification.ts', { './bridge': { invoke: (...args) => invoke(...args) } }).previewNotificationSound;

(async () => {
  let current = { nickname: 'legacy', group: '', directUsers: [], minimizeBehavior: 'tray' }, writes = [];
  invoke = async (command, args) => {
    if (command === 'app.info') return { success: true, dataDir: 'test-only', port: 2427 };
    if (command === 'config.set') { writes.push(args); current = { ...current, ...args }; }
    return { success: true, config: current };
  };
  await config.getState().loadConfig(); assert.equal(config.getState().config.notificationSound, false);
  await config.getState().saveConfig({ notificationSound: true });
  assert.equal(writes.length, 1); assert.deepEqual(JSON.parse(JSON.stringify(writes[0])), { notificationSound: true });
  assert.equal(config.getState().config.notificationSound, true);
  console.log('PASS old configuration stays muted and enabling uses one config.set');

  await config.getState().resetConfig();
  assert.equal(writes.length, 2); assert.equal(writes[1].notificationSound, false);
  invoke = async () => { throw new Error('disk full'); };
  await assert.rejects(config.getState().saveConfig({ notificationSound: true }), /disk full/);
  assert.equal(config.getState().config.notificationSound, false);
  invoke = async () => ({ success: true, config: { ...types.DEFAULT_CONFIG, notificationSound: 'true' } });
  await assert.rejects(config.getState().saveConfig({ notificationSound: true }), /提示音设置/);
  console.log('PASS reset mutes sound; failed saves and invalid boolean responses preserve state');

  let calls = 0, finish;
  invoke = (command) => {
    assert.equal(command, 'notification.test_sound'); ++calls;
    return new Promise((resolve) => { finish = resolve; });
  };
  assert.equal(calls, 0); // Importing the service/configuration never plays a preview.
  const pending = preview(); assert.equal(await preview(), false);
  finish({ success: true, durationMs: 1200 }); assert.equal(await pending, true); assert.equal(calls, 1);
  console.log('PASS preview is explicit, does not save settings, and duplicate clicks are gated');

  invoke = async () => { throw new Error('device unavailable'); };
  await assert.rejects(preview(), /device unavailable/);
  for (const response of [{ success: false, durationMs: 1200 }, { success: true }, { success: true, durationMs: 0 }]) {
    invoke = async () => response; await assert.rejects(preview(), /后端未确认/);
  }
  invoke = async () => ({ success: true, durationMs: 800 }); assert.equal(await preview(), true);
  console.log('PASS playback errors cannot report success and subsequent previews can retry');

  const mock = load('src/services/bridge.ts', {
    '@tauri-apps/api/core': { isTauri: () => false, invoke: () => { throw new Error('Unexpected native IPC'); } },
    '@tauri-apps/api/event': { listen: () => { throw new Error('Unexpected native subscription'); } },
    '../types': types, '../utils/netValidation': validation,
  }, (source) => source.replaceAll('import.meta.hot', 'undefined').replaceAll('import.meta.env.VITE_MOCK_BRIDGE', "'1'"));
  await assert.rejects(mock.invoke('notification.test_sound'), /Windows/);
  await assert.rejects(mock.invoke('config.set', { notificationSound: 'yes' }), /布尔/);
  const result = await mock.invoke('config.set', { notificationSound: true }); assert.equal(result.config.notificationSound, true);
  assert.equal((await mock.invoke('app.info')).capabilities.notificationSound, false);
  console.log('PASS explicit browser mock rejects native sound and validates the persisted switch');
})().catch((error) => { console.error(error); process.exitCode = 1; });
