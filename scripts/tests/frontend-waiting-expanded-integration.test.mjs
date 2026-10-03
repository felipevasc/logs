/* Source/SVG contracts only; runtime performance and rendered motion remain CI work. */
import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fullPreview, planValidation } from '../ci/validation-plan.mjs';
const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const hash = value => createHash('sha256').update(value).digest('hex');
const window = {};
vm.runInNewContext(source.replace('  function mount(', '  window.integrationArtwork = { scenes, reactionScenery };\n  function mount('), { window, Intl });

test('the complete seven-family six-reaction component has two bounded source assets and no external artwork', () => {
  const jsBytes = Buffer.byteLength(source), cssBytes = Buffer.byteLength(css);
  assert.ok(jsBytes < 46000 && cssBytes < 240000 && jsBytes + cssBytes < 280000);
  assert.deepEqual(Object.keys(window.WaitingVisuals.adapters).sort(), ['access', 'calculation', 'checkpoint', 'composition', 'reading', 'restoration', 'verification']);
  assert.deepEqual(Object.keys(window.WaitingVisuals.repertoire).sort(), ['coffee', 'manual', 'review', 'stretch', 'visor', 'wave']);
  assert.doesNotMatch(css, /url\(|@import|@font-face/i);
  assert.doesNotMatch(source, /<(?:image|foreignObject)|\b(?:href|src)=/);
});

test('the five established scenes and reaction scenery retain the exact shelf/coffee separation from 825aa8a', () => {
  const { scenes, reactionScenery } = window.integrationArtwork;
  const art = ['reading', 'checkpoint', 'calculation', 'composition', 'verification'].map(family => [family, scenes[family], reactionScenery(family)]);
  assert.equal(hash(JSON.stringify(art)), '9601f9d5848495f3bfd8823de070758930fa8b9839e5b473196952505da34b64');
  assert.ok(reactionScenery('restoration').includes('wv-manual-kit'));
  assert.equal(reactionScenery('access'), '');
});

test('all 183 base keyframes including manual shelf and short aliases are byte-identical', () => {
  const frames = [];
  for (const match of css.matchAll(/@keyframes ([\w-]+) \{/g)) {
    let end = match.index + match[0].length, depth = 1;
    while (depth && end < css.length) { if (css[end] === '{') depth++; if (css[end] === '}') depth--; end++; }
    assert.equal(depth, 0);
    if (match[1] !== 'wv-gesture-boundary' && !/^wv-(?:access|restore|visor|wave)-/.test(match[1])) frames.push([match[1], css.slice(match.index, end)]);
  }
  assert.equal(frames.length, 183);
  assert.equal(hash(JSON.stringify(frames)), 'efac8af440c4a8d6a2a71c5dd126f9d9291188bbe4978e157137b1e8a362ae73');
});

test('the expanded checkpoint retains every independent waiting regression gate', () => {
  const gates = ['test-waiting-visuals.mjs', 'test-waiting-reactions.mjs', 'test-waiting-verification.mjs', 'test-waiting-manual.mjs',
    'test-waiting-boundaries.mjs', 'test-waiting-reduced-motion.mjs', 'test-waiting-restore-access.mjs', 'test-waiting-micro-reactions.mjs',
    'test-case-report-waiting.mjs', 'test-timeline-export-waiting.mjs'];
  for (const name of gates) {
    assert.ok(fullPreview.includes(name));
    assert.ok(planValidation(['frontend/waiting-visuals.js']).preview.includes(name));
    assert.ok(planValidation(['frontend/waiting-visuals.css']).preview.includes(name));
  }
});
