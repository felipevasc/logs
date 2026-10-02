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
  assert.match(source, /sample\.fraction >= \.17 && sample\.fraction < \.81 \? 1 : 0/);
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
