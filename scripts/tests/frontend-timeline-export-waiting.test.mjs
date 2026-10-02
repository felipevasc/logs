import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/timeline-export.js', import.meta.url), 'utf8');
const ui = source.slice(source.indexOf('  // Decorate only'), source.indexOf('  function attach('));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const result = () => ({ blob: {}, pages: 3, width: 600, height: 900 });
const drawing = (completed = 0, total = 3, format = 'pdf') => ({ label: format === 'pdf' ? 'Desenhando páginas…' : 'Desenhando imagem completa…', completed, total, unit: format === 'pdf' ? 'páginas desenhadas' : 'imagem desenhada' });
const flush = async () => { for (let index = 0; index < 8; index++) await Promise.resolve(); };

function fixture({ lateness = 0, visual = true } = {}) {
  let now = 0, serial = 0;
  const timers = new Map(), mounted = [], renders = [], saves = [], toasts = [];
  class Node {
    constructor() { this.children = []; this.hidden = false; this.textContent = ''; this.classes = new Set(); this.classList = { add: name => this.classes.add(name), remove: name => this.classes.delete(name) }; }
    append(child) { child.parent = this; this.children.push(child); }
    remove() { this.parent.children = this.parent.children.filter(child => child !== this); this.parent = null; }
    removeAttribute(name) { delete this[name]; }
    get isConnected() { return this === document.body || !!this.parent?.isConnected; }
    set innerHTML(value) {
      this.markup = value; this.parts = Object.fromEntries(['[data-tx-status]', '[data-tx-waiting]', '[data-tx-save]', '[data-tx-cancel]', 'progress'].map(selector => { const child = new Node(); this.append(child); return [selector, child]; }));
      this.inputs = ['png', 'pdf'].map(value => { const child = new Node(); child.value = value; this.append(child); return child; });
      this.inputs[0].checked = true; this.parts['[data-tx-waiting]'].hidden = true; this.parts.progress.hidden = true;
    }
    querySelector(selector) { return selector === 'input:checked' ? this.inputs.find(input => input.checked) : this.parts[selector]; }
    querySelectorAll(selector) { return selector === 'input' ? this.inputs : [...this.inputs, ...this.children.filter(child => child.isToggle), this.parts['[data-tx-cancel]'], this.parts['[data-tx-save]']]; }
    focus(options) { document.activeElement = this; this.focuses = (this.focuses || 0) + 1; this.focusOptions = options; }
  }
  const document = {}; document.body = new Node(); const origin = new Node(); document.body.append(origin); document.activeElement = origin;
  const render = (spec, options) => { const task = deferred(); renders.push({ ...task, spec, options }); options.progress(0, 2, 'Preparando fontes e desenho…', { label: 'Preparando fontes e desenho…' }); return task.promise; };
  const save = (blob, spec) => { const task = deferred(); saves.push({ ...task, blob, spec }); return task.promise; };
  const toast = (...args) => toasts.push(args);
  const window = { toast, ...(visual ? { WaitingVisuals: { mount(host, receipt) {
    const toggle = new Node(); toggle.isToggle = true; host.parent.append(toggle);
    const view = { host, toggle, receipts: [receipt], destroyed: false, update(value) { this.receipts.push(value); }, destroy() { this.destroyed = true; toggle.remove(); } }; mounted.push(view); return view;
  } } } : {}) };
  const context = vm.createContext({ window, document, AbortController, DOMException, performance: { now: () => now },
    text: () => new Node(), toast, render, save, MAX_PAGES: 500,
    measure: spec => ({ source: spec.source }), pngPlan: () => ({ allowed: true, width: 600, height: 900, ratio: 1 }), planPages: () => [{}, {}],
    abort: signal => { if (signal?.aborted) throw new DOMException('Exportação cancelada', 'AbortError'); },
    setTimeout: (fn, ms) => { const id = ++serial; timers.set(id, { fn, at: now + Math.trunc(ms) + (id === 1 ? lateness : 0) }); return id; }, clearTimeout: id => timers.delete(id)
  });
  vm.runInContext(`let dialog, waitingSerial=0; ${ui}; window.ExportUI={open};`, context);
  const advance = to => { for (;;) { const next = [...timers].filter(([, task]) => task.at <= to).sort((a, b) => a[1].at - b[1].at)[0]; if (!next) break; timers.delete(next[0]); now = next[1].at; next[1].fn(); } now = to; };
  const open = (spec = { source: origin, type: 'matrix', title: 'Captured timeline', filename: 'captured-timeline' }) => { window.ExportUI.open(spec, origin); return document.body.children.at(-1); };
  return { open, advance, mounted, timers, renders, saves, toasts, origin, document };
}

test('fast render never flashes or retains its result for animation; saving is distinct', async () => {
  const f = fixture(), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick();
  assert.equal(modal.querySelector('progress').value, undefined, 'preparation has no final denominator');
  f.renders[0].resolve(result()); await flush();
  assert.equal(f.mounted.length, 0); assert.equal(f.timers.size, 0); assert.equal(f.saves.length, 1);
  assert.equal(modal.querySelector('[data-tx-status]').textContent, 'Escolha onde salvar o arquivo…');
  assert.equal(modal.querySelector('progress').hidden, true);
  f.saves[0].resolve({ saved: false }); await run; f.advance(10000);
  assert.equal(f.mounted.length, 0); assert.equal(modal.querySelector('[data-tx-status]').textContent, 'Salvamento cancelado. A timeline continua disponível.');
  assert.equal(modal.querySelector('[data-tx-save]').disabled, false);
});

test('one attempt uses real stage/count receipts, one measured 4 s threshold and no polling', async () => {
  const f = fixture({ lateness: .6 }), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick();
  f.advance(249); assert.equal(f.mounted.length, 0); f.advance(251); const view = f.mounted[0];
  assert.equal(view.receipts[0].phaseId, 'command:timeline_export_render'); assert.equal(view.receipts[0].elapsedMs, 250.6);
  assert.equal(view.receipts[0].total, undefined); assert.equal(modal.querySelector('[data-tx-status]').hidden, true);
  assert.equal(modal.querySelector('progress').hidden, true);
  f.renders[0].options.progress(1, 3, 'Desenhando página 2 de 3…', drawing(1, 3));
  assert.equal(view.receipts.at(-1).completed, 1); assert.equal(view.receipts.at(-1).total, 3); assert.equal(view.receipts.at(-1).label, 'Desenhando páginas…');
  const count = view.receipts.length; f.advance(3999); assert.equal(view.receipts.length, count); f.advance(4001);
  assert.ok(view.receipts.at(-1).elapsedMs >= 4000); assert.ok(view.receipts.at(-1).elapsedMs < 4001);
  f.advance(10000); assert.equal(view.receipts.length, count + 1); assert.equal(f.timers.size, 0);
  f.renders[0].options.progress(3, 3, 'Arquivo pronto.', { label: 'Arquivo pronto.' });
  assert.equal(view.receipts.at(-1).total, undefined); assert.equal(view.receipts.at(-1).completed, undefined);
  assert.ok(view.receipts.every(receipt => receipt.operationId === view.receipts[0].operationId));
  assert.ok(view.receipts.every(receipt => !('phaseCount' in receipt) && !('estimateMs' in receipt)));
  f.renders[0].resolve(result()); await flush(); assert.equal(view.destroyed, true);
  assert.equal(modal.querySelector('[data-tx-waiting]').hidden, true); assert.equal(modal.querySelector('[data-tx-status]').hidden, false);
  f.saves[0].resolve({ saved: true }); await run; assert.equal(f.toasts.length, 1); assert.equal(f.origin.focuses, 1);
});

test('PNG has one image count rather than a global percentage or estimated page total', async () => {
  const f = fixture(), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick();
  f.renders[0].options.progress(0, 1, 'Desenhando imagem completa…', drawing(0, 1, 'png')); f.advance(300);
  const receipt = f.mounted[0].receipts.at(-1); assert.equal(receipt.total, 1); assert.equal(receipt.completed, 0); assert.equal(receipt.unit, 'imagem desenhada');
  assert.doesNotMatch(receipt.label, /%|página/); modal.querySelector('[data-tx-cancel]').onclick(); f.renders[0].resolve(result()); await run;
});

test('Cancel and Escape abort exact attempt before/after admission; late callback or result never revives or saves', async () => {
  for (const mounted of [false, true]) for (const escape of [false, true]) {
    const f = fixture(), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick();
    if (mounted) f.advance(300);
    if (escape) modal.onkeydown({ key: 'Escape', preventDefault() {}, stopPropagation() {} }); else modal.querySelector('[data-tx-cancel]').onclick();
    const status = modal.querySelector('[data-tx-status]').textContent;
    assert.equal(f.renders[0].options.signal.aborted, true); assert.equal(modal.isConnected, false); assert.equal(f.timers.size, 0);
    if (mounted) assert.equal(f.mounted[0].destroyed, true);
    f.renders[0].options.progress(2, 3, 'Stale progress', drawing(2)); f.renders[0].resolve(result()); await run; f.advance(10000);
    assert.equal(modal.querySelector('[data-tx-status]').textContent, status); assert.equal(f.saves.length, 0); assert.equal(f.toasts.length, 0);
  }
});

test('close A, open B, then A settles does not replace B owner or render receipt', async () => {
  const f = fixture(), a = f.open(), first = a.querySelector('[data-tx-save]').onclick(); f.advance(300);
  a.querySelector('[data-tx-cancel]').onclick(); const b = f.open({ source: f.origin, type: 'activity', title: 'B' }), second = b.querySelector('[data-tx-save]').onclick();
  f.advance(600); const view = f.mounted[1], receipt = view.receipts.at(-1);
  assert.notEqual(receipt.operationId, f.mounted[0].receipts[0].operationId);
  f.renders[0].options.progress(99, 99, 'Stale A', drawing(99, 99)); f.renders[0].resolve(result()); await first;
  assert.equal(b.isConnected, true); assert.equal(view.destroyed, false); assert.equal(view.receipts.at(-1), receipt); assert.equal(f.saves.length, 0);
  f.renders[1].resolve(result()); await flush(); assert.equal(f.saves[0].spec.title, 'B'); f.saves[0].resolve({ saved: true }); await second;
});

test('error and retry keep original errors, fresh operation IDs and an inert repeated click', async () => {
  const f = fixture(), modal = f.open(), button = modal.querySelector('[data-tx-save]'), run = button.onclick(); await button.onclick();
  assert.equal(f.renders.length, 1); f.advance(300); const first = f.mounted[0]; f.renders[0].reject(Error('Falha de desenho.')); await run;
  assert.equal(first.destroyed, true); assert.equal(modal.querySelector('[data-tx-status]').textContent, 'Falha de desenho.'); assert.ok(modal.querySelector('[data-tx-status]').classes.has('error'));
  assert.equal(f.timers.size, 0); assert.equal(button.disabled, false);
  modal.inputs[0].checked = false; modal.inputs[1].checked = true;
  const retry = button.onclick(); f.advance(600); assert.notEqual(f.mounted[1].receipts[0].operationId, first.receipts[0].operationId);
  assert.equal(f.renders[1].spec.format, 'pdf'); assert.equal(f.mounted[1].receipts[0].total, undefined);
  f.renders[0].options.progress(9, 9, 'Old error retry receipt', drawing(9, 9)); assert.equal(f.mounted[1].receipts.length, 1);
  modal.querySelector('[data-tx-cancel]').onclick(); f.renders[1].reject(new DOMException('cancelled', 'AbortError')); await retry; assert.equal(f.saves.length, 0);
});

test('save/write guard stays intact and receipt is required before success', async () => {
  const f = fixture(), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick(); f.advance(300); f.renders[0].resolve(result()); await flush();
  modal.querySelector('[data-tx-cancel]').onclick(); modal.onkeydown({ key: 'Escape', preventDefault() {}, stopPropagation() {} });
  assert.equal(modal.isConnected, true); assert.equal(f.mounted[0].destroyed, true); assert.equal(f.toasts.length, 0);
  assert.equal(modal.querySelector('[data-tx-cancel]').disabled, true); assert.equal(f.saves[0].spec.filename, 'captured-timeline');
  f.saves[0].reject(Error('Não foi possível gravar.')); await run;
  assert.equal(modal.querySelector('[data-tx-status]').textContent, 'Não foi possível gravar.'); assert.equal(f.toasts.length, 0); assert.equal(f.timers.size, 0);
  modal.querySelector('[data-tx-cancel]').onclick(); assert.equal(modal.isConnected, false);
});

test('removed host cannot start save and missing visuals retain real compact status', async () => {
  for (const visual of [false, true]) {
    const f = fixture({ visual }), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick();
    if (visual) modal.remove(); f.advance(10000); assert.equal(f.mounted.length, 0); assert.equal(f.timers.size, 0);
    if (!visual) assert.equal(modal.querySelector('[data-tx-status]').textContent, 'Preparando fontes e desenho…');
    f.renders[0].resolve(result()); await flush(); if (!visual) f.saves[0].resolve({ saved: false }); await run;
    assert.equal(f.saves.length, visual ? 0 : 1);
  }
});

test('focus trap includes visible motion toggle and excludes it under reduced motion', async () => {
  const f = fixture(), modal = f.open(), run = modal.querySelector('[data-tx-save]').onclick(); f.advance(300);
  const toggle = f.mounted[0].toggle, cancel = modal.querySelector('[data-tx-cancel]'); cancel.focus();
  modal.onkeydown({ key: 'Tab', shiftKey: false, preventDefault() {} }); assert.equal(f.document.activeElement, toggle);
  toggle.hidden = true; cancel.focus(); modal.onkeydown({ key: 'Tab', shiftKey: true, preventDefault() {} }); assert.equal(f.document.activeElement, cancel);
  cancel.onclick(); f.renders[0].resolve(result()); await run;
});

test('adapter does not parse stage strings, use polling, or alter source/filter/snapshot/native contracts', () => {
  assert.doesNotMatch(ui, /setInterval|requestAnimationFrame|phaseCount|estimateMs|parseInt|\.match\(/);
  assert.match(source, /current\.board !== info\.board \|\| current\.board\.firstElementChild !== firstChild/);
  assert.match(source, /const frozen = snapshot\(info\)/);
  assert.match(source, /core\.invoke\("export_timeline", \{ format: spec\.format, filename, base64: await base64\(blob\) \}\)/);
  const exports = source.slice(source.lastIndexOf('  return { attach'));
  assert.doesNotMatch(exports, /renderWaiting/);
});

// Execute the real render loop with only layout/raster/PDF dependencies replaced.
// This specifically covers appendices changing the final task denominator.
async function renderedFixture(format, { appendix = false, failCanvas = false, cancelCanvas = false } = {}) {
  const controller = new AbortController(), callbacks = [], canvases = [], stages = [];
  let drawn = 0;
  class Node {
    constructor(className = '') { this.className = className; this.children = []; this.style = {}; this.offsetWidth = 600; this.offsetHeight = 500; }
    append(child) { this.children.push(child); }
    prepend(child) { this.children.unshift(child); }
    insertBefore(child) { this.children.push(child); }
    replaceChildren(child) { this.children = [child]; }
    setAttribute() {}
    remove() { this.removed = true; }
  }
  class PDF {
    constructor() { this.pages = 1; this.internal = { pageSize: { getWidth: () => 800, getHeight: () => 600 } }; }
    setProperties() {} addPage() { this.pages++; } addImage() { drawn++; }
    output() { return { size: 120 }; } getNumberOfPages() { return this.pages; }
  }
  const board = { firstElementChild: {} }, info = { board, width: 600, height: 500, matrix: false };
  const context = vm.createContext({ window: { jspdf: { jsPDF: PDF }, htmlToImage: { toCanvas: async () => {
    if (failCanvas) throw Error('Canvas rejected'); if (cancelCanvas) controller.abort();
    const canvas = { width: 600, height: 500, toBlob(fn) { drawn++; fn({ size: 120 }); } }; canvases.push(canvas); return canvas;
  } } }, document: { body: { append: node => stages.push(node) }, fonts: { ready: Promise.resolve() } },
    MAX_PAGES: 500, MAX_BYTES: 32 * 1024 * 1024, MAX_EDGE: 16000, MAX_PIXELS: 24000000, HEADER: 84, FOOTER: 26, PDF_WIDTH: 1060,
    abort: signal => { if (signal?.aborted) throw new DOMException('Exportação cancelada', 'AbortError'); },
    measure: spec => spec.source?.className === 'tx-note-appendix' ? { ...info, appendix: true } : info,
    pngPlan: () => ({ allowed: true }), planPages: value => Array.from({ length: value.appendix ? 1 : 2 }, () => ({ width: 600, height: 500 })),
    snapshot: value => ({ notes: appendix && !value.appendix ? ['Full note'] : [] }), text: (tag, className) => new Node(className),
    pageNode: () => new Node(), loadLibraries: async () => '', nextFrame: async () => {}, getComputedStyle: () => ({ backgroundColor: '#fff' })
  });
  vm.runInContext(`${source.slice(source.indexOf('  async function render('), source.indexOf('  const base64'))}; window.renderTest=render;`, context);
  const task = context.window.renderTest({ format }, { signal: controller.signal, progress(completed, total, message, detail) {
    callbacks.push({ completed, total, message, detail: { ...detail } });
    if (detail.completed !== undefined) assert.equal(detail.completed, drawn, 'counts represent actual completed drawings');
  } });
  if (failCanvas || cancelCanvas) await assert.rejects(task, failCanvas ? /Canvas rejected/ : { name: 'AbortError' });
  else { const result = await task; assert.equal(result.pages, format === 'png' ? 1 : appendix ? 3 : 2); }
  assert.ok(stages.every(stage => stage.removed), 'render stages are cleaned after success, failure and cancellation');
  if (!cancelCanvas) assert.ok(canvases.every(canvas => canvas.width === 1 && canvas.height === 1));
  return callbacks;
}

test('real PDF drawing receipts use appended final tasks while preserving legacy callback arguments', async () => {
  const receipts = await renderedFixture('pdf', { appendix: true });
  assert.equal(receipts[0].completed, 0); assert.equal(receipts[0].total, 2); assert.equal(receipts[0].detail.total, undefined);
  assert.deepEqual(receipts.slice(1, -1).map(receipt => receipt.detail), [drawing(0, 3), drawing(1, 3), drawing(2, 3)]);
  assert.deepEqual(receipts.slice(1, -1).map(receipt => receipt.message), ['Desenhando página 1 de 3…', 'Desenhando página 2 de 3…', 'Desenhando página 3 de 3…']);
  assert.equal(receipts.at(-1).completed, 3); assert.equal(receipts.at(-1).total, 3); assert.equal(receipts.at(-1).detail.total, undefined);
});

test('real PNG drawing, cancellation and error keep existing render semantics', async () => {
  const receipts = await renderedFixture('png', { appendix: true }); assert.deepEqual(receipts[1].detail, drawing(0, 1, 'png'));
  assert.equal(receipts.at(-1).message, 'Arquivo pronto.');
  await renderedFixture('pdf', { failCanvas: true }); await renderedFixture('png', { cancelCanvas: true });
});

test('Timeline adapter reuses the same composition family with static unknown/save phases', () => {
  const context = vm.createContext({ window: {}, Intl });
  vm.runInContext(readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8'), context);
  const { derive } = context.window.WaitingVisuals;
  assert.equal(derive({ operationId: 'timeline-export-1', phaseId: 'command:timeline_export_render', state: 'running' }).family, 'composition');
  assert.equal(derive({ operationId: 'case-report-1', phaseId: 'command:case_report_render', state: 'running' }).family, 'composition');
  assert.equal(derive({ operationId: 'timeline-export-1', phaseId: 'command:export_timeline', state: 'running' }).family, 'neutral');
  assert.equal(derive({ operationId: 'timeline-export-1', phaseId: 'command:timeline_export_render', state: 'cancelling' }).family, 'neutral');
});
