/* Synthetic transport + real DOM: no triage until a deliberate calculation. */
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' });
const errors = []; page.on('pageerror', error => errors.push(error.message));
const calls = () => page.evaluate(() => window.__mockCommandCalls?.triage || 0);
const stateIs = value => page.waitForFunction(value => document.querySelector('[data-compromises-results]')?.dataset.analysisState === value, value);
let phase = 'startup';
try {
  await mkdir('output/playwright', { recursive: true });
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && state.rows.length && !state.loadOverlay);
  assert.equal(await calls(), 0, 'startup/import must not calculate Comprometimentos');
  const original = await page.evaluate(() => ({ caseId: activeCase().id, rows: structuredClone(state.rows.slice(0, 3)) }));
  phase = 'passive navigation';
  await page.evaluate(() => WorkspaceContext.setScope('dataset', { animate: false, page: 'summary' }));
  await page.waitForSelector('#ws-attention[data-analysis-state="idle"]');
  await page.screenshot({ path: 'output/playwright/compromises-demand-summary.png' });
  for (const name of ['compromises', 'case-timeline', 'summary', 'compromises']) await page.evaluate(name => Workspace.showPage(name), name);
  await stateIs('idle'); assert.equal(await calls(), 0);
  assert.match(await page.locator('[data-compromises-results]').innerText(), /ainda não calculados/);
  assert.doesNotMatch(await page.locator('[data-compromises-results]').innerText(), /Nenhum indício/);
  await page.screenshot({ path: 'output/playwright/compromises-demand-idle.png' });
  phase = 'navigation during calculation and cancellation';
  await page.evaluate(() => { window.__mockLatency = { triage: 2000 }; });
  await page.locator('[data-calculate-compromises]').click(); await stateIs('calculating');
  await page.waitForFunction(() => window.__mockCommandCalls.triage === 1);
  await page.evaluate(() => Workspace.showPage('explore'));
  await page.evaluate(() => Workspace.showPage('compromises')); await stateIs('calculating');
  assert.equal(await calls(), 1);
  assert.equal(await page.locator('[data-compromises-results]').getAttribute('aria-busy'), 'true');
  await page.locator('[data-cancel-compromises]').click(); await stateIs('cancelled');
  assert.equal(await calls(), 1); assert.equal(await page.evaluate(() => Security.cached()), null);
  assert.ok(await page.evaluate(() => __mockRequests.some(call => call.cmd === 'cancel_task' && call.operationId)));
  phase = 'failure and explicit retry';
  await page.evaluate(() => { window.__mockLatency = {}; window.__mockFailures = { triage: 'Falha sintética de leitura' }; });
  await page.locator('[data-calculate-compromises]').click(); await stateIs('failed');
  assert.match(await page.locator('[data-compromises-results]').innerText(), /Falha sintética/);
  await page.evaluate(() => { window.__mockFailures = {}; });
  await page.locator('[data-calculate-compromises]').click(); await stateIs('ready');
  assert.equal(await calls(), 3);
  assert.match(await page.locator('.evidence-empty').innerText(), /Nenhum indício no nível/);
  for (const name of ['summary', 'compromises', 'summary', 'compromises']) await page.evaluate(name => Workspace.showPage(name), name);
  assert.equal(await calls(), 3, 'returning to a valid result must not rescan');
  await page.locator('[data-evidence-min="2"]').first().click();
  assert.ok(await page.locator('.sec-episode').count()); assert.equal(await calls(), 3);
  phase = 'rules invalidate but never calculate';
  await page.evaluate(async () => {
    const settings = (await api('detection_rules', {}, { silent: true })).settings;
    await api('detection_settings_save', { settings: { ...settings, threats: !settings.threats } });
    Security.invalidate(); await Workspace.showPage('compromises');
  });
  await stateIs('stale'); assert.equal(await calls(), 3);
  await page.screenshot({ path: 'output/playwright/compromises-demand-stale.png' });
  await page.locator('[data-calculate-compromises]').click(); await stateIs('ready'); assert.equal(await calls(), 4);
  phase = 'reimport';
  await page.evaluate(() => Workspace.showPage('import'));
  await page.locator('#ws-empty-open').waitFor({ state: 'visible' });
  assert.equal(await calls(), 4, 'opening the import chooser must not calculate');
  // Import is the file chooser, not the legacy source form containing #btn-load.
  // Reopen the exact existing sources through their real user-facing action.
  await page.evaluate(() => Workspace.showPage('sources'));
  const reload = page.getByRole('button', { name: 'Recarregar fontes', exact: true });
  const oldReload = await reload.elementHandle();
  const beforeLoad = await page.evaluate(() => ({ generation: state.sourcePublication?.generation ?? 0,
    source: structuredClone(state.currentArtifact.source), loads: window.__mockCommandCalls.load_files || 0 }));
  await reload.click();
  await page.waitForFunction(before => (state.sourcePublication?.generation ?? 0) > before && !state.loadOverlay && state.loaded && !WorkspaceContext.changing && !WorkspaceContext.sourceBusy, beforeLoad.generation);
  // The click handler rerenders Sources after loadData/Workspace.loaded settle.
  // Wait for that completion, so it cannot overwrite the next navigation.
  await page.waitForFunction(button => !button.isConnected, oldReload);
  await reload.waitFor({ state: 'visible' }); await oldReload.dispose();
  const afterLoad = await page.evaluate(() => ({ source: state.currentArtifact.source,
    loads: window.__mockCommandCalls.load_files || 0, owner: state.sourcePublication?.analysisContext?.caseId }));
  assert.deepEqual(afterLoad.source, beforeLoad.source, 'reload must retain all existing source paths');
  assert.equal(afterLoad.loads, beforeLoad.loads + 1, 'the visible action must really reload the multi-file fixture');
  assert.equal(afterLoad.owner, original.caseId);
  await page.evaluate(() => Workspace.showPage('compromises')); await stateIs('stale'); assert.equal(await calls(), 4);
  phase = 'Case isolation';
  await page.evaluate(() => newCase('Comprometimentos sob demanda B'));
  await page.waitForFunction(() => !WorkspaceContext.changing && !WorkspaceContext.sourceBusy);
  assert.notEqual(await page.evaluate(() => activeCase().id), original.caseId, 'a new isolated Case was created');
  await page.evaluate(async rows => {
    activeCase().items = [{ id: 'demand-case-fixture', kind: 'events', label: 'Preservados', rows }]; caseEventsCache.sig = null;
    await WorkspaceContext.setScope('case', { animate: false, page: 'compromises' });
  }, original.rows);
  await stateIs('idle'); assert.equal(await calls(), 4); assert.equal(await page.evaluate(() => Security.last()), null);
  await page.locator('[data-calculate-compromises]').click(); await stateIs('ready');
  assert.equal(await calls(), 5); assert.equal(await page.evaluate(() => Security.cached().total), 3);
  await page.evaluate(id => WorkspaceContext.changeCase(id), original.caseId);
  await page.evaluate(() => Workspace.showPage('compromises'));
  assert.equal(await page.evaluate(() => activeCase().id), original.caseId, 'the Case switch must complete');
  assert.equal(await calls(), 5, 'reopening another Case does not start a calculation');
  assert.notEqual(await page.evaluate(() => Security.cached()?.total), 3, 'Case B results never leak into Case A');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ startupScans: 0, explicitRequests: 5, cancellation: true, errorRetry: true, staleRulesAndData: true, caseIsolation: true, repeatedNavigation: true, errors }, null, 2));
} catch (error) {
  await captureFailure(page, 'test-security-on-demand', error, { phase }); throw error;
} finally { await browser.close(); }
