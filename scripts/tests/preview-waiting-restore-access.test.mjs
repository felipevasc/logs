/* Static/pure only: does not certify browser rendering, a WebM or native latency. */
import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { fullPreview, planValidation } from '../ci/validation-plan.mjs';
import { artifactFor } from '../preview/record-evidence.mjs';
const source = readFileSync(new URL('../preview/test-waiting-restore-access.mjs', import.meta.url), 'utf8');
const visual = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const progress = readFileSync(new URL('../../frontend/waiting-progress.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const native = readFileSync(new URL('../../src-tauri/src/metadata_checkpoint.rs', import.meta.url), 'utf8');
const workflow = readFileSync(new URL('../../.github/workflows/checks.yml', import.meta.url), 'utf8');
const window = {}; vm.runInNewContext(visual, { window, Intl }); vm.runInNewContext(progress, { window });
const api = window.WaitingVisuals;
function choose(episode) {
  const operationId = 'restore-access-restoration-fixture';
  const model = api.derive(window.WaitingProgress.snapshot({ operationId, phaseId: 'metadata-restore', elapsedMs: 60000 }));
  for (let seed = 0; seed < 4096; seed++) {
    if (api.createDirector(operationId, seed).boundary(model)?.episode === episode) return { seed, model, operationId };
  }
  assert.fail(`No first-boundary ${episode} seed`);
}

test('restore/access preview uses the exact native IDs and unmodified progress labels', () => {
  assert.match(source, /const families = \['access', 'restoration'\]/);
  assert.match(source, /const phases = \['metadata-lock', 'metadata-restore'\]/);
  for (const [phaseId, family, label] of [['metadata-lock', 'access', 'Aguardando acesso'], ['metadata-restore', 'restoration', 'Retomando metadados']]) {
    const model = api.derive(window.WaitingProgress.snapshot({ operationId: 'fixture', phaseId, completed: 8, total: 8, unit: 'registros' }));
    assert.equal(model.family, family); assert.equal(model.status, label); assert.equal(model.state, 'running');
    assert.equal(model.metric, '8 / 8 registros');
  }
  assert.match(source, /fixture com dados sintéticos/); assert.match(source, /não mede a operação nativa/);
  assert.match(source, /WaitingVisuals\.mount\(host, receipt, \{ reactionSeed \}\)/);
  assert.doesNotMatch(source, /<svg|innerHTML\s*=|addStyleTag\(|\.route\(|state\.(?:rows|loaded)\s*=/);
});

test('existing native lock retries at 100ms with a five-second deadline, never a visual long loop', () => {
  const block = native.slice(native.indexOf('"metadata-lock"'), native.indexOf('"metadata-lock"') + 1100);
  assert.match(block, /waiting\.elapsed\(\) >= Duration::from_secs\(5\)/);
  assert.match(block, /std::thread::sleep\(Duration::from_millis\(100\)\)/);
  for (const elapsedMs of [0, 4000, 5000, 120000, Number.MAX_SAFE_INTEGER]) {
    const model = api.derive({ operationId: 'fixture', phaseId: 'metadata-lock', state: 'running', elapsedMs });
    assert.equal(model.motionMode, 'gesture'); assert.equal(api.createDirector('fixture').boundary(model), null);
  }
  assert.equal(api.adapters.access.cycleMs, 2400); assert.equal(api.adapters.access.reactions, false);
  assert.match(source, /results\.long\[0\]\.animations, results\.short\[0\]\.animations/);
  assert.match(source, /fixture\.receipts\[family\].*elapsedMs: family === 'access' \? 120000 : 4000/);
});

test('restoration promotes without receipts and access defers only the latest known family', () => {
  assert.match(source, /results\.shortEndpoints\.every\(s => s\.isTrusted && s\.elapsedTime === 2\.4/);
  assert.match(source, /results\.short\[1\]\.pace, 'loop'/);
  assert.match(source, /event\.animationName !== 'wv-gesture-boundary'/);
  assert.match(source, /\['metadata-restore', 'metadata-scan', 'metadata-checkpoint-sync'\]/);
  assert.match(source, /pendingAccessReceipt\.family, 'access'/);
  assert.match(source, /pendingAccessReceipt\.stableSvg && results\.pendingAccessReceipt\.stableTracks/);
  assert.match(source, /pendingAccessReceipt\.afterTimes, results\.pendingAccessReceipt\.beforeTimes/);
  assert.match(source, /pendingAccessApplied\.endpoint, \{ isTrusted: true, elapsedTime: 2\.4, family: 'access'/);
  assert.match(source, /pendingAccessApplied\.family, 'checkpoint'/);
  assert.match(source, /pendingAccessApplied\.replacedSvg && results\.pendingAccessApplied\.freshTracks/);
});

test('first eligible natural visor and diagnostic manual have stable real director seeds', () => {
  for (const episode of ['visor', 'manual']) {
    const { seed, model, operationId } = choose(episode);
    assert.equal(choose(episode).seed, seed);
    const director = api.createDirector(operationId, seed);
    assert.equal(director.boundary({ ...model, elapsedMs: 10000 }), null, 'young work cycle consumes no random draw');
    assert.equal(director.boundary({ ...model, elapsedMs: 60000 + 7200 }).episode, episode);
  }
  for (const episode of Object.keys(api.repertoire)) assert.ok(choose(episode));
  assert.match(source, /mode === 'natural' \? 'visor' : 'manual'/);
  assert.match(source, /seed < 4096/);
  assert.match(source, /createDirector\(operationId, seed\)\.boundary\(model\)\?\.episode === episode/);
});

test('restoration joins all seven shared travel tracks for coffee and manual with empty hands', () => {
  assert.equal(api.adapters.restoration.safePoint, 'record-reseated-empty-hand');
  assert.equal(api.adapters.restoration.homeX, 20); assert.equal(api.adapters.restoration.cycleMs, 7200);
  const travel = css.split('\n').filter(line => line.includes('[data-family="restoration"]') && line.includes('animation: wv-coffee-'));
  assert.equal(travel.length, 7);
  for (const line of travel) {
    assert.ok(line.includes(':is([data-episode="coffee"], [data-episode="manual"])'));
    assert.ok(line.includes('[data-family="verification"]'));
  }
  assert.match(source, /results\.manualStarted\[1\]\.heldOpacity, 0/);
  assert.match(source, /results\.manualStarted\[1\]\.seatedOpacity, 1/);
  assert.match(source, /manualTracks\.some\(a => a\.name === 'wv-coffee-travel-checkpoint'\)/);
  assert.doesNotMatch(css, /@keyframes wv-coffee-restor/);
});

test('natural video contains no seek, pause, acceleration or diagnostic context before finalization', () => {
  const run = source.slice(source.indexOf('  browser = await launchBrowser('));
  const finalize = run.indexOf('await finalizeNaturalVideo()');
  const natural = run.slice(0, finalize);
  assert.ok(finalize > 0 && run.indexOf('await openApplication(false)') > finalize);
  assert.ok(run.indexOf('getScreenCTM()') > run.indexOf('await openApplication(false)'));
  assert.doesNotMatch(natural, /\.seek\(|__waitingMotionSampling\.begin|pauseArtwork\(|emulateMedia/);
  assert.doesNotMatch(source, /\.(?:pause|play)\s*\(|dispatchEvent|new AnimationEvent|playbackRate\s*=(?!=)/);
  assert.match(natural, /\['work', 'work'\], \['visor', 'react'\], \['work', 'work'\]/);
  assert.match(natural, /visorDurationMs >= 4700 && results\.visorDurationMs < 7800/);
  assert.match(natural, /results\.sequence\.stableSvg, true/);
  assert.match(natural, /results\.sequence\.applicationAfter, results\.sequence\.applicationBefore/);
});

test('rendered diagnostics measure both 17%/81% transfers and restore clocks governed by CSS', () => {
  assert.match(source, /addInitScript\(installMotionSampling\)/);
  assert.match(source, /\[\.14, \.17, \.20, \.78, \.81, \.84\]/);
  assert.match(source, /\[\[91, 55\], \[94, 50\], \[111, 50\], \[111, 62\], \[94, 62\], \[99, 55\]\]/);
  assert.match(source, /sample\.heldOpacity \+ sample\.seatedOpacity, 1/);
  assert.match(source, /assertRestorationOwnership\(sample\)/);
  assert.match(source, /sample\.heldOpacity, sample\.expectedHeld/);
  assert.match(source, /sample\.kind === 'exact-transfer'/);
  assert.match(source, /finally \{ await sampling\.restore\(\); \}/);
  assert.match(source, /finally \{ await pose\.evaluate\(sampling => sampling\.restore\(\)\)/);
  assert.match(source, /product-pause-before-any-seek/); assert.match(source, /product-pause-after-restored-seek/);
  assert.match(source, /Promise\.all\([^\n]+map\(a => a\.ready\)\)/);
  assert.match(source, /frozen\.map\(s => s\.animations\.map\(a => a\.currentTime\)\)/);
});

test('offscreen, reduced-motion, both themes and actual owner/terminal interruption remain covered', () => {
  assert.match(source, /style\.top = '200vh'/); assert.match(source, /style\.top = '150px'/);
  assert.match(source, /page\.emulateMedia\(\{ reducedMotion: 'reduce' \}\)/);
  for (const name of ['contact-dark', 'contact-light', 'reduced-motion-light']) assert.ok(source.includes(`waiting-restore-access-${name}.png`));
  const owner = source.slice(source.indexOf('results.ownerInterruption = await page.evaluate('), source.indexOf('assert.equal(results.ownerInterruption.beforeEpisode'));
  assert.doesNotMatch(owner, /await .*\n.*await |setTimeout|requestAnimationFrame/);
  assert.match(source, /results\.ownerInterruption\.beforeEpisode, 'manual'/);
  assert.match(source, /results\.ownerInterruption\.history, \[\]/);
  const terminal = source.slice(source.indexOf('results.terminal = await page.evaluate('), source.indexOf('for (const sample of results.terminal)'));
  assert.doesNotMatch(terminal.slice(terminal.indexOf('\n') + 1), /await |setTimeout|requestAnimationFrame/);
  assert.match(terminal, /view\.update\(\{ \.\.\.receipt, state \}\)/);
  assert.match(source, /sample\.before\.episode, 'manual'/);
  assert.match(source, /sample\.after\.propsPresent, false/);
  assert.match(source, /results\.terminal\.map\(s => s\.after\.state\), \['error', 'completed'\]/);
});

test('new evidence stays in other with manual and has a strict per-file transfer gate', () => {
  for (const filename of ['waiting-manual-real-preview.webm', 'waiting-restore-access-real-preview.webm']) {
    assert.equal(artifactFor(filename), 'preview-recordings-other');
    assert.doesNotMatch(workflow, new RegExp(`!output/playwright/${filename.replaceAll('.', '\\.')}`));
  }
  assert.match(source, /evidenceFileLimitBytes: 32 \* 1024 \* 1024/);
  assert.match(source, /results\.video\.bytes > 0 && results\.video\.bytes < results\.limits\.evidenceFileLimitBytes/);
  assert.match(source, /estimatedNaturalBytes: 6000000, estimatedOtherArchiveBytes: 16000000/);
  assert.ok(16000000 < 32 * 1024 * 1024);
  assert.match(source, /waiting-restore-access-results\.json/); assert.match(source, /waiting-restore-access-video\.json/);
  assert.match(source, /totalTimeoutMs: 110000, targetRuntimeMs: \[55000, 80000\]/);
});

test('full and affected selection adds restore/access without dropping established gates', () => {
  const existing = ['test-waiting-visuals.mjs', 'test-waiting-reactions.mjs', 'test-waiting-verification.mjs', 'test-waiting-manual.mjs', 'test-case-report-waiting.mjs', 'test-timeline-export-waiting.mjs'];
  for (const path of ['frontend/waiting-visuals.js', 'frontend/waiting-visuals.css', 'frontend/tasks.js', 'scripts/ci/validation-plan.mjs']) {
    const plan = planValidation([path]); assert.equal(plan.native, false);
    for (const name of [...existing, 'test-waiting-restore-access.mjs']) assert.ok(plan.preview.includes(name));
  }
  assert.ok(fullPreview.includes('test-waiting-restore-access.mjs'));
});


test('short-to-long restoration creates its boundary clock with the newly assigned arm and record tracks', () => {
  const clocks = css.split('\n').filter(line => line.includes('[data-family="restoration"]') && line.includes('.wv-work-boundary {'));
  assert.equal(clocks.length, 1);
  assert.ok(clocks[0].includes('[data-pace="loop"]'), 'a finished gesture clock must not retain 2.4s while new prop tracks start at zero');
  assert.match(source, /results\.visorStarted\[1\]\.heldOpacity, 0/);
  assert.match(source, /results\.visorStarted\[1\]\.seatedOpacity, 1/);
});


test('all six reacting families gate their boundary to the long hand/prop assignment', () => {
  const names = Object.keys(api.adapters).filter(family => api.adapters[family].reactions);
  assert.equal(names.length, 6);
  for (const family of names) {
    const clocks = css.split('\n').filter(line => line.includes(`[data-family="${family}"]`) && line.includes('.wv-work-boundary {'));
    assert.equal(clocks.length, 1, `${family}: one clock assignment`);
    assert.ok(clocks[0].includes('[data-pace="loop"]'), `${family}: no stale short gesture clock`);
  }
});


test('new families preserve animation identities and frozen clocks under reduced motion, with an initial static fallback', () => {
  assert.match(source, /animationIds: new WeakMap\(\)/);
  assert.match(source, /results\.reducedFrozen\.map\(s => s\.poses\), results\.reduced\.map\(s => s\.poses\)/);
  assert.match(source, /beforeReduced\[0\]\.animations\.every\(a => a\.playState === 'finished'\)/);
  assert.match(source, /results\.reducedResumed\[0\]\.poses, beforeReduced\[0\]\.poses/);
  assert.match(source, /identities\(results\.reduced\), identities\(beforeReduced\)/);
  assert.match(source, /s\.animations\.length > 0 && s\.animations\.every/);
  assert.match(source, /identities\(results\.reducedFrozen\), identities\(results\.reduced\)/);
  assert.match(source, /identities\(results\.reducedResumed\), identities\(beforeReduced\)/);
  assert.match(source, /a\.currentTime >= results\.reducedFrozen/);
  assert.match(source, /if \(sample\.family === 'restoration'\)/);
  assert.match(source, /mountFixture\('diagnostic', 'static'\)/);
  assert.match(source, /results\.initiallyReduced\.every\(s => s\.animations\.length === 0 && s\.motion === 'static'\)/);
});


const probeSource = source.slice(source.indexOf('const restorationSampling ='), source.indexOf("const output = resolve('output/playwright')"));
const probeApi = vm.runInNewContext(probeSource + '\n({ restorationSampling, restorationProbePlan, assertRestorationOwnership })', { assert });
const plainProbes = () => JSON.parse(JSON.stringify(probeApi.restorationProbePlan()));
const ownershipSample = (probe, heldOpacity = probe.expectedHeld ?? 0, seatedOpacity = 1 - heldOpacity) => ({ ...probe, heldOpacity, seatedOpacity,
  timeline: ['wv-restore-held', 'wv-restore-seated', 'wv-restore-head', 'wv-restore-gaze', 'wv-restore-arm', 'wv-restore-hand', 'wv-work-boundary']
    .map(name => ({ name, durationMs: 7200, currentTime: probe.requestedTimeMs, progress: probe.requestedTimeMs / 7200, playState: 'paused', pending: false })) });

test('ownership probes keep all prior samples and bound both sides of each discontinuity to one millisecond', () => {
  const probes = plainProbes();
  assert.equal(probes.length, 20);
  assert.deepEqual(probes.filter(p => p.kind === 'exact-transfer').map(p => [p.fraction, p.requestedTimeMs]), [[.17, 1224], [.81, 5832]]);
  assert.deepEqual(probes.filter(p => p.transferFraction !== undefined).map(p => [p.kind, p.requestedTimeMs, p.expectedHeld]),
    [['before-transfer', 1223, 0], ['after-transfer', 1225, 1], ['before-transfer', 5831, 1], ['after-transfer', 5833, 0]]);
  for (const fraction of [0, .14, .16999, .17, .20, .30, .42, .48, .58, .64, .78, .80999, .81, .84, .94, .99999]) assert.ok(probes.some(p => p.fraction === fraction));
  assert.equal(probeApi.restorationSampling.maxTimeErrorMs, .01);
  assert.ok(probeApi.restorationSampling.maxTimeErrorMs < 7200 * .00001 / 4, 'even the preserved 0.072ms pre-edge probes remain on their measured side');
  assert.ok(probes.filter(p => p.transferFraction !== undefined).every(p => Math.abs(p.requestedTimeMs - 7200 * p.transferFraction) === 1));
});

test('CI70 ownership evidence isolates the exact-edge ambiguity without accepting a wrong side, duplication or disappearance', () => {
  // CI70 run37014947141, head726ca2c3, checkout27812756: distilled browser JSON,
  // not a new rendered execution or evidence of the engine internal precision.
  const observed = [[0,0],[.14,0],[.16999,0],[.17,0],[.20,1],[.30,1],[.42,1],[.48,1],
    [.58,1],[.64,1],[.78,1],[.80999,1],[.81,1],[.84,0],[.94,0],[.99999,0]];
  assert.deepEqual(observed.filter(([fraction, held]) => held !== (fraction >= .17 && fraction < .81 ? 1 : 0)).map(([fraction]) => fraction), [.17, .81]);
  const probes = plainProbes();
  for (const [fraction, held] of observed) {
    const probe = probes.find(p => p.fraction === fraction);
    // Synthetic observable clocks isolate this pure gate. CI70 did not save seek clocks.
    assert.doesNotThrow(() => probeApi.assertRestorationOwnership(ownershipSample(probe, held)));
  }
  for (const probe of probes) {
    for (const pair of [[0,0],[1,1],[.5,.5]]) assert.throws(() => probeApi.assertRestorationOwnership(ownershipSample(probe, ...pair)));
    if (probe.kind !== 'exact-transfer') assert.throws(() => probeApi.assertRestorationOwnership(ownershipSample(probe, 1 - probe.expectedHeld)));
    else for (const held of [0,1]) assert.doesNotThrow(() => probeApi.assertRestorationOwnership(ownershipSample(probe, held)));
  }
});

test('every sampled clock must expose the requested side at the stated precision and paused state', () => {
  const probe = plainProbes().find(p => p.kind === 'after-transfer');
  for (const mutate of [s => s.timeline.pop(), s => { s.timeline[0].currentTime += .02; }, s => { s.timeline[0].currentTime = null; },
    s => { s.timeline[0].progress = null; }, s => { s.timeline[0].durationMs = 2400; }, s => { s.timeline[0].playState = 'running'; },
    s => { s.timeline[0].pending = true; }]) {
    const sample = ownershipSample(probe); mutate(sample); assert.throws(() => probeApi.assertRestorationOwnership(sample));
  }
  assert.match(source, /currentTime: animation\.currentTime, progress: animation\.effect\.getComputedTiming\(\)\.progress/);
  assert.match(source, /keyframes: animation\.effect\.getKeyframes\(\)\.map/);
  assert.match(source, /results\.rig\.samples\.length, 20/);
  assert.match(source, /results\.rig\.contacts\.length, 10/);
  assert.match(source, /probe\.transferFraction !== undefined/);
});
