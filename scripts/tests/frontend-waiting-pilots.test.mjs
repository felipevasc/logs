import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const source = app.slice(app.indexOf('let areaLoadingSerial ='), app.indexOf('// spinner discreto no título'));
function fixture({ integerTimers = false, firstTimerLateness = 0 } = {}) {
  let now = 0, serial = 0;
  const timers = new Map(), mounted = [], listeners = new Set();
  const node = () => ({ isConnected: true, children: [], classList: { add() {}, remove() {} },
    appendChild(child) { child.parent = this; this.children.push(child); },
    remove() { if (this.parent) this.parent.children = this.parent.children.filter(item => item !== this); this.isConnected = false; } });
  const container = node();
  const context = vm.createContext({ performance: { now: () => now }, el: node, esc: String,
    document: { addEventListener: (_type, fn) => listeners.add(fn), removeEventListener: (_type, fn) => listeners.delete(fn) },
    requestAnimationFrame: fn => fn(),
    setTimeout: (fn, delay) => { const id = ++serial; timers.set(id, { fn, at: now + (integerTimers ? Math.trunc(delay) : delay) + (id === 1 ? firstTimerLateness : 0) }); return id; },
    clearTimeout: id => timers.delete(id),
    window: { WaitingVisuals: { mount: (host, snapshot) => {
      const view = { host, receipts: [snapshot], destroyed: false,
        update(next) { this.receipts.push(next); }, destroy() { this.destroyed = true; } };
      mounted.push(view); return view;
    } } },
  });
  vm.runInContext(source, context);
  const advance = to => {
    for (;;) {
      const next = [...timers].filter(([, job]) => job.at <= to).sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      timers.delete(next[0]); now = next[1].at; next[1].fn();
    }
    now = to;
  };
  const cancel = operationId => { for (const fn of listeners) fn({ detail: { operationId, state: 'cancelling' } }); };
  return { context, container, mounted, timers, advance, listeners, cancel };
}

test('instant group waits do not flash or delay results', () => {
  const f = fixture(), wait = f.context.areaLoading(f.container, 'Calculando resumo', { phaseId: 'command:aggregate_events' });
  f.advance(100); wait.done(); f.advance(5000);
  assert.equal(f.mounted.length, 0); assert.equal(f.container.children.length, 0); assert.equal(f.timers.size, 0);
  assert.equal(f.listeners.size, 0);
});

test('cancellation is bound to the exact operation and cannot start a later loop', () => {
  const f = fixture(), wait = f.context.areaLoading(f.container, 'Calculando', { phaseId: 'command:pivot' });
  wait.bindOperation('pivot-a');
  f.advance(250); const view = f.mounted[0];
  f.cancel('another-query'); assert.equal(view.receipts.at(-1).state, 'running');
  f.cancel('pivot-a'); assert.equal(view.receipts.at(-1).state, 'cancelling'); assert.equal(view.receipts.at(-1).label, '');
  const count = view.receipts.length;
  f.advance(6000); assert.equal(view.receipts.length, count, 'cancel clears the decorative long-wait timer');
  wait.done(); assert.equal(f.listeners.size, 0);
});

test('cancellation before delayed appearance produces a static wait without a loop timer', () => {
  const f = fixture(), wait = f.context.areaLoading(f.container, 'Calculando', { phaseId: 'command:pivot' });
  wait.bindOperation('pivot-a'); f.cancel('pivot-a'); f.advance(6000);
  assert.equal(f.mounted[0].receipts[0].state, 'cancelling'); assert.equal(f.timers.size, 0);
  wait.done(); assert.equal(f.listeners.size, 0);
});

test('long calculation uses measured elapsed time and a single threshold update', () => {
  const f = fixture(), wait = f.context.areaLoading(f.container, 'Cruzando dados', { phaseId: 'command:pivot' });
  f.advance(250);
  const view = f.mounted[0];
  assert.equal(view.receipts[0].elapsedMs, 250); assert.equal(view.receipts[0].phaseId, 'command:pivot');
  assert.equal(view.receipts[0].total, undefined); assert.equal(view.receipts[0].phaseCount, undefined);
  f.advance(3999); assert.equal(view.receipts.length, 1);
  f.advance(4000); assert.equal(view.receipts.length, 2); assert.equal(view.receipts[1].elapsedMs, 4000);
  assert.equal(view.receipts[0].operationId, view.receipts[1].operationId);
  f.advance(9000); assert.equal(view.receipts.length, 2, 'no recurring visual polling');
  wait.done(); assert.equal(view.destroyed, true); assert.equal(f.container.children.length, 0);
  assert.equal(f.timers.size, 0, 'no exit animation retains an overlay over results');
});

test('fractional callback timing still crosses the long-wait threshold with integer browser timers', () => {
  // Browser timeout conversion discards fractions; a sub-millisecond-late initial
  // callback must not leave the only threshold receipt at 3999.6ms forever.
  const f = fixture({ integerTimers: true, firstTimerLateness: 0.6 });
  const wait = f.context.areaLoading(f.container, 'Calculando resumo', { phaseId: 'command:aggregate_events' });
  f.advance(251);
  const view = f.mounted[0];
  assert.equal(view.receipts[0].elapsedMs, 250.6);
  f.advance(3999); assert.equal(view.receipts.length, 1, 'no loop before its measured threshold');
  f.advance(4001);
  assert.equal(view.receipts.length, 2, 'only the single long-wait transition updates the visual');
  assert.ok(view.receipts[1].elapsedMs >= 4000, `measured threshold receipt was too early: ${view.receipts[1].elapsedMs}`);
  assert.ok(view.receipts[1].elapsedMs < 4001, 'no large extra animation delay');
  f.advance(10000); assert.equal(view.receipts.length, 2, 'the threshold is not a recurring poll');
  assert.equal(f.timers.size, 0);
  wait.done(); assert.equal(f.listeners.size, 0);
});

test('settlement before loop threshold disposes the view and timer', () => {
  const f = fixture(), wait = f.context.areaLoading(f.container, 'Calculando', { phaseId: 'command:pivot' });
  f.advance(1000); const view = f.mounted[0]; wait.done(); wait.done(); f.advance(6000);
  assert.equal(view.destroyed, true); assert.equal(view.receipts.length, 1); assert.equal(f.timers.size, 0);
});

test('removed region does not create a detached waiting component', () => {
  const f = fixture(), wait = f.context.areaLoading(f.container, 'Calculando', { phaseId: 'command:pivot' });
  f.container.isConnected = false; f.advance(6000); wait.done();
  assert.equal(f.mounted.length, 0); assert.equal(f.container.children.length, 0); assert.equal(f.timers.size, 0);
});
