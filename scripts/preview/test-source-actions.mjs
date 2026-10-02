/* Production Sources controls with synthetic source/hash transport. The before
   comparison reconstructs the previous row markup with the same fixture data;
   it is not a historical app build or installed WebView/native-engine evidence. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser(), page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: 'Real Sources controls and keyboard; synthetic list/hash/timestamp/clipboard transport. Before images reconstruct prior source-row markup; not native engine or installed WebView verification.', visuals: [] };
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));
const rows = page.locator('.sources-table tbody tr'), menu = page.locator('.ctx-menu');
const button = index => rows.nth(index).locator('.source-menu-trigger');
const name = index => rows.nth(index).locator('.source-path-toggle');
const path = index => rows.nth(index).locator('.source-full-path');
const check = index => rows.nth(index).locator('.source-check');
const focusIs = async locator => assert.equal(await locator.evaluate(node => node === document.activeElement), true);
const selectAction = async (index, label) => { await button(index).click(); await menu.waitFor({ state: 'visible' }); await menu.getByRole('menuitem', { name: label, exact: true }).click(); };
const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
async function screenshot(filename, caption) {
  await page.evaluate(text => {
    const label = document.createElement('div'); label.id = 'source-fixture-label'; label.textContent = text;
    label.style.cssText = 'position:fixed;bottom:8px;left:12px;z-index:99999;padding:6px 10px;background:#142139;color:#fff;border:1px solid #9ac7ff;font:12px system-ui;pointer-events:none';
    document.body.append(label);
  }, caption);
  try { await page.screenshot({ path: resolve(output, filename) }); }
  finally { await page.locator('#source-fixture-label').evaluate(node => node.remove()); }
}
try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && document.querySelector('#load-overlay').hidden);
  const fixtures = await page.evaluate(async () => {
    await WorkspaceContext.setScope('dataset', { page: 'explore', animate: false });
    await loadExplorerAnalytics(explorerKey(), workspaceScope(), backendFilters()).promise;
    const sources = [
      { name: 'application.log', path: 'C:\\Investigação\\Servidor A\\Pasta com espaços\\application.log', format: 'jsonl', bytes: 1762345, count: 58210, start: 1700000000000, end: 1700003600000, unparsed: 0, sampled: 20, undated: 0 },
      { name: 'application.log', path: 'D:\\Evidências\\Pacote com espaços.zip!/servidor-b/subpasta/área <literal>/application.log', format: 'jsonl', bytes: 982345, count: 21810, start: 1700000000000, end: 1700003600000, unparsed: 0, sampled: 20, undated: 3 },
      { name: 'audit.log', path: 'C:\\Investigação\\Host de destino\\' + 'segmento-longo-'.repeat(8) + '\\audit.log', format: 'text', bytes: 3456, count: 110, start: null, end: null, unparsed: 3, sampled: 20, undated: 110 },
    ];
    const originalApi = api, originalConfirm = window.confirm, originalCustody = recordCustody;
    window.__sourceTest = { sources, copied: [], calls: [], configs: [], hashes: [], custody: [], confirmations: [], copyFail: false, applied: [], originals: { api: originalApi, confirm: originalConfirm, recordCustody: originalCustody } };
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { if (__sourceTest.copyFail) throw Error('clipboard denied'); __sourceTest.copied.push(text); } } });
    window.confirm = text => { __sourceTest.confirmations.push(text); return false; };
    recordCustody = (artifact, hashes) => { __sourceTest.custody.push({ id: artifact.id, hashes }); return false; };
    api = async (command, args = {}, options = {}) => {
      if (!['list_sources', 'source_hashes', 'get_ts_config'].includes(command)) return originalApi(command, args, options);
      __sourceTest.calls.push({ command, args: structuredClone(args) });
      if (command === 'list_sources') return structuredClone(__sourceTest.sources);
      return new Promise((resolve, reject) => { (command === 'source_hashes' ? __sourceTest.hashes : __sourceTest.configs).push({ args: structuredClone(args), resolve, reject }); });
    };
    await Workspace.showPage('sources');
    return sources;
  });
  assert.equal(await rows.count(), fixtures.length);

  phase = 'before/after resting geometry and themes';
  for (const width of [1440, 1024]) for (const theme of ['dark', 'light']) {
    await page.setViewportSize({ width, height: 960 });
    await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme); await settle();
    const after = await page.evaluate(() => {
      const table = document.querySelector('.sources-table'), row = table.querySelector('tbody tr'), trigger = row.querySelector('.source-path-toggle');
      const badge = table.querySelector('tbody tr:last-child .source-reading-cell .tag'), style = getComputedStyle(badge);
      const cell = badge.closest('td'), cellStyle = getComputedStyle(cell);
      return { rowHeight: row.getBoundingClientRect().height, tableWidth: table.getBoundingClientRect().width, font: getComputedStyle(table).fontSize,
        readingBadge: { text: badge.textContent, boxes: badge.getClientRects().length, display: style.display, boxSizing: style.boxSizing,
          width: badge.getBoundingClientRect().width, available: cell.clientWidth - parseFloat(cellStyle.paddingLeft) - parseFloat(cellStyle.paddingRight), font: style.fontSize },
        nameHeight: trigger.getBoundingClientRect().height, actionHeight: row.querySelector('.source-menu-trigger').getBoundingClientRect().height,
        overflow: document.documentElement.scrollWidth > innerWidth, pathHidden: [...table.querySelectorAll('.source-path-details')].every(node => node.hidden) };
    });
    await screenshot(`source-actions-after-${theme}-${width}.png`, 'Fontes · Preview sintético · Controles atuais · Sem validação WebView');
    // Exact previous row structure/classes. No live handlers are attached to the
    // reconstruction; restore the production DOM immediately after measuring it.
    await page.evaluate(() => {
      const current = document.querySelector('.sources-table'), before = document.createElement('table'); before.className = 'ws-table'; before.id = 'source-before';
      before.innerHTML = `<thead><tr><th></th><th>Arquivo</th><th>Formato</th><th class="num">Tamanho</th><th class="num">Eventos</th><th>Leitura</th><th class="num">Ação</th></tr></thead><tbody>${__sourceTest.sources.map((source, i) => `<tr><td><input class="source-check" type="checkbox" checked></td><td><strong>${esc(source.name)}</strong><div class="pattern-meta" title="${esc(source.path)}">${source.start != null ? `${esc(fmtTs(source.start))} — ${esc(fmtTs(source.end))}` : 'Sem horário reconhecido'}</div><div class="source-actions"><button class="text-button">Data/hora</button><button class="text-button">SHA-256</button></div></td><td><span class="tag">${esc(source.format)}</span></td><td class="num">${source.bytes >= 1e6 ? `${(source.bytes / 1e6).toFixed(1)} MB` : `${fmtNum(Math.ceil(source.bytes / 1000))} KB`}</td><td class="num">${fmtNum(source.count)}</td><td>${source.unparsed ? `<span class="tag">${source.unparsed}/${source.sampled} não interpretados na amostra</span>` : '<span class="tag good">Disponível</span>'}${source.undated ? `<div class="pattern-meta">${fmtNum(source.undated)} sem horário</div>` : ''}</td><td class="num"><button class="icon-btn danger source-remove-btn" title="Remover da análise"><i class="fas fa-trash-can"></i></button></td></tr>`).join('')}</tbody>`;
      current.before(before); current.hidden = true; document.querySelector('.source-table-scroll').classList.remove('source-table-scroll'); document.querySelector('.source-toolbar').classList.remove('source-toolbar');
    });
    const before = await page.locator('#source-before').evaluate(table => ({ rowHeight: table.querySelector('tbody tr').getBoundingClientRect().height,
      tableWidth: table.getBoundingClientRect().width, font: getComputedStyle(table).fontSize, overflow: document.documentElement.scrollWidth > innerWidth }));
    await screenshot(`source-actions-before-${theme}-${width}.png`, 'Fontes · Preview sintético · Layout anterior reconstruído com os mesmos dados');
    await page.evaluate(() => { document.querySelector('#source-before').remove(); document.querySelector('.sources-table').hidden = false; document.querySelector('.sources-table').parentElement.classList.add('source-table-scroll'); document.querySelector('.source-controls').classList.add('source-toolbar'); });
    assert.equal(after.font, before.font); assert.equal(after.overflow, false); assert.equal(after.pathHidden, true);
    assert.equal(after.readingBadge.text, '3/20 não interpretados na amostra');
    assert.equal(after.readingBadge.boxes, 1); assert.equal(after.readingBadge.display, 'inline-block');
    assert.equal(after.readingBadge.boxSizing, 'border-box'); assert.ok(after.readingBadge.width <= after.readingBadge.available + 1);
    assert.ok(after.actionHeight >= 32 && after.nameHeight >= 28);
    assert.ok(after.rowHeight < before.rowHeight, `${width}/${theme}: compact rows must actually save vertical space`);
    results.visuals.push({ width, theme, before, after, savedHeight: before.rowHeight - after.rowHeight });
  }

  phase = '640px access through a keyboard-scrollable table';
  await page.setViewportSize({ width: 640, height: 960 }); await settle();
  const narrow = await page.locator('.source-table-scroll').evaluate(node => ({ clientWidth: node.clientWidth, scrollWidth: node.scrollWidth, overflowX: getComputedStyle(node).overflowX }));
  assert.ok(narrow.scrollWidth > narrow.clientWidth); assert.equal(narrow.overflowX, 'auto');
  await button(0).focus();
  assert.ok(await page.locator('.source-table-scroll').evaluate(node => node.scrollLeft > 0), 'focused menu trigger scrolls into the reachable viewport');
  await page.keyboard.press('Enter'); await menu.waitFor({ state: 'visible' });
  const menuBox = await menu.boundingBox(); assert.ok(menuBox.x >= 0 && menuBox.x + menuBox.width <= 640);
  await page.keyboard.press('Escape'); await focusIs(button(0));
  await name(0).focus(); await page.keyboard.press('Enter');
  assert.equal(await path(0).inputValue(), fixtures[0].path); await path(0).focus(); await page.keyboard.press('Control+A');
  assert.equal(await path(0).evaluate(node => node.value.slice(node.selectionStart, node.selectionEnd)), fixtures[0].path);
  await screenshot('source-actions-narrow-640.png', 'Fontes · Preview sintético · Tabela rolável por teclado em 640px');
  await name(0).click(); await page.setViewportSize({ width: 1024, height: 960 });
  await page.locator('.source-table-scroll').evaluate(node => { node.scrollLeft = 0; }); await settle();
  results.narrow = { ...narrow, menuReachable: true, pathSelectable: true };

  phase = 'direct disclosure, exact paths and selected-source preservation';
  await check(1).uncheck();
  const selection = await check(1).isChecked(); assert.equal(selection, false);
  const callsBefore = await page.evaluate(() => __sourceTest.calls.length);
  for (const index of [0, 1, 2]) {
    await name(index).focus(); await page.keyboard.press(index === 1 ? 'Space' : 'Enter');
    assert.equal(await name(index).getAttribute('aria-expanded'), 'true'); assert.equal(await path(index).inputValue(), fixtures[index].path);
    await path(index).focus(); await page.keyboard.press('Control+A');
    assert.equal(await path(index).evaluate(node => node.value.slice(node.selectionStart, node.selectionEnd)), fixtures[index].path, 'full path is selectable without ellipsis');
    const nativeKeys = await path(index).evaluate(node => {
      let syntheticContext = 0;
      const observe = event => { if (!event.isTrusted) syntheticContext++; };
      node.addEventListener('contextmenu', observe);
      const states = ['F10', 'ContextMenu'].map(key => {
        const event = new KeyboardEvent('keydown', { key, shiftKey: key === 'F10', bubbles: true, cancelable: true });
        node.dispatchEvent(event); return { key, prevented: event.defaultPrevented };
      });
      node.removeEventListener('contextmenu', observe);
      return { states, syntheticContext };
    });
    assert.deepEqual(nativeKeys, { states: [{ key: 'F10', prevented: false }, { key: 'ContextMenu', prevented: false }], syntheticContext: 0 });

    await rows.nth(index).getByRole('button', { name: 'Copiar caminho', exact: true }).click();
    assert.equal(await page.evaluate(() => __sourceTest.copied.at(-1)), fixtures[index].path);
    await name(index).click(); assert.equal(await path(index).isVisible(), false);
  }
  assert.equal(await check(1).isChecked(), false); assert.equal(await page.evaluate(() => __sourceTest.calls.length), callsBefore, 'disclosure/copy cannot query native sources');

  phase = 'shared keyboard menu, one layer of Escape, right click and Tab';
  await button(1).focus(); await page.keyboard.press('Shift+F10'); await menu.waitFor({ state: 'visible' });
  assert.equal(await menu.getAttribute('aria-label'), 'Ações da fonte application.log'); await focusIs(menu.getByRole('menuitem').first());
  await page.keyboard.press('End'); await focusIs(menu.getByRole('menuitem', { name: 'Remover da análise', exact: true }));
  await page.keyboard.press('Home'); await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter');
  await menu.waitFor({ state: 'hidden' }); await focusIs(button(1)); assert.equal(await page.evaluate(() => __sourceTest.copied.at(-1)), fixtures[1].path);
  await name(1).click({ button: 'right' }); await menu.waitFor({ state: 'visible' });
  await page.keyboard.press('Escape'); await focusIs(name(1)); assert.equal(await name(1).getAttribute('aria-expanded'), 'false');
  await button(0).press('Enter'); await menu.waitFor({ state: 'visible' }); await page.keyboard.press('Tab'); await focusIs(check(1));
  await page.keyboard.press('Shift+F10'); await menu.waitFor({ state: 'visible' });
  await page.keyboard.press('Escape'); await focusIs(check(1)); assert.equal(await check(1).isChecked(), false);
  await button(0).click(); await page.locator('#ws-title').click(); await menu.waitFor({ state: 'hidden' });
  results.keyboard = { namedMenu: true, exactPathCopy: true, rightClickNameReturn: true, checkboxKeyboardReturn: true, nativePathKeyboardMenuNotIntercepted: true, escapeSingleLayer: true, tabAfterCaller: true, outsideClick: true };

  phase = 'confirmation retained and selection filter exactness';
  await selectAction(1, 'Remover da análise'); assert.equal(await rows.count(), fixtures.length);
  assert.match(await page.evaluate(() => __sourceTest.confirmations.at(-1)), /Remover "application.log" da análise/);
  assert.equal(await check(1).isChecked(), false);
  const selectionFilter = await page.evaluate(() => {
    // The click closes over the existing function; capture its backend effect
    // through the workspace's refresh-independent filter state immediately.
    document.querySelector('#ws-source-apply').click();
    return structuredClone(state.filters.find(item => item.column === 'caminho'));
  });
  assert.equal(selectionFilter.op, 'regex');
  const selected = new RegExp(selectionFilter.value);
  assert.equal(selected.test(fixtures[0].path), true); assert.equal(selected.test(fixtures[1].path), false); assert.equal(selected.test(fixtures[2].path), true);
  await page.waitForFunction(() => document.querySelector('#events-table').getAttribute('aria-busy') !== 'true');
  await page.evaluate(() => Workspace.showPage('sources'));

  phase = 'timestamp newest request and stale source response';
  await selectAction(0, 'Data/hora'); await page.waitForFunction(() => __sourceTest.configs.length === 1);
  await selectAction(1, 'Data/hora'); await page.waitForFunction(() => __sourceTest.configs.length === 2);
  await page.evaluate(() => __sourceTest.configs[1].resolve(null)); await page.locator('#ts-modal').waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(() => state.tsEditingPath), fixtures[1].path);
  await page.locator('#ts-close').click(); await page.evaluate(() => __sourceTest.configs[0].resolve(null)); await settle();
  assert.equal(await page.locator('#ts-modal').isVisible(), false);
  await selectAction(0, 'Data/hora'); await page.waitForFunction(() => __sourceTest.configs.length === 3);
  await page.evaluate(async () => { await Workspace.showPage('sources'); __sourceTest.configs[2].resolve(null); }); await settle();
  assert.equal(await page.locator('#ts-modal').isVisible(), false);
  await selectAction(0, 'Data/hora'); await page.waitForFunction(() => __sourceTest.configs.length === 4);
  await page.locator('#ws-source-config').click();
  assert.equal(await page.locator('#workspace-home').isVisible(), false);
  // The timestamp draft lives in its closed modal; mark it to detect any late write.
  await page.evaluate(() => { document.querySelector('#ts-clock').value = '123'; });
  await page.locator('#file-format').focus();
  await page.evaluate(() => __sourceTest.configs[3].resolve({ clock_adjustment_ms: 999000, sources: [] })); await settle();
  assert.equal(await page.locator('#ts-modal').isVisible(), false); assert.equal(await page.locator('#ts-clock').inputValue(), '123'); await focusIs(page.locator('#file-format'));
  await page.evaluate(() => Workspace.showPage('sources'));
  results.timestamp = { configurationNavigationKeepsDraft: true, exactPaths: await page.evaluate(() => __sourceTest.configs.map(item => item.args.path)), newestOnly: true, replacementIgnored: true };

  phase = 'hash progress, duplicate suppression, own/package copy and stale completion';
  await selectAction(1, 'Calcular SHA-256'); await page.waitForFunction(() => __sourceTest.hashes.length === 1);
  await button(0).click(); assert.equal(await menu.getByRole('menuitem', { name: 'Calculando SHA-256…', exact: true }).getAttribute('aria-disabled'), 'true');
  await page.keyboard.press('Escape'); assert.equal(await page.evaluate(() => __sourceTest.hashes.length), 1);
  await page.evaluate(() => {
    __sourceTest.hashes[0].resolve([
      { path: __sourceTest.sources[0].path, name: 'application.log', bytes: 1762345, sha256: 'a'.repeat(64) },
      { path: __sourceTest.sources[1].path, name: 'application.log', bytes: 982345, sha256: 'b'.repeat(64), origin: 'extraído' },
      { path: __sourceTest.sources[1].path.split('!/')[0], name: 'Pacote com espaços.zip', bytes: 2000000, sha256: 'c'.repeat(64) },
    ]);
  });
  await rows.nth(1).locator('.hash-copy').first().waitFor({ state: 'visible' }); assert.equal(await rows.nth(1).locator('.hash-copy').count(), 2);
  await rows.nth(1).locator('.hash-copy').last().click(); assert.equal(await page.evaluate(() => __sourceTest.copied.at(-1)), 'c'.repeat(64));
  assert.equal(await rows.nth(1).locator('.source-action-status').isVisible(), false);
  // Reset only these benign fixture acknowledgements through production cleanup,
  // then exercise real copy controls with a deterministic burst before capture.
  await page.evaluate(() => {
    for (const [node, record] of toastFeedback) if (['Caminho copiado.', 'SHA-256 copiado.'].includes(record.message)) removeToastFeedback(node);
  });
  for (let i = 0; i < 4; i++) await rows.nth(1).getByRole('button', { name: 'Copiar caminho', exact: true }).click();
  for (let i = 0; i < 3; i++) await rows.nth(1).locator('.hash-copy').last().click();
  await settle();
  const copyFeedback = await page.evaluate(() => [...document.querySelectorAll('#toast-feedback-area .toast')]
    .filter(node => ['Caminho copiado.', 'SHA-256 copiado.'].includes(node.querySelector('.toast-message')?.textContent))
    .map(node => ({ text: node.querySelector('.toast-message').textContent, count: node.querySelector('.toast-repeat-count').textContent,
      role: node.getAttribute('role'), passive: node.classList.contains('toast-passive') })));
  assert.deepEqual(copyFeedback, [
    { text: 'Caminho copiado.', count: '×4', role: 'status', passive: true },
    { text: 'SHA-256 copiado.', count: '×3', role: 'status', passive: true },
  ]);
  results.copyFeedback = copyFeedback;
  await screenshot('source-actions-path-and-hashes-1024.png', 'Fontes · Preview sintético · Caminho e SHA-256 sob demanda');
  await selectAction(0, 'Calcular SHA-256'); await page.waitForFunction(() => __sourceTest.hashes.length === 2);
  const custodyBefore = await page.evaluate(() => __sourceTest.custody.length);
  await page.evaluate(async () => { await Workspace.showPage('sources'); __sourceTest.hashes[1].resolve([{ path: __sourceTest.sources[0].path, name: 'stale', bytes: 1, sha256: 'd'.repeat(64) }]); }); await settle();
  assert.equal(await page.locator('.sources-table .hash-copy').count(), 0); assert.equal(await page.evaluate(() => __sourceTest.custody.length), custodyBefore);
  results.hashes = { oneRequestAtATime: true, exactArchiveMemberAndPackage: true, staleReplyIgnored: true };

  phase = 'logical focus after replacement and stale menu action';
  await button(1).focus(); await page.keyboard.press('Enter'); await menu.waitFor({ state: 'visible' });
  await page.evaluate(() => Workspace.showPage('sources'));
  await page.keyboard.press('Escape'); await focusIs(button(1));
  await button(1).focus(); await page.keyboard.press('Enter');
  const copiesBefore = await page.evaluate(() => __sourceTest.copied.length);
  await page.evaluate(() => Workspace.showPage('sources'));
  await menu.getByRole('menuitem', { name: 'Copiar caminho', exact: true }).click();
  assert.equal(await page.evaluate(() => __sourceTest.copied.length), copiesBefore);
  await page.evaluate(() => { __sourceTest.copyFail = true; });
  await selectAction(0, 'Copiar caminho'); await page.getByRole('alert').filter({ hasText: 'Não foi possível copiar. Selecione o texto e tente novamente.' }).waitFor({ state: 'visible' });
  await name(0).click(); assert.equal(await path(0).inputValue(), fixtures[0].path);
  results.recovery = { logicalFocusAfterRerender: true, staleMenuRejected: true, clipboardErrorKeepsSelectablePath: true };

  assert.deepEqual(errors, []); results.ok = true;
  writeFileSync(resolve(output, 'source-actions.json'), JSON.stringify(results, null, 2)); console.log(JSON.stringify(results, null, 2));
} catch (error) { await captureFailure(page, 'source-actions', error, { phase, errors, results }); throw error; }
finally { await browser.close(); }
