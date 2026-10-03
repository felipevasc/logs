/* Static contracts only. CI must verify real CSSAnimation identity/currentTime. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { fullPreview, planValidation } from '../ci/validation-plan.mjs';

const source = readFileSync(new URL('../preview/test-waiting-reduced-motion.mjs', import.meta.url), 'utf8');
// Optional extracted base file keeps the before/after cascade failure reproducible.
const workspace = readFileSync(process.env.WAITING_REDUCED_WORKSPACE_CSS || new URL('../../frontend/workspace.css', import.meta.url), 'utf8');
const visual = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const controller = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
function mediaBlock(css, marker) {
  const start = css.indexOf(marker); assert.ok(start >= 0);
  const open = css.indexOf('{', start); let end = open + 1, depth = 1;
  while (depth && end < css.length) { if (css[end] === '{') depth++; if (css[end] === '}') depth--; end++; }
  assert.equal(depth, 0); return css.slice(open + 1, end - 1).replace(/\/\*[\s\S]*?\*\//g, '').trim();
}

test('global reduced policy excludes only already-animated WaitingVisuals art from animation cancellation', () => {
  const rules = [...mediaBlock(workspace, '@media(prefers-reduced-motion:reduce)').matchAll(/([^{}]+)\{([^{}]+)\}/g)]
    .map(([, selector, body]) => ({ selector: selector.trim(), body: body.trim() }));
  assert.deepEqual(rules, [
    { selector: '*', body: 'transition:none!important;scroll-behavior:auto!important' },
    { selector: '*:not(.waiting-visual[data-animated="true"] .wv-art *)', body: 'animation:none!important' },
  ]);
  const local = mediaBlock(visual, '@media (prefers-reduced-motion: reduce)');
  assert.match(local, /\.waiting-visual \.wv-art \* \{ animation-play-state: paused !important; \}/);
  assert.match(local, /\.waiting-visual \.wv-motion-toggle \{ display: none; \}/);
  assert.doesNotMatch(local, /animation:\s*none/);
});

test('existing controller latch leaves initial reduced scenes static and does not undo user pause', () => {
  assert.match(controller, /moving = current.canAnimate && motionEnabled && !reduced/);
  assert.match(controller, /if \(moving\) root.dataset.animated = 'true'/);
  assert.match(controller, /if \(!current.canAnimate\) root.dataset.animated = 'false'/);
  assert.match(controller, /control.hidden = finishing \|\| !current.canAnimate \|\| reduced/);
  assert.doesNotMatch(controller, /setTimeout|setInterval|requestAnimationFrame|animation\.currentTime/);
  assert.match(source, /reducedMotion: 'reduce'/);
  assert.match(source, /s\.animated === 'false'.*s\.animations\.length === 0.*!s\.controlVisible/);
  assert.match(source, /manualPaused = true/); assert.match(source, /reducedRound\('manual-pause'\)/);
  assert.match(source, /pre-existing user pause survives both preference changes/);
  assert.match(source, /sample.controlVisible, false/); assert.match(source, /sample.controlVisible, true/);
  assert.match(source, /sample.controlPressed, original.controlPressed/);
  assert.match(source, /sample.controlLabel, original.controlLabel/);
});

test('fixture seeds choose an exact review at the first natural boundary across all five families', () => {
  const window = {}; vm.runInNewContext(controller, { window, Intl }); const api = window.WaitingVisuals;
  const phases = { reading: 'metadata-scan', checkpoint: 'metadata-checkpoint-sync', calculation: 'command:aggregate_events',
    composition: 'command:case_report_render', verification: 'metadata-validate' };
  const definitions = source.slice(source.indexOf('const definitions = ['), source.indexOf('const output ='));
  assert.deepEqual([...definitions.matchAll(/family: '([^']+)'/g)].map(m => m[1]), Object.keys(phases));
  for (const [family, phaseId] of Object.entries(phases)) {
    const operationId = `reduced-${family}`, model = api.derive({ operationId, phaseId, state: 'running', elapsedMs: 60000 });
    let selected;
    for (let seed = 0; seed < 4096; seed++) if (api.createDirector(operationId, seed).boundary(model)?.episode === 'review') { selected = seed; break; }
    assert.notEqual(selected, undefined);
    const director = api.createDirector(operationId, selected);
    assert.equal(director.boundary({ ...model, elapsedMs: 60000 + api.adapters[family].cycleMs }).episode, 'review');
    assert.equal(director.inspect().cycle, 1);
  }
  assert.match(source, /WaitingVisuals\.mount\(host, receipt, \{ reactionSeed \}\)/);
  assert.match(source, /\.boundary\(model\)\?\.episode === 'review'/);
});

test('continuity covers work, both bridges and reaction with all real identities, clocks and matrices', () => {
  for (const round of ['work', 'prepare', 'react', 'resume']) assert.ok(source.includes(`reducedRound('${round}'`));
  assert.match(source, /new WeakMap\(\)/);
  assert.match(source, /art.getAnimations\(\{ subtree: true \}\)/);
  assert.match(source, /sample\.animations\.map\(a => \[a\.id, a\.name, a\.target, a\.durationMs, a\.playbackRate\]\)/);
  assert.match(source, /identity\(sample\), identity\(before\[i\]\)/);
  assert.match(source, /identity\(sample\), identity\(original\)/);
  assert.match(source, /frozen\.map\(times\), paused\.map\(times\)/);
  assert.match(source, /frozen\.map\(s => s\.poses\), paused\.map\(s => s\.poses\)/);
  assert.match(source, /node.getScreenCTM\(\)/);
  assert.match(source, /a\.currentTime >= last\.animations\[j\]\.currentTime/);
  assert.match(source, /a\.currentTime > last\.animations\[j\]\.currentTime/);
  assert.match(source, /CSS.supports\('selector\(\*:not/);
  assert.doesNotMatch(source, /dispatchEvent|new AnimationEvent|\.currentTime\s*=(?!=)|playbackRate\s*=(?!=)|\.(?:pause|play)\s*\(|sampling\.seek|recordVideo|\.webm|addStyleTag|\.route\(/);
});

test('ordinary app animation, transitions and scroll stay disabled while reduced', () => {
  assert.match(source, /probe.className = 'spin'/);
  assert.match(source, /transition: 'opacity 1s', scrollBehavior: 'smooth'/);
  assert.match(source, /animationName: 'none', transitionProperty: 'none', scrollBehavior: 'auto', animations: 0/);
  assert.match(source, /\(await probe\(\)\).animationName, 'li-spin'/);
  assert.match(source, /panel.append\(label, row, probe\)/);
});

test('terminal and destroy during reduced remove timelines permanently and preserve application data', () => {
  assert.match(source, /reducedRound\('react', \{ dispose: true \}\)/);
  assert.match(source, /terminal.view.update\(\{ ...terminal.receipt, state: 'error' \}\)/);
  assert.match(source, /removed.view.destroy\(\)/);
  assert.match(source, /round.disposal.terminal.animations.length, 0/);
  assert.match(source, /round.disposal.destroyed.connected, false/);
  assert.match(source, /round.disposalAfterResume.every\(s => s.animations.length === 0 && s.motion === 'static'\)/);
  assert.match(source, /assert.deepEqual\(results.applicationAfter, results.applicationBefore\)/);
  assert.match(source, /sample.status, original.status/); assert.match(source, /sample.metric, original.metric/);
  assert.match(source, /waiting-reduced-motion-results.json/);
  assert.match(source, /nativeEngineVerified: false, installedWebViewVerified: false/);
});

test('CI selects reduced continuity for complete scope, global CSS, waiting CSS and the direct script', () => {
  assert.ok(fullPreview.includes('test-waiting-reduced-motion.mjs'));
  for (const path of ['frontend/workspace.css', 'frontend/waiting-visuals.css', 'frontend/waiting-visuals.js', 'scripts/preview/test-waiting-reduced-motion.mjs']) {
    const plan = planValidation([path]); assert.equal(plan.native, false); assert.ok(plan.preview.includes('test-waiting-reduced-motion.mjs'));
  }
});
