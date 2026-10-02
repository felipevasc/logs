/* Pure Node geometry and lifecycle contracts. These sample the written CSS, not
   rendered browser frames; natural playback, themes and video remain CI work. */
import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const receipt = (overrides = {}) => ({ operationId: 'load-17', phaseId: 'metadata-scan', state: 'running',
  label: 'Lendo dados', completed: 27, total: 27, elapsedMs: 60000, ...overrides });
const phases = { reading: 'metadata-scan', checkpoint: 'metadata-checkpoint-write', calculation: 'analytics-sql',
  composition: 'command:case_report_render', verification: 'metadata-validate', restoration: 'metadata-restore' };
const seeds = { visor: 49, wave: 0 };
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
  const window = { performance: { now }, matchMedia: () => media, ...(intersection ? { IntersectionObserver: Observer } : {}) };
  document.defaultView = window;
  const context = vm.createContext({ window, document, Intl }); vm.runInContext(source, context);
  const api = window.WaitingVisuals;
  return { api, host, document, media, observers, parts: visual => {
    const [art, status, metric, details, control] = visual.element.children;
    return { art, status, metric, details, control };
  } };
}


function frames(name) {
  const header = `@keyframes ${name} {`, start = css.indexOf(header);
  assert.ok(start >= 0, `${name} exists`);
  let end = start + header.length, depth = 1;
  while (depth && end < css.length) { if (css[end] === '{') depth++; if (css[end] === '}') depth--; end++; }
  assert.equal(depth, 0);
  return [...css.slice(start + header.length, end - 1).matchAll(/([^{}]+)\{([^{}]+)\}/g)].flatMap(match => {
    const properties = Object.fromEntries([...match[2].matchAll(/([\w-]+):\s*([^;]+);/g)].map(m => [m[1], m[2].trim()]));
    return match[1].trim().split(',').map(selector => ({ time: parseFloat(selector), properties }));
  }).sort((a, b) => a.time - b.time);
}
const tracks = new Map([...css.matchAll(/@keyframes (wv-(?:visor|wave)-[\w-]+) \{/g)].map(m => [m[1], frames(m[1])]));
// Invert cubic-bezier(.42,0,.58,1), matching the actual ease-in-out keyframe timing.
function ease(value) {
  const curve = (t, a, b) => 3 * (1 - t) ** 2 * t * a + 3 * (1 - t) * t ** 2 * b + t ** 3;
  let low = 0, high = 1;
  for (let i = 0; i < 36; i++) { const mid = (low + high) / 2; if (curve(mid, .42, .58) < value) low = mid; else high = mid; }
  return curve((low + high) / 2, 0, 1);
}
function at(name, time, property = 'transform', family = 'reading') {
  const entries = tracks.get(name).filter(entry => property in entry.properties);
  const before = entries.filter(entry => entry.time <= time).at(-1), after = entries.find(entry => entry.time >= time);
  const resolve = value => value.replaceAll('var(--wv-home-head)', family === 'reading' ? '-4deg' : '4deg')
    .replaceAll('var(--wv-home-gaze)', family === 'reading' ? '0px' : '1px');
  const a = resolve(before.properties[property]), b = resolve(after.properties[property]);
  if (before === after || a === b) return a;
  const weight = ease((time - before.time) / (after.time - before.time));
  const numbers = [...b.matchAll(/-?\d*\.?\d+/g)].map(m => Number(m[0]));
  let i = 0;
  return a.replace(/-?\d*\.?\d+/g, value => String(+value + (numbers[i++] - +value) * weight));
}
function rotate(point, angle, origin) {
  const radians = angle * Math.PI / 180, [x, y] = point.map((v, i) => v - origin[i]);
  return [origin[0] + x * Math.cos(radians) - y * Math.sin(radians), origin[1] + x * Math.sin(radians) + y * Math.cos(radians)];
}
function frontPoint(point, episode, time, family = 'reading', wrist = false) {
  const angle = part => parseFloat(at(`wv-${episode}-${part}`, time).slice(7));
  if (wrist) point = rotate(point, angle('wrist'), [91, 55]);
  point = rotate(point, angle('hand'), [82, 58]);
  point = rotate(point, angle('arm'), [72, 52]);
  return [point[0] + (family === 'reading' ? 0 : 20), point[1]];
}
function near(actual, expected, tolerance = 1e-7) {
  assert.ok(Math.hypot(...actual.map((v, i) => v - expected[i])) <= tolerance, `${actual} near ${expected}`);
}
function boundary(f, visual, type = 'work', overrides = {}) {
  const art = f.parts(visual).art;
  const className = type === 'prepare' || type === 'resume' ? 'wv-adapter-boundary' : `wv-${type}-boundary`;
  art.emit(type === 'work' ? 'animationiteration' : 'animationend', {
    target: art.children.find(node => node.className === className), animationName: `wv-${type}-boundary`, pseudoElement: '', ...overrides });
}
function active(episode, family = 'reading') {
  const f = fixture(), visual = f.api.mount(f.host, receipt({ phaseId: phases[family] }), { reactionSeed: seeds[episode], showDetails: true });
  f.observers[0].deliver(true); boundary(f, visual);
  assert.equal(visual.element.dataset.episode, episode, 'a fixed seed must explicitly choose the intended reaction');
  if (f.api.adapters[family].bridgeMs) { assert.equal(visual.element.dataset.adapter, 'prepare'); boundary(f, visual, 'prepare'); }
  assert.equal(visual.element.dataset.adapter, 'react');
  return { f, visual };
}

test('two small reactions have exact CSS durations, measured gates and low weight', () => {
  const { api } = fixture();
  assert.deepEqual(plain(api.repertoire.visor), { minElapsedMs: 24000, durationMs: 4800, cooldown: 4, weight: 1 });
  assert.deepEqual(plain(api.repertoire.wave), { minElapsedMs: 36000, durationMs: 3600, cooldown: 4, weight: 1 });
  for (const [episode, seconds] of [['visor', 4.8], ['wave', 3.6]]) {
    assert.ok(css.includes(`[data-episode="${episode}"][data-adapter="react"] { --wv-reaction-cycle: ${seconds}s; }`));
    for (const family of Object.keys(phases)) {
      const model = api.derive(receipt({ phaseId: phases[family] }));
      assert.equal(api.createDirector('load-17', seeds[episode]).boundary(model).episode, episode);
      const director = api.createDirector('gate', seeds[episode]);
      for (let i = 0; i < 500; i++) {
        const event = director.boundary(api.derive(receipt({ phaseId: phases[family], elapsedMs: api.repertoire[episode].minElapsedMs - 1 })));
        assert.notEqual(event?.episode, episode); director.finish();
      }
    }
  }
  for (const state of ['queued', 'paused', 'cancelling', 'cancelled', 'completed', 'error', 'unknown']) {
    assert.equal(api.createDirector('load-17', 0).boundary(api.derive(receipt({ state }))), null);
  }
  for (const overrides of [{ elapsedMs: undefined }, { elapsedMs: 3999 }, { phaseId: 'future-phase' }, { phaseId: 'metadata-checkpoint-committed' }]) {
    assert.equal(api.createDirector('load-17', 0).boundary(api.derive(receipt(overrides))), null);
  }
});

test('selection remains deterministic, non-repeating, cooldown-bound and work-dominant', () => {
  const { api } = fixture(), model = api.derive(receipt());
  const run = seed => {
    const director = api.createDirector('repertoire-balance', seed), events = [];
    for (let cycle = 0; cycle < 10000; cycle++) { const event = director.boundary(model); if (event) { events.push(plain(event)); director.finish(); } }
    return events;
  };
  const events = run('micro-contract');
  assert.deepEqual(events, run('micro-contract')); assert.notDeepEqual(events, run('other-contract'));
  assert.deepEqual([...new Set(events.map(e => e.episode))].sort(), Object.keys(api.repertoire).sort());
  assert.ok(events.length < 2000, 'at least four fifths of boundaries stay on work');
  assert.ok(events.filter(e => seeds[e.episode] !== undefined).length < events.length * .35, 'new gestures do not dominate the six-part repertoire');
  for (let i = 1; i < events.length; i++) {
    assert.notEqual(events[i].episode, events[i - 1].episode);
    assert.ok(events[i].cycle - events[i - 1].cycle > api.repertoire[events[i - 1].episode].cooldown);
    assert.ok(!(api.repertoire[events[i].episode].long && api.repertoire[events[i - 1].episode].long));
  }
});

test('visor hand makes two real sweeps on the face, with both anchors sharing identical contacts', () => {
  for (const family of Object.keys(phases)) {
    const x = family === 'reading' ? 0 : 20;
    for (const [time, expected] of [[36, [71.5, 36.5]], [43, [69, 36.5]], [48, [71.5, 36.5]], [55, [69, 36.5]], [62, [71.5, 36.5]]]) {
      near(frontPoint([91, 55], 'visor', time, family), [expected[0] + x, expected[1]]);
    }
    for (let time = 36; time <= 62; time += .125) {
      const point = frontPoint([91, 55], 'visor', time, family);
      assert.ok(point[0] >= x + 69 - 1e-7 && point[0] <= x + 71.5 + 1e-7);
      assert.ok(Math.abs(point[1] - 36.5) < .1, 'contact cannot lift off the shallow horizontal visor sweep');
      assert.equal(at('wv-visor-head', time), 'rotate(0deg)');
      assert.equal(at('wv-visor-head-b', time), 'rotate(0deg)', 'variation must not move the visor away from contact');
      assert.ok(point[0] < x + 73 && point[1] > 29 && point[1] < 41, 'actual palm touches the existing visor bed');
    }
  }
  for (let time = 0; time <= 100; time += .125) {
    const point = frontPoint([91, 55], 'visor', time);
    assert.ok(point[0] >= 69 - 1e-7 && point[1] >= 36.4, 'the hand approaches from the lower right and never traverses the head/crown');
  }
});

test('the wave articulates the original wrist at a fixed forearm endpoint beside the head', () => {
  const vertices = [[90, 53], [93, 53], [93, 58], [90, 58], [88, 56]];
  for (let time = 38; time <= 66; time += .125) near(frontPoint([91, 55], 'wave', time, 'reading', true), [83, 37]);
  for (const [time, angle] of [[40, -18], [48, 16], [56, -14], [64, 10]]) assert.equal(at('wv-wave-wrist', time), `rotate(${angle}deg)`);
  for (let time = 0; time <= 100; time += .125) for (const point of vertices) {
    const transformed = frontPoint(point, 'wave', time, 'reading', true);
    for (const family of ['reading', 'checkpoint']) for (const suffix of ['', '-b']) {
      const headAngle = parseFloat(at(`wv-wave-head${suffix}`, time, 'transform', family).slice(7));
      const headLocal = rotate(transformed, -headAngle, [64, 46]);
      assert.ok(headLocal[0] > 79.1 || headLocal[1] > 48.1, `palm clears the padded head silhouette at ${time}%: ${headLocal}`);
    }
    assert.ok(transformed[1] > 30 && transformed[1] < 66, 'gesture stays beside the face and inside the compact stage');
  }
  assert.equal(at('wv-wave-smile', 48, 'opacity'), '.9');
  assert.equal(at('wv-wave-visor', 48, 'opacity'), '.15');
  assert.match(css, /\.wv-react-hand > :is\(\.wv-react-palm, \.wv-react-fingers\) \{ transform-origin: 91px 55px; \}/);
  assert.match(css, /\.wv-react-hand > :is\(\.wv-react-palm, \.wv-react-fingers\) \{ animation: wv-wave-wrist/);
});

test('all added tracks close exactly, use only transform/opacity, and leave feet and body planted', () => {
  assert.equal(tracks.size, 16);
  for (const [name, entries] of tracks) {
    assert.deepEqual(entries[0].properties, entries.at(-1).properties, `${name} closes without a snap`);
    for (const frame of entries) assert.ok(Object.keys(frame.properties).every(key => ['transform', 'opacity'].includes(key)));
  }
  const added = css.slice(css.indexOf('/* Empty-hand micro-reactions'));
  assert.doesNotMatch(added, /@keyframes wv-(?:visor|wave)-(?:travel|body|leg|knee|foot)|\.wv-reaction-actor.*animation|\.wv-react-(?:body|leg|knee|foot).*animation/);
  assert.doesNotMatch(added, /filter:|will-change:|animation[^;]*infinite|data-family=/);
  assert.doesNotMatch(source, /setTimeout|setInterval|requestAnimationFrame|Math\.random|Date\.now/);
  for (const episode of Object.keys(seeds)) for (const family of Object.keys(phases)) {
    for (const part of ['arm', 'hand', 'head', 'gaze']) assert.equal(at(`wv-${episode}-${part}`, 0, 'transform', family), at(`wv-${episode}-${part}`, 100, 'transform', family));
  }
});

test('every adapter parks/retrieves its own tool and updates no progress, status or SVG during either gesture', () => {
  for (const episode of Object.keys(seeds)) for (const family of Object.keys(phases)) {
    const { f, visual } = active(episode, family), { art, status, metric, details } = f.parts(visual);
    const before = { html: art.innerHTML, writes: art.htmlWrites, status: status.textContent, announcements: status.writes, metric: metric.textContent, details: details.textContent };
    for (let i = 0; i < 500; i++) visual.update(receipt({ phaseId: phases[family] }));
    assert.deepEqual({ html: art.innerHTML, writes: art.htmlWrites, status: status.textContent, announcements: status.writes, metric: metric.textContent, details: details.textContent }, before);
    assert.equal(metric.textContent, '27 / 27'); assert.equal(visual.element.dataset.state, 'running');
    assert.doesNotMatch(status.textContent, /sucesso|falhou|conclu[ií]do|100%/i);
    boundary(f, visual, 'episode');
    if (f.api.adapters[family].bridgeMs) { assert.equal(visual.element.dataset.adapter, 'resume'); assert.equal(visual.inspect().cooldown, 0); boundary(f, visual, 'resume'); }
    assert.equal(visual.element.dataset.episode, 'work'); assert.equal(visual.inspect().cooldown, 4);
    assert.deepEqual(plain(visual.inspect().history), [episode]);
    assert.equal(art.htmlWrites, before.writes); visual.destroy();
  }
});

test('pause/offscreen/document-hidden/reduced/panel gates retain every micro-reaction and reject synthetic cycles', () => {
  for (const episode of Object.keys(seeds)) for (const mode of ['manual', 'offscreen', 'hidden', 'reduced', 'panel']) {
    const { f, visual } = active(episode, 'composition');
    const pause = value => {
      if (mode === 'manual') f.parts(visual).control.emit('click');
      if (mode === 'offscreen') f.observers[0].deliver(!value);
      if (mode === 'hidden') { f.document.hidden = value; f.document.emit('visibilitychange'); }
      if (mode === 'reduced') { f.media.matches = value; f.media.emit('change'); }
      if (mode === 'panel') visual.setVisible(!value);
    };
    const before = plain(visual.inspect()); pause(true);
    assert.equal(visual.element.dataset.motion, 'static'); assert.equal(visual.element.dataset.animated, 'true');
    boundary(f, visual, 'work'); boundary(f, visual, 'episode', { elapsedTime: 0 });
    visual.update(receipt({ phaseId: phases.composition, completed: 3, total: 9 }));
    assert.deepEqual(plain(visual.inspect()), before);
    pause(false); assert.equal(visual.element.dataset.motion, 'running'); assert.deepEqual(plain(visual.inspect()), before);
    pause(true); boundary(f, visual, 'episode', { elapsedTime: f.api.repertoire[episode].durationMs / 1000 });
    assert.equal(visual.element.dataset.adapter, 'react');
    pause(false); assert.equal(visual.element.dataset.adapter, 'resume', 'one reached CSS endpoint is reconciled exactly once');
    boundary(f, visual, 'resume'); assert.equal(visual.element.dataset.episode, 'work');
    visual.destroy();
  }
});

test('terminal/cancelling/owner changes abort immediately and stale signals cannot revive a gesture', () => {
  for (const episode of Object.keys(seeds)) for (const state of ['paused', 'cancelling', 'cancelled', 'error', 'completed']) {
    const { f, visual } = active(episode, 'verification'), art = f.parts(visual).art;
    const oldTarget = art.children.find(node => node.className === 'wv-episode-boundary');
    visual.update(receipt({ phaseId: phases.verification, state }));
    assert.equal(visual.element.dataset.episode, 'work'); assert.equal(visual.element.dataset.motion, 'static');
    art.emit('animationend', { target: oldTarget, animationName: 'wv-episode-boundary', elapsedTime: f.api.repertoire[episode].durationMs / 1000 });
    assert.equal(visual.element.dataset.episode, 'work'); visual.destroy();
  }
  for (const episode of Object.keys(seeds)) {
    const { f, visual } = active(episode), art = f.parts(visual).art;
    const oldTarget = art.children.find(node => node.className === 'wv-episode-boundary');
    visual.update(receipt({ operationId: 'replacement-owner' }));
    assert.deepEqual(plain(visual.inspect().history), []); assert.equal(visual.element.dataset.episode, 'work');
    art.emit('animationend', { target: oldTarget, animationName: 'wv-episode-boundary' });
    assert.equal(visual.element.dataset.episode, 'work');
    visual.destroy(); boundary(f, visual); assert.equal(visual.element.dataset.motion, 'static');
  }
});
