import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const tasksSource = readFileSync(new URL('../../frontend/tasks.js', import.meta.url), 'utf8');
const section = (source, start, end) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
function fixture({ blurToBody = false } = {}) {
  const listeners = new Map(), nodes = new Map(), tasks = new Map(), latest = new Map(), native = [], views = [];
  const state = { loadOverlay: false, progressOperationId: 'unrelated-query' };
  const document = { activeElement: null, querySelectorAll: () => [], addEventListener: (type, fn) => listeners.set(type, fn),
    dispatchEvent: event => listeners.get(event.type)?.(event) };
  const node = selector => {
    if (!nodes.has(selector)) {
      const value = { selector, textContent: '', style: {}, dataset: {}, parentElement: {}, isConnected: true,
        classList: { toggle() {} }, getClientRects() { return this.hidden ? [] : [{}]; }, focus() { document.activeElement = this; },
        contains(child) { return this.selector === '#load-overlay' && !!child?.selector.startsWith('#load-') && child.selector !== '#load-trigger'; } };
      // Browsers can blur synchronously on hidden/disabled. Focus recovery must
      // capture the old active element before changing either property.
      for (const property of ['hidden', 'disabled']) {
        let flag = false;
        Object.defineProperty(value, property, { get: () => flag, set(next) { flag = !!next; if (flag && document.activeElement === value) document.activeElement = blurToBody ? document.body : null; } });
      }
      nodes.set(selector, value);
    }
    return nodes.get(selector);
  };
  document.body = node('body'); document.documentElement = node('html');
  const context = vm.createContext({ state, document, $: node, performance: { now: () => 1000 }, fmtNum: String, pushLoadStep() {},
    tasks, latest, inflight: new Map(), schedule() {},
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
    base: (command, args) => { native.push({ command, args }); return Promise.resolve(); },
    window: {
      __TAURI__: { event: { listen: (_type, fn) => { listeners.set('operation-progress', fn); return Promise.resolve(); } } },
      WaitingVisuals: { mount: (_host, receipt) => {
        const view = { receipt, destroyed: false, update(next) { this.receipt = next; node('#load-visual .wv-motion-toggle').hidden = next.state === 'cancelling'; }, destroy() { this.destroyed = true; } };
        views.push(view); return view;
      } },
      PerformanceTools: { estimate: (_previous, p) => ({ completed: p.completed, total: p.total, percent: null }), phaseSeconds: () => null, duration: String },
    },
  });
  vm.runInContext(readFileSync(new URL('../../frontend/waiting-progress.js', import.meta.url), 'utf8'), context);
  vm.runInContext(section(tasksSource, '  function cancel(entry)', '  function detail(t)'), context);
  context.window.Tasks = { operationFor: key => latest.get(key)?.operationId || null,
    cancelOperation: id => context.cancel(tasks.get(id)), progress: p => context.progress(p) };
  vm.runInContext(section(app, 'function setWorkbar(', '// ------------------------------------------------------------------ overlay de carga'), context);
  vm.runInContext(section(app, 'let loadStepCount =', '\nfunction pushLoadStep'), context);
  vm.runInContext(section(app, 'function mirrorLoadOverlay(', '// live-refresh quando'), context);
  const start = (id, key = 'source-load') => {
    const entry = { operationId: id, owners: new Set([key]), cancelled: false, started: 0 };
    tasks.set(id, entry); latest.set(key, entry);
    document.dispatchEvent({ type: 'task-state-change', detail: { operationId: id, latestKey: key, state: 'running', started: true } });
    return entry;
  };
  const progress = (id, cancellable = true, extra = {}) => listeners.get('operation-progress')({ payload: {
    operationId: id, phaseId: 'metadata-scan', phase: 'Indexando metadados', cancellable, completed: 5, total: 10, unit: 'registros', ...extra,
  } });
  const show = (key = 'source-load') => context.showLoadOverlay('Preparando a fonte', key);
  return { context, document, state, node, tasks, latest, native, views, start, progress, show };
}

test('foreground button is a separate accessible operation control wired to the exact owner', () => {
  const html = readFileSync(new URL('../../frontend/index.html', import.meta.url), 'utf8');
  assert.match(html, /<button id="load-cancel"[^>]*type="button"[^>]*aria-describedby="load-cancel-help"[^>]*disabled>Cancelar operação<\/button>/);
  assert.ok(html.indexOf('id="load-cancel"') < html.indexOf('id="load-progress-details"'), 'Cancel stays outside collapsed details');
  assert.ok(app.includes('$("#load-cancel").onclick = cancelLoadOperation;'));
  const f = fixture(); f.show(); f.start('load-a');
  assert.equal(f.state.loadOverlayOperationId, 'load-a', 'identity binds before first progress');
  assert.equal(f.node('#load-cancel').disabled, true, 'start alone does not invent cancellation eligibility');
  f.context.cancelLoadOperation(); f.context.cancelWorkbarTask();
  assert.equal(f.native.length, 0, 'neither control falls back to stale workbar identity');
  f.progress('load-a'); assert.equal(f.node('#load-cancel').disabled, false);
});

test('only owner receipts enable Cancel and noncancellable phases transfer only affected focus', () => {
  const f = fixture(); f.show(); f.start('load-a'); f.start('query', 'query');
  f.progress('query'); assert.equal(f.node('#load-cancel').disabled, true);
  f.progress('load-a'); f.node('#load-cancel').focus();
  f.progress('load-a', false);
  assert.equal(f.node('#load-cancel').disabled, true);
  assert.equal(f.document.activeElement, f.node('#load-progress-details > summary'));
  f.context.cancelLoadOperation(); assert.equal(f.native.length, 0);
  f.node('#elsewhere').focus(); f.progress('load-a'); f.progress('load-a', false);
  assert.equal(f.document.activeElement, f.node('#elsewhere'), 'progress never steals unrelated focus');
});

test('actual Tasks cancellation targets one task once and remains pending/static until settlement', () => {
  const f = fixture(); f.show(); const task = f.start('load-a'); f.start('query', 'query'); f.progress('load-a');
  f.node('#load-cancel').focus();
  f.context.cancelLoadOperation(); f.context.cancelLoadOperation(); f.context.cancelWorkbarTask();
  assert.equal(f.native.length, 1); assert.equal(f.native[0].command, 'cancel_task');
  assert.equal(f.native[0].args.operationId, 'load-a');
  assert.equal(task.cancelled, true); assert.equal(f.tasks.get('query').cancelled, false);
  assert.equal(f.node('#load-cancel').disabled, true); assert.equal(f.state.loadOverlay, true);
  assert.equal(f.views.at(-1).receipt.state, 'cancelling'); assert.equal(f.views.at(-1).destroyed, false);
  assert.equal(f.document.activeElement, f.node('#load-progress-details > summary'));
  f.progress('load-a', true); assert.equal(f.views.at(-1).receipt.state, 'cancelling', 'late receipts cannot restart motion');
  f.context.mirrorLoadOverlay('Finalizando', '', null, null, false);
  assert.equal(f.views.at(-1).receipt.state, 'cancelling', 'frontend finalization cannot restart pending-cancel motion');
  f.context.hideLoadOverlay(false); assert.equal(f.views.at(-1).destroyed, true); assert.equal(f.node('#load-overlay').hidden, true);
});

test('cancelLatest before first progress retains exact owner after removing its map entry', () => {
  const f = fixture(); f.show(); f.start('load-a'); f.node('#load-visual .wv-motion-toggle').focus();
  f.context.cancelLatest('source-load');
  assert.equal(f.latest.has('source-load'), false);
  assert.equal(f.views.at(-1).receipt.operationId, 'load-a'); assert.equal(f.views.at(-1).receipt.state, 'cancelling');
  assert.equal(f.document.activeElement, f.node('#load-progress-details > summary'), 'hidden Pause does not strand focus');
});

test('an old source card cannot cancel or display a replacement task with the same latest key', () => {
  const f = fixture(); f.show(); f.start('load-a'); f.progress('load-a');
  f.start('clear-or-replacement'); f.progress('clear-or-replacement');
  assert.equal(f.state.loadOverlayOperationId, 'load-a'); assert.equal(f.views.at(-1).receipt.operationId, 'load-a');
  assert.equal(f.node('#load-cancel').disabled, true, 'replacement start immediately removes stale eligibility');
  f.context.cancelLoadOperation(); assert.equal(f.native.length, 0);
  f.show(); f.start('load-new'); f.progress('load-new');
  f.context.cancel(f.tasks.get('load-a')); f.progress('load-a');
  assert.equal(f.views.at(-1).receipt.operationId, 'load-new'); assert.equal(f.views.at(-1).receipt.state, 'running');
  assert.equal(f.node('#load-cancel').disabled, false);
});

test('timestamp multi-file tasks reset eligibility and reject the previous file cancellation', () => {
  const f = fixture(); f.show('timestamp-config'); f.start('ts-a', 'timestamp-config'); f.progress('ts-a');
  f.start('ts-b', 'timestamp-config');
  assert.equal(f.state.loadOverlayOperationId, 'ts-b'); assert.equal(f.node('#load-cancel').disabled, true);
  assert.equal(f.views.at(-1).receipt.completed, undefined, 'a new file cannot inherit the previous file counters');
  assert.equal(f.views.at(-1).receipt.phaseId, '');
  f.context.cancel(f.tasks.get('ts-a')); f.progress('ts-a');
  assert.equal(f.views.at(-1).receipt.operationId, 'ts-b'); assert.equal(f.views.at(-1).receipt.state, 'running');
  f.progress('ts-b'); f.context.cancelLoadOperation(); assert.equal(f.native.at(-1).args.operationId, 'ts-b');
});

test('native settlement and frontend finalization cannot offer cancellation or target other work', () => {
  const f = fixture(); f.show(); f.start('load-a'); f.progress('load-a');
  f.tasks.delete('load-a'); f.latest.delete('source-load');
  f.state.activeOperation = { kind: 'load' }; f.context.updateOperation('Artefato carregado', 'Sessão em gravação', null, false);
  assert.equal(f.node('#load-cancel').disabled, true); assert.equal(f.state.loadOverlay, true);
  f.context.cancelLoadOperation(); assert.equal(f.native.length, 0);
});

test('settlement restores focus only if the user stayed inside the overlay', () => {
  for (const location of ['inside', 'elsewhere']) {
    const f = fixture(); f.node('#load-trigger').focus(); f.show();
    const current = f.node(location === 'inside' ? '#load-progress-details > summary' : '#elsewhere'); current.focus();
    f.context.hideLoadOverlay(false);
    assert.equal(f.document.activeElement, location === 'inside' ? f.node('#load-trigger') : current);
  }
  const f = fixture(); f.node('#load-trigger').disabled = true; f.node('#load-trigger').focus(); f.show();
  f.node('#load-progress-details > summary').focus(); f.context.hideLoadOverlay(true);
  assert.equal(f.document.activeElement, f.node('.nav-pages button.selected:not([hidden])'), 'a still-disabled trigger gets a visible navigation fallback');
});


test('rejected source task retires Cancel before slow reconciliation, preserving focus and pending state', async () => {
  const drain = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
  for (const location of ['button', 'elsewhere', 'cancel-requested']) {
    const f = fixture({ blurToBody: true });
    Object.assign(f.state, { artifactSwitchVersion: 0, refreshVersion: 0, cases: { active: 'case-a', cases: [{ id: 'case-a', artifacts: [] }] } });
    f.context.window.Tasks.cancelLatest = f.context.cancelLatest;
    let rejectLoad, releaseRecovery, reconciliationStarted = false;
    Object.assign(f.context, {
      skeletonRows() {}, applySourceSpec() {}, toast() {},
      sourceIdentityUnavailable: () => { f.state.sourceIdentityUnconfirmed = true; },
      api: command => {
        assert.equal(command, 'load_file'); f.start('load-error');
        return new Promise((_resolve, reject) => { rejectLoad = reject; }).finally(() => {
          // Tasks removes a settled entry before loadData observes the rejection.
          f.tasks.delete('load-error'); f.latest.delete('source-load');
        });
      },
      reconcilePublishedSource: () => {
        reconciliationStarted = true;
        return new Promise(resolve => { releaseRecovery = resolve; });
      },
    });
    vm.runInContext(section(app, 'async function loadData(', '\nasync function clearData('), f.context);
    f.node('#btn-load').focus();
    const work = f.context.loadData({ kind: 'file', path: '/logs/a.jsonl', paths: ['/logs/a.jsonl'] });
    await drain(); assert.equal(typeof rejectLoad, 'function');
    assert.equal(f.document.activeElement, f.document.body, 'disabling the real trigger can synchronously blur to body');
    assert.equal(vm.runInContext('loadReturnFocus', f.context), f.node('#btn-load'), 'capture the logical load trigger before disabling it');
    f.progress('load-error');
    f.node(location === 'elsewhere' ? '#elsewhere' : '#load-cancel').focus();
    if (location === 'cancel-requested') f.context.cancelLoadOperation();
    const cancelCalls = f.native.length, receiptState = f.views.at(-1).receipt.state;
    rejectLoad(Error('Native load failed')); await drain();
    assert.equal(reconciliationStarted, true);
    assert.equal(f.context.window.Tasks.operationFor('source-load'), null);
    assert.equal(f.state.loadOverlay, true, 'source recovery still owns the visible waiting card');
    assert.equal(f.node('#load-cancel').disabled, true, 'settled native work is no longer offered during recovery');
    assert.equal(f.state.loadOverlayCancellable, false);
    assert.equal(f.document.activeElement, f.node(location === 'elsewhere' ? '#elsewhere' : '#load-progress-details > summary'));
    assert.equal(f.views.at(-1).receipt.state, receiptState, 'retiring Cancel never restarts or settles pending-cancel presentation');
    assert.match(f.node('#load-cancel-help').textContent, location === 'cancel-requested' ? /Aguardando confirmação/ : /indisponível/);
    f.context.cancelLoadOperation(); assert.equal(f.native.length, cancelCalls, 'the settled owner cannot receive another cancel');
    releaseRecovery(false); assert.equal(await work, false);
    assert.equal(f.state.loadOverlay, false, 'normal recovery settlement still closes the card');
    assert.equal(f.views.at(-1).destroyed, true);
  }
});


test('body and non-focusable return candidates cannot swallow settlement focus', () => {
  for (const origin of ['body', 'html', '#not-focusable']) {
    const f = fixture({ blurToBody: true });
    const target = f.node(origin); target.focus = () => {}; f.document.activeElement = target;
    f.show(); f.node('#load-progress-details > summary').focus(); f.context.hideLoadOverlay(false);
    assert.equal(f.document.activeElement, f.node('.nav-pages button.selected:not([hidden])'));
  }
});

test('timestamp configuration captures the logical Apply trigger before btnBusy blurs it', async () => {
  const f = fixture({ blurToBody: true }); let settle;
  Object.assign(f.context, {
    tsConfigPaths: () => ['/logs/a.jsonl'], buildTsConfig: () => ({ sources: ['message'], format: 'unix' }),
    btnBusy: button => { button.disabled = true; return () => { button.disabled = false; }; },
    commitTsConfig: () => new Promise(resolve => { settle = resolve; }), toast() {}, refresh() {},
  });
  vm.runInContext(section(app, 'async function applyTsConfig()', '// ------------------------------------------------------------------ campo derivado'), f.context);
  f.node('#ts-apply').focus(); const work = f.context.applyTsConfig();
  assert.equal(f.document.activeElement, f.document.body);
  assert.equal(vm.runInContext('loadReturnFocus', f.context), f.node('#ts-apply'));
  f.node('#elsewhere').focus(); settle(); await work;
  assert.equal(f.document.activeElement, f.node('#elsewhere'), 'timestamp settlement cannot steal newer outside focus');
});
