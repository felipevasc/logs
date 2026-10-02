/* Contracts for the CI-only browser script. Passing Node is not a rendered pass. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { fullPreview, planValidation } from '../ci/validation-plan.mjs';

const source = readFileSync(new URL('../preview/test-waiting-boundaries.mjs', import.meta.url), 'utf8');
test('boundary preview covers exactly the five existing production families in parallel', () => {
  const definitions = source.slice(source.indexOf('const definitions = ['), source.indexOf('const output ='));
  assert.deepEqual([...definitions.matchAll(/family: '([^']+)'/g)].map(m => m[1]), ['reading', 'checkpoint', 'calculation', 'composition', 'verification']);
  assert.match(source, /definitions\.map\(\(definition, index\) =>/);
  assert.match(source, /WaitingVisuals\.mount\(host, receipt, \{ reactionSeed \}\)/);
  assert.match(source, /recibos sintéticos/);
  assert.match(source, /nativeEngineVerified: false, installedWebViewVerified: false/);
  assert.doesNotMatch(source, /<svg|innerHTML\s*=|addStyleTag\(|\.route\(|state\.(?:rows|loaded)\s*=/);
});
test('finished and active short gestures become entirely new loop timelines in the same SVG', () => {
  assert.match(source, /mountFixture\('finished'\)/); assert.match(source, /mountFixture\('interrupted'\)/);
  assert.match(source, /a\.playState === 'finished'/); assert.match(source, /a\.currentTime >= 700 && a\.playState === 'running'/);
  assert.match(source, /freshIdentities: entry\.loopTracks\.every\(animation => !before\.includes\(animation\)\)/);
  assert.match(source, /results\.loopStart\.every\(s => s\.stableSvg && s\.freshIdentities\)/);
  assert.match(source, /results\.activeLoopStart\.every\(s => s\.stableSvg && s\.freshIdentities\)/);
  assert.equal((source.match(/!s\.tracks\.some\(t => t\.name === 'wv-work-boundary'\)/g) || []).length, 2);
});
test('first trusted boundary measures every joint/prop and the separate controller clock', () => {
  assert.match(source, /querySelector\('\.wv-work'\)\.getAnimations\(\{ subtree: true \}\)/);
  assert.match(source, /querySelector\('\.wv-work-boundary'\)\.getAnimations\(\)/);
  assert.match(source, /root\.addEventListener\('animationiteration', event => \{/);
  assert.match(source, /\}, true\);/);
  assert.match(source, /assert\.equal\(sample\.isTrusted, true\)/);
  assert.match(source, /definition\.workCount \+ 1/);
  assert.match(source, /Math\.abs\(track\.currentTime - clock\.currentTime\) < 1/);
  assert.match(source, /Math\.abs\(track\.startTime - clock\.startTime\) < 1/);
  assert.match(source, /sample\.elapsedTime \* 1000, entry\.definition\.cycleMs/);
  assert.match(source, /sample\.tracks\.every\(t => Math\.abs\(t\.currentTime - entry\.definition\.cycleMs\) < 300\)/);
  assert.match(source, /assert\.equal\(entry\.afterBoundary\.cycle, 1\)/);
  assert.match(source, /node\.getScreenCTM\(\)/); assert.match(source, /sample\.endpointPose\.length, entry\.definition\.workCount/);
});
test('natural playback and product pause remain unmodified, with no video or seek dependency', () => {
  assert.doesNotMatch(source, /dispatchEvent|new AnimationEvent|playbackRate\s*=(?!=)|\.currentTime\s*=(?!=)|\.(?:pause|play)\s*\(|sampling\.seek|recordVideo|\.webm/);
  assert.match(source, /\.wv-motion-toggle'\)\.all\(\)\) await control\.click\(\)/);
  assert.match(source, /\.map\(animation => animation\.ready\)/);
  assert.match(source, /assert\.deepEqual\(frozen\.map\(s => s\.tracks\), paused\.map\(s => s\.tracks\)/);
  assert.match(source, /track\.currentTime > paused\[index\]\.tracks\[i\]\.currentTime/);
  assert.match(source, /assert\.equal\(track\.playbackRate, 1\)/);
});
test('phase replacement happens before the old boundary and observes a fresh complete next cycle', () => {
  assert.match(source, /fixture\.raf = requestAnimationFrame\(tick\)/);
  assert.match(source, /phaseChange\.remainingMs > 0 && phaseChange\.remainingMs <= 600/);
  assert.match(source, /oldClockDetached: !oldClock\.effect\.target\.isConnected/);
  assert.match(source, /hadNoBoundary: entry\.firstBoundary === null/);
  assert.match(source, /freshIdentities: entry\.loopTracks\.every\(a => !previousTracks\.includes\(a\)\)/);
  assert.match(source, /results\.phaseBoundaries = await collectBoundaries\(\)/);
  assert.match(source, /assert\.equal\(entry\.boundaryEvents\.length, 1\)/);
  assert.match(source, /e\.isTrusted && e\.isCurrentClock/);
  assert.match(source, /assert\.deepEqual\(results\.applicationAfter, results\.applicationBefore\)/);
});
test('boundary script is selected for full, shared waiting and direct script changes', () => {
  assert.ok(fullPreview.includes('test-waiting-boundaries.mjs'));
  for (const path of ['frontend/waiting-visuals.js', 'frontend/waiting-visuals.css', 'frontend/tasks.js', 'scripts/preview/test-waiting-boundaries.mjs']) {
    const plan = planValidation([path]);
    assert.equal(plan.native, false); assert.ok(plan.preview.includes('test-waiting-boundaries.mjs'));
  }
  assert.match(source, /waiting-boundaries-results\.json/);
  for (const shot of ['finished-gestures', 'first-review', 'new-phase-review']) assert.ok(source.includes(`waiting-boundaries-${shot}.png`));
});

test('same-family checkpoint phase change updates status near boundary without scheduling or restarting motion', () => {
  assert.match(source, /changeCheckpointPhaseNearBoundary/);
  assert.match(source, /fromPhase: 'metadata-checkpoint-sync'/);
  assert.match(source, /phaseId: 'metadata-checkpoint-write'/);
  assert.match(source, /same.remainingMs > 0 && same.remainingMs <= 600/);
  assert.match(source, /assert.notEqual\(same.afterStatus, same.beforeStatus\)/);
  assert.match(source, /assert.ok\(same.stableSvg && same.stableTracks && same.hadNoBoundary\)/);
  assert.match(source, /assert.equal\(same.after.cycle, 0\)/);
  assert.match(source, /assert.deepEqual\(same.history, \[\]/);
  assert.match(source, /assert.deepEqual\(same.after.tracks, same.before.tracks/);
  const run = source.slice(source.indexOf("phase = 'real pause and resume"));
  assert.ok(run.indexOf('changeCheckpointPhaseNearBoundary()') < run.indexOf('results.firstBoundaries = await collectBoundaries()'));
});
