// Isolated regression fixtures. Pass a preview URL to also verify the integrated assets.
import assert from "node:assert/strict";
import { readFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = path => readFileSync(resolve(root, path), "utf8");
const fallback = `${process.env.LOCALAPPDATA}/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe`;
const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || (existsSync(chromium.executablePath()) ? undefined : existsSync(fallback) ? fallback : undefined);
const browser = await chromium.launch({ executablePath });
const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, timezoneId: "America/Sao_Paulo" });
const errors = [];
page.on("pageerror", error => errors.push(error.message));
try {
  await page.setContent('<html data-theme="dark"><body style="padding:0"><main id="fixture" style="width:min(1100px,calc(100% - 40px));margin:20px auto"></main></body></html>');
  await page.addStyleTag({ content: ["frontend/styles.css", "frontend/workspace.css", "frontend/timeline-refinements.css"].map(read).join("\n") });
  await page.addScriptTag({ content: read("frontend/timeline.js") });
  await page.addScriptTag({ content: read("frontend/icon-picker.js") });
  await page.addScriptTag({ content: read("frontend/case-timeline.js") });
  await page.evaluate(async () => {
    window.fmtNum = value => Number(value).toLocaleString("pt-BR");
    window.esc = value => String(value).replace(/[&<>"']/g, character => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]);
    const start = Date.UTC(2026, 8, 22, 12);
    const counts = [2, 0, 0, 7, 0, 0, 0, 3];
    window.rangeData = { start, end: start + 7999, bucketMs: 1000, total: 12, errors: 1, warnings: 2,
      buckets: counts.map((count, index) => ({ timestamp: start + index * 1000, count, errors: index === 7 ? 1 : 0, warnings: index === 0 ? 2 : 0 })) };
    window.api = async () => window.rangeData;
    window.applied = null;
    window.volume = window.createTimelineView({ content: document.querySelector("#fixture"), getOverview: async () => ({ start, end: start + 7999, undated: 0 }), sourceKey: () => "fixture", filters: () => [], isActive: () => true, applyRange: (from, to) => { window.applied = { from, to }; } });
    await window.volume.load();
  });
  await page.locator("#tl-interactive").focus();
  await page.keyboard.press("End");
  assert.match(await page.locator("#tl-selection").innerText(), /3\s+eventos/);
  await page.locator('[data-tl="explore"]').click();
  assert.deepEqual(await page.evaluate(() => window.applied), { from: Date.UTC(2026, 8, 22, 12, 0, 7), to: Date.UTC(2026, 8, 22, 12, 0, 7, 999) });
  await page.getByRole("button", { name: /Maior pausa/ }).click();
  assert.match(await page.locator("#tl-selection").innerText(), /09:00:04.*09:00:06/s);
  await page.locator("#tl-interactive").focus();
  await page.keyboard.press("Home");
  await page.keyboard.press("Shift+ArrowRight");
  assert.equal(await page.locator("#tl-selected-band").evaluate(node => node.style.width), "25%");
  mkdirSync(resolve(root, "output/playwright"), { recursive: true });
  await page.screenshot({ path: resolve(root, "output/playwright/timeline-volume-regression.png") });
  await page.evaluate(async () => {
    window.rangeData = { start: 0, end: 0, bucketMs: 1, total: 1, errors: 0, warnings: 0,
      buckets: [{ timestamp: 0, count: 1, errors: 0, warnings: 0 }, { timestamp: 1, count: 0, errors: 0, warnings: 0 }] };
    window.volume.invalidate();
    await window.volume.load();
  });
  assert.equal(await page.locator(".tl-bin").count(), 1, "out-of-range buckets are omitted");
  await page.locator('[data-tl="explore"]').click();
  assert.deepEqual(await page.evaluate(() => window.applied), { from: 0, to: 0 });

  // Context changes retain independent zoom and cannot paint a superseded response.
  await page.evaluate(async () => {
    window.timelineScope = "dataset"; window.timelineRequests = [];
    window.WorkspaceContext = { scope: () => window.timelineScope };
    window.timelineCase = [{ timestamp: 20000 }, { timestamp: 21000 }];
    window.analyticsRequest = scope => ({ filters: [], ...(scope === "case" ? { caseEvents: window.timelineCase } : {}) });
    window.api = async (name, args) => {
      window.timelineRequests.push({ name, ...args });
      const isCase = Array.isArray(args.caseEvents);
      const counts = isCase ? [1, 1] : [2, 0, 0, 7, 0, 0, 0, 3];
      const origin = isCase ? 20000 : 0;
      const buckets = counts.map((count, index) => ({ timestamp: origin + index * 1000, count, errors: 0, warnings: 0 })).filter(bucket => bucket.timestamp >= args.start && bucket.timestamp <= args.end);
      const result = { start: args.start, end: args.end, bucketMs: 1000, total: buckets.reduce((sum, bucket) => sum + bucket.count, 0), errors: 0, warnings: 0, buckets };
      if (isCase && window.delayCaseTimeline) await new Promise(resolve => { window.releaseCaseTimeline = resolve; });
      return result;
    };
    window.scopedVolume = window.createTimelineView({ content: document.querySelector("#fixture"), getOverview: async () => window.timelineScope === "case" ? { start: 20000, end: 21999, undated: 0 } : { start: 0, end: 7999, undated: 0 }, sourceKey: () => window.timelineScope, filters: () => [], isActive: () => true, applyRange() {} });
    window.changeTimelineScope = async scope => { window.timelineScope = scope; document.dispatchEvent(new CustomEvent("workspace-context-change", { detail: { scope } })); await window.scopedVolume.load(); };
    await window.scopedVolume.load();
  });
  await page.locator('[data-tl="zoom"]').click();
  await page.waitForFunction(() => document.querySelector(".tl-summary-numbers b")?.textContent === "7");
  const datasetZoom = await page.locator(".tl-summary").innerText();
  await page.evaluate(() => window.changeTimelineScope("case"));
  assert.equal(await page.locator(".tl-summary-numbers b").first().innerText(), "2");
  assert.equal(await page.evaluate(() => window.timelineRequests.at(-1).caseEvents.length), 2);
  assert.ok(await page.evaluate(() => window.timelineRequests.every(request => request.bucketCount === 120)), "source and case requests remain bounded aggregates");
  await page.evaluate(() => window.changeTimelineScope("dataset"));
  assert.equal(await page.locator(".tl-summary").innerText(), datasetZoom, "returning to Análise preserves its zoom");
  assert.equal(await page.evaluate(() => window.timelineRequests.length), 3, "cached context restores without fetching all events");
  await page.evaluate(async () => {
    await window.changeTimelineScope("case"); window.scopedVolume.invalidate(); window.delayCaseTimeline = true;
    window.pendingCaseTimeline = window.scopedVolume.load();
  });
  await page.waitForFunction(() => typeof window.releaseCaseTimeline === "function");
  await page.evaluate(async () => { await window.changeTimelineScope("dataset"); window.releaseCaseTimeline(); await window.pendingCaseTimeline; });
  assert.equal(await page.locator(".tl-summary").innerText(), datasetZoom, "late case data cannot replace the source timeline");
  const savedVolume = await page.evaluate(() => window.scopedVolume.capture());
  assert.deepEqual(Object.keys(savedVolume).sort(), ["history", "range", "selected"]);
  await page.evaluate(async snapshot => {
    const fresh = window.createTimelineView({ content: document.querySelector("#fixture"), getOverview: async () => ({ start: 0, end: 7999, undated: 0 }), sourceKey: () => "reloaded-source", filters: () => [], isActive: () => true, applyRange() {} });
    fresh.restore(snapshot); await fresh.load();
  }, savedVolume);
  assert.equal(await page.locator(".tl-summary").innerText(), datasetZoom, "saved timeline configuration restores zoom without saved results");

  await page.evaluate(() => {
    const start = Date.UTC(2026, 8, 22, 12, 34, 56, 789);
    window.caseData = { id: "fixture", items: [{ id: "source", label: "Aplicação", rows: Array.from({ length: 20 }, (_, index) => ({ id: index, timestamp: start + index * 60000, name: `Evento ${index}`, message: `Detalhes ${index}` })) }],
      manual: [{ id: "milestone", start: start + 30000, end: start + 45876, name: "Marco preciso" }],
      timeline: { groups: [], annotations: [], edits: {}, layout: {}, compact: false, matrixZoom: 4 } };
    window.saved = 0;
    window.callbacks = { passes: () => true, detail() {}, bucket() {}, menu(x, y, items) { window.lastMenu = items; }, notify(message) { throw Error(message); }, createAt() {},
      save() { window.saved++; document.querySelector("#fixture").innerHTML = ""; window.renderCase(); } };
    window.renderCase = () => window.CaseTimeline.render(document.querySelector("#fixture"), window.caseData, "horizontal", window.callbacks);
    window.renderCase();
  });
  await page.locator('.ct-entry[data-id="e:source:0"]').click();
  await page.locator('.ct-entry[data-id="e:source:3"]').click({ modifiers: ["Shift"] });
  assert.equal(await page.locator(".ct-entry.selected").count(), 5, "Shift selects intervening milestone too");
  await page.locator('[data-ct-action="clear"]').click();
  await page.locator('.ct-entry[data-id="e:source:0"]').focus();
  await page.keyboard.press("ArrowDown");
  assert.equal(await page.evaluate(() => document.activeElement.dataset.id), "m:milestone");
  await page.keyboard.press("Enter");
  assert.equal(await page.locator('input[name="start"]').inputValue(), "2026-09-22T09:35:26.789");
  assert.equal(await page.locator('input[name="end"]').inputValue(), "2026-09-22T09:35:42.665");
  await page.locator('input[name="name"]').fill("Marco renomeado");
  await page.getByRole("button", { name: "Salvar", exact: true }).click();
  assert.equal(await page.evaluate(() => window.caseData.manual[0].start), Date.UTC(2026, 8, 22, 12, 35, 26, 789));
  assert.equal(await page.evaluate(() => window.caseData.manual[0].end), Date.UTC(2026, 8, 22, 12, 35, 42, 665));
  await page.evaluate(() => { document.querySelector(".ct-scroll").scrollLeft = 1000; });
  await page.locator('[data-ct-action="compact"]').click();
  await page.waitForFunction(() => document.querySelector(".ct-scroll").scrollLeft === 1000);
  await page.locator('[data-ct-action="note"]').click();
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".ct-editor-backdrop").isVisible(), false);
  assert.equal(await page.evaluate(() => document.activeElement.dataset.ctAction), "note");
  const entry = page.locator('.ct-entry[data-id="e:source:10"]');
  await entry.scrollIntoViewIfNeeded();
  const before = await entry.boundingBox();
  await page.mouse.move(before.x + 10, before.y + 10);
  await page.mouse.down();
  await page.mouse.move(before.x + 10, before.y + 60);
  await entry.dispatchEvent("pointercancel", { pointerId: 1 });
  await page.mouse.up();
  const after = await entry.boundingBox();
  assert.equal(after.y, before.y, "horizontal marks remain in their fixed occurrence row");
  assert.deepEqual(errors, []);
  await page.screenshot({ path: resolve(root, "output/playwright/timeline-regression.png") });
  await page.evaluate(() => {
    window.caseData.timeline.compact = false;
    window.caseData.timeline.groups = [{ id: "group", ids: ["e:source:0", "e:source:1"], name: "Grupo filtrado" }];
    window.caseData.timeline.annotations = [{ id: "note", anchor: "group", text: "Nota preservada", icon: "fa-comment", arrow: "forward" }];
    window.callbacks.passes = row => row.id !== 1;
    window.CaseTimeline.render(document.querySelector("#fixture"), window.caseData, "vertical", window.callbacks);
  });
  assert.equal(await page.locator(".ct-note").count(), 1, "group note survives filtering to one member");
  await page.locator(".ct-note").focus();
  await page.keyboard.press("Enter");
  assert.equal(await page.locator('textarea[name="text"]').inputValue(), "Nota preservada");
  await page.keyboard.press("Escape");
  await page.screenshot({ path: resolve(root, "output/playwright/timeline-vertical-regression.png") });
  assert.equal(await page.locator(".ct-axis-tick").first().evaluate(node => getComputedStyle(node).zIndex), "6", "vertical timestamps stay above crossing lines");
  await page.evaluate(() => {
    window.callbacks.passes = () => true;
    window.CaseTimeline.render(document.querySelector("#fixture"), window.caseData, "vertical", window.callbacks);
  });
  assert.equal(await page.locator(".ct-entry-count").first().innerText(), "2", "group count is a compact number without an × prefix");
  const countGeometry = await page.locator('.ct-entry[data-id="group"]').evaluate(node => {
    const badge = node.querySelector(".ct-entry-count").getBoundingClientRect(), title = node.querySelector("strong").getBoundingClientRect();
    return { separated: badge.bottom <= title.top || badge.left >= title.right || badge.right <= title.left };
  });
  assert.equal(countGeometry.separated, true, "group badge does not cover its title");
  await page.evaluate(() => {
    const start = Date.UTC(2026, 8, 22, 12);
    const events = [["a0", 0, "Autenticação concluída"], ["b0", 0, "Consulta ao catálogo"], ["a1", 60, "Autenticação concluída"], ["c0", 120, "Falha na conexão"], ["a2", 180, "Autenticação concluída"], ["b1", 300, "Consulta ao catálogo"]];
    window.caseData = { id: "matrix", items: [{ id: "svc", label: "API · produção", rows: events.map(([id, seconds, name]) => ({ id, timestamp: start + seconds * 1000, name, message: `${name} no serviço de aplicação` })) }],
      manual: [{ id: "deploy", start: start + 80000, end: start + 200000, name: "Atualização do serviço" }],
      timeline: { groups: [], annotations: [{ id: "n1", anchor: "e:svc:a1", text: "Confirmar relação entre a atualização e a falha de conexão.", icon: "fa-comment", arrow: "forward", vertical: { x: 80, y: 30 }, horizontal: { x: 60, y: 100 } }], edits: {}, layout: {}, compact: false } };
    window.renderCase = () => window.CaseTimeline.render(document.querySelector("#fixture"), window.caseData, "horizontal", window.callbacks);
    window.renderCase();
  });
  assert.equal(await page.locator(".ct-matrix-lane").count(), 4, "repeated occurrences share their row");
  assert.equal(await page.locator(".ct-matrix-dot").count(), 7, "every occurrence retains its timestamp mark");
  const matrix = await page.evaluate(() => {
    const point = id => { const rect = document.querySelector(`[data-marker-anchor="e:svc:${id}"]`).getBoundingClientRect(); return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 }; };
    return { first: point("a0"), sameTime: point("b0"), oneMinute: point("a1"), threeMinutes: point("a2"), scrollWidth: document.querySelector(".ct-scroll").scrollWidth, viewport: document.querySelector(".ct-scroll").clientWidth };
  });
  assert.equal(matrix.first.x, matrix.sameTime.x, "same timestamps align across rows");
  assert.equal(matrix.first.y, matrix.threeMinutes.y, "same occurrences share one row");
  assert.ok(Math.abs((matrix.oneMinute.x - matrix.first.x) / (matrix.threeMinutes.x - matrix.first.x) - 1 / 3) < .001, "horizontal spacing is proportional to elapsed time");
  assert.ok(matrix.scrollWidth <= matrix.viewport + 2, "the full period fits by default");
  await page.locator(".ct-lane-label").filter({ hasText: "Autenticação concluída" }).click();
  assert.equal(await page.locator(".ct-entry.selected").count(), 3, "one click selects an occurrence row");
  await page.locator('[data-ct-action="clear"]').click();
  await page.locator('[data-ct-zoom="in"]').click();
  await page.evaluate(() => { document.querySelector(".ct-scroll").scrollLeft = 400; });
  const sticky = await page.evaluate(() => {
    const viewport = document.querySelector(".ct-scroll").getBoundingClientRect(), label = document.querySelector(".ct-lane-label").getBoundingClientRect(), corner = document.querySelector(".ct-matrix-corner").getBoundingClientRect();
    return { viewport: viewport.left, label: label.left, corner: corner.left };
  });
  assert.equal(sticky.label, sticky.viewport, "occurrence labels remain visible while scrolling time");
  assert.equal(sticky.corner, sticky.viewport, "matrix corner follows the frozen labels");
  await page.locator('[data-ct-zoom="fit"]').click();
  await page.locator('.ct-link-hit[data-arrow="n1"]').waitFor();
  const arrowPoint = await page.locator('.ct-link-hit[data-arrow="n1"]').evaluate(path => { const point = path.getPointAtLength(path.getTotalLength() * .5).matrixTransform(path.getScreenCTM()); return { x: point.x, y: point.y }; });
  await page.mouse.click(arrowPoint.x, arrowPoint.y, { button: "right" });
  assert.equal(await page.evaluate(() => window.lastMenu[0].label), "Editar seta…", "right click targets the connector itself");
  await page.evaluate(() => window.lastMenu[0].onClick());
  assert.equal(await page.locator("#ct-editor-title").innerText(), "Editar seta");
  await page.locator('select[name="arrow"]').selectOption("both");
  await page.locator('select[name="lineStyle"]').selectOption("dashed");
  await page.locator('select[name="startSide"]').selectOption("bottom");
  await page.locator('select[name="endSide"]').selectOption("left");
  await page.getByRole("button", { name: "Salvar", exact: true }).click();
  await page.waitForFunction(() => document.activeElement?.dataset.arrow === "n1");
  assert.deepEqual(await page.evaluate(() => { const note = window.caseData.timeline.annotations[0]; return { arrow: note.arrow, lineStyle: note.lineStyle, endpoints: note.endpoints.matrix, vertical: note.vertical, horizontal: note.horizontal }; }),
    { arrow: "both", lineStyle: "dashed", endpoints: { start: "bottom", end: "left" }, vertical: { x: 80, y: 30 }, horizontal: { x: 60, y: 100 } }, "arrow options save without rewriting other orientation geometry");
  assert.equal(await page.locator(".ct-link-line").getAttribute("stroke-dasharray"), "6 5");
  await page.keyboard.press("Enter");
  assert.equal(await page.locator("#ct-editor-title").innerText(), "Editar seta", "keyboard opens arrow options");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Shift+F10");
  assert.equal(await page.evaluate(() => window.lastMenu[1].label), "Pontas da seta…");
  await page.evaluate(() => window.lastMenu.find(item => item.label === "Ajustar curva").onClick());
  assert.equal(await page.locator(".ct-curve-handle").count(), 1);
  assert.equal(await page.locator(".ct-endpoint-handle").count(), 2);
  const curve = await page.locator(".ct-curve-handle").boundingBox();
  await page.mouse.move(curve.x + curve.width / 2, curve.y + curve.height / 2);
  await page.mouse.down();
  await page.mouse.move(curve.x + curve.width / 2 + 40, curve.y + curve.height / 2 + 30);
  await page.locator(".ct-board").dispatchEvent("pointercancel", { pointerId: 1 });
  await page.mouse.up();
  assert.equal(await page.evaluate(() => window.caseData.timeline.annotations[0].curve?.matrix), undefined, "cancelling a curve drag restores its previous geometry");
  await page.locator(".ct-link-hit").focus();
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".ct-curve-handle").count(), 0);
  await page.locator(".ct-note").focus();
  await page.keyboard.press("Enter");
  assert.equal(await page.locator('select[name="arrow"]').count(), 0, "note editor focuses on note content");
  await page.locator('textarea[name="text"]').fill("Verificar se a falha acompanha a atualização do serviço.");
  await page.getByRole("button", { name: "Salvar", exact: true }).click();
  assert.equal(await page.evaluate(() => window.caseData.timeline.annotations[0].arrow), "both", "editing the note retains arrow choices");
  assert.equal(await page.evaluate(() => window.caseData.manual.length), 1, "arrow interactions do not create accidental milestones");
  await page.evaluate(() => { window.caseData.timeline.annotations[0].anchor = "a:e:svc:a1"; window.renderCase(); });
  assert.equal(await page.locator(".ct-note").count(), 1, "notes from automatic groups survive expanding repetitions");
  await page.evaluate(() => { window.caseData.timeline.annotations[0].anchor = "e:svc:a1"; window.renderCase(); });
  for (const width of [1024, 1200]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["dark", "light"]) {
      await page.evaluate(theme => { document.documentElement.dataset.theme = theme; window.renderCase(); }, theme);
      await page.locator(".ct-link-hit").waitFor();
      await page.screenshot({ path: resolve(root, `output/playwright/timeline-matrix-${theme}-${width}.png`) });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, `${theme} ${width}: no page overflow`);
    }
  }
  const matrixFixture = await page.evaluate(() => window.caseData);
  await page.evaluate(() => {
    window.caseData.items[0].rows.push(...Array.from({ length: 25 }, (_, index) => ({ id: `extra${index}`, name: `Etapa ${index}`, timestamp: window.caseData.items[0].rows[0].timestamp + index * 10000 })));
    window.renderCase(); document.querySelector(".ct-scroll").scrollTop = 160;
  });
  assert.equal(await page.evaluate(() => document.querySelector(".ct-matrix-ruler").getBoundingClientRect().top === document.querySelector(".ct-scroll").getBoundingClientRect().top), true, "time header stays visible while scrolling rows");
  await page.evaluate(() => { window.caseData = { id: "empty", items: [] }; window.renderCase(); });
  await page.getByRole("button", { name: "Criar marco", exact: true }).click();
  await page.locator('.ct-empty-form input[name="name"]').fill("Primeiro marco");
  await page.locator('.ct-empty-form input[name="start"]').fill("2026-09-22T10:11:12.345");
  await page.locator('.ct-empty-form button[type="submit"]').click();
  assert.equal(await page.locator(".ct-entry.is-manual").count(), 1);
  assert.equal(await page.evaluate(() => window.caseData.manual[0].start), Date.UTC(2026, 8, 22, 13, 11, 12, 345));
  let integrated = null;
  if (process.argv[2]) {
    const preview = await browser.newPage();
    preview.on("pageerror", error => errors.push(error.message));
    await preview.goto(process.argv[2]);
    await preview.waitForFunction(() => window.CaseTimeline && window.Workspace);
    integrated = await preview.evaluate(async () => ({ timeline: (await (await fetch("/case-timeline.js")).text()).includes("ct-matrix-ruler"), styles: (await (await fetch("/timeline-refinements.css")).text()).includes("ct-link-hit") }));
    assert.deepEqual(integrated, { timeline: true, styles: true }, "preview serves the matrix and arrow interaction assets");
    await preview.waitForFunction(() => state.loaded && state.rows.length && window.WorkspaceContext?.ready && !WorkspaceContext.changing && document.querySelector("#load-overlay").hidden);
    await preview.evaluate(() => Workspace.showPage("case-timeline"));
    await preview.locator("#tl-interactive").waitFor();
    assert.equal(await preview.evaluate(() => WorkspaceContext.scope()), "dataset", "source chronology never opens the case implicitly");
    assert.equal(await preview.locator(".tl-summary-numbers b").first().innerText(), "6.000");
    await preview.setViewportSize({ width: 1024, height: 768 });
    await preview.screenshot({ path: resolve(root, "output/playwright/timeline-context-analysis-dark.png") });
    await preview.evaluate(async fixture => {
      // Backend Event IDs are numeric; retain named fixture anchors as stable refs.
      for (const item of fixture.items) item.rows = item.rows.map((row, id) => ({ ...row, event_ref: String(row.id), id }));
      state.cases.cases.push(fixture); state.cases.active = fixture.id;
      if (window.WorkspaceContext) await WorkspaceContext.setScope("case");
      await Workspace.showPage("case-timeline");
    }, matrixFixture);
    await preview.locator('.small-seg [data-view="timeline"]').click();
    await preview.locator(".ct-matrix").waitFor();
    await preview.locator('.workspace-nav [data-page="summary"]').click();
    await preview.locator('.workspace-nav [data-page="case-timeline"]').click();
    await preview.locator(".ct-matrix").waitFor();
    assert.equal(await preview.evaluate(() => activeCase().workspace.analysisView), "timeline", "returning to timeline retains the last chosen orientation");
    const beforeSwitch = await preview.evaluate(() => JSON.stringify(activeCase().timeline));
    await preview.evaluate(() => WorkspaceContext.setScope("dataset", { page: "case-timeline", animate: false }));
    await preview.locator("#tl-interactive").waitFor();
    assert.equal(await preview.locator(".tl-summary-numbers b").first().innerText(), "6.000");
    await preview.evaluate(() => WorkspaceContext.setScope("case", { page: "case-timeline", animate: false }));
    await preview.locator(".ct-matrix").waitFor();
    assert.equal(await preview.evaluate(() => JSON.stringify(activeCase().timeline)), beforeSwitch, "source chronology does not mutate saved case notes or layout");
    await preview.evaluate(async () => { document.documentElement.dataset.theme = "light"; await Workspace.showPage("timeline"); });
    await preview.locator("#tl-interactive").waitFor();
    assert.equal(await preview.locator('.workspace-nav [data-page="timeline"]').count(), 0, "one timeline navigation entry");
    assert.equal(await preview.locator('.workspace-nav [data-page="case-timeline"]').getAttribute("aria-current"), "page");
    assert.equal(await preview.getByRole("group", { name: "Visualização da linha do tempo" }).count(), 1);
    assert.equal(await preview.locator(".tl-summary-numbers b").first().innerText(), await preview.evaluate(() => fmtNum(caseEvents().filter(event => event.timestamp != null).length)), "case Atividade counts only saved dated records");
    await preview.evaluate(() => Promise.allSettled(document.getAnimations().filter(animation => animation.animationName?.startsWith("context-")).map(animation => animation.finished)));
    await preview.waitForFunction(() => !state.activeOperation && !activity.count && document.querySelector("#activity-indicator").hidden);
    await preview.screenshot({ path: resolve(root, "output/playwright/timeline-context-case-light.png") });
    await preview.locator('[data-timeline-mode="horizontal"]').click();
    await preview.locator(".ct-matrix").waitFor();
    await preview.locator("#case-timeline-volume").click();
    await preview.locator("#tl-interactive").waitFor();
    await preview.locator('.workspace-nav [data-page="summary"]').click();
    await preview.locator('.workspace-nav [data-page="case-timeline"]').click();
    await preview.locator("#tl-interactive").waitFor();
    assert.equal(await preview.locator("#ws-title").innerText(), "Linha do tempo", "returning to the single timeline entry preserves Volume mode");
    await preview.locator('[data-timeline-mode="vertical"]').click();
    await preview.locator(".ct-vertical").waitFor();
    await preview.locator('.small-seg [data-view="timeline"]').click();
    await preview.locator(".ct-matrix").waitFor();
    await preview.evaluate(() => saveCases());
    await preview.reload();
    await preview.waitForFunction(() => activeCase()?.id === "matrix");
    await preview.evaluate(() => WorkspaceContext.setScope("case", { page: "case-timeline", animate: false }));
    await preview.locator('.workspace-nav [data-page="case-timeline"]').click();
    await preview.locator(".ct-matrix").waitFor();
    integrated.orientationPersisted = true;
    await preview.screenshot({ path: resolve(root, "output/playwright/timeline-matrix-integrated.png") });
    await preview.close();
  }
  assert.deepEqual(errors, []);
  writeFileSync(resolve(root, "output/playwright/timeline-validation.json"), JSON.stringify({ passed: true, matrix, sticky, countGeometry, integrated, errors, screenshots: [1024, 1200].flatMap(width => ["dark", "light"].map(theme => `timeline-matrix-${theme}-${width}.png`)) }, null, 2));
  console.log("PASS: volume boundaries, gaps, keyboard ranges, epoch, case selection, precise dates, matrix lanes and proportional time, frozen labels/header, zoom, direct arrow menu, keyboard arrow editor, note isolation, compact badge, dark/light responsive layouts");
} finally {
  await browser.close();
}
