import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const receipt = (overrides = {}) => ({ operationId: 'load-17', phaseId: 'metadata-scan', state: 'running', label: 'Indexando metadados', ...overrides });
const plain = value => JSON.parse(JSON.stringify(value));
const load = () => { const window = {}; vm.runInNewContext(source, { window, Intl }); return window.WaitingVisuals; };

// Minimal DOM: enough for the component's contract, plus an optional WAAPI stub.
function fixture({ reduced = false, intersection = true, animate = true } = {}) {
  const animations = [];
  class Target {
    listeners = new Map();
    addEventListener(type, callback) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(callback); }
    removeEventListener(type, callback) { this.listeners.get(type)?.delete(callback); }
    count(type) { return this.listeners.get(type)?.size || 0; }
  }
  class FakeAnimation {
    constructor(node, keyframes, options) { Object.assign(this, { node, keyframes, options, playState: 'running', currentTime: 0, startTime: null, cancelled: false }); animations.push(this); }
    pause() { this.playState = 'paused'; }
    play() { this.playState = 'running'; }
    cancel() { this.playState = 'idle'; this.cancelled = true; }
    finish() { this.playState = 'finished'; this.onfinish?.(); }
  }
  class Node extends Target {
    constructor(document, tag) { super(); Object.assign(this, { ownerDocument: document, tag, children: [], dataset: {}, attributes: {}, style: {}, hidden: false, html: '' }); }
    append(...nodes) { for (const node of nodes) { node.remove(); node.parent = this; this.children.push(node); } }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter(node => node !== this); this.parent = null; }
    get isConnected() { return this === this.ownerDocument.body || !!this.parent?.isConnected; }
    set textContent(value) { this.text = String(value); }
    get textContent() { return this.text || ''; }
    set innerHTML(value) { this.html = value; this.stubs = new Map(); }
    get innerHTML() { return this.html; }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    getAttribute(name) { return this.attributes[name] ?? null; }
    stub(selector) {
      if (!this.stubs.has(selector)) { const node = new Node(this.ownerDocument, selector); node.parent = this; this.stubs.set(selector, node); }
      return this.stubs.get(selector);
    }
    querySelectorAll(selector) { return this.html.includes(selector.match(/"([^"]+)"/)?.[1] ?? '\u0000') ? [this.stub(selector)] : []; }
  }
  if (animate) {
    Node.prototype.querySelector = function (selector) { return this.html.includes(selector.slice(1)) ? this.stub(selector) : null; };
    Node.prototype.animate = function (keyframes, options) { return new FakeAnimation(this, keyframes, options); };
  }
  const document = new Target(); document.hidden = false; document.timeline = { currentTime: 1000 };
  document.createElement = name => new Node(document, name);
  document.body = document.createElement('body');
  const host = document.createElement('div'); document.body.append(host);
  const media = new Target(); media.matches = reduced;
  const observers = [];
  class Observer {
    constructor(callback) { this.callback = callback; observers.push(this); }
    observe(target) { this.target = target; }
    deliver(isIntersecting) { this.callback([{ target: this.target, isIntersecting, intersectionRatio: isIntersecting ? 1 : 0 }]); }
    disconnect() { this.disconnected = true; }
  }
  let now = 0;
  const window = { performance: { now: () => now }, matchMedia: () => media, ...(intersection ? { IntersectionObserver: Observer } : {}) };
  document.defaultView = window;
  vm.runInContext(source, vm.createContext({ window, document, Intl }));
  const api = window.WaitingVisuals;
  const clocks = () => animations.filter(a => a.node.tag === '.wv-timer');
  return { api, host, document, media, observers, animations, clocks, advance: ms => { now += ms; },
    parts: view => { const [art, status, metric, details] = view.element.children; return { art, status, metric, details }; } };
}

test('only exact real phase IDs select a contextual family; the loader is live for any running operation', () => {
  const { derive } = load();
  assert.equal(derive(receipt()).family, 'reading');
  assert.equal(derive(receipt({ phaseId: 'command:case_report_render' })).family, 'composition');
  for (const phaseId of ['metadata-checkpoint-write', 'engine-checkpoint-publish']) assert.equal(derive(receipt({ phaseId })).family, 'checkpoint');
  for (const phaseId of ['analytics-sql', 'command:aggregate_events', 'command:pivot']) assert.equal(derive(receipt({ phaseId })).family, 'calculation');
  for (const phaseId of ['', 'future-phase', 'METADATA-SCAN', 'toString', '__proto__']) {
    const model = derive(receipt({ phaseId, label: 'Calculando 100%' }));
    assert.equal(model.family, 'neutral'); assert.equal(model.canAnimate, false); assert.equal(model.live, true);
  }
  assert.equal(derive(receipt({ operationId: '' })).live, false);
  for (const state of ['queued', 'cancelling']) assert.equal(derive(receipt({ state })).live, true);
  for (const state of ['paused', 'error', 'cancelled', 'completed', 'idle', 'bogus']) assert.equal(derive(receipt({ state })).live, false);
  assert.equal(derive(receipt({ phaseId: 'engine-checkpoint-committed' })).canAnimate, false);
});

test('only supplied valid measurements appear; no fabricated totals, phase counts or estimates', () => {
  const { derive } = load();
  assert.equal(derive(receipt({ completed: 12, total: 99, unit: 'registros' })).metric, '12 / 99 registros');
  assert.equal(derive(receipt({ completed: 12, total: 0 })).metric, '12');
  assert.equal(derive(receipt({ completed: 120, total: 99 })).metric, '120');
  assert.equal(derive(receipt({ total: 99 })).metric, '');
  assert.deepEqual(plain(derive(receipt({ phaseIndex: 2, phaseCount: 1 })).details), ['Etapa 2']);
  assert.deepEqual(plain(derive(receipt({ elapsedMs: 61000, estimateMs: 5000 })).details), ['1 min 1 s decorridos', '≈ 5 s restantes nesta etapa']);
  assert.deepEqual(plain(derive(receipt({ state: 'paused', estimateMs: 5000 })).details), []);
  assert.equal(derive(receipt({ state: 'error', label: 'Disco cheio' })).status, 'Não foi possível continuar · Disco cheio');
});

test('every skit starts and ends with an empty room, for many random variations', () => {
  const { compile, skits } = load();
  const outside = (x, half) => x + half < 4 || x - half > 196;
  for (const name of Object.keys(skits)) {
    for (let seed = 1; seed <= 25; seed++) {
      const clip = compile(name, { seed });
      for (const time of [0, clip.duration]) {
        const probe = clip.probe(time);
        assert.ok(probe.robot <= -24 || probe.robot >= 224, `${name}/${seed}: robot visible at ${time} ms (${probe.robot})`);
        // Ceiling screens wait above the room (their bottom edge above the window).
        for (const prop of probe.props) assert.ok(prop.opacity < 0.01 || prop.y < -112 || outside(prop.x, prop.half), `${name}/${seed}: ${prop.id} visible at ${time} ms`);
      }
      assert.ok(clip.duration >= 3000 && clip.duration <= 30000, `${name}: ${clip.duration} ms`);
    }
  }
});

test('skits animate only transform, opacity and chalk stroke offsets with ordered keyframes', () => {
  const { compile, skits } = load();
  const allowed = new Set(['offset', 'easing', 'transform', 'opacity', 'strokeDashoffset']);
  for (const name of Object.keys(skits)) {
    const clip = compile(name, { seed: 3 });
    assert.ok(clip.tracks.length > 5, name);
    for (const track of clip.tracks) {
      let previous = 0;
      for (const frame of track.keyframes) {
        for (const key of Object.keys(frame)) assert.ok(allowed.has(key), `${name}: ${key}`);
        assert.ok(frame.offset >= previous && frame.offset <= 1, `${name}/${track.name}: offset order`);
        previous = frame.offset;
      }
      assert.equal(track.keyframes[0].offset, 0); assert.equal(track.keyframes.at(-1).offset, 1);
    }
    // Only constant artwork enters the markup; no ids, external references or scripts.
    assert.doesNotMatch(clip.markup, /\sid=|href|<script|url\(/i, name);
  }
});

test('the director varies skits, never repeats the last two and respects phase and elapsed gates', () => {
  const { createDirector, skits } = load();
  let seed = 7;
  const random = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  const director = createDirector({ random });
  const first = director.next('reading', 0);
  assert.ok(skits[first.name].opener || skits[first.name].families.includes('reading'));
  const seen = new Set([first.name]), history = [first.name];
  for (let i = 0; i < 300; i++) {
    const { name } = director.next('reading', 120000);
    assert.ok(!history.slice(-2).includes(name), `${name} repeated`);
    assert.ok(skits[name].kind === 'fun' || skits[name].families.includes('reading'), name);
    history.push(name); seen.add(name);
  }
  assert.ok(seen.size >= 10, `variety: ${[...seen]}`);
  assert.ok(history.filter(name => name === 'reading').length > 40, 'context work still appears often');
  const early = createDirector({ random });
  early.next('neutral', 0);
  for (let i = 0; i < 200; i++) {
    const { name } = early.next('neutral', 4000);
    assert.equal(skits[name].kind, 'fun'); assert.ok(skits[name].minElapsedMs <= 4000, `${name} before its time`);
  }
  for (let i = 0; i < 50; i++) { const gap = director.gap(); assert.ok(gap >= 1400 && gap <= 4800); }
  assert.ok(director.gap(true) <= 1200, 'the first robot appears soon after the loader');
});

test('without animation support the room is static and there is no pause button', () => {
  const f = fixture({ animate: false });
  const view = f.api.mount(f.host, receipt({ completed: 5, total: 10, unit: 'registros', label: '<img src=x>' }));
  const parts = f.parts(view);
  assert.equal(view.element.children.length, 4);
  assert.equal(parts.art.getAttribute('aria-hidden'), 'true');
  assert.match(parts.art.innerHTML, /wv-ring/); assert.match(parts.art.innerHTML, /wv-scan/);
  assert.doesNotMatch(parts.art.innerHTML, /img/);
  assert.equal(parts.status.textContent, '<img src=x>'); assert.equal(parts.status.getAttribute('aria-live'), 'polite');
  assert.equal(parts.metric.textContent, '5 / 10 registros');
  assert.equal(parts.details.hidden, true);
  f.observers[0].deliver(true);
  assert.equal(view.element.dataset.motion, 'running');
  assert.equal(view.inspect().mode, 'empty');
  assert.equal(view.complete(), false); assert.equal(view.element.isConnected, false); assert.equal(view.update(receipt()), false);
  assert.equal(f.document.count('visibilitychange'), 0);
});

test('a visible running operation waits briefly, plays a whole skit, then empties the room before the next', () => {
  const f = fixture();
  const view = f.api.mount(f.host, receipt(), { random: () => 0.3 });
  assert.equal(f.clocks().length, 0, 'nothing is scheduled before the room is visible');
  f.observers[0].deliver(true);
  assert.equal(view.inspect().mode, 'gap');
  const gap = f.clocks()[0];
  assert.ok(gap.options.duration <= 1200);
  view.update(receipt({ phaseId: 'engine-checkpoint-write', completed: 3 }));
  assert.equal(f.clocks().length, 1, 'receipts never reschedule or cut anything');
  gap.finish();
  const { mode, skit } = view.inspect();
  assert.equal(mode, 'skit'); assert.ok(skit);
  assert.equal(view.element.dataset.skit, skit);
  const tracks = f.animations.filter(a => a.node.tag !== '.wv-timer');
  assert.ok(tracks.length > 5 && tracks.every(a => a.options.fill === 'both' && a.startTime === 1000));
  const skitClock = f.clocks()[1];
  for (let i = 0; i < 5; i++) view.update(receipt({ phaseId: i % 2 ? 'engine-index' : 'analytics-sql', completed: i }));
  assert.equal(view.inspect().skit, skit, 'phase changes wait for the room to be empty');
  skitClock.finish();
  assert.ok(tracks.every(a => a.cancelled));
  assert.equal(view.inspect().mode, 'gap'); assert.equal(view.element.dataset.skit, '');
  view.destroy();
});

test('hiding, offscreen and reduced motion freeze every track; terminal states never start new skits', () => {
  const f = fixture();
  const view = f.api.mount(f.host, receipt(), { random: () => 0.6 });
  f.observers[0].deliver(true); f.clocks()[0].finish();
  const live = () => f.animations.filter(a => !a.cancelled && a.playState !== 'finished');
  view.setVisible(false);
  assert.equal(view.element.dataset.motion, 'static');
  assert.ok(live().every(a => a.playState === 'paused'));
  view.setVisible(true);
  assert.ok(live().every(a => a.playState === 'running' || Number.isFinite(a.startTime)));
  f.observers[0].deliver(false); assert.equal(view.element.dataset.motion, 'static');
  f.observers[0].deliver(true); assert.equal(view.element.dataset.motion, 'running');
  view.update(receipt({ state: 'cancelling' }));
  f.clocks().at(-1).finish();
  assert.equal(view.inspect().mode, 'empty', 'a cancelling operation lets the robot leave and stops there');
  view.update(receipt({ state: 'error', label: 'Falhou' }));
  assert.equal(view.element.dataset.motion, 'static'); assert.equal(view.element.dataset.state, 'error');
  view.destroy();
  const reduced = fixture({ reduced: true });
  const still = reduced.api.mount(reduced.host, receipt());
  reduced.observers[0].deliver(true);
  assert.equal(still.inspect().skit, 'still', 'reduced motion shows a standing robot instead of motion');
  assert.ok(reduced.animations.filter(a => !a.cancelled && a.playState !== 'finished').every(a => a.playState === 'paused'));
  reduced.media.matches = false; for (const callback of reduced.media.listeners.get('change')) callback();
  assert.ok(reduced.animations.filter(a => !a.cancelled && a.playState !== 'finished').every(a => Number.isFinite(a.startTime)), 'resuming lets the robot walk out');
  still.destroy();
  assert.equal(reduced.media.count('change'), 0);
});

test('the room styles stay local, dark, themed by tokens and paused when motion is not allowed', () => {
  assert.doesNotMatch(css, /url\(|@import|wv-motion-toggle/);
  assert.match(css, /\.wv-scan \{[\s\S]*animation: wv-scan/);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\s*\.waiting-visual \*, \.waiting-visual \*::after \{ animation-play-state: paused !important; \}/);
  assert.match(css, /\.waiting-visual\[data-motion="static"\] \*/);
  assert.match(css, /\.waiting-visual\[data-mirror="true"\] \.wv-unmirror \{ transform: scale\(-1, 1\); \}/);
  assert.ok(Buffer.byteLength(source) < 160000 && Buffer.byteLength(css) < 48000, 'the component stays small');
});
