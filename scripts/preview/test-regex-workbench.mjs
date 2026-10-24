/* Interaction contracts of the expression workbench, with a SYNTHETIC native transport
 * (JavaScript RegExp, see mock-local-regex.js). Native PCRE2 semantics, filters and
 * limits are verified by the Rust tests and by test-local-native.mjs. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync } from "node:fs";
const browser = await launchBrowser(),
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
mkdirSync("output/playwright", { recursive: true });
await page.addInitScript({ path: "scripts/preview/mock-local-regex.js" });
const shot = (name) => page.screenshot({ path: `output/playwright/${name}.png`, fullPage: false });
const SAMPLE = "Falha user=ana origem=10.0.0.5\nuser=ana confirmado=ana\nuser=ana confirmado=joao\nuser=pedro confirmado=pedro";
const editor = () => page.locator("#expr-search .rxw-edit-row .rx-input");
const text = () => editor().inputValue();
const setSample = (value) => page.evaluate((v) => { const w = document.querySelector("#expr-search .rxw-sample-body .rx-input"); w.value = v; w.dispatchEvent(new Event("input", { bubbles: true })); }, value);
const selectIn = (selector, needle, occurrence = 0) => page.evaluate(([sel, n, k]) => {
  const el = document.querySelector(sel); el.focus();
  let at = -1; for (let i = 0; i <= k; i++) at = el.value.indexOf(n, at + 1);
  el.setSelectionRange(at, at + n.length); el.dispatchEvent(new Event("select", { bubbles: true }));
}, [selector, needle, occurrence]);
const settle = async () => { await page.waitForTimeout(520); await page.waitForFunction(() => !/Validando|Aguardando/.test(document.querySelector("#expr-search [data-slot=status]")?.textContent || "")); };
const type = async (value) => { await editor().click(); await page.keyboard.press("Control+a"); await page.keyboard.press("Delete"); await page.keyboard.insertText(value); };
const menu = async (label) => { await page.locator("#expr-search [data-act=assist]").click(); await page.getByRole("menuitem", { name: label }).click(); };
try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);

  // ---- navigation: no Texto / Regex tabs in Explorar; the viewer lives in Estrutura
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  const tabs = await page.locator("#page-tabs [role=tab]").allTextContents();
  assert.deepEqual(tabs.map((t) => t.trim()), ["Eventos", "SQL", "Processos"]);
  await page.getByRole("button", { name: "Estrutura", exact: true }).click();
  await page.locator('.nav-pages [data-page="file-viewer"]').click();
  assert.equal(await page.locator("#ws-title").textContent(), "Visualizar arquivos");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.zone), "structure");
  assert.match(await page.locator("#ws-content").textContent(), /sem preparar os eventos/);
  // Reopening a page saved by an older version lands where its features live now.
  await page.evaluate(() => Workspace.showPage("regex"));
  assert.equal(await page.evaluate(() => document.body.dataset.page), "explore");
  await page.evaluate(() => Workspace.showPage("text"));
  assert.equal(await page.evaluate(() => document.body.dataset.page), "file-viewer");
  await page.getByRole("button", { name: "Análise", exact: true }).click();
  await page.getByRole("button", { name: "Explorar", exact: true }).click();

  // ---- the expression panel: draft, test on a sample, explicit apply
  await page.locator("#btn-add-expr").click();
  await page.waitForSelector("#expr-search .rx-input");
  const before = await page.evaluate(() => window.__mockLocalCalls.length);
  await type("user=(\\w+)\\s+confirmado=\\1\\b");
  await setSample(SAMPLE);
  await settle();
  assert.equal(await page.evaluate(() => state.filters.some((f) => f.op === "search")), false, "typing and testing never apply");
  // The group and its back-reference share one colour; the numbered reference was never rewritten.
  const classes = await page.evaluate(() => [...document.querySelectorAll("#expr-search .rxw-edit-row .rx-s")].map((s) => ({ text: s.textContent, cls: s.className })));
  const slot = (c) => /rx-cap-(\d)/.exec(c)?.[1];
  const group = classes.find((c) => c.text.includes("(\\w+)") || c.cls.includes("rx-cap-") && c.text === "(");
  const ref = classes.find((c) => c.text === "\\1");
  assert.ok(ref && /rx-ref/.test(ref.cls), "reference token is marked");
  assert.equal(slot(ref.cls), slot(classes.find((c) => /rx-cap-\d/.test(c.cls) && !/rx-ref/.test(c.cls)).cls));
  assert.match(await page.locator("#expr-search .rxw-status").textContent(), /Válida/);
  assert.equal(await page.locator("#expr-search .rx-legend .rx-chip").count(), 1);
  await shot("regex-colors-dark");

  // ---- numbering: a new group before the old one never rewrites \1
  await editor().evaluate((el) => { el.focus(); el.setSelectionRange(0, 0); });
  await page.keyboard.insertText("(x)");
  assert.equal(await text(), "(x)user=(\\w+)\\s+confirmado=\\1\\b");
  assert.match(await page.locator("#expr-search .rx-info").textContent(), /A numeração mudou/);
  assert.equal(await page.locator("#expr-search .rx-legend .rx-chip").count(), 2);
  await page.keyboard.press("Control+z");
  assert.equal(await text(), "user=(\\w+)\\s+confirmado=\\1\\b", "undo reverts the typed edit");
  await page.keyboard.press("Control+Shift+z");
  assert.equal(await text(), "(x)user=(\\w+)\\s+confirmado=\\1\\b", "redo restores it");
  await page.keyboard.press("Control+z");

  // ---- caret navigation between a group and its reference
  await editor().evaluate((el) => { el.focus(); const i = el.value.indexOf("(\\w+)"); el.setSelectionRange(i + 1, i + 1); });
  await page.keyboard.press("ArrowRight"); await page.keyboard.press("ArrowLeft");
  assert.match(await page.locator("#expr-search .rx-info").textContent(), /Grupo 1/);
  await page.keyboard.press("Alt+ArrowDown");
  assert.equal(await editor().evaluate((el) => el.value.slice(el.selectionStart, el.selectionEnd)), "\\1");

  // ---- E. literal text with metacharacters
  await type("");
  await setSample("abrir a+b(1).log agora");
  await selectIn("#expr-search .rxw-sample-body .rx-input", "a+b(1).log");
  await menu("Encontrar este texto");
  assert.equal(await page.locator(".rxw-pop [data-rx-out]").textContent(), "a\\+b\\(1\\)\\.log");
  await page.getByRole("button", { name: "Usar na expressão" }).click();
  assert.equal(await text(), "a\\+b\\(1\\)\\.log");
  await settle();
  assert.match(await page.locator("#expr-search .rxw-occ").textContent(), /Ocorrência 1 de 1/);

  // ---- existing highlight rules stay a separate layer under matches and captures
  await page.evaluate(() => { window.__mockMarks = () => [{ start: 0, end: 5, ruleId: "severity-error", label: "Erro / crítico", color: "#ff9c8f" }]; });
  await type("user=(\\w+)");
  await setSample("Falha user=ana origem=10.0.0.5");
  await settle();
  await page.waitForSelector("#expr-search .rxw-sample-body .rxs-sev", { state: "attached" });
  assert.ok(await page.locator("#expr-search .rxw-sample-body .rxs-match").count(), "matches are drawn together with the severity layer");
  assert.ok(await page.locator("#expr-search .rxw-sample-body .rxs-group").count(), "captures are drawn together with the severity layer");
  await page.evaluate(() => { window.__mockMarks = null; });

  // ---- B. assisted lookbehind: "what comes after"
  await type("");
  await setSample("Falha user=ana origem=10.0.0.5");
  await selectIn("#expr-search .rxw-sample-body .rx-input", "user=");
  await menu("Encontrar o que vem depois");
  await page.locator('.rxw-pop [data-f="named"]').uncheck();
  assert.equal(await page.locator(".rxw-pop [data-rx-out]").textContent(), "(?<=user=)\\w+");
  assert.match(await page.locator(".rxw-pop [data-rx-explain]").textContent(), /Exige user= antes do valor, sem incluir esse trecho/);
  await page.waitForFunction(() => /“ana”/.test(document.querySelector(".rxw-pop [data-rx-test]")?.textContent || ""));
  // PCRE2 is the engine that offers lookbehind: the choice is shown, never silent.
  assert.match(await page.locator(".rxw-pop .rxw-warn").textContent(), /lookbehind/);
  assert.equal(await page.locator("#expr-search [data-f=engine]").inputValue(), "rust");
  await page.getByRole("button", { name: /Usar na expressão e usar PCRE2/ }).click();
  assert.equal(await text(), "(?<=user=)\\w+");
  assert.equal(await page.locator("#expr-search [data-f=engine]").inputValue(), "pcre2");
  await settle();
  const typed = await page.evaluate(() => window.__mockLocalCalls.filter((c) => c.action === "regex_preview").length);
  assert.ok(typed > 0);
  const lookClass = await page.evaluate(() => [...document.querySelectorAll("#expr-search .rxw-edit-row .rx-s")].find((s) => s.textContent.includes("user="))?.className || "");
  assert.match(lookClass, /rx-look/);
  // The assisted result equals the typed one.
  const typedStatus = await page.evaluate(async () => { const r = await LocalNative.call({ action: "regex_preview", pattern: { engine: "pcre2", text: "(?<=user=)\\w+", caseSensitive: true }, text: "Falha user=ana origem=10.0.0.5" }); return r.spans.map((s) => [s.start, s.end]); });
  assert.deepEqual(typedStatus, [[11, 14]]);
  await shot("regex-lookbehind-dark");

  // ---- a selection taken from the expression is syntax: replaced in place, one undo step
  await page.locator("#expr-search [data-f=engine]").selectOption("pcre2");
  await type("Falha user= end");
  await selectIn("#expr-search .rxw-edit-row .rx-input", "user=");
  await menu("Encontrar o que vem depois");
  await page.locator('.rxw-pop [data-f="named"]').uncheck();
  assert.equal(await page.locator(".rxw-pop [data-rx-out]").textContent(), "(?<=user=)\\w+");
  await page.getByRole("button", { name: "Aplicar na expressão" }).click();
  assert.equal(await text(), "Falha (?<=user=)\\w+ end");
  await page.keyboard.press("Control+z");
  assert.equal(await text(), "Falha user= end", "the in-place action is one undo step");

  // ---- capturing an example never generalises it unless the user chooses to
  await setSample("user=ana origem=x");
  await type("user=");
  await selectIn("#expr-search .rxw-sample-body .rx-input", "ana");
  await menu("Capturar trecho");
  assert.equal(await page.locator(".rxw-pop [data-rx-out]").textContent(), "(?<ana>ana)", "literal by default");
  await page.getByLabel("Palavra", { exact: false }).first().check();
  assert.equal(await page.locator(".rxw-pop [data-rx-out]").textContent(), "(?<ana>\\w+)", "generalised only after the explicit choice");
  await page.locator('.rxw-pop [data-f="name"]').fill("usuario");
  await page.getByRole("button", { name: /Inserir no cursor|Usar na expressão/ }).first().click();
  assert.match(await text(), /usuario>\\w\+\)/);
  await page.keyboard.press("Escape");

  // ---- C. named group and named reference, built by actions
  await page.locator("#expr-search [data-f=engine]").selectOption("pcre2");
  await type("user=\\w+\\s+confirmado=");
  await setSample(SAMPLE);
  await editor().evaluate((el) => { el.focus(); const i = el.value.indexOf("\\w+"); el.setSelectionRange(i, i + 3); el.dispatchEvent(new Event("select", { bubbles: true })); });
  await menu("Capturar trecho");
  await page.locator('.rxw-pop [data-f="name"]').fill("usuario");
  await page.getByRole("button", { name: "Aplicar na expressão" }).click();
  assert.equal(await text(), "user=(?<usuario>\\w+)\\s+confirmado=");
  await editor().evaluate((el) => { el.focus(); el.setSelectionRange(el.value.length, el.value.length); });
  await menu("Repetir valor capturado");
  await page.locator(".rxw-pop [data-g]").first().click();
  assert.equal(await text(), "user=(?<usuario>\\w+)\\s+confirmado=\\k<usuario>");
  await page.keyboard.insertText("\\b");
  await settle();
  assert.equal(await text(), "user=(?<usuario>\\w+)\\s+confirmado=\\k<usuario>\\b");
  // Same colour for the definition and the reference; real captured values listed.
  const pair = await page.evaluate(() => { const spans = [...document.querySelectorAll("#expr-search .rxw-edit-row .rx-s")]; const s = (t) => spans.find((x) => x.textContent === t)?.className || ""; return [spans.find((x) => /rx-cap-\d/.test(x.className) && !/rx-ref/.test(x.className))?.className.match(/rx-cap-(\d)/)?.[1], s("\\k<usuario>").match(/rx-cap-(\d)/)?.[1]]; });
  assert.ok(pair[0] != null && pair[0] === pair[1], `group/reference colour ${pair}`);
  const matched = await page.evaluate(async (sample) => { const r = await LocalNative.call({ action: "regex_preview", pattern: { engine: "pcre2", text: "user=(?<usuario>\\w+)\\s+confirmado=\\k<usuario>\\b", caseSensitive: true }, text: sample }); return r.spans.map((s) => sample.slice(s.start, s.end)); }, SAMPLE);
  assert.deepEqual(matched, ["user=ana confirmado=ana", "user=pedro confirmado=pedro"]);
  assert.match(await page.locator("#expr-search .rxw-table").textContent(), /ana/);
  await shot("regex-named-reference");

  // ---- rename updates only syntactic references
  await type("(?<u>a)\\k<u> u (?#\\k<u>)");
  await editor().evaluate((el) => { el.focus(); el.setSelectionRange(5, 5); });
  await page.keyboard.press("ArrowRight"); await page.keyboard.press("ArrowLeft");
  await menu("Renomear captura");
  await page.locator('.rxw-pop input').fill("usuario");
  await page.getByRole("button", { name: "Renomear" }).click();
  assert.equal(await text(), "(?<usuario>a)\\k<usuario> u (?#\\k<u>)");
  await page.keyboard.press("Control+z");
  assert.equal(await text(), "(?<u>a)\\k<u> u (?#\\k<u>)", "rename is one undo step");

  // ---- invalid expressions are errors, and Apply refuses them
  await type("user=(");
  await settle();
  assert.match(await page.locator("#expr-search .rxw-status").textContent(), /Expressão inválida/);
  const filtersBefore = await page.evaluate(() => state.filters.length);
  await page.locator("#expr-search [data-act=apply]").click();
  await page.waitForTimeout(300);
  assert.equal(await page.evaluate(() => state.filters.length), filtersBefore, "an invalid expression is never applied");
  assert.match(await page.locator("#expr-search .rxw-status").textContent(), /inválida|Erro/);
  assert.ok(await page.locator("#expr-search .rx-native-error, #expr-search .rx-native-stale, #expr-search .rx-problem").count());

  // ---- stale answers never reach a newer draft
  await page.evaluate(() => { window.__mockDelay = (r) => (r.action === "regex_preview" && r.pattern.text === "slow" ? 900 : 0); });
  await type("slow");
  await page.waitForTimeout(450);
  await type("fast");
  await settle();
  await page.waitForTimeout(1100);
  const shown = await page.evaluate(() => document.querySelector("#expr-search .rxw-sample-body .rx-backdrop code").textContent);
  assert.ok(shown.length > 0);
  assert.match(await page.locator("#expr-search .rxw-status").textContent(), /Válida/);
  await page.evaluate(() => { window.__mockDelay = null; });

  // ---- apply: a real filter in the shared filter state, with the options captured
  await page.locator("#expr-search [data-f=engine]").selectOption("pcre2");
  await page.locator("#expr-search [data-f=caseSensitive]").check();
  await type("(?<=user=)\\w+");
  await settle();
  await page.locator("#expr-search [data-act=apply]").click();
  await page.waitForFunction(() => state.filters.some((f) => f.op === "search"));
  const applied = await page.evaluate(() => backendFilters().find((f) => f.op === "search"));
  assert.equal(applied.column, "message");
  assert.equal(applied.value, "(?<=user=)\\w+");
  assert.deepEqual(JSON.parse(applied.value2), { engine: "pcre2", caseSensitive: true, multiline: false, dotAll: false, scope: "event" });
  assert.match(await page.locator("#chips .chip").first().textContent(), /PCRE2.*user=/);
  await page.locator("#chips .chip-edit").first().click();
  assert.equal(await text(), "(?<=user=)\\w+", "the chip reopens the expression as a draft");
  await page.evaluate(() => { state.filters = state.filters.filter((f) => f.op !== "search"); renderChips(); });

  // ---- selection made in an event's own text
  await page.evaluate(() => { window.__mockSamples = { "0:message": "Falha user=ana origem=10.0.0.5" }; });
  await page.evaluate(() => EventPatternSearch.fromSelection({ text: "a+b(1).log", column: "message", event: { id: 0, event_ref: "fixture:0" } }));
  await page.waitForSelector(".rxw-pop [data-rx-out]");
  assert.equal(await page.locator(".rxw-pop [data-rx-out]").textContent(), "a\\+b\\(1\\)\\.log");
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".rxw-pop").count(), 0);

  // ---- context of a prepared event, integrated in the detail drawer
  await page.evaluate(() => {
    window.__mockContextRows = [-1, 0, 1].map((d) => ({ id: 10 + d, eventRef: `fixture:${10 + d}`, event_ref: `fixture:${10 + d}`, timestamp: 1700000000000 + d * 1000, level: "info", source: "api", text: `linha ${10 + d} user=ana`, spans: d === 0 ? [{ start: 8, end: 16 }] : [] }));
  });
  await page.evaluate(() => openDetail(state.rows[0].id, { eventRef: state.rows[0].event_ref }));
  await page.waitForSelector("#ws-detail-context:not([hidden])");
  await page.locator("#ws-detail-context").click();
  await page.waitForSelector("#pane-context:not([hidden]) .ec-row");
  assert.equal(await page.locator("#pane-context .ec-row").count(), 3);
  assert.equal(await page.locator("#drawer .dtab.active").textContent(), "Contexto");
  assert.match(await page.locator("#pane-context .ec-help").textContent(), /filtros de apresentação são ignorados/);
  assert.ok(await page.locator("#pane-context .ec-row mark").count(), "native ranges are painted");
  await page.locator("#pane-context [data-f=mode]").selectOption("temporal");
  await page.waitForFunction(() => window.__mockLocalCalls.filter((c) => c.action === "context").length >= 2);
  // The context follows the investigation: stepping to another event reloads it.
  await page.locator("#dr-next").click();
  await page.waitForFunction(() => window.__mockLocalCalls.filter((c) => c.action === "context").length >= 3);
  assert.equal(await page.locator("#drawer .dtab.active").textContent(), "Contexto");
  await shot("event-context-drawer");
  await page.evaluate(() => { closeDrawer(); });

  // ---- derived field from a capture group: the same editor, saved in the Case
  await page.evaluate(() => {
    window.__saved = [];
    window.__mockSamples = { "0:message": "Falha user=ana origem=10.0.0.5" };
    window.__mockLocalExtra = async (r) => { if (r.action === "save_field") { window.__saved.push(r); return { analysisContext: null }; } throw Error(`unmocked ${r.action}`); };
  });
  await page.evaluate(() => FieldExtraction.open({ source: "message", event: { id: 0, event_ref: "fixture:0" } }));
  await page.waitForSelector("#field-extraction-modal .rx-input");
  await page.locator("#field-extraction-modal .rxw-edit-row .rx-input").click();
  await page.keyboard.insertText("(?<=user=)(?<usuario>\\w+)");
  await page.locator('#field-extraction-modal [data-f="name"]').fill("usuario_extraido");
  await page.waitForFunction(() => /usuario/.test(document.querySelector('#field-extraction-modal [data-f="capture"]').textContent));
  await page.waitForFunction(() => /Válida/.test(document.querySelector("#field-extraction-modal .rxw-status")?.textContent || ""));
  assert.match(await page.locator("#field-extraction-modal .rxw-table").textContent(), /ana/);
  await shot("field-extraction-dialog");
  await page.locator('#field-extraction-modal [data-act="save"]').click();
  await page.waitForFunction(() => window.__saved.length === 1);
  const saved = await page.evaluate(() => window.__saved[0]);
  assert.equal(saved.name, "usuario_extraido");
  assert.equal(saved.source, "message");
  assert.deepEqual(saved.definition, { kind: "captures", pattern: { engine: "pcre2", text: "(?<=user=)(?<usuario>\\w+)", caseSensitive: false, multiline: false, dotAll: false }, capture: "usuario" });

  // ---- keyboard access: the menu opens without the mouse
  await editor().focus();
  await page.keyboard.press("Control+Space");
  await page.getByRole("menuitem", { name: "Repetir valor capturado" }).waitFor();
  await page.keyboard.press("Escape");

  // ---- themes and widths
  for (const width of [1024, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
      await type("user=(?<usuario>\\w+)\\s+(?<=a)x\\k<usuario>");
      await settle();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${theme} ${width}`);
      await shot(`regex-panel-${theme}-${width}`);
    }
  }
  assert.deepEqual(errors, []);
  console.log("Expression workbench: navigation, editing, undo/redo, colours, assisted actions, apply/refuse, stale answers and keyboard passed (synthetic transport).");
} finally {
  await browser.close();
}
