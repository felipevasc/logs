import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const html = readFileSync(new URL('../../frontend/index.html', import.meta.url), 'utf8');
const section = (start, end) => {
  const from = app.indexOf(start), to = app.indexOf(end, from);
  assert.ok(from >= 0 && to > from, `Source section exists: ${start}`);
  return app.slice(from, to);
};
const plain = value => JSON.parse(JSON.stringify(value));

// Native-like detach/focus model. The actual column handlers, persistence and
// table renderer run here; native Tab/Space/drag behavior belongs to browser CI.
function fixture() {
  let document;
  const listeners = new Map(), store = new Map(), effects = { queries: 0, saves: 0, drawers: 0, notices: [], renders: 0 };
  class Node {
    constructor(tag = 'div', cls = '', text = '') { this.tagName = tag.toUpperCase(); this.className = cls; this.textContent = text; this.children = []; this.dataset = {}; this.style = {}; this.attrs = {}; }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    get classList() { return { add() {}, remove() {}, toggle() {} }; }
    set innerHTML(value) { this.replaceChildren(); this.textContent = value; }
    get innerHTML() { return this.textContent; }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key] ?? null; }
    remove() { if (this.contains(document.activeElement)) document.activeElement = document.body; if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this); this.parentElement = null; }
    append(...children) { for (const child of children) this.appendChild(child); }
    appendChild(child) { if (child.tagName === 'FRAGMENT') { for (const item of [...child.children]) this.appendChild(item); return child; } child.remove(); child.parentElement = this; this.children.push(child); return child; }
    prepend(child) { this.appendChild(child); this.children.unshift(this.children.pop()); }
    replaceChildren(...children) { for (const child of [...this.children]) child.remove(); this.append(...children); }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    matches(selector) { return selector.split(',').some(item => {
      item = item.trim(); const parts = item.split(/\s+/), last = parts.pop();
      if (parts.length) { if (!this.matches(last)) return false; let parent = this.parentElement; while (parent && !parent.matches(parts.join(' '))) parent = parent.parentElement; return !!parent; }
      if (item === '[hidden]') return !!this.hidden;
      if (item === '[inert]') return !!this.inert;
      if (item === '[aria-hidden="true"]') return this.attrs['aria-hidden'] === 'true';
      if (item === 'input:not(:disabled)') return this.tagName === 'INPUT' && !this.disabled;
      if (item.startsWith('#')) return this.id === item.slice(1);
      if (item.startsWith('.')) return this.className.split(' ').includes(item.slice(1));
      return this.tagName.toLowerCase() === item;
    }); }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    getClientRects() { return this.isConnected && !this.closest('[hidden],[inert],[aria-hidden="true"]') ? [{}] : []; }
    focus() { document.activeElement = this; }
  }
  document = { createElement: tag => new Node(tag), createDocumentFragment: () => new Node('fragment'),
    addEventListener(type, callback) { listeners.set(type, [...listeners.get(type) || [], callback]); },
  };
  document.body = new Node('body'); document.activeElement = document.body;
  const node = (tag, id = '', parent = document.body, cls = '') => { const item = new Node(tag, cls); item.id = id; parent.append(item); return item; };
  const table = node('table', 'events-table'), head = node('thead', '', table), body = node('tbody', '', table);
  const trigger = node('button', 'btn-colpicker'), pop = node('div', 'col-pop'); pop.hidden = true; pop.tabIndex = -1;
  trigger.setAttribute('aria-expanded', 'false'); node('div', 'col-list', pop);
  node('p', '', node('div', 'empty-state')); node('a', 'skip-records'); node('select', 'page-size'); node('input', 'quick-search');
  for (const id of ['drawer', 'filter-pop', 'name-pop', 'detail-value-modal', 'codes-modal', 'settings-modal', 'format-modal', 'derive-modal', 'chart-modal', 'ts-modal', 'manual-form', 'case-add-modal', 'case-item-modal']) node('div', id).hidden = true;
  const $ = selector => document.body.querySelector(selector), artifact = {};
  const rows = [{ id: 1, event_ref: 'source:1', fields: {} }, { id: 2, event_ref: 'source:2', fields: {} }];
  const state = { columns: ['timestamp', 'level', 'message', 'host'], visibleCols: ['timestamp', 'host', 'message'], colWidths: { host: 172, message: 300 }, rows, total: 2, loaded: true,
    cases: { active: 'case-a' }, currentArtifact: { id: 'source-a' }, selectedEventRows: new Map([[1, rows[0]]]), page: 3, sortCol: 'timestamp', sortDir: 'desc' };
  let scope = 'dataset';
  const context = vm.createContext({ state, document, $, window: {}, el: (tag, cls, text) => new Node(tag, cls, text), colLabel: String,
    workspaceScope: () => scope, currentCaseArtifact: () => artifact,
    localStorage: { getItem: key => store.get(key) ?? null, setItem: (key, value) => { effects.saves++; store.set(key, value); } },
    getComputedStyle: () => ({ visibility: 'visible' }), positionPop() {}, toast: message => effects.notices.push(message),
    api() { effects.queries++; throw Error('Column changes must not query'); }, refresh() { effects.queries++; },
    ensureSelectionOwner() {}, recordContextKey: () => 'owner-a', recordTargetMatches: (target, event) => target.event === event,
    buildEventRow(event, columns = state.visibleCols, { recordActions = false, recordKey = null } = {}) {
      const row = new Node('tr'); row.dataset.eventId = String(event.id);
      if (recordActions) { const cell = node('td', '', row, 'record-actions-cell'), button = node('button', '', cell, 'record-actions-trigger'); button._recordTarget = { event, key: recordKey }; }
      for (const column of columns) { const cell = node('td', '', row); cell.dataset.column = column; }
      return row;
    }, updatePager() { effects.renders++; }, focusRecordTarget() {},
    ctxEl: null, tlPop: null, namePopExact: false, closeCtxMenu() {}, closeTlPop() {}, closeDrawer() { effects.drawers++; $('#drawer').hidden = true; },
    closeFilterPop() { $('#filter-pop').hidden = true; }, closeNamePop() { $('#name-pop').hidden = true; }, closeDetailValue() {}, closeCaseNameInput() {},
    detailStep() {}, pendingCaseAdd: null, editingCaseItem: null,
  });
  vm.runInContext(section('function visiblePreferenceKey()', '\nasync function removeArtifact'), context);
  vm.runInContext(section('function renderTable(', '// ------------------------------------------------------------------ histograma'), context);
  vm.runInContext(section('function setColumnVisible(', '// ------------------------------------------------------------------ drawer'), context);
  vm.runInContext(section('function toggleDetailColumn(', 'function showDetailNameMenu('), context);
  vm.runInContext(section('function bindKeyboard()', '// ------------------------------------------------------------------ init'), context);
  vm.runInContext(section('  $("#btn-colpicker").onclick =', '  $("#page-size").onchange ='), context);
  context.bindKeyboard(); context.renderTable({ total: state.total, rows }, { reuseRows: true }); effects.renders = 0;
  const input = column => $('#col-list').children[state.columns.indexOf(column)]?.querySelector('input');
  const event = (target, extra = {}) => ({ target, prevented: false, stopped: false, preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; }, stopImmediatePropagation() { this.stopped = true; }, ...extra });
  const dispatch = (type, ev) => { for (const callback of listeners.get(type) || []) { callback(ev); if (ev.stopped) break; } return ev; };
  const toggle = (column, checked) => { const checkbox = input(column); checkbox.checked = checked; checkbox.onchange(); };
  return { context, state, artifact, effects, rows, store, document, $, head, body, input, event, dispatch, toggle, scope: value => { scope = value; } };
}

test('checkbox add/remove preserves dragged order and existing record DOM, selection, widths and paging', () => {
  const f = fixture(); f.state.visibleCols = ['timestamp', 'message', 'host'];
  f.context.renderTable({ total: f.state.total, rows: f.rows }, { reuseRows: true });
  const [row] = f.body.children, action = row.querySelector('.record-actions-trigger'), host = row.children.find(cell => cell.dataset.column === 'host');
  const target = f.head.children[0].children.find(header => header.dataset.column === 'message');
  target.ondrop(f.event(target, { dataTransfer: { getData: type => type === 'text/col' ? 'host' : '' } }));
  assert.deepEqual(plain(f.state.visibleCols), ['timestamp', 'host', 'message']);
  f.context.openColPop(); const focused = f.document.activeElement;
  f.toggle('level', true); assert.deepEqual(plain(f.state.visibleCols), ['timestamp', 'host', 'message', 'level']);
  f.toggle('message', false); assert.deepEqual(plain(f.state.visibleCols), ['timestamp', 'host', 'level']);
  assert.equal(f.body.children[0], row); assert.equal(row.querySelector('.record-actions-trigger'), action); assert.equal(row.children.find(cell => cell.dataset.column === 'host'), host);
  assert.equal(f.document.activeElement, focused); assert.deepEqual([...f.state.selectedEventRows.keys()], [1]);
  assert.deepEqual(f.state.colWidths, { host: 172, message: 300 }); assert.equal(f.state.page, 3); assert.equal(f.effects.queries, 0);
  assert.deepEqual(row.children.slice(1).map(cell => cell.dataset.column), ['timestamp', 'host', 'level']);
});

test('detail column actions share order semantics, append at end, and retain timestamp without pinning it', () => {
  const f = fixture(); f.state.visibleCols = ['host', 'timestamp', 'message'];
  f.context.toggleDetailColumn('level'); assert.deepEqual(plain(f.state.visibleCols), ['host', 'timestamp', 'message', 'level']);
  f.context.toggleDetailColumn('message'); assert.deepEqual(plain(f.state.visibleCols), ['host', 'timestamp', 'level']);
  const saves = f.effects.saves; f.context.toggleDetailColumn('timestamp'); f.context.toggleDetailColumn('unknown');
  assert.deepEqual(plain(f.state.visibleCols), ['host', 'timestamp', 'level']); assert.equal(f.effects.saves, saves); assert.equal(f.effects.queries, 0);
});

test('ordered visibility and widths persist within the existing Case/source key only', () => {
  const f = fixture(); f.context.toggleDetailColumn('level');
  f.state.visibleCols = ['timestamp']; f.state.colWidths = {}; assert.equal(f.context.restoreVisiblePreferences(), true);
  assert.deepEqual(plain(f.state.visibleCols), ['timestamp', 'host', 'message', 'level']); assert.deepEqual(plain(f.state.colWidths), { host: 172, message: 300 });
  assert.deepEqual(plain(f.artifact.visibleCols), plain(f.state.visibleCols));
  f.state.currentArtifact.id = 'source-b'; f.state.visibleCols = ['message']; f.context.restoreVisiblePreferences(); assert.deepEqual(f.state.visibleCols, ['message']);
  f.scope('case'); f.context.restoreVisiblePreferences(); assert.deepEqual(f.state.visibleCols, ['message']); assert.equal(f.effects.queries, 0);
});

test('repeat show/hide and unknown fields do not write or rerender unnecessarily', () => {
  const f = fixture(); assert.equal(f.context.setColumnVisible('host', true), false); assert.equal(f.context.setColumnVisible('level', false), false);
  assert.equal(f.context.setColumnVisible('unknown', true), false); assert.equal(f.context.setColumnVisible('timestamp', false), false);
  assert.equal(f.effects.saves, 0); assert.equal(f.effects.renders, 0); assert.equal(f.effects.queries, 0);
});

test('open focuses the first enabled native checkbox and Escape closes Columns alone with focus return', () => {
  const f = fixture(); f.$('#drawer').hidden = false; f.$('#btn-colpicker').focus(); f.$('#btn-colpicker').onclick(f.event(f.$('#btn-colpicker')));
  assert.equal(f.$('#col-pop').hidden, false); assert.equal(f.$('#btn-colpicker').getAttribute('aria-expanded'), 'true'); assert.equal(f.document.activeElement, f.input('level'));
  assert.equal(f.input('timestamp').disabled, true); assert.doesNotMatch(f.input('timestamp').parentElement.title, /primeira/);
  const key = f.dispatch('keydown', f.event(f.document.activeElement, { key: 'Escape' }));
  assert.equal(key.prevented, true); assert.equal(f.$('#col-pop').hidden, true); assert.equal(f.$('#btn-colpicker').getAttribute('aria-expanded'), 'false');
  assert.equal(f.document.activeElement, f.$('#btn-colpicker')); assert.equal(f.$('#drawer').hidden, false); assert.equal(f.effects.drawers, 0); assert.equal(f.effects.queries, 0);
});

test('outside click dismisses without stealing focus, while trigger toggle and repeated cycles keep state coherent', () => {
  const f = fixture();
  for (let count = 0; count < 3; count++) {
    f.context.openColPop(); f.$('#quick-search').focus(); f.dispatch('click', f.event(f.$('#quick-search')));
    assert.equal(f.$('#col-pop').hidden, true); assert.equal(f.document.activeElement, f.$('#quick-search')); assert.equal(f.$('#btn-colpicker').getAttribute('aria-expanded'), 'false');
    f.$('#btn-colpicker').focus(); f.$('#btn-colpicker').onclick(f.event(f.$('#btn-colpicker')));
    f.$('#btn-colpicker').focus(); f.$('#btn-colpicker').onclick(f.event(f.$('#btn-colpicker')));
    assert.equal(f.$('#col-pop').hidden, true); assert.equal(f.document.activeElement, f.$('#btn-colpicker'));
  }
});

test('empty/disabled-only checklist has a focus target; return never focuses a hidden, disabled or removed trigger', () => {
  for (const columns of [[], ['timestamp']]) {
    const f = fixture(); f.state.columns = columns; f.context.openColPop(); assert.equal(f.document.activeElement, f.$('#col-pop'));
    f.context.closeColPop(); assert.equal(f.document.activeElement, f.$('#btn-colpicker'));
  }
  for (const change of ['hidden', 'disabled', 'removed']) {
    const f = fixture(); f.context.openColPop(); const trigger = f.$('#btn-colpicker');
    if (change === 'removed') trigger.remove(); else trigger[change] = true;
    assert.doesNotThrow(() => f.context.closeColPop()); assert.notEqual(f.document.activeElement, trigger);
  }
});

test('Escape during composition does not dismiss Columns', () => {
  const f = fixture(); f.context.openColPop(); f.dispatch('keydown', f.event(f.document.activeElement, { key: 'Escape', isComposing: true }));
  assert.equal(f.$('#col-pop').hidden, false);
});

test('Columns markup keeps native controls and identifies its open/closed relationship', () => {
  const trigger = html.match(/<button\b[^>]*\bid="btn-colpicker"[^>]*>/)?.[0];
  assert.match(trigger, /aria-controls="col-pop"/); assert.match(trigger, /aria-expanded="false"/);
  const pop = html.match(/<div\b[^>]*\bid="col-pop"[^>]*>/)?.[0]; assert.match(pop, /tabindex="-1"/); assert.match(pop, /aria-labelledby="col-pop-title"/);
  assert.match(html, /id="col-pop-title"/); assert.doesNotMatch(pop, /aria-modal="true"|role="menu"/);
});
