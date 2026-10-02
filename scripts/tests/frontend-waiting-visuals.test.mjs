import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const receipt = (overrides = {}) => ({ operationId: 'load-17', phaseId: 'metadata-scan', state: 'running', label: 'Indexando metadados', ...overrides });

function fixture({ reduced = false, intersection = true, legacyMedia = false } = {}) {
  class Target {
    listeners = new Map();
    addEventListener(type, callback) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(callback); }
    removeEventListener(type, callback) { this.listeners.get(type)?.delete(callback); }
    emit(type) { for (const callback of this.listeners.get(type) || []) callback({ type }); }
    count(type) { return this.listeners.get(type)?.size || 0; }
  }
  class Node extends Target {
    constructor(document, tag) { super(); this.ownerDocument = document; this.tag = tag; this.children = []; this.dataset = {}; this.attributes = {}; this.hidden = false; this.writes = 0; this.htmlWrites = 0; }
    append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter(node => node !== this); this.parent = null; }
    get isConnected() { return this === this.ownerDocument.body || !!this.parent?.isConnected; }
    set textContent(value) { this.text = String(value); this.writes++; }
    get textContent() { return this.text || ''; }
    set innerHTML(value) { this.html = value; this.htmlWrites++; }
    get innerHTML() { return this.html || ''; }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    getAttribute(name) { return this.attributes[name] ?? null; }
  }
  const document = new Target(); document.hidden = false;
  document.createElement = name => new Node(document, name);
  document.body = document.createElement('body');
  const host = document.createElement('div'); document.body.append(host);
  const media = new Target(); media.matches = reduced;
  if (legacyMedia) {
    media.addListener = callback => Target.prototype.addEventListener.call(media, 'change', callback);
    media.removeListener = callback => Target.prototype.removeEventListener.call(media, 'change', callback);
    media.addEventListener = undefined; media.removeEventListener = undefined;
  }
  const observers = [];
  class Observer {
    constructor(callback, options) { this.callback = callback; this.options = options; this.disconnected = false; observers.push(this); }
    observe(target) { this.target = target; }
    deliver(isIntersecting, intersectionRatio = isIntersecting ? 1 : 0) { this.callback([{ target: this.target, isIntersecting, intersectionRatio }]); }
    disconnect() { this.disconnected = true; }
  }
  const window = { matchMedia: () => media, ...(intersection ? { IntersectionObserver: Observer } : {}) };
  document.defaultView = window;
  const context = vm.createContext({ window, document, Intl }); vm.runInContext(source, context);
  const api = window.WaitingVisuals;
  return { api, host, document, media, observers, parts: visual => {
    const [art, status, metric, details, control] = visual.element.children;
    return { art, status, metric, details, control };
  } };
}

test('only exact real phase IDs select one of three scene families', () => {
  const { api } = fixture();
  assert.equal(api.derive(receipt()).family, 'reading');
  for (const phaseId of ['metadata-checkpoint-write', 'metadata-checkpoint-sync', 'metadata-checkpoint-publish']) assert.equal(api.derive(receipt({ phaseId })).family, 'checkpoint');
  for (const phaseId of ['analytics-select', 'analytics-verify', 'analytics-sql', 'command:aggregate_events', 'command:pivot']) assert.equal(api.derive(receipt({ phaseId })).family, 'calculation');
  for (const phaseId of ['', 'future-phase', 'METADATA-SCAN', 'toString', '__proto__']) {
    const model = api.derive(receipt({ phaseId, label: 'Calculando estatísticas 100%' }));
    assert.equal(model.family, 'neutral'); assert.equal(model.canAnimate, false);
  }
  assert.equal(api.derive(receipt({ operationId: '' })).family, 'neutral');
  assert.equal(api.derive(receipt({ operationId: 42 })).canAnimate, false);
});

test('subtle variation is deterministic per operation/family and stable across receipts', () => {
  const { api } = fixture();
  const a = api.derive(receipt({ operationId: 'op-1' }));
  const b = api.derive(receipt({ operationId: 'op-2' }));
  assert.notEqual(a.variant, b.variant);
  for (let completed = 0; completed < 5; completed++) {
    assert.equal(api.derive(receipt({ operationId: 'op-1', completed, elapsedMs: completed * 3000 })).variant, a.variant);
  }
  for (const phaseId of ['metadata-scan', 'metadata-checkpoint-write', 'analytics-sql']) {
    assert.ok(['a', 'b'].includes(api.derive(receipt({ phaseId })).variant));
  }
});

test('stopped, unknown and terminal states stay static without inferring success', () => {
  const { api } = fixture();
  for (const state of ['paused', 'queued', 'cancelling', 'cancelled', 'error', 'completed', 'idle', 'unknown', 'ready', '', undefined, 'toString']) {
    const model = api.derive(receipt({ state }));
    assert.equal(model.family, 'neutral', String(state)); assert.equal(model.canAnimate, false);
  }
  const allCounted = api.derive(receipt({ completed: 100, total: 100 }));
  assert.equal(allCounted.state, 'running'); assert.equal(allCounted.canAnimate, true);
  assert.doesNotMatch(allCounted.status, /concluído|sucesso|100%/i);
  assert.equal(api.derive(null).family, 'neutral');
});

test('checkpoint committed remains a partial, static checkpoint even at the supplied total', () => {
  const model = fixture().api.derive(receipt({ phaseId: 'metadata-checkpoint-committed', label: '', completed: 500, total: 500, elapsedMs: 9000, estimateMs: 0 }));
  assert.equal(model.family, 'checkpoint'); assert.equal(model.partialCheckpoint, true);
  assert.equal(model.canAnimate, false); assert.equal(model.state, 'running');
  assert.equal(model.label, 'Checkpoint preservado'); assert.equal(model.metric, '500 / 500');
  assert.deepEqual(plain(model.details), ['9 s decorridos']);
});

test('only supplied valid measurements appear; no fabricated totals, phase counts or estimates', () => {
  const { api } = fixture();
  const empty = api.derive(receipt());
  assert.equal(empty.metric, ''); assert.deepEqual(plain(empty.details), []); assert.equal(empty.motionMode, 'gesture');
  for (const invalid of [null, undefined, -1, Infinity, NaN, '100', false, Number.MAX_SAFE_INTEGER + 1]) {
    const model = api.derive(receipt({ completed: invalid, total: 100, elapsedMs: invalid, estimateMs: invalid, phaseIndex: invalid, phaseCount: 4 }));
    assert.equal(model.metric, ''); assert.deepEqual(plain(model.details), []);
  }
  assert.equal(api.derive(receipt({ total: 50 })).metric, '', 'no invented zero completed');
  assert.equal(api.derive(receipt({ completed: 12, total: 0, unit: 'registros' })).metric, '12 registros');
  assert.equal(api.derive(receipt({ completed: 12, total: 10 })).metric, '12');
  assert.equal(api.derive(receipt({ completed: 0, total: 100 })).metric, '0 / 100');
  const measured = api.derive(receipt({ completed: 1200, total: 3000, unit: 'registros', phaseIndex: 2, phaseCount: 4, elapsedMs: 65000, estimateMs: 12000 }));
  assert.equal(measured.metric, '1.200 / 3.000 registros');
  assert.deepEqual(plain(measured.details), ['Etapa 2 de 4', '1 min 5 s decorridos', '≈ 12 s restantes nesta etapa']);
  assert.equal(measured.motionMode, 'loop');
  assert.deepEqual(plain(api.derive(receipt({ phaseIndex: 3, phaseCount: 2 })).details), ['Etapa 3']);
  assert.deepEqual(plain(api.derive(receipt({ phaseCount: 7 })).details), []);
});

test('mount uses safe text, decorative SVG and one concise live status', () => {
  const f = fixture(); const attack = '<img src=x onerror=alert(1)>';
  const visual = f.api.mount(f.host, receipt({ label: attack, unit: attack, completed: 2 }));
  const { art, status, metric, details, control } = f.parts(visual);
  assert.equal(f.host.children.length, 1); assert.equal(status.textContent, attack);
  assert.equal(metric.textContent, `2 ${attack}`); assert.doesNotMatch(art.innerHTML, /<img|onerror/);
  assert.equal(status.getAttribute('role'), 'status'); assert.equal(status.getAttribute('aria-live'), 'polite');
  assert.equal(art.getAttribute('aria-hidden'), 'true'); assert.match(art.innerHTML, /aria-hidden="true" focusable="false"/);
  assert.equal(details.hidden, true); assert.equal(control.type, 'button');
  assert.equal(visual.element.dataset.motion, 'static', 'does not start until visibility is established');
  visual.destroy();
});

test('receipt updates replace missing values without rerendering scenes or announcing counters', () => {
  const f = fixture(); const visual = f.api.mount(f.host, receipt({ completed: 1, total: 10 }), { showDetails: true });
  const { art, status, metric, details } = f.parts(visual); const statusWrites = status.writes;
  f.observers[0].deliver(true);
  visual.update(receipt({ completed: 2, total: 10, elapsedMs: 4500 }));
  assert.equal(art.htmlWrites, 1); assert.equal(status.writes, statusWrites); assert.equal(metric.textContent, '2 / 10');
  assert.equal(details.textContent, '4 s decorridos'); assert.equal(details.hidden, false);
  assert.equal(visual.element.dataset.pace, 'loop');
  visual.update(receipt({ phaseId: 'analytics-sql', label: 'Calculando estatísticas' }));
  assert.equal(art.htmlWrites, 2); assert.equal(status.textContent, 'Calculando estatísticas');
  assert.equal(metric.hidden, true); assert.equal(details.hidden, true);
  visual.update(receipt({ operationId: 'load-18', phaseId: 'analytics-sql' }));
  assert.equal(art.htmlWrites, 3, 'a different operation gets a fresh scene'); visual.destroy();
});

test('manual pause never cancels work, survives receipts, and can be resumed', () => {
  const f = fixture(); const visual = f.api.mount(f.host, receipt({ elapsedMs: 5000 }));
  const { control } = f.parts(visual); f.observers[0].deliver(true);
  assert.equal(visual.element.dataset.motion, 'running');
  control.emit('click'); assert.equal(visual.element.dataset.motion, 'static');
  assert.equal(control.textContent, 'Retomar animação'); assert.match(control.getAttribute('aria-label'), /a operação continua/);
  visual.update(receipt({ completed: 55, elapsedMs: 10000 })); assert.equal(visual.element.dataset.motion, 'static');
  control.emit('click'); assert.equal(visual.element.dataset.motion, 'running');
  visual.setMotionEnabled(false); assert.equal(visual.element.dataset.motion, 'static'); visual.destroy();
});

test('offscreen, hidden tab, explicit hidden panel and detached roots do not animate', () => {
  const f = fixture(); const visual = f.api.mount(f.host, receipt({ elapsedMs: 9000 })); const observer = f.observers[0];
  observer.deliver(true); assert.equal(visual.element.dataset.motion, 'running');
  observer.deliver(false); assert.equal(visual.element.dataset.motion, 'static');
  observer.deliver(true, 0); assert.equal(visual.element.dataset.motion, 'static');
  observer.deliver(true); f.document.hidden = true; f.document.emit('visibilitychange'); assert.equal(visual.element.dataset.motion, 'static');
  f.document.hidden = false; f.document.emit('visibilitychange'); assert.equal(visual.element.dataset.motion, 'running');
  visual.setVisible(false); assert.equal(visual.element.hidden, true); assert.equal(visual.element.dataset.motion, 'static');
  visual.update(receipt()); assert.equal(visual.element.dataset.motion, 'static');
  visual.setVisible(true); assert.equal(visual.element.hidden, false); assert.equal(visual.element.dataset.motion, 'running');
  f.host.remove(); visual.update(receipt()); assert.equal(visual.element.dataset.motion, 'static'); visual.destroy();
});

test('reduced motion has a complete static equivalent and respects system changes', () => {
  for (const legacyMedia of [false, true]) {
    const f = fixture({ reduced: true, legacyMedia }); const visual = f.api.mount(f.host, receipt({ elapsedMs: 9000 }));
    const { art, status, control } = f.parts(visual); f.observers[0].deliver(true);
    assert.equal(visual.element.dataset.motion, 'static'); assert.equal(control.hidden, true);
    assert.match(art.innerHTML, /wv-read-card/); assert.equal(status.textContent, 'Indexando metadados');
    f.media.matches = false; f.media.emit('change'); assert.equal(visual.element.dataset.motion, 'running'); assert.equal(control.hidden, false);
    f.media.matches = true; f.media.emit('change'); assert.equal(visual.element.dataset.motion, 'static');
    visual.destroy(); assert.equal(f.media.count('change'), 0);
  }
  const f = fixture({ intersection: false }); const visual = f.api.mount(f.host, receipt({ elapsedMs: 9000 }));
  assert.equal(visual.element.dataset.motion, 'static', 'no observer fails safely to static'); visual.destroy();
});

test('terminal transitions stop immediately, with no minimum animation completion delay', () => {
  const f = fixture(); const visual = f.api.mount(f.host, receipt({ elapsedMs: 9000 })); f.observers[0].deliver(true);
  for (const state of ['paused', 'cancelling', 'error', 'cancelled', 'completed']) {
    visual.update(receipt({ state })); assert.equal(visual.element.dataset.motion, 'static'); assert.equal(visual.element.dataset.family, 'neutral');
    assert.equal(f.parts(visual).control.hidden, true);
    visual.update(receipt()); assert.equal(visual.element.dataset.motion, 'running');
  }
  visual.update(receipt({ phaseId: 'metadata-checkpoint-committed' }));
  assert.equal(visual.element.dataset.motion, 'static'); assert.equal(visual.element.dataset.checkpoint, 'preserved'); visual.destroy();
});

test('destroy is idempotent, removes only its own node/listeners, and ignores late callbacks', () => {
  const f = fixture(); const sibling = f.document.createElement('aside'); f.host.append(sibling);
  const visual = f.api.mount(f.host, receipt()); const { art, control } = f.parts(visual); const observer = f.observers[0];
  assert.equal(f.document.count('visibilitychange'), 1); assert.equal(f.media.count('change'), 1);
  visual.destroy(); visual.destroy();
  assert.equal(f.host.children.length, 1); assert.equal(f.host.children[0], sibling);
  assert.equal(observer.disconnected, true); assert.equal(f.document.count('visibilitychange'), 0); assert.equal(f.media.count('change'), 0); assert.equal(control.count('click'), 0);
  const writes = art.htmlWrites; observer.deliver(true); f.document.emit('visibilitychange'); f.media.emit('change');
  assert.equal(visual.update(receipt()), false); visual.setVisible(true); visual.setMotionEnabled(true);
  assert.equal(art.htmlWrites, writes); assert.equal(visual.element.dataset.motion, 'static');
});

test('two instances keep independent visibility, user controls and destruction', () => {
  const f = fixture(); const a = f.api.mount(f.host, receipt()); const b = f.api.mount(f.host, receipt({ operationId: 'different' }));
  f.observers[0].deliver(true); f.observers[1].deliver(true);
  a.setMotionEnabled(false); assert.equal(a.element.dataset.motion, 'static'); assert.equal(b.element.dataset.motion, 'running');
  a.destroy(); assert.equal(f.document.count('visibilitychange'), 1); assert.equal(b.element.isConnected, true);
  f.document.hidden = true; f.document.emit('visibilitychange'); assert.equal(b.element.dataset.motion, 'static'); b.destroy();
});

test('motion budget is CSS-only, constrained and independently reduced-motion safe', () => {
  assert.doesNotMatch(source, /\b(?:setTimeout|setInterval|requestAnimationFrame|fetch|XMLHttpRequest)\s*\(/);
  assert.doesNotMatch(source, /(?:src|href)=["']https?:/);
  assert.doesNotMatch(css, /\b(?:filter|backdrop-filter|will-change)\s*:/);
  assert.match(css, /prefers-reduced-motion:\s*reduce/); assert.match(css, /animation:\s*none\s*!important/);
  assert.match(css, /\[data-motion="running"\]/); assert.match(css, /contain:\s*layout paint/);
  assert.match(css, /\.area-loading-semantic \.wv-motion-toggle\s*\{\s*pointer-events:\s*auto/);
  assert.doesNotMatch(css, /pointer-events:\s*none/);
  for (const keyframes of css.split('@keyframes ').slice(1)) {
    const body = keyframes.split('@')[0];
    assert.doesNotMatch(body, /\b(?:width|height|left|top|margin|padding|filter|box-shadow)\s*:/);
  }
});
