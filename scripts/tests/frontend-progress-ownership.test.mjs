import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const listener = source.slice(source.indexOf('const progressSamples ='), source.indexOf('// live-refresh quando'));
const tasks = new Map([
  ['load-new', { operationId: 'load-new', started: 0 }],
  ['count-old', { operationId: 'count-old', started: 0 }],
  ['timestamps', { operationId: 'timestamps', started: 0 }],
]);
const received = [], bars = [];
let callback, owner = 'load-new';
const state = { loadOverlay: true, progressOperationId: null };
const context = vm.createContext({
  state, performance: { now: () => 1000 }, fmtNum: String,
  setWorkbar: (...args) => bars.push(args),
  window: {
    __TAURI__: { event: { listen: (_name, fn) => { callback = fn; return Promise.resolve(); } } },
    Tasks: {
      progress: payload => { received.push(payload.operationId); return tasks.get(payload.operationId); },
      operationFor: key => key === 'source-load' ? owner : key === 'timestamp-config' ? 'timestamps' : null,
    },
    PerformanceTools: {
      estimate: (_old, payload) => ({ completed: payload.completed, total: payload.total, rate: null, eta: null, percent: null }),
      phaseSeconds: payload => Number.isFinite(payload.phaseElapsedMs) ? payload.phaseElapsedMs / 1000 : null,
      duration: value => `${value}s`,
    },
  },
});
vm.runInContext(listener, context);
const emit = (operationId, phase, extra = {}) => callback({ payload: { operationId, phase, phaseId: phase, completed: 10, total: 100, unit: 'registros', cancellable: true, ...extra } });

emit('load-new', 'Retomando metadados');
assert.equal(bars.at(-1)[0], 'Retomando metadados');
assert.equal(state.progressOperationId, 'load-new');
emit('count-old', 'Confirmando filtro anterior');
assert.equal(received.at(-1), 'count-old', 'each task still receives its own detailed progress');
assert.equal(bars.length, 1, 'an unrelated query must not overwrite the active source load overlay');
assert.equal(state.progressOperationId, 'load-new', 'Cancel continues to address the visible source load');
emit(undefined, 'Preparação de índice em segundo plano');
assert.equal(bars.length, 1, 'unowned background progress must not masquerade as foreground loading');

// The native load can settle before the existing session-save acknowledgement.
// Keep the import overlay stable during that remaining frontend finalization.
owner = null;
emit('count-old', 'Contando outra consulta');
assert.equal(bars.length, 1, 'no remaining source task does not grant another task ownership of the overlay');

state.loadOverlay = false;
emit('count-old', 'Contando outra consulta', { unit: 'candidatos', selected: 3, phaseElapsedMs: 2000, total: 0 });
assert.equal(bars.length, 2, 'normal query progress reaches the workbar outside a source load');
assert.equal(state.progressOperationId, 'count-old');
assert.match(bars.at(-1)[1], /2s nesta etapa/);
assert.match(bars.at(-1)[1], /3 selecionados/);
assert.equal(bars.at(-1)[2], null, 'an unknown total has no invented percentage');
emit('replaced', 'Resposta antiga');
assert.equal(bars.length, 2, 'replaced tasks remain ignored');

const nodes = new Map(), timers = [];
context.$ = id => { if (!nodes.has(id)) nodes.set(id, { hidden: false, style: {}, parentElement: {} }); return nodes.get(id); };
context.document = { querySelectorAll: () => [] };
context.pushLoadStep = () => {};
context.setTimeout = fn => timers.push(fn);
vm.runInContext(source.slice(source.indexOf('let loadStepCount ='), source.indexOf('\nfunction pushLoadStep')), context);
context.showLoadOverlay('Aplicando configuração de data/hora', 'timestamp-config');
emit('timestamps', 'Recalculando timestamps');
assert.equal(bars.at(-1)[0], 'Recalculando timestamps', 'timestamp configuration keeps its own foreground progress');
emit('count-old', 'Contagem em segundo plano');
assert.equal(bars.at(-1)[0], 'Recalculando timestamps');
assert.ok(source.includes('config: empty ? null : cfg }, { latest: "timestamp-config" }'), 'timestamp command uses the matching progress owner');
context.hideLoadOverlay(true);
context.showLoadOverlay('Nova fonte');
timers.shift()();
assert.equal(context.$('#load-overlay').hidden, false, 'previous success animation cannot hide a newer load');
vm.runInContext(source.slice(source.indexOf('function updateOperation('), source.indexOf('\nfunction cancelWorkbarTask')), context);
state.activeOperation = { kind: 'load' };
context.updateOperation('Artefato carregado', 'Sessão em gravação', null, false);
assert.equal(bars.at(-1)[3], false, 'frontend finalization must not offer cancellation after native publication');
assert.ok(source.includes('eventos indexados`, null, false)'), 'the successful load continuation marks its commit boundary');
console.log('Progress ownership: source overlay isolation and independent task details passed');
