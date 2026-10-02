import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const progressSource = readFileSync(new URL('../../frontend/waiting-progress.js', import.meta.url), 'utf8');
const nativeSource = readFileSync(new URL('../../src-tauri/src/metadata_checkpoint.rs', import.meta.url), 'utf8');
const window = {};
// Constants only: inspect the actual artwork without adding a production API.
vm.runInNewContext(source.replace('  function mount(', '  window.restoreFixture = { scenes, taskRobot, restoreRecord, reactionScenery };\n  function mount('), { window, Intl });
vm.runInNewContext(progressSource, { window });
const { scenes, taskRobot, restoreRecord, reactionScenery } = window.restoreFixture;
const api = window.WaitingVisuals;
const receipt = (overrides = {}) => ({ operationId: 'restore-17', phaseId: 'metadata-restore', state: 'running', ...overrides });
const plain = value => JSON.parse(JSON.stringify(value));

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
  const header = `@keyframes ${name} {`;
  const start = css.indexOf(header);
  assert.ok(start >= 0, `animation ${name} exists`);
  let end = start + header.length, depth = 1;
  while (depth && end < css.length) {
    if (css[end] === '{') depth++;
    if (css[end] === '}') depth--;
    end++;
  }
  const result = [];
  for (const match of css.slice(start + header.length, end - 1).matchAll(/([^{}]+)\{([^{}]+)\}/g)) {
    const properties = Object.fromEntries([...match[2].matchAll(/([\w-]+):\s*([^;]+);/g)].map(m => [m[1], m[2].trim()]));
    for (const selector of match[1].trim().split(',')) result.push({ time: parseFloat(selector), properties });
  }
  return result.sort((a, b) => a.time - b.time);
}
const tracks = new Map([...css.matchAll(/@keyframes (wv-(?:restore|access)-[\w-]+) \{/g)].map(m => [m[1], frames(m[1])]));
function at(name, time, property = 'transform') {
  const entries = tracks.get(name);
  const before = entries.filter(entry => entry.time <= time).at(-1);
  const after = entries.find(entry => entry.time >= time);
  assert.ok(before && after, `bounded ${name} at ${time}`);
  if (property === 'opacity') return Number(before.properties.opacity); // steps(1, end)
  if (before === after || before.properties[property] === after.properties[property]) return before.properties[property];
  const weight = (time - before.time) / (after.time - before.time);
  const numbers = [...after.properties[property].matchAll(/-?\d*\.?\d+/g)].map(m => Number(m[0]));
  let i = 0;
  return before.properties[property].replace(/-?\d*\.?\d+/g, value => String(+value + (numbers[i++] - +value) * weight));
}
function rotate(point, angle, origin) {
  const radians = angle * Math.PI / 180;
  const [x, y] = point.map((value, i) => value - origin[i]);
  return [origin[0] + x * Math.cos(radians) - y * Math.sin(radians), origin[1] + x * Math.sin(radians) + y * Math.cos(radians)];
}
function heldPoint(point, time) {
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]]]) point = rotate(point, parseFloat(at(`wv-restore-${part}`, time).slice(7)), origin);
  return [point[0] + 20, point[1]]; // unchanged body and fixed actor root
}
function parentsOf(art, target) {
  const stack = [];
  for (const match of art.matchAll(/<(\/?)([a-z]+)\b([^>]*)>/g)) {
    if (match[1]) { stack.pop(); continue; }
    const classes = /class="([^"]+)"/.exec(match[3])?.[1]?.split(' ') || [];
    if (classes.includes(target)) return stack.flat();
    if (!match[3].endsWith('/')) stack.push(classes);
  }
  assert.fail(`missing ${target}`);
}
function signal(f, visual, type = 'animationiteration', target = 'wv-work-boundary', name = 'wv-work-boundary') {
  const art = f.parts(visual).art;
  art.emit(type, { target: art.children.find(node => node.className === target), animationName: name });
}
function startReaction(f, visual) {
  f.observers[0].deliver(true);
  for (let i = 0; i < 100 && visual.element.dataset.episode === 'work'; i++) signal(f, visual);
  assert.notEqual(visual.element.dataset.episode, 'work');
  assert.equal(visual.element.dataset.adapter, 'react');
}

test('only exact native lock and restore IDs select the two new families', () => {
  for (const [phaseId, family, label] of [['metadata-lock', 'access', 'Aguardando acesso'], ['metadata-restore', 'restoration', 'Retomando metadados']]) {
    const mapping = [...source.slice(source.indexOf('const PHASES'), source.indexOf('const STATES')).matchAll(new RegExp(`'([^']+)': '${family}'`, 'g'))].map(m => m[1]);
    assert.deepEqual(mapping, [phaseId]);
    const model = api.derive(window.WaitingProgress.snapshot(receipt({ phaseId })));
    assert.equal(model.family, family); assert.equal(model.status, label);
    assert.match(nativeSource, new RegExp(`"${phaseId}"`));
  }
  for (const phaseId of ['access', 'restoration', 'metadata-resume', 'metadata-unlock', 'METADATA-LOCK', 'metadata-lock:waiting', 'command:metadata_restore']) {
    assert.equal(api.derive(receipt({ phaseId, label: 'Retomando metadados' })).family, 'neutral');
  }
});

test('lock age belongs to the operation and can never turn this adapter into a loop or reaction', () => {
  assert.deepEqual(plain(api.adapters.access), { reactions: false, gestureOnly: true, homeX: 20, head: 4, cycleMs: 2400, safePoint: 'empty-hands-closed-terminal' });
  const director = api.createDirector('restore-17');
  for (const elapsedMs of [null, 0, 3999, 4000, 5000, 45000, 120000, Number.MAX_SAFE_INTEGER]) {
    const model = api.derive(receipt({ phaseId: 'metadata-lock', elapsedMs }));
    assert.equal(model.motionMode, 'gesture'); assert.equal(model.canAnimate, true);
    for (let i = 0; i < 100; i++) assert.equal(director.boundary(model), null);
  }
  assert.equal(director.inspect().cycle, 0);
  assert.deepEqual(plain(director.inspect().history), []);
  assert.equal(reactionScenery('access'), '');
  assert.doesNotMatch(css, /data-family="access"[^{}]*\{[^}]*--wv-repeat:\s*infinite/);
  for (const line of css.split('\n').filter(line => /\.wv-access-.*animation:/.test(line))) assert.match(line, /2\.4s ease-in-out 1 both/);
  assert.doesNotMatch(css, /data-family="access"[^{}]*\.wv-work-boundary|\.wv-access-(?:station|slot)[^{}]*\{[^}]*animation/);
});

test('access receives repeated old-age receipts without restarting, queue numbering or opening its terminal', () => {
  let now = 0;
  const f = fixture({ now: () => now });
  const view = f.api.mount(f.host, receipt({ phaseId: 'metadata-lock', elapsedMs: 120000 }));
  f.observers[0].deliver(true);
  const { art } = f.parts(view);
  for (let i = 0; i < 20; i++) {
    now += 100;
    view.update(receipt({ phaseId: 'metadata-lock', elapsedMs: 120000 + i * 100 }));
    signal(f, view);
  }
  assert.equal(art.htmlWrites, 1); assert.equal(view.element.dataset.pace, 'gesture');
  assert.equal(view.element.dataset.episode, 'work'); assert.equal(view.inspect().cycle, 0);
  assert.doesNotMatch(art.innerHTML, /wv-kitchen|wv-reaction-actor|wv-cup|<text|lock-icon|padlock/);
  assert.ok(scenes.access.includes(taskRobot()));
  assert.match(scenes.access, /M120 43h38v31h-38Z/);
  assert.match(scenes.access, /wv-access-actor" transform="translate\(20 0\)"/);
  view.destroy();
});

test('restoration counter equality and its final receipt remain an unfinished opening', () => {
  for (const label of ['Restaurando metadados preservados', 'Metadados preservados restaurados']) {
    const model = api.derive(receipt({ completed: 8192, total: 8192, unit: 'registros', elapsedMs: 120000, label }));
    assert.equal(model.family, 'restoration'); assert.equal(model.state, 'running');
    assert.equal(model.canAnimate, true); assert.equal(model.partialCheckpoint, false);
    assert.equal(model.status, label); assert.equal(model.metric, '8.192 / 8.192 registros');
    assert.doesNotMatch(model.status, /concluído|sucesso|100%|aprovado/i);
  }
  const loop = nativeSource.slice(nativeSource.indexOf('while lines.len() < state.sealed_rows'));
  assert.ok(loop.indexOf('"metadata-restore"') < loop.indexOf('hash.clone().finalize()'), 'N/N may be emitted before final hash validation');
  for (const [phaseId, family] of [['metadata-scan', 'reading'], ['metadata-json-boundaries', 'reading'], ['metadata-columns', 'reading'], ['metadata-map-validate', 'verification']]) {
    assert.equal(api.derive(receipt({ phaseId })).family, family);
  }
});

test('restoration literally reuses taskRobot and the record belongs to its articulated hand', () => {
  assert.ok(scenes.restoration.includes(taskRobot(`<g class="wv-restore-held">${restoreRecord}</g>`)));
  assert.match(scenes.restoration, /wv-restore-actor" transform="translate\(20 0\)"/);
  const parents = parentsOf(scenes.restoration, 'wv-restore-held');
  assert.equal(parents.at(-1), 'wv-task-hand');
  assert.ok(parents.includes('wv-task-arm') && parents.includes('wv-task-body'));
  assert.ok(parentsOf(scenes.restoration, 'wv-restore-seated').includes('wv-restore-station'));
  assert.equal(scenes.restoration.split(restoreRecord).length - 1, 2);
  assert.match(restoreRecord, /v-5h-3v-4h3Z/, 'tab surrounds finger (91,55)');
  assert.match(restoreRecord, /wv-restore-marker" cx="99" cy="55"/, 'both copies have the same existing dot');
  for (const family of ['access', 'restoration']) assert.doesNotMatch(scenes[family], /wv-archive-drawer|wv-verify-lens|wv-compose-press|wv-kitchen|<text|\bid=|(?:href|src)=/);
  for (const prop of ['held', 'seated']) for (const frame of tracks.get(`wv-restore-${prop}`)) assert.deepEqual(Object.keys(frame.properties), ['opacity']);
});

test('restoration transfer matrices coincide for the entire record at both contact holds', () => {
  const matrix = /wv-restore-seated" transform="matrix\(([^)]+)\)"/.exec(scenes.restoration)[1].split(' ').map(Number);
  assert.deepEqual(matrix, [1, 0, 0, 1, 20, 0]);
  for (const time of [14, 17, 20, 78, 81, 84]) for (const point of [[91, 55], [94, 50], [111, 50], [111, 62], [94, 62], [99, 55]]) {
    const actual = heldPoint(point, time);
    assert.ok(Math.hypot(actual[0] - point[0] - 20, actual[1] - point[1]) < 1e-9, `zero-jump handoff at ${time}%: ${point}`);
  }
  assert.deepEqual(heldPoint([91, 55], 17), [111, 55], 'the tab fits the unextended rig');
  assert.match(scenes.restoration, /M109 58v4h24v-4/, 'support ledge is exactly at record bottom y62');
  for (let time = 0; time <= 100; time += .125) assert.equal(at('wv-restore-held', time, 'opacity') + at('wv-restore-seated', time, 'opacity'), 1);
  for (const time of [0, 16.999, 81, 100]) assert.equal(at('wv-restore-held', time, 'opacity'), 0);
  for (const time of [17, 20, 48, 64, 80.999]) assert.equal(at('wv-restore-held', time, 'opacity'), 1);
  for (const prop of ['held', 'seated']) assert.match(css, new RegExp(`\\.wv-restore-${prop} \\{ animation: wv-restore-${prop} var\\(--wv-restore-cycle\\) steps\\(1, end\\)`));
});

test('the held marker comes toward the visor, changes position with the gaze and stays in bounds', () => {
  const source = heldPoint([99, 55], 17), first = heldPoint([99, 55], 42), second = heldPoint([99, 55], 58);
  assert.ok(heldPoint([91, 55], 30)[0] < 111, 'record first withdraws toward its holder');
  assert.ok(Math.hypot(first[0] - 84, first[1] - 35) < Math.hypot(source[0] - 84, source[1] - 35) - 15);
  assert.ok(second[1] - first[1] > 1, 'second glance follows the marker lower');
  assert.notEqual(at('wv-restore-gaze', 42), at('wv-restore-gaze', 58));
  for (let time = 17; time <= 81; time += .125) for (const point of [[90, 52], [94, 49], [112, 49], [112, 63], [94, 63], [90, 58]]) {
    const actual = heldPoint(point, time);
    assert.ok(actual[0] > 0 && actual[0] < 164 && actual[1] > 0 && actual[1] < 96, `record bounds at ${time}%: ${actual}`);
  }
  assert.doesNotMatch(css, /\.wv-restore-(?:station|support|marker)\s*\{[^}]*animation/);
  assert.doesNotMatch(css, /\.wv-restore-actor\s*\{[^}]*animation/);
});

test('access foot adjustment keeps the back support planted and returns to the shared home rig', () => {
  let back = rotate([61, 88], 76.595, [61, 74.5]);
  back = rotate(back, -19.008, [61, 61]);
  assert.ok(Math.abs(back[1] - 81) < .001);
  for (let time = 0; time <= 100; time += .25) {
    const knee = parseFloat(at('wv-access-knee', time).slice(7)), foot = parseFloat(at('wv-access-foot', time).slice(7));
    const front = rotate(rotate([68, 88], knee, [68, 74.5]), -56.043, [68, 61]);
    assert.ok(front[1] <= 81.001, 'swing foot never sinks into the floor');
    assert.ok(Math.abs(-56.043 + knee + foot) < .000001, 'boot stays level');
  }
  assert.doesNotMatch(css, /\.wv-access-actor \.wv-task-(?:leg-back|knee-back|foot-back|body|arm|hand)\s*\{[^}]*animation/);
});

test('short and long restore paths end empty-handed at x20/body0/head4/arm40/hand-85', () => {
  for (const [name, entries] of tracks) {
    assert.deepEqual(entries[0].properties, entries.at(-1).properties, `${name} returns home`);
    for (const frame of entries) assert.ok(Object.keys(frame.properties).every(key => ['transform', 'opacity'].includes(key)));
  }
  for (const time of [0, 100]) {
    assert.equal(at('wv-restore-head', time), 'rotate(4deg)');
    assert.equal(at('wv-restore-arm', time), 'rotate(40deg)');
    assert.equal(at('wv-restore-hand', time), 'rotate(-85deg)');
    assert.equal(at('wv-restore-gaze', time), 'translate(1px, 0)');
    assert.equal(at('wv-restore-held', time, 'opacity'), 0);
  }
  assert.match(css, /--wv-restore-cycle: 2\.4s/);
  assert.match(css, /data-family="restoration"\]\[data-pace="loop"\] \{ --wv-restore-cycle: 7\.2s/);
  assert.doesNotMatch(css, /data-animated="true"\] \.wv-restore-(?:held|seated) \{ animation:/, 'short gesture leaves record seated');
  assert.doesNotMatch(css, /\.wv-restore-actor \.wv-task-body\s*\{[^}]*animation/);
  assert.match(css, /\.wv-restore-held \{ opacity: 0; \}/);
  assert.match(css, /\.wv-restore-seated \{ opacity: 1; \}/);
});

test('restoration shares existing reactions only at the validated empty-hand boundary', () => {
  assert.deepEqual(plain(api.adapters.restoration), { reactions: true, homeX: 20, head: 4, cycleMs: 7200, safePoint: 'record-reseated-empty-hand' });
  const f = fixture(), view = f.api.mount(f.host, receipt({ elapsedMs: 120000 }));
  startReaction(f, view);
  assert.match(css, /data-family="restoration"\]\[data-pace="loop"\] \.wv-work-boundary \{ animation: wv-work-boundary var\(--wv-restore-cycle\)/);
  const coffee = css.split('\n').filter(line => line.includes('[data-family="restoration"]') && line.includes('[data-episode="coffee"]'));
  assert.equal(coffee.length, 7);
  assert.ok(coffee.every(line => /wv-coffee-(?:travel-checkpoint|checkpoint-)/.test(line)));
  assert.ok(coffee.every(line => line.includes(':is([data-episode="coffee"], [data-episode="manual"])')), 'manual uses the same complete x20 walk');
  assert.doesNotMatch(css, /@keyframes wv-coffee-restor/);
  assert.match(css, /data-family="restoration"\]:not\(\[data-episode="work"\]\) \.wv-work > \.wv-restore-actor \{ visibility: hidden/);
  signal(f, view, 'animationend', 'wv-episode-boundary', 'wv-episode-boundary');
  assert.equal(view.element.dataset.episode, 'work'); assert.equal(view.element.dataset.adapter, 'work');
  view.destroy();
});

test('receipt equality never interrupts a restore episode, but a terminal state or new owner immediately does', () => {
  const f = fixture(), view = f.api.mount(f.host, receipt({ elapsedMs: 120000 }));
  startReaction(f, view);
  const { art, metric } = f.parts(view), episode = view.element.dataset.episode, writes = art.htmlWrites;
  view.update(receipt({ elapsedMs: 121000, completed: 8192, total: 8192 }));
  assert.equal(view.element.dataset.episode, episode); assert.equal(metric.textContent, '8.192 / 8.192');
  assert.equal(art.htmlWrites, writes);
  const stale = art.children.find(node => node.className === 'wv-episode-boundary');
  view.update(receipt({ operationId: 'another-owner', elapsedMs: 120000 }));
  assert.equal(view.element.dataset.episode, 'work'); assert.equal(art.htmlWrites, writes + 1);
  art.emit('animationend', { target: stale, animationName: 'wv-episode-boundary' });
  assert.equal(view.element.dataset.episode, 'work');
  for (const state of ['paused', 'queued', 'cancelling', 'cancelled', 'error', 'completed', 'idle', 'unknown']) {
    view.update(receipt({ state, elapsedMs: 120000 }));
    assert.equal(view.element.dataset.family, 'neutral'); assert.equal(view.element.dataset.motion, 'static');
    assert.equal(view.element.dataset.animated, 'false');
    assert.doesNotMatch(art.innerHTML, /wv-restore-station|wv-access-station|wv-kitchen/);
  }
  view.destroy();
});

test('new families preserve pause, offscreen, hidden-tab and reduced-motion safeguards without new schedulers', () => {
  for (const phaseId of ['metadata-lock', 'metadata-restore']) {
    const f = fixture(), view = f.api.mount(f.host, receipt({ phaseId, elapsedMs: 120000 }));
    f.observers[0].deliver(true);
    const before = plain(view.inspect()), writes = f.parts(view).art.htmlWrites;
    const pairs = [
      [() => view.setMotionEnabled(false), () => view.setMotionEnabled(true)],
      [() => f.observers[0].deliver(false), () => f.observers[0].deliver(true)],
      [() => view.setVisible(false), () => view.setVisible(true)],
      [() => { f.media.matches = true; f.media.emit('change'); }, () => { f.media.matches = false; f.media.emit('change'); }],
      [() => { f.document.hidden = true; f.document.emit('visibilitychange'); }, () => { f.document.hidden = false; f.document.emit('visibilitychange'); }]
    ];
    for (const [pause, resume] of pairs) {
      pause(); assert.equal(view.element.dataset.motion, 'static');
      signal(f, view); assert.deepEqual(plain(view.inspect()), before);
      resume(); assert.equal(view.element.dataset.motion, 'running');
      assert.equal(f.parts(view).art.htmlWrites, writes);
    }
    view.destroy();
    const reduced = fixture({ reduced: true }), staticView = reduced.api.mount(reduced.host, receipt({ phaseId, elapsedMs: 120000 }));
    reduced.observers[0].deliver(true);
    assert.equal(staticView.element.dataset.animated, 'false'); assert.equal(staticView.element.dataset.motion, 'static');
    staticView.destroy();
  }
  assert.doesNotMatch(source, /\b(?:setTimeout|setInterval|requestAnimationFrame|fetch|XMLHttpRequest)\s*\(/);
  assert.doesNotMatch(css, /\b(?:filter|backdrop-filter|will-change)\s*:/);
  assert.match(css, /prefers-reduced-motion: reduce/);
  const palette = css.slice(css.indexOf('/* Journal access'));
  assert.match(palette, /--wv-accent: #d7b579/); assert.match(palette, /--wv-accent: #79501c/);
  assert.match(palette, /inline-size: min\(100%, 240px\); block-size: 120px;/);
  assert.match(palette, /@media \(forced-colors: active\)[\s\S]+--wv-accent: CanvasText/);
});
