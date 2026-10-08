// Exercise physical-pixel geometry and the real capture-to-send flow without a GUI.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const ts = require('typescript');
function load(relative, requireModule = () => { throw new Error('Unexpected import'); }) {
  const filename = path.join(__dirname, '..', relative);
  const compiled = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
    fileName: filename, compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const exports = {};
  vm.runInNewContext(compiled, { exports, require: requireModule, console }, { filename });
  return exports;
}
const geometry = load('src/utils/screenshot.ts');
const plain = (value) => JSON.parse(JSON.stringify(value));
for (const scale of [1, 1.5, 2]) {
  const rect = { left: 40, top: 20, width: 1920 / scale, height: 1080 / scale };
  assert.deepEqual(plain(geometry.imagePoint(40 + 600 / scale, 20 + 450 / scale, rect, 1920, 1080)), { x: 600, y: 450 });
}
console.log('PASS 100%, 150%, 200% CSS-to-image scaling');
// The desktop's negative monitor origin does not participate in local canvas coordinates.
assert.deepEqual(plain(geometry.imagePoint(-1, -1, { left: 0, top: 0, width: 1280, height: 720 }, 1920, 1080)), { x: 0, y: 0 });
assert.deepEqual(plain(geometry.imagePoint(9999, 9999, { left: 0, top: 0, width: 1280, height: 720 }, 1920, 1080)), { x: 1920, y: 1080 });
assert.deepEqual(plain(geometry.imageSelection({ x: 600.8, y: 400.2 }, { x: 10.2, y: 20.8 }, 1920, 1080)), { x: 10, y: 20, w: 591, h: 381 });
assert.deepEqual(plain(geometry.imageSelection({ x: -10, y: -10 }, { x: 9999, y: 9999 }, 1920, 1080)), { x: 0, y: 0, w: 1920, h: 1080 });
assert.deepEqual(plain(geometry.imageSelection({ x: 20, y: 30 }, { x: 20, y: 30 }, 1920, 1080)), { x: 20, y: 30, w: 0, h: 0 });
console.log('PASS reverse/edge/empty crop bounds');

let invoke, sendImage;
const { captureAndSend } = load('src/services/screenshot.ts', (name) => {
  if (name === './bridge') return { invoke: (...args) => invoke(...args) };
  if (name === '../stores/messageStore') return { useMessageStore: { getState: () => ({ sendImage: (...args) => sendImage(...args), error: 'queue full' }) } };
  throw new Error(`Unexpected import ${name}`);
});
(async () => {
  let calls = [];
  sendImage = async () => { throw new Error('Cancelled capture must not send'); };
  invoke = async (name) => { calls.push(name); return { success: true, cancelled: true }; };
  await captureAndSend('original');
  assert.deepEqual(calls, ['screenshot.start']);
  console.log('PASS cancel does not send or fabricate a message');

  let finishCapture, currentRecipient = 'original';
  invoke = (name) => {
    assert.equal(name, 'screenshot.start');
    return new Promise((resolve) => { finishCapture = resolve; });
  };
  sendImage = async (target, assetId) => { calls.push([target, assetId]); return true; };
  calls = [];
  const pending = captureAndSend(currentRecipient);
  currentRecipient = 'other';
  finishCapture({ success: true, image: { assetId: 'screenshot-asset' } });
  await pending;
  assert.deepEqual(calls, [['original', 'screenshot-asset']]);
  console.log('PASS original recipient and existing image.send flow; accepted asset retained');

  for (const throws of [false, true]) {
    calls = [];
    invoke = async (name, args) => {
      calls.push([name, args]);
      return name === 'screenshot.start' ? { success: true, image: { assetId: 'unused' } } : { success: true };
    };
    sendImage = async () => { if (throws) throw new Error('IPC failed'); return false; };
    await assert.rejects(captureAndSend('original'), throws ? /IPC failed/ : /queue full/);
    assert.deepEqual(plain(calls[1]), ['image.discard', { assetId: 'unused' }]);
  }
  console.log('PASS rejected/throwing sends discard only the unreferenced screenshot');

  invoke = async () => { throw new Error('capture failed'); };
  await assert.rejects(captureAndSend('original'), /capture failed/);
  invoke = async () => ({ success: true });
  await assert.rejects(captureAndSend('original'), /有效图片/);
  console.log('PASS capture failure and missing asset do not send');
})().catch((error) => { console.error(error); process.exitCode = 1; });
