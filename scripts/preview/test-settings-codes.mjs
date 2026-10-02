/* Settings catalog draft continuity and tab reflow in the real browser.
   Reads/saves use an explicitly synthetic controllable transport. No native/WebView claims. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser(), page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20000);
const errors = [], results = { evidence: 'Real browser keyboard, focus and geometry with synthetic catalog transport; no installed WebView or native zoom verification' };
let phase = 'startup'; page.on('pageerror', error => errors.push(error.message));
const tab = name => page.locator(`#settings-modal [data-settings-tab="${name}"]`);
const editor = page.locator('#codes-editor'), status = page.locator('#codes-status');
const usable = () => page.waitForFunction(() => !document.querySelector('#codes-editor').disabled && !document.querySelector('#codes-save').disabled);
const count = name => page.evaluate(name => window.__settingsFixture[name], name);
const ask = async (accept, action, match) => {
  const question = page.waitForEvent('dialog'); const acting = action(); const dialog = await question;
  assert.match(dialog.message(), match); await (accept ? dialog.accept() : dialog.dismiss()); await acting;
};
async function screenshot(name) {
  await page.evaluate(() => {
    const label = document.createElement('div'); label.id = 'settings-fixture-label'; label.textContent = 'Configurações · Preview sintético · Recuperação visível por fixture · Sem validação WebView';
    label.style.cssText = 'position:fixed;bottom:4px;left:8px;right:8px;z-index:99999;background:#142139;color:white;padding:4px;font:11px system-ui;pointer-events:none'; document.body.append(label);
  });
  try { await page.screenshot({ path: resolve(output, name) }); }
  finally { await page.locator('#settings-fixture-label').evaluate(node => node.remove()); }
}
try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);
  await page.evaluate(() => {
    window.__settingsOriginalApi = api;
    const fixture = window.__settingsFixture = { stored: '{}', reads: 0, saves: 0, readQueue: [], saveQueue: [], holdRead: false, holdSave: false };
    api = async (command, args, options) => {
      if (command === 'get_codes') {
        fixture.reads++;
        if (fixture.holdRead) return new Promise((resolve, reject) => fixture.readQueue.push({ resolve, reject }));
        return fixture.stored;
      }
      if (command === 'save_codes') {
        fixture.saves++;
        if (fixture.holdSave) return new Promise((resolve, reject) => fixture.saveQueue.push({ text: args.text, resolve: () => { fixture.stored = args.text; resolve(); }, reject }));
        fixture.stored = args.text; return;
      }
      return window.__settingsOriginalApi(command, args, options);
    };
  });
  phase = 'manual tab keyboard and draft continuity';
  await page.locator('#btn-settings').click();
  assert.equal(await tab('interface').evaluate(node => node === document.activeElement), true);
  await page.keyboard.press('ArrowRight'); assert.equal(await tab('codes').evaluate(node => node === document.activeElement), true);
  assert.equal(await count('reads'), 0, 'moving focus does not query a pane');
  await page.keyboard.press('Enter'); await usable();
  await page.keyboard.press('/'); assert.equal(await tab('codes').evaluate(node => node === document.activeElement), true);
  await page.locator('#codes-save').focus(); await page.keyboard.press('Tab');
  assert.equal(await page.locator('#settings-close').evaluate(node => node === document.activeElement), true);
  await page.keyboard.press('Shift+Tab'); assert.equal(await page.locator('#codes-save').evaluate(node => node === document.activeElement), true);
  await editor.fill('{"localDraft":1}');
  await tab('interface').click(); await tab('codes').click(); await tab('codes').click();
  assert.equal(await editor.inputValue(), '{"localDraft":1}'); assert.equal(await count('reads'), 1);
  assert.equal(await tab('codes').getAttribute('aria-selected'), 'true');
  results.tabDraft = 'repeated active-tab clicks and away/back keep unsaved JSON without extra reads';

  phase = 'deliberate reload and editing while reading';
  await ask(false, () => page.locator('#codes-reload').click(), /descartar.*não salvas/); assert.equal(await count('reads'), 1);
  await page.evaluate(() => { window.__settingsFixture.holdRead = true; });
  await ask(true, () => page.locator('#codes-reload').click(), /descartar.*não salvas/);
  await page.waitForFunction(() => window.__settingsFixture.readQueue.length === 1);
  await editor.fill('{"newerDraft":2}'); await page.evaluate(() => window.__settingsFixture.readQueue.shift().resolve('{"olderRead":1}'));
  await usable(); assert.equal(await editor.inputValue(), '{"newerDraft":2}'); assert.match(await status.textContent(), /mais recente foi mantida/);
  await page.evaluate(() => { window.__settingsFixture.holdRead = false; });

  phase = 'external change off-tab preserves draft';
  await tab('interface').click(); await page.evaluate(async () => { window.__settingsFixture.stored = '{"external":3}'; await handleMcpStateChanged('codes'); });
  await tab('codes').click(); assert.equal(await editor.inputValue(), '{"newerDraft":2}'); assert.match(await status.textContent(), /mudou fora/);
  await ask(false, () => page.locator('#codes-save').click(), /Salva[r].*substituirá/); assert.equal(await count('saves'), 0);
  await ask(true, () => page.locator('#codes-reload').click(), /descartar.*não salvas/); await usable();
  assert.equal(await editor.inputValue(), '{"external":3}');
  results.externalChange = 'MCP update is visible, dirty text retained, replacement only after deliberate approval';

  phase = 'save receipt cannot label later typing saved';
  await page.evaluate(() => { window.__settingsFixture.holdSave = true; });
  await editor.fill('{"submitted":4}'); await page.locator('#codes-save').click();
  await page.waitForFunction(() => window.__settingsFixture.saveQueue.length === 1);
  assert.equal(await page.locator('#codes-save').isDisabled(), true);
  assert.equal(await editor.evaluate(node => node === document.activeElement), true, 'focus stays in the editor before disabling Save');
  await editor.fill('{"later":5}'); await page.evaluate(() => window.__settingsFixture.saveQueue.shift().resolve()); await usable();
  assert.equal(await editor.inputValue(), '{"later":5}'); assert.match(await status.textContent(), /edição posterior ainda não foi salva/);
  await page.locator('#codes-save').click(); await page.waitForFunction(() => window.__settingsFixture.saveQueue.length === 1);
  await page.evaluate(() => window.__settingsFixture.saveQueue.shift().reject(Error('Falha recuperável de gravação'))); await usable();
  assert.equal(await editor.inputValue(), '{"later":5}'); assert.match(await status.textContent(), /Falha recuperável/);
  results.saveDraft = 'receipt marks submitted version only; failure retains editable JSON for retry';

  phase = 'Escape discard cancellation and late read after reopening';
  await editor.focus(); await ask(false, () => page.keyboard.press('Escape'), /não salvas/);
  assert.equal(await page.locator('#settings-modal').isVisible(), true); assert.equal(await editor.evaluate(node => node === document.activeElement), true);
  await ask(true, () => page.locator('#settings-close').click(), /não salvas/);
  assert.equal(await page.locator('#btn-settings').evaluate(node => node === document.activeElement), true);
  await page.evaluate(() => { window.__settingsFixture.holdRead = true; window.__settingsFixture.holdSave = false; });
  await page.locator('#btn-settings').click(); await tab('codes').click(); await page.waitForFunction(() => window.__settingsFixture.readQueue.length === 1);
  await page.locator('#settings-close').click(); await page.locator('#btn-settings').click(); await tab('codes').click();
  await page.waitForFunction(() => window.__settingsFixture.readQueue.length === 2);
  await page.evaluate(() => window.__settingsFixture.readQueue[1].resolve('{"newSession":6}')); await usable();
  await editor.fill('{"currentDraft":7}'); await page.evaluate(() => window.__settingsFixture.readQueue[0].resolve('{"staleSession":0}'));
  await page.waitForFunction(() => document.querySelector('#codes-editor').value === '{"currentDraft":7}');
  assert.equal(await editor.inputValue(), '{"currentDraft":7}');
  await page.evaluate(() => { window.__settingsFixture.readQueue = []; window.__settingsFixture.holdRead = false; });
  results.sessionIsolation = 'old read ignored after close/reopen; declined Escape retains current editor/focus';

  phase = 'pending save close and reopen';
  await page.evaluate(() => { window.__settingsFixture.holdSave = true; });
  await page.locator('#codes-save').click(); await page.waitForFunction(() => window.__settingsFixture.saveQueue.length === 1);
  const beforeRead = await count('reads');
  await ask(true, () => page.locator('#codes-cancel').click(), /gravação já enviada continuará/);
  await page.locator('#btn-settings').click(); await tab('codes').click();
  assert.equal(await editor.isDisabled(), true); assert.equal(await count('reads'), beforeRead);
  await page.evaluate(() => window.__settingsFixture.saveQueue.shift().resolve()); await usable();
  assert.equal(await editor.inputValue(), '{"currentDraft":7}'); assert.doesNotMatch(await status.textContent(), /Catálogo salvo/);
  results.pendingSave = 'close does not cancel submitted write; reopened read waits and does not reuse old success status';

  phase = 'tab strip within compact modal'; results.layouts = [];
  await page.evaluate(async () => { await UiScale.set(2, false); });
  results.scale = await page.evaluate(() => UiScale.status());
  assert.equal(results.scale.requested, 2); assert.equal(results.scale.applied, 1, 'preview does not apply native zoom');
  results.recoveryTab = 'Shown explicitly as a layout fixture; no recovery operations are invoked';
  for (const theme of ['dark', 'light']) for (const width of [1440, 1024, 320]) {
    await page.setViewportSize({ width, height: width === 1440 ? 960 : 720 });
    await page.evaluate(theme => { document.documentElement.dataset.theme = theme; }, theme);
    await tab('codes').click();
    await tab('recovery').evaluate(node => { node.hidden = false; });
    const layout = await page.locator('.settings-modal').evaluate(modal => {
      const bounds = modal.getBoundingClientRect(), strip = modal.querySelector('.settings-tabs'), body = modal.querySelector(':scope > .modal-body');
      const tabs = [...strip.querySelectorAll('button')].filter(button => !button.hidden).map(button => {
        const rect = button.getBoundingClientRect(); return { label: button.textContent.trim(), left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, font: parseFloat(getComputedStyle(button).fontSize), height: rect.height,
          hit: button.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2)) };
      });
      return { width: innerWidth, theme: document.documentElement.dataset.theme, modal: { left: bounds.left, right: bounds.right, bottom: bounds.bottom }, overflow: strip.scrollWidth > strip.clientWidth + 1,
        bodyOverflow: body.scrollWidth > body.clientWidth + 1, tabs, rows: new Set(tabs.map(tab => Math.round(tab.top))).size };
    });
    assert.equal(layout.overflow, false); assert.equal(layout.bodyOverflow, false); assert.ok(layout.modal.left >= 0 && layout.modal.right <= width);
    for (const button of layout.tabs) { assert.ok(button.left >= layout.modal.left && button.right <= layout.modal.right); assert.ok(button.font >= 12 && button.height >= 36); assert.equal(button.hit, true, button.label); }
    if (width === 320) assert.ok(layout.rows > 1); else assert.equal(layout.rows, 1);
    await tab('codes').focus(); await page.keyboard.press('End');
    assert.equal(await tab('updates').evaluate(node => node === document.activeElement), true);
    await page.keyboard.press('Home'); assert.equal(await tab('interface').evaluate(node => node === document.activeElement), true);
    await page.keyboard.press('ArrowRight'); await page.keyboard.press('Enter');
    assert.equal(await tab('codes').getAttribute('aria-selected'), 'true');
    assert.equal(await editor.inputValue(), '{"currentDraft":7}');
    await tab('recovery').evaluate(node => { node.hidden = false; });
    await screenshot(`settings-codes-${theme}-${width}.png`); results.layouts.push(layout);
  }
  assert.deepEqual(errors, []); results.errors = errors;
  writeFileSync(resolve(output, 'settings-codes-results.json'), JSON.stringify(results, null, 2)); console.log(JSON.stringify(results, null, 2));
} catch (error) { await captureFailure(page, 'settings-codes', error, { phase, errors, results }); throw error; }
finally { await browser.close(); }
