import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const receipt = (overrides = {}) => ({ operationId: 'load-17', phaseId: 'metadata-scan', state: 'running', label: 'Indexando metadados', ...overrides });

function fixture({ reduced = false, intersection = true, legacyMedia = false, now = () => 0 } = {}) {
  class Target {
    listeners = new Map();
    addEventListener(type, callback) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(callback); }
    removeEventListener(type, callback) { this.listeners.get(type)?.delete(callback); }
    emit(type, event = {}) { for (const callback of this.listeners.get(type) || []) callback({ type, ...event }); }
    count(type) { return this.listeners.get(type)?.size || 0; }
  }
  class Node extends Target {
    constructor(document, tag) { super(); this.ownerDocument = document; this.tag = tag; this.children = []; this.dataset = {}; this.attributes = {}; this.hidden = false; this.writes = 0; this.htmlWrites = 0; }
    append(...nodes) { for (const node of nodes) { node.remove(); node.parent = this; this.children.push(node); } }
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
  const window = { performance: { now }, matchMedia: () => media, ...(intersection ? { IntersectionObserver: Observer } : {}) };
  document.defaultView = window;
  const context = vm.createContext({ window, document, Intl }); vm.runInContext(source, context);
  const api = window.WaitingVisuals;
  return { api, host, document, media, observers, parts: visual => {
    const [art, status, metric, details, control] = visual.element.children;
    return { art, status, metric, details, control };
  } };
}

test('only exact real phase IDs select a known scene family', () => {
  const { api } = fixture();
  assert.equal(api.derive(receipt()).family, 'reading');
  assert.equal(api.derive(receipt({phaseId:'command:case_report_render'})).family, 'composition');
  for (const phaseId of ['metadata-checkpoint-write', 'metadata-checkpoint-sync', 'metadata-checkpoint-publish']) assert.equal(api.derive(receipt({ phaseId })).family, 'checkpoint');
  for (const phaseId of ['analytics-select', 'analytics-sql', 'command:aggregate_events', 'command:pivot']) assert.equal(api.derive(receipt({ phaseId })).family, 'calculation');
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
  assert.equal(visual.element.dataset.pace, 'gesture', 'receipt does not cut the gesture');
  gestureBoundary(f, visual);
  assert.equal(visual.element.dataset.pace, 'loop');
  visual.update(receipt({ phaseId: 'analytics-sql', label: 'Calculando estatísticas' }));
  assert.equal(art.htmlWrites, 1, 'phase receipt does not cut the active cycle'); assert.equal(status.textContent, 'Calculando estatísticas');
  reactionBoundary(f, visual);
  assert.equal(art.htmlWrites, 2); assert.equal(visual.element.dataset.family, 'calculation');
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
  visual.setVisible(false); assert.equal(visual.element.dataset.visible, 'false'); assert.equal(visual.element.inert, true); assert.equal(visual.element.dataset.motion, 'static');
  visual.update(receipt()); assert.equal(visual.element.dataset.motion, 'static');
  visual.setVisible(true); assert.equal(visual.element.dataset.visible, 'true'); assert.equal(visual.element.inert, false); assert.equal(visual.element.dataset.motion, 'running');
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
  assert.equal(visual.element.dataset.motion, 'running'); assert.equal(visual.element.dataset.checkpoint, 'preserved'); visual.destroy();
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
  assert.match(css, /prefers-reduced-motion:\s*reduce/); assert.match(css, /animation-play-state:\s*paused\s*!important/);
  assert.match(css, /\[data-animated="true"\]/); assert.match(css, /contain:\s*layout paint/);
  assert.match(css, /\.area-loading-semantic \.wv-motion-toggle\s*\{\s*pointer-events:\s*auto/);
  assert.match(css, /\[data-finishing="true"\][^{}]*\{ pointer-events: none !important;/);
  for (const { name, frames } of animationFrames().values()) {
    for (const frame of frames) {
      assert.ok(Object.keys(frame.properties).every(property => ['transform', 'opacity'].includes(property)), `${name} only composites transform/opacity`);
    }
  }
});

test('area scenes hide stale table values behind an opaque local surface without altering the art', () => {
  const body = css.split('.area-loading-semantic > .waiting-visual {')[1]?.split('}')[0];
  assert.ok(body, 'the surface is scoped to regional waits, not the foreground loading card');
  assert.match(body, /background:\s*var\(--bg-1\)/);
  assert.match(body, /padding:\s*8px 10px/);
  assert.doesNotMatch(body, /opacity:|font-size:|transform:|animation:|filter:|height:/);
});


// Parse balanced keyframe blocks: normal rules between animations are not motion.
let cachedAnimationFrames;
function animationFrames() {
  if (cachedAnimationFrames) return cachedAnimationFrames;
  const result = new Map();
  const headers = /@keyframes\s+([\w-]+)\s*\{/g;
  for (let match; (match = headers.exec(css));) {
    let end = headers.lastIndex, depth = 1;
    while (depth && end < css.length) { if (css[end] === '{') depth++; if (css[end] === '}') depth--; end++; }
    assert.equal(depth, 0, `closed keyframe block ${match[1]}`);
    const body = css.slice(headers.lastIndex, end - 1), frames = [];
    for (const rule of body.matchAll(/([^{}]+)\{([^{}]+)\}/g)) {
      const properties = Object.fromEntries([...rule[2].matchAll(/([\w-]+)\s*:\s*([^;]+);/g)].map(item => [item[1], item[2].trim()]));
      for (const selector of rule[1].trim().split(',')) {
        assert.match(selector.trim(), /^(?:\d+(?:\.\d+)?%|from|to)$/, `valid percentage in ${match[1]}: ${selector}`);
        const time = selector.trim() === 'from' ? 0 : selector.trim() === 'to' ? 100 : parseFloat(selector);
        assert.ok(time >= 0 && time <= 100);
        frames.push({ time, properties });
      }
    }
    result.set(match[1], { name: match[1], frames: frames.sort((a, b) => a.time - b.time) });
    headers.lastIndex = end;
  }
  cachedAnimationFrames = result;
  return result;
}

function atContact(name, time, property = 'transform') {
  const frames = animationFrames().get(name).frames.filter(frame => property in frame.properties);
  const exact = frames.find(frame => frame.time === time);
  if (exact) return exact.properties[property];
  const before = frames.filter(frame => frame.time < time).at(-1), after = frames.find(frame => frame.time > time);
  assert.equal(before.properties[property], after.properties[property], `${name} is held at this contact`);
  return before.properties[property];
}

function transformPoint(point, value, origin = [0, 0]) {
  for (const match of [...value.matchAll(/([a-zA-Z]+)\(([^)]+)\)/g)].reverse()) {
    const values = [...match[2].matchAll(/-?\d*\.?\d+(?:e[-+]?\d+)?/g)].map(item => Number(item[0]));
    if (match[1] === 'rotate') {
      const angle = values[0] * Math.PI / 180, [x, y] = [point[0] - origin[0], point[1] - origin[1]];
      point = [origin[0] + x * Math.cos(angle) - y * Math.sin(angle), origin[1] + x * Math.sin(angle) + y * Math.cos(angle)];
    } else if (match[1] === 'translateX') point = [point[0] + values[0], point[1]];
    else if (match[1] === 'scaleX') point = [origin[0] + (point[0] - origin[0]) * values[0], point[1]];
    else if (match[1] === 'translateY') point = [point[0], point[1] + values[0]];
    else if (match[1] === 'translate') point = [point[0] + values[0], point[1] + (values[1] || 0)];
    else assert.fail(`unrecognized transform ${match[1]}`);
  }
  return point;
}

function readerContactPoint(point, time, prefix = 'reader') {
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]], ['body', [65, 64]]]) {
    point = transformPoint(point, atContact(`wv-${prefix}-${part}`, time), origin);
  }
  return prefix === 'reader' ? transformPoint(point, atContact('wv-reader-travel', time)) : point;
}

test('reading robot owns its carried sheet and has one stable choreography', () => {
  const f = fixture(); const a = f.api.mount(f.host, receipt({ operationId: 'op-1' }));
  const b = f.api.mount(f.host, receipt({ operationId: 'op-2' }));
  const art = f.parts(a).art.innerHTML;
  assert.equal(art, f.parts(b).art.innerHTML, 'both variant hashes show the same reading pilot');
  const stack = []; let owner;
  for (const tag of art.matchAll(/<(\/?)([a-z]+)\b([^>]*)>/g)) {
    if (tag[1]) { stack.pop(); continue; }
    const classes = /class="([^"]+)"/.exec(tag[3])?.[1]?.split(' ') || [];
    if (classes.includes('wv-read-card')) {
      owner = stack.at(-1);
      assert.doesNotMatch(tag[3], /transform=/, 'the sheet cannot escape its hand');
    }
    if (!tag[3].endsWith('/')) stack.push(classes);
  }
  assert.ok(owner?.includes('wv-reader-hand'), 'paper is a direct child of the forearm/hand');
  assert.match(art, /wv-reader-head/); assert.match(art, /wv-reader-visor/);
  for (const side of ['front', 'back']) for (const joint of ['leg', 'knee', 'foot']) assert.match(art, new RegExp(`wv-reader-${joint}-${side}`));
  assert.doesNotMatch(css, /\[data-variant="[ab]"\][^{]*\.wv-(?:reader|read-card)/);
  a.destroy(); b.destroy();
});

test('the hand meets the source and tray exactly at pickup and release', () => {
  const f = fixture(); const view = f.api.mount(f.host, receipt()); const art = f.parts(view).art.innerHTML;
  const translation = target => /transform="translate\(([-\d.]+) ([-\d.]+)\)"/.exec(art.match(new RegExp(`<g class="wv-reader-${target}"[^>]*>`))[0]).slice(1).map(Number);
  for (const [time, target, prefix] of [[22, 'source', 'reader'], [67, 'filed', 'reader'], [42, 'source', 'inspect']]) {
    const offset = translation(target);
    for (const point of [[90, 34], [108, 62], [91, 55]]) {
      const actual = readerContactPoint(point, time, prefix);
      assert.ok(actual.every((number, axis) => Math.abs(number - point[axis] - offset[axis]) < .00001), `${target} contact at ${time}%: ${actual}`);
    }
  }
  assert.equal(atContact('wv-reader-paper', 22, 'opacity'), '1');
  assert.equal(atContact('wv-reader-paper', 67, 'opacity'), '0');
  assert.equal(atContact('wv-reader-filed', 67, 'opacity'), '1');
  assert.ok(Math.abs(readerContactPoint([108, 62], 67)[1] - 70) < .25, 'paper bottom lands on the tray rim');
  view.destroy();
});

test('short inspection and long story have separate pacing and close without a jump', () => {
  assert.match(css, /--wv-reader-cycle:\s*2\.4s/);
  assert.match(css, /\[data-family="reading"\]\[data-pace="loop"\]\s*\{\s*--wv-reader-cycle:\s*7\.2s/);
  for (const [name, { frames }] of animationFrames()) {
    if (name.startsWith('wv-reader-')) assert.deepEqual(frames[0].properties, frames.at(-1).properties, `${name} closes its loop`);
    if (name.endsWith('-paper')) assert.ok(frames.every(frame => Object.keys(frame.properties).join() === 'opacity'), 'paper never animates a transform of its own');
  }
  assert.equal(atContact('wv-inspect-paper', 100, 'opacity'), '1', 'short gesture ends with paper held for inspection');
  assert.match(css, /\.wv-reader-body\s*\{[^}]*transform:\s*rotate\(-4deg\)/, 'paused/reduced pose stays expressive');
  assert.match(css, /\[data-animated="true"\]\[data-pace="loop"\] \.wv-reader \{ animation: wv-reader-travel/);
  assert.doesNotMatch(css, /\[data-animated="true"\] \.wv-reader \{ animation:/, 'short gesture never walks');
  const sourceBytes = Buffer.byteLength(source), cssBytes = Buffer.byteLength(css);
  assert.ok(sourceBytes < 43000 && cssBytes < 240000 && sourceBytes + cssBytes < 280000,
    'seven families plus shared CSS-only stories stay under a bounded 280 KB total / 43 KB JS / 240 KB CSS source budget, with no external assets');
});

test('two walking steps plant one foot while the other lifts, then return to the same stance', () => {
  function footAt(side, time) {
    const x = side === 'front' ? 68 : 61;
    let point = [x, 88];
    for (const [part, origin] of [['foot', [x, 88]], ['knee', [x, 74.5]], ['leg', [x, 61]]]) {
      point = transformPoint(point, atContact(`wv-reader-${part}-${side}`, time), origin);
    }
    return transformPoint(point, atContact('wv-reader-travel', time));
  }
  for (const [side, times, expectedX] of [
    ['front', [34, 38, 42], 74], ['back', [42, 46, 50], 102],
    ['back', [76, 80, 84], 102], ['front', [84, 88, 92], 74],
  ]) {
    for (const time of times) {
      const [x, y] = footAt(side, time);
      assert.ok(Math.abs(x - expectedX) < .001 && Math.abs(y - 81) < .001, `${side} support stays planted at ${time}%`);
    }
  }
  for (const [side, time] of [['back', 38], ['front', 46], ['front', 80], ['back', 88]]) {
    assert.ok(footAt(side, time)[1] < 78, `${side} swing foot clears the floor at ${time}%`);
  }
});

test('reading light palette has strong outline contrast and forced colors wins its cascade', () => {
  const reading = '.waiting-visual[data-family="reading"]';
  const light = `html[data-theme="light"] ${reading}`;
  function ruleProperties(selector, start = 0) {
    const index = css.indexOf(selector, start); assert.ok(index >= 0, `rule exists: ${selector}`);
    const bodyStart = css.indexOf('{', index), bodyEnd = css.indexOf('}', bodyStart);
    return Object.fromEntries([...css.slice(bodyStart, bodyEnd).matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(match => [match[1], match[2].trim()]));
  }
  const original = ruleProperties(reading), palette = ruleProperties(light);
  assert.equal(original['--wv-accent'], '#d7b579', 'dark gold is unchanged');
  assert.equal(original['--wv-reader-shell'], '#242933', 'dark shell is unchanged');
  assert.equal(original['--wv-reader-paper'], '#141b25', 'dark paper is unchanged');
  function luminance(hex) {
    const rgb = [1, 3, 5].map(index => parseInt(hex.slice(index, index + 2), 16) / 255)
      .map(channel => channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4);
    return rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722;
  }
  for (const surface of ['#ffffff', palette['--wv-reader-shell'], palette['--wv-reader-paper']]) {
    const values = [luminance(palette['--wv-accent']), luminance(surface)].sort((a, b) => b - a);
    assert.ok((values[0] + .05) / (values[1] + .05) >= 4.5, `bronze outline contrasts with ${surface}`);
  }
  const forced = css.indexOf('@media (forced-colors: active)');
  assert.ok(forced > css.indexOf(light), 'system colors are declared after both palettes');
  // The exact light selector in the later media block has equal specificity.
  for (const selector of [reading, light]) {
    const system = ruleProperties(selector, forced);
    assert.equal(system['--wv-accent'], 'CanvasText');
    assert.equal(system['--wv-reader-shell'], 'Canvas');
    assert.equal(system['--wv-reader-paper'], 'Canvas');
  }
});

function artFor(family) {
  const phaseId = { checkpoint: 'metadata-checkpoint-write', calculation: 'analytics-sql', neutral: 'future-phase', reading: 'metadata-scan', composition: 'command:case_report_render', verification: 'metadata-validate' }[family];
  const f = fixture(), view = f.api.mount(f.host, receipt({ phaseId }));
  const art = f.parts(view).art.innerHTML; view.destroy(); return art;
}
function parentsOf(art, child) {
  const stack = [];
  for (const tag of art.matchAll(/<(\/?)([a-z]+)\b([^>]*)>/g)) {
    if (tag[1]) { stack.pop(); continue; }
    const classes = /class="([^"]+)"/.exec(tag[3])?.[1]?.split(' ') || [];
    if (classes.includes(child)) return stack.flat();
    if (!tag[3].endsWith('/')) stack.push(classes);
  }
  assert.fail(`missing artwork ${child}`);
}
function taskPoint(point, time, prefix, offset) {
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]], ['body', [65, 64]]]) {
    point = transformPoint(point, atContact(`wv-${prefix}-${part}`, time), origin);
  }
  return [point[0] + offset, point[1]];
}
function matrixPoint(point, matrix) {
  const [a, b, c, d, e, f] = matrix;
  return [a * point[0] + c * point[1] + e, b * point[0] + d * point[1] + f];
}

test('archive and sorting keep the approved protagonist and direct hand/object ownership', () => {
  for (const family of ['checkpoint', 'calculation', 'neutral']) {
    const art = artFor(family);
    assert.equal([...art.matchAll(/class="wv-task"/g)].length, 1, `${family}: exactly one robot`);
    assert.match(art, /x="50" y="23" width="28" height="24" rx="7"/, 'approved head silhouette');
    assert.match(art, /d="M59 47h10l6 7-3 10H57l-4-10Z"/, 'approved torso silhouette');
    for (const side of ['front', 'back']) for (const joint of ['leg', 'knee', 'foot']) assert.match(art, new RegExp(`wv-task-${joint}-${side}`));
    assert.doesNotMatch(art, /wv-helper|wv-point|wv-save-card|<text\b|\b(?:id|href)=|<image\b/, 'no old mascot, counted labels or external SVG assets');
  }
  const heldParents = parentsOf(artFor('calculation'), 'wv-group-held');
  assert.equal(heldParents.at(-1), 'wv-task-hand');
  assert.ok(heldParents.includes('wv-task-arm') && heldParents.includes('wv-task-body'));
  assert.ok(parentsOf(artFor('checkpoint'), 'wv-archive-folder').includes('wv-archive-drawer'), 'folder is carried by the actual drawer');
  assert.ok(parentsOf(artFor('checkpoint'), 'wv-archive-handle').includes('wv-archive-drawer'), 'handle and drawer cannot drift apart');
  assert.doesNotMatch(artFor('neutral'), /wv-(?:archive|group)-(?:station|held|drawer)/, 'stopped robot does not continue working');
});

test('sorting transfers the same tile at both exact contact poses, including the short gesture', () => {
  const art = artFor('calculation');
  const matrix = target => /transform="matrix\(([^)]+)\)"/.exec(art.match(new RegExp(`<g class="wv-group-${target}"[^>]*>`))[0])[1].split(/\s+/).map(Number);
  for (const [prefix, time, target] of [['group', 22, 'source'], ['group', 66, 'filed'], ['sort', 35, 'source']]) {
    for (const point of [[93, 47], [103, 57], [91, 55]]) {
      const hand = taskPoint(point, time, prefix, 26), prop = matrixPoint(point, matrix(target));
      assert.ok(hand.every((n, i) => Math.abs(n - prop[i]) < .000001), `${prefix} ${target} at ${time}%: ${hand} = ${prop}`);
    }
  }
  assert.equal(atContact('wv-group-held', 22, 'opacity'), '1');
  assert.equal(atContact('wv-group-held', 66, 'opacity'), '0');
  assert.equal(atContact('wv-group-filed', 66, 'opacity'), '1');
  assert.equal(atContact('wv-group-well', 66), 'translateY(0)', 'reaction begins after release, not before contact');
  assert.equal(atContact('wv-sort-held', 100, 'opacity'), '1', 'short gesture ends holding the tile');
  assert.equal(atContact('wv-sort-head', 100), 'rotate(8deg)');
  assert.equal(atContact('wv-sort-visor', 68), 'scaleX(1)', 'short squint waits for its own inspection pose');
  for (const name of ['wv-group-held', 'wv-sort-held']) {
    assert.ok(animationFrames().get(name).frames.every(frame => Object.keys(frame.properties).join() === 'opacity'), 'the carried tile never moves independently of the hand');
  }
});

test('archive fingers stay on the sliding handle throughout each pull and push, not only key poses', () => {
  const frames = animationFrames();
  function interpolated(name, time) {
    const values = frames.get(name).frames.filter(f => f.properties.transform);
    const before = values.filter(f => f.time <= time).at(-1), after = values.find(f => f.time >= time);
    const a = before.properties.transform, b = after.properties.transform;
    const start = [...a.matchAll(/-?\d*\.?\d+(?:e[-+]?\d+)?/g)].map(m => +m[0]), end = [...b.matchAll(/-?\d*\.?\d+(?:e[-+]?\d+)?/g)].map(m => +m[0]);
    const t = before.time === after.time ? 0 : (time - before.time) / (after.time - before.time);
    let i = 0; return a.replace(/-?\d*\.?\d+(?:e[-+]?\d+)?/g, () => String(start[i] + (end[i] - start[i++]) * t));
  }
  for (const [prefix, first, last, open, distance] of [['archive', 18, 64, 32, -10], ['peek', 24, 76, 45, -3]]) {
    for (let t = first; t <= last; t += .25) {
      let hand = [91, 55];
      for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]], ['body', [65, 64]]]) {
        hand = transformPoint(hand, interpolated(`wv-${prefix}-${part}`, t), origin);
      }
      hand[0] += 20;
      const handle = transformPoint([110, 59], interpolated(`wv-${prefix}-drawer`, t));
      assert.ok(Math.hypot(hand[0] - handle[0], hand[1] - handle[1]) < .06, `${prefix} has subpixel grip at ${t}%: ${hand} / ${handle}`);
    }
    assert.equal(parseFloat(/\(([^)]+)/.exec(atContact(`wv-${prefix}-drawer`, open))[1]), distance);
    for (const part of ['body', 'arm', 'hand']) assert.match(css, new RegExp(`animation: wv-${prefix}-${part}[^;]+ linear `), 'coupled joints and drawer use the same timing curve');
  }
  assert.equal(atContact('wv-archive-label', 64), 'rotate(0deg)', 'label only reacts once the drawer arrives');
});

test('task loops close smoothly, short stories are distinct, and all static states are complete', () => {
  for (const [name, { frames }] of animationFrames()) {
    if (/^wv-(archive|group)-/.test(name)) assert.deepEqual(frames[0].properties, frames.at(-1).properties, `${name} closes continuously`);
  }
  assert.match(css, /--wv-task-cycle: 2\.8s/); assert.match(css, /--wv-task-cycle: 6\.8s/);
  assert.match(css, /--wv-task-cycle: 2\.6s/); assert.match(css, /--wv-task-cycle: 6\.4s/);
  assert.match(css, /\[data-family="checkpoint"\]\[data-animated="true"\] \.wv-task-arm \{ animation: wv-peek-arm/);
  assert.match(css, /\[data-family="calculation"\]\[data-animated="true"\] \.wv-task-arm \{ animation: wv-sort-arm/);
  assert.doesNotMatch(css, /\[data-family="neutral"\][^{]*\{[^}]*animation:/);
  for (const phaseId of ['metadata-checkpoint-write', 'analytics-sql']) {
    const f = fixture({ reduced: true }), view = f.api.mount(f.host, receipt({ phaseId, elapsedMs: 8000 }));
    f.observers[0].deliver(true);
    assert.equal(view.element.dataset.motion, 'static'); assert.equal(f.parts(view).control.hidden, true);
    assert.match(f.parts(view).art.innerHTML, /wv-task-head/);
    for (const state of ['cancelling', 'cancelled', 'error', 'paused', 'completed']) {
      view.update(receipt({ phaseId, state }));
      assert.equal(view.element.dataset.family, 'neutral'); assert.equal(view.element.dataset.motion, 'static');
      assert.doesNotMatch(f.parts(view).art.innerHTML, /wv-archive-drawer|wv-group-held/);
    }
    view.destroy();
  }
  const f = fixture(), partial = f.api.mount(f.host, receipt({ phaseId: 'metadata-checkpoint-committed', completed: 100, total: 100 }));
  f.observers[0].deliver(true);
  assert.equal(partial.element.dataset.motion, 'static'); assert.equal(partial.element.dataset.checkpoint, 'preserved');
  assert.doesNotMatch(f.parts(partial).art.innerHTML, /lock|checkmark|success|badge/i); partial.destroy();
});

test('new stages retain high-contrast light/dark palettes, compact dimensions and forced colors', () => {
  const selector = '.waiting-visual:is([data-family="checkpoint"], [data-family="calculation"], [data-family="neutral"])';
  function properties(marker, start = 0) {
    const index = css.indexOf(marker, start); assert.ok(index >= 0);
    const body = css.slice(css.indexOf('{', index) + 1, css.indexOf('}', index));
    return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(m => [m[1], m[2].trim()]));
  }
  function luminance(hex) {
    const rgb = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4);
    return rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722;
  }
  for (const [marker, surface] of [[selector, '#0d1117'], [`html[data-theme="light"] ${selector}`, '#ffffff']]) {
    const palette = properties(marker);
    for (const background of [surface, palette['--wv-task-shell'], palette['--wv-task-paper']]) {
      const l = [luminance(palette['--wv-accent']), luminance(background)].sort((a, b) => b - a);
      assert.ok((l[0] + .05) / (l[1] + .05) >= 4.5, `robot and machine outline contrasts with ${background}`);
    }
    const forced = properties(marker, css.indexOf('@media (forced-colors: active)'));
    assert.equal(forced['--wv-accent'], 'CanvasText'); assert.equal(forced['--wv-task-shell'], 'Canvas'); assert.equal(forced['--wv-task-paper'], 'Canvas');
  }
  const dimensions = css.slice(css.indexOf(`${selector} .wv-art`)).split('}')[0];
  assert.match(dimensions, /240px/); assert.match(dimensions, /block-size: 120px/);
  assert.doesNotMatch(css, /wv-(?:group|archive)[^{]*\{[^}]*animation[^;]*(?:filter|blur|shadow)/);
});

test('shared reactions preserve approved reading artwork, base choreography and transport-free controller', () => {
  const hash = value => createHash('sha256').update(value).digest('hex');
  assert.equal(hash(source.slice(source.indexOf('    reading:', source.indexOf('  const scenes')), source.indexOf('    checkpoint:', source.indexOf('  const scenes')))), '24e668b03de07c74936eb5bd679000caebd4e4c8ee704a683caefd9a66b5185e');
  assert.match(source, /Object\.freeze\(\{ derive, mount, repertoire, adapters, createDirector, createElapsedClock \}\)/);
  assert.equal(hash(css.slice(css.indexOf('/* Reading pilot:'), css.indexOf('/* Same protagonist,')).replaceAll('[data-animated="true"]', '[data-motion="running"]')), '9a695f3a6e19798186cd55a3932434d81d9fca4f8cbe97610c5f0d4b95fcdc43');
  assert.doesNotMatch(source, /\b(?:invoke|listen|emit|fetch|setTimeout|setInterval|requestAnimationFrame)\s*\(/, 'no IPC, transport, timer or per-frame animation controller was introduced');
});


test('report proof belongs to its hand or press bed; the rigid grip cannot drift from its platen', () => {
  const art=artFor('composition');
  assert.ok(parentsOf(art,'wv-compose-held').includes('wv-task-hand'));
  assert.ok(parentsOf(art,'wv-compose-loaded').includes('wv-compose-bed'));
  assert.ok(parentsOf(art,'wv-compose-grip').includes('wv-compose-press'));
  assert.doesNotMatch(art,/success|badge|checkmark|lock|<image|<filter|<foreignObject/i);
  assert.equal((art.match(/class="wv-task-head"/g)||[]).length,1,'the same robot operates this station alone');
  const offset=/class="wv-compose-loaded" transform="translate\(([^)]+)\)"/.exec(art)[1].split(/\s+/).map(Number);
  for(const time of [28,86])for(const point of [[93,52],[109,63],[91,55]]) {
    const hand=taskPoint(point,time,'compose',24);
    assert.ok(hand.every((value,i)=>Math.abs(value-point[i]-offset[i])<.000001),`two-corner proof transfer at ${time}%`);
  }
  assert.equal(atContact('wv-compose-held',28,'opacity'),'0');assert.equal(atContact('wv-compose-loaded',28,'opacity'),'1');
  assert.equal(atContact('wv-compose-held',86,'opacity'),'1');assert.equal(atContact('wv-compose-loaded',86,'opacity'),'0');
  for(const part of ['held','loaded'])assert.ok(animationFrames().get(`wv-compose-${part}`).frames.every(frame=>Object.keys(frame.properties).join()==='opacity'),'proofs have no independent travel');
});

test('report press contact is continuous through both loaded strokes and the output reacts afterward', () => {
  const frames=animationFrames();
  function transform(name,time) {
    const values=frames.get(name).frames,before=values.filter(frame=>frame.time<=time).at(-1),after=values.find(frame=>frame.time>=time);
    const a=before.properties.transform,b=after.properties.transform,start=[...a.matchAll(/-?\d*\.?\d+(?:e[-+]?\d+)?/g)].map(m=>+m[0]),end=[...b.matchAll(/-?\d*\.?\d+(?:e[-+]?\d+)?/g)].map(m=>+m[0]);
    const weight=before.time===after.time?0:(time-before.time)/(after.time-before.time);let i=0;
    return a.replace(/-?\d*\.?\d+(?:e[-+]?\d+)?/g,()=>String(start[i]+(end[i]-start[i++])*weight));
  }
  for(let time=38;time<=76;time+=.25) {
    let hand=[91,55];for(const [part,origin] of [['hand',[82,58]],['arm',[72,52]],['body',[65,64]]])hand=transformPoint(hand,transform(`wv-compose-${part}`,time),origin);
    hand[0]+=24;const grip=transformPoint([114,46],transform('wv-compose-press',time));
    assert.ok(Math.hypot(hand[0]-grip[0],hand[1]-grip[1])<.025,`continuous press contact at ${time}%: ${hand} / ${grip}`);
  }
  assert.equal(atContact('wv-compose-press',38),'translateY(0.00000000px)');
  assert.equal(atContact('wv-compose-press',56),'translateY(7.00000000px)');
  assert.equal(atContact('wv-compose-bed',54),'translateY(0)','bed does not react before the loaded downward press');
  assert.equal(atContact('wv-compose-bed',56),'translateY(.7px)');
  assert.equal(atContact('wv-compose-bed',84),'translateY(0)','output is stable before the robot retrieves it');
});

test('report has a distinct short alignment, a closed long story and complete accessible resting art', () => {
  assert.match(css,/--wv-compose-cycle: 2\.8s/);assert.match(css,/--wv-compose-cycle: 7\.6s/);
  for(const [name,{frames}] of animationFrames())if(name.startsWith('wv-compose-'))assert.deepEqual(frames[0].properties,frames.at(-1).properties,`${name} closes continuously`);
  assert.equal(atContact('wv-align-body',42),'rotate(12deg)');assert.equal(atContact('wv-align-body',100),'rotate(-4deg)');
  assert.doesNotMatch(css,/\[data-family="composition"\]\[data-animated="true"\] \.wv-compose-(press|bed|held|loaded) \{/,'short alignment retains the page in its hand and does not run the press loop faster');
  for(const reduced of [false,true]) {
    const f=fixture({reduced}),view=f.api.mount(f.host,receipt({phaseId:'command:case_report_render',elapsedMs:8000}));f.observers[0].deliver(true);
    assert.equal(view.element.dataset.motion,reduced?'static':'running');assert.match(f.parts(view).art.innerHTML,/wv-compose-station/);
    view.setMotionEnabled(false);assert.equal(view.element.dataset.motion,'static');
    view.setMotionEnabled(true);f.observers[0].deliver(false);assert.equal(view.element.dataset.motion,'static');
    for(const state of ['cancelling','cancelled','error','completed']) {view.update(receipt({phaseId:'command:case_report_render',state}));assert.equal(view.element.dataset.family,'neutral');assert.doesNotMatch(f.parts(view).art.innerHTML,/wv-compose-press/);}
    view.destroy();assert.equal(f.observers[0].disconnected,true);assert.equal(f.document.count('visibilitychange'),0);
  }
  const palette=css.slice(css.indexOf('/* Report pilot:'));
  assert.match(palette,/html\[data-theme="light"\] \.waiting-visual\[data-family="composition"\] \{\s*--wv-accent: #79501c/);
  assert.match(palette,/@media \(forced-colors: active\)[\s\S]+--wv-accent: CanvasText/);
});

test('approved archive/sorting artwork and work-only keyframes remain intact', () => {
  const hash=value=>createHash('sha256').update(value).digest('hex');
  assert.ok(css.includes('@keyframes wv-archive-drawer') && css.includes('@keyframes wv-group-held'));
  assert.match(css, /data-motion="static"[\s\S]+animation-play-state: paused !important/);
  assert.equal(hash(source.slice(source.indexOf('    checkpoint:', source.indexOf('  const scenes')),source.indexOf('    composition:', source.indexOf('  const scenes')))),'827876243a4ceb9bf070094fd427ea0e71d55cd3fa1174f6018a11a5a53457c2');
});

function reactionBoundary(f, visual, type = 'animationiteration', overrides = {}) {
  const art = f.parts(visual).art;
  const name = type === 'animationend' ? 'wv-episode-boundary' : 'wv-work-boundary';
  art.emit(type, { target: art.children.find(node => node.className === name), animationName: name, pseudoElement: '', ...overrides });
}

function coffeeFixture() {
  const f = fixture();
  const visual = f.api.mount(f.host, receipt({ elapsedMs: 60000 }), { reactionSeed: 17 });
  f.observers[0].deliver(true); reactionBoundary(f, visual);
  assert.equal(visual.element.dataset.episode, 'coffee');
  return { f, visual };
}

test('repertoire is seeded, gated by real elapsed time, visibly varied and anti-repetitive', () => {
  const { api } = fixture();
  const run = seed => {
    const director = api.createDirector('seeded-operation', seed), events = [];
    const model = api.derive(receipt({ elapsedMs: 60000 }));
    for (let cycle = 0; cycle < 1000; cycle++) {
      const event = director.boundary(model);
      if (event) { events.push({ episode: event.episode, variant: event.variant, cycle }); director.finish(); }
    }
    return events;
  };
  const events = run('same'); assert.deepEqual(events, run('same')); assert.notDeepEqual(events, run('different'));
  assert.deepEqual([...new Set(events.map(event => event.episode))].sort(), ['coffee', 'manual', 'review', 'stretch', 'visor', 'wave']);
  assert.deepEqual([...new Set(events.map(event => event.variant))].sort(), ['a', 'b']);
  for (let index = 1; index < events.length; index++) {
    assert.notEqual(events[index].episode, events[index - 1].episode);
    assert.ok(!(api.repertoire[events[index].episode].long && api.repertoire[events[index - 1].episode].long), 'long stories always have a short reaction between them');
    assert.ok(events[index].cycle - events[index - 1].cycle > api.repertoire[events[index - 1].episode].cooldown);
  }
  const director = api.createDirector('gating');
  for (const elapsedMs of [undefined, 0, 500, 3999, 4000, 14999]) for (let i = 0; i < 50; i++) {
    assert.equal(director.boundary(api.derive(receipt({ elapsedMs }))), null);
  }
  for (const phaseId of ['metadata-checkpoint-committed', 'future-phase']) {
    assert.equal(director.boundary(api.derive(receipt({ phaseId, elapsedMs: 60000 }))), null, 'unproven handoffs never enter a shared episode');
  }
});

test('boundary events alone choose reactions; 1000 receipts cannot reroll or replace the SVG', () => {
  const { f, visual } = coffeeFixture(); const { art, status } = f.parts(visual);
  const state = plain(visual.inspect()), writes = art.htmlWrites, announcements = status.writes;
  for (let completed = 0; completed < 1000; completed++) visual.update(receipt({ completed, elapsedMs: 60000 + completed }));
  assert.deepEqual(plain(visual.inspect()), state); assert.equal(art.htmlWrites, writes); assert.equal(status.writes, announcements);
  assert.equal(f.parts(visual).metric.textContent, '999');
  assert.equal(status.textContent, 'Indexando metadados'); assert.doesNotMatch(status.textContent, /café|descanso|revisando|restantes/i);
  reactionBoundary(f, visual, 'animationend');
  assert.equal(visual.element.dataset.episode, 'work'); assert.equal(visual.inspect().cooldown, 4);
  visual.destroy();
});

test('all pauses preserve episode/random state and latched CSS names instead of restarting tracks', () => {
  const { f, visual } = coffeeFixture(), before = plain(visual.inspect());
  const pauses = [
    [() => visual.setMotionEnabled(false), () => visual.setMotionEnabled(true)],
    [() => f.observers[0].deliver(false), () => f.observers[0].deliver(true)],
    [() => { f.document.hidden = true; f.document.emit('visibilitychange'); }, () => { f.document.hidden = false; f.document.emit('visibilitychange'); }],
    [() => { f.media.matches = true; f.media.emit('change'); }, () => { f.media.matches = false; f.media.emit('change'); }],
    [() => visual.setVisible(false), () => visual.setVisible(true)]
  ];
  for (const [pause, resume] of pauses) {
    pause(); assert.equal(visual.element.dataset.motion, 'static'); assert.equal(visual.element.dataset.animated, 'true');
    reactionBoundary(f, visual, 'animationend'); reactionBoundary(f, visual);
    assert.deepEqual(plain(visual.inspect()), before);
    resume(); assert.equal(visual.element.dataset.motion, 'running'); assert.deepEqual(plain(visual.inspect()), before);
  }
  assert.equal(visual.element.hidden, false, 'panel hiding preserves CSS playheads');
  assert.match(css, /data-visible="false"\][^{}]*\*[^{]*\{ visibility: hidden !important;/, 'episode actor cannot override an invisible ancestor');
  assert.doesNotMatch(css, /animation:\s*none\s*!important/, 'system motion changes freeze rather than recreate clocks');
  visual.destroy();
});

test('phase adaptation completes the current reaction before adopting the latest family', () => {
  const { f, visual } = coffeeFixture(); const { art } = f.parts(visual), state = plain(visual.inspect());
  visual.update(receipt({ phaseId: 'metadata-columns', label: 'Lendo colunas', elapsedMs: 1 }));
  assert.equal(art.htmlWrites, 1); assert.equal(visual.element.dataset.episode, 'coffee'); assert.equal(f.parts(visual).status.textContent, 'Lendo colunas');
  assert.deepEqual(plain(visual.inspect()), state);
  visual.update(receipt({ phaseId: 'metadata-checkpoint-write', elapsedMs: 60000 }));
  assert.equal(visual.element.dataset.episode, 'coffee'); assert.equal(visual.element.dataset.family, 'reading');
  reactionBoundary(f, visual, 'animationend');
  assert.equal(visual.element.dataset.family, 'checkpoint');
  assert.equal(visual.element.dataset.episode, 'work'); assert.equal(visual.inspect().seed, state.seed);
  assert.equal(visual.inspect().cooldown, 4); assert.deepEqual(plain(visual.inspect().history), ['coffee']);
  visual.update(receipt({ operationId: 'next-operation', elapsedMs: 60000 }));
  assert.notEqual(visual.inspect().seed, state.seed); assert.deepEqual(plain(visual.inspect().history), []);
  visual.destroy();
});

test('wrong, stale, detached and terminal boundaries cannot advance or revive reactions', () => {
  const { f, visual } = coffeeFixture(); const art = f.parts(visual).art, before = plain(visual.inspect());
  const stale = art.children.find(node => node.className === 'wv-episode-boundary');
  for (const overrides of [{ target: visual.element }, { animationName: 'unrelated' }, { pseudoElement: '::before' }]) reactionBoundary(f, visual, 'animationend', overrides);
  assert.deepEqual(plain(visual.inspect()), before);
  f.host.remove(); reactionBoundary(f, visual, 'animationend'); assert.deepEqual(plain(visual.inspect()), before); f.document.body.append(f.host);
  visual.update(receipt({ operationId: 'new', elapsedMs: 60000 }));
  reactionBoundary(f, visual, 'animationend', { target: stale }); assert.equal(visual.element.dataset.episode, 'work');
  for (const state of ['paused', 'cancelling', 'cancelled', 'error', 'completed', 'unknown']) {
    visual.update(receipt({ state, elapsedMs: 60000 }));
    assert.equal(visual.element.dataset.family, 'neutral'); assert.equal(visual.element.dataset.animated, 'false');
    assert.doesNotMatch(art.innerHTML, /wv-kitchen|wv-reaction-actor/);
  }
  visual.destroy(); assert.equal(art.count('animationiteration'), 0); assert.equal(art.count('animationend'), 0);
});

function interpolated(name, time, variableValues = {}) {
  const frames = animationFrames().get(name).frames;
  const a = frames.filter(frame => frame.time <= time + .000001).at(-1), b = frames.find(frame => frame.time >= time - .000001);
  const replaceVars = value => value.replace(/var\((--[\w-]+)\)/g, (_, key) => variableValues[key] || '0');
  const start = replaceVars(a.properties.transform), finish = replaceVars(b.properties.transform);
  if (a.time === b.time || start === finish) return start;
  const values = [...finish.matchAll(/-?\d*\.?\d+(?:e[-+]?\d+)?/g)].map(m => +m[0]); let index = 0;
  const weight = (time - a.time) / (b.time - a.time);
  return start.replace(/-?\d*\.?\d+(?:e[-+]?\d+)?/g, number => String(+number + (values[index++] - +number) * weight));
}
function coffeePoint(point, time, family, wrist = false) {
  if (wrist) point = transformPoint(point, interpolated('wv-coffee-wrist', time), [91, 55]);
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]]]) point = transformPoint(point, interpolated(`wv-coffee-${part}`, time), origin);
  return transformPoint(point, interpolated(`wv-coffee-travel-${family}`, time));
}

test('coffee is the same anchored prop through pickup, two sips and exact shelf return in both adapters', () => {
  for (const family of ['reading', 'checkpoint']) {
    const art = artFor(family);
    assert.ok(parentsOf(art, 'wv-cup-held').includes('wv-react-hand'));
    assert.ok(parentsOf(art, 'wv-cup-wrist').includes('wv-cup-held'));
    assert.match(art, /class="wv-cup-shelf" transform="translate\(82 0\)"/);
    for (const time of [28.125, 82.8125]) for (const point of [[92, 50], [99, 58], [91, 55]]) {
      const actual = coffeePoint(point, time, family, true);
      assert.ok(actual.every((value, axis) => Math.abs(value - point[axis] - (axis ? 0 : 82)) < .00001), `${family} cup handoff has no jump at ${time}%`);
    }
    assert.equal(atContact('wv-coffee-held', 28.125, 'opacity'), '1');
    assert.equal(atContact('wv-coffee-shelf', 28.125, 'opacity'), '0');
    assert.equal(atContact('wv-coffee-held', 82.8125, 'opacity'), '0');
    assert.equal(atContact('wv-coffee-shelf', 82.8125, 'opacity'), '1');
    for (const seconds of [16, 17.5, 21, 22]) {
      const finger = coffeePoint([91, 55], seconds / 32 * 100, family);
      const home = family === 'reading' ? 0 : 20;
      assert.ok(Math.hypot(finger[0] - 74 - home, finger[1] - 43) < .001, 'cup reaches the visor at both calm sips');
    }
  }
});

test('the hatch follows the fingers through opening and closing, never moving on its own', () => {
  for (const family of ['reading', 'checkpoint']) for (const [start, end] of [[6.5, 8.5], [27.5, 29]]) {
    for (let seconds = start; seconds <= end; seconds += .125) {
      const time = seconds / 32 * 100;
      const finger = coffeePoint([91, 55], time, family);
      const handle = transformPoint([173, 55], interpolated('wv-coffee-hatch', time));
      assert.ok(Math.hypot(finger[0] - handle[0], finger[1] - handle[1]) < .02, `${family} hatch contact at ${seconds}s`);
      assert.equal(atContact('wv-coffee-held', time, 'opacity'), '0', 'the same hand is empty before touching the hatch');
    }
  }
});

test('reaction paths close at their adapters and all travel is CSS transform/opacity only', () => {
  const frames = animationFrames();
  for (const [name, value] of frames) if (/^wv-(coffee|manual|review|stretch)-/.test(name)) assert.deepEqual(value.frames[0].properties, value.frames.at(-1).properties, `${name} returns to its safe pose`);
  for (const [family, home] of [['reading', 0], ['checkpoint', 20]]) {
    for (const time of [0, 100]) {
      assert.equal(atContact(`wv-coffee-travel-${family}`, time), `translateX(${home.toFixed(6)}px)`);
      assert.equal(atContact('wv-coffee-arm', time), 'rotate(40deg)'); assert.equal(atContact('wv-coffee-hand', time), 'rotate(-85deg)');
    }
  }
  assert.match(css, /--wv-reaction-cycle: 32s/); assert.match(css, /--wv-reaction-cycle: 12s/); assert.match(css, /--wv-reaction-cycle: 10s/);
  assert.doesNotMatch(source, /\b(?:setTimeout|setInterval|requestAnimationFrame|fetch|XMLHttpRequest)\s*\(/);
});

test('a monotonic measured decoration clock extrapolates only private eligibility between sparse receipts', () => {
  let now = 100;
  const { api } = fixture(); const clock = api.createElapsedClock(() => now);
  const model = overrides => api.derive(receipt(overrides));
  clock.observe(model({ elapsedMs: 4000 }));
  now = 42100; assert.equal(clock.read(), 46000);
  now = 41000; assert.equal(clock.read(), 46000, 'a backwards clock cannot rewind decoration age');
  clock.observe(model({ completed: 2 }));
  now = 43100; assert.equal(clock.read(), 47000, 'missing displayed elapsed does not erase an existing private measurement');
  assert.deepEqual(plain(model({ completed: 2 }).details), [], 'no private age is copied into user-facing receipts');
  clock.observe(model({ elapsedMs: 8000 }));
  assert.equal(clock.read(), 8000, 'an authoritative new measurement rebases the clock');
  now += 1000; assert.equal(clock.read(), 9000);
  clock.observe(model({ phaseId: 'metadata-columns' }));
  now += 10000; assert.equal(clock.read(), null, 'new phase without a measurement cannot inherit old eligibility');
  clock.observe(model({ phaseId: 'metadata-columns', elapsedMs: 5000 }));
  now += 1000; assert.equal(clock.read(), 6000);
  clock.observe(model({ operationId: 'new-owner', elapsedMs: 100 }));
  now += 1000; assert.equal(clock.read(), 1100);
});

test('quiet operations qualify at a real boundary without receipts, timers, synthetic metrics or live announcements', () => {
  let now = 0;
  const f = fixture({ now: () => now });
  const visual = f.api.mount(f.host, receipt({ elapsedMs: 4000 }), { reactionSeed: 47, showDetails: true });
  const { status, metric, details, art } = f.parts(visual), announced = status.writes;
  f.observers[0].deliver(true);
  now = 42000;
  assert.equal(visual.element.dataset.episode, 'work', 'wall time alone schedules no work');
  reactionBoundary(f, visual);
  assert.equal(visual.element.dataset.episode, 'coffee', 'the boundary observes measured 4s plus real 42s');
  assert.equal(status.writes, announced); assert.equal(status.textContent, 'Indexando metadados');
  assert.equal(metric.hidden, true); assert.equal(details.textContent, '4 s decorridos'); assert.equal(art.htmlWrites, 1);
  const state = plain(visual.inspect());
  visual.setMotionEnabled(false); now += 100000; reactionBoundary(f, visual, 'animationend');
  assert.deepEqual(plain(visual.inspect()), state, 'elapsed time during pause does not drain or queue episodes');
  visual.setMotionEnabled(true); assert.deepEqual(plain(visual.inspect()), state);
  visual.destroy();
});

function adapterBoundary(f, visual, stage = visual.element.dataset.adapter, overrides = {}) {
  const art = f.parts(visual).art;
  art.emit('animationend', { target: art.children.find(node => node.className === 'wv-adapter-boundary'),
    animationName: `wv-${stage}-boundary`, pseudoElement: '', ...overrides });
}
function adaptedCoffeeFixture(phaseId) {
  const f = fixture();
  const visual = f.api.mount(f.host, receipt({ phaseId, elapsedMs: 60000 }), { reactionSeed: 17 });
  f.observers[0].deliver(true); reactionBoundary(f, visual);
  assert.equal(visual.element.dataset.episode, 'coffee'); assert.equal(visual.element.dataset.adapter, 'prepare');
  return { f, visual };
}

test('calculation has an empty hand only at the completed long loop and reuses the shared anchor', () => {
  const { api } = fixture(); assert.equal(api.adapters.calculation.reactions, true);
  assert.equal(api.adapters.calculation.homeX, 26); assert.equal(api.adapters.calculation.anchorX, 20);
  assert.equal(atContact('wv-group-held', 100, 'opacity'), '0');
  assert.equal(atContact('wv-sort-held', 100, 'opacity'), '1');
  const director = api.createDirector('load-17', 17);
  assert.equal(director.boundary(api.derive(receipt({ phaseId: 'analytics-sql', elapsedMs: 3999 }))), null);
  assert.equal(director.boundary(api.derive(receipt({ phaseId: 'analytics-sql', elapsedMs: 60000 }))).episode, 'coffee');
  const { f, visual } = adaptedCoffeeFixture('analytics-sql'), art = f.parts(visual).art;
  adapterBoundary(f, visual); assert.equal(visual.element.dataset.adapter, 'react');
  reactionBoundary(f, visual, 'animationend'); assert.equal(visual.element.dataset.adapter, 'resume');
  assert.equal(visual.inspect().cooldown, 0, 'cooldown begins only after returning to the task anchor');
  adapterBoundary(f, visual); assert.equal(visual.element.dataset.adapter, 'work'); assert.equal(visual.element.dataset.episode, 'work');
  assert.equal(visual.inspect().cooldown, 4); assert.equal(art.htmlWrites, 1);
  assert.match(css, /:is\(\[data-family="checkpoint"\], \[data-family="calculation"\], \[data-family="composition"\], \[data-family="verification"\], \[data-family="restoration"\]\):is\(\[data-episode="coffee"\], \[data-episode="manual"\]\)\[data-adapter="react"\] \.wv-reaction-actor \{ animation: wv-coffee-travel-checkpoint/);
  assert.doesNotMatch(css, /@keyframes wv-coffee-(?:calculation|composition)/, 'whole excursions are shared, never duplicated per family');
  visual.destroy();
});

test('composition stages park the same proof, complete the shared reaction and recover before work resumes', () => {
  const { f, visual } = adaptedCoffeeFixture('command:case_report_render');
  const { art, status } = f.parts(visual), initial = plain(visual.inspect());
  assert.equal(initial.adapterStage, 'prepare'); assert.equal(initial.episode, 'coffee');
  assert.ok(parentsOf(art.innerHTML, 'wv-proof-held').includes('wv-react-hand'));
  assert.match(art.innerHTML, /class="wv-proof-parked" transform="translate\(26\.341973495 1\.717610627\)"/);
  reactionBoundary(f, visual, 'animationend'); assert.equal(visual.element.dataset.adapter, 'prepare', 'an old coffee end cannot skip parking');
  adapterBoundary(f, visual, 'resume'); assert.equal(visual.element.dataset.adapter, 'prepare');
  adapterBoundary(f, visual); assert.equal(visual.element.dataset.adapter, 'react');
  reactionBoundary(f, visual); assert.equal(visual.element.dataset.adapter, 'react', 'work boundaries cannot stack reactions');
  reactionBoundary(f, visual, 'animationend'); assert.equal(visual.element.dataset.adapter, 'resume');
  adapterBoundary(f, visual, 'prepare'); assert.equal(visual.element.dataset.adapter, 'resume', 'a late preparation end cannot skip retrieval');
  adapterBoundary(f, visual); assert.equal(visual.element.dataset.adapter, 'work');
  assert.equal(visual.element.dataset.episode, 'work'); assert.equal(visual.inspect().cooldown, 4);
  assert.equal(art.htmlWrites, 1); assert.equal(status.textContent, 'Indexando metadados');
  visual.destroy();
});

test('pauses freeze every adapter stage while terminal, owner and family changes remove it immediately', () => {
  for (const phaseId of ['analytics-sql', 'command:case_report_render']) for (const stage of ['prepare', 'react', 'resume']) {
    const { f, visual } = adaptedCoffeeFixture(phaseId);
    if (stage !== 'prepare') adapterBoundary(f, visual);
    if (stage === 'resume') reactionBoundary(f, visual, 'animationend');
    const before = plain(visual.inspect());
    visual.setMotionEnabled(false); adapterBoundary(f, visual); reactionBoundary(f, visual, 'animationend');
    assert.deepEqual(plain(visual.inspect()), before); assert.equal(visual.element.dataset.animated, 'true');
    visual.setMotionEnabled(true); assert.deepEqual(plain(visual.inspect()), before);
    visual.update(receipt({ phaseId, state: 'cancelling' }));
    assert.equal(visual.element.dataset.adapter, 'work'); assert.equal(visual.element.dataset.episode, 'work');
    assert.equal(visual.element.dataset.family, 'neutral'); assert.equal(visual.element.dataset.animated, 'false');
    assert.doesNotMatch(f.parts(visual).art.innerHTML, /wv-proof-parked|wv-reaction-actor/);
    adapterBoundary(f, visual, stage); assert.equal(visual.element.dataset.adapter, 'work');
    visual.destroy();
  }
  const { f, visual } = adaptedCoffeeFixture('command:case_report_render');
  const stale = f.parts(visual).art.children.find(node => node.className === 'wv-adapter-boundary');
  visual.update(receipt({ phaseId: 'future-unknown', elapsedMs: 60000 }));
  adapterBoundary(f, visual, 'prepare', { target: stale });
  assert.equal(visual.element.dataset.adapter, 'work'); assert.equal(visual.element.dataset.motion, 'static');
  assert.equal(visual.inspect().cooldown, 4); assert.deepEqual(plain(visual.inspect().history), ['coffee']);
  visual.update(receipt({ operationId: 'new', phaseId: 'analytics-sql', elapsedMs: 60000 }));
  adapterBoundary(f, visual, 'prepare', { target: stale }); assert.equal(visual.element.dataset.adapter, 'work');
  assert.deepEqual(plain(visual.inspect().history), []); visual.destroy();
});

function bridgePoint(point, stage, time) {
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]], ['body', [65, 64]]]) {
    point = transformPoint(point, interpolated(`wv-bridge-composition-${stage}-${part}`, time), origin);
  }
  return transformPoint(point, interpolated(`wv-bridge-composition-${stage}-travel`, time));
}

test('composition deposits and retrieves both proof corners at the already-validated bed contact', () => {
  for (const [stage, time] of [['prepare', 40], ['resume', 60]]) {
    for (const point of [[93, 52], [109, 63], [91, 55]]) {
      const actual = bridgePoint(point, stage, time), offset = [26.341973495, 1.717610627];
      assert.ok(actual.every((value, axis) => Math.abs(value - point[axis] - offset[axis]) < .000001), `${stage} keeps proof attached through contact`);
    }
    assert.equal(atContact(`wv-bridge-composition-${stage}-proof-held`, time, 'opacity'), stage === 'prepare' ? '0' : '1');
    assert.equal(atContact(`wv-bridge-composition-${stage}-proof-parked`, time, 'opacity'), stage === 'prepare' ? '1' : '0');
  }
  assert.match(css, /data-family="composition"\]\[data-adapter="react"\] \.wv-proof-parked \{ opacity: 1;/);
  for (const [stage, time, expected] of [['prepare', 0, [-4, -26, 0, 8, 24]], ['prepare', 100, [0, 40, -85, 4, 20]], ['resume', 0, [0, 40, -85, 4, 20]], ['resume', 100, [-4, -26, 0, 8, 24]]]) {
    for (const [index, part] of ['body', 'arm', 'hand', 'head', 'travel'].entries()) {
      const transform = atContact(`wv-bridge-composition-${stage}-${part}`, time);
      const value = Number(/\((-?[\d.]+)/.exec(transform)[1]); assert.equal(value, expected[index], `${stage} ${part} endpoint matches exact ownership pose`);
    }
  }
});

test('adapter shuffles keep one planted foot and reverse to the same stance', () => {
  const frames = animationFrames();
  for (const family of ['calculation', 'composition']) for (const stage of ['prepare', 'resume']) {
    const allTimes = frames.get(`wv-bridge-${family}-${stage}-leg-front`).frames.map(frame => frame.time);
    const foot = (side, time) => {
      const x = side === 'front' ? 68 : 61; let point = [x, 88];
      for (const [part, origin] of [['foot', [x, 88]], ['knee', [x, 74.5]], ['leg', [x, 61]]]) point = transformPoint(point, interpolated(`wv-bridge-${family}-${stage}-${part}-${side}`, time), origin);
      return transformPoint(point, interpolated(`wv-bridge-${family}-${stage}-travel`, time));
    };
    for (const time of allTimes) {
      const a = foot('front', time), b = foot('back', time);
      assert.ok(Math.min(Math.abs(a[1] - 81), Math.abs(b[1] - 81)) < .00001, `${family} ${stage} always has a planted support at ${time}%`);
      assert.ok(a[1] <= 81.00001 && b[1] <= 81.00001, 'feet never sink through the ground');
    }
    const startX = family === 'calculation' ? 26 : 24;
    assert.equal(atContact(`wv-bridge-${family}-${stage}-travel`, 0), `translateX(${(stage === 'prepare' ? startX : 20).toFixed(8)}px)`);
    assert.equal(atContact(`wv-bridge-${family}-${stage}-travel`, 100), `translateX(${(stage === 'prepare' ? 20 : startX).toFixed(8)}px)`);
  }
});

test('coffee nook remains exclusive to coffee, before approach, and leaves after the complete return', () => {
  assert.match(css, /\.waiting-visual \.wv-kitchen \{ opacity: 0; \}/);
  assert.match(css, /\[data-episode="coffee"\]\[data-adapter="react"\] \.wv-kitchen \{\s*animation: wv-coffee-kitchen/);
  assert.doesNotMatch(css, /\[data-episode="(?:work|review|stretch)"\][^{]*\.wv-kitchen\s*\{/);
  assert.equal(atContact('wv-coffee-kitchen', 0, 'opacity'), '0');
  assert.equal(atContact('wv-coffee-kitchen', 5, 'opacity'), '1');
  assert.equal(atContact('wv-coffee-travel-reading', 5), 'translateX(0.000000px)', 'nook is established before the first step');
  assert.equal(atContact('wv-coffee-kitchen', 97, 'opacity'), '1');
  assert.equal(atContact('wv-coffee-travel-checkpoint', 97), 'translateX(20.000000px)', 'the robot is home before the nook fades out');
  assert.equal(atContact('wv-coffee-kitchen', 100, 'opacity'), '0');
});

test('subtle vector steam stays inside the articulated cup and shares paused reaction clocks', () => {
  const art = artFor('reading'), steamParents = parentsOf(art, 'wv-cup-steam');
  // The first copy is the stored cup; the carried copy is explicitly wrist-owned.
  assert.ok(steamParents.includes('wv-cup-shelf'));
  assert.match(art, /class="wv-cup-held"><g class="wv-cup-wrist"><g class="wv-cup-steam"><path d="M94 47c-1-1 1-2 0-3"\/><path d="M97 46c-1-1 1-2 0-3"\/>/);
  assert.match(css, /\.wv-cup-held \.wv-cup-steam \{\s*animation: wv-coffee-steam var\(--wv-reaction-cycle\)/);
  assert.doesNotMatch(css, /\b(?:filter|backdrop-filter|will-change)\s*:/);
  const frames = animationFrames().get('wv-coffee-steam').frames;
  for (const { properties } of frames) {
    assert.ok(Number(properties.opacity) <= .42);
    assert.ok(Math.abs(Number(/\((-?[\d.]+)/.exec(properties.transform)[1])) <= .8);
  }
});

test('an already completed endpoint delivered during a pause is reconciled exactly once on resume', () => {
  for (const [phaseId, bridgeMs] of [['analytics-sql', 1200], ['command:case_report_render', 3200]]) for (const stage of ['prepare', 'react', 'resume']) {
    for (const mode of ['manual', 'offscreen', 'hidden', 'panel', 'reduced']) {
      const { f, visual } = adaptedCoffeeFixture(phaseId);
      if (stage !== 'prepare') adapterBoundary(f, visual);
      if (stage === 'resume') reactionBoundary(f, visual, 'animationend');
      const frozen = plain(visual.inspect());
      const toggle = paused => {
        if (mode === 'manual') visual.setMotionEnabled(!paused);
        if (mode === 'offscreen') f.observers[0].deliver(!paused);
        if (mode === 'hidden') { f.document.hidden = paused; f.document.emit('visibilitychange'); }
        if (mode === 'panel') visual.setVisible(!paused);
        if (mode === 'reduced') { f.media.matches = paused; f.media.emit('change'); }
      };
      const completed = () => stage === 'react'
        ? reactionBoundary(f, visual, 'animationend', { elapsedTime: 32 })
        : adapterBoundary(f, visual, stage, { elapsedTime: bridgeMs / 1000 });
      toggle(true); completed(); completed();
      assert.deepEqual(plain(visual.inspect()), frozen, `${mode} preserves the completed pose until resumed`);
      toggle(false);
      const expected = stage === 'prepare' ? 'react' : stage === 'react' ? 'resume' : 'work';
      assert.equal(visual.element.dataset.adapter, expected);
      assert.deepEqual(plain(visual.inspect().history), ['coffee'], 'reconciliation cannot draw another episode');
      const reconciled = plain(visual.inspect()); completed();
      assert.deepEqual(plain(visual.inspect()), reconciled, 'duplicate stale completion cannot advance again');
      visual.destroy();
    }
  }
});

test('a deferred completion is duration-validated, never stores work iterations and dies with its scene owner', () => {
  for (const invalid of [undefined, null, -1, 0, .5, 1.1, 1.3, 32, Infinity, NaN]) {
    const { f, visual } = adaptedCoffeeFixture('analytics-sql');
    visual.setMotionEnabled(false); adapterBoundary(f, visual, 'prepare', { elapsedTime: invalid });
    visual.setMotionEnabled(true); assert.equal(visual.element.dataset.adapter, 'prepare'); visual.destroy();
  }
  const { f, visual } = adaptedCoffeeFixture('command:case_report_render');
  visual.setMotionEnabled(false); adapterBoundary(f, visual, 'prepare', { elapsedTime: 3.2 });
  visual.update(receipt({ operationId: 'new-owner', phaseId: 'analytics-sql', elapsedMs: 60000 }));
  visual.setMotionEnabled(true);
  assert.equal(visual.element.dataset.adapter, 'work'); assert.deepEqual(plain(visual.inspect().history), []);
  visual.setMotionEnabled(false);
  reactionBoundary(f, visual, 'animationiteration', { elapsedTime: 64 });
  const before = plain(visual.inspect()); visual.setMotionEnabled(true); assert.deepEqual(plain(visual.inspect()), before);
  visual.destroy();
});

function manualFrontPoint(point, seconds, family = 'checkpoint', book = false) {
  const time = seconds / 32 * 100;
  if (book) {
    point = transformPoint(point, 'translate(27px, -3.7px)');
    point = transformPoint(point, interpolated('wv-manual-wrist', time), [91, 55]);
  }
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]]]) point = transformPoint(point, interpolated(`wv-manual-${part}`, time), origin);
  return transformPoint(point, interpolated(`wv-coffee-travel-${family}`, time));
}
function manualBackPoint(point, seconds, family = 'checkpoint', book = false) {
  const time = seconds / 32 * 100;
  if (book) {
    point = transformPoint(point, 'translate(-10px, 2.33809621px)');
    point = transformPoint(point, interpolated('wv-manual-support-wrist', time), [54, 65]);
  }
  point = transformPoint(point, interpolated('wv-manual-arm-back', time), [56, 51]);
  return transformPoint(point, interpolated(`wv-coffee-travel-${family}`, time));
}
const nearPoint = (a, b, tolerance = .00001, message = 'same point') => assert.ok(Math.hypot(a[0] - b[0], a[1] - b[1]) < tolerance, `${message}: ${a} ≈ ${b}`);

test('manual has a closed front-hand transport, one rear-hand support and exact ownership handoffs', () => {
  for (const family of ['reading', 'checkpoint', 'calculation', 'composition', 'verification']) {
    const art = artFor(family);
    assert.ok(parentsOf(art, 'wv-manual-held').includes('wv-react-hand'));
    assert.ok(parentsOf(art, 'wv-manual-supported').includes('wv-react-arm-back'));
    assert.ok(parentsOf(art, 'wv-manual-cover-fold').includes('wv-manual-supported'));
    assert.ok(parentsOf(art, 'wv-manual-page').includes('wv-manual-supported'));
    assert.ok(parentsOf(art, 'wv-manual-shelf').includes('wv-manual-kit'));
    assert.ok(!parentsOf(art, 'wv-manual-shelf').includes('wv-kitchen'), 'book owns an open shelf outside the coffee fixture');
    assert.match(art, /class="wv-manual-shelf" transform="translate\(109 -3.7\)"/);
    assert.equal([...art.matchAll(/class="wv-react-arm-back"/g)].length, 1, 'no second supporting arm');
    assert.match(art, /class="wv-react-arm-back"><path class="wv-react-limb" d="m56 51-7 9 5 5"/, 'the original rear silhouette is retained');
    const travel = family === 'reading' ? 'reading' : 'checkpoint';
    for (const seconds of [9, 26.5]) for (const point of [[64, 48.5], [74.5, 63], [64, 58.7]]) {
      nearPoint(manualFrontPoint(point, seconds, travel, true), [point[0] + 109, point[1] - 3.7], .00001, `${family}: shelf contact at ${seconds}s`);
    }
    for (const seconds of [15, 22]) for (const point of [[64, 48.5], [74.5, 63], [85, 48.5]]) {
      nearPoint(manualFrontPoint(point, seconds, travel, true), manualBackPoint(point, seconds, travel, true), .00001, `${family}: complete matrices coincide at ${seconds}s`);
    }
  }
  for (const [seconds, shelf, front, back] of [[0,1,0,0],[9,0,1,0],[14.5,0,1,0],[15,0,0,1],[20,0,0,1],[22,0,1,0],[26.5,1,0,0],[32,1,0,0]]) {
    const time = seconds / 32 * 100;
    assert.equal(atContact('wv-manual-shelf-book', time, 'opacity'), String(shelf));
    assert.equal(atContact('wv-manual-held', time, 'opacity'), String(front));
    assert.equal(atContact('wv-manual-supported', time, 'opacity'), String(back));
    assert.equal(shelf + front + back, 1, 'one owner at a time');
  }
});

test('manual is supported by both palms, and cover/page edges follow actual front-hand contacts', () => {
  const y = 51 + Math.sqrt(136);
  for (const family of ['reading', 'checkpoint']) {
    const home = family === 'reading' ? 0 : 20;
    for (const seconds of [16.2,16.6,20.2,20.6]) {
      nearPoint(manualBackPoint([54,65], seconds, family), [64 + home,y], .00001, 'rear palm supports left lower corner');
      nearPoint(manualFrontPoint([91,55], seconds, family), [85 + home,y], .00001, 'front palm supports right lower corner');
    }
    for (const [name,start,end] of [['wv-manual-cover-fold',15,16],['wv-manual-cover-fold',21,22],['wv-manual-page',18.2,19.6]]) {
      for (let seconds = start; seconds <= end + .00001; seconds += .0125) {
        const edge = transformPoint([85,58.7], interpolated(name, seconds / 32 * 100), [74.5,0]);
        nearPoint(manualFrontPoint([91,55], seconds, family), [edge[0] + home,edge[1]], .03, `${name} never moves without finger contact at ${seconds}s`);
        nearPoint(manualBackPoint([54,65], seconds, family), [64 + home,y], .00001, 'rear support cannot drift while front hand works');
      }
    }
    for (const [seconds,point] of [[17,[77,52]],[17.4,[83,52]],[17.6,[77,55]],[18,[83,55]]]) {
      nearPoint(manualFrontPoint([91,55], seconds, family), [point[0]+home,point[1]], .00001, 'finger follows the printed lines');
    }
  }
  assert.equal(atContact('wv-manual-head', 20.2 / 32 * 100), 'rotate(-5deg)', 'looks back toward the task station before closing');
  assert.equal(atContact('wv-manual-cover-fold', 22 / 32 * 100), 'scaleX(-1)', 'the book is closed before transport ownership returns');
  assert.equal(atContact('wv-manual-page-visible', 21 / 32 * 100, 'opacity'), '0', 'the turned leaf rejoins the matching static left page before closing');
  const page = animationFrames().get('wv-manual-page').frames;
  assert.deepEqual(page.filter(frame => frame.time >= 56.875 && frame.time <= 61.25).map(frame => frame.properties.transform), ['scaleX(1)', 'scaleX(-1)'], 'exactly one turn');
  assert.ok(page.find(frame => frame.properties.transform === 'scaleX(1)' && frame.time > 61.25).time > 68.75, 'reset happens only while the supported book is invisible');
});

test('manual shares travel but owns its open shelf, closes its path, and never enters a short operation', () => {
  const { api } = fixture();
  assert.deepEqual(plain(api.repertoire.manual), { minElapsedMs:60000, durationMs:32000, cooldown:5, weight:1, long:true });
  for (const age of [3999,45000,59999]) {
    const director = api.createDirector('manual-gate', 5);
    for (let cycle=0; cycle<200; cycle++) {
      const event=director.boundary(api.derive(receipt({elapsedMs:age})));
      assert.notEqual(event?.episode,'manual'); director.finish();
    }
  }
  assert.doesNotMatch(css, /@keyframes wv-manual-(?:travel|reading|checkpoint|calculation|composition|leg|knee|foot)/, 'no duplicated excursion curves for any adapter');
  for (const part of ['reaction-actor','react-leg-front','react-knee-front','react-foot-front','react-leg-back','react-knee-back','react-foot-back']) {
    const rules = css.split('\n').filter(line => line.includes(`] .wv-${part} { animation: wv-coffee-`));
    assert.equal(rules.filter(line => line.includes(':is([data-episode="coffee"], [data-episode="manual"])')).length,2, `${part} shares both walking paths`);
  }
  assert.match(css, /\[data-episode="manual"\] \.wv-cup-shelf \{ visibility: hidden; \}/);
  assert.equal(atContact('wv-coffee-kitchen',0,'opacity'),'0'); assert.equal(atContact('wv-coffee-kitchen',100,'opacity'),'0');
  for (const family of ['reading','checkpoint']) {
    for (const time of [0,100]) {
      assert.equal(atContact('wv-manual-arm',time),'rotate(40.00000000deg)');
      assert.equal(atContact('wv-manual-hand',time),'rotate(-85.00000000deg)');
      assert.equal(atContact(`wv-coffee-travel-${family}`,time),`translateX(${family === 'reading' ? '0' : '20'}.000000px)`);
    }
  }
});

test('manual uses every existing adapter and respects pauses, pending completion and terminal removal', () => {
  for (const phaseId of ['metadata-scan','metadata-checkpoint-write','analytics-sql','command:case_report_render','metadata-validate']) {
    const f=fixture(), visual=f.api.mount(f.host,receipt({phaseId,elapsedMs:60000}),{reactionSeed:4});
    f.observers[0].deliver(true); reactionBoundary(f,visual);
    assert.equal(visual.element.dataset.episode,'manual');
    const bridge=f.api.adapters[visual.element.dataset.family].bridgeMs;
    if(bridge) { assert.equal(visual.element.dataset.adapter,'prepare'); adapterBoundary(f,visual); }
    assert.equal(visual.element.dataset.adapter,'react');
    const {art,status}=f.parts(visual), writes=status.writes;
    visual.setMotionEnabled(false);
    reactionBoundary(f,visual,'animationend',{elapsedTime:32});
    assert.equal(visual.element.dataset.adapter,'react');
    visual.setMotionEnabled(true);
    if(bridge) { assert.equal(visual.element.dataset.adapter,'resume'); adapterBoundary(f,visual); }
    assert.equal(visual.element.dataset.episode,'work');
    assert.equal(visual.inspect().cooldown,5); assert.deepEqual(plain(visual.inspect().history),['manual']);
    assert.equal(art.htmlWrites,1); assert.equal(status.writes,writes);
    visual.update(receipt({phaseId,state:'completed'}));
    assert.doesNotMatch(art.innerHTML,/wv-manual|wv-kitchen|wv-reaction-actor/);
    visual.destroy();
  }
});

test('manual shelf and coffee nook are exclusive props, with the shelf established before approach and removed after return', () => {
  const art = artFor('checkpoint');
  assert.ok(parentsOf(art,'wv-cup-shelf').includes('wv-kitchen'));
  assert.ok(!parentsOf(art,'wv-cup-shelf').includes('wv-manual-kit'));
  assert.ok(parentsOf(art,'wv-manual-shelf-board').includes('wv-manual-kit'));
  assert.ok(parentsOf(art,'wv-manual-shelf-bracket').includes('wv-manual-kit'));
  assert.match(css, /\.wv-manual-kit \{ opacity: 0; \}/, 'the shelf has no resting scenery outside the episode');
  for (const part of ['kitchen','kitchen-hatch']) {
    const animated = css.split('\n').filter(line => line.includes(`] .wv-${part} {`) && line.includes('data-animated'));
    assert.equal(animated.length,1);
    assert.ok(animated[0].includes('[data-episode="coffee"][data-adapter="react"]'));
    assert.ok(!animated[0].includes('manual'), `${part} never appears or animates during manual`);
  }
  assert.match(css, /\[data-episode="manual"\]\[data-adapter="react"\] \.wv-manual-kit \{\s*animation: wv-manual-kit/);
  assert.match(css, /\[data-episode="manual"\]\[data-adapter="react"\] \.wv-manual-shelf \{ animation: wv-manual-shelf-book/);
  for (const [time,opacity] of [[0,'0'],[5,'1'],[97,'1'],[100,'0']]) assert.equal(atContact('wv-manual-kit',time,'opacity'),opacity);
  for (const family of ['reading','checkpoint']) {
    const home=family==='reading' ? 0 : 20;
    for (const time of [5,97]) assert.equal(atContact(`wv-coffee-travel-${family}`,time),`translateX(${home}.000000px)`, 'shelf precedes the first step and outlasts the complete return');
  }
});

test('manual reaches directly toward its visible book and retracts after returning it, without a phantom hatch gesture', () => {
  for (const family of ['reading','checkpoint']) {
    const neutral = manualFrontPoint([91,55],6.5,family);
    for (const seconds of [8.75,9,26.5,26.9]) nearPoint(manualFrontPoint([91,55],seconds,family),[173,55],.00001,'hand meets the closed book at shelf height');
    const point = (seconds) => manualFrontPoint([91,55],seconds,family);
    for (const [start,end,from,to] of [[6.5,8.75,neutral,[173,55]],[26.9,28.5,[173,55],neutral]]) {
      for (let seconds=start;seconds<=end+.00001;seconds+=.025) {
        const fraction=(seconds-start)/(end-start), expected=from.map((v,axis)=>v+(to[axis]-v)*fraction);
        nearPoint(point(seconds),expected,.02,'direct reach/retract stays at book height instead of pulling a door upward');
        const time=seconds/32*100;
        assert.equal(atContact('wv-manual-held',time,'opacity'),'0');
        assert.equal(atContact('wv-manual-supported',time,'opacity'),'0');
      }
    }
    nearPoint(point(28.5),neutral,.00001,'empty hand is settled before walking home');
    nearPoint(point(29.3),neutral,.00001,'no trailing closing gesture');
  }
});


function gestureBoundary(f, visual, overrides = {}) {
  const art = f.parts(visual).art, family = visual.element.dataset.family;
  const elapsedTime = ['checkpoint', 'composition'].includes(family) ? 2.8 : ['calculation', 'verification'].includes(family) ? 2.6 : 2.4;
  art.emit('animationend', { target: art.children.find(node => node.className === 'wv-work-boundary'),
    animationName: 'wv-gesture-boundary', elapsedTime, pseudoElement: '', ...overrides });
}

test('quiet first receipts progress from their complete glance to continuous work without another receipt', () => {
  for (const elapsedMs of [undefined, 0, 3999]) {
    const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs })); f.observers[0].deliver(true);
    const { art, status, details } = f.parts(visual), announcements = status.writes;
    assert.equal(visual.element.dataset.pace, 'gesture'); gestureBoundary(f, visual);
    assert.equal(visual.element.dataset.pace, 'loop'); assert.equal(visual.element.dataset.motion, 'running');
    assert.equal(art.htmlWrites, 1); assert.equal(status.writes, announcements);
    assert.equal(details.textContent, elapsedMs === undefined ? '' : `${Math.floor(elapsedMs / 1000)} s decorridos`);
    for (let cycle = 0; cycle < 8; cycle++) reactionBoundary(f, visual);
    assert.equal(visual.inspect().cycle, 8); assert.equal(visual.element.dataset.episode, 'work');
    assert.match(css, /animation: wv-gesture-boundary var\(--wv-gesture-cycle\) linear 1 both;/); visual.destroy();
  }
});

test('missing and reset elapsed receipts cannot demote a running loop or recreate art', () => {
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 9000 })); f.observers[0].deliver(true);
  for (const elapsedMs of [undefined, 0, 100, 3999, undefined]) {
    visual.update(receipt({ elapsedMs, completed: 3 }));
    assert.equal(visual.element.dataset.pace, 'loop'); assert.equal(f.parts(visual).art.htmlWrites, 1);
  }
  reactionBoundary(f, visual); assert.equal(visual.inspect().cycle, 1); visual.destroy();
});

test('rapid family changes coalesce at the complete work boundary while real receipts stay immediate', () => {
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 9000 })); f.observers[0].deliver(true);
  const { art, status, metric } = f.parts(visual), staleClock = art.children.find(node => node.className === 'wv-work-boundary');
  for (let index = 0; index < 1000; index++) {
    visual.update(receipt({ phaseId: index % 2 ? 'engine-checkpoint-publish' : 'engine-index', elapsedMs: index,
      label: `Phase ${index}`, completed: index, total: 2000 }));
    assert.equal(visual.element.dataset.family, 'reading'); assert.equal(art.htmlWrites, 1);
  }
  assert.equal(status.textContent, 'Phase 999'); assert.equal(metric.textContent, '999 / 2.000');
  reactionBoundary(f, visual); assert.equal(visual.element.dataset.family, 'checkpoint'); assert.equal(art.htmlWrites, 2);
  assert.equal(visual.element.dataset.pace, 'loop'); assert.equal(visual.inspect().cycle, 0);
  reactionBoundary(f, visual, 'animationiteration', { target: staleClock }); assert.equal(visual.inspect().cycle, 0);
  reactionBoundary(f, visual); assert.equal(visual.inspect().cycle, 1); visual.destroy();
});

test('phase changes during bridges complete parking, reaction and recovery with the active adapter', () => {
  const { f, visual } = adaptedCoffeeFixture('command:case_report_render');
  visual.update(receipt({ phaseId: 'engine-index', elapsedMs: 1000 })); assert.equal(visual.element.dataset.family, 'composition');
  adapterBoundary(f, visual); assert.equal(visual.element.dataset.adapter, 'react');
  reactionBoundary(f, visual, 'animationend'); assert.equal(visual.element.dataset.adapter, 'resume');
  assert.equal(visual.element.dataset.family, 'composition'); adapterBoundary(f, visual);
  assert.equal(visual.element.dataset.family, 'reading'); assert.equal(visual.element.dataset.adapter, 'work');
  assert.equal(visual.inspect().cooldown, 4); visual.destroy();
});

test('quiet promotion pauses with visibility and reconciles only a complete endpoint on resume', () => {
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 0 })); f.observers[0].deliver(true);
  visual.setMotionEnabled(false); gestureBoundary(f, visual, { elapsedTime: 1 });
  visual.setMotionEnabled(true); assert.equal(visual.element.dataset.pace, 'gesture');
  visual.setMotionEnabled(false); gestureBoundary(f, visual); assert.equal(visual.element.dataset.pace, 'gesture');
  visual.setMotionEnabled(true); assert.equal(visual.element.dataset.pace, 'loop');
  visual.update(receipt({ state: 'completed' })); gestureBoundary(f, visual);
  assert.equal(visual.element.dataset.family, 'neutral'); assert.equal(visual.element.dataset.animated, 'false'); visual.destroy();
});

test('access closes its finite gesture before pending or late phase changes without trapping the scene', () => {
  for (const late of [false, true]) {
    const f = fixture(), visual = f.api.mount(f.host, receipt({ phaseId: 'metadata-lock', elapsedMs: 10000 })); f.observers[0].deliver(true);
    if (late) gestureBoundary(f, visual);
    visual.update(receipt({ phaseId: 'engine-index', elapsedMs: 10000 }));
    if (!late) { assert.equal(visual.element.dataset.family, 'access'); gestureBoundary(f, visual); }
    assert.equal(visual.element.dataset.family, 'reading');
    for (const phaseId of ['future-unknown', 'engine-cancelled']) {
      visual.update(receipt({ phaseId, elapsedMs: 10000 })); assert.equal(visual.element.dataset.motion, 'static');
      assert.equal(visual.element.dataset.animated, 'false'); visual.update(receipt({ elapsedMs: 10000 }));
    }
    visual.destroy();
  }
});

test('elapsed receipts cannot cut a gesture or replace paused animation identities', () => {
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 3500 })); f.observers[0].deliver(true);
  visual.update(receipt({ elapsedMs: 4000 })); assert.equal(visual.element.dataset.pace, 'gesture');
  visual.setMotionEnabled(false); visual.update(receipt({ elapsedMs: 65000 }));
  assert.equal(visual.element.dataset.pace, 'gesture'); assert.equal(visual.element.dataset.motion, 'static');
  f.media.matches = true; f.media.emit('change'); visual.setMotionEnabled(true);
  visual.update(receipt({ elapsedMs: 66000 })); assert.equal(visual.element.dataset.pace, 'gesture');
  f.media.matches = false; f.media.emit('change'); assert.equal(visual.element.dataset.pace, 'gesture');
  gestureBoundary(f, visual); assert.equal(visual.element.dataset.pace, 'loop'); visual.destroy();
});

test('partial checkpoints preserve the active scene but never start another reaction', () => {
  const { f, visual } = coffeeFixture(); const { art } = f.parts(visual);
  for (const phaseId of ['metadata-checkpoint-committed', 'engine-checkpoint-committed']) {
    visual.update(receipt({ phaseId, elapsedMs: 60000, completed: 100, total: 100 }));
    assert.equal(visual.element.dataset.episode, 'coffee'); assert.equal(visual.element.dataset.motion, 'running'); assert.equal(art.htmlWrites, 1);
  }
  reactionBoundary(f, visual, 'animationend'); for (let index = 0; index < 20; index++) reactionBoundary(f, visual);
  assert.equal(visual.element.dataset.episode, 'work'); assert.deepEqual(plain(visual.inspect().history), ['coffee']); visual.destroy();
});

test('successful short work releases its host immediately and finishes only the existing inert gesture', () => {
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 0 })); f.observers[0].deliver(true);
  const { art, status, control } = f.parts(visual);
  assert.equal(visual.complete(), true); assert.equal(visual.element.parent, f.document.body);
  assert.equal(visual.element.inert, true); assert.equal(visual.element.getAttribute('aria-hidden'), 'true');
  assert.equal(visual.element.dataset.state, 'completed'); assert.equal(control.hidden, true); assert.equal(status.textContent, 'Concluído');
  assert.equal(visual.inspect().finishing, true); assert.equal(visual.update(receipt()), false);
  visual.setVisible(true); assert.equal(visual.element.inert, true); assert.equal(visual.element.getAttribute('aria-hidden'), 'true');
  gestureBoundary(f, visual); assert.equal(visual.element.isConnected, false); assert.equal(art.count('animationend'), 0);
  assert.equal(f.document.count('workspace-context-change'), 0); assert.equal(f.document.count('analysis-context-change'), 0);
});

test('successful long work and reactions finish their complete path without adding new work', () => {
  for (const phaseId of ['metadata-scan', 'analytics-sql', 'command:case_report_render']) {
    const f = fixture(), visual = f.api.mount(f.host, receipt({ phaseId, elapsedMs: 60000 }), { reactionSeed: 17 }); f.observers[0].deliver(true);
    reactionBoundary(f, visual); assert.equal(visual.element.dataset.episode, 'coffee'); assert.equal(visual.complete(), true);
    if (visual.element.dataset.adapter === 'prepare') adapterBoundary(f, visual);
    reactionBoundary(f, visual, 'animationend'); if (visual.element.dataset.adapter === 'resume') adapterBoundary(f, visual);
    assert.equal(visual.element.isConnected, false); assert.deepEqual(plain(visual.inspect().history), ['coffee']);
  }
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 5000 })); f.observers[0].deliver(true);
  assert.equal(visual.complete(), true); reactionBoundary(f, visual); assert.equal(visual.element.isConnected, false);
});

test('completion has one owner and cleans up on context, visibility, reduced motion, destroy or a new task', () => {
  for (const stop of ['workspace-context-change', 'analysis-context-change', 'hidden', 'reduced', 'new-task', 'destroy']) {
    const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 5000 })); f.observers[0].deliver(true); visual.complete();
    if (stop === 'hidden') { f.document.hidden = true; f.document.emit('visibilitychange'); }
    else if (stop === 'reduced') { f.media.matches = true; f.media.emit('change'); }
    else if (stop === 'new-task') f.api.mount(f.host, receipt({ operationId: 'next' })).destroy();
    else if (stop === 'destroy') visual.destroy(); else f.document.emit(stop);
    assert.equal(visual.element.isConnected, false, stop); assert.equal(f.document.count('workspace-context-change'), 0);
  }
  const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 5000 })); f.observers[0].deliver(true);
  visual.setMotionEnabled(false); assert.equal(visual.complete(), false); assert.equal(visual.element.isConnected, false);
});

test('terminal and cancelling states stop immediately while retaining a committed checkpoint phase ID', () => {
  for (const phaseId of ['metadata-checkpoint-committed', 'engine-checkpoint-committed']) for (const state of ['cancelling', 'cancelled', 'completed', 'error', 'paused']) {
    const f = fixture(), visual = f.api.mount(f.host, receipt({ elapsedMs: 5000 })); f.observers[0].deliver(true);
    visual.update(receipt({ phaseId, elapsedMs: 6000 })); assert.equal(visual.element.dataset.motion, 'running');
    visual.update(receipt({ phaseId, state, elapsedMs: 6100 })); assert.equal(visual.element.dataset.motion, 'static', `${phaseId}/${state}`);
    assert.equal(visual.element.dataset.family, 'neutral'); assert.equal(visual.element.dataset.animated, 'false'); visual.destroy();
  }
});
