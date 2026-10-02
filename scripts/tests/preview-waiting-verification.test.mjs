/* Static/pure contracts only. This suite never starts a server or browser and
   cannot certify rendered contacts, natural playback, screenshots or the WebM. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../preview/test-waiting-verification.mjs', import.meta.url), 'utf8');
const visualSource = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const progressSource = readFileSync(new URL('../../frontend/waiting-progress.js', import.meta.url), 'utf8');
const window = {};
vm.runInNewContext(visualSource, { window, Intl });
vm.runInNewContext(progressSource, { window });
const plain = value => JSON.parse(JSON.stringify(value));
const literal = name => plain(vm.runInNewContext(source.match(new RegExp(`const ${name} = (\\[[^;]+\\]);`))[1]));
const phases = ['metadata-validate', 'metadata-map-validate', 'canonical-verify', 'analytics-verify'];

test('verification browser fixture uses exactly the real phases and received progress labels', () => {
  assert.deepEqual(literal('phases'), phases);
  assert.deepEqual(literal('expectedLabels'), phases.map(phaseId => window.WaitingProgress.snapshot({ phaseId }).label));
  assert.match(source, /WaitingProgress\.snapshot\(\{ operationId, phaseId, completed: 4, total: 4, unit: 'registros', elapsedMs \}\)/);
  assert.match(source, /WaitingVisuals\.mount\(host, receipt, \{ reactionSeed \}\)/);
  assert.match(source, /Fixture de conferência: dados sintéticos/);
  assert.match(source, /não mede a operação nativa/);
  assert.doesNotMatch(source, /<svg|innerHTML\s*=|addStyleTag\(|\.route\(/);
});

test('first-boundary coffee seed is discoverable with the real director and verification adapter', () => {
  const operationId = 'verification-coffee-fixture';
  const receipt = window.WaitingProgress.snapshot({ operationId, phaseId: phases[0], completed: 4, total: 4, unit: 'registros', elapsedMs: 60000 });
  const model = window.WaitingVisuals.derive(receipt);
  let seed;
  for (let candidate = 0; candidate < 4096; candidate++) {
    if (window.WaitingVisuals.createDirector(operationId, candidate).boundary(model)?.episode === 'coffee') { seed = candidate; break; }
  }
  assert.notEqual(seed, undefined);
  const director = window.WaitingVisuals.createDirector(operationId, seed);
  assert.equal(director.boundary(model).episode, 'coffee');
  assert.equal(director.inspect().cycle, 1);
  assert.deepEqual(plain(window.WaitingVisuals.adapters.verification), {
    reactions: true, homeX: 20, head: 4, cycleMs: 9600, safePoint: 'lens-docked-empty-hand',
  });
  assert.equal(window.WaitingVisuals.repertoire.coffee.durationMs, 32000);
  assert.match(source, /WaitingVisuals\.createDirector\(operationId, seed\)\.boundary\(model\)\?\.episode === 'coffee'/);
  assert.doesNotMatch(source, /dispatchEvent|new AnimationEvent|playbackRate\s*=(?!=)/);
  assert.match(source, /mode === 'coffee' \? 60000 : mode === 'loop' \? 4000 : 0/);
});

test('natural cycles retain CSS governance and never rewind on cleanup', () => {
  assert.match(source, /addInitScript\(installMotionSampling\)/);
  assert.doesNotMatch(source, /\.(?:pause|play)\s*\(/);
  const capture = source.slice(source.indexOf('async function recordNaturalCycle('), source.indexOf('function assertWorkStates('));
  assert.match(capture, /await sampling\.seek\(0\)/);
  assert.match(capture, /await sampling\.release\(\)/);
  assert.match(capture, /finally \{\s*await sampling\.restore\(\{ restoreTime: false \}\)/);
  assert.match(capture, /animation\.playbackRate !== 1/);
  assert.match(source, /results\.short = await recordNaturalCycle\(2600\)/);
  assert.match(source, /results\.loop = await recordNaturalCycle\(9600\)/);
  assert.match(source, /results\.loop\.inspections\.every\(inspection => inspection\.history\.length === 0\)/);
});

test('the real 1x video is finalized before all rendered contact and pose diagnostics', () => {
  const run = source.slice(source.indexOf("  browser = await launchBrowser("));
  const short = run.indexOf('results.short = await recordNaturalCycle(2600)');
  const loop = run.indexOf('results.loop = await recordNaturalCycle(9600)');
  const coffee = run.indexOf("phase = 'natural shared coffee and return'");
  const returned = run.indexOf("mark('coffee-natural-return')");
  const finalized = run.indexOf('await finalizeNaturalVideo()');
  const diagnostic = run.indexOf('await openApplication(false)');
  const rig = run.indexOf('results.rig =');
  const firstGeometry = run.indexOf('getScreenCTM()');
  assert.ok(short >= 0 && short < loop && loop < coffee && coffee < returned && returned < finalized);
  assert.ok(finalized < diagnostic && diagnostic < rig && rig < firstGeometry);
  assert.match(source, /if \(recording\) \{/);
  assert.match(source, /recording \? \{ recordVideo:/);
  assert.match(source, /waiting-verification-real-preview\.webm/);
  assert.match(source, /waiting-verification-results\.json/);
  assert.match(source, /waiting-verification-video\.json/);
  assert.match(source, /naturalDurationMs >= 31900 && results\.coffee\.naturalDurationMs < 35000/);
  assert.match(source, /\[\['work', 'work'\], \['coffee', 'react'\], \['work', 'work'\]\]/);
});

test('contact/ownership/static-dock diagnostics cover both holds and restore complete pose clocks', () => {
  assert.match(source, /\[\.16, \.18, \.20, \.78, \.82, \.84\]/);
  assert.match(source, /\[\[91, 55\], \[102, 46\], \[95\.7, 46\], \[108\.3, 46\], \[102, 39\.7\]\]/);
  assert.match(source, /actorTransform, 'translate\(20 0\)'/);
  assert.match(source, /'\.wv-verify-station', '\.wv-verify-cradle', '\.wv-verify-document', '\.wv-verify-reference', '\.wv-verify-docked'/);
  assert.match(source, /sample\.heldOpacity \+ sample\.dockedOpacity, 1/);
  assert.match(source, /contactToleranceCssPx: \.15, staticMatrixTolerance: \.00001/);
  assert.ok((source.match(/await sampling\.restore\(\)/g) || []).length >= 2);
  assert.match(source, /await pose\.evaluate\(sample => sample\.restore\(\)\)/);
  assert.match(source, /await carriedPose\.evaluate\(sample => sample\.restore\(\)\)/);
});

test('pause sampling settles pending CSS tasks and freezes visible hand-owned steam', () => {
  assert.match(source, /pending: animation\.pending/);
  assert.match(source, /Promise\.all\(animations\.map\(animation => animation\.ready\)\)/);
  const pause = source.slice(source.indexOf('async function pauseCoffee('), source.indexOf('try {\n  browser ='));
  const requested = pause.indexOf('const requested =');
  const settle = pause.indexOf('await settlePausedArtwork()');
  const baseline = pause.indexOf('const settled =');
  const frozen = pause.indexOf('const frozen =');
  assert.ok(requested >= 0 && requested < settle && settle < baseline && baseline < frozen);
  assert.match(pause, /assert\.deepEqual\(frozen\.animations\.map/);
  assert.match(source, /steamOwnedByWrist: steam\.parentElement === wrist && wrist\.parentElement === held/);
  assert.match(source, /postSamplingPause\.settled\.cupOpacity, 1/);
  assert.match(source, /postSamplingPause\.settled\.steamOpacity > 0/);
  assert.match(source, /coffee-product-pause-before-coffee-poses/);
  assert.match(source, /coffee-product-pause-after-coffee-poses/);
});

test('coverage includes both themes, actual reduced-motion and synchronous error interruption', () => {
  for (const filename of ['inspection-dark', 'inspection-light', 'reduced-motion-light', 'reduced-motion-dark'])
    assert.ok(source.includes(`waiting-verification-${filename}.png`));
  assert.match(source, /page\.emulateMedia\(\{ reducedMotion: 'reduce' \}\)/);
  const interruption = source.slice(source.indexOf('results.errorInterruption = await page.evaluate('), source.indexOf('assert.equal(results.errorInterruption.before.episode'));
  assert.match(interruption, /view\.update\(WaitingProgress\.snapshot\(/);
  assert.doesNotMatch(interruption.slice(interruption.indexOf('\n') + 1), /await |setTimeout|requestAnimationFrame/);
  assert.match(source, /assert\.equal\(after\.motion, 'static'\)/);
  assert.match(source, /assert\.equal\(after\.kitchenPresent, false\)/);
  assert.match(source, /totalTimeoutMs: 110000, targetRuntimeMs: \[65000, 80000\]/);
  assert.match(source, /naturalHoldBudgetMs: 53800, diagnosticBoundaryBudgetMs: 9600/);
  assert.ok(2600 + 9600 + 9600 + 32000 + 9600 < 80000, 'natural and diagnostic holds leave setup/rendering headroom');
});
