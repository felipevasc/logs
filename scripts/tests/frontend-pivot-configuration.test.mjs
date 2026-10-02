import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/analysis-workbench.js', import.meta.url), 'utf8');
const workspace = readFileSync(new URL('../../frontend/workspace-context.js', import.meta.url), 'utf8');
const part = (text, start, end) => text.slice(text.indexOf(start), text.indexOf(end, text.indexOf(start)));
const plain = value => JSON.parse(JSON.stringify(value));

function fixture() {
  const nodes = new Map(), focus = [], calls = [];
  const document = { activeElement: null };
  class Node {
    constructor(tag = 'div', className = '', text = '') { Object.assign(this, { tag, className, children: [], hidden: false, attrs: {}, value: '', _text: text, _top: 0, _left: 0 }); }
    set id(value) { this._id = value; nodes.set(`#${value}`, this); }
    get id() { return this._id; }
    get textContent() { return this._text + this.children.map(node => node.textContent).join(''); }
    set textContent(value) { this._text = value; this.children = []; }
    get firstElementChild() { return this.children[0] || null; }
    append(...children) { for (const child of children) { child.parentElement?.removeChild(child); this.children.push(child); child.parentElement = this; } }
    removeChild(child) { this.children.splice(this.children.indexOf(child), 1); child.parentElement = null; }
    prepend(...children) { for (const child of children.reverse()) { child.parentElement?.removeChild(child); this.children.unshift(child); child.parentElement = this; } }
    before(child) { const parent = this.parentElement; parent.children.splice(parent.children.indexOf(this), 0, child); child.parentElement = parent; }
    after(child) { const parent = this.parentElement; parent.children.splice(parent.children.indexOf(this) + 1, 0, child); child.parentElement = parent; }
    replaceChildren(...children) { for (const child of this.children) child.parentElement = null; this.children = []; this._text = ''; this.append(...children); }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key] ?? null; }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    focus(options) { document.activeElement = this; focus.push({ node: this, options }); }
    get scrollTop() { return this.hidden ? 0 : this._top; }
    set scrollTop(value) { this._top = Math.min(value, this === nodes.get('#cube-table-view') ? nodes.get('#aw-pivot-config-zones')?.hidden ? 300 : 500 : 1000); }
    get scrollLeft() { return this.hidden ? 0 : this._left; }
    set scrollLeft(value) { this._left = value; }
  }
  const el = (...args) => new Node(...args), $ = selector => nodes.get(selector) || null;
  const add = (selector, tag, parent) => { const node = el(tag); nodes.set(selector, node); parent?.append(node); return node; };
  const panel = add('.cube-panel'), main = add('.cube-main', 'div', panel), fields = add('.cube-fields', 'div', panel);
  const zones = add('.cube-zones', 'div', main), output = add('.cube-output', 'div', main), table = add('#cube-table-view', 'div', output);
  const body = add('#cube-table tbody', 'tbody', table); body.append(el('tr', '', 'result'));
  add('#cube-table thead', 'thead', table); add('#btn-cube-clear', 'button', zones); add('.cube-hint', 'p', fields); add('#cube-field-list', 'div', fields);
  add('#aw-group-pager'); add('#aw-group-summary'); add('#group-table thead'); add('#group-table tbody');
  const cube = { id: 'table-a', rows: ['level'], cols: [], values: [{ func: 'count', column: '*', alias: 'qtd' }] };
  const context = vm.createContext({ $, el, document, window: {}, state: { analyticsScope: 'dataset' }, activeCube: () => cube,
    cubeState: { collapsed: new Set([['a']]), result: { cells: [[[42]]] }, requestVersion: 9 },
    colLabel: field => ({ level: 'Nível', source: 'Origem', code: 'Código' }[field] || field), fmtNum: String,
    runCube: () => calls.push('runCube'), renderCubeTable: () => calls.push('renderCubeTable'), markCubeTableChanged: () => calls.push('markChanged'),
    clearTimeout() {}, AGG_FUNCS: [['count', 'Contagem']],
  });
  vm.runInContext(part(source, '  const measureLabels =', '  function calculationError('), context);
  vm.runInContext(part(source, '  function renderPivotConfiguration()', '  function pivotPreset('), context);
  vm.runInContext('let groupPresentationCache = null, pivotPresentationCache = null;', context);
  vm.runInContext(part(source, '  window.WorkspaceAnalysis =', '  document.addEventListener?.('), context);
  vm.runInContext(part(workspace, '  function defaults()', '  function capture()'), context);
  context.pivotShell();
  return { $, el, cube, context, document, calls, focus, toggle: () => $('#aw-pivot-config-toggle').onclick(), capture: () => plain(context.window.WorkspaceAnalysis.capture()) };
}

test('pivot starts expanded with an explicit native button controlling all builder regions', () => {
  const f = fixture(), toggle = f.$('#aw-pivot-config-toggle');
  assert.equal(toggle.tag, 'button'); assert.equal(toggle.type, 'button');
  assert.equal(toggle.getAttribute('aria-expanded'), 'true');
  for (const id of toggle.getAttribute('aria-controls').split(' ')) assert.equal(f.$(`#${id}`).hidden, false);
  assert.equal(f.$('#aw-pivot-config-summary').hidden, true);
  assert.equal(f.capture().pivot.configurationCollapsed, false);
  assert.deepEqual(f.calls, []);
});

test('repeated folding changes layout only, preserving DOM, configuration, result, filters and focus', () => {
  const f = fixture(), row = f.$('#cube-table tbody').firstElementChild, cube = plain(f.cube), result = f.context.cubeState.result;
  const add = f.el('select'), search = f.$('#aw-field-search'); f.$('#aw-pivot-config-zones').append(add); search.value = 'rare.child';
  const toggle = f.$('#aw-pivot-config-toggle'); toggle.focus();
  for (let repeat = 0; repeat < 3; repeat++) {
    f.toggle(); assert.equal(toggle.getAttribute('aria-expanded'), 'false'); assert.equal(f.$('#aw-pivot-fields').hidden, true);
    assert.equal(f.$('#aw-pivot-config-summary').hidden, false); assert.equal(toggle.textContent, 'Editar configuração');
    f.toggle(); assert.equal(toggle.getAttribute('aria-expanded'), 'true'); assert.equal(f.$('#aw-pivot-config-zones').contains(add), true);
  }
  assert.deepEqual(plain(f.cube), cube); assert.equal(f.context.cubeState.result, result); assert.equal(f.context.cubeState.requestVersion, 9);
  assert.equal(f.$('#cube-table tbody').firstElementChild, row); assert.equal(search.value, 'rare.child');
  assert.equal(f.document.activeElement, toggle); assert.equal(f.focus.length, 1); assert.deepEqual(f.calls, []);
});

test('folding safely returns hidden focus to the disclosure without stealing focus elsewhere', () => {
  for (const selector of ['#aw-pivot-config-zones', '#aw-pivot-config-actions', '#aw-pivot-fields']) {
    const f = fixture(), input = f.el('input'); f.$(selector).append(input); input.focus(); f.toggle();
    assert.equal(f.document.activeElement, f.$('#aw-pivot-config-toggle')); assert.deepEqual(plain(f.focus.at(-1).options), { preventScroll: true });
  }
  const f = fixture(), other = f.el('input'); other.focus(); f.toggle();
  assert.equal(f.document.activeElement, other); assert.equal(f.focus.length, 1);
});

test('the readable summary uses current axes, calculations, aliases and unavailable fields as text', () => {
  const f = fixture(); f.context.window.AnalysisFields = { available: field => field !== 'missing' };
  f.cube.rows = ['level', 'rare.<script>\u200b.path']; f.cube.cols = ['source'];
  f.cube.values = [{ func: 'sum', column: 'code', alias: 'Total calculado' }, { func: 'avg', column: 'missing', alias: '' }];
  f.toggle();
  const items = f.$('#aw-pivot-config-summary').children.map(node => node.children.map(child => child.textContent));
  assert.deepEqual(items, [['Linhas', 'Nível › rare.<script>\u200b.path'], ['Colunas', 'Origem'], ['Medidas', 'Total calculado (Soma · Código); Média · missing (indisponível)']]);
  assert.ok(f.$('#aw-pivot-config-summary').children.every(item => item.children[1].children.length === 0), 'authored names never become HTML');
  f.cube.rows = ['source']; f.cube.cols = ['level']; f.context.renderPivotConfiguration();
  assert.equal(f.$('#aw-pivot-config-summary').children[0].children[1].textContent, 'Origem');
  assert.equal(f.$('#aw-pivot-config-zones').hidden, true); assert.deepEqual(f.calls, []);
});

test('empty axes and the default count have a short unambiguous summary', () => {
  const f = fixture(); f.cube.rows = []; f.toggle();
  assert.deepEqual(f.$('#aw-pivot-config-summary').children.map(node => node.children[1].textContent), ['Total', 'Sem divisão', 'Registros']);
});

test('folding restores an edge-clamped scroll position and the palette search scroll', () => {
  const f = fixture(), table = f.$('#cube-table-view'), fields = f.$('#aw-pivot-fields');
  table.scrollTop = 450; table.scrollLeft = 78; fields.scrollTop = 122;
  f.toggle(); assert.equal(table.scrollTop, 300); assert.equal(table.scrollLeft, 78);
  f.toggle(); assert.equal(table.scrollTop, 450); assert.equal(table.scrollLeft, 78); assert.equal(fields.scrollTop, 122);
});

test('a deliberate new scroll or replaced result wins over the pre-folding position', () => {
  for (const change of ['scroll', 'result']) {
    const f = fixture(), table = f.$('#cube-table-view'); table.scrollTop = 450; f.toggle();
    if (change === 'scroll') table.scrollTop = 150;
    else f.$('#cube-table tbody').replaceChildren(f.el('tr', '', 'new result'));
    f.toggle(); assert.equal(table.scrollTop, change === 'scroll' ? 150 : 300);
  }
});

test('current positions in the shared viewport and idempotent state calls stay untouched', () => {
  const f = fixture(), table = f.$('#cube-table-view'); table.scrollTop = 140; table.scrollLeft = 21;
  f.context.setPivotConfigurationCollapsed(false); f.toggle(); f.context.setPivotConfigurationCollapsed(true); f.toggle();
  assert.equal(table.scrollTop, 140); assert.equal(table.scrollLeft, 21); assert.deepEqual(f.calls, []);
});

test('capture and sanitized restoration keep the choice separate for each workspace context', () => {
  const f = fixture(); f.toggle(); const dataset = f.capture();
  f.context.window.WorkspaceAnalysis.restore(f.context.sanitize({}).workbench);
  assert.equal(f.capture().pivot.configurationCollapsed, false, 'an untouched Case starts expanded');
  const caseState = f.capture();
  f.context.window.WorkspaceAnalysis.restore(f.context.sanitize({ workbench: dataset }).workbench);
  assert.equal(f.capture().pivot.configurationCollapsed, true); assert.equal(f.$('#aw-pivot-config-zones').hidden, true);
  f.context.window.WorkspaceAnalysis.restore(f.context.sanitize({ workbench: caseState }).workbench);
  assert.equal(f.capture().pivot.configurationCollapsed, false); assert.equal(f.$('#aw-pivot-config-zones').hidden, false);
  assert.deepEqual(f.calls, []);
});

test('old snapshots and invalid saved values never opt a user into folding', () => {
  for (const value of [undefined, null, false, 'true', 'false', 1, {}, []]) {
    const f = fixture(), saved = { pivot: { ...(value === undefined ? {} : { configurationCollapsed: value }) } };
    f.toggle(); f.context.window.WorkspaceAnalysis.restore(saved);
    assert.equal(f.capture().pivot.configurationCollapsed, false, `direct restore rejects ${JSON.stringify(value)}`);
    const sanitized = f.context.sanitize({ workbench: saved });
    assert.equal(sanitized.workbench.pivot.configurationCollapsed, false);
  }
});

test('context restoration invalidates the old scroll checkpoint', () => {
  const f = fixture(), table = f.$('#cube-table-view'); table.scrollTop = 450; f.toggle();
  f.context.window.WorkspaceAnalysis.restore({ pivot: { configurationCollapsed: true } });
  f.toggle(); assert.equal(table.scrollTop, 300, 'no old-context scroll position returns');
});

test('configuration CSS keeps readable labels and targets without affecting non-pivot layouts', () => {
  const css = readFileSync(new URL('../../frontend/workspace.css', import.meta.url), 'utf8');
  assert.match(css, /\.cube-panel \.aw-pivot-config-toggle[^}]*min-height: 30px/);
  assert.match(css, /\.cube-panel \.aw-pivot-config-summary[^}]*font-size: 12px/);
  assert.match(css, /\.cube-panel \.aw-pivot-config-item dd[^}]*overflow-wrap: anywhere/);
  assert.match(source, /renderCubeZones = function[\s\S]*renderPivotConfiguration\(\);/);
});
