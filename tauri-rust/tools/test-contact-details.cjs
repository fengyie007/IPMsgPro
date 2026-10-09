// Render the actual contact details with synthetic stores/clipboard; no browser.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const assert = require('node:assert/strict'), ts = require('typescript');
const React = require('react'), { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '../src'), modules = new Map();
const notices = [], copied = [];
let failCopy = false;
const user = { id: 'peer', nickname: '联系人', username: 'account', hostname: 'PC', group: '',
  ip: '127.0.0.1', port: 2425, status: 'online', version: '1', mac: 'A0-B1-C2-D3-E4-F5' };
const userState = { users: [user] };
const workspace = { contactId: user.id, openChat() {} };
const useWorkspaceStore = selector => selector(workspace);
useWorkspaceStore.getState = () => workspace;
function load(file) {
  if (modules.has(file)) return modules.get(file);
  const exports = {}; modules.set(file, exports);
  const code = ts.transpileModule(fs.readFileSync(file, 'utf8').replaceAll('import.meta.hot', 'undefined')
    .replaceAll('import.meta.env.VITE_MOCK_BRIDGE', "'1'"), { fileName: file,
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true } }).outputText;
  vm.runInNewContext(code, { exports, console, setTimeout, clearTimeout, setInterval, clearInterval,
    navigator: { clipboard: { async writeText(value) { if (failCopy) throw new Error('clipboard unavailable'); copied.push(value); } } },
    require(name) {
      if (name === '../stores/userStore') return { useUserStore: selector => selector(userState) };
      if (name === '../stores/workspaceStore') return { useWorkspaceStore };
      if (name === '../stores/toastStore') return { toast: { success: text => notices.push(['success', text]), error: text => notices.push(['error', text]) } };
      if (name === '../utils/useScrollMemory') return { useScrollMemory: () => ({}) };
      if (name === './UserAvatar') return () => null;
      if (name === '@tauri-apps/api/core') return { isTauri: () => false, invoke: () => assert.fail('unexpected native IPC') };
      if (name === '@tauri-apps/api/event') return { listen: () => assert.fail('unexpected native subscription') };
      if (!name.startsWith('.')) return require(name);
      const target = path.resolve(path.dirname(file), name);
      return load(fs.existsSync(target + '.ts') ? target + '.ts' : fs.existsSync(target + '.tsx') ? target + '.tsx' : path.join(target, 'index.ts'));
    } }, { filename: file });
  return exports;
}
const ContactDetails = load(path.join(root, 'components/ContactDetails.tsx')).default;
const render = () => renderToStaticMarkup(React.createElement(ContactDetails));
function elements(tree) {
  if (Array.isArray(tree)) return tree.flatMap(elements);
  if (!React.isValidElement(tree)) return [];
  return [tree, ...elements(tree.props.children)];
}
const click = async title => {
  const button = elements(ContactDetails()).find(item => item.type === 'button' && item.props.title === title);
  assert(button, title); button.props.onClick(); await new Promise(setImmediate);
};

(async () => {
  assert(render().includes('A0-B1-C2-D3-E4-F5')); assert(render().includes('复制MAC地址'));
  user.status = 'offline'; assert(render().includes('A0-B1-C2-D3-E4-F5'));
  delete user.mac; assert(render().includes('未提供')); assert(!render().includes('复制MAC地址'));
  user.mac = '<script>test</script>'; assert(!render().includes('<script>')); assert(render().includes('&lt;script&gt;'));
  user.mac = 'A0-B1-C2-D3-E4-F5';
  console.log('PASS contact MAC display survives offline status, missing values have no copy button and peer text is escaped');

  await click('复制MAC地址'); await click('复制联系地址');
  assert.deepEqual(copied, ['A0-B1-C2-D3-E4-F5', '127.0.0.1:2425']);
  assert.equal(notices[0][1], '已复制MAC地址'); assert.equal(notices[1][1], '已复制联系地址');
  failCopy = true; await click('复制MAC地址'); assert.equal(notices.at(-1)[0], 'error'); assert.equal(copied.length, 2);
  console.log('PASS MAC and endpoint copy use the correct text and clipboard failures are reported');

  const bridge = load(path.join(root, 'services/bridge.ts'));
  const local = await bridge.invoke('user.local');
  assert.equal(local.username, 'me-2427');
  const contacts = await bridge.invoke('user.list');
  assert.equal(contacts.users[0].mac, '02-11-22-33-44-55'); assert.equal(contacts.users[1].mac, undefined);
  console.log('PASS explicit browser mock uses the unbranded login and includes supplied/missing MAC examples');
})().catch(error => { console.error(error); process.exitCode = 1; });
