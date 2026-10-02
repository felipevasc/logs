/* Static/Node preparation contract only. Does not launch a browser or certify
   rendered frames; the generated natural WebM must be reviewed after CI. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { fullPreview, planValidation } from '../ci/validation-plan.mjs';
import { artifactFor } from '../preview/record-evidence.mjs';

const source = readFileSync(new URL('../preview/test-waiting-micro-reactions.mjs', import.meta.url), 'utf8');
const product = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const workflow = readFileSync(new URL('../../.github/workflows/checks.yml', import.meta.url), 'utf8');
const window = {};
vm.runInNewContext(product, { window, Intl });
const api = window.WaitingVisuals;
const cardsText = source.slice(source.indexOf('const cards = '), source.indexOf('\nconst output'));
const cards = vm.runInNewContext(cardsText.replace('const cards = ', 'globalThis.cards = ') + '\ncards;');
const plain = value => JSON.parse(JSON.stringify(value));

test('four independent cards cover both reactions, two anchors and real preparation/return adapters', () => {
  assert.deepEqual(plain(cards.map(c => [c.family, c.episode])), [
    ['reading', 'visor'], ['reading', 'wave'], ['composition', 'visor'], ['composition', 'wave'],
  ]);
  for (const card of cards) {
    assert.equal(api.derive({ operationId: 'micro-fixture', phaseId: card.phaseId, state: 'running' }).family, card.family);
    assert.equal(card.durationMs, api.repertoire[card.episode].durationMs);
  }
  assert.match(source, /fixture com dados sintéticos/);
  assert.match(source, /recibos sintéticos.*não mede a operação nativa/);
  assert.match(source, /WaitingVisuals\.mount\(host, receipt, \{ reactionSeed \}\)/);
  assert.doesNotMatch(source, /<svg|innerHTML\s*=|addStyleTag\(|\.route\(|state\.(?:rows|loaded)\s*=/);
  assert.doesNotMatch(source, /wv-(?:coffee|manual|cup|kitchen)-/, 'the preview does not depend on shelf/coffee/manual props');
  assert.match(source, /events\[react\]\.proofParkedOpacity, 1/);
  assert.match(source, /prepareMs >= 3100.*resumeMs >= 3100/);
  assert.match(source, /sequence\.applicationAfter, results\.sequence\.applicationBefore/);
  assert.match(source, /sequence\.stableSvg, true/);
});

test('every first-boundary seed is discovered with a deterministic bounded search', () => {
  const found = [];
  for (const card of cards) {
    const operationId = `micro-${card.key}-fixture`;
    const model = api.derive({ operationId, phaseId: card.phaseId, state: 'running', elapsedMs: 60000 });
    const discover = () => {
      for (let seed = 0; seed < 4096; seed++) if (api.createDirector(operationId, seed).boundary(model)?.episode === card.episode) return seed;
      assert.fail(`no ${card.key} seed`);
    };
    const seed = discover(); assert.equal(discover(), seed);
    assert.equal(api.createDirector(operationId, seed).boundary({ ...model, elapsedMs: 60000 + api.adapters[card.family].cycleMs }).episode, card.episode);
    found.push({ key: card.key, seed });
  }
  assert.equal(found.length, 4);
  assert.match(source, /seed < 4096/);
  assert.match(source, /createDirector\(receipt\.operationId, seed\)\.boundary\(model\)\?\.episode === card\.episode/);
  assert.match(source, /results\.seeds = await page\.evaluate\(\(\) => __microPreview\.seeds\)/);
  assert.match(source, /if \(reactionSeed === undefined\) throw Error/);
});

test('the short recording includes natural work, gesture and return before any diagnostic sampling', () => {
  const run = source.slice(source.indexOf('try {\n  browser ='));
  const finalize = run.indexOf('await finalizeNaturalVideo()'), secondContext = run.indexOf('await openApplication(false)');
  const firstSeek = run.indexOf('sampling.seek('), firstButton = run.indexOf("locator('.wv-motion-toggle').all()");
  assert.ok(finalize > 0 && secondContext > finalize && firstSeek > secondContext && firstButton > secondContext);
  assert.doesNotMatch(run.slice(0, finalize), /\.seek\(|__waitingMotionSampling\.begin|setMotionEnabled|\.click\(/);
  assert.match(run.slice(0, finalize), /events\.some\(event => event\.key === key && event\.adapter === 'react'\)/);
  assert.match(source, /events\.map\(e => \[e\.episode, e\.adapter\]\), expected/);
  assert.match(source, /\[card\.episode, 'prepare'\], \[card\.episode, 'react'\], \[card\.episode, 'resume'\]/);
  assert.match(source, /a\.durationMs === card\.durationMs && a\.iterations === '1' && a\.playbackRate === 1/);
  assert.match(source, /includesDiagnosticSeeks: false, naturalComplete: false/);
  assert.match(source, /results\.video\.naturalComplete = true;\s*await finalizeNaturalVideo\(\)/);
  assert.match(source, /await context\.close\(\); context = null;\s*await video\.saveAs/);
  assert.match(source, /await video\.delete\(\); video = null/);
  assert.doesNotMatch(source, /\.pause\(|\.play\(|dispatchEvent|new AnimationEvent|playbackRate\s*=(?!=)/);
});

test('recording and fallback evidence budgets remain bounded and preserve original video bytes', () => {
  for (const card of cards) {
    const adapter = api.adapters[card.family];
    assert.ok(adapter.cycleMs + (adapter.bridgeMs || 0) * 2 + card.durationMs <= 18800);
  }
  assert.match(source, /naturalBudgetMs: 22000, recordingMaxMs: 30000/);
  assert.match(source, /videoMaxBytes: 6_000_000/);
  assert.match(source, /video\.bytes <= results\.limits\.videoMaxBytes/);
  assert.match(source, /captureWallMs <= results\.limits\.recordingMaxMs/);
  assert.match(source, /recordingTimer = setTimeout/);
  assert.match(source, /results\.video\.recordingLimitReached = true/);
  assert.match(source, /fallbackArtifact: 'preview-recordings-other'/);
  assert.match(source, /knownCompanionBudgetBytes: 5_614_425 \+ 6_000_000/);
  assert.ok(5_614_425 + 6_000_000 + 6_000_000 < 32_000_000);
  assert.equal(artifactFor('waiting-micro-reactions-real-preview.webm'), 'preview-recordings-other');
  const fallback = workflow.slice(workflow.indexOf('name: preview-recordings-other'), workflow.indexOf('\n  # Native changes'));
  assert.match(fallback, /output\/playwright\/\*\*\/\*\.webm/);
  assert.doesNotMatch(fallback, /!output\/playwright\/waiting-micro-reactions/);
  assert.doesNotMatch(source, /ffmpeg|transcod|unlinkSync|rmSync/);
  assert.match(source, /waiting-micro-reactions-results\.json/);
  assert.match(source, /waiting-micro-reactions-video\.json/);
});

test('rendered diagnostics measure visor contacts, original wrist paths, head clearance and planted feet', () => {
  assert.match(source, /new DOMPoint\(x, y\)\.matrixTransform\(node\.getScreenCTM\(\)\)/);
  assert.match(source, /node\.getScreenCTM\(\)\.inverse\(\)/);
  assert.match(source, /\[36, 43, 48, 55, 62\]/);
  assert.match(source, /percent === 43 \|\| percent === 55 \? 69 : 71\.5/);
  assert.match(source, /point\(head, x, 36\.5\)/);
  assert.match(source, /distanceCssPx < \.15/);
  assert.match(source, /rig\.samples\.every\(p => p\.clearHead\)/);
  assert.match(source, /p\.x > 79\.1 \|\| p\.y > 48\.1/);
  assert.match(source, /\[-18, 16, -14, 10\]\.entries\(\)/);
  assert.match(source, /rig\.wrists\[i\]\.fingerMatrix, rig\.wrists\[i\]\.palmMatrix/);
  assert.match(source, /Math\.hypot\(p\.center\.x - 83, p\.center\.y - 37\) < \.001/);
  assert.match(source, /JSON\.stringify\(s\.feet\) === JSON\.stringify\(rig\.samples\[0\]\.feet\)/);
  assert.match(source, /rig\.endpoints\[0\], rig\.endpoints\[1\]/);
  assert.match(source, /finally \{ await sampling\.restore\(\); \}/);
  assert.match(source, /finally \{ await poses\.evaluate\(sampling => sampling\.restore\(\)\)/);
  assert.match(source, /percent === 99\.9/);
  assert.doesNotMatch(source, /sampling\.seek\(durationMs\)|percent of \[[^\]]*\b100\b/);
});

test('pause and fresh terminal interruptions preserve the real product controls', () => {
  const run = source.slice(source.indexOf('try {\n  browser ='));
  assert.ok(run.indexOf('results.pauseSettled =') < run.indexOf('results.rigs ='));
  assert.match(source, /results\.pauseFrozen\.map\(s => s\.animations\.map\(a => a\.currentTime\)\)/);
  assert.match(source, /results\.receiptDuringPause\[0\]\.metric, '1\.300 \/ 6\.300 registros'/);
  const terminal = run.slice(run.indexOf('results.terminal = await page.evaluate('), run.indexOf('for (const sample of results.terminal)'));
  assert.doesNotMatch(terminal.slice(terminal.indexOf('\n') + 1), /await |setTimeout|requestAnimationFrame/);
  assert.match(terminal, /\['cancelled', 'error', 'completed', 'cancelling'\]/);
  assert.match(source, /sample\.before\.adapter, 'react'/);
  assert.match(source, /sample\.before\.motion, 'running'/);
  assert.match(source, /sample\.actorPresent, false/);
  for (const name of ['contact-diagnostic-dark', 'contact-diagnostic-light', 'reduced-motion-light']) assert.ok(source.includes(`waiting-micro-${name}.png`));
});

test('reduced motion retains nonempty animation identities and exact frozen clocks, then resumes the same microepisodes', () => {
  const reduced = source.slice(source.indexOf('const beforeReduced ='), source.indexOf('results.terminal ='));
  const enter = reduced.indexOf("await page.emulateMedia({ reducedMotion: 'reduce' })");
  const enable = reduced.indexOf('view.setMotionEnabled(true)');
  const exit = reduced.indexOf("await page.emulateMedia({ reducedMotion: 'no-preference' })");
  const resumed = reduced.indexOf('results.reducedResumed =');
  const fresh = reduced.indexOf('await mountFixture(true)');
  assert.ok(enter > 0 && enable > enter && exit > enable && resumed > exit && fresh > resumed);
  assert.match(source, /animationIds: new WeakMap\(\), nextAnimationId: 0/);
  assert.match(source, /if \(!fixture\.animationIds\.has\(a\)\) fixture\.animationIds\.set\(a, \+\+fixture\.nextAnimationId\)/);
  assert.match(source, /id: fixture\.animationIds\.get\(a\)/);
  assert.match(source, /sample\.animations\.map\(a => \[a\.id, a\.name, a\.target, a\.durationMs, a\.iterations, a\.playbackRate\]\)/);
  assert.match(reduced, /results\.beforeReduced = await states\(\)/);
  assert.match(reduced, /beforeReduced\.every\(s => s\.motion === 'static' && s\.adapter === 'react' && s\.animations\.length > 0/);
  assert.match(reduced, /await settle\(\); results\.reduced = await states\(\)/);
  assert.match(reduced, /results\.reduced\.every\(s => s\.motion === 'static' && s\.animations\.length > 0\s*&& s\.animations\.every\(a => a\.playState !== 'running' && !a\.pending\)/);
  assert.match(reduced, /results\.reduced\.map\(identity\), beforeReduced\.map\(identity\)/);
  assert.match(reduced, /results\.reduced\.map\(s => \[s\.episode, s\.adapter\]\), beforeReduced\.map\(s => \[s\.episode, s\.adapter\]\)/);
  assert.match(reduced, /results\.reduced\.map\(s => s\.animations\.map\(a => a\.currentTime\)\), beforeReduced\.map\(s => s\.animations\.map\(a => a\.currentTime\)\)/);
  assert.match(reduced, /await page\.waitForTimeout\(220\); results\.reducedFrozen = await states\(\)/);
  assert.match(reduced, /results\.reducedFrozen\.map\(identity\), results\.reduced\.map\(identity\)/);
  assert.match(reduced, /results\.reducedFrozen\.map\(s => s\.animations\.map\(a => a\.currentTime\)\), results\.reduced\.map\(s => s\.animations\.map\(a => a\.currentTime\)\)/);
  assert.match(reduced, /results\.reducedFrozen\.every\(s => s\.motion === 'static' && s\.animations\.every\(a => a\.playState !== 'running' && !a\.pending\)/);
  assert.match(reduced, /sameReceipt\(results\.reducedResumed, beforeReduced\)/);
  assert.match(reduced, /results\.reducedResumed\.map\(identity\), beforeReduced\.map\(identity\)/);
  assert.match(reduced, /assert\.equal\(sample\.motion, 'running'\)/);
  assert.match(reduced, /assert\.equal\(sample\.episode, cards\[i\]\.episode\)/);
  assert.match(reduced, /assert\.equal\(sample\.adapter, 'react'\)/);
  assert.match(reduced, /a\.currentTime >= frozen\.animations\[j\]\.currentTime/);
  assert.match(reduced, /a\.name === 'wv-episode-boundary' \|\| a\.name\.startsWith\(`wv-\$\{sample\.episode\}-`\)/);
  assert.match(reduced, /assert\.equal\(reactionTracks\.length, sample\.episode === 'visor' \? 7 : 10\)/);
  assert.match(reduced, /reactionTracks\.every\(a => a\.playState === 'running' && !a\.pending\s*&& a\.currentTime > frozen\.animations\.find\(previous => previous\.id === a\.id\)\.currentTime\)/);
  assert.doesNotMatch(reduced.slice(0, fresh), /mountFixture\(|\.currentTime\s*=(?!=)|sampling\.seek|new WeakMap\(|animationIds\s*=/);
  assert.doesNotMatch(source, /no promise of|no continuity claim|discard a gesture|never assume it preserves continuity/);
  assert.match(reduced.slice(fresh), /await mountFixture\(true\)/, 'terminal checks remount only after proving reduced-motion continuity');
});

test('full, affected waiting and direct preview selection retain the dedicated check without native rebuilds', () => {
  const name = 'test-waiting-micro-reactions.mjs';
  assert.equal(fullPreview.filter(test => test === name).length, 1);
  for (const path of ['frontend/waiting-visuals.js', 'frontend/waiting-visuals.css', 'frontend/tasks.js', `scripts/preview/${name}`]) {
    const plan = planValidation([path]); assert.equal(plan.native, false); assert.ok(plan.preview.includes(name));
  }
});


test('reduced microepisodes keep every SVG pose as well as animation clocks', () => {
  assert.match(source, /results\.reduced\.map\(s => s\.poses\), beforeReduced\.map\(s => s\.poses\)/);
  assert.match(source, /results\.reducedFrozen\.map\(s => s\.poses\), results\.reduced\.map\(s => s\.poses\)/);
});
