import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const discovery = readFileSync(new URL('../../frontend/discovery.js', import.meta.url), 'utf8');
const section = (start, end) => app.slice(app.indexOf(start), app.indexOf(end, app.indexOf(start)));
const plain = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

// No browser/socket: model native detach/focus and run the actual row renderer,
// identity guards and detail admission. Browser coverage owns Tab and geometry.
function fixture(count = 5) {
  const listeners = new Map(), messages = [], menus = [], effects = [], requests = [];
  let document, scope = 'dataset', signature = 'members-a';
  class Element {
    constructor(tag, cls = '', text = '') { this.tagName = tag.toUpperCase(); this.className = cls; this.textContent = text; this.children = []; this.dataset = {}; this.style = {}; this.attrs = {}; }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    get classList() { const node = this; return { add(...names) { node.className = [...new Set([...node.className.split(' '), ...names])].join(' '); }, remove(name) { node.className = node.className.split(' ').filter(value => value !== name).join(' '); }, toggle(name, on) { on ? this.add(name) : this.remove(name); } }; }
    get innerHTML() { return this.textContent; }
    set innerHTML(value) { this.replaceChildren(); this.textContent = value; }
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
      if (item === '[data-column]') return Object.hasOwn(this.dataset, 'column');
      if (item.endsWith('[data-column]')) return this.matches(item.slice(0, -13)) && this.matches('[data-column]');
      const attribute = item.match(/^\[([^=]+)=["']([^"']+)["']\]$/);
      if (attribute) return this.getAttribute(attribute[1]) === attribute[2];
      if (item.startsWith('#')) return this.id === item.slice(1);
      if (item.startsWith('.')) return this.className.split(' ').includes(item.slice(1));
      return this.tagName.toLowerCase() === item;
    }); }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    getClientRects() { return this.isConnected && !this.closest('[hidden],[inert]') ? [this.getBoundingClientRect()] : []; }
    getBoundingClientRect() { return { left: 0, bottom: 32, width: 100, height: 32 }; }
    focus() { document.activeElement = this; this.onfocus?.(); }
  }
  document = { createElement: tag => new Element(tag), createDocumentFragment: () => new Element('fragment'),
    querySelectorAll: selector => document.body.querySelectorAll(selector),
    addEventListener: (type, fn) => { const list = listeners.get(type) || []; list.push(fn); listeners.set(type, list); },
    removeEventListener: (type, fn) => listeners.set(type, (listeners.get(type) || []).filter(item => item !== fn)),
  };
  document.body = new Element('body'); document.activeElement = document.body;
  const node = (tag, id, parent = document.body, cls = '') => { const item = new Element(tag, cls); item.id = id; parent.append(item); return item; };
  const table = node('table', 'events-table'), head = node('thead', '', table), body = node('tbody', '', table);
  node('p', '', node('div', 'empty-state')); node('a', 'skip-records'); node('select', 'page-size'); node('input', 'quick-search'); node('button', 'btn-add-filter');
  const drawer = node('aside', 'drawer'); drawer.hidden = true; node('button', 'dr-close', drawer);
  node('div', 'drawer-badges', drawer); node('div', 'pane-overview', drawer); node('pre', 'pane-json', drawer); node('pre', 'pane-raw', drawer);
  for (const id of ['dr-prev', 'dr-next', 'dr-copy']) node('button', id, drawer);
  node('div', 'drawer-scrim'); node('button', 'btn-right-inspect'); node('div', 'detail-value-modal').hidden = true;
  const $ = selector => document.body.querySelector(selector);
  const rows = Array.from({ length: count }, (_, id) => ({ id, event_ref: `source:${id}`, message: `Full message ${id}`, fields: { exact: '  original  ' }, level: 'Informação' }));
  const state = { rows, loaded: true, quick: '', visibleCols: ['message', 'exact'], colWidths: { message: 100 }, cases: { active: 'a' }, selectedEventRows: new Map(), datasetRevision: 1,
    currentArtifact: { id: 'source', loadedAt: 1 }, owner: { caseId: 'a', instance: 1, identity: { analysisId: 'analysis-a', configRevision: 0, visibilityRevision: 0 }, sourceGeneration: 1, sourceKey: 'source-a' } };
  const window = { AnalysisContexts: { capture: () => structuredClone(state.owner), isCurrent: owner => JSON.stringify(owner) === JSON.stringify(state.owner) },
    Tasks: { cancelLatest() {} }, ExclusionArchive: { updateButtons() {}, selectedMenuItem(ev, anchor) { effects.push(['archive-menu', ev, anchor]); return { label: 'Archive', onClick: () => effects.push(['archive', ev]) }; } } };
  const context = vm.createContext({ state, window, document, $, structuredClone,
    getComputedStyle: () => ({ visibility: 'visible' }), el: (tag, cls, text) => new Element(tag, cls, text),
    workspaceScope: () => scope, caseSig: () => signature, caseEvents: () => state.rows,
    cellValue: (ev, column) => ev[column] ?? ev.fields[column], colLabel: String, trunc: String, eventComment: () => '', levelColor: () => '#000',
    esc: String, escRe: String, toast: message => messages.push(message), updatePager() {}, saveVisibleCols() {}, refresh() {},
    showCtxMenu: (x, y, items, options) => menus.push({ items, options }),
    openTrail: ev => effects.push(['trail', ev]), addEventToAnalysis: id => effects.push(['case', id]),
    openCommentModal: ev => effects.push(['comment', ev]), sendVisibleToCase: () => effects.push(['visible']),
    openSendToTrailModal: list => effects.push(['send-trail', list]),
    removeEventFromCase: (ev, options) => effects.push(['remove', ev, options]),
    closeDetailValue() {}, switchDetailTab() {},
    caseArgs: async args => args,
    api: (command, args) => { const pending = deferred(); requests.push({ command, args, ...pending }); return pending.promise; },
    showDetail(ev, source, admission) { state.currentDetailEv = ev; state.detailAdmission = admission; },
  });
  vm.runInContext(section('function eventCellMenu(', '\nlet lastCaseRemoval'), context);
  vm.runInContext(section('function ensureSelectionOwner(', '// envia a página visível'), context);
  vm.runInContext(section('function tableValuePreview(', '// ------------------------------------------------------------------ histograma'), context);
  vm.runInContext(section('let detailRequest =', 'function openContextInspector('), context);
  vm.runInContext(section('function closeDrawer()', 'async function copyDetail('), context);
  const render = (options = {}) => context.renderTable({ total: state.rows.length, rows: state.rows }, options);
  const button = id => body.children.find(row => Number(row.dataset.eventId) === id)?.querySelector('.record-actions-trigger');
  const event = (target, extra = {}) => ({ target, preventDefault() {}, stopPropagation() {}, ...extra });
  render();
  return { context, state, window, document, $, node, rows, render, button, event, head, body, table, menus, effects, messages, requests, listeners,
    scope: value => { scope = value; }, signature: value => { signature = value; } };
}

test('500 native rows have one named action button each, no grid/cell tabindex, and one skip route', () => {
  const f = fixture(500), columns = [...f.state.visibleCols], before = JSON.stringify(f.rows);
  assert.equal(f.body.children.length, 500);
  assert.equal(f.document.querySelectorAll('.record-actions-trigger').length, 500);
  for (const row of f.body.children) {
    assert.equal(row.tagName, 'TR'); assert.equal(row.children.length, columns.length + 1);
    assert.ok(row.children.every(cell => cell.tagName === 'TD' && !Object.hasOwn(cell.attrs, 'tabindex')));
    const trigger = row.querySelector('button'); assert.equal(trigger.type, 'button'); assert.equal(trigger.getAttribute('aria-label'), `Ações do registro ${row.dataset.eventId}`);
    assert.equal(trigger.getAttribute('aria-haspopup'), 'menu'); assert.equal(row.getAttribute('role'), null);
  }
  assert.equal(f.head.children[0].children.length, columns.length + 1);
  assert.equal(f.table.querySelector('colgroup').children.length, columns.length + 1);
  assert.equal(f.$('#skip-records').hidden, false); f.$('#skip-records').onclick(f.event(f.$('#skip-records')));
  assert.equal(f.document.activeElement, f.$('#page-size'));
  assert.deepEqual(f.state.visibleCols, columns); assert.equal(JSON.stringify(f.rows), before);
  assert.equal(f.context.buildEventRow(f.rows[0]).children.length, columns.length, 'historical tables remain unchanged');
});

test('record actions share genuine callbacks and preserve an existing unrelated selection', () => {
  const f = fixture(); f.state.selectedEventRows = new Map([[0, f.rows[0]], [1, f.rows[1]]]);
  const trigger = f.button(4); trigger.onclick(f.event(trigger)); const menu = f.menus.at(-1);
  assert.equal(menu.options.trigger, trigger); assert.equal(menu.items[0].label, 'Ver detalhes');
  assert.equal(menu.items.some(item => /Copiar valor|Criar filtro|Filtrar igual|Enviar todos com/.test(item.label || '')), false);
  for (const label of ['Investigar possível trilha', 'Enviar evento ao caso', 'Adicionar comentário', 'Jogar evento para uma trilha...', 'Archive']) menu.items.find(item => item.label === label).onClick();
  assert.equal(f.effects.find(item => item[0] === 'trail')[1], f.rows[4]);
  assert.equal(f.effects.find(item => item[0] === 'case')[1], 4);
  assert.equal(f.effects.find(item => item[0] === 'comment')[1], f.rows[4]);
  assert.deepEqual([...f.state.selectedEventRows.keys()], [0, 1]);
  const selected = f.button(0); selected.oncontextmenu(f.event(selected));
  f.menus.at(-1).items.find(item => item.label === 'Jogar 2 eventos para uma trilha...').onClick();
  assert.deepEqual(Array.from(f.effects.at(-1)[1]), [f.rows[0], f.rows[1]]);
});

test('record context menu in a Case uses exact occurrence-removal callback and captured owner', () => {
  const f = fixture(); f.scope('case'); f.render(); const trigger = f.button(2); trigger.onclick(f.event(trigger));
  const items = f.menus.at(-1).items; assert.equal(items.some(item => item.icon === 'fa-microscope' || item.icon === 'fa-briefcase'), false);
  items.find(item => item.label === 'Remover este registro do Caso').onClick();
  const removal = f.effects.at(-1); assert.equal(removal[1], f.rows[2]); assert.equal(removal[2].signature, 'members-a');
  assert.deepEqual(plain(removal[2].owner), f.state.owner); assert.equal(removal[2].anchor, trigger);
});

test('rerender and reuseRows restore the logical record after detach and column/row reorder', () => {
  const f = fixture(); const original = f.button(3); original.focus();
  f.state.visibleCols.reverse(); f.state.rows = [...f.rows].reverse(); f.render({ reuseRows: true });
  assert.equal(f.document.activeElement, original, 'moving the same DOM node still loses browser focus during detach');
  assert.deepEqual(f.body.children[0].children.slice(1).map(cell => cell.dataset.column), f.state.visibleCols);
  f.state.rows = f.state.rows.map(row => ({ ...row, message: 'fresh' })); f.render();
  assert.equal(f.document.activeElement, f.button(3)); assert.notEqual(f.button(3), original);
  assert.equal(f.document.activeElement._recordTarget.event.message, 'fresh');
});

test('removed identity or reused numeric ID falls back to pagination, never the neighboring row', () => {
  for (const replacement of ['removed', 'new-ref', 'legacy-copy', 'empty-ref']) {
    const f = fixture(); if (replacement === 'legacy-copy') { delete f.rows[2].event_ref; f.render(); }
    if (replacement === 'empty-ref') { f.rows[2].event_ref = ''; f.render(); }
    f.button(2).focus();
    f.state.rows = replacement === 'removed' ? f.rows.filter(row => row.id !== 2) : f.rows.map(row => row.id !== 2 ? row : { ...row, ...(replacement === 'new-ref' ? { event_ref: 'replacement:2' } : {}) });
    f.render({ reuseRows: true }); assert.equal(f.document.activeElement, f.$('#page-size'), replacement);
  }
});

test('pending query rerender never steals focus from search or another surface', () => {
  const f = fixture(); f.button(2).focus(); f.$('#quick-search').focus();
  f.state.rows = f.rows.map(row => ({ ...row })); f.render(); assert.equal(f.document.activeElement, f.$('#quick-search'));
  const modal = f.node('button', 'modal-action'); modal.focus(); f.render({ reuseRows: true }); assert.equal(f.document.activeElement, modal);
});

test('source, Case, analysis, revision, scope and evidence changes block stale triggers and callbacks', () => {
  for (const change of ['source', 'case', 'analysis', 'revision', 'scope', 'evidence']) {
    const f = fixture(); if (change === 'evidence') { f.scope('case'); f.render(); }
    const trigger = f.button(2); trigger.onclick(f.event(trigger)); const action = f.menus.at(-1).items[0];
    if (change === 'source') f.state.owner.sourceGeneration++;
    if (change === 'case') f.state.cases.active = 'b';
    if (change === 'analysis') f.state.owner.identity.analysisId = 'other';
    if (change === 'revision') f.state.owner.identity.visibilityRevision++;
    if (change === 'scope') f.scope('case'); if (change === 'evidence') f.signature('members-b');
    action.onClick(); assert.equal(f.requests.length, 0, change);
    const count = f.menus.length; trigger.onclick(f.event(trigger)); assert.equal(f.menus.length, count, change);
    trigger.focus(); assert.equal(f.document.activeElement, f.$('#page-size'), 'a still-mounted stale menu origin is invalid');
    f.render({ reuseRows: true }); assert.equal(f.document.activeElement, f.$('#page-size'), change);
  }
});

test('drawer focuses stable loading control immediately and returns by event_ref after fresh rows', async () => {
  const f = fixture(); const trigger = f.button(2); trigger.focus(); trigger.onclick(f.event(trigger));
  const pending = f.menus.at(-1).items[0].onClick();
  assert.equal(f.document.activeElement, f.$('#dr-close')); assert.equal(f.$('#drawer').hidden, false);
  assert.equal(f.requests[0].args.eventRef, 'source:2'); assert.deepEqual(plain(f.requests[0].args.analysisContext), f.state.owner.identity);
  f.state.rows = [...f.rows].reverse().map(row => ({ ...row })); f.render();
  assert.equal(f.document.activeElement, f.$('#dr-close'), 'render cannot steal drawer focus');
  f.requests[0].resolve(f.state.rows.find(row => row.id === 2)); await pending;
  assert.equal(f.document.activeElement, f.$('#dr-close'), 'async content cannot refocus another control');
  f.context.closeDrawer(); assert.equal(f.document.activeElement, f.button(2));
});

test('late detail reply/close cannot rebind an origin after source swap or removal, or steal outside focus', async () => {
  for (const change of ['source', 'removed', 'outside', 'closed']) {
    const f = fixture(), trigger = f.button(2); trigger.focus(); trigger.onclick(f.event(trigger)); const pending = f.menus.at(-1).items[0].onClick();
    if (change === 'source') { f.state.owner.sourceGeneration++; f.render(); }
    if (change === 'removed') { f.state.rows = f.rows.filter(row => row.id !== 2); f.render(); }
    if (change === 'outside') f.$('#quick-search').focus();
    f.context.closeDrawer();
    const expected = change === 'outside' ? f.$('#quick-search') : change === 'closed' ? f.button(2) : f.$('#page-size');
    assert.equal(f.document.activeElement, expected, change);
    f.requests[0].resolve(f.rows[2]); await pending; assert.equal(f.document.activeElement, expected, change);
    assert.equal(f.state.currentDetailEv, null, 'closed detail does not accept a late response');
  }
});

test('header decorators and resizing use data identifiers; action column has no field menu or saved width', () => {
  const f = fixture(); f.context.fieldTop = field => f.effects.push(['top', field]);
  vm.runInContext(discovery.slice(discovery.indexOf('  const oldTable=renderTable;'), discovery.indexOf('  const oldDetail=showDetail;')), f.context);
  f.state.visibleCols.reverse(); f.render({ reuseRows: true });
  const [actions, first, second] = f.head.children[0].children;
  assert.equal(actions.oncontextmenu, undefined); assert.equal(actions.draggable, undefined);
  for (const header of [first, second]) { header.oncontextmenu(f.event(header)); f.menus.at(-1).items.find(item => item.icon === 'fa-ranking-star').onClick(); assert.equal(f.effects.at(-1)[1], header.dataset.column); }
  first.querySelector('.col-grip').onmousedown(f.event(first, { clientX: 10 }));
  f.listeners.get('mousemove')[0]({ clientX: 30 }); f.listeners.get('mouseup')[0]({ clientX: 30 });
  assert.equal(f.table.querySelector('colgroup').children[0].style.width, undefined);
  assert.equal(f.table.querySelector('colgroup').children[1].style.width, '120px');
  assert.equal(f.state.colWidths.exact, 120); assert.equal(Object.hasOwn(f.state.colWidths, 'undefined'), false);
});

test('Ctrl/Cmd and Shift mouse selection remains unchanged; new button does not select silently', () => {
  const f = fixture(); const row = id => f.body.children.find(item => Number(item.dataset.eventId) === id);
  row(0).onclick(f.event(row(0), { ctrlKey: true })); row(2).onclick(f.event(row(2), { metaKey: true }));
  assert.deepEqual([...f.state.selectedEventRows.keys()], [0, 2]);
  row(4).onclick(f.event(row(4), { shiftKey: true })); assert.deepEqual([...f.state.selectedEventRows.keys()], [2, 3, 4]);
  row(1).onclick(f.event(f.button(1))); assert.deepEqual([...f.state.selectedEventRows.keys()], [2, 3, 4]);
});


// Install the actual shared menu and app capture hook atop the row fixture.
// This combined path is essential: renderTable must leave menu focus alone,
// then Escape/Tab must resolve the replaced logical record at dismissal time.
function recordMenuFixture() {
  const f = fixture(), windowListeners = new Map();
  f.window.addEventListener = (type, fn) => windowListeners.set(type, fn);
  f.document.documentElement = { clientWidth: 1200, clientHeight: 800 };
  vm.runInContext(readFileSync(new URL('../../frontend/context-menu.js', import.meta.url), 'utf8'), f.context);
  vm.runInContext(section('let ctxEl = null;', '\nconst trunc ='), f.context);
  const controller = vm.runInContext('ctxMenu', f.context);
  const key = key => windowListeners.get('keydown')({ key, target: f.document.activeElement, preventDefault() {}, stopImmediatePropagation() {} });
  const open = id => { const trigger = f.button(id); trigger.focus(); trigger.onclick(f.event(trigger)); return trigger; };
  return { ...f, controller, key, open };
}

test('menu Escape and Tab resolve the same event_ref after requery detaches the caller', () => {
  for (const key of ['Escape', 'Tab']) {
    const f = recordMenuFixture(), old = f.open(2), menuItem = f.document.activeElement;
    f.state.rows = [...f.rows].reverse().map(row => ({ ...row })); f.render();
    assert.equal(old.isConnected, false); assert.notEqual(f.button(2), old);
    assert.equal(f.document.activeElement, menuItem, 'query cannot steal focus from the open menu');
    assert.equal(f.requests.length, 0, 'opening and rerendering actions never issue a native query');
    f.key(key); assert.equal(f.controller.element, null); assert.equal(f.document.activeElement, f.button(2), key);
  }
});

test('menu dismissal falls back to pagination for a removed row or a reused ID with different ref', () => {
  for (const change of ['removed', 'new-ref']) for (const key of ['Escape', 'Tab']) {
    const f = recordMenuFixture(); f.open(2);
    f.state.rows = change === 'removed' ? f.rows.filter(row => row.id !== 2)
      : f.rows.map(row => row.id === 2 ? { ...row, event_ref: 'replacement:2' } : row);
    f.render(); f.key(key);
    assert.equal(f.document.activeElement, f.$('#page-size'), `${change}: ${key}`);
    assert.equal(f.requests.length, 0);
  }
});

test('menu resolver rejects still-connected stale source, Case, analysis and revision callers', () => {
  for (const change of ['source', 'case', 'analysis', 'revision', 'scope', 'evidence']) {
    const f = recordMenuFixture(); if (change === 'evidence') { f.scope('case'); f.render(); }
    const old = f.open(2);
    if (change === 'source') f.state.owner.sourceGeneration++;
    if (change === 'case') f.state.cases.active = 'b';
    if (change === 'analysis') f.state.owner.identity.analysisId = 'b';
    if (change === 'revision') f.state.owner.identity.visibilityRevision++;
    if (change === 'scope') f.scope('case'); if (change === 'evidence') f.signature('members-b');
    assert.equal(old.isConnected, true); f.key('Escape');
    assert.equal(f.document.activeElement, f.$('#page-size'), change);
    assert.equal(f.requests.length, 0);
  }
});

test('record menu paging retains the original captured identity through replacement and rerender', () => {
  const f = recordMenuFixture(); f.open(2);
  f.state.rows = f.rows.map(row => ({ ...row })); f.render();
  f.context.showCtxMenu(0, 0, [{ label: 'Next menu page', onClick() {} }]);
  f.state.rows = [...f.state.rows].reverse().map(row => ({ ...row })); f.render();
  f.key('Escape'); assert.equal(f.document.activeElement, f.button(2));
  f.open(2); f.context.showCtxMenu(0, 0, [{ label: 'Next menu page', onClick() {} }]);
  f.state.owner.sourceGeneration++; f.key('Escape');
  assert.equal(f.document.activeElement, f.$('#page-size'), 'paging never recaptures an obsolete logical caller');
  assert.equal(f.requests.length, 0);
});
