/* Interface scale (automatic, settings, shortcuts, persistence) and layouts that must fit small windows. */
import assert from "node:assert/strict";
import { chromium } from "playwright";
const url = process.argv[2] || "http://127.0.0.1:4173";
const browser = await chromium.launch();
const errors = [], results = {};
try {
  // A 2000×1054 screen at 150% system scaling: the window is 1333×703 CSS pixels.
  const context = await browser.newContext({ viewport: { width: 1333, height: 703 }, deviceScaleFactor: 1.5 });
  const page = await context.newPage();
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && state.loaded && !state.loadOverlay && window.UiScale);
  assert.equal(await page.evaluate(() => UiScale.current()), 0.85, "automatic scale zooms out on a small high-DPI window");
  await page.keyboard.press("Control+Minus");
  assert.equal(await page.evaluate(() => UiScale.current()), 0.8);
  assert.equal(await page.evaluate(() => localStorage.getItem("li-ui-scale")), "0.8");
  await page.reload();
  await page.waitForFunction(() => window.UiScale && state.loaded);
  assert.equal(await page.evaluate(() => UiScale.current()), 0.8, "a chosen size survives a restart");
  await page.keyboard.press("Control+0");
  assert.deepEqual(await page.evaluate(() => [UiScale.current(), UiScale.setting()]), [0.85, "auto"]);
  results.shortcuts = "Ctrl − / Ctrl 0 e persistência";

  await page.evaluate(() => openSettings());
  await page.waitForSelector("#settings-pane-interface [data-scale]");
  await page.locator('#settings-pane-interface [data-scale="1"]').click();
  assert.equal(await page.evaluate(() => UiScale.current()), 1);
  assert.equal(await page.locator('#settings-pane-interface [data-scale="1"]').getAttribute("aria-checked"), "true");
  await page.locator('#settings-pane-interface [data-scale="auto"]').click();
  assert.equal(await page.evaluate(() => UiScale.setting()), "auto");
  await page.evaluate(() => { document.querySelector("#settings-modal").hidden = true; });
  results.settings = "Configurações → Interface";

  // Layouts at 100% in the same small window.
  await page.evaluate(async () => { await Workspace.showPage("explore"); switchTab("group"); });
  await page.waitForFunction(() => document.querySelectorAll("#group-table tbody tr").length > 1);
  const widths = await page.evaluate(() => [document.querySelector("#group-table").getBoundingClientRect().width, document.querySelector("#group-table").parentElement.clientWidth]);
  assert.ok(widths[0] >= widths[1] - 2, `group table fills its panel: ${widths}`);
  await page.evaluate(() => Workspace.showPage("compare"));
  await page.waitForSelector(".compare-form input");
  const overflow = await page.evaluate(() => { const box = document.querySelector(".compare-form").getBoundingClientRect(); return [...document.querySelectorAll(".compare-form input, .compare-form button")].filter(e => e.getBoundingClientRect().right > box.right + 1).length; });
  assert.equal(overflow, 0, "period fields stay inside the card");
  results.layout = "Resumir usa a largura toda; Comparar sem estouro";

  // A regular screen without system scaling keeps text legible.
  const plain = await browser.newPage({ viewport: { width: 1024, height: 680 }, deviceScaleFactor: 1 });
  await plain.goto(url);
  await plain.waitForFunction(() => window.UiScale);
  assert.equal(await plain.evaluate(() => UiScale.current()), 0.9);
  const large = await browser.newPage({ viewport: { width: 1920, height: 1000 } });
  await large.goto(url);
  await large.waitForFunction(() => window.UiScale);
  assert.equal(await large.evaluate(() => UiScale.current()), 1);
  results.automatic = "85% (1333×703 @150%), 90% (1024×680 @100%), 100% (1920×1000)";

  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ ...results, errors }, null, 2));
} finally {
  await browser.close();
}
