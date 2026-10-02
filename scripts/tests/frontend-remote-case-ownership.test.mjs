import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../../frontend/remote-sources.js', import.meta.url), 'utf8');
const tick = async () => { for (let i = 0; i < 15; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const connection = (id = 'access-a') => ({ id, name: `${id} confidential name`, kind: 'elasticsearch', kibanaVersion: 'auto', url: `https://${id}.example.test`, username: `${id}-user`, index: 'legacy-global-index', timeField: '@timestamp', maxRecords: 100, query: { match: { confidential: 'global query' } }, hasPassword: true, passwordSaved: true });
const snapshot = { path: '/snapshots/only-A.jsonl', count: 3 };
function fixture(store = null) {
  const nodes = new Map(), calls = [], saves = [], opened = [], pages = [], notices = [], cancellations = [], handlers = new Map(), events = new Map();
  class Element {
    constructor(tag = 'div', cls = '', text = '') { this.tagName = tag.toUpperCase(); this.textContent = text; this.value = ''; this.hidden = false; this.disabled = false; this.checked = false; this.children = []; this.dataset = {}; this.listeners = new Map(); this.attrs = {}; this.parentElement = { open: false }; this.classes = new Set(cls.split(' ')); this.classList = { add: x => this.classes.add(x), remove: x => this.classes.delete(x), contains: x => this.classes.has(x), toggle: (x, yes) => yes ? this.classes.add(x) : this.classes.delete(x) }; }
    set innerHTML(value) { this.html = value; for (const match of value.matchAll(/\bid="([^"]+)"/g)) if (!nodes.has(match[1])) nodes.set(match[1], new Element()); }
    get innerHTML() { return this.html || ''; }
    setAttribute(key, value) { this.attrs[key] = value; } removeAttribute(key) { delete this.attrs[key]; }
    append(...items) { this.children.push(...items); } after() {} replaceChildren(...items) { this.children = items; }
    querySelector(selector) { if (selector === '.remote-modal') return panel; return null; }
    querySelectorAll(selector) { return selector === 'button' ? this.children.filter(item => item.tagName === 'BUTTON') : []; }
    addEventListener(name, callback) { this.listeners.set(name, callback); } focus() {}
    emit(name) { this.listeners.get(name)?.({ target: this }); }
  }
  const panel = new Element('section', 'remote-modal'), host = new Element(), body = new Element();
  const state = { loaded: true, cases: store || { active: 'a', cases: [{ id: 'a', workspace: {} }, { id: 'b', workspace: {} }] } };
  const activeCase = () => state.cases.cases.find(item => item.id === state.cases.active) || null;
  let scope = 'dataset', changeScope = async () => { scope = 'dataset'; }, load = async () => true;
  const library = new Map(['access-a', 'access-b'].map(id => [id, connection(id)]));
  const backend = async (command, args) => {
    if (command === 'remote_list') return { connections: [...library.values()].map(item => structuredClone(item)), persistentSecrets: true };
    if (command === 'remote_save') { const saved = { ...args.connection, id: args.connection.id || 'new-access', hasPassword: !!args.password, passwordSaved: !!args.rememberPassword }; library.set(saved.id, saved); return saved; }
    if (command === 'remote_delete') { library.delete(args.id); return null; }
    if (command === 'remote_test') return { message: 'Acesso confirmado' };
    if (command === 'remote_import') return snapshot;
    throw Error(command);
  };
  const context = vm.createContext({ state, activeCase, URL, structuredClone, console,
    document: { body, getElementById: id => nodes.get(id), addEventListener: (name, callback) => events.set(name, callback) },
    el: (tag, cls, text) => new Element(tag, cls, text), $: () => new Element(),
    window: { AnalysisContexts: { capture: () => ({ item: activeCase() }), owns: owner => state.cases.cases.includes(owner.item) },
      WorkspaceContext: { scope: () => scope, setScope: () => changeScope() }, Workspace: { showPage: async page => { pages.push(page); } },
      Tasks: { cancelLatest: key => cancellations.push(key), operationFor: key => key }, __TAURI__: { event: { listen: async (name, callback) => events.set(name, callback) } } },
    saveCases: async () => { saves.push(structuredClone(state.cases)); return true; }, startOperation() {}, finishOperation() {}, fmtNum: String, toast: (...args) => notices.push(args),
    api: async (command, args = {}, options = {}) => { calls.push({ command, args: structuredClone(args), options }); return handlers.has(command) ? handlers.get(command)(args, options) : backend(command, args); },
    loadData: async (source, options) => { opened.push({ source, options, active: activeCase() }); return load(); },
  });
  vm.runInContext(source.replace('  caseChanged();\n})();', '  caseChanged(); window.__testRemote = { ui, act, selectConnection, refreshConnections, normalizeDraft, openSnapshot };\n})();'), context);
  const q = id => nodes.get(`rs-${id}`), remote = context.window.RemoteSources, internal = context.window.__testRemote;
  return { context, state, a: state.cases.cases[0], b: state.cases.cases[1], calls, saves, opened, pages, notices, cancellations, handlers, events, library, backend, q, remote, ...internal,
    mount: () => remote.mount(host), switchTo: item => { state.cases.active = item.id; remote.caseChanged(); },
    input: (id, value) => { q(id).value = value; q('fields').emit('input'); },
    replaceA: () => { const replacement = { id: 'a', workspace: {} }; state.cases.cases[0] = replacement; state.cases.active = 'a'; remote.caseChanged(); },
    delayScope: gate => { scope = 'case'; changeScope = () => gate.promise; }, delayLoad: gate => { load = () => gate.promise; } };
}
async function choose(f, id = 'access-a') { await f.q('reload').onclick(); f.selectConnection(f.library.get(id)); }

test('global library is explicit; A → B → A restores only that Case selection and query', async () => {
  const f = fixture(); await f.mount(); assert.equal(f.calls.length, 0); assert.equal(f.q('list').hidden, true);
  await choose(f); f.input('index', 'only-a-*'); f.input('query', '{"match":{"case":"a"}}'); f.input('from', '2026-10-01T11:30'); f.input('password', 'synthetic-secret-A');
  assert.equal(f.q('query').value, '{"match":{"case":"a"}}');
  f.switchTo(f.b); await f.mount();
  assert.equal(f.q('url').value, ''); assert.equal(f.q('name').value, ''); assert.equal(f.q('username').value, ''); assert.equal(f.q('password').value, '');
  assert.equal(f.q('query').value, ''); assert.equal(f.q('index').value, 'logs-*'); assert.equal(f.q('list').hidden, true); assert.equal(f.ui.selected, null);
  assert.equal(f.calls.filter(call => call.command === 'remote_list').length, 1, 'B does not automatically read the global library');
  f.input('index', 'only-b-*'); f.switchTo(f.a); await f.mount();
  assert.equal(f.q('url').value, connection().url); assert.equal(f.q('index').value, 'only-a-*'); assert.equal(f.q('from').value, '2026-10-01T11:30'); assert.equal(f.q('list').hidden, true);
  assert.equal(f.q('query').value, '{"match":{"case":"a"}}'); assert.equal(f.q('password').value, '');
  const stored = JSON.stringify(f.state.cases);
  for (const absent of ['synthetic-secret-A', 'confidential name', '.example.test', 'access-a-user', 'passwordSaved', 'hasPassword', 'legacy-global-index', 'global query']) assert.ok(!stored.includes(absent), absent);
  assert.equal(f.b.workspace.remoteSource.draft.index, 'only-b-*');
});
test('Case body reopen restores whitelist query and saved ID, without serializing endpoint drafts or vault secrets', async () => {
  const f = fixture(); await choose(f); f.input('query', '{"match_all":{}}'); f.input('limit', '17'); f.input('to', '2026-10-02T12:00');
  f.input('url', 'https://temporary.example.test/?token=synthetic-secret'); f.input('password', 'never-persist');
  const clean = JSON.parse(JSON.stringify(f.state.cases)); const reopened = fixture(clean); await reopened.mount();
  assert.equal(reopened.q('url').value, connection().url); assert.equal(reopened.q('limit').value, '17'); assert.equal(reopened.q('query').value, '{"match_all":{}}'); assert.equal(reopened.q('list').hidden, true);
  assert.ok(!JSON.stringify(clean).includes('synthetic-secret')); assert.ok(!JSON.stringify(clean).includes('never-persist'));
  const sanitized = f.normalizeDraft({ connectionId: 'x', password: 'secret', url: 'private', draft: { index: 'case-*', queryText: '{', password: 'secret', username: 'user', rememberPassword: true } });
  assert.deepEqual(Object.keys(sanitized).sort(), ['connectionId', 'draft', 'schemaVersion']); assert.equal(JSON.stringify(sanitized).includes('secret'), false);
});
test('unsaved access fields are isolated by Case instance and password is cleared on transition', async () => {
  const f = fixture(); f.input('name', 'A unsaved'); f.input('url', 'https://a.example.test'); f.input('password', 'secret');
  f.switchTo(f.b); assert.equal(f.q('name').value, ''); f.input('name', 'B draft');
  f.switchTo(f.a); assert.equal(f.q('name').value, 'A unsaved'); assert.equal(f.q('password').value, '');
  f.replaceA(); assert.equal(f.q('name').value, '');
});
test('global save has neutral query and captured Case alone receives its selected ID', async () => {
  const f = fixture(); await choose(f); f.selectConnection(); f.input('name', 'New access'); f.input('url', 'https://new.example.test'); f.input('index', 'private-a-*'); f.input('query', '{"match":{"case":"a"}}');
  const gate = deferred(); f.handlers.set('remote_save', async args => { await gate.promise; return f.backend('remote_save', args); });
  const saving = f.act('save'); await tick(); f.switchTo(f.b); const bStatus = f.q('status-text').textContent; gate.resolve(); await saving;
  const sent = f.calls.find(call => call.command === 'remote_save').args.connection;
  assert.equal(sent.index, 'logs-*'); assert.equal(sent.query, null); assert.equal(sent.maxRecords, 100000);
  assert.equal(f.a.workspace.remoteSource.connectionId, 'new-access'); assert.equal(f.a.workspace.remoteSource.draft.index, 'private-a-*');
  assert.equal(f.ui.selected, null); assert.equal(f.q('status-text').textContent, bStatus); assert.equal(f.b.workspace.remoteSource, undefined);
  f.switchTo(f.a); await f.mount(); assert.equal(f.q('url').value, 'https://new.example.test'); assert.equal(f.q('query').value, '{"match":{"case":"a"}}');
});
test('late library read or failure cannot enumerate another Case or replace newer navigation', async () => {
  for (const failure of [false, true]) {
    const f = fixture(), gate = deferred(); f.handlers.set('remote_list', () => gate.promise); const loading = f.q('reload').onclick(); f.switchTo(f.b);
    const before = f.q('status-text').textContent; failure ? gate.reject(Error('private-a failure')) : gate.resolve({ connections: [connection()], persistentSecrets: true }); await loading;
    assert.equal(f.q('list').hidden, true); assert.equal(f.ui.connections.length, 0); assert.equal(f.q('url').value, ''); assert.equal(f.q('status-text').textContent, before);
  }
});
test('late save/error cannot replace B draft or settle B active request', async () => {
  const f = fixture(); await choose(f); const aGate = deferred(), bGate = deferred(); f.handlers.set('remote_save', () => aGate.promise);
  const saving = f.act('save'); f.switchTo(f.b); await choose(f, 'access-b'); f.handlers.set('remote_test', () => bGate.promise); const testing = f.act('test');
  aGate.reject(Error('A failed')); await saving; assert.equal(f.ui.busy, true); assert.equal(f.ui.action, 'test'); assert.ok(!f.q('status-text').textContent.includes('A failed'));
  bGate.resolve({ message: 'B okay' }); await testing; assert.equal(f.q('status-text').textContent, 'B okay');
});
test('missing or deleted access never selects a different endpoint and retains the Case recorte', async () => {
  const f = fixture(); await choose(f); f.input('index', 'only-a-*'); f.library.delete('access-a'); f.remote.unmount(); await f.mount();
  assert.equal(f.ui.selected, null); assert.equal(f.a.workspace.remoteSource.connectionId, null); assert.equal(f.q('url').value, ''); assert.equal(f.q('index').value, 'only-a-*');
  await choose(f, 'access-b'); await f.q('delete').onclick(); assert.equal(f.ui.selected, null); assert.equal(f.q('url').value, ''); assert.equal(f.q('index').value, 'only-a-*'); assert.equal(f.a.workspace.remoteSource.connectionId, null);
});
test('late deletion clears only owning Case selection, never selects B or paints its status', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.handlers.set('remote_delete', async args => { await gate.promise; return f.backend('remote_delete', args); });
  const deleting = f.q('delete').onclick(); f.switchTo(f.b); await choose(f, 'access-b'); const before = f.q('status-text').textContent; gate.resolve(); await deleting;
  assert.equal(f.a.workspace.remoteSource.connectionId, null); assert.equal(f.b.workspace.remoteSource.connectionId, 'access-b'); assert.equal(f.ui.selected.id, 'access-b'); assert.equal(f.q('status-text').textContent, before);
});
test('late remote result stays private to A; explicit reopen succeeds only after returning to A', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.handlers.set('remote_import', () => gate.promise); const importing = f.act('import'); f.switchTo(f.b);
  const before = f.q('status-text').textContent; gate.resolve(snapshot); await importing;
  assert.equal(f.opened.length, 0); assert.equal(f.ui.lastImport, null); assert.equal(f.q('open-result').hidden, true); assert.equal(f.q('status-text').textContent, before);
  f.switchTo(f.a); await f.mount(); assert.equal(f.ui.lastImport.captured.item, f.a); await f.q('open-result').onclick();
  assert.equal(f.opened.length, 1); assert.equal(f.opened[0].options.caseId, 'a'); assert.equal(f.pages.at(-1), 'summary');
});
test('cancel addresses only its captured request and late completion never publishes the cancelled snapshot', async () => {
  const f = fixture(); await choose(f); const aGate = deferred(), bGate = deferred(); let counter = 0; f.handlers.set('remote_import', () => ++counter === 1 ? aGate.promise : bGate.promise);
  const aRequest = f.act('import'); const aKey = f.ui.request.key; f.q('cancel').onclick(); f.switchTo(f.b); await choose(f, 'access-b'); const bRequest = f.act('import'); const bKey = f.ui.request.key;
  assert.notEqual(aKey, bKey); assert.deepEqual(f.cancellations, [aKey]); aGate.resolve(snapshot); await aRequest; assert.equal(f.ui.busy, true); assert.equal(f.ui.request.key, bKey);
  f.q('cancel').onclick(); bGate.resolve(snapshot); await bRequest; assert.equal(f.opened.length, 0); assert.equal(f.ui.lastImport, null);
  f.switchTo(f.a); assert.equal(f.ui.lastImport, null);
});
test('deleted/recreated same-ID Case cannot claim a late saved connection or snapshot', async () => {
  for (const action of ['save', 'import']) {
    const f = fixture(); await choose(f); const gate = deferred(); f.handlers.set(`remote_${action}`, () => gate.promise); const pending = f.act(action); f.replaceA();
    gate.resolve(action === 'save' ? connection('new-id') : snapshot); await pending; assert.equal(f.ui.selected, null); assert.equal(f.ui.lastImport, null); assert.equal(f.opened.length, 0); assert.equal(f.state.cases.cases[0].workspace.remoteSource, undefined);
  }
});
test('A → B → A while awaiting source preparation does not resume stale open intent', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.delayScope(gate); const importing = f.act('import'); await tick(); f.switchTo(f.b); f.switchTo(f.a); gate.resolve(); await importing;
  assert.equal(f.opened.length, 0); assert.equal(f.pages.length, 0);
});
test('late load completion does not navigate B after a remote file was admitted for A', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.delayLoad(gate); const importing = f.act('import'); await tick(); assert.equal(f.opened[0].options.caseId, 'a');
  f.switchTo(f.b); gate.resolve(true); await importing; assert.equal(f.pages.length, 0);
});
test('Case bar synchronously resets remote ownership for creation, activation and body replacement', () => {
  const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8'); assert.match(app, /function renderCaseBar\(\) \{\s*window\.RemoteSources\?\.caseChanged\(\)/);
});
test('a superseded same-Case import cannot replace the newer retained result', async () => {
  const f = fixture(); await choose(f); const first = deferred(), second = deferred(); let count = 0; f.handlers.set('remote_import', () => ++count === 1 ? first.promise : second.promise);
  const old = f.act('import'); f.switchTo(f.b); f.switchTo(f.a); await f.mount(); const newer = f.act('import');
  const latest = { path: '/snapshots/latest-a.jsonl', count: 4 }; second.resolve(latest); await newer; first.resolve(snapshot); await old;
  f.switchTo(f.b); f.switchTo(f.a); await f.mount(); assert.equal(f.ui.lastImport.result.path, latest.path);
});
test('late saved access cannot replace a newer same-Case selection after A → B → A', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.handlers.set('remote_save', () => gate.promise);
  const saving = f.act('save'); f.switchTo(f.b); f.switchTo(f.a); await f.mount(); await choose(f, 'access-b'); gate.resolve(connection('old-save')); await saving;
  assert.equal(f.a.workspace.remoteSource.connectionId, 'access-b'); assert.equal(f.ui.selected.id, 'access-b');
});
test('unmount retires library load intent, clears password and requires an explicit library opening again', async () => {
  const f = fixture(); await f.mount(); await choose(f); f.input('password', 'synthetic-secret');
  const gate = deferred(); f.handlers.set('remote_list', () => gate.promise); const loading = f.q('reload').onclick(); f.remote.unmount();
  assert.equal(f.q('password').value, ''); gate.resolve({ connections: [connection('private')], persistentSecrets: true }); await loading;
  assert.equal(f.q('list').hidden, true); assert.equal(f.ui.connections.length, 0);
});
test('a library snapshot captured before a newer save cannot erase its new selection', async () => {
  const f = fixture(); const oldList = deferred(); f.handlers.set('remote_list', () => oldList.promise); const listing = f.q('reload').onclick();
  f.input('name', 'New A'); f.input('url', 'https://new-a.example.test'); await f.act('save');
  assert.equal(f.a.workspace.remoteSource.connectionId, 'new-access'); oldList.resolve({ connections: [], persistentSecrets: true }); await listing;
  assert.equal(f.a.workspace.remoteSource.connectionId, 'new-access'); assert.equal(f.ui.selected.id, 'new-access'); assert.equal(f.q('url').value, 'https://new-a.example.test');
});
test('save after same-Case unmount/remount reconciles its ID without repainting and the next query edit retains it', async () => {
  const f = fixture(); await f.mount(); f.input('name', 'New A'); f.input('url', 'https://new-a.example.test'); const gate = deferred();
  f.handlers.set('remote_save', async args => { await gate.promise; return f.backend('remote_save', args); });
  const saving = f.act('save'); f.remote.unmount(); await f.mount(); const status = f.q('status-text').textContent;
  gate.resolve(); await saving; assert.equal(f.a.workspace.remoteSource.connectionId, 'new-access'); assert.equal(f.q('status-text').textContent, status);
  f.input('query', '{"match_all":{}}'); assert.equal(f.a.workspace.remoteSource.connectionId, 'new-access');
  await f.act('test'); assert.equal(f.calls.find(call => call.command === 'remote_test').args.connection.id, 'new-access');
});
test('stale missing connection snapshot cannot erase a newer selected endpoint', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.handlers.set('remote_list', () => gate.promise); const listing = f.q('reload').onclick();
  f.selectConnection(f.library.get('access-b')); gate.resolve({ connections: [], persistentSecrets: true }); await listing;
  assert.equal(f.a.workspace.remoteSource.connectionId, 'access-b'); assert.equal(f.ui.selected.id, 'access-b'); assert.equal(f.q('url').value, connection('access-b').url);
});
test('import finishing after same-Case remount exposes an explicit result without reviving automatic navigation', async () => {
  const f = fixture(); await f.mount(); await choose(f); const gate = deferred(); f.handlers.set('remote_import', () => gate.promise);
  const importing = f.act('import'); f.remote.unmount(); await f.mount(); gate.resolve(snapshot); await importing;
  assert.equal(f.opened.length, 0); assert.equal(f.ui.lastImport.result.path, snapshot.path); assert.equal(f.q('open-result').hidden, false); assert.equal(f.pages.length, 0);
  await f.q('open-result').onclick(); assert.equal(f.opened.length, 1); assert.equal(f.pages.at(-1), 'summary');
});
test('after remote query commits its Cancel is retired and local source loading uses its own cancellation flow', async () => {
  const f = fixture(); await f.mount(); await choose(f); const gate = deferred(); f.delayLoad(gate);
  f.context.window.Workspace.showPage = async page => { f.remote.unmount(); f.pages.push(page); };
  const importing = f.act('import'); await tick(); assert.equal(f.opened.length, 1);
  assert.equal(f.q('cancel').hidden, true); assert.equal(f.q('cancel').disabled, true); f.q('cancel').onclick(); assert.equal(f.cancellations.length, 0);
  gate.resolve(false); await importing; assert.equal(f.pages.length, 0); assert.equal(f.q('open-result').hidden, false);
});
test('a cancelled captured request cannot navigate or toast success after local source settlement', async () => {
  const f = fixture(); await choose(f); const gate = deferred(); f.delayLoad(gate); const importing = f.act('import'); await tick();
  f.ui.request.cancelled = true; gate.resolve(true); await importing;
  assert.equal(f.pages.length, 0); assert.equal(f.notices.length, 0);
});
test('delete after same-Case remount clears the binding so subsequent query edits cannot resurrect the removed ID', async () => {
  const f = fixture(); await f.mount(); await choose(f); f.input('index', 'preserve-query-*'); const gate = deferred();
  f.handlers.set('remote_delete', async args => { await gate.promise; return f.backend('remote_delete', args); });
  const deleting = f.q('delete').onclick(); f.remote.unmount(); await f.mount(); gate.resolve(); await deleting;
  assert.equal(f.ui.connectionId, null); assert.equal(f.ui.selected, null); assert.equal(f.q('delete').hidden, true);
  f.input('query', '{"match_all":{}}'); assert.equal(f.a.workspace.remoteSource.connectionId, null); assert.equal(f.q('index').value, 'preserve-query-*');
});
test('a late deleted binding cannot clear a newly selected different connection after same-Case remount', async () => {
  const f = fixture(); await f.mount(); await choose(f); const gate = deferred(); f.handlers.set('remote_delete', async args => { await gate.promise; return f.backend('remote_delete', args); });
  const deleting = f.q('delete').onclick(); f.remote.unmount(); await f.mount(); f.selectConnection(f.library.get('access-b')); gate.resolve(); await deleting;
  assert.equal(f.ui.selected.id, 'access-b'); assert.equal(f.a.workspace.remoteSource.connectionId, 'access-b');
});
