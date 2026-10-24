/* Extração e normalização: presentation, ownership and interaction contracts in the browser
 * preview. The native engine is replaced by canned answers here; Grok/PCRE2 semantics,
 * ranges, conversions and indexes are verified by the Rust tests and test-local-native.mjs. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync } from "node:fs";
const browser = await launchBrowser(),
  page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
mkdirSync("output/playwright", { recursive: true });
await page.addInitScript(() => {
  const line = (user, ip, ms) => `falha ${ip} usuario=${user} acao=login duracao=${ms}ms`;
  const texts = [line("ana", "10.0.0.5", "12.5"), line("bruno", "192.168.1.20", "3"), "linha sem o formato"];
  window.__calls = [];
  window.__mockLocalAnalysis = async ({ request: r }) => {
    window.__calls.push(r);
    const map = r.map_json ? JSON.parse(r.map_json) : null;
    const pattern = map?.rules?.[0]?.extract?.patterns?.[0] ?? "";
    const outputs = map ? Object.keys(map.rules[0]?.fields || {}).map((field) => ({ field, rule: map.rules[0].id })) : [];
    const evaluate = (list) => ({
      ok: true, scope: "sample", inspected: list.length, diagnostics: [],
      results: list.map((text, id) => {
        const hit = pattern.includes("%{WORD:usuario}") && text.includes("usuario=");
        const start = text.indexOf("usuario=") + 8, end = text.indexOf(" ", start);
        return { id, fields: hit ? { usuario: text.slice(start, end) } : {}, error: null, trace: {
          rules: [{ rule: "regra-1", eligible: true, extraction: hit ? "recognized" : "not_recognized", extracted: hit ? [["usuario", text.slice(start, end), [start, end]]] : [] }],
          outputs: hit ? [{ field: "usuario", rule: "regra-1", status: "ok", source: "extract:usuario", original: text.slice(start, end), normalized: text.slice(start, end), range: [start, end] }] : [],
          conflicts: [] } };
      }),
      rules: [{ rule: "regra-1", eligible: list.length, recognized: pattern.includes("%{WORD:usuario}") ? 2 : 0, notRecognized: 1, errors: 0 }],
      outputs: [],
    });
    switch (r.action) {
      case "map_catalog": return { grok: { patterns: [{ name: "IP", category: "Rede", description: "Endereço IP.", example: "10.0.0.5", definition: "x" }, { name: "WORD", category: "Texto", description: "Palavra.", example: "login", definition: "x" }] }, templates: [{ id: "t", title: "Modelo de teste", description: "Exemplo.", samples: [texts[0]], map: { id: "modelo", version: "1", title: "Modelo de teste", schema: 2, rules: [{ id: "regra-1", fields: { usuario: { from: ["usuario"], type: "text" } }, extract: { kind: "grok", patterns: ["usuario=%{WORD:usuario}"], anchor: "search", source: "message" } }] } }], limits: {} };
      case "map_check": return { valid: !pattern.includes("%{NOPE"), error: pattern.includes("%{NOPE") ? "Padrão desconhecido: NOPE" : null, diagnostics: pattern.includes("%{NOPE") ? [{ rule: "regra-1", code: "unknown", message: "Padrão desconhecido: NOPE", start: pattern.indexOf("%{NOPE"), end: pattern.indexOf("%{NOPE") + 7, alternative: 0 }] : [], outputs };
      case "map_sample": return { isSample: true, population: 120000, candidates: 3, examined: 3, withoutSubject: 0, field: "message", seed: r.seed ?? 1, strategy: "Amostra estratificada e repetível: 3 registros examinados.", distribution: [{ file: "C:\\logs\\a.log", fileName: "a.log", records: 120000, selected: 3 }], shapes: [{ shape: "a 0.0.0.0", count: 2 }],
        samples: texts.map((text, id) => ({ id, eventRef: `fx:${id}`, file: "C:\\logs\\a.log", fileName: "a.log", offset: String(id * 90), text, truncated: false, bytes: text.length, shape: "a", shapeRank: id === 2 ? 1 : 0, shapeCount: id === 2 ? 1 : 2 })) };
      case "map_suggest": return { scope: "sample", total: r.texts.length, extractions: [], suggestions: [{ id: "ip", kind: "value", label: "Endereço IP", expression: "IP", field: "ip", found: 2, total: r.texts.length, examples: ["10.0.0.5"], message: "Este trecho parece um endereço ip. Encontrado em 2 de 3 amostras.", positions: [{ sample: 0, start: 6, end: 14 }] }] };
      case "map_evaluate": return evaluate(texts.slice(0, r.samples.length));
      case "map_try": return evaluate(r.texts);
      case "map_scan": return { ok: true, scope: "complete", countsComplete: true, inspected: 120000, affected: 118000, conflicts: 0, elapsedMs: 900, diagnostics: [], rules: [{ rule: "regra-1", eligible: 120000, recognized: 118000, notRecognized: 2000, errors: 0, firstError: null, byAlternative: { 0: 118000 } }], outputs: [{ rule: "regra-1", field: "usuario", states: { ok: 118000, missing: 2000 } }], files: [{ file: "C:\\logs\\a.log", inspected: 120000, affected: 118000, recognized: 118000, notRecognized: 2000 }], successes: [{ id: 0, eventRef: "fx:0", file: "C:\\logs\\a.log", offset: "0", text: texts[0] }], failures: [{ id: 2, eventRef: "fx:2", file: "C:\\logs\\a.log", offset: "180", reason: "A regra regra-1 não reconheceu o texto.", text: texts[2] }] };
      case "map_diff": return { recompute: true, global: false, added: ["usuario"], removed: [], changed: [], unchanged: [], rulesAdded: [], rulesRemoved: [], rulesChanged: ["regra-1"], indexesToRebuild: [], metadataOnly: false };
      case "save_map": return null;
      default: throw Error(`Unmocked presentation request ${r.action}`);
    }
  };
});
const waitReady = () => page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);
try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await waitReady();
  // The old tab is gone from Explorar and the page lives in Estrutura.
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  assert.deepEqual((await page.locator("#page-tabs [role=tab]").allTextContents()).map((t) => t.trim()), ["Eventos", "SQL", "Processos"]);
  await page.getByRole("button", { name: "Estrutura", exact: true }).click();
  await page.locator('.nav-pages [data-page="extraction"]').click();
  await page.waitForSelector("#ex-left");
  assert.equal(await page.evaluate(() => document.body.dataset.page), "extraction");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.zone), "structure");
  assert.match(await page.locator("#ws-title").textContent(), /Extração e normalização/);
  assert.match(await page.locator(".ex-case").textContent(), /nunca são alterados/);

  // Samples: strategy and distribution are shown, the rare shape is flagged.
  await page.getByRole("button", { name: /Sortear amostras/ }).click();
  await page.waitForFunction(() => document.querySelectorAll("#ex-sample-list .ex-sample").length === 3);
  assert.match(await page.locator(".ex-strategy").textContent(), /Amostra estratificada/);
  assert.match(await page.locator("#ex-sample-list").textContent(), /formato raro/);
  assert.match(await page.locator("#ex-suggest").textContent(), /2 de 3 amostras/);
  assert.equal(await page.locator("[data-sel]").first().isDisabled(), true, "actions need a selection");

  // Select "ana" and extract it: the pattern is written to the definition and the chips follow.
  await page.evaluate(() => {
    const node = document.querySelector("#ex-sample-list .ex-sample-text").firstChild;
    const at = node.textContent.indexOf("ana");
    getSelection().setBaseAndExtent(node, at, node, at + 3);
  });
  await page.waitForFunction(() => !document.querySelector('[data-sel="extract"]').disabled);
  await page.locator('[data-sel="extract"]').click();
  await page.waitForFunction(() => document.querySelector("[data-grok]")?.value.includes("%{WORD:usuario}"));
  assert.equal(await page.locator(".ex-alt .ex-chip").count(), 1);
  await page.waitForFunction(() => document.querySelector("#ex-right-body")?.textContent.includes("Prévia rápida"));
  await page.waitForSelector("#ex-sample-list .ex-mark");
  const color = await page.locator("#ex-sample-list .ex-mark").first().evaluate((n) => n.style.getPropertyValue("--c"));
  assert.match(color, /^#[0-9a-f]{6}$/i);
  assert.match(await page.locator(".ex-values").textContent(), /usuario/);
  assert.equal(await page.evaluate(() => Extraction.state.def.rules[0].fields.usuario.from[0]), "usuario");

  // One source of truth: the JSON tab shows the same definition, and bad text is refused with a diagnostic.
  await page.getByRole("tab", { name: "Definição" }).click();
  assert.match(await page.locator("#ex-json").inputValue(), /%\{WORD:usuario\}/);
  await page.locator("#ex-json").fill("{ not json");
  await page.locator('[data-act="json-apply"]').click();
  assert.match(await page.locator("#ex-json-msg").textContent(), /recusado/);
  await page.evaluate(() => { Extraction.state.def.rules[0].extract.patterns[0] += " %{NOPE:x}"; });
  await page.locator("[data-grok]").fill("usuario=%{WORD:usuario} %{NOPE:x}");
  await page.waitForSelector(".ex-diag");
  assert.match(await page.locator(".ex-diag").textContent(), /NOPE/);
  assert.equal(await page.locator('[data-act="ex-save"]').isDisabled(), true, "an invalid definition cannot be saved");
  await page.locator("[data-grok]").fill("usuario=%{WORD:usuario}");
  await page.waitForFunction(() => !document.querySelector(".ex-diag"));

  // Undo and redo walk the history; the draft is kept apart in the Case preferences.
  await page.locator('[data-act="undo"]').click();
  assert.match(await page.locator("[data-grok]").inputValue(), /NOPE|%\{WORD:usuario\}/);
  assert.ok(await page.evaluate(() => Object.keys(LocalExplorer.extraction().drafts).length >= 1));
  await page.locator('[data-act="redo"]').click();

  // Complete verification is a separate, explicit scope.
  await page.getByRole("tab", { name: "Conjunto completo" }).click();
  await page.locator('[data-act="scan"]').click();
  await page.waitForFunction(() => document.querySelector("#ex-right-body")?.textContent.includes("Contagem completa"));
  assert.match(await page.locator("#ex-right-body").textContent(), /118\.000/);
  assert.match(await page.locator("#ex-right-body").textContent(), /Exemplos que falharam/);

  // Effect of the edit, then save.
  await page.getByRole("tab", { name: "Efeito da edição" }).click();
  await page.locator('[data-act="diff"]').click();
  await page.waitForFunction(() => document.querySelector("#ex-right-body")?.textContent.includes("Campos novos"));
  await page.locator("#ex-name").fill("norm");
  await page.locator("#ex-name").dispatchEvent("change");
  await page.locator('[data-act="ex-save"]').click();
  await page.waitForFunction(() => window.__calls.some((c) => c.action === "save_map"));
  const saved = await page.evaluate(() => window.__calls.find((c) => c.action === "save_map"));
  assert.equal(saved.name, "norm");
  assert.match(saved.map_json, /WORD:usuario/);

  // Templates load as drafts and are tried on their own example lines, with no source needed.
  await page.getByRole("tab", { name: "Modelos" }).click();
  await page.locator('[data-act="template"]').click();
  await page.waitForFunction(() => document.querySelector("#ex-right-body")?.textContent.includes("Prévia rápida"));

  // Layout: no horizontal overflow, light and dark, wide and narrow.
  for (const width of [1440, 1024, 390]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${theme} ${width}px`);
      if (width !== 390) await page.screenshot({ path: `output/playwright/extraction-${theme}-${width}.png`, fullPage: true });
    }
  }
  assert.deepEqual(errors, []);
  console.log("Extração e normalização: navigation, samples, visual extraction, single source of truth, diagnostics, undo, full scan, diff, save, templates and responsive layout passed (mock presentation).");
} finally {
  await browser.close();
}
