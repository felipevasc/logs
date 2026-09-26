/* Triage, search language, entity pivots, palette, lanes and Case intel in the preview. */
import assert from "node:assert/strict";
import { chromium } from "playwright";
const url = process.argv[2] || "http://127.0.0.1:4173";
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
try {
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && state.loaded && !state.loadOverlay);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { animate: false }));

  // Summary: an episode opens exactly the records of its detections.
  await page.evaluate(() => Workspace.showPage("summary"));
  await page.waitForSelector(".sec-episode");
  const episode = await page.evaluate(() => ({ title: document.querySelector(".sec-episode h3")?.textContent, markers: document.querySelectorAll(".sec-markers .sec-marker, .sec-markers button").length }));
  assert.ok(episode.title);
  await page.locator(".sec-episode").first().hover();
  await page.locator(".sec-episode [data-act='records']").first().click();
  await page.waitForFunction(() => document.body.dataset.page === "explore" && state.filters.some(f => f.label) && state.rows.length > 0 && state.rows.every(r => r.code === "4625" || r.code === "4624"));
  const scoped = await page.evaluate(() => ({ chip: [...document.querySelectorAll(".chip")].map(c => c.textContent).join(" | "), total: state.total, codes: [...new Set(state.rows.map(r => r.code))] }));
  assert.match(scoped.chip, /Episódio: /);
  assert.ok(scoped.codes.every(code => code === "4625" || code === "4624"), JSON.stringify(scoped.codes));
  results.episodeRecords = `${scoped.total} registros do episódio “${episode.title}”`;

  // Search language: incomplete input keeps the previous results; a valid query applies.
  await page.evaluate(() => { state.filters = []; renderChips(); });
  const search = page.locator("#quick-search");
  await search.fill("@action:logon @outcome:failure");
  await search.dispatchEvent("input");
  await page.waitForFunction(() => state.rows.length > 0 && state.rows.every(r => r.code === "4625"));
  const failures = await page.evaluate(() => state.total);
  await search.fill("@user:(ana");
  await search.dispatchEvent("input");
  await page.waitForTimeout(400);
  assert.equal(await page.evaluate(() => state.total), failures, "an invalid expression never replaces the active one");
  assert.ok(await page.locator(".query-error").isVisible());
  await search.fill("");
  await search.focus();
  await page.keyboard.type("@user:", { delay: 20 });
  await page.waitForFunction(() => { const counts = [...document.querySelectorAll(".query-suggest:not([hidden]) .query-option small")]; return counts.length > 1 && counts.every(c => /\d/.test(c.textContent)); });
  const options = await page.locator(".query-suggest .query-option span").allTextContents();
  assert.ok(options.length > 1 && !options.includes("(vazio)"), JSON.stringify(options));
  await page.keyboard.press("Escape");
  await search.fill("");
  await search.dispatchEvent("input");
  results.search = `falhas de logon: ${failures}; sugestões: ${options.length}`;

  // Event detail: normalized action and entities.
  await page.evaluate(() => { state.quick = "code:4625"; $("#quick-search").value = state.quick; state.page = 0; return Workspace.showPage("explore"); });
  await page.waitForFunction(() => state.rows.length && state.rows[0].code === "4625");
  await page.locator("#events-table tbody tr").first().click();
  await page.waitForSelector(".insight-block .insight-action");
  const insight = await page.evaluate(() => ({ action: document.querySelector(".insight-action")?.textContent, chips: document.querySelectorAll(".insight-entities .entity-chip").length }));
  assert.match(insight.action, /Autenticação/);
  assert.ok(insight.chips >= 1, "entities of the record are one click away");
  results.insights = insight;
  await page.keyboard.press("Escape");
  await page.evaluate(() => { state.quick = ""; $("#quick-search").value = ""; });

  // Command palette.
  await page.keyboard.press("Control+k");
  await page.waitForSelector(".palette-overlay:not([hidden])");
  await page.keyboard.type("forca bruta");
  const palette = await page.locator(".palette-item span").allTextContents();
  assert.ok(palette.some(t => /força bruta/i.test(t)), JSON.stringify(palette));
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".palette-overlay").isHidden(), true);
  results.palette = palette.length;

  // Timeline lanes stay off until asked, then align with the chart.
  await page.evaluate(() => Workspace.showPage("case-timeline"));
  await page.waitForSelector("[data-lanes]");
  assert.equal(await page.locator("#tl-swims").isHidden(), true);
  await page.click("[data-lanes]");
  await page.locator(".ctx-menu .ctx-item", { hasText: /^Origem$/ }).click();
  await page.waitForSelector(".tl-swim .tl-swim-strip");
  const lanes = await page.locator(".tl-swim").count();
  const strip = await page.locator(".tl-swim-strip").first().boundingBox();
  await page.mouse.click(strip.x + strip.width * 0.5, strip.y + 5);
  const selection = await page.evaluate(() => document.querySelector("#tl-selected-band")?.style.left);
  assert.ok(lanes >= 2 && selection);
  results.lanes = lanes;

  // Case: pivots on the case timeline, hypotheses and report synthesis.
  await page.evaluate(() => WorkspaceContext.setScope("case", { animate: false }));
  await page.evaluate(() => Workspace.showPage("case-timeline"));
  await page.waitForSelector(".ct-entry");
  await page.locator(".ct-entry").first().click({ button: "right" });
  const menu = await page.locator(".ctx-menu .ctx-item span").allTextContents();
  assert.ok(menu.includes("Contexto em todas as fontes (±5 min)"), JSON.stringify(menu));
  await page.keyboard.press("Escape");
  await page.mouse.click(5, 500);
  await page.evaluate(() => Workspace.showPage("evidence"));
  await page.locator(".intel-tab").nth(1).click();
  await page.locator(".intel-panel input").fill("Credencial usada após força bruta");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => activeCase().intel?.hypotheses?.length === 1);
  const facts = await page.evaluate(() => CaseIntel.synthesis(activeCase()));
  assert.equal(facts.hypotheses[0].text, "Credencial usada após força bruta");
  results.caseIntel = "pivôs, hipótese e síntese do relatório";

  // Rules: disabling a rule removes its detections from the triage.
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { animate: false }));
  await page.evaluate(async () => { await api("detection_settings_save", { settings: { disabled: ["auth.bruteforce.source", "auth.bruteforce.success"], suppress: [], threats: true } }); Security.invalidate(); });
  const after = await page.evaluate(async () => (await Security.get({ force: true })).detections.length);
  assert.equal(after, 0);
  await page.evaluate(() => api("detection_settings_save", { settings: { disabled: [], suppress: [], threats: true } }));
  results.rules = "regras desativadas não geram detecções";

  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ ...results, errors }, null, 2));
} finally {
  await browser.close();
}
