/* Pure/static contract: no browser/server/Rust. Rendered contacts, themes, natural
   choreography and WebM must pass the real preview in CI and visual review. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { fullPreview, planValidation } from '../ci/validation-plan.mjs';

const source = readFileSync(new URL('../preview/test-waiting-manual.mjs', import.meta.url), 'utf8');
const coffee = readFileSync(new URL('../preview/test-waiting-reactions.mjs', import.meta.url), 'utf8');
const verification = readFileSync(new URL('../preview/test-waiting-verification.mjs', import.meta.url), 'utf8');
const visualSource = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const window = {};
vm.runInNewContext(visualSource, { window, Intl });
const api = window.WaitingVisuals;
const plain = value => JSON.parse(JSON.stringify(value));
const phases = { reading: 'metadata-scan', checkpoint: 'metadata-checkpoint-sync', calculation: 'command:aggregate_events',
  composition: 'command:case_report_render', verification: 'metadata-validate' };
function discover(operationId, family, episode) {
  const model = api.derive({ operationId, phaseId: phases[family], state: 'running', elapsedMs: 60000 });
  for (let seed = 0; seed < 4096; seed++) {
    if (api.createDirector(operationId, seed).boundary(model)?.episode === episode) return { seed, model };
  }
  assert.fail(`No seed found for ${family}/${episode}`);
}

test('manual fixture renders real reading/composition with labelled, independent synthetic receipts', () => {
  assert.match(source, /const families = \['reading', 'composition'\]/);
  assert.match(source, /const phases = \['metadata-scan', 'command:case_report_render'\]/);
  assert.match(source, /fixture com dados sintéticos/);
  assert.match(source, /recibos sintéticos.*não mede a operação nativa/);
  assert.match(source, /WaitingVisuals\.mount\(host, receipt, \{ reactionSeed \}\)/);
  assert.match(source, /completed: index === 0 \? 1200 : 7, total: index === 0 \? 6300 : 18/);
  assert.match(source, /assert\.deepEqual\(results\.sequence\.applicationAfter, results\.sequence\.applicationBefore\)/);
  assert.match(source, /updated\[1\]\.metric, before\[1\]\.metric/);
  assert.doesNotMatch(source, /<svg|innerHTML\s*=|addStyleTag\(|\.route\(|state\.(?:rows|loaded)\s*=/);
});

test('all current coffee/manual fixtures discover deterministic first-boundary seeds after repertoire expansion', () => {
  const selections = [];
  for (const family of Object.keys(phases)) for (const episode of ['coffee', 'manual']) {
    const operationId = episode === 'manual' ? `manual-${family}-fixture`
      : family === 'verification' ? 'verification-coffee-fixture' : `reaction-${family}`;
    const { seed, model } = discover(operationId, family, episode);
    const director = api.createDirector(operationId, seed);
    assert.equal(director.boundary(model).episode, episode);
    assert.equal(director.inspect().cycle, 1);
    assert.equal(discover(operationId, family, episode).seed, seed);
    // The real private clock advances until the first safe boundary. It does
    // not alter the already-eligible repertoire used by the seed search.
    const bridgedAge = { ...model, elapsedMs: 60000 + api.adapters[family].cycleMs };
    assert.equal(api.createDirector(operationId, seed).boundary(bridgedAge).episode, episode);
    selections.push({ family, episode, seed });
  }
  assert.equal(selections.length, 10);
  for (const script of [source, coffee, verification]) {
    assert.match(script, /seed < 4096/);
    assert.match(script, /createDirector\([^\n]+\)\.boundary\(model\)\?\.episode/);
  }
  assert.match(source, /results\.seeds = await page\.evaluate\(\(\) => __manualPreview\.seeds\)/);
  assert.match(coffee, /reactionSeed: __reactionPreview\.seeds\[family\]/);
  assert.doesNotMatch(coffee, /reading: 32, checkpoint: 31/);
});

test('manual retains every adapter including verification anchor20 and both shared travel paths', () => {
  assert.deepEqual(plain(api.adapters.verification), { reactions: true, homeX: 20, head: 4,
    cycleMs: 9600, safePoint: 'lens-docked-empty-hand' });
  assert.deepEqual(plain(api.repertoire.manual), { minElapsedMs: 60000, durationMs: 32000, cooldown: 5, weight: 1, long: true });
  assert.equal(api.repertoire.coffee.long, true);
  for (const part of ['reaction-actor', 'react-leg-front', 'react-knee-front', 'react-foot-front', 'react-leg-back', 'react-knee-back', 'react-foot-back']) {
    const lines = css.split('\n').filter(line => line.includes(`] .wv-${part} { animation: wv-coffee-`));
    assert.equal(lines.length, 2);
    assert.ok(lines.every(line => line.includes(':is([data-episode="coffee"], [data-episode="manual"])')));
    assert.ok(lines.some(line => line.includes('[data-family="verification"]')));
  }
});

test('natural WebM is finalized before the second context, pause checks, geometry and seeks', () => {
  const run = source.slice(source.indexOf('  browser = await launchBrowser();'));
  const finalize = run.indexOf('await finalizeNaturalVideo()');
  const diagnostic = run.indexOf('await openApplication(false)');
  const geometry = run.indexOf('getScreenCTM()');
  const firstSeek = run.indexOf('sampling.seek(');
  const firstPause = run.indexOf('await pauseManual(');
  assert.ok(finalize > 0 && diagnostic > finalize && geometry > diagnostic && firstSeek > diagnostic && firstPause > diagnostic);
  assert.doesNotMatch(run.slice(0, finalize), /\.seek\(|__waitingMotionSampling\.begin|pauseManual\(/);
  assert.match(source, /recording \? \{ recordVideo:/);
  assert.match(source, /waiting-manual-real-preview\.webm/);
  assert.match(source, /waiting-manual-results\.json/);
  assert.match(source, /waiting-manual-video\.json/);
  assert.match(source, /durationMs >= 31900 && durationMs < 35000/);
  assert.match(source, /prepareMs >= 3100.*resumeMs >= 3100/);
  assert.match(source, /\['manual', 'prepare'\], \['manual', 'react'\], \['manual', 'resume'\]/);
  assert.match(source, /a\.durationMs === 32000 && a\.iterations === '1' && a\.playbackRate === 1/);
});

test('rendered geometry measures shelf matrices, handoffs, both palms, reading and exactly one page turn', () => {
  assert.match(source, /\[9, 26\.5\]/);
  assert.match(source, /\[15, 22\]/);
  assert.match(source, /\[\[64, 48\.5\], \[74\.5, 63\], \[85, 48\.5\]\]/);
  assert.match(source, /\[16\.2, 16\.6, 20\.2, 20\.6\]/);
  assert.match(source, /\['cover-open', cover, 15, 16\], \['page-turn', leaf, 18\.2, 19\.6\], \['cover-close', cover, 21, 22\]/);
  assert.match(source, /step <= 16/);
  assert.match(source, /\[\[17, 77, 52\], \[17\.4, 83, 52\], \[17\.6, 77, 55\], \[18, 83, 55\]\]/);
  assert.match(source, /p\.scaleX < turn\[i - 1\]\.scaleX/);
  assert.match(source, /reset\.supportedOpacity, 0/);
  assert.match(source, /p\.shelf \+ p\.front \+ p\.back - 1/);
  assert.match(source, /finally \{ await sampling\.restore\(\); \}/);
});

test('pause before/after sampling settles CSS tasks, preserves exact clocks and never uses WAAPI overrides', () => {
  assert.match(source, /addInitScript\(installMotionSampling\)/);
  assert.doesNotMatch(source, /\.(?:pause|play)\s*\(|dispatchEvent|new AnimationEvent|playbackRate\s*=(?!=)|reactionPreview/);
  assert.doesNotMatch(visualSource, /reactionPreview|setTimeout|requestAnimationFrame|animation\.currentTime/);
  const run = source.slice(source.indexOf('  browser = await launchBrowser();'));
  assert.ok(run.indexOf('results.preSamplingPause =') < run.indexOf('results.rigs ='));
  assert.ok(run.indexOf('results.postSamplingPause =') > run.indexOf('results.rigs ='));
  assert.ok(run.indexOf('results.postSamplingPause =') > run.indexOf('await poses.evaluate(sampling => sampling.restore())'));
  const pause = source.slice(source.indexOf('async function pauseManual('), source.indexOf('try {\n  browser ='));
  assert.ok(pause.indexOf('const requested =') < pause.indexOf('await settlePausedArtwork()'));
  assert.ok(pause.indexOf('await settlePausedArtwork()') < pause.indexOf('const settled ='));
  assert.match(source, /Promise\.all\(animations\.map\(animation => animation\.ready\)\)/);
  assert.match(pause, /assert\.deepEqual\(frozen\.animations\.map/);
  assert.match(source, /await poses\.evaluate\(sampling => sampling\.restore\(\)\)/);
  assert.doesNotMatch(source, /roots\(\)\.evaluateHandle/);
  assert.doesNotMatch(coffee, /roots\.evaluateHandle/);
  assert.match(source, /const poses = await page\.evaluateHandle\(async \(\) =>/);
});

test('both themes, hidden/reduced-motion and immediate running-episode interruption are covered', () => {
  for (const file of ['supported-diagnostic-dark', 'supported-diagnostic-light', 'reduced-motion-light'])
    assert.ok(source.includes(`waiting-manual-${file}.png`));
  assert.match(source, /page\.emulateMedia\(\{ reducedMotion: 'reduce' \}\)/);
  assert.match(source, /results\.hiddenFrozen\.map\(s => s\.animations\.map/);
  const terminal = source.slice(source.indexOf('results.terminal = await page.evaluate('), source.indexOf('for (const sample of results.terminal)'));
  assert.doesNotMatch(terminal.slice(terminal.indexOf('\n') + 1), /await |setTimeout|requestAnimationFrame/);
  assert.match(terminal, /state: family === 'reading' \? 'error' : 'completed'/);
  assert.match(source, /sample\.before\.episode, 'manual'/);
  assert.match(source, /sample\.after\.manualPresent, false/);
  assert.match(source, /sample\.after\.kitchenPresent, false/);
});

test('legacy coffee pose carries an explicit layering review instead of inferring appearance from keyframes', () => {
  assert.match(coffee, /rearArmAfterTorso:/);
  assert.match(coffee, /waiting-reactions-coffee-layering-diagnostic-dark\.png/);
  assert.match(coffee, /compare this coffee pose visually with the approved reference/);
  assert.match(coffee, /finally \{ await coffeePose\.evaluate\(sampling => sampling\.restore\(\)\)/);
});

test('manual preview is budgeted below smoke timeout and selected for full/shared waiting validation', () => {
  assert.match(source, /totalTimeoutMs: 110000, targetRuntimeMs: \[70000, 90000\]/);
  assert.match(source, /naturalBudgetMs: 46000, diagnosticBoundaryBudgetMs: 10800/);
  assert.ok(api.adapters.composition.cycleMs + api.adapters.composition.bridgeMs * 2 + api.repertoire.manual.durationMs <= 46000);
  for (const path of ['frontend/waiting-visuals.js', 'frontend/waiting-visuals.css', 'frontend/tasks.js']) {
    const plan = planValidation([path]);
    assert.equal(plan.native, false); assert.ok(plan.preview.includes('test-waiting-manual.mjs'));
  }
  assert.ok(fullPreview.includes('test-waiting-manual.mjs'));
});

test('manual evidence requires its own temporary open shelf and hides all effective coffee artwork', () => {
  assert.match(source, /manualKitOpacity: opacity\('\.wv-manual-kit'\)/);
  assert.match(source, /hatchOpacity: effectiveOpacity\(root\.querySelector\('\.wv-kitchen-hatch'\)\)/);
  assert.match(source, /steamOpacity: Math\.max/);
  for (const prop of ['kitchenOpacity','hatchOpacity','cupOpacity','steamOpacity']) assert.ok(source.includes(`assert.equal(state.${prop}, 0)`));
  assert.match(source, /results\.initial\.every\(s => [^\n]+s\.manualKitOpacity === 0/);
  assert.match(source, /results\.naturalSupported\.every\(s => [^\n]+s\.manualKitOpacity === 1 && s\.shelfOpacity === 0/);
  assert.match(source, /results\.returned\.every\(s => [^\n]+s\.manualKitOpacity === 0/);
  assert.match(source, /shelfParentIsKit: shelf\.parentElement === kit, shelfOutsideKitchen: !kitchen\.contains\(shelf\)/);
  assert.match(source, /p\.kitchen === 0 && p\.hatch === 0 && p\.cup === 0 && p\.steam === 0/);
  assert.match(source, /p\.seconds >= 1\.6 && p\.seconds <= 31\.04/);
  assert.match(source, /\[1\.6, 31\.04\].*actorX/);
  assert.match(source, /sample\.after\.manualKitPresent, false/);
  assert.doesNotMatch(source, /(?:state|s)\.kitchenOpacity\s*(?:>|===)\s*1/);
});

test('manual preview measures direct empty-hand reach and retract rather than operating the coffee hatch', () => {
  assert.match(source, /\[8\.75, 9, 26\.5, 26\.9\]/);
  assert.match(source, /\[\[6\.5, 8\.75\], \[26\.9, 28\.5\]\]/);
  assert.match(source, /result\.reachPaths\.push/);
  assert.match(source, /rig\.reachContacts\.every\(p => p\.distance < \.15\)/);
  assert.match(source, /p\.distance < \.15 && p\.verticalSpan < 1 && p\.frontOwner === 0 && p\.backOwner === 0/);
  assert.match(coffee, /wv-kitchen-hatch/);
  assert.match(coffee, /wv-cup-steam/);
});
