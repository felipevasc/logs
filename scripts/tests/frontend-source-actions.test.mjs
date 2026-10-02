import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/workspace.js', import.meta.url), 'utf8');
const start = source.indexOf('  // Source actions capture'), end = source.indexOf('  async function renderSources()', start);
assert.ok(start >= 0 && end > start);
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; };
const plain = value => JSON.parse(JSON.stringify(value));

function fixture() {
  let document, key = 'source-a', owner = 1;
  const messages = [], copies = [], requests = [], timestamps = [], custody = [], removed = [], menus = [];
  class Element {
    constructor(tag, cls = '', text = '') { this.tagName = tag.toUpperCase(); this.className = cls; this.textContent = text; this.children = []; this.dataset = {}; this.attrs = {}; this.style = {}; }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    append(...nodes) { for (const node of nodes) { node.remove(); node.parentElement = this; this.children.push(node); } }
    remove() { if (this.contains(document.activeElement)) document.activeElement = document.body; if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(node => node !== this); this.parentElement = null; }
    replaceChildren(...nodes) { for (const child of [...this.children]) child.remove(); this.append(...nodes); }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key] ?? null; }
    matches(selector) { return selector.split(',').some(value => {
      value = value.trim();
      if (value === '[data-source-row]') return Object.hasOwn(this.dataset, 'sourceRow');
      if (value.startsWith('.')) return this.className.split(' ').includes(value.slice(1));
      if (value.startsWith('#')) return this.id === value.slice(1);
      return this.tagName.toLowerCase() === value;
    }); }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    getBoundingClientRect() { return { left: 10, bottom: 30 }; }
    focus(options) { document.activeElement = this; this.lastFocus = options; }
  }
  document = { querySelector: selector => document.body.querySelector(selector) };
  document.body = new Element('body'); document.activeElement = document.body;
  const el = (...args) => new Element(...args), $ = selector => document.querySelector(selector);
  const add = el('button'); add.id = 'ws-source-add'; document.body.append(add);
  const artifact = { id: 'artifact-a' }, active = { artifacts: [artifact] };
  const sources = [
    { name: 'same.log', path: 'C:\\evidence\\first folder\\same.log', format: 'jsonl', count: 41, bytes: 3456, start: 1, end: 9, unparsed: 0, undated: 2 },
    { name: 'same.log', path: 'D:\\archives\\case.zip!/nested/<literal>/same.log', format: 'text', count: 42, bytes: 4567, start: null, end: null, unparsed: 3, sampled: 20 },
  ];
  let copy = async text => { copies.push(text); }, caseItem = active;
  const window = { AnalysisContexts: { capture: () => owner, isCurrent: value => value === owner },
    ContextMenu: { create(config) { return { config, open(x, y, items, options) { menus.push({ x, y, items, options, resolver: config.captureReturnFocus(options.trigger) }); } }; } } };
  const context = vm.createContext({ window, document, el, $, home: { hidden: false }, content: { hidden: false }, page: 'sources', sourceList: sources, state: { currentArtifact: artifact },
    sourceKey: () => key, navigator: { clipboard: { writeText: text => copy(text) } },
    toast: (text, type) => messages.push({ text, type }), fmtTs: value => `time ${value}`, fmtBytes: value => `${value} bytes`, fmtNum: String,
    activeCase: () => caseItem, recordCustody: (item, hashes) => { custody.push({ item, hashes }); return true; }, saveCases: () => custody.push('save'),
    openTsModal: async (path, current, returnFocus) => { const pending = deferred(); timestamps.push({ path, current, returnFocus, ...pending }); await pending.promise; if (current()) timestamps.push({ opened: path }); },
    api: (command, args) => { const pending = deferred(); requests.push({ command, args, ...pending }); return pending.promise; },
    removeSource: async index => removed.push(index),
  });
  vm.runInContext(source.slice(start, end), context);
  let table, view;
  const render = (nextSources = sources) => {
    table?.remove(); table = el('table'); document.body.append(table);
    view = { version: 1, key, owner, sources: nextSources, table, hashing: false };
    context.sourceList = nextSources; context.nextView = view;
    vm.runInContext('sourceView = nextView', context);
    nextSources.forEach((source, index) => table.append(context.buildSourceRow(source, index, view)));
    return table.children.map(row => row._sourceTarget);
  };
  let targets = render();
  const event = (target, extras = {}) => ({ target, type: 'click', preventDefault() {}, stopPropagation() {}, ...extras });
  const menu = target => { context.openSourceMenu(event(target.menu), target); return menus.at(-1); };
  return { context, document, sources, targets, copies, messages, requests, timestamps, custody, removed, menus, menu, event, add,
    render, view: () => view, table: () => table, key: value => { key = value; }, owner: value => { owner = value; }, case: value => { caseItem = value; },
    copy: fn => { copy = fn; }, };
}

const item = (menu, label) => menu.items.find(value => value.label === label);

test('same-named sources have exact accessible identity and a closed keyboard path disclosure', () => {
  const f = fixture();
  for (const [index, target] of f.targets.entries()) {
    assert.equal(target.toggle.tagName, 'BUTTON'); assert.equal(target.toggle.type, 'button');
    assert.equal(target.toggle.getAttribute('aria-expanded'), 'false'); assert.equal(target.details.hidden, true);
    assert.equal(target.toggle.getAttribute('aria-controls'), target.details.id);
    assert.ok(target.toggle.getAttribute('aria-label').endsWith(f.sources[index].path));
    const path = target.details.querySelector('textarea');
    assert.equal(path.value, f.sources[index].path); assert.equal(path.readOnly, true);
    assert.equal(path.getAttribute('data-native-context-menu'), '');
    assert.equal(target.menu.getAttribute('aria-haspopup'), 'menu');
    assert.ok(target.menu.getAttribute('aria-label').endsWith(f.sources[index].path));
    assert.equal(target.details.querySelectorAll('.source-hashes')[0].children.length, 0);
    assert.equal(f.table().children[index].querySelectorAll('.source-actions').length, 0);
  }
  assert.notEqual(f.targets[0].toggle.getAttribute('aria-label'), f.targets[1].toggle.getAttribute('aria-label'));
  assert.deepEqual(f.requests, []);
});

test('disclosing, copying exact Windows/archive paths and repeated collapse do not change source selection', async () => {
  const f = fixture(); f.table().children[1].querySelector('input').checked = false;
  for (const target of f.targets) {
    target.toggle.focus(); target.toggle.onclick(); assert.equal(target.details.hidden, false);
    assert.equal(target.toggle.getAttribute('aria-expanded'), 'true');
    await target.details.querySelector('button').onclick();
    target.details.querySelector('textarea').focus(); target.toggle.onclick();
    assert.equal(f.document.activeElement, target.toggle); assert.deepEqual(plain(target.toggle.lastFocus), { preventScroll: true });
    assert.equal(target.details.hidden, true);
  }
  assert.deepEqual(f.copies, f.sources.map(source => source.path));
  assert.deepEqual(f.table().children.map(row => row.querySelector('input').checked), [true, false]);
  assert.deepEqual(f.requests, []); assert.deepEqual(f.removed, []);
});

test('one named shared menu exposes occasional actions and focuses the logical source after same-context rerender', () => {
  const f = fixture(), target = f.targets[1], menu = f.menu(target);
  assert.equal(menu.options.trigger, target.menu); assert.equal(menu.options.label, 'Ações da fonte same.log');
  assert.deepEqual(plain(menu.items.filter(item => !item.sep).map(item => item.label)), ['Ver caminho completo', 'Copiar caminho', 'Data/hora', 'Calcular SHA-256', 'Remover da análise']);
  assert.equal(item(menu, 'Remover da análise').danger, true);
  const fresh = f.render([...f.sources].reverse().map(source => ({ ...source })));
  assert.equal(menu.resolver(), fresh[0].menu, 'return is by full path, never old row index');
  f.key('replacement'); assert.equal(menu.resolver(), null);
});

test('context-click uses the same menu and leaves textarea native selection/context untouched', () => {
  const f = fixture(), target = f.targets[0], row = f.table().children[0];
  row.oncontextmenu(f.event(target.toggle, { type: 'contextmenu', clientX: 101, clientY: 202 }));
  assert.equal(f.menus.at(-1).options.trigger, target.toggle); assert.equal(f.menus.at(-1).x, 101); assert.equal(f.menus.at(-1).y, 202);
  assert.equal(f.menus.at(-1).resolver(), target.toggle);
  row.oncontextmenu(f.event(target.details.querySelector('textarea'), { type: 'contextmenu' })); assert.equal(f.menus.length, 1);
  const checkbox = row.querySelector('input');
  row.oncontextmenu(f.event(checkbox, { type: 'contextmenu' }));
  assert.equal(f.menus.at(-1).options.trigger, checkbox); assert.equal(f.menus.at(-1).resolver(), checkbox);
  assert.equal(checkbox.checked, true);
});

test('captured actions reject source/owner/list replacement, navigation and mutated paths', async () => {
  for (const change of ['key', 'owner', 'list', 'page', 'path', 'home-hidden', 'content-hidden']) {
    const f = fixture(), target = f.targets[0], menu = f.menu(target);
    if (change === 'key') f.key('b'); if (change === 'owner') f.owner(2);
    if (change === 'list') f.render(f.sources.map(source => ({ ...source })));
    if (change === 'home-hidden') f.context.home.hidden = true; if (change === 'content-hidden') f.context.content.hidden = true;
    if (change === 'page') f.context.page = 'explore'; if (change === 'path') target.source.path = 'changed';
    for (const action of menu.items.filter(item => !item.sep)) await action.onClick();
    assert.deepEqual(f.copies, [], change); assert.deepEqual(f.timestamps, [], change); assert.deepEqual(f.requests, [], change); assert.deepEqual(f.removed, [], change);
  }
});

test('timestamp configuration keeps the exact path and rejects older replies and a replaced source', async () => {
  for (const change of ['newer', 'source', 'configuration']) {
    const f = fixture(), pending = f.context.configureSourceTime(f.targets[0]);
    assert.equal(f.timestamps[0].path, f.sources[0].path); assert.equal(f.timestamps[0].current(), true);
    let newer;
    if (change === 'newer') newer = f.context.configureSourceTime(f.targets[1]);
    else if (change === 'configuration') f.context.home.hidden = true; else f.owner(2);
    assert.equal(f.timestamps[0].current(), false); f.timestamps[0].resolve(); await pending;
    assert.equal(f.timestamps.some(value => value.opened), false);
    if (newer) { f.timestamps[1].resolve(); await newer; assert.equal(f.timestamps.at(-1).opened, f.sources[1].path); }
  }
});

test('hashing preserves the native command, prevents repeat work and associates exact own/package hashes', async () => {
  const f = fixture(), target = f.targets[1], pending = f.context.hashSources(target);
  assert.equal(target.status.hidden, false); assert.match(target.status.textContent, /Calculando/);
  await f.context.hashSources(f.targets[0]); assert.equal(f.requests.length, 1);
  assert.equal(item(f.menu(target), 'Calculando SHA-256…').disabled, true);
  assert.equal(f.requests[0].command, 'source_hashes'); assert.deepEqual(plain(f.requests[0].args), {});
  const hashes = [
    { path: f.sources[0].path, name: 'same.log', bytes: 3456, sha256: 'a'.repeat(64) },
    { path: f.sources[1].path, name: 'same.log', bytes: 4567, sha256: 'b'.repeat(64), origin: 'extraído' },
    { path: f.sources[1].path.split('!/')[0], name: 'case.zip', bytes: 7777, sha256: 'c'.repeat(64) },
  ];
  f.requests[0].resolve(hashes); await pending;
  assert.equal(f.targets[0].hashes.children.length, 1); assert.equal(target.hashes.children.length, 2);
  assert.equal(target.details.hidden, false); assert.equal(f.targets[0].details.hidden, true);
  await target.hashes.children[0].onclick(); await target.hashes.children[1].onclick();
  assert.deepEqual(f.copies, ['b'.repeat(64), 'c'.repeat(64)]);
  assert.equal(f.custody[0].item, f.context.state.currentArtifact); assert.equal(f.custody[0].hashes, hashes); assert.equal(f.custody[1], 'save');
  assert.equal(target.status.hidden, true); assert.equal(f.view().hashing, false);
});

test('late hashes cannot paint or record custody in a replacement or different Case', async () => {
  for (const change of ['owner', 'list', 'page', 'case', 'configuration']) {
    const f = fixture(), target = f.targets[0], pending = f.context.hashSources(target);
    if (change === 'owner') f.owner(2); if (change === 'list') f.render();
    if (change === 'configuration') f.context.home.hidden = true;
    if (change === 'page') f.context.page = 'explore'; if (change === 'case') f.case({ artifacts: [f.context.state.currentArtifact] });
    f.requests[0].resolve([{ path: target.path, sha256: 'd'.repeat(64), name: 'same.log', bytes: 1 }]); await pending;
    assert.deepEqual(f.custody, [], change);
    if (change !== 'case') assert.equal(target.hashes.children.length, 0, change);
  }
});

test('clipboard failures stay recoverable and removal delegates the actual index to its existing confirmation', async () => {
  const f = fixture(), target = f.targets[1]; f.copy(async () => { throw Error('denied'); });
  await f.context.copySourceText(target, target.path, 'copied');
  assert.equal(f.messages.at(-1).type, 'err'); assert.match(f.messages.at(-1).text, /Selecione o texto/);
  const pendingCopy = deferred(); f.copy(() => pendingCopy.promise);
  const pending = f.context.copySourceText(target, target.path, 'stale copied'); f.owner(2); pendingCopy.resolve(); await pending;
  assert.equal(f.messages.some(item => item.text === 'stale copied'), false);
  f.owner(1); await item(f.menu(target), 'Remover da análise').onClick(); assert.deepEqual(f.removed, [1]);
  assert.match(source, /if \(!confirm\(`Remover "\$\{target.name\}" da análise\?`\)\) return;/);
});


test('failed hashing restores the action for retry and stale failures stay quiet', async () => {
  for (const stale of [false, true]) {
    const f = fixture(), target = f.targets[0], pending = f.context.hashSources(target);
    if (stale) f.context.home.hidden = true;
    f.requests[0].reject(Error('read failed')); await pending;
    assert.equal(f.view().hashing, false);
    assert.equal(f.messages.length, stale ? 0 : 1);
    if (!stale) { assert.equal(target.status.hidden, true); assert.match(f.messages[0].text, /read failed/); assert.equal(item(f.menu(target), 'Calcular SHA-256').disabled, false); }
  }
});


test('Sources has a named keyboard-scrollable wrapper and keeps a readable minimum instead of clipping actions', () => {
  const css = readFileSync(new URL('../../frontend/workspace.css', import.meta.url), 'utf8');
  assert.match(source, /class="source-table-scroll" role="region" aria-label="Arquivos carregados" tabindex="0"/);
  assert.match(css, /\.source-table-scroll\{[^}]*overflow-x:auto/);
  assert.match(css, /\.sources-table\{[^}]*min-width:650px/);
  assert.match(css, /\.source-path-toggle\{[^}]*min-height:28px/);
});
