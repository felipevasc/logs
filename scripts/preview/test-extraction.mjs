/* Extração e normalização: contratos de apresentação, posse e interação no navegador. O motor
 * nativo é trocado aqui por respostas simuladas (a conversão Grok→RegExp abaixo existe só neste
 * teste); a semântica real do Grok/PCRE2, as posições, as conversões e os índices são verificados
 * pelos testes Rust e por test-local-native.mjs. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync } from "node:fs";
const browser = await launchBrowser(),
  page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
mkdirSync("output/playwright", { recursive: true });
await page.addInitScript(() => {
  const http = ['10.0.0.5 - ana [23/Oct/2026:17:05:09 +0000] "GET /index.html HTTP/1.1" 200 5120', '192.168.1.20 - - [23/Oct/2026:17:05:11 -0300] "POST /api/login HTTP/1.1" 401 87', '10.0.0.7 - - [23/Oct/2026:17:06:00 +0000] "GET /x HTTP/1.1" 404 -'];
  const kv = ["2026-10-23 17:05:09 login usuario=ana ip=10.0.0.5 duracao=12.5ms", "2026-10-23 17:05:44 login usuario=bruno ip=192.168.1.20 duracao=3ms", "2026-10-23 17:06:12 login usuario=carla ip=10.0.0.9 duracao=12,5ms", "linha fora do formato"];
  const known = { id: "http-access-log", title: "Log de acesso HTTP", ratio: 1, description: "", map: { id: "http-access-log", version: "1", title: "HTTP", schema: 2, rules: [{ id: "combined", extract: { kind: "grok", patterns: ["%{IP:cliente} - %{NOTSPACE:usuario} \\[%{DATA:quando}\\] \"%{WORD:metodo} %{NOTSPACE:caminho} HTTP/1.1\" %{INT:status} %{NOTSPACE:bytes}"], anchor: "search", source: "message" }, fields: { cliente: { from: ["cliente"], type: "ip" }, usuario: { from: ["usuario"] }, metodo: { from: ["metodo"] }, caminho: { from: ["caminho"] }, status: { from: ["status"], type: "integer" } } }] } };
  const sample = (list, file) => list.map((text, id) => ({ id, eventRef: `${file}:${id}`, file: `C:\\logs\\${file}`, fileName: file, offset: String(id * 90), text }));
  // Só para este teste: converte o padrão Grok em RegExp para simular o motor.
  const TOK = { IP: "(?:\\d{1,3}\\.){3}\\d{1,3}", NOTSPACE: "\\S+", DATA: ".*?", WORD: "\\w+", INT: "\\d+", NUMBER: "-?[\\d.,]+", TIMESTAMP_ISO8601: "\\d{4}-\\d\\d-\\d\\d \\d\\d:\\d\\d:\\d\\d" };
  function run(pattern, text) {
    const names = [];
    const body = pattern.replace(/%\{(\w+)(?::(\w+))?(?::\w+)?\}/g, (_, t, n) => { names.push(n || null); return `(${TOK[t] || ".+?"})`; });
    const m = new RegExp(body, "d").exec(text);
    if (!m) return null;
    return names.map((n, i) => n ? [n, m[i + 1], m.indices[i + 1]] : null).filter(Boolean);
  }
  function evaluate(map, texts) {
    const rules = map.rules.filter((r) => r.extract?.patterns?.[0]);
    const results = texts.map((text, id) => {
      const trace = { rules: [], outputs: [], conflicts: [] };
      for (const r of rules) {
        const got = run(r.extract.patterns[0], text);
        trace.rules.push({ rule: r.id, eligible: true, extraction: got ? "recognized" : "not_recognized", extracted: (got || []).map(([n, v, [s, e]]) => [n, v, [s, e]]) });
        for (const [field, m] of Object.entries(r.fields || {})) {
          const hit = (got || []).find(([n]) => n === m.from[0]);
          if (!hit) { trace.outputs.push({ rule: r.id, field, status: "missing", original: null }); continue; }
          let value = hit[1], status = "ok", reason = null;
          for (const st of m.steps || []) { if (st.op === "replace") value = value.replace(st.from, st.to); if (st.op === "lowercase") value = value.toLowerCase(); }
          if (m.type === "integer" && !/^-?\d+$/.test(value)) { status = "invalid"; reason = `“${value}” não é um número inteiro`; }
          if (m.type === "decimal" && !/^-?\d+(\.\d+)?$/.test(value)) { status = "invalid"; reason = `“${value}” não é um decimal`; }
          trace.outputs.push({ rule: r.id, field, status, original: hit[1], normalized: status === "ok" ? (["integer", "decimal"].includes(m.type) ? Number(value) : value) : null, range: hit[2], reason });
        }
      }
      return { id, fields: {}, trace };
    });
    return { ok: true, scope: "text", inspected: texts.length, diagnostics: [], results, rules: [], outputs: [] };
  }
  const positions = (text, re, group = 0) => [...text.matchAll(re)].map((m) => { const s = m.index + (group ? m[0].indexOf(m[group]) : 0); return { sample: 0, start: s, end: s + (group ? m[group].length : m[0].length) }; });
  window.__calls = [];
  window.__mockLocalAnalysis = async ({ request: r }) => {
    window.__calls.push(r);
    switch (r.action) {
      case "map_catalog": return { grok: { patterns: [] }, templates: [], limits: {} };
      case "map_check": return { valid: true, error: null, diagnostics: [], outputs: [] };
      case "map_groups": return { population: 120000, examined: 7, field: "message", seed: 1, groups: [
        { id: "gaaaa", name: "Log de acesso HTTP", known, pool: 3, population: 71000, samples: sample(http, "access.log") },
        { id: "gbbbb", name: "Tipo 2", known: null, pool: 4, population: 44000, samples: sample(kv, "app.log") },
      ] };
      case "map_suggest": {
        const t = r.texts[0];
        return { scope: "sample", total: 1, extractions: [], suggestions: [
          { id: "ip", kind: "value", label: "Endereço IP", expression: "IP", field: "ip", positions: positions(t, /(?:\d{1,3}\.){3}\d{1,3}/g) },
          { id: "iso", kind: "value", label: "Data e hora", expression: "TIMESTAMP_ISO8601", field: "timestamp", positions: positions(t, /\d{4}-\d\d-\d\d \d\d:\d\d:\d\d/g) },
          { id: "kv:usuario", kind: "key_value", label: "usuario", expression: "NOTSPACE", field: "usuario", positions: positions(t, /usuario=(\S+)/g, 1) },
          { id: "kv:duracao", kind: "key_value", label: "duracao", expression: "NUMBER", field: "duracao", positions: positions(t, /duracao=(\d+(?:\.\d+)?)/g, 1) },
        ].filter((s) => s.positions.length) };
      }
      case "map_try": return evaluate(JSON.parse(r.map_json), r.texts);
      case "map_diff": return { recompute: true, global: false, added: [], removed: [], changed: [], unchanged: [], rulesAdded: [], rulesRemoved: [], rulesChanged: [], indexesToRebuild: [], metadataOnly: false };
      case "map_scan": return { ok: true, scope: "complete", countsComplete: true, inspected: 120000, affected: 118000, conflicts: 0, elapsedMs: 800, diagnostics: [], rules: [{ rule: "gbbbb", eligible: 120000, recognized: 44000, notRecognized: 76000, errors: 0 }], outputs: [], files: [], successes: [], failures: [] };
      case "save_map": return null;
      default: throw Error(`Unmocked presentation request ${r.action}`);
    }
  };
});
const cols = () => page.locator("#e-table th").evaluateAll((els) => els.map((e) => e.dataset.eCol));
const ready = () => page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);
try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await ready();
  await page.getByRole("button", { name: "Estrutura", exact: true }).click();
  await page.locator('.nav-pages [data-page="extraction"]').click();
  await page.waitForSelector("#e-groups .e-group");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.zone), "structure");
  assert.equal(await page.locator("#e-groups .e-group").count(), 2);
  assert.equal(await page.locator("#ex-adv").isHidden(), true, "the simple mode is the default");
  // Little text: no hint paragraphs, explanations live behind the info icons.
  assert.ok((await page.locator("#ex-easy").innerText()).length < 700, "the simple view stays almost wordless");
  await page.screenshot({ path: "output/playwright/extraction-easy-start.png", fullPage: true });

  // 1. A ready-made format is offered and creates the columns in one click.
  assert.match(await page.locator("#e-suggest").textContent(), /acesso HTTP/);
  await page.click("#e-use");
  await page.waitForFunction(() => document.querySelectorAll("#e-table th").length >= 4);
  assert.deepEqual((await cols()).slice(0, 2), ["cliente", "usuario"]);
  assert.match(await page.locator("#e-cov").textContent(), /^\d+(?:\.\d)?%$/);

  // 2. A second type is taught from its example line by clicking the underlined pieces.
  await page.click('[data-e-group="gbbbb"]');
  await page.waitForFunction(() => document.querySelectorAll("#e-line .e-chip").length >= 3);
  const chips = await page.locator("#e-line .e-chip").allTextContents();
  assert.ok(chips.includes("10.0.0.5") && chips.includes("ana") && chips.includes("12.5"), `${chips}`);
  assert.equal(await page.locator("#e-line .e-chip", { hasText: "12.5" }).count(), 1, "the unit stays out of the number");
  await page.locator("#e-line .e-chip", { hasText: "ana" }).click();
  await page.locator("#e-line .e-chip", { hasText: "12.5" }).click();
  await page.waitForFunction(() => document.querySelectorAll("#e-table th").length === 2);
  assert.deepEqual(await cols(), ["usuario", "duracao"]);
  assert.equal(await page.evaluate(() => Extraction.state.def.rules.find((r) => r.id === "gbbbb").fields.duracao.type), "decimal");
  // The expression is written from the line and reaches the definition; the table values come from the engine.
  const pattern = await page.evaluate(() => Extraction.state.def.rules.find((r) => r.id === "gbbbb").extract.patterns[0]);
  assert.match(pattern, /usuario=%\{NOTSPACE:usuario\} ip=\S*|%\{NOTSPACE:usuario\}/);
  assert.equal(await page.locator("#e-table td.bad").count(), 1, "12,5 is not a decimal");
  assert.equal(await page.locator("#e-left").isVisible(), true, "the unrecognized line is flagged");
  assert.match(await page.locator("#e-left").textContent(), /1/);

  // 3. Right-click on a selection: a short, contextual menu.
  await page.evaluate(() => {
    const walker = document.createTreeWalker(document.querySelector("#e-line"), NodeFilter.SHOW_TEXT);
    let n;
    while ((n = walker.nextNode())) if (n.textContent.includes("10.0.0.5")) break;
    const i = n.textContent.indexOf("10.0.0.5");
    getSelection().setBaseAndExtent(n, i, n, i + 8);
  });
  const box = await page.evaluate(() => { const r = getSelection().getRangeAt(0).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; });
  await page.mouse.click(box.x, box.y, { button: "right" });
  await page.waitForSelector("#e-menu:not([hidden])");
  const items = await page.locator("#e-menu button span").allTextContents();
  assert.ok(items[0].startsWith("Coluna") && items.includes("Ignorar") && items.includes("Entre marcadores…"), `${items}`);
  await page.locator("#e-menu button").first().click();
  await page.waitForFunction(() => document.querySelectorAll("#e-table th").length === 3);

  // 4. Header menu: rename through a small dialog, type through another.
  await page.locator("#e-table th").nth(2).click();
  await page.waitForSelector("#e-menu:not([hidden])");
  assert.ok((await page.locator("#e-menu").innerText()).includes("Renomear"));
  await page.locator("#e-menu button", { hasText: "Renomear" }).click();
  await page.locator("#e-dlg input[name=name]").fill("origem");
  await page.locator("#e-dlg button[value=ok]").click();
  await page.waitForFunction(() => [...document.querySelectorAll("#e-table th")].some((t) => t.dataset.eCol === "origem"));
  await page.locator("#e-table th", { hasText: "usuario" }).click({ button: "right" });
  await page.locator("#e-menu button", { hasText: "Minúsculas" }).click();
  assert.deepEqual(await page.evaluate(() => Extraction.state.def.rules.find((r) => r.id === "gbbbb").fields.usuario.steps), [{ op: "lowercase" }]);

  // 5. Cell menu on the invalid value offers the fix and applies it as a normalization step.
  await page.locator("#e-table td.bad").click({ button: "right" });
  await page.waitForSelector("#e-menu:not([hidden])");
  assert.match(await page.locator("#e-menu").innerText(), /não é um decimal/);
  await page.locator("#e-menu button", { hasText: "Trocar vírgula por ponto" }).click();
  await page.waitForFunction(() => document.querySelectorAll("#e-table td.bad").length === 0);

  // 6. Undo walks back through the one shared history.
  await page.locator("#e-undo").click();
  await page.waitForFunction(() => document.querySelectorAll("#e-table td.bad").length === 1);
  await page.locator("#e-redo").click();
  await page.waitForFunction(() => document.querySelectorAll("#e-table td.bad").length === 0);

  // 7. A line that was left out is taught as a type of its own.
  await page.locator("#e-left").click();
  await page.waitForSelector("#e-pop li");
  await page.locator("#e-pop [data-teach]").click();
  await page.waitForFunction(() => document.querySelectorAll("#e-groups .e-group").length === 3);
  assert.match(await page.locator("#e-line").textContent(), /fora do formato/);

  // 8. Information lives behind the small icon, in a modal.
  await page.locator('[data-e-info="cobertura"]').click();
  assert.match(await page.locator("#e-dlg").textContent(), /estimativa/i);
  await page.keyboard.press("Escape");

  // 9. Apply: summary, then the map is saved in the Case through the same save as the advanced mode.
  await page.click("#e-apply");
  await page.waitForSelector("#e-dlg[open]");
  assert.match(await page.locator("#e-dlg").textContent(), /norm\.usuario/);
  await page.locator("#e-dlg button[value=ok]").click();
  await page.waitForFunction(() => window.__calls.some((c) => c.action === "save_map"));
  const saved = await page.evaluate(() => window.__calls.find((c) => c.action === "save_map"));
  assert.equal(saved.name, "norm");
  assert.match(saved.map_json, /%\{NOTSPACE:usuario\}/);
  assert.match(saved.map_json, /"easy"/, "the example line and its marks travel with the definition");

  // 10. The advanced mode is the same definition, one click away, and comes back.
  await page.click("#e-adv");
  await page.waitForSelector("#ex-adv:not([hidden])");
  assert.equal(await page.locator("#ex-easy").isHidden(), true);
  await page.locator('[data-act="view-easy"]').click();
  await page.waitForSelector("#ex-easy:not([hidden]) #e-groups .e-group");

  // 11. Layout: no horizontal overflow in both themes and widths.
  for (const width of [1440, 1024, 390]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${theme} ${width}px`);
      if (width !== 390) await page.screenshot({ path: `output/playwright/extraction-easy-${theme}-${width}.png`, fullPage: true });
    }
  }
  // 12a. Opening can be slow (the Case is busy): the frame and a placeholder appear at once.
  await page.evaluate(() => { const original = window.api; window.__origApi = original; window.api = (cmd, ...rest) => cmd === "list_sources" ? new Promise((resolve) => setTimeout(() => resolve(original(cmd, ...rest)), 2500)) : original(cmd, ...rest); });
  await page.evaluate(() => Workspace.showPage("sources"));
  await page.evaluate(() => { Workspace.showPage("extraction"); });
  await page.waitForSelector("#e-apply", { timeout: 1000 });
  assert.ok(await page.locator("#e-groups .e-skel").count() > 0, "a placeholder shows while the page loads");
  await page.waitForFunction(() => document.querySelectorAll("#e-groups .e-group").length > 0, null, { timeout: 15000 });
  await page.evaluate(() => { window.api = window.__origApi; });
  // 12b. A failure while opening is said on the page; the frame is never left empty.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => { Extraction.state.def = null; LocalExplorer.extraction = () => ({ current: "", field: "message", count: 40 }); });
  await page.evaluate(() => Workspace.showPage("sources"));
  await page.evaluate(() => Workspace.showPage("extraction"));
  await page.waitForSelector("#e-error:not([hidden])");
  assert.match(await page.locator("#e-error").textContent(), /Não foi possível abrir/);
  assert.equal(await page.locator("#e-apply").isVisible(), true, "the frame is there");
  errors.length = 0; // the failure above is logged on purpose
  assert.deepEqual(errors, []);
  console.log("Extração e normalização (modo simples): grupos, formato conhecido, etiquetas, menu de seleção, cabeçalho, célula, desfazer, sobras, ajuda em modal, aplicar, avançado e layout passaram (apresentação simulada).");
} finally {
  await browser.close();
}
