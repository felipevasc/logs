import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const progressSource = readFileSync(new URL('../../frontend/waiting-progress.js', import.meta.url), 'utf8');
const window = {};
// Inspect constant artwork without broadening the production API or needing a browser.
vm.runInNewContext(source.replace('  function mount(', '  window.verificationFixture = { scenes, taskRobot, verifyLens };\n  function mount('), { window, Intl });
vm.runInNewContext(progressSource, { window });
const { scenes, taskRobot, verifyLens } = window.verificationFixture;
const api = window.WaitingVisuals;
const phases = ['metadata-validate', 'metadata-map-validate', 'canonical-verify', 'analytics-verify'];
const receipt = (phaseId = phases[0], overrides = {}) => ({ operationId: 'verification-17', phaseId, state: 'running', ...overrides });
const plain = value => JSON.parse(JSON.stringify(value));

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
const tracks = new Map([...css.matchAll(/@keyframes (wv-verify-[\w-]+) \{/g)].map(m => [m[1], frames(m[1])]));
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
function heldPoint(point, time, prefix = 'wv-verify') {
  for (const [part, origin] of [['hand', [82, 58]], ['arm', [72, 52]], ['body', [65, 64]]]) {
    const value = at(`${prefix}-${part}`, time);
    point = rotate(point, parseFloat(value.slice('rotate('.length)), origin);
  }
  return [point[0] + 20, point[1]];
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

test('verification adapts exactly four existing protocol phases and keeps their received labels', () => {
  const mapping = [...source.slice(source.indexOf('const PHASES'), source.indexOf('const STATES')).matchAll(/'([^']+)': 'verification'/g)].map(m => m[1]);
  assert.deepEqual(mapping.sort(), [...phases].sort());
  for (const phaseId of phases) {
    const progress = window.WaitingProgress.snapshot({ ...receipt(phaseId), completed: 4, total: 4, unit: 'registros' });
    const model = api.derive(progress);
    assert.equal(model.family, 'verification');
    assert.equal(model.canAnimate, true);
    assert.match(model.status, /^Conferindo /);
    assert.equal(model.state, 'running', 'equal counters never fabricate a successful check');
    assert.doesNotMatch(model.status, /sucesso|aprovad|falhou|100%/i);
  }
  for (const phaseId of ['metadata-scan', 'metadata-columns', 'analytics-time-index', 'pivot-values']) assert.equal(api.derive(receipt(phaseId)).family, 'reading');
  for (const phaseId of ['analytics-select', 'analytics-sql']) assert.equal(api.derive(receipt(phaseId)).family, 'calculation');
  for (const phaseId of ['verification', 'metadata-verify', 'command:verification', 'ANALYTICS-VERIFY']) assert.equal(api.derive(receipt(phaseId)).family, 'neutral');
});

test('the new actor literally reuses the articulated protagonist and its existing pivots', () => {
  assert.ok(scenes.verification.includes(taskRobot(`<g class="wv-verify-held">${verifyLens}</g>`)));
  assert.match(scenes.verification, /class="wv-verify-actor" transform="translate\(20 0\)"/);
  for (const [part, x, y] of [['body', 65, 64], ['head', 64, 46], ['arm', 72, 52], ['hand', 82, 58]]) {
    assert.match(css, new RegExp(`\\.wv-task-${part} \\{ transform-origin: ${x}px ${y}px;`));
  }
  for (const side of ['front', 'back']) for (const joint of ['leg', 'knee', 'foot']) assert.ok(scenes.verification.includes(`wv-task-${joint}-${side}`));
  assert.doesNotMatch(scenes.verification, /wv-(?:reader|react|kitchen|cup)-|<text|\bid=|(?:href|src)=/);
});

test('the lens is owned by the hand or the fixed cradle, never an independent moving prop', () => {
  const heldParents = parentsOf(scenes.verification, 'wv-verify-held');
  assert.equal(heldParents.at(-1), 'wv-task-hand');
  assert.ok(heldParents.includes('wv-task-arm'));
  assert.ok(heldParents.includes('wv-task-body'));
  assert.ok(parentsOf(scenes.verification, 'wv-verify-docked').includes('wv-verify-station'));
  assert.match(scenes.verification, /<g class="wv-verify-held">/);
  assert.equal(scenes.verification.split(verifyLens).length - 1, 2, 'one identical carried and one identical parked tool');
  assert.match(verifyLens, /d="m91 55 /, 'handle begins inside the existing palm');
  for (const name of ['wv-verify-held', 'wv-verify-docked']) for (const frame of tracks.get(name)) assert.deepEqual(Object.keys(frame.properties), ['opacity']);
  assert.doesNotMatch(css, /\.wv-verify-(?:station|cradle|document|reference)\s*\{[^}]*animation/);
});

test('pickup and return align the whole tool exactly, throughout both contact holds', () => {
  const matrix = /class="wv-verify-docked" transform="matrix\(([^)]+)\)"/.exec(scenes.verification)[1].split(' ').map(Number);
  const [a, b, c, d, e, f] = matrix;
  for (const time of [16, 18, 20, 78, 82, 84]) for (const point of [[91, 55], [102, 46], [95.7, 46], [108.3, 46], [102, 39.7]]) {
    const actual = heldPoint(point, time), parked = [a * point[0] + c * point[1] + e, b * point[0] + d * point[1] + f];
    assert.ok(Math.hypot(actual[0] - parked[0], actual[1] - parked[1]) < 1e-7, `zero-jump handoff at ${time}% for ${point}`);
  }
  const center = heldPoint([102, 46], 18);
  assert.ok(Math.abs(center[1] + 6.3 - 69) < .8, 'lens rim rests on the cradle ledge');
  assert.ok(center[0] > 113 && center[0] < 127, 'lens fits within the physical cradle');
});

test('tool ownership changes atomically at contact, with exactly one visible copy', () => {
  for (let time = 0; time <= 100; time += .125) assert.equal(at('wv-verify-held', time, 'opacity') + at('wv-verify-docked', time, 'opacity'), 1);
  for (const time of [0, 17.999, 82, 100]) assert.equal(at('wv-verify-held', time, 'opacity'), 0);
  for (const time of [18, 20, 40, 68, 81.999]) assert.equal(at('wv-verify-held', time, 'opacity'), 1);
  for (const prop of ['held', 'docked']) assert.match(css, new RegExp(`\\.wv-verify-${prop} \\{ animation: wv-verify-${prop} var\\(--wv-verify-cycle\\) steps\\(1, end\\)`));
});

test('inspection moves the tool between rows while the comparison reference stays fixed', () => {
  const top = heldPoint([102, 46], 34), middle = heldPoint([102, 46], 48), lower = heldPoint([102, 46], 62);
  for (const point of [top, middle, lower]) {
    assert.ok(point[0] >= 120 && point[0] <= 134, 'lens remains over the inspected sheet');
    assert.ok(point[1] >= 33 && point[1] <= 58, 'lens remains within the physical sheet');
  }
  assert.ok(middle[1] - top[1] > 3);
  assert.ok(lower[1] - middle[1] > 5, 'inspection covers clearly separated rows');
  assert.notEqual(at('wv-verify-head', 34), at('wv-verify-head', 48), 'head turns toward the reference');
  assert.notEqual(at('wv-verify-gaze', 34), at('wv-verify-gaze', 48));
});

test('the tool stays within the compact scene and leaves the coffee hatch region clear', () => {
  // All tracks use the same ease-in-out; linear interpolation visits the same poses.
  // Sampling the rigid circle perimeter also covers the glint and lens stroke.
  for (let time = 18; time < 82; time += .125) {
    for (let angle = 0; angle < 360; angle += 15) {
      const theta = angle * Math.PI / 180;
      const point = heldPoint([102 + 7.2 * Math.cos(theta), 46 + 7.2 * Math.sin(theta)], time);
      assert.ok(point[0] > 0 && point[0] < 164 && point[1] > 0 && point[1] < 96, `tool bounds at ${time}%: ${point}`);
    }
  }
  assert.match(scenes.verification, /M106 71h56/);
  assert.doesNotMatch(css, /\.wv-verify-actor\s*\{[^}]*animation/);
  assert.doesNotMatch(scenes.verification, /wv-kitchen|wv-reaction-actor/);
});

test('short and long motion close at the same empty-handed checkpoint-compatible pose', () => {
  assert.match(css, /--wv-verify-cycle: 2\.6s/);
  assert.match(css, /data-family="verification"\]\[data-pace="loop"\] \{ --wv-verify-cycle: 9\.6s/);
  for (const [name, entries] of tracks) {
    assert.deepEqual(entries[0].properties, entries.at(-1).properties, `${name} starts and ends identically`);
    for (const frame of entries) assert.ok(Object.keys(frame.properties).every(key => ['transform', 'opacity'].includes(key)));
  }
  for (const prefix of ['wv-verify', 'wv-verify-glance']) for (const time of [0, 100]) {
    for (const [part, angle] of [['body', 0], ['head', 4], ['arm', 40], ['hand', -85]]) assert.equal(at(`${prefix}-${part}`, time), `rotate(${angle}deg)`);
    assert.equal(at(`${prefix}-gaze`, time), 'translate(1px, 0)');
  }
  assert.match(css, /\.wv-verify-held \{ opacity: 0; \}/);
  assert.match(css, /\.wv-verify-docked \{ opacity: 1; \}/);
  assert.doesNotMatch(css, /data-animated="true"\] \.wv-verify-(?:held|docked) \{ animation:/, 'short gesture keeps the lens parked');
});

test('verification shares the checkpoint reaction anchor only after the empty-handed work boundary', () => {
  assert.deepEqual(plain(api.adapters.verification), { reactions: true, homeX: 20, head: 4, cycleMs: 9600, safePoint: 'lens-docked-empty-hand' });
  const director = api.createDirector('verification-17');
  let selected = null;
  for (let i = 0; i < 100 && !selected; i++) selected = director.boundary(api.derive(receipt(phases[i % phases.length], { elapsedMs: 120000 })));
  assert.ok(selected && ['coffee', 'review', 'stretch'].includes(selected.episode));
  assert.match(css, /data-family="verification"\]\[data-pace="loop"\] \.wv-work-boundary \{ animation: wv-work-boundary var\(--wv-verify-cycle\)/);
  const coffee = css.split('\n').filter(line => line.includes('[data-family="verification"]') && line.includes('[data-episode="coffee"]'));
  assert.ok(coffee.length >= 7 && coffee.every(line => /wv-coffee-(?:travel-checkpoint|checkpoint-)/.test(line)), 'reuse the existing checkpoint walk rather than duplicate tracks');
  assert.match(css, /\.wv-compose-actor, \.wv-verify-actor\) \{ visibility: hidden/);
  assert.doesNotMatch(css, /@keyframes wv-coffee-verification/);
});

test('verification keeps the existing quiet, readable light/dark and static fallbacks', () => {
  const block = css.slice(css.indexOf('/* Verification:'), css.indexOf('/* Shared reactions:'));
  assert.match(block, /--wv-accent: #d7b579; --wv-task-shell: #242933; --wv-task-paper: #141b25;/);
  assert.match(block, /html\[data-theme="light"\][\s\S]+--wv-accent: #79501c;/);
  assert.match(block, /inline-size: min\(100%, 240px\); block-size: 120px;/);
  assert.match(block, /@media \(forced-colors: active\)[\s\S]+--wv-accent: CanvasText;/);
  assert.match(css, /data-motion="static"\] \.wv-art \* \{ animation-play-state: paused !important;/);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\)[\s\S]+animation-play-state: paused !important;/);
  for (const state of ['queued', 'paused', 'cancelling', 'cancelled', 'error', 'completed']) {
    const model = api.derive(receipt(phases[0], { state, elapsedMs: 30000 }));
    assert.equal(model.family, 'neutral'); assert.equal(model.canAnimate, false);
  }
  assert.doesNotMatch(source, /\b(?:setTimeout|setInterval|requestAnimationFrame|fetch|XMLHttpRequest)\s*\(/);
});
