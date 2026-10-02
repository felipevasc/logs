import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/updates.js', import.meta.url), 'utf8');
const part = (start, end) => {
  const from = source.indexOf(start), to = source.indexOf(end, from);
  assert.ok(from >= 0 && to > from, `real controller boundary: ${start}`);
  return source.slice(from, to);
};
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const snapshot = (revision, phase = 'available', extra = {}) => ({ snapshotRevision: revision, phase,
  available: { version: '0.6.0' }, skippedVersion: null, installOnClose: false, downloaded: 0, checkOnStart: true, ...extra });

// Execute the actual reducer, async commands, startup, public entry points and
// event listener. Only DOM rendering and IPC delivery are controlled here.
function fixture(invoke = () => { throw Error('unexpected IPC'); }, save = async () => true) {
  const listeners = new Map(), renders = [], panes = [], opened = [], hidden = [], toasts = [];
  const pane = { hidden: false };
  const context = vm.createContext({ status: null, notice: null, snapshotRevision: null, downloadRequests: 0, preparingInstall: false,
    document: { querySelector: selector => selector === '#settings-pane-updates' ? pane : null },
    window: { __TAURI__: { event: { listen: (name, callback) => { listeners.set(name, callback); return Promise.resolve(() => {}); } } } },
    call: (command, args) => invoke(command, args), saveCases: save,
    render: () => renders.push(context.status), renderPane: () => panes.push({ status: context.status, notice: context.notice }),
    open: () => opened.push(context.status), hide: () => hidden.push(context.status), toast: (...args) => toasts.push(args),
  });
  vm.runInContext(part('  function set(next', '  function dialog()'), context);
  vm.runInContext(part('  async function download()', '  const openPage'), context);
  vm.runInContext(part('  async function startup()', '  window.__TAURI__?.event?.listen("update-close-requested"'), context);
  vm.runInContext('testAPI = ' + part('  return {\n    open: async', '\n})();').replace(/^  return /, ''), context);
  return { context, renders, panes, opened, hidden, toasts, emit: value => listeners.get('update-state')({ payload: value }) };
}

test('Ready event survives an older Downloading invoke response through the actual download action', async () => {
  const reply = deferred(), f = fixture(command => { assert.equal(command, 'update_download'); return reply.promise; });
  f.emit(snapshot('1'));
  const pending = f.context.download();
  f.emit(snapshot('2', 'downloading', { downloaded: 10 }));
  f.emit(snapshot('3', 'ready', { downloaded: 100 }));
  reply.resolve(snapshot('2', 'downloading', { downloaded: 10 }));
  await pending;
  assert.equal(f.context.status.phase, 'ready');
  assert.equal(f.context.status.downloaded, 100);
  assert.deepEqual(f.renders.map(s => s.phase), ['available', 'downloading', 'ready']);
  assert.equal(f.opened.length, 1);
  assert.equal(f.context.downloadRequests, 0);
});

test('user download readiness is announced even when Ready arrives before its first progress snapshot', async () => {
  const reply = deferred(), f = fixture(() => reply.promise);
  f.emit(snapshot('1'));
  const pending = f.context.download();
  f.emit(snapshot('3', 'ready')); f.emit(snapshot('2', 'downloading'));
  reply.resolve(snapshot('2', 'downloading')); await pending;
  assert.equal(f.opened.length, 1); assert.equal(f.context.status.phase, 'ready');
});

test('late initial status read and overlapping check responses cannot overwrite newer events', async () => {
  const initial = deferred(), check = deferred();
  const f = fixture(command => command === 'update_status' ? initial.promise : check.promise);
  const opening = f.context.testAPI.open(), checking = f.context.testAPI.check();
  f.emit(snapshot('11', 'checking')); f.emit(snapshot('12', 'ready'));
  initial.resolve(snapshot('9', 'idle')); await opening;
  check.resolve(snapshot('10')); await checking;
  assert.equal(f.context.status.snapshotRevision, '12'); assert.equal(f.context.status.phase, 'ready');
});

test('equal event and response are idempotent while newer cancel and retry phases remain valid', async () => {
  const replies = [deferred(), deferred()]; let index = 0;
  const f = fixture(() => replies[index++].promise);
  f.emit(snapshot('1')); const first = f.context.download();
  f.emit(snapshot('2', 'downloading'));
  const count = f.renders.length;
  assert.equal(f.context.set(snapshot('2', 'idle')), true);
  assert.equal(f.renders.length, count); assert.equal(f.context.status.phase, 'downloading');
  f.emit(snapshot('3')); const retry = f.context.download();
  f.emit(snapshot('4', 'downloading', { downloaded: 20 }));
  replies[0].resolve(snapshot('2', 'downloading', { downloaded: 1 })); await first;
  assert.equal(f.context.downloadRequests, 1);
  f.emit(snapshot('5', 'ready')); replies[1].resolve(snapshot('4', 'downloading')); await retry;
  assert.equal(f.context.downloadRequests, 0); assert.equal(f.context.status.phase, 'ready');
  assert.deepEqual(f.renders.map(s => s.phase), ['available', 'downloading', 'available', 'downloading', 'ready']);
});

test('decimal revisions preserve ordering beyond Number precision and across digit widths', () => {
  const f = fixture();
  for (const revision of ['9', '10', '9007199254740992', '9007199254740993', '99999999999999999999999999999999999999', '100000000000000000000000000000000000000', '340282366920938463463374607431768211455']) {
    assert.equal(f.context.set(snapshot(revision)), true);
    assert.equal(f.context.status.snapshotRevision, revision);
  }
  assert.equal(f.context.set(snapshot('9007199254740992', 'idle')), false);
  assert.equal(f.context.status.snapshotRevision, '340282366920938463463374607431768211455');
});

test('invalid or unversioned snapshots never acquire reducer authority', () => {
  const f = fixture(); f.emit(snapshot('1', 'ready'));
  for (const revision of [undefined, null, 2, '', '-1', '01', '1.0', '1e3', ' 2', '2\n', '1'.repeat(40)]) {
    assert.equal(f.context.set(snapshot(revision)), false, String(revision));
  }
  assert.equal(f.renders.length, 1); assert.equal(f.context.status.phase, 'ready');
});

test('older snapshots cannot roll back progress, preferences or install-on-close', () => {
  const f = fixture();
  f.emit(snapshot('20', 'ready', { downloaded: 100, installOnClose: true, skippedVersion: '0.5.9', checkOnStart: false }));
  f.emit(snapshot('19', 'downloading', { downloaded: 1 }));
  assert.equal(f.context.status.downloaded, 100); assert.equal(f.context.status.installOnClose, true);
  assert.equal(f.context.status.skippedVersion, '0.5.9'); assert.equal(f.context.status.checkOnStart, false);
});

for (const kind of ['updated', 'failed']) for (const revision of ['1', '2']) {
  test(`${kind} startup notice survives an older/equal phase revision ${revision} and is announced once`, async () => {
    const message = { kind, from: '0.5.0', to: '0.5.1', backup: '/synthetic/backup' };
    const f = fixture(() => snapshot(revision, 'idle', { notice: message }));
    f.emit(snapshot('2', 'ready'));
    await f.context.startup(); await f.context.startup(); f.emit(snapshot('3', 'ready'));
    assert.equal(f.context.status.phase, 'ready'); assert.equal(f.context.status.snapshotRevision, '3');
    assert.equal(f.context.notice, message); assert.equal(f.toasts.length, 1);
    assert.equal(f.toasts[0][1], kind === 'updated' ? 'ok' : 'err');
    assert.equal(f.panes.at(-1).notice, message, 'settings retains the one-shot notice after normal snapshots');
  });
}

test('failed saves render a local error without forging a native revision or accepting stale native state', async () => {
  const f = fixture(command => command === 'update_status' ? snapshot('2', 'ready') : snapshot('2', 'ready'), async () => false);
  f.emit(snapshot('2', 'ready'));
  await assert.rejects(f.context.install(), /salvar os casos/);
  assert.match(f.context.status.error, /salvar os casos/); assert.equal(f.context.snapshotRevision, '2');
  f.emit(snapshot('1')); f.emit(snapshot('2', 'ready'));
  assert.match(f.context.status.error, /salvar os casos/);
  f.emit(snapshot('3', 'ready')); assert.equal(f.context.status.error, undefined);
});

for (const method of ['skip', 'installOnClose']) {
  test(`stale ${method} reply cannot hide or announce an obsolete action; equal current reply can`, async () => {
    const old = deferred(), fresh = deferred(); let count = 0;
    const f = fixture(() => count++ ? fresh.promise : old.promise);
    f.emit(snapshot('1'));
    const pending = f.context[method](); f.emit(snapshot('3', 'ready'));
    old.resolve(snapshot('2', 'ready', { installOnClose: true })); await pending;
    assert.equal(f.hidden.length, 0); assert.equal(f.toasts.length, 0);
    const current = f.context[method](); const response = snapshot('4', 'ready', { installOnClose: true });
    f.emit(response); fresh.resolve(response); await current;
    assert.equal(f.hidden.length, 1); assert.equal(f.toasts.length, 1);
  });
}

test('preview producer allocates read revisions, reuses publish revisions and consumes startup notices once', async () => {
  const storage = new Map(), context = vm.createContext({
    window: { addEventListener() {}, __mockUpdate: { version: '0.6.0' }, __mockUpdateNotice: { kind: 'updated', to: '0.5.1' } },
    localStorage: { getItem: key => storage.get(key) || null, setItem: (key, value) => storage.set(key, value) },
    structuredClone, btoa, atob, TextEncoder, setTimeout,
  });
  vm.runInContext(readFileSync(new URL('../preview/mock-tauri.js', import.meta.url), 'utf8'), context);
  const invoke = context.window.__TAURI__.core.invoke, events = [];
  await context.window.__TAURI__.event.listen('update-state', ({ payload }) => events.push(payload));
  const first = await invoke('update_status'), second = await invoke('update_status');
  assert.equal(typeof first.snapshotRevision, 'string'); assert.ok(BigInt(second.snapshotRevision) > BigInt(first.snapshotRevision));
  const startup = await invoke('update_startup');
  assert.equal(startup.snapshotRevision, events.at(-1).snapshotRevision);
  assert.equal(startup.notice.kind, 'updated'); assert.equal(events.at(-1).notice, null);
  assert.equal((await invoke('update_startup')).notice, null);
  const skipped = await invoke('update_skip', { version: '0.6.0' });
  assert.equal(skipped.snapshotRevision, events.at(-1).snapshotRevision);
  assert.equal(skipped.skippedVersion, '0.6.0');
});
