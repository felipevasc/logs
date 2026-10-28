/* Extração e normalização, fluxo "Colunas": contratos de apresentação, posse e interação no
 * navegador. O motor nativo é trocado aqui por respostas simuladas (RegExp do navegador no lugar de
 * PCRE2 e Grok); a semântica real do Grok/PCRE2, as posições, as conversões, a verificação dos
 * padrões e o enquadramento dos registros são verificados pelos testes Rust e por
 * test-local-native.mjs. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync } from "node:fs";
const browser = await launchBrowser(),
  page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
mkdirSync("output/playwright", { recursive: true });
await page.addInitScript(() => {
  const apache = ['10.0.0.5 - ana [23/Oct/2026:17:05:09 +0000] "GET /index.html HTTP/1.1" 200 5120', '192.168.1.20 - - [23/Oct/2026:17:05:11 -0300] "POST /api/login HTTP/1.1" 401 87', '10.0.0.7 - - [23/Oct/2026:17:06:00 +0000] "GET /missing HTTP/1.1" 404 -'];
  const java = [
    "2026-10-23 17:05:09,120 ERROR [pool-3] br.app.Pedido - Falha ao salvar pedido 8841 (cliente 77) em 12.5ms\njava.lang.IllegalStateException: estoque zerado (sku 42) (lote A7) (filial 3)\n\tat br.app.Estoque.baixar(Estoque.java:88)\n\tat br.app.Pedido.salvar(Pedido.java:41)",
    "2026-10-23 17:05:44,410 INFO [pool-1] br.app.Login - usuario=bruno entrou de 192.168.1.20 em 3ms",
    "2026-10-23 17:06:12,001 ERROR [pool-2] br.app.Pagamento - Timeout no gateway 10.0.0.9 (tentativa 3) em 250ms\njava.net.SocketTimeoutException: Read timed out (host gw-1) (porta 443) (filial 9)\n\tat br.app.Gateway.chamar(Gateway.java:130)",
  ];
  const detectedHttp = (t) => { const m = /^(\S+) - (\S+) \[([^\]]+)\] "(\w+) (\S+) [^"]*" (\d+)/.exec(t); return { standard: {}, fields: { clientip: m[1], timestamp: m[3], verb: m[4], request: m[5], response: m[6] } }; };
  const detectedJava = (t) => { const m = /^(\S+ \S+) (\w+) \[([^\]]+)\] (\S+)/.exec(t); return { standard: { level: m[2] }, fields: { thread: m[3], logger: m[4] } }; };
  const files = { "C:\\logs\\access.log": { name: "access.log", list: apache, detected: detectedHttp }, "C:\\logs\\app.log": { name: "app.log", list: java, detected: detectedJava } };
  const sampleOf = (path) => files[path].list.map((text, id) => ({ id, eventRef: `${files[path].name}:${id}`, file: path, fileName: files[path].name, offset: String(id * 90), text, truncated: false, detected: files[path].detected(text) }));
  const TOK = { IP: "(?:\\d{1,3}\\.){3}\\d{1,3}", NOTSPACE: "\\S+", DATA: ".*?", WORD: "\\w+", INT: "-?\\d+", NUMBER: "-?\\d+(?:\\.\\d+)?", TIMESTAMP_ISO8601: "\\d{4}-\\d\\d-\\d\\d[T ]\\d\\d:\\d\\d:\\d\\d(?:[.,]\\d+)?" };
  const toRegex = (pattern, custom) => pattern.replace(/%\{(\w+)(?::(\w+))?\}/g, (_, t, n) => { const src = custom?.[t] ?? TOK[t] ?? ".+?"; return n ? `(?<${n}>${src})` : `(?:${src})`; });
  function evaluate(map, texts) {
    const rules = map.rules.filter((r) => r.extract && (r.extract.patterns?.[0] || r.extract.pattern));
    const results = texts.map((text, id) => {
      const trace = { rules: [], outputs: [], conflicts: [] };
      for (const r of rules) {
        const source = r.extract.kind === "grok" ? toRegex(r.extract.patterns[0], map.grokPatterns) : r.extract.pattern;
        let m = null, joined = null;
        try { m = new RegExp(source, "d").exec(text); } catch { /* inválida */ }
        if (m && r.extract.all) { const every = [...text.matchAll(new RegExp(source, "g"))]; joined = {}; for (const n of Object.keys(m.groups || {})) joined[n] = every.map((x) => x.groups[n]).filter((v) => v != null).join(", "); }
        const extracted = m ? Object.entries(m.groups || {}).map(([n, v]) => [n, joined?.[n] ?? v, m.indices.groups[n]]) : [];
        trace.rules.push({ rule: r.id, eligible: true, extraction: m ? "recognized" : "not_recognized", extracted });
        for (const [field, mapping] of Object.entries(r.fields || {})) {
          const hit = extracted.find(([n]) => n === mapping.from[0]);
          if (!hit) { trace.outputs.push({ rule: r.id, field, status: "missing", original: null }); continue; }
          let status = "ok";
          if (mapping.type === "integer" && !/^-?\d+$/.test(hit[1])) status = "invalid";
          if (mapping.type === "decimal" && !/^-?\d+(\.\d+)?$/.test(hit[1])) status = "invalid";
          trace.outputs.push({ rule: r.id, field, status, original: hit[1], normalized: status === "ok" ? (["integer", "decimal"].includes(mapping.type) ? Number(hit[1]) : hit[1]) : null, range: hit[2] });
        }
      }
      return { id, fields: {}, trace };
    });
    return { ok: true, scope: "sample", inspected: texts.length, diagnostics: [], results, rules: [], outputs: [] };
  }
  window.__calls = [];
  window.__ts = [];
  window.__reloads = 0;
  const textsOf = {};
  for (const path of Object.keys(files)) for (const s of sampleOf(path)) textsOf[`${s.fileName}:${s.id}`] = s.text;
  window.__mockLocalAnalysis = async ({ request: r }) => {
    window.__calls.push(r);
    switch (r.action) {
      case "map_catalog": return { grok: { patterns: [] }, templates: [], limits: {} };
      case "map_check": return { valid: true, error: null, diagnostics: [], outputs: [] };
      case "map_sample": {
        const paths = r.sources || Object.keys(files);
        const samples = paths.flatMap(sampleOf);
        return { isSample: true, population: 80400, examined: samples.length, field: r.field, seed: 1, samples, distribution: Object.keys(files).map((p) => ({ file: p, fileName: files[p].name, records: p.endsWith("access.log") ? 71000 : 9400, selected: 3 })), shapes: [] };
      }
      case "map_frame": return { format: "mixed", detected: r.path.endsWith("app.log") ? "^\\d{4}-\\d\\d-\\d\\d " : null, pattern: r.pattern || null, records: (r.pattern === "^BAD" ? [] : java.slice(0, 2).map((text, i) => ({ offset: String(i), text, lines: text.split("\n").length }))), more: false };
      case "map_verify": return { results: r.patterns.map((p) => { try { const re = new RegExp(p, "d"); return { error: null, ranges: r.texts.map((t) => { const m = re.exec(t); return m && m.indices[1] ? m.indices[1] : null; }), ...(r.counts ? { counts: r.texts.map((t) => [...t.matchAll(new RegExp(p, "g"))].length) } : {}) }; } catch (e) { return { error: String(e), ranges: [] }; } }) };
      case "map_evaluate": return evaluate(JSON.parse(r.map_json), r.samples.map((s) => textsOf[s.event_ref]));
      case "map_try": return evaluate(JSON.parse(r.map_json), r.texts);
      case "map_diff": return { recompute: true, global: false, added: [], removed: [], changed: [], unchanged: [], rulesAdded: [], rulesRemoved: [], rulesChanged: [], indexesToRebuild: [], metadataOnly: false };
      case "map_scan": return { ok: true, scope: "complete", countsComplete: true, inspected: 80400, affected: 79000, conflicts: 0, elapsedMs: 800, diagnostics: [], rules: [], outputs: [], files: [], successes: [], failures: [] };
      case "save_map": return null;
      default: throw Error(`Unmocked presentation request ${r.action}`);
    }
  };
});
const ready = () => page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);
/** Seleciona `text` (a `nth` ocorrência dentro do registro mostrado) como um usuário faria com o mouse. */
const select = (text, nth = 0) => page.evaluate(([t, n]) => {
  const el = document.querySelector("#cx-record"), all = el.textContent;
  let start = -1;
  for (let k = 0; k <= n; k++) start = all.indexOf(t, start + 1);
  if (start < 0) throw new Error(`texto não encontrado: ${t}`);
  const at = (pos) => { const wk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT); let node, acc = 0; while ((node = wk.nextNode())) { if (pos <= acc + node.textContent.length) return [node, pos - acc]; acc += node.textContent.length; } return [node, node.textContent.length]; };
  const [sn, so] = at(start), [en, eo] = at(start + t.length);
  getSelection().setBaseAndExtent(sn, so, en, eo);
}, [text, nth]);
const rows = () => page.locator("#cx-rows tr").evaluateAll((trs) => trs.map((tr) => [...tr.children].map((td) => td.textContent.trim())));
try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await ready();
  await page.evaluate(() => {
    window.commitTsConfig = async (paths, config) => { window.__ts.push({ paths, config }); };
    window.loadData = async () => { window.__reloads++; return true; };
    const original = window.api;
    window.api = (cmd, ...rest) => (cmd === "get_ts_config" ? Promise.resolve(null) : original(cmd, ...rest));
  });
  await page.getByRole("button", { name: "Estrutura", exact: true }).click();
  await page.locator('.nav-pages [data-page="extraction"]').click();
  await page.waitForSelector("#cx-record .cx-m");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.zone), "structure");
  assert.equal(await page.locator("#ex-adv").isHidden(), true, "a tela simples é a padrão");
  assert.equal(await page.locator("#cx-tabs .cx-tab").count(), 2, "uma aba por arquivo");
  assert.ok((await page.locator("#ex-easy").innerText()).length < 900, "a tela fica quase sem texto; as explicações ficam atrás dos ícones");
  await page.screenshot({ path: "output/playwright/extraction-columns-start.png", fullPage: true });

  // 1. O que a indexação já reconheceu aparece marcado e na tabela, sem criar nada.
  assert.ok((await page.locator("#cx-record .cx-m.exists").count()) >= 3, "campos já reconhecidos marcados na amostra");
  const first = await rows();
  assert.ok(first.some((r) => r[0].startsWith("clientip") && /já existe/.test(r[0])), JSON.stringify(first));
  assert.ok(first.some((r) => r[0].startsWith("verb") && r[1] === "Método HTTP"));
  assert.ok(first.some((r) => r[0].startsWith("response") && r[1] === "Código de resposta HTTP"), "o tipo é inferido dos valores");

  // 2. Registro de várias linhas: aba do arquivo Java, início de registro detectado.
  await page.locator("#cx-tabs .cx-tab", { hasText: "app.log" }).click();
  await page.waitForFunction(() => /1\/3/.test(document.querySelector("#cx-pos")?.textContent || "") && document.querySelector("#cx-record").textContent.includes("estoque zerado"));
  await page.waitForFunction(() => /\^\\d\{4\}/.test(document.querySelector("#cx-start")?.textContent || ""));

  // 3. Selecionar o trecho só mostra a dica; nada é calculado antes do clique nela.
  const callsBefore = await page.evaluate(() => window.__calls.filter((c) => c.action === "map_verify").length);
  await select("filial 3");
  await page.locator("#cx-record").dispatchEvent("mouseup");
  await page.waitForSelector("#cx-seltip:not([hidden])");
  assert.equal(await page.locator("#cx-new.draft").count(), 0);
  assert.equal(await page.evaluate(() => window.__calls.filter((c) => c.action === "map_verify").length), callsBefore);
  await page.click("#cx-seltip");
  await page.waitForSelector("#cx-new.draft .cx-opt");
  // O motor nativo confirma as sugestões; a tabela mostra a coluna nova primeiro e em destaque.
  await page.waitForFunction(() => window.__calls.some((c) => c.action === "map_verify"));
  await page.waitForFunction(() => !/confirmando/.test(document.querySelector("#cx-new").textContent));
  assert.ok((await page.locator("#cx-new .cx-opt").count()) >= 8, "muitas possibilidades verificadas");
  const labels = await page.locator("#cx-new .cx-opt b").allTextContents();
  assert.ok(labels.some((l) => /2ª linha/.test(l)) && labels.some((l) => /3º|último/.test(l)), "localiza pela linha e pela posição");
  const draftRow = await page.locator("#cx-rows tr.draftrow").first().innerText();
  assert.match(draftRow, /novo/);
  assert.equal(await page.locator("#cx-rows tr").first().getAttribute("class"), "draftrow", "a coluna nova é a primeira linha");
  assert.ok((await page.locator("#cx-record .cx-m.draft").textContent()).startsWith("filial 3"), "o rascunho marca o trecho escolhido");
  // Cada sugestão devolve exatamente o trecho (conferido aqui com o mesmo registro).
  const wrong = await page.evaluate(() => { const d = ExtractionColumns.state.draft, rec = ExtractionColumns.state.recs[d.rec].text; return d.cands.filter((c) => { const m = new RegExp(c.re, "d").exec(rec); return !(c.part ? m : m && m.indices[1][0] === d.s && m.indices[1][1] === d.e); }).length; });
  assert.equal(wrong, 0, "nenhuma sugestão deixa de reproduzir a seleção");

  // 3b. Várias ocorrências no registro: a opção só aparece quando o motor contou mais de uma.
  assert.ok(await page.evaluate(() => window.__calls.filter((c) => c.action === "map_verify").every((c) => c.counts === true)), "a verificação pede as contagens");
  await page.evaluate(() => { const d = ExtractionColumns.state.draft; d.cands[d.pick].counts = d.cands[d.pick].counts.map(() => 1); });
  await page.locator("#cx-new .cx-opt").first().click();
  assert.equal(await page.locator("#cx-every").count(), 0, "uma só ocorrência: sem a opção");
  await page.evaluate(() => { const d = ExtractionColumns.state.draft; d.cands[d.pick].counts = d.cands[d.pick].counts.map(() => 3); });
  await page.locator("#cx-new .cx-opt").first().click();
  await page.waitForSelector("#cx-every");
  assert.match(await page.locator(".cx-every").innerText(), /aparece 3 vezes/);
  await page.check("#cx-every");
  assert.equal(await page.locator("#cx-m-regex").getAttribute("aria-pressed"), "true", "todas as ocorrências só existem em Regex");
  await page.uncheck("#cx-every");
  await page.locator("#cx-new .cx-opt").first().click();

  // 4. Nome fácil: sugestão de nome em um clique, e o tipo vem de um seletor com busca.
  assert.ok((await page.locator("#cx-new .cx-sugg button").count()) >= 1);
  await page.fill("#cx-nameinput", "filial");
  await page.click("#cx-typebtn");
  await page.fill("#cx-tq", "texto");
  assert.ok((await page.locator("#cx-tp button").count()) >= 1);
  await page.locator("#cx-tp button", { hasText: "Texto" }).first().click();
  assert.match(await page.locator("#cx-typebtn").innerText(), /Texto/);
  await page.screenshot({ path: "output/playwright/extraction-columns-draft.png", fullPage: true });
  await page.click("#cx-d-ok");
  await page.waitForFunction(() => document.querySelectorAll("#cx-rows tr[data-c]").length === 1 && !document.querySelector("#cx-new.draft"));
  // A coluna é uma regra da mesma definição, lendo o registro inteiro.
  const rule = await page.evaluate(() => Extraction.state.def.rules.find((r) => r.id === "col-filial"));
  assert.equal(rule.extract.source, "raw");
  assert.ok(["grok", "regex"].includes(rule.extract.kind));
  assert.equal(rule.fields.filial.type, "text");
  assert.ok(await page.evaluate(() => Object.keys(Extraction.state.def.ui.columns).includes("filial")));
  await page.waitForFunction(() => document.querySelector("#cx-record .cx-m.col") !== null, null, { timeout: 5000 }).catch(async (e) => { console.log(JSON.stringify(await page.evaluate(() => ({ res: ExtractionColumns.state.res && { error: ExtractionColumns.state.res.error, r0: ExtractionColumns.state.res.results?.[0]?.trace }, last: window.__calls.filter((c) => c.action === "map_evaluate").slice(-1)[0]?.map_json, idx: ExtractionColumns.state.idx })))); throw e; });
  assert.match((await rows()).find((r) => r[0].startsWith("filial"))[2], /3/);
  // O tipo pode ser trocado na tabela; valores que não cabem no tipo são sinalizados, nunca escondidos.
  await page.locator('#cx-rows [data-ty="filial"]').click();
  await page.fill("#cx-tq", "inteiro");
  await page.locator("#cx-tp button", { hasText: "Inteiro" }).first().click();
  await page.waitForFunction(() => document.querySelector("#cx-rows .tag.warn") !== null);
  await page.locator('#cx-rows [data-ty="filial"]').click();
  await page.fill("#cx-tq", "texto");
  await page.locator("#cx-tp button", { hasText: "Texto" }).first().click();
  await page.waitForFunction(() => document.querySelector("#cx-rows .tag.warn") === null);

  // 5. Outra coluna por clique num pedaço sublinhado; o seletor de nome sugere e a regra nova convive.
  await page.locator("#cx-record .cx-m.cand").first().click();
  await page.waitForSelector("#cx-new.draft .cx-opt");
  // 5b. Todas as ocorrências: a coluna vira Regex com `all`, tipo texto e a marca "lista" na tabela.
  await page.evaluate(() => { const d = ExtractionColumns.state.draft; d.cands[d.pick].counts = d.cands[d.pick].counts.map(() => 3); });
  await page.locator("#cx-new .cx-opt").first().click();
  await page.check("#cx-every");
  await page.fill("#cx-nameinput", "varios");
  await page.click("#cx-d-ok");
  await page.waitForFunction(() => Extraction.state.def.rules.some((r) => r.id === "col-varios"));
  const everyRule = await page.evaluate(() => Extraction.state.def.rules.find((r) => r.id === "col-varios"));
  assert.equal(everyRule.extract.kind, "regex");
  assert.equal(everyRule.extract.all, true);
  assert.equal(everyRule.fields.varios.type, "text");
  assert.match(await page.locator('#cx-rows tr[data-c="varios"]').innerText(), /lista/);
  await page.locator('#cx-rows tr[data-c="varios"]').click({ button: "right" });
  await page.locator("#cx-menu", { hasText: "Remover coluna" }).getByText("Remover coluna").click();
  await page.waitForFunction(() => !Extraction.state.def.rules.some((r) => r.id === "col-varios"));
  await page.locator("#cx-record .cx-m.cand").first().click();
  await page.waitForSelector("#cx-new.draft .cx-opt");
  await page.click("#cx-d-cancel");
  assert.equal(await page.locator("#cx-new.draft").count(), 0);

  // 6. Renomear (duplo clique) e remover (botão direito) na tabela; desfazer/refazer.
  await page.locator('#cx-rows [data-nm="filial"]').dblclick();
  await page.keyboard.press("Control+A");
  await page.keyboard.type("unidade");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => Object.keys(Extraction.state.def.ui.columns).join() === "unidade");
  const renamed = await page.evaluate(() => Extraction.state.def.rules.find((r) => r.id === "col-unidade"));
  assert.ok(renamed && !(await page.evaluate(() => Extraction.state.def.rules.some((r) => r.id === "col-filial"))));
  assert.ok(JSON.stringify(renamed.extract).includes("unidade"), "a expressão acompanha o nome");
  await page.locator("#cx-undo").click();
  await page.waitForFunction(() => Object.keys(Extraction.state.def.ui.columns).join() === "filial");
  await page.locator("#cx-redo").click();
  await page.waitForFunction(() => Object.keys(Extraction.state.def.ui.columns).join() === "unidade");
  await page.locator('#cx-rows tr[data-c="unidade"]').click({ button: "right" });
  await page.waitForSelector("#cx-menu:not([hidden])");
  assert.ok((await page.locator("#cx-menu").innerText()).includes("Remover coluna"));
  await page.keyboard.press("Escape");

  // 7. O que é um registro: prévia antes de salvar, e salvar relê o arquivo.
  await page.click("#cx-start-edit");
  await page.waitForSelector("#cx-dlg[open]");
  await page.fill("#cx-dlg input[name=v]", "^\\d{4}-");
  await page.click("#cx-rs-try");
  await page.waitForFunction(() => /registros nas primeiras linhas/.test(document.querySelector("#cx-rs-prev")?.textContent || ""));
  assert.ok(await page.evaluate(() => window.__calls.some((c) => c.action === "map_frame" && c.pattern === "^\\d{4}-" && c.count === 40)));
  await page.locator("#cx-dlg button[value=ok]").click();
  await page.waitForFunction(() => window.__ts.length === 1 && window.__reloads === 1);
  const ts = await page.evaluate(() => window.__ts[0]);
  assert.deepEqual(ts.paths, ["C:\\logs\\app.log"]);
  assert.equal(ts.config.record_start, "^\\d{4}-");

  // 8. Informação fica atrás do ícone discreto, em um modal.
  await page.locator('[data-info="registro"]').click();
  assert.match(await page.locator("#cx-dlg").textContent(), /Rust regex/);
  await page.keyboard.press("Escape");

  // 9. Aplicar: resumo e conferência; a definição é salva pelo mesmo caminho do modo avançado.
  await page.click("#cx-apply");
  await page.waitForSelector("#cx-dlg[open]");
  assert.match(await page.locator("#cx-dlg").textContent(), /norm\.unidade/);
  await page.locator("#cx-dlg button[value=ok]").click();
  await page.waitForFunction(() => window.__calls.some((c) => c.action === "save_map"));
  const saved = await page.evaluate(() => window.__calls.find((c) => c.action === "save_map"));
  assert.equal(saved.name, "norm");
  assert.match(saved.map_json, /"col-unidade"/);
  assert.match(saved.map_json, /"source": "raw"/);

  // 10. O modo avançado é a mesma definição, a um clique, e volta.
  await page.click("#cx-adv");
  await page.waitForSelector("#ex-adv:not([hidden])");
  assert.equal(await page.locator("#ex-easy").isHidden(), true);
  await page.locator('[data-act="view-easy"]').click();
  await page.waitForSelector("#ex-easy:not([hidden]) #cx-record");

  // 11. Layout: sem rolagem horizontal nos dois temas e larguras.
  for (const width of [1440, 1024, 390]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${theme} ${width}px`);
      if (width !== 390) await page.screenshot({ path: `output/playwright/extraction-columns-${theme}-${width}.png`, fullPage: true });
    }
  }
  // 12a. Um Caso que não responde às chamadas preparatórias não prende a página: o quadro aparece de
  // imediato e a página abre depois de um tempo limitado.
  await page.evaluate(() => Workspace.showPage("sources"));
  await page.evaluate(() => { const original = window.api; window.__origApi = original; window.api = (cmd, ...rest) => ["list_sources", "list_derived_fields"].includes(cmd) ? new Promise(() => {}) : original(cmd, ...rest); });
  await page.evaluate(() => { Workspace.showPage("extraction"); });
  await page.waitForSelector("#cx-apply", { timeout: 1000 });
  await page.waitForFunction(() => document.querySelector("#cx-record .cx-m") !== null, null, { timeout: 20000 });
  // Nada pode ser salvo antes de as definições do Caso serem conhecidas: poderia substituir uma delas.
  await page.click("#cx-apply");
  await page.waitForFunction(() => /ainda estão carregando|Crie ao menos/.test(document.querySelector("#cx-toast")?.textContent || ""));
  await page.evaluate(() => { window.api = window.__origApi; });
  // 12b. Uma falha ao abrir é dita na página; o quadro nunca fica vazio.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => { Extraction.state.def = null; LocalExplorer.extraction = () => ({ current: "", field: "message", count: 40 }); });
  await page.evaluate(() => Workspace.showPage("sources"));
  await page.evaluate(() => Workspace.showPage("extraction"));
  await page.waitForSelector("#cx-error:not([hidden])");
  assert.match(await page.locator("#cx-error").textContent(), /Não foi possível abrir/);
  assert.equal(await page.locator("#cx-apply").isVisible(), true, "o quadro está lá");
  errors.length = 0; // a falha acima é registrada de propósito
  assert.deepEqual(errors, []);
  console.log("Extração e normalização (Colunas): campos reconhecidos, abas, dica de seleção, sugestões verificadas, nome e tipo, coluna como regra, renomear, desfazer, início de registro, ajuda em modal, aplicar, avançado e layout passaram (apresentação simulada).");
} finally {
  await browser.close();
}
