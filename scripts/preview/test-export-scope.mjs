/* Browser presentation/dispatch evidence with controlled preview transport.
   Native archive bytes, protected content and receipts have separate native gates. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const url = process.argv[2] || 'http://127.0.0.1:4173', output = resolve('output/playwright');
mkdirSync(output, { recursive: true });
const browser = await launchBrowser(), results = { evidence: 'Real export controls and production dispatch with preview transport; not native archive verification', modes: {} };
let page, phase = 'startup';
try {
  for (const nativeCase of [false, true]) {
    const mode = nativeCase ? 'native' : 'legacy', errors = [];
    page = await browser.newPage({ viewport: { width: 1024, height: 768 }, reducedMotion: 'reduce' });
    page.on('pageerror', error => errors.push(error.message));
    if (nativeCase) await page.addInitScript(() => { window.__mockNativeCaseBootstrapEnabled = true; });
    await page.goto(url);
    await page.waitForFunction(native => window.WorkspaceContext?.ready && !WorkspaceContext.changing && document.querySelector('#load-overlay').hidden &&
      (native ? CaseEvidence.active === true && nativeEvidenceServices().session.isClean() : state.loaded && state.rows.length > 0), nativeCase);
    const fixture = await page.evaluate(async native => {
      if (native) await window.workspaceBootstrap;
      else {
        await WorkspaceContext.setScope('dataset', { page: 'explore', animate: false });
        await loadExplorerAnalytics(explorerKey(), workspaceScope(), backendFilters()).promise;
        activeCase().items = state.rows.slice(0, 2).map((row, i) => ({ id: `export-item-${i}`, label: `Preserved item ${i}`, summary: 'Export draft', rows: [structuredClone(row)] }));
        caseEventsCache.sig = null;
      }
      const originalApi = api, originalSave = dialogApi.save;
      const transfer = window.CaseEvidenceTransfer.openExport, report = window.CaseReport.open;
      window.__exportTest = { calls: [], saves: [], fail: false, path: null, originals: { transfer, report }, restore() { api = originalApi; dialogApi.save = originalSave; } };
      dialogApi.save = async options => { __exportTest.saves.push(structuredClone(options)); return __exportTest.path; };
      api = async (command, args = {}, options = {}) => {
        if (!['export_events', 'export_document', 'export_investigation', 'case_export_native'].includes(command)) return originalApi(command, args, options);
        __exportTest.calls.push({ command, args: structuredClone(args) });
        if (__exportTest.fail) { const error = Error('EXPORT_TEST_REFUSAL: preserved target; retry available'); if (!options.silent) toast(error.message, 'err'); throw error; }
        if (command === 'case_export_native') return { kind: 'native_case_export', path: args.request.path, format: args.request.path.toLowerCase().endsWith('.licase') ? 'licase' : 'json', cases: 1, records: 2, masked: args.request.mask };
        return command === 'export_events' ? 3 : null;
      };
      return { caseId: activeCase().id, items: activeCase().items.length, nativeActive: CaseEvidence.active === true };
    }, nativeCase);
    assert.equal(fixture.nativeActive, nativeCase);
    const modal = page.locator('#ws-export-modal'), select = page.locator('#ws-export-kind'), mask = page.locator('#ws-mask');
    const text = () => modal.locator('#ws-export-scope').innerText();
    const help = () => modal.locator('#ws-export-help').innerText();
    const open = async () => { await page.locator('#ws-export').click(); await modal.waitFor({ state: 'visible' }); };
    const close = async () => { await page.locator('#ws-export-close').click(); await modal.waitFor({ state: 'hidden' }); };
    const formats = await select.locator('option').evaluateAll(nodes => nodes.map(node => node.value));
    assert.deepEqual(formats, ['jsonl', 'csv', 'case-pdf', 'report', 'case']);
    phase = `${mode}: retained formats`;
    for (const kind of formats) {
      // Intentionally set the retained choice without a change event, then use
      // the actual global opener. This reproduces the original stale-scope bug.
      await select.evaluate((node, value) => { node.value = value; }, kind);
      await page.evaluate(() => { state.total = 3; });
      await open();
      assert.equal(await page.evaluate(() => document.activeElement.id), 'ws-export-kind');
      const initial = { scope: await text(), help: await help() };
      assert.match(initial.scope, ['jsonl', 'csv'].includes(kind) ? /Recorte atual da Análise · 3 registros/ : new RegExp(`Caso completo · ${fixture.items} itens`));
      assert.match(initial.help, new RegExp({ jsonl: 'JSONL.*todos os resultados', csv: 'CSV.*todos os resultados', 'case-pdf': 'PDF com cronologia.*proteção de textos própria', report: 'Markdown.*sem imagens', case: 'LICASE com evidências' }[kind]));
      assert.equal(await mask.isVisible(), kind !== 'case-pdf'); assert.equal(await mask.isChecked(), true);
      await select.dispatchEvent('change'); assert.deepEqual({ scope: await text(), help: await help() }, initial);
      await close(); await open(); assert.deepEqual({ scope: await text(), help: await help() }, initial);
      assert.equal(await select.inputValue(), kind);
      await page.keyboard.press('Escape'); await modal.waitFor({ state: 'hidden' });
    }
    assert.equal(await page.evaluate(() => __exportTest.calls.length + __exportTest.saves.length), 0, 'opening and changing cannot export or request a save path');
    await open(); await select.selectOption('case');
    assert.match(await help(), nativeCase ? /JSON recusa Casos com referências ou histórico de exclusões/ : /JSON legado não transporta os arquivos das referências/);
    assert.match(await help(), /referências originais ou proveniência de exclusões.*recusada.*ocultação/);
    assert.match(await help(), /Imagens não recebem ocultação/);
    await mask.uncheck(); await select.selectOption('case-pdf'); assert.equal(await mask.isChecked(), false); assert.equal(await mask.isVisible(), false);
    await select.selectOption('report'); assert.equal(await mask.isVisible(), true); assert.equal(await mask.isChecked(), false);
    await mask.check(); await close();

    phase = `${mode}: Case scope and count boundaries`;
    if (!nativeCase) {
      await page.evaluate(() => WorkspaceContext.setScope('case', { page: 'evidence', animate: false }));
      await page.locator('#ws-evidence-export').click(); await modal.waitFor({ state: 'visible' });
      assert.equal(await select.inputValue(), 'case'); assert.match(await text(), /Caso completo · 2 itens/); assert.match(await help(), /^LICASE/);
      for (const kind of ['jsonl', 'csv']) {
        await page.evaluate(() => { state.total = 0; }); await select.selectOption(kind); assert.match(await text(), /Recorte atual do Caso · 0 registros/);
        await page.evaluate(() => { state.total = null; }); await select.dispatchEvent('change'); assert.match(await text(), /Total de registros/); assert.doesNotMatch(await text(), /0 registros/);
      }
      await close(); await page.evaluate(() => WorkspaceContext.setScope('dataset', { page: 'explore', animate: false }));
    }
    // A preserved native stub has no safely countable item array. Temporarily
    // substitute metadata only; never run an export with this synthetic stub.
    const unknown = await page.evaluate(() => {
      const original = activeCase(), index = state.cases.cases.indexOf(original);
      state.cases.cases[index] = { id: original.id, kind: 'preserved_case_unavailable' };
      document.querySelector('#ws-export-kind').value = 'case'; document.querySelector('#ws-export').click();
      const scope = document.querySelector('#ws-export-scope').textContent;
      document.querySelector('#ws-export-close').click(); state.cases.cases[index] = original;
      return scope;
    });
    assert.match(unknown, /Caso preservado · quantidade de itens indisponível/); assert.doesNotMatch(unknown, /0 itens/);

    phase = `${mode}: cancellation, failure and retry`;
    await select.evaluate(node => { node.value = 'case'; }); await open();
    const before = await page.evaluate(() => ({ draft: structuredClone(activeCase()), filters: structuredClone(state.filters) }));
    await page.locator('#ws-export-save').click();
    await page.waitForFunction(() => !document.querySelector('#ws-export-save').disabled && __exportTest.saves.length === 1);
    assert.equal(await modal.isVisible(), true); assert.equal(await mask.isChecked(), true);
    assert.equal(await page.evaluate(() => __exportTest.calls.length), 0);
    await page.evaluate(() => { __exportTest.fail = true; __exportTest.path = '/tmp/scope-case.licase'; });
    await page.locator('#ws-export-save').click();
    await page.waitForFunction(() => !document.querySelector('#ws-export-save').disabled && __exportTest.calls.length === 1);
    assert.equal(await modal.isVisible(), true); assert.equal(await mask.isChecked(), true); assert.equal(await select.inputValue(), 'case');
    assert.match(await page.locator('#toast-area').innerText(), /EXPORT_TEST_REFUSAL/);
    // Legacy capture is allowed to refresh view preferences; preserve the
    // authored Case texts/items and filter draft across the failed transfer.
    const after = await page.evaluate(() => ({ draft: structuredClone(activeCase()), filters: structuredClone(state.filters) }));
    assert.equal(after.draft.name, before.draft.name); assert.deepEqual(after.draft.items, before.draft.items); assert.deepEqual(after.filters, before.filters);
    await close(); await open(); assert.equal(await select.inputValue(), 'case'); assert.equal(await mask.isChecked(), true); assert.match(await text(), /Caso completo/);
    await page.evaluate(() => { __exportTest.fail = false; }); await page.locator('#ws-export-save').click(); await modal.waitFor({ state: 'hidden' });
    const dispatch = await page.evaluate(() => ({ calls: __exportTest.calls, saves: __exportTest.saves, unchangedControllers: __exportTest.originals.transfer === CaseEvidenceTransfer.openExport && __exportTest.originals.report === CaseReport.open }));
    assert.equal(dispatch.unchangedControllers, true); assert.equal(dispatch.calls.length, 2);
    assert.deepEqual(dispatch.calls.map(call => call.command), Array(2).fill(nativeCase ? 'case_export_native' : 'export_investigation'));
    for (const call of dispatch.calls) {
      const args = nativeCase ? call.args.request : call.args; assert.equal(args.mask, true); assert.equal(args.path, '/tmp/scope-case.licase');
      if (nativeCase) { const doc = JSON.parse(args.documentJson); assert.equal(doc.evidenceViewVersion, 1); assert.equal(doc.cases[0].items[0].rows.kind, 'native_evidence_container'); assert.equal(Object.hasOwn(args, 'data'), false); }
    }
    assert.deepEqual(dispatch.saves[0].filters.map(filter => filter.extensions), [['licase'], ['json']]);

    phase = `${mode}: PDF handoff and keyboard`;
    await open(); await select.selectOption('case-pdf');
    const callCount = await page.evaluate(() => __exportTest.calls.length);
    await page.locator('#ws-export-save').click(); await modal.waitFor({ state: 'hidden' });
    const pdf = page.getByRole('dialog', { name: 'Relatório do Caso', exact: true }); await pdf.waitFor({ state: 'visible' });
    assert.equal(await page.evaluate(() => __exportTest.calls.length), callCount);
    await pdf.getByRole('button', { name: 'Cancelar', exact: true }).click(); await pdf.waitFor({ state: 'hidden' });
    await open(); assert.equal(await select.inputValue(), 'case-pdf'); assert.equal(await mask.isChecked(), true);
    await select.selectOption('case');
    await page.locator('#ws-export-save').focus(); await page.keyboard.press('Tab'); assert.equal(await page.evaluate(() => document.activeElement.id), 'ws-export-close');
    await page.keyboard.press('Shift+Tab'); assert.equal(await page.evaluate(() => document.activeElement.id), 'ws-export-save');

    phase = `${mode}: compact layouts`;
    const layouts = [];
    for (const theme of ['dark', 'light']) {
      await page.evaluate(value => { if (document.documentElement.dataset.theme !== value) document.querySelector('#btn-theme').click(); }, theme);
      for (const width of [1024, 640]) {
        await page.setViewportSize({ width, height: 640 });
        const layout = await modal.evaluate(node => {
          const body = node.querySelector('.modal-body'), dialog = node.querySelector('[role="dialog"]'), bounds = dialog.getBoundingClientRect();
          return { width: innerWidth, theme: document.documentElement.dataset.theme, left: bounds.left, right: bounds.right, top: bounds.top, bottom: bounds.bottom, bodyWidth: body.clientWidth, bodyScrollWidth: body.scrollWidth, helpWidth: document.querySelector('#ws-export-help').getBoundingClientRect().width, pageOverflow: document.documentElement.scrollWidth > innerWidth };
        });
        assert.equal(layout.theme, theme); assert.ok(layout.left >= 0 && layout.right <= width + 1 && layout.top >= 0 && layout.bottom <= 641);
        assert.ok(layout.bodyScrollWidth <= layout.bodyWidth + 1); assert.equal(layout.pageOverflow, false); assert.ok(layout.helpWidth > 0);
        await page.locator('#ws-export-save').scrollIntoViewIfNeeded(); assert.equal(await page.locator('#ws-export-save').isVisible(), true);
        await page.screenshot({ path: resolve(output, `export-scope-${mode}-${theme}-${width}.png`) }); layouts.push(layout);
      }
    }
    assert.deepEqual(errors, []); results.modes[mode] = { retainedFormats: formats, scopeAndCounts: true, maskPreferenceRetained: true, cancelAndRetry: true, dispatch: nativeCase ? 'case_export_native' : 'export_investigation', pdfHandoff: true, keyboardTrap: true, layouts, errors };
    await page.close(); page = null;
  }
  writeFileSync(resolve(output, 'export-scope-results.json'), JSON.stringify(results, null, 2));
  console.log('Export scope: retained formats, truthful capabilities, masking, cancellation, retry, native dispatch and compact layouts passed');
} catch (error) {
  if (page) await captureFailure(page, 'export-scope', error, { phase, results });
  throw error;
} finally { await browser.close(); }
