import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/styles.css', import.meta.url), 'utf8');
const source = app.slice(app.indexOf('const repeatableToastMessages ='), app.indexOf('// ------------------------------------------------------------------ indicador global'));

function harness({ width = 1024, height = 768 } = {}) {
  let document, now = 0, nextId = 0;
  const timers = new Map(), frames = new Map(), documentListeners = new Map(), windowListeners = new Map(), observers = new Set();
  const changed = target => { for (const observer of observers) if (observer.target?.contains(target)) observer.callback(); };
  const listenerAPI = listeners => ({
    addEventListener(name, callback) { listeners.set(name, callback); },
    removeEventListener(name, callback) { if (listeners.get(name) === callback) listeners.delete(name); },
  });
  class Node {
    constructor(tag = 'div', className = '', text = '') {
      this.tagName = tag.toUpperCase(); this.className = className; this.textContent = text; this.children = []; this.attributes = {};
      this.style = { removeProperty(name) { delete this[name]; } }; this.hidden = false;
      this.bounds = { x: 0, y: 0, width: 180, height: 36 };
    }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    appendChild(node) { this.children.push(node); node.parentElement = this; changed(this); return node; }
    remove() { const parent = this.parentElement; if (parent) parent.children.splice(parent.children.indexOf(this), 1); this.parentElement = null; if (parent) changed(parent); }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    getAttribute(name) { return this.attributes[name] ?? null; }
    getClientRects() { return this.isConnected && !this.hidden ? [this.getBoundingClientRect()] : []; }
    getBoundingClientRect() {
      const b = this.bounds, lane = ['toast-area', 'toast-feedback-area'].includes(this.id);
      let w = b.width, h = lane ? this.children.length * 44 : b.height;
      let x = b.x, y = b.y;
      if (lane) { x = window.innerWidth - 18 - w; y = window.innerHeight - 18 - h; }
      else if (this.parentElement && (this.parentElement.id?.startsWith('toast-') || this.parentElement.className.startsWith('toast '))) {
        const parent = this.parentElement.getBoundingClientRect();
        x = parent.x; y = parent.y + this.parentElement.children.indexOf(this) * 44;
        if (this.tagName === 'BUTTON') { x += 10; y = parent.y + 4; w = 80; h = 28; }
      }
      if (this.style.left !== undefined) x = Number.parseFloat(this.style.left);
      if (this.style.top !== undefined) y = Number.parseFloat(this.style.top);
      return { x, y, width: w, height: h, left: x, top: y, right: x + w, bottom: y + h };
    }
    focus() { document.activeElement = this; documentListeners.get('focusin')?.(); }
  }
  const obstacles = [];
  document = { body: new Node('body'), documentElement: new Node('html'), querySelectorAll: () => obstacles, ...listenerAPI(documentListeners) };
  document.activeElement = document.body;
  const interactiveArea = document.body.appendChild(new Node()); interactiveArea.id = 'toast-area';
  const window = { innerWidth: width, innerHeight: height, ...listenerAPI(windowListeners) };
  const context = vm.createContext({ document, window, $: () => interactiveArea, el: (...args) => new Node(...args),
    MutationObserver: class {
      constructor(callback) { this.callback = callback; }
      observe(target) { this.target = target; observers.add(this); }
      disconnect() { observers.delete(this); }
    },
    setTimeout(fn, ms) { const id = ++nextId; timers.set(id, { fn, at: now + ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
    requestAnimationFrame(fn) { const id = ++nextId; frames.set(id, fn); return id; },
    cancelAnimationFrame(id) { frames.delete(id); },
  });
  vm.runInContext(source, context);
  const flushFrames = () => { for (const [id, fn] of [...frames]) { frames.delete(id); fn(); } };
  const advance = ms => {
    const end = now + ms;
    while ([...timers.values()].some(timer => timer.at <= end)) {
      const [id, timer] = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
      now = timer.at; timers.delete(id); timer.fn();
    }
    now = end;
  };
  const control = bounds => { const node = document.body.appendChild(new Node('button')); node.bounds = bounds; return node; };
  const undoStart = app.indexOf('      const notice = el("div", "toast ok"');
  const undoCode = app.slice(undoStart, app.indexOf('\n    }', undoStart));
  return { context, document, window, interactiveArea, obstacles, timers, frames, documentListeners, windowListeners, observers, Node, control, advance, flushFrames,
    get area() { return document.body.children.find(node => node.id === 'toast-feedback-area') || null; },
    get notices() { return this.area?.children || []; },
    toast(message, type = 'ok') { context.toast(message, type); flushFrames(); return this.notices.at(-1); },
    appendRealUndo() {
      Object.assign(context, { transaction: { count: 1 }, record: {}, undoCaseOccurrenceRemoval: async () => false });
      vm.runInContext(`{ ${undoCode} }`, context);
      return interactiveArea.children.at(-1).children[0];
    },
    records: () => vm.runInContext('toastFeedback.size', context),
  };
}

const intersects = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;

test('rapid copies reuse one confirmation and renew only its cleanup timer', () => {
  const h = harness(), focused = h.control({ x: 20, y: 100, width: 200, height: 40 }); focused.focus();
  const first = h.toast('Valor copiado.');
  h.advance(3000);
  for (let i = 0; i < 4; i++) h.toast('Valor copiado.');
  assert.equal(h.notices.length, 1); assert.equal(h.notices[0], first);
  assert.equal(first.children[0].textContent, 'Valor copiado.'); assert.equal(first.children[1].textContent, '×5');
  assert.equal(h.document.activeElement, focused); assert.equal(h.timers.size, 1);
  h.advance(4199); assert.equal(h.notices.length, 1);
  h.advance(1); assert.equal(h.notices.length, 0); assert.equal(h.records(), 0); assert.equal(h.timers.size, 0);
  assert.equal(h.documentListeners.size, 0); assert.equal(h.windowListeners.size, 0); assert.equal(h.frames.size, 0);
  assert.equal(h.area, null); assert.equal(h.observers.size, 0);
  for (const prop of ['top', 'right', 'bottom', 'left']) assert.equal(h.interactiveArea.style[prop], undefined);
});

test('each supported copy/filter message coalesces, but only an exact benign match', () => {
  const h = harness();
  for (const message of ['Copiado.', 'Valor copiado.', 'Nome copiado.', 'JSON copiado.', 'SHA-256 copiado.', 'Filtro atualizado.', 'Filtro adicionado.']) {
    h.toast(message); h.toast(message);
  }
  assert.equal(h.notices.length, 7);
  for (const node of h.notices) assert.equal(node.children[1].textContent, '×2');
  h.toast('Filtro atualizado. ', 'ok'); h.toast('Filtro atualizado. ', 'ok');
  assert.equal(h.notices.length, 9, 'no trimming or approximate matching can silently group different information');
});

test('errors, context warnings and evidence receipts remain separate even when equal', () => {
  const h = harness();
  for (const [message, type] of [['Não foi possível copiar.', 'err'], ['O contexto mudou.', 'info'], ['1 ocorrência(s) removida(s). Use Desfazer para restaurar.', 'ok']]) {
    h.toast(message, type); h.toast(message, type);
  }
  assert.equal(h.notices.length, 6); assert.equal(h.timers.size, 6);
  assert.equal(h.notices.some(node => node.className.includes('toast-passive')), false, 'errors, warnings and receipts are selectable rather than click-through');
  assert.deepEqual(h.notices.map(node => node.getAttribute('role')), ['alert', 'alert', 'status', 'status', 'status', 'status']);
  h.advance(4200); assert.equal(h.records(), 0);
});

test('accessible live content is concise and repeats do not rewrite it or move focus', () => {
  const h = harness(); h.context.toast('JSON copiado.', 'ok');
  const node = h.notices[0], label = node.children[0], badge = node.children[1];
  assert.equal(node.getAttribute('role'), 'status'); assert.equal(node.getAttribute('aria-atomic'), 'true');
  assert.equal(label.textContent, '', 'live region is connected before the text is announced');
  h.flushFrames(); assert.equal(label.textContent, 'JSON copiado.');
  let writes = 0;
  Object.defineProperty(label, 'textContent', { get: () => 'JSON copiado.', set: () => { writes++; } });
  h.toast('JSON copiado.');
  assert.equal(writes, 0); assert.equal(badge.getAttribute('aria-hidden'), 'true'); assert.equal(badge.hidden, false);
  assert.equal(h.document.activeElement, h.document.body);
});

test('detached and expired acknowledgements start fresh without stale timers or counts', () => {
  const h = harness(), first = h.toast('Filtro atualizado.'); first.remove();
  const second = h.toast('Filtro atualizado.');
  assert.notEqual(first, second); assert.equal(h.records(), 1); assert.equal(h.timers.size, 1); assert.equal(second.children[1].hidden, true);
  h.advance(4200); const third = h.toast('Filtro atualizado.');
  assert.notEqual(second, third); assert.equal(third.children[1].hidden, true);
  h.context.toast('Valor copiado.', 'ok');
  h.advance(4200); assert.equal(h.frames.size, 0, 'expiry also cancels a pending announcement frame');
});

test('feedback clears focused controls and visible editor actions without moving focus', () => {
  const h = harness(), focused = h.control({ x: 820, y: 700, width: 160, height: 40 }); focused.focus();
  h.toast('Valor copiado.');
  assert.equal(intersects(h.area.getBoundingClientRect(), focused.getBoundingClientRect()), false);
  const editor = h.control({ x: 18, y: 660, width: 320, height: 100 }); h.obstacles.push(editor);
  h.context.positionToastFeedback();
  assert.equal(intersects(h.area.getBoundingClientRect(), focused.getBoundingClientRect()), false);
  assert.equal(intersects(h.area.getBoundingClientRect(), editor.getBoundingClientRect()), false);
  assert.equal(h.document.activeElement, focused);
  focused.bounds = { x: 810, y: 18, width: 190, height: 40 }; focused.focus();
  assert.equal(intersects(h.area.getBoundingClientRect(), focused.getBoundingClientRect()), false);
  h.window.innerWidth = 600; h.window.innerHeight = 400; h.windowListeners.get('resize')();
  assert.ok(h.area.getBoundingClientRect().right <= 600);
});

test('real Undo appended after positioned feedback grows inside its own bottom-anchored lane', () => {
  const h = harness({ width: 1024, height: 768 }); h.toast('Valor copiado.');
  const managedLane = h.area, undo = h.appendRealUndo();
  const rect = undo.getBoundingClientRect();
  assert.notEqual(managedLane, h.interactiveArea); assert.equal(managedLane.parentElement, h.document.body);
  assert.equal(h.interactiveArea.children.length, 1); assert.equal(h.notices.length, 1);
  assert.ok(rect.top >= 0 && rect.bottom <= 768, 'real append grows upward from the original bottom anchor');
  assert.equal(intersects(h.area.getBoundingClientRect(), h.interactiveArea.getBoundingClientRect()), false, 'legacy append moves only managed feedback out of the way');
  h.appendRealUndo();
  assert.ok(h.interactiveArea.getBoundingClientRect().bottom <= 768);
  assert.ok(h.interactiveArea.getBoundingClientRect().top >= 0);
  assert.equal(intersects(h.area.getBoundingClientRect(), h.interactiveArea.getBoundingClientRect()), false);
  for (const prop of ['top', 'right', 'bottom', 'left']) assert.equal(h.interactiveArea.style[prop], undefined);
});

test('passive expiration and feedback growth leave the actual Undo button rectangle unchanged without focus', () => {
  const h = harness(); h.toast('Valor copiado.'); const undo = h.appendRealUndo();
  const before = undo.getBoundingClientRect();
  assert.equal(h.document.activeElement, h.document.body);
  h.advance(2000); h.toast('JSON copiado.'); h.toast('Não foi possível copiar.', 'err');
  assert.deepEqual(undo.getBoundingClientRect(), before, 'managed growth cannot shift a sibling in the legacy lane');
  h.advance(2200); assert.deepEqual(undo.getBoundingClientRect(), before, 'first acknowledgement expires while other feedback remains');
  h.advance(2000); assert.deepEqual(undo.getBoundingClientRect(), before, 'last managed notice/lane cleanup cannot move Undo');
  assert.equal(h.area, null); assert.equal(h.records(), 0); assert.equal(h.observers.size, 0);
  assert.equal(undo.isConnected, true); assert.equal(h.timers.size, 1, 'only the real independent 15 s Undo timer remains');
  h.advance(8800); assert.equal(undo.isConnected, false, 'the original Undo lifetime still owns its cleanup');
});

test('managed expiration cannot change focused Undo geometry, focus or action', async () => {
  const h = harness(); h.toast('Valor copiado.'); const undo = h.appendRealUndo();
  const before = undo.getBoundingClientRect(); undo.focus();
  h.toast('Valor copiado.'); h.advance(4200);
  assert.deepEqual(undo.getBoundingClientRect(), before); assert.equal(h.document.activeElement, undo);
  assert.equal(h.records(), 0); assert.equal(h.interactiveArea.children.length, 1);
  await undo.onclick(); assert.equal(undo.disabled, false); assert.equal(undo.isConnected, true);
});

test('only benign feedback is click-through; legacy lane and typography stay unchanged', () => {
  assert.match(css, /#toast-area\s*\{[^}]*bottom: 18px; right: 18px; z-index: 120;[^}]*flex-direction: column; gap: 8px;/s);
  assert.doesNotMatch(css.match(/#toast-area\s*\{([^}]*)\}/s)[1], /pointer-events|align-items|top:|left:/);
  assert.match(css, /#toast-feedback-area\s*\{[^}]*max-width: calc\(100vw - 36px\); pointer-events: none/s);
  assert.match(css, /\.toast\s*\{[^}]*font-size: 12\.5px; max-width: 420px;/s);
  assert.match(css, /\.toast-feedback\s*\{[^}]*overflow-wrap: anywhere;[^}]*pointer-events: auto; user-select: text;/s);
  assert.match(css, /\.toast-passive\s*\{[^}]*pointer-events: none/s);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{ \.toast-feedback \{ animation: none;/);
});
