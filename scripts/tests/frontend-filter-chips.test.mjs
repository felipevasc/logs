import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');

function harness(filters = [{ column: 'message', op: 'equals_exact', value: 'alpha\r\nbravo', value2: null }]) {
  let document;
  class Node {
    constructor(tag = 'div', cls = '', text = '') { this.tagName = tag.toUpperCase(); this.className = cls; this.textContent = text; this.children = []; this.attributes = {}; this.classList = { toggle() {} }; this._value = undefined; this.selectedOption = null; }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    get options() { return this.children; }
    get value() { return this.tagName === 'SELECT' ? this.selectedOption?.value ?? '' : this._value ?? (this.tagName === 'OPTION' ? this.textContent : ''); }
    set value(value) {
      if (this.tagName === 'SELECT') this.selectedOption = this.children.find(option => option.value === String(value)) || null;
      else this._value = String(value);
    }
    set innerHTML(value) {
      if (this.children.some(child => child.contains(document.activeElement))) document.activeElement = document.body;
      for (const child of this.children) child.parentElement = null;
      this.children = []; this.selectedOption = null; this.html = value;
    }
    appendChild(child) {
      child.parentElement = this; this.children.push(child);
      if (this.tagName === 'SELECT' && this.children.length === 1) this.selectedOption = child;
      return child;
    }
    contains(node) { return this === node || this.children.some(child => child.contains(node)); }
    matches(selector) { return selector.split(',').some(part => {
      if (part === '[hidden]') return !!this.hidden;
      if (part === '[inert]') return !!this.inert;
      if (part.startsWith('.')) return this.className.split(' ').includes(part.slice(1));
      if (part.startsWith('#')) return this.id === part.slice(1);
      return this.tagName.toLowerCase() === part;
    }); }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    getAttribute(name) { return this.attributes[name] ?? null; }
    getClientRects() { return this.isConnected && !this.closest('[hidden],[inert]') ? [{}] : []; }
    focus() { document.activeElement = this; }
  }
  document = { body: new Node('body'), activeElement: null };
  document.activeElement = document.body;
  const boxes = [0, 1].map(() => document.body.appendChild(new Node('div', 'chips chips-sync')));
  document.querySelectorAll = selector => selector === '.chips-sync' ? boxes : document.body.querySelectorAll(selector);
  const nodes = new Map();
  function $(selector) {
    if (!nodes.has(selector)) {
      const tag = selector.includes('fp-val') ? 'textarea' : ['#fp-col', '#fp-op'].includes(selector) ? 'select' : selector === '#quick-search' ? 'input' : 'button';
      const node = document.body.appendChild(new Node(tag)); node.id = selector.slice(1); nodes.set(selector, node);
    }
    return nodes.get(selector);
  }
  $('#filter-pop').hidden = true;
  const state = { filters, quick: '', columns: ['message', 'code'], datasetRevision: 1, currentArtifact: { id: 'source-a', loadedAt: 1 }, derivedFields: [], page: 0 };
  const effects = { refreshes: 0, removed: [], menus: [], notices: [], cancelled: 0 };
  const live = { scope: 'dataset', caseId: 'case-a' };
  let context;
  context = vm.createContext({ state, document, $, window: { QueryLang: { validate: () => null }, CanonicalFields: { cancel: () => effects.cancelled++ } },
    el: (tag, cls, text) => new Node(tag, cls, text), colLabel: String, fmtFilterSideValue: (_column, value) => value,
    workspaceScope: () => live.scope, activeCase: () => ({ id: live.caseId }), positionPop() {}, renderFilterTabs() {},
    filtersChanged: () => { effects.refreshes++; context.renderChips(); },
    removeFilter: index => { effects.removed.push(index); state.filters.splice(index, 1); context.renderChips(); },
    invertFilter() {}, showCtxMenu: (_x, _y, items) => effects.menus.push(items), toast: text => effects.notices.push(text),
  });
  vm.runInContext(app.slice(app.indexOf('const OPS ='), app.indexOf('const AGG_FUNCS =')), context);
  vm.runInContext(app.slice(app.indexOf('function chipLabel('), app.indexOf('\nfunction invertFilter(')), context);
  vm.runInContext(app.slice(app.indexOf('const filterChipTargets ='), app.indexOf('// ponto único de reação a mudanças de filtro')), context);
  vm.runInContext(app.slice(app.indexOf('let currentEditFilterIndex ='), app.indexOf('\nfunction positionPop(')), context);
  context.renderChips();
  function click(node) { const event = { stopped: false, stopPropagation() { this.stopped = true; } }; node.focus(); node.onclick(event); return event; }
  return { context, document, state, live, boxes, effects, $, click,
    labels: (box = 0) => boxes[box].querySelectorAll('.chip-edit'),
    redraw: () => context.renderChips(),
  };
}

test('filter label is a native sibling button with full text and independent removal', () => {
  const text = 'full <literal> '.repeat(80), h = harness([{ column: 'message', op: 'equals_exact', value: text }]);
  const label = h.labels()[0], chip = label.parentElement, remove = chip.querySelector('.x');
  assert.equal(label.tagName, 'BUTTON'); assert.equal(label.type, 'button'); assert.equal(label.textContent.includes(text), true);
  assert.equal(label.getAttribute('aria-label').includes(text), true); assert.equal(label.title.includes(text), true);
  assert.equal(remove.parentElement, chip); assert.equal(label.querySelectorAll('button').length, 0);
  assert.equal(h.click(remove).stopped, true); assert.deepEqual(h.effects.removed, [0]);
  assert.equal(h.$('#filter-pop').hidden, true); assert.equal(h.effects.refreshes, 0, 'removal never enters the edit path');
});

test('primary click opens the existing literal editor without a query; cancel preserves data and focus', () => {
  const h = harness(), before = JSON.stringify(h.state.filters), label = h.labels()[0];
  assert.equal(h.click(label).stopped, true, 'opening click cannot bubble into the outside-click closer');
  assert.equal(h.$('#filter-pop').hidden, false); assert.equal(h.$('#fp-val').value, JSON.stringify('alpha\r\nbravo'));
  h.$('#fp-val').value = JSON.stringify('edited\r\ntext'); h.context.closeFilterPop();
  assert.equal(JSON.stringify(h.state.filters), before); assert.equal(h.document.activeElement, label);
  assert.equal(h.effects.refreshes, 0); assert.equal(h.effects.cancelled, 1);
});

test('context menu preserves all actions and uses the same focusable edit label', () => {
  const h = harness(), label = h.labels()[0];
  label.parentElement.oncontextmenu({ preventDefault() {}, stopPropagation() {}, clientX: 5, clientY: 5 });
  const menu = h.effects.menus[0];
  assert.deepEqual(Array.from(menu.filter(item => !item.sep), item => item.label), ['Inverter filtro', 'Editar filtro', 'Remover filtro']);
  menu.find(item => item.label === 'Editar filtro').onClick(); h.context.closeFilterPop();
  assert.equal(h.document.activeElement, label); assert.equal(h.effects.refreshes, 0);
});

test('cancel after rerender returns to the same filter and mirrored container, never a neighbor', () => {
  const h = harness([{ column: 'code', op: 'equals_exact', value: '200' }, { column: 'code', op: 'equals_exact', value: '404' }]);
  h.click(h.labels(1)[1]); h.redraw(); h.context.closeFilterPop();
  assert.equal(h.document.activeElement, h.labels(1)[1]);
  h.click(h.labels(1)[1]); h.state.filters.pop(); h.redraw(); h.context.closeFilterPop();
  assert.equal(h.document.activeElement, h.$('#btn-add-filter'));
});

test('removing the focused filter or replacing context uses fallback rather than the adjacent chip', () => {
  const h = harness([{ column: 'code', op: 'equals_exact', value: '200' }, { column: 'code', op: 'equals_exact', value: '404' }]);
  h.labels()[0].focus(); h.state.filters.shift(); h.redraw();
  assert.equal(h.document.activeElement, h.$('#btn-add-filter'));
  h.labels()[0].focus(); h.live.scope = 'case'; h.redraw();
  assert.equal(h.document.activeElement, h.$('#btn-add-filter'));
});

test('save restores the replacement chip and a late query render cannot steal moved focus', () => {
  const h = harness(); h.click(h.labels()[0]); h.$('#fp-val').value = JSON.stringify('saved\r\nvalue');
  assert.equal(h.context.applyFilterPop(), true); assert.equal(h.state.filters[0].value, 'saved\r\nvalue');
  assert.equal(h.effects.refreshes, 1); assert.equal(h.document.activeElement, h.labels()[0]);
  h.redraw(); assert.equal(h.document.activeElement, h.labels()[0], 'query completion preserves chip focus');
  h.$('#quick-search').focus(); h.redraw();
  assert.equal(h.document.activeElement, h.$('#quick-search'), 'later query completion respects the user’s new focus');
});

test('changed source/context keeps the draft and uses existing fallback instead of stale chip focus', () => {
  const h = harness(); h.click(h.labels()[0]); h.$('#fp-val').value = 'draft'; h.state.datasetRevision++;
  assert.equal(h.context.applyFilterPop(), false); assert.equal(h.$('#filter-pop').hidden, false);
  assert.equal(h.$('#fp-val').value, 'draft'); assert.equal(h.effects.refreshes, 0);
  h.context.closeFilterPop(); assert.equal(h.document.activeElement, h.$('#btn-add-filter'));
  const stale = h.labels()[0]; h.click(stale); assert.equal(h.$('#filter-pop').hidden, true, 'stale connected chip cannot edit new source context');
});

test('reordered filters are resolved by object identity at primary activation', () => {
  const h = harness([{ column: 'code', op: 'equals_exact', value: '200' }, { column: 'code', op: 'equals_exact', value: '404' }]);
  const label = h.labels()[1]; h.state.filters.shift(); h.click(label);
  assert.equal(h.$('#fp-val').value, '404'); h.context.closeFilterPop();
  h.redraw(); h.click(label); assert.equal(h.$('#filter-pop').hidden, true, 'detached handlers cannot reopen the editor');
});

test('native-like select rejects values absent from its option list', () => {
  const h = harness(); h.click(h.labels()[0]);
  const select = h.$('#fp-op'); assert.equal(select.value, 'equals_exact');
  select.value = 'not-an-option'; assert.equal(select.value, '');
  const before = JSON.stringify(h.state.filters); h.$('#fp-val').value = 'draft';
  assert.equal(h.context.applyFilterPop(), false); assert.equal(h.$('#filter-pop').hidden, false);
  assert.equal(h.$('#fp-val').value, 'draft'); assert.equal(JSON.stringify(h.state.filters), before);
  assert.equal(h.effects.refreshes, 0); assert.match(h.effects.notices.at(-1), /operador.*rascunho/);
});

test('existing in_exact and detection get a readable temporary option and preserve exact values', () => {
  for (const [op, value] of [['in_exact', ' a \nb,c\n a \n'], ['in_exact', 'one\r\ntwo\nthree\rfour'], ['in_exact', '  \n '], ['detection', 'auth.bruteforce.source']]) {
    const h = harness([{ column: op === 'in_exact' ? 'event_ref' : '_all', op, value, value2: null }]);
    h.click(h.labels()[0]);
    assert.equal(h.$('#fp-op').value, op);
    assert.match(h.$('#fp-op').options.find(option => option.value === op).textContent, op === 'in_exact' ? /lista exata.*por linha/ : /detecção/);
    assert.equal(h.$('#fp-val').value, value.includes('\r') ? JSON.stringify(value) : value);
    assert.equal(h.effects.refreshes, 0, 'opening the native filter never queries');
    assert.equal(h.context.applyFilterPop(), true);
    assert.equal(h.state.filters[0].op, op); assert.equal(h.state.filters[0].value, value); assert.equal(h.effects.refreshes, 1);
    assert.equal(h.document.activeElement, h.labels()[0]);
    h.context.openFilterPop();
    assert.equal(h.$('#fp-op').options.some(option => ['in_exact', 'detection'].includes(option.value)), false, 'new-filter catalogue stays unchanged');
  }
});

test('unknown and inherited operator names preserve draft, state and query count even if another op is chosen', () => {
  for (const op of ['', 'unknown', 'constructor', 'toString', '__proto__', 'ends_with', 'threat_rule']) {
    const h = harness([{ column: 'message', op, value: 'original', value2: null }]);
    const before = JSON.stringify(h.state.filters); h.click(h.labels()[0]);
    assert.equal(h.$('#fp-op').value, '');
    h.$('#fp-val').value = 'preserve this draft';
    for (const choice of ['', 'equals_exact']) {
      h.$('#fp-op').value = choice;
      assert.equal(h.context.applyFilterPop(), false);
      assert.equal(h.$('#filter-pop').hidden, false); assert.equal(h.$('#fp-val').value, 'preserve this draft');
      assert.equal(JSON.stringify(h.state.filters), before); assert.equal(h.effects.refreshes, 0);
      assert.match(h.effects.notices.at(-1), /operador.*rascunho/);
    }
  }
});

test('special operators cannot be injected into another existing filter', () => {
  const h = harness(); h.click(h.labels()[0]);
  // Even a forged option must not bypass the exact existing-operator allowlist.
  h.$('#fp-op').appendChild({ value: 'in_exact' }); h.$('#fp-op').value = 'in_exact';
  h.$('#fp-val').value = 'draft'; const before = JSON.stringify(h.state.filters);
  assert.equal(h.context.applyFilterPop(), false); assert.equal(JSON.stringify(h.state.filters), before);
  assert.equal(h.$('#fp-val').value, 'draft'); assert.equal(h.effects.refreshes, 0);
});
