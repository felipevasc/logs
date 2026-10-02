import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/tasks.js', import.meta.url), 'utf8');
const code = source.slice(source.indexOf('  function cancel(entry)'), source.indexOf('  function progress(payload)'));
function fixture() {
  const tasks = new Map(), latest = new Map(), inflight = new Map(), events = [], native = [];
  const context = vm.createContext({ tasks, latest, inflight, window: {}, performance: { now: () => 0 }, schedule() {},
    serial: 0, named: new Set(['load_file']), READS: new Set(), origin: () => ({ key: 'import' }),
    run: () => new Promise(() => {}), setTimeout() {}, VISIBLE_AFTER: 300,
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
    document: { dispatchEvent: event => events.push(event) },
    base: (command, args) => { native.push({ command, args }); return Promise.resolve(); },
  });
  vm.runInContext(code, context);
  vm.runInContext(source.slice(source.indexOf('  function start(cmd'), source.indexOf('  api = function')), context);
  const entry = (id, key) => { const value = { operationId: id, owners: new Set(key ? [key] : []), keepAlive: false }; tasks.set(id, value); if (key) latest.set(key, value); return value; };
  return { context, entry, events, native };
}

test('direct cancellation sends one local state event for the same native operation', () => {
  const f = fixture(), task = f.entry('op-a');
  f.context.cancel(task); f.context.cancel(task);
  assert.equal(f.events.length, 1); assert.equal(f.events[0].type, 'task-state-change');
  assert.equal(f.events[0].detail.operationId, 'op-a'); assert.equal(f.events[0].detail.state, 'cancelling');
  assert.equal(f.native.length, 1); assert.equal(f.native[0].command, 'cancel_task');
  assert.equal(f.native[0].args.operationId, 'op-a');
});

test('named task start publishes its identity before progress or cancelLatest', () => {
  const f = fixture();
  const task = f.context.start('load_file', {}, { latest: 'source-load' }, null);
  assert.equal(f.events[0].detail.started, true);
  assert.equal(f.events[0].detail.latestKey, 'source-load');
  assert.equal(f.events[0].detail.operationId, task.operationId);
  f.context.cancelLatest('source-load');
  assert.equal(f.events[1].detail.state, 'cancelling');
  assert.equal(f.events[1].detail.operationId, task.operationId);
});

test('cancelLatest and cancelAll share immediate state propagation without claiming completion', () => {
  const f = fixture(); f.entry('group-a', 'group'); f.entry('pivot-b', 'pivot');
  f.context.cancelLatest('group'); assert.equal(f.events[0].detail.operationId, 'group-a');
  f.context.cancelAll();
  assert.deepEqual(f.events.map(event => event.detail.operationId), ['group-a', 'pivot-b']);
  assert.ok(f.events.every(event => event.detail.state === 'cancelling'));
});
