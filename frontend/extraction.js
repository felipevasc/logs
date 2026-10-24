/* Extração e normalização: one declarative definition (the source of truth) edited visually or
 * as text, tried on real samples by the native engine, checked on the whole set, and applied
 * to the Case only when saved. Nothing here runs Grok or PCRE2 in JavaScript. */
window.Extraction = (() => {
  "use strict";
  const M = window.ExtractionModel;
  const call = (request, options) => window.LocalNative.call(request, options);
  const h = (value) => esc(String(value ?? ""));
  const num = (value) => Number(value || 0).toLocaleString("pt-BR");
  const KINDS = [["grok", "Grok"], ["dissect", "Delimitadores fixos"], ["json", "JSON"], ["xml", "XML"], ["delimited", "Colunas delimitadas"], ["key_value", "Chave=valor"], ["regex", "Expressão regular"]];
  const TYPES = [["", "Sem conversão"], ["text", "Texto"], ["integer", "Inteiro"], ["decimal", "Decimal"], ["boolean", "Booleano"], ["datetime", "Data/hora"], ["ip", "Endereço IP"]];
  const STEPS = [["trim", "Remover espaços nas pontas"], ["lowercase", "Minúsculas"], ["uppercase", "Maiúsculas"], ["basename", "Nome do arquivo (sem pasta)"], ["hex_to_int", "Hexadecimal → decimal"], ["strip_prefix", "Remover prefixo"], ["strip_suffix", "Remover sufixo"], ["remove_chars", "Remover caracteres"], ["replace", "Substituir texto"]];
  const STATUS = { ok: "ok", missing: "ausente", empty: "vazio", invalid: "inválido", default: "padrão", kept_text: "texto mantido", overridden: "sobreposto", rejected: "rejeitado" };
  const ACTIONS = [
    ["extract", "Extrair como campo", "fa-crosshairs"], ["recognize", "Reconhecer formato", "fa-wand-magic-sparkles"],
    ["after", "Capturar depois de…", "fa-arrow-right-to-bracket"], ["before", "Capturar antes de…", "fa-arrow-right-from-bracket"],
    ["between", "Entre marcadores…", "fa-brackets-curly"], ["split", "Separar por delimitador", "fa-grip-lines-vertical"],
    ["ignore", "Ignorar este trecho", "fa-eye-slash"], ["fixed", "Texto fixo", "fa-lock"],
    ["variation", "Permitir variação", "fa-shuffle"], ["optional", "Tornar opcional", "fa-circle-question"],
  ];
  const SOURCES = ["message", "raw"];
  const menu = window.ContextMenu?.create({ captureReturnFocus: () => null, fallbackFocus: () => [] });
  let root = null, generation = 0, ownerKey = "", timers = {};
  const S = {
    name: "norm", installed: null, base: "", def: null, ruleId: null, samples: null, active: 0, segs: {}, quick: null, scan: null, diff: null,
    view: "easy", suggest: null, catalog: null, tab: "quick", index: null, serial: { quick: 0, check: 0, scan: 0 }, hist: new M.History(), texts: [], selection: null,
    diagnostics: [], outputs: [], valid: true, error: "", notice: "", noticeError: false, chip: null, sources: [], pickedSources: null, building: null, json: "",
  };
  const prefs = () => window.LocalExplorer.extraction();
  const q = (selector) => root?.querySelector(selector);
  const alive = (g) => g === generation && root?.isConnected;
  const text = () => JSON.stringify(S.def, null, 2);
  // Any difference from the saved definition is pending, cosmetic ones included: saving writes them too.
  const dirty = () => JSON.stringify(S.def) !== (S.base || JSON.stringify(fresh()));
  const installedMaps = () => (state.derivedFields || []).filter((d) => d.advanced?.kind === "map");
  const rule = () => S.def.rules.find((r) => r.id === S.ruleId) || null;
  const ruleIndex = () => S.def.rules.findIndex((r) => r.id === S.ruleId);
  const colors = () => S.def.ui?.colors || {};
  const swatch = (name) => `<span class="ex-swatch" style="background:${M.fieldColor(name, colors())}" aria-hidden="true"></span>`;
  const say = (message = "", error = false) => { S.notice = message; S.noticeError = error; const n = q("#ex-notice"); if (n) { n.hidden = !message; n.dataset.error = String(error); n.textContent = message; } };
  const later = (key, fn, wait = 450) => { clearTimeout(timers[key]); timers[key] = setTimeout(fn, wait); };

  // ------------------------------------------------------------------ definition
  function fresh() {
    const d = M.blank("novo-mapa", "Novo mapa");
    const r = M.blankRule("regra-1");
    r.extract = { kind: "grok", patterns: [""], anchor: "search", source: "message" };
    d.rules.push(r);
    return d;
  }
  function setDef(def, { record = true } = {}) {
    S.def = def;
    if (!S.def.rules.some((r) => r.id === S.ruleId)) S.ruleId = S.def.rules[0]?.id ?? null;
    if (record && S.hist.push(text())) saveDraft();
    S.json = text();
  }
  function commit() {
    S.hist.push(text()); S.json = text();
    saveDraft(); refreshDerived(); paintHeader(); paintResultsHeader(); S.onChange?.();
  }
  function saveDraft() {
    const key = S.installed || "__novo__";
    prefs().drafts[key] = { map: text(), savedAt: Date.now(), base: S.base };
    prefs().current = key;
    later("draft", () => window.LocalExplorer.keep(), 800);
  }
  function dropDraft(key = S.installed || "__novo__") { delete prefs().drafts[key]; window.LocalExplorer.keep(); }
  function refreshDerived() {
    later("check", () => void check(), 250);
    later("quick", () => void evaluateQuick(), 450);
  }
  /** Opens a saved map (or a new one), restoring its unsaved draft unless told to ignore it. */
  function loadInstalled(name, { ignoreDraft = false } = {}) {
    const item = installedMaps().find((d) => d.name === name);
    S.installed = item ? item.name : null;
    S.name = item ? item.name : "norm";
    S.base = item ? JSON.stringify(item.advanced.map) : "";
    const draft = ignoreDraft ? null : prefs().drafts[item ? item.name : "__novo__"];
    let def = item ? structuredClone(item.advanced.map) : fresh();
    if (draft && draft.base === S.base && draft.map !== S.base) {
      try {
        def = JSON.parse(draft.map);
        say("Rascunho não salvo restaurado. Salve para aplicar ao Caso ou descarte para voltar ao mapa salvo.");
      } catch { /* a damaged draft is ignored */ }
    }
    setDef(def, { record: false });
    S.hist.reset(text());
    S.quick = null; S.scan = null; S.diff = null; S.segs = {}; S.chip = null;
    prefs().current = S.installed || "__novo__";
  }

  // ------------------------------------------------------------------ backend
  async function check() {
    const mine = ++S.serial.check, g = generation;
    try {
      const r = await call({ action: "map_check", map_json: text() }, { latest: "map-check" });
      if (mine !== S.serial.check || !alive(g)) return;
      S.diagnostics = r.diagnostics || []; S.outputs = r.outputs || []; S.valid = r.valid; S.error = r.error || "";
      paintDiagnostics(); paintHeader();
    } catch (e) { if (!window.LocalNative.superseded(e) && alive(g)) say(String(e.message || e), true); }
  }
  async function evaluateQuick() {
    const mine = ++S.serial.quick, g = generation;
    if (!S.valid) return;
    const sampleItems = S.samples?.samples || [];
    try {
      let result;
      if (sampleItems.length) result = await call({ action: "map_evaluate", map_json: text(), samples: sampleItems.map((s) => ({ id: s.id, event_ref: s.eventRef })) }, { latest: "map-quick" });
      else if (S.texts.length) result = await call({ action: "map_try", map_json: text(), texts: S.texts }, { latest: "map-quick" });
      else return;
      if (mine !== S.serial.quick || !alive(g)) return;
      S.quick = result; paintSamples(); paintRight();
    } catch (e) { if (!window.LocalNative.superseded(e) && alive(g)) say(String(e.message || e), true); }
  }
  async function drawSamples(reseed = false) {
    const g = generation;
    const p = prefs();
    const request = { action: "map_sample", count: p.count, field: p.field, seed: reseed ? Math.floor(Math.random() * 1e9) : S.samples?.seed ?? 1 };
    if (S.pickedSources) request.sources = [...S.pickedSources];
    const r = await call(request, { latest: "map-sample" });
    if (!alive(g)) return;
    S.samples = r; S.texts = []; S.active = 0; S.quick = null; S.segs = {};
    paintLeft(); void suggestNow(); void evaluateQuick();
  }
  async function suggestNow() {
    const g = generation;
    const lines = S.samples ? S.samples.samples.map((s) => s.text) : S.texts;
    if (!lines.length) { S.suggest = null; paintSuggestions(); return; }
    try {
      const r = await call({ action: "map_suggest", texts: lines }, { latest: "map-suggest" });
      if (alive(g)) { S.suggest = r; paintSuggestions(); }
    } catch (e) { if (!window.LocalNative.superseded(e) && alive(g)) say(String(e.message || e), true); }
  }
  async function scan() {
    const g = generation, mine = ++S.serial.scan;
    S.scan = { running: true }; paintRight();
    try {
      const r = await call({ action: "map_scan", map_json: text() }, { latest: "map-scan", filters: !!q("#ex-scan-filters")?.checked });
      if (mine !== S.serial.scan || !alive(g)) return;
      S.scan = r;
    } catch (e) {
      if (mine !== S.serial.scan || !alive(g)) return;
      S.scan = { error: window.LocalNative.superseded(e) ? "Conferência cancelada. Nenhum resultado parcial é exibido como completo." : String(e.message || e) };
    }
    paintRight();
  }
  async function computeDiff() {
    const g = generation;
    const prefix = S.name;
    const status = await call({ action: "map_diff", map_json: text(), current_json: S.base || null, prefix, indexed: (S.index?.indexes || []).map((i) => i.field) }, { latest: "map-diff" });
    if (alive(g)) S.diff = status;
    return status;
  }
  async function refreshIndexes() {
    const g = generation;
    try {
      const r = await api("field_index_status", {}, { silent: true, analysisOwner: window.AnalysisContexts.capture() });
      if (alive(g)) { S.index = r; if (S.tab === "indexes") paintRight(); }
    } catch { if (alive(g)) S.index = null; }
  }
  async function save() {
    if (!S.valid) { say(S.error || "Corrija os problemas antes de salvar.", true); return; }
    const name = (q("#ex-name").value || "").trim();
    if (!name) { say("Informe o nome do campo que recebe os valores (por exemplo, norm).", true); return; }
    const g = generation;
    const diff = await computeDiff();
    if (!alive(g)) return;
    if (diff.indexesToRebuild?.length && !confirm(`Esta alteração invalida ${diff.indexesToRebuild.length} índice(s): ${diff.indexesToRebuild.join(", ")}. Eles voltam a ser usados depois de recriados. Salvar mesmo assim?`)) return;
    await call({ action: "save_map", name, map_json: text() }, { latest: "local-save" });
    await loadDerivedFields();
    S.installed = name; S.name = name; S.base = JSON.stringify(S.def); dropDraft("__novo__"); dropDraft(name);
    S.diff = null; S.hist.reset(text());
    say(`Mapa salvo no Caso como “${name}”. Filtros, SQL e árvores de processos usam os novos campos.`);
    paintAll(); void refreshIndexes();
  }

  // ------------------------------------------------------------------ rendering: header
  function caseLine() {
    const c = activeCase?.();
    const files = S.sources.length;
    return `<span><i class="fas fa-briefcase" aria-hidden="true"></i> Caso: <strong>${h(c?.name || "—")}</strong></span><span><i class="fas fa-folder-open" aria-hidden="true"></i> ${files ? `${num(files)} fonte(s) visíveis` : "Nenhuma fonte aberta"}${files ? ` · ${h(S.sources.slice(0, 3).map((s) => s.name).join(", "))}${files > 3 ? "…" : ""}` : ""}</span><span><i class="fas fa-shield" aria-hidden="true"></i> Os arquivos originais nunca são alterados; o mapa é salvo no Caso.</span>`;
  }
  function paintHeader() {
    const node = q("#ex-head");
    if (!node) return;
    const installed = installedMaps();
    const pending = dirty();
    const html = `<div class="ex-case">${caseLine()}</div><div class="ex-bar">
      <label class="ex-grow">Mapa do Caso<select id="ex-installed"><option value="">Novo mapa</option>${installed.map((d) => `<option value="${h(d.name)}" ${d.name === S.installed ? "selected" : ""}>${h(d.name)} · ${h(d.advanced.map.title)}${d.advanced.map.enabled === false ? " (desativado)" : ""}</option>`).join("")}</select></label>
      <label>Nome do campo<input id="ex-name" value="${h(S.name)}" maxlength="256" aria-describedby="ex-name-help"></label>
      <span class="ex-pending" ${pending ? "" : "hidden"} role="status"><i class="fas fa-circle" aria-hidden="true"></i> Alterações não salvas</span>
      <button type="button" class="btn ghost small" data-act="view-easy"><i class="fas fa-wand-magic-sparkles" aria-hidden="true"></i> Modo simples</button>
      <button type="button" class="btn ghost small" data-act="undo" ${S.hist.canUndo ? "" : "disabled"} title="Desfazer (Ctrl+Z)"><i class="fas fa-rotate-left" aria-hidden="true"></i> Desfazer</button>
      <button type="button" class="btn ghost small" data-act="redo" ${S.hist.canRedo ? "" : "disabled"} title="Refazer (Ctrl+Y)"><i class="fas fa-rotate-right" aria-hidden="true"></i> Refazer</button>
      <button type="button" class="btn primary small" data-act="ex-save" ${S.valid && pending ? "" : "disabled"}><i class="fas fa-floppy-disk" aria-hidden="true"></i> Salvar no Caso</button>
    </div><div class="ex-bar ex-bar-secondary">
      <button type="button" class="btn ghost small" data-act="discard" ${pending ? "" : "disabled"}>Descartar rascunho</button>
      <button type="button" class="btn ghost small" data-act="duplicate" ${S.installed ? "" : "disabled"}>Duplicar</button>
      <button type="button" class="btn ghost small" data-act="toggle" ${S.installed ? "" : "disabled"}>${S.def.enabled === false ? "Ativar" : "Desativar"}</button>
      <button type="button" class="btn ghost small danger" data-act="remove" ${S.installed ? "" : "disabled"}>Remover do Caso</button>
      <span class="ex-grow"></span>
      <button type="button" class="btn ghost small" data-tab="templates">Modelos</button>
      <button type="button" class="btn ghost small" data-act="import">Importar…</button>
      <button type="button" class="btn ghost small" data-act="export">Exportar</button>
      <input id="ex-import-file" type="file" accept=".json,application/json" hidden>
    </div><p id="ex-name-help" class="lx-help">Os valores ficam em <code>${h(S.name || "norm")}.&lt;campo&gt;</code> e em <code>${h(S.name || "norm")}._provenance.*</code>. Campos customizados não são alterados por esta página.</p>
    <div class="lx-notice" id="ex-notice" role="status" ${S.notice ? "" : "hidden"} data-error="${S.noticeError}">${h(S.notice)}</div>`;
    // Rebuilding identical markup would swallow a click that is in progress on one of its buttons.
    if (node.dataset.html === html) return;
    node.dataset.html = html;
    node.innerHTML = html;
    const file = q("#ex-import-file");
    if (file) file.onchange = async () => { const f = file.files[0]; file.value = ""; if (f) importText(await f.text()); };
  }
  function paintDiagnostics() {
    const node = q("#ex-diagnostics");
    if (!node) return;
    const items = [...(S.error && !S.diagnostics.length ? [{ message: S.error }] : []), ...S.diagnostics];
    node.innerHTML = items.length ? `<ul class="ex-diag" role="alert">${items.map((d, i) => `<li><i class="fas fa-triangle-exclamation" aria-hidden="true"></i> ${d.rule ? `<strong>${h(d.rule)}</strong>: ` : ""}${h(d.message)}${d.end > d.start ? ` <button type="button" class="btn ghost small" data-act="diag" data-i="${i}">Ver no texto</button>` : ""}</li>`).join("")}</ul>` : `<p class="lx-help ex-ok"><i class="fas fa-circle-check" aria-hidden="true"></i> Definição válida: ${num(S.outputs.length)} saída(s).</p>`;
  }

  // ------------------------------------------------------------------ rendering: samples (left)
  function sampleHtml(sample, index) {
    const result = S.quick?.results?.[index];
    const ranges = [];
    for (const t of result?.trace?.rules || []) for (const [field, , range] of t.extracted || []) if (range) ranges.push({ start: range[0], end: range[1], field });
    for (const seg of S.segs[S.ruleId]?.list || []) if (S.segs[S.ruleId].sample === index && seg.kind === "field") ranges.push({ start: seg.start, end: seg.end, field: seg.name });
    const parts = M.paintPlan(sample.text, ranges);
    const body = parts.map((p) => (p.field ? `<mark class="ex-mark" style="--c:${M.fieldColor(p.field, colors())}" title="${h(p.field)}">${h(p.text)}</mark>` : h(p.text))).join("");
    const rare = sample.shapeCount <= Math.max(2, Math.floor((S.samples?.examined || 1) / 50));
    return `<li class="ex-sample ${index === S.active ? "active" : ""}" data-i="${index}"><div class="ex-sample-head"><span title="${h(sample.file)}">${h(sample.fileName)}</span><span>posição ${h(sample.offset)}</span>${rare ? '<span class="lx-badge">formato raro</span>' : ""}${result?.trace?.rules.some((r) => r.extraction === "not_recognized") && !result.trace.rules.some((r) => r.extraction === "recognized") ? '<span class="lx-badge" data-warn="true">não reconhecida</span>' : ""}</div><pre class="ex-sample-text" tabindex="0" data-i="${index}" aria-label="Amostra ${index + 1}. Selecione um trecho para ver as ações.">${body}</pre>${sample.truncated ? `<small>Texto truncado a 16 KiB (${num(sample.bytes)} bytes no total).</small>` : ""}</li>`;
  }
  function paintSamples() {
    const list = q("#ex-sample-list");
    if (!list) return;
    if (S.samples) { list.innerHTML = S.samples.samples.map(sampleHtml).join(""); return; }
    list.innerHTML = S.texts.length ? S.texts.map((t, i) => sampleHtml({ text: t, fileName: "Texto colado", file: "", offset: "—", shapeCount: 99, bytes: t.length }, i)).join("") : '<li class="lx-empty">Sorteie amostras dos arquivos abertos ou cole algumas linhas para começar.</li>';
  }
  function paintActions() {
    const node = q("#ex-actions");
    if (node) node.innerHTML = ACTIONS.map(([id, label, icon]) => `<button type="button" class="btn ghost small" data-sel="${id}" ${S.selection ? "" : "disabled"} title="${S.selection ? h(label) : "Selecione um trecho de uma amostra"}"><i class="fas ${icon}" aria-hidden="true"></i> ${h(label)}</button>`).join("");
  }
  function paintSuggestions() {
    const node = q("#ex-suggest");
    if (!node) return;
    if (!S.suggest) { node.innerHTML = ""; return; }
    const items = [...(S.suggest.extractions || []).map((e, i) => ({ ...e, group: "x", i })), ...(S.suggest.suggestions || []).filter((s) => s.kind === "value").map((s, i) => ({ ...s, group: "v", i }))];
    node.innerHTML = items.length ? `<h3>Sugestões locais</h3><ul class="ex-suggest-list">${items.map((s) => `<li><div><strong>${h(s.label)}</strong><div>${h(s.message || `${s.explanation} Funciona em ${num(s.found)} de ${num(s.total)} amostras.`)}</div>${s.examples ? `<small>Ex.: ${h(s.examples.join(" · "))}</small>` : ""}</div><div class="ex-suggest-actions"><button type="button" class="btn ghost small" data-act="accept" data-g="${s.group}" data-i="${s.i}">Aceitar</button><button type="button" class="btn ghost small" data-act="adjust" data-g="${s.group}" data-i="${s.i}">Ajustar</button><button type="button" class="btn ghost small" data-act="dismiss" data-g="${s.group}" data-i="${s.i}">Descartar</button></div></li>`).join("")}</ul>` : '<p class="lx-help">Nenhum formato reconhecível foi encontrado nas amostras.</p>';
  }
  function paintLeft() {
    const node = q("#ex-left");
    if (!node) return;
    const p = prefs(), s = S.samples;
    node.innerHTML = `<header><h2>1 · Amostras reais</h2></header>
      <div class="ex-controls"><label>Campo<select id="ex-field">${[...new Set([...SOURCES, p.field, ...(state.columns || []).slice(0, 80)])].map((c) => `<option ${c === p.field ? "selected" : ""}>${h(c)}</option>`).join("")}</select></label>
      <label>Quantidade<select id="ex-count">${[20, 40, 80, 120].map((n) => `<option ${n === p.count ? "selected" : ""}>${n}</option>`).join("")}</select></label>
      <button type="button" class="btn primary small" data-act="sample" ${S.sources.length ? "" : "disabled"}><i class="fas fa-dice" aria-hidden="true"></i> ${s ? "Outra amostra" : "Sortear amostras"}</button></div>
      ${S.sources.length > 1 ? `<details class="ex-sources"><summary>Arquivos considerados (${S.pickedSources ? S.pickedSources.size : S.sources.length} de ${S.sources.length})</summary>${S.sources.map((f) => `<label class="lx-inline"><input type="checkbox" data-src="${h(f.path)}" ${!S.pickedSources || S.pickedSources.has(f.path) ? "checked" : ""}> ${h(f.name)}</label>`).join("")}</details>` : ""}
      ${s ? `<p class="lx-help ex-strategy">${h(s.strategy)}</p><details class="ex-dist"><summary>Distribuição por arquivo e formatos</summary><table class="lx-table"><thead><tr><th>Arquivo</th><th>Registros</th><th>Sorteadas</th></tr></thead><tbody>${s.distribution.map((d) => `<tr><td title="${h(d.file)}">${h(d.fileName)}</td><td>${num(d.records)}</td><td>${num(d.selected)}</td></tr>`).join("")}</tbody></table><p class="lx-help">${s.shapes.map((x) => `${num(x.count)}× <code>${h(x.shape)}</code>`).join(" · ")}</p>${s.withoutSubject ? `<p class="lx-help">${num(s.withoutSubject)} registro(s) sorteado(s) sem o campo “${h(s.field)}” ficaram de fora.</p>` : ""}</details>` : '<p class="lx-help">Sem arquivos abertos? Cole linhas abaixo ou use um modelo para testar a definição.</p>'}
      <details class="ex-paste" ${!s && S.texts.length ? "open" : ""}><summary>Colar linhas de exemplo</summary><textarea id="ex-paste" rows="4" spellcheck="false" placeholder="Uma linha por registro">${h(S.texts.join("\n"))}</textarea><button type="button" class="btn ghost small" data-act="paste">Usar estas linhas</button></details>
      <div id="ex-actions" class="ex-actions" role="toolbar" aria-label="Ações sobre o trecho selecionado"></div>
      <ol id="ex-sample-list" class="ex-samples"></ol><div id="ex-suggest" class="ex-suggest"></div>`;
    paintActions(); paintSamples(); paintSuggestions();
    const field = q("#ex-field"), count = q("#ex-count");
    if (field) field.onchange = () => { p.field = field.value; window.LocalExplorer.keep(); };
    if (count) count.onchange = () => { p.count = +count.value; window.LocalExplorer.keep(); };
  }

  // ------------------------------------------------------------------ rendering: rules (center)
  const input = (path, value, attrs = "") => `<input data-p='${h(JSON.stringify(path))}' value="${h(value ?? "")}" ${attrs}>`;
  const select = (path, value, options) => `<select data-p='${h(JSON.stringify(path))}'>${options.map(([v, l]) => `<option value="${h(v)}" ${v === (value ?? "") ? "selected" : ""}>${h(l)}</option>`).join("")}</select>`;
  function chipsHtml(pattern, a) {
    return M.chips(pattern).map((c, k) => c.type === "token"
      ? `<button type="button" class="ex-chip" style="--c:${c.field ? M.fieldColor(c.field, colors()) : "#8a8f98"}" data-act="chip" data-a="${a}" data-k="${k}" aria-label="Editar ${h(c.pattern)}${c.field ? ` como ${h(c.field)}` : " sem nome"}">${h(c.pattern)}${c.field ? `<b>:${h(c.field)}</b>` : ""}${c.conv ? `<i>:${h(c.conv)}</i>` : ""}</button>`
      : `<span class="ex-lit">${h(c.text)}</span>`).join("");
  }
  function chipEditor(r, ri) {
    const c = S.chip;
    if (!c || c.rule !== r.id) return "";
    const chip = M.chips(r.extract.patterns[c.alt] || "")[c.k];
    if (!chip || chip.type !== "token") return "";
    return `<form class="ex-chip-edit" data-form="chip"><strong>Editar token</strong><label>Padrão<input name="pattern" list="ex-catalog-names" value="${h(chip.pattern)}"></label><label>Nome do campo<input name="field" value="${h(chip.field)}" placeholder="(sem nome: ignorar)"></label><label>Conversão<select name="conv"><option value="">nenhuma</option><option value="int" ${chip.conv === "int" ? "selected" : ""}>inteiro</option><option value="float" ${chip.conv === "float" ? "selected" : ""}>decimal</option></select></label><button type="submit" class="btn primary small">Aplicar</button><button type="button" class="btn ghost small" data-act="chip-close">Fechar</button></form>`;
  }
  function grokEditor(r, ri) {
    const x = r.extract, stats = S.scan?.rules?.find((s) => s.rule === r.id) || null;
    const catalog = S.catalog?.grok?.patterns || [];
    return `<div class="ex-grok">${x.patterns.map((p, a) => `<div class="ex-alt"><label>Alternativa ${a + 1}${a ? "" : " (tentada primeiro)"}<textarea rows="2" spellcheck="false" data-grok="${a}" data-p='${h(JSON.stringify(["rules", ri, "extract", "patterns", a]))}' aria-label="Expressão Grok, alternativa ${a + 1}">${h(p)}</textarea></label><div class="ex-chips" aria-label="Tokens da alternativa ${a + 1}">${chipsHtml(p, a)}</div><div class="ex-alt-tools">${stats?.byAlternative?.[a] != null ? `<span class="lx-badge">reconheceu ${num(stats.byAlternative[a])} no conjunto</span>` : ""}<button type="button" class="btn ghost small" data-act="alt-up" data-a="${a}" ${a ? "" : "disabled"}>↑</button><button type="button" class="btn ghost small" data-act="alt-down" data-a="${a}" ${a < x.patterns.length - 1 ? "" : "disabled"}>↓</button><button type="button" class="btn ghost small" data-act="alt-remove" data-a="${a}" ${x.patterns.length > 1 ? "" : "disabled"}>Remover</button></div></div>`).join("")}
      <button type="button" class="btn ghost small" data-act="alt-add">Adicionar alternativa</button>${chipEditor(r, ri)}
      <div class="ex-row"><label>Âncora${select(["rules", ri, "extract", "anchor"], x.anchor, [["search", "Procurar em qualquer ponto"], ["start", "Começa no início do texto"], ["full", "Casa o texto inteiro"]])}</label><label>Campo lido${select(["rules", ri, "extract", "source"], x.source, SOURCES.map((s) => [s, s]))}</label><label class="lx-inline"><input type="checkbox" data-p='${h(JSON.stringify(["rules", ri, "extract", "caseInsensitive"]))}' data-kind="bool" ${x.caseInsensitive ? "checked" : ""}> Ignorar maiúsculas</label><label class="lx-inline"><input type="checkbox" data-p='${h(JSON.stringify(["rules", ri, "extract", "dotAll"]))}' data-kind="bool" ${x.dotAll ? "checked" : ""}> O ponto inclui quebras de linha</label></div>
      <details class="ex-catalog"><summary>Catálogo de padrões e padrões reutilizáveis</summary><input id="ex-catalog-q" placeholder="Buscar padrão (IP, data, caminho…)" aria-label="Buscar no catálogo Grok"><datalist id="ex-catalog-names">${[...catalog.map((e) => e.name), ...Object.keys(S.def.grokPatterns || {})].map((n) => `<option value="${h(n)}">`).join("")}</datalist><ul id="ex-catalog-list" class="ex-catalog-list"></ul>
      <h4>Padrões desta definição</h4>${Object.entries(S.def.grokPatterns || {}).map(([name, def]) => `<div class="ex-row"><code>${h(name)}</code><input data-p='${h(JSON.stringify(["grokPatterns", name]))}' value="${h(def)}" aria-label="Definição de ${h(name)}"><button type="button" class="btn ghost small" data-act="custom-remove" data-n="${h(name)}">Remover</button></div>`).join("") || '<p class="lx-help">Nenhum. Um padrão reutilizável pode usar outros (<code>%{IP}</code>) e fica disponível para todas as regras.</p>'}
      <form class="ex-row" data-form="custom"><input name="name" placeholder="NOME_DO_PADRAO" pattern="[A-Z][A-Z0-9_]*" aria-label="Nome do padrão"><input name="def" placeholder="%{IP}:%{POSINT}" aria-label="Definição do padrão"><button type="submit" class="btn ghost small">Adicionar padrão</button></form></details></div>`;
  }
  function mapEditor(label, path, value, a, b, hint) {
    const entries = Object.entries(value || {});
    return `<div class="ex-map"><h4>${h(label)}</h4>${entries.map(([k, v]) => `<div class="ex-row"><input data-rename='${h(JSON.stringify(path))}' data-old="${h(k)}" value="${h(k)}" aria-label="${h(a)}"><input data-p='${h(JSON.stringify([...path, k]))}' value="${h(v)}" aria-label="${h(b)}"><button type="button" class="btn ghost small" data-act="map-remove" data-path='${h(JSON.stringify(path))}' data-k="${h(k)}">Remover</button></div>`).join("")}<button type="button" class="btn ghost small" data-act="map-add" data-path='${h(JSON.stringify(path))}'>Adicionar</button><p class="lx-help">${h(hint)}</p></div>`;
  }
  function extractionEditor(r, ri) {
    const x = r.extract;
    if (!x) return '<p class="lx-help">Esta regra não extrai nada: ela só normaliza campos que já existem no registro.</p>';
    const base = ["rules", ri, "extract"];
    switch (x.kind) {
      case "grok": return grokEditor(r, ri);
      case "dissect": return `<label>Padrão<input data-p='${h(JSON.stringify([...base, "pattern"]))}' value="${h(x.pattern)}" class="lx-code"></label><p class="lx-help">Texto fixo e campos: <code>%{usuario} entrou de %{ip}</code>. Use <code>%{?ignorar}</code> para descartar, <code>%{+campo}</code> para juntar.</p>`;
      case "json": return mapEditor("Propriedades", [...base, "paths"], x.paths, "Nome do campo", "Caminho no JSON", "Caminhos como usuario.nome, itens[0].id. O texto pode ter um prefixo antes do JSON.");
      case "xml": return mapEditor("Elementos", [...base, "paths"], x.paths, "Nome do campo", "Caminho no XML", "Exemplos: EventData/Data[@Name='Image'], System/Provider@Name.");
      case "key_value": return `<div class="ex-row"><label>Separador chave/valor<input data-p='${h(JSON.stringify([...base, "separator"]))}' value="${h(x.separator ?? "=")}" maxlength="8"></label><label>Entre pares<input data-p='${h(JSON.stringify([...base, "pairDelimiter"]))}' value="${h(x.pairDelimiter ?? "")}" placeholder="(espaço)" maxlength="8"></label><label>Aspas<input data-p='${h(JSON.stringify([...base, "quote"]))}' value="${h(x.quote ?? "")}" maxlength="1"></label></div>${mapEditor("Chaves lidas", [...base, "keys"], x.keys, "Nome do campo", "Chave no texto", "Somente as chaves listadas viram campos.")}`;
      case "delimited": return `<div class="ex-row"><label>Delimitador<input data-p='${h(JSON.stringify([...base, "delimiter"]))}' value="${h(x.delimiter)}" maxlength="8"></label><label>Aspas<input data-p='${h(JSON.stringify([...base, "quote"]))}' value="${h(x.quote ?? "")}" maxlength="1"></label></div><label>Colunas (uma por linha, na ordem)<textarea rows="5" data-p='${h(JSON.stringify([...base, "columns"]))}' data-kind="lines">${h((x.columns || []).join("\n"))}</textarea></label>`;
      case "regex": return `<label>Expressão (use grupos nomeados <code>(?&lt;campo&gt;…)</code>)<textarea rows="3" class="lx-code" spellcheck="false" data-p='${h(JSON.stringify([...base, "pattern"]))}'>${h(x.pattern)}</textarea></label><div class="ex-row"><label>Motor${select([...base, "engine"], x.engine || "pcre2", [["pcre2", "PCRE2"], ["rust", "Rust regex"]])}</label><label class="lx-inline"><input type="checkbox" data-p='${h(JSON.stringify([...base, "caseSensitive"]))}' data-kind="bool-true" ${x.caseSensitive === false ? "" : "checked"}> Diferenciar maiúsculas</label></div>`;
      default: return "";
    }
  }
  function stepRow(ri, name, steps, si) {
    const st = steps[si], path = ["rules", ri, "fields", name, "steps", si];
    const params = { strip_prefix: ["value", "Prefixo"], strip_suffix: ["value", "Sufixo"], remove_chars: ["chars", "Caracteres"] }[st.op];
    return `<div class="ex-row ex-step">${select([...path, "op"], st.op, STEPS)}${params ? input([...path, params[0]], st[params[0]], `placeholder="${params[1]}" aria-label="${params[1]}"`) : ""}${st.op === "replace" ? input([...path, "from"], st.from, 'placeholder="Trocar" aria-label="Trocar"') + input([...path, "to"], st.to, 'placeholder="Por" aria-label="Por"') : ""}<button type="button" class="btn ghost small" data-act="step-up" data-ri="${ri}" data-n="${h(name)}" data-s="${si}" ${si ? "" : "disabled"}>↑</button><button type="button" class="btn ghost small" data-act="step-remove" data-ri="${ri}" data-n="${h(name)}" data-s="${si}">Remover</button></div>`;
  }
  function outputRow(r, ri, name) {
    const m = r.fields[name], path = ["rules", ri, "fields", name], steps = m.steps || [];
    const rows = (S.quick?.outputs || []).find((o) => o.rule === r.id && o.field === name);
    const states = rows ? Object.entries(rows.states).map(([k, v]) => `${num(v)} ${STATUS[k] || k}`).join(" · ") : "";
    const typeIndex = [["", "Sem índice"], ["exact", "Igualdade"], ...(["integer", "decimal", "datetime"].includes(m.type) ? [["range", "Intervalos"]] : []), ...(!m.type || m.type === "text" ? [["terms", "Palavras"]] : [])];
    return `<details class="ex-output" ${S.openOutput === `${r.id}/${name}` ? "open" : ""} data-key="${h(r.id)}/${h(name)}"><summary>${swatch(name)}<strong>${h(name)}</strong><span class="lx-badge">${h(TYPES.find((t) => t[0] === (m.type || ""))?.[1] || m.type)}</span>${m.literal !== undefined ? '<span class="lx-badge">texto fixo</span>' : ""}<span class="ex-states">${h(states)}</span></summary>
      <div class="ex-output-body"><div class="ex-row"><label>Nome${input(["__rename", ri, name], name, "data-rename-output")}</label><label>Vem de (em ordem de preferência)${input([...path, "from"], (m.from || []).join(", "), 'data-kind="csv"')}</label><label>Tipo${select([...path, "type"], m.type, TYPES)}</label></div>
      ${m.literal !== undefined ? `<label>Valor fixo${input([...path, "literal"], m.literal)}</label>` : ""}
      ${m.type === "datetime" ? `<div class="ex-row"><label>Formato da data${input([...path, "datetime", "format"], m.datetime?.format, 'placeholder="iso8601, epoch_s, epoch_ms ou %d/%m/%Y %H:%M:%S" list="ex-date-formats"')}</label><label>Fuso horário${input([...path, "datetime", "timezone"], m.datetime?.timezone, 'placeholder="utc ou +03:00"')}</label><label>Ano assumido${input([...path, "datetime", "defaultYear"], m.datetime?.defaultYear, 'type="number" placeholder="só para datas sem ano" data-kind="int"')}</label></div><datalist id="ex-date-formats"><option value="iso8601"><option value="epoch_s"><option value="epoch_ms"><option value="%d/%b/%Y:%H:%M:%S %z"><option value="%Y-%m-%d %H:%M:%S"><option value="%d/%m/%Y %H:%M:%S"><option value="%b %e %H:%M:%S"></datalist>` : ""}
      <h4>Normalização, na ordem</h4>${steps.map((_, si) => stepRow(ri, name, steps, si)).join("")}<button type="button" class="btn ghost small" data-act="step-add" data-ri="${ri}" data-n="${h(name)}">Adicionar etapa</button>
      <div class="ex-row"><label>Se o valor não existir${select([...path, "onMissing"], m.onMissing, [["", "Não emitir"], ["default", "Usar valor padrão"]])}</label>${m.onMissing === "default" ? `<label>Valor padrão${input([...path, "default"], m.default)}</label>` : ""}<label>Se estiver vazio${select([...path, "onEmpty"], m.onEmpty, [["", "Não emitir"], ["keep", "Emitir texto vazio"]])}</label><label>Se a conversão falhar${select([...path, "onInvalid"], m.onInvalid, [["", "Não emitir"], ["keep_text", "Manter o texto original"]])}</label><label>Se regras discordarem${select([...path, "conflict"], m.conflict, [["", "Vale a de maior precedência"], ["reject", "Não emitir valor"]])}</label><label>Índice (preferência)${select([...path, "index"], m.index, typeIndex)}</label></div>
      <label>Nota${input([...path, "description"], m.description, 'placeholder="Para quem for ler esta definição depois"')}</label>
      <p class="lx-help">Ausente, vazio, inválido, zero e falso são estados diferentes: zero e falso são valores e nunca somem.</p>
      <button type="button" class="btn ghost small danger" data-act="output-remove" data-ri="${ri}" data-n="${h(name)}">Remover saída</button></div></details>`;
  }
  function paintCenter() {
    const node = q("#ex-center");
    if (!node) return;
    const r = rule(), ri = ruleIndex();
    node.innerHTML = `<header><h2>2 · Regras e saídas</h2></header>
      <div class="ex-row"><label class="ex-grow">Título${input(["title"], S.def.title)}</label><label>Versão${input(["version"], S.def.version, 'size="6"')}</label><label>Id${input(["id"], S.def.id)}</label></div>
      <label>Descrição${input(["description"], S.def.description, 'placeholder="Para que serve este mapa"')}</label>
      <div class="ex-rules" role="tablist" aria-label="Regras (da mais para a menos prioritária em caso de empate)">${S.def.rules.map((x) => `<button type="button" role="tab" class="ex-rule-tab ${x.id === S.ruleId ? "selected" : ""} ${x.enabled === false ? "off" : ""}" aria-selected="${x.id === S.ruleId}" data-rule="${h(x.id)}">${h(x.id)}${x.enabled === false ? " · desativada" : ""}</button>`).join("")}<button type="button" class="btn ghost small" data-act="rule-add"><i class="fas fa-plus" aria-hidden="true"></i> Regra</button></div>
      ${r ? `<section class="ex-rule" aria-label="Regra ${h(r.id)}"><div class="ex-row"><label>Id da regra${input(["__ruleid", ri], r.id)}</label><label>Prioridade${input(["rules", ri, "priority"], r.priority ?? 0, 'type="number" data-kind="int"')}</label><button type="button" class="btn ghost small" data-act="rule-up" ${ri ? "" : "disabled"}>↑</button><button type="button" class="btn ghost small" data-act="rule-down" ${ri < S.def.rules.length - 1 ? "" : "disabled"}>↓</button><button type="button" class="btn ghost small" data-act="rule-toggle">${r.enabled === false ? "Ativar" : "Desativar"}</button><button type="button" class="btn ghost small" data-act="rule-dup">Duplicar</button><button type="button" class="btn ghost small danger" data-act="rule-remove">Remover</button></div>
      <details class="ex-selector" ${Object.keys(r.selector || {}).length || (r.when || []).length ? "open" : ""}><summary>Quando aplicar</summary><div class="ex-row">${["provider:Provedor", "channel:Canal", "eventId:Código do evento", "version:Versão"].map((s) => { const [k, l] = s.split(":"); return `<label>${l}${input(["rules", ri, "selector", k], r.selector?.[k])}</label>`; }).join("")}</div><h4>Condições adicionais</h4>${(r.when || []).map((w, wi) => `<div class="ex-row"><input data-p='${h(JSON.stringify(["rules", ri, "when", wi, "column"]))}' value="${h(w.column)}" aria-label="Campo"><select data-p='${h(JSON.stringify(["rules", ri, "when", wi, "op"]))}'>${[["equals", "é igual a"], ["contains", "contém"], ["starts_with", "começa com"], ["not_empty", "existe"], ["empty", "não existe"], ["term", "tem as palavras"]].map(([v, l]) => `<option value="${v}" ${w.op === v ? "selected" : ""}>${l}</option>`).join("")}</select><input data-p='${h(JSON.stringify(["rules", ri, "when", wi, "value"]))}' value="${h(w.value)}" aria-label="Valor"><button type="button" class="btn ghost small" data-act="when-remove" data-w="${wi}">Remover</button></div>`).join("")}<button type="button" class="btn ghost small" data-act="when-add" ${(r.when || []).length >= 16 ? "disabled" : ""}>Adicionar condição</button></details>
      <h3>Extração</h3><label>Como ler o texto${select(["rules", ri, "__kind"], r.extract?.kind || "", [["", "Nenhuma (só normalizar campos existentes)"], ...KINDS])}</label><div id="ex-diagnostics"></div>${extractionEditor(r, ri)}
      <h3>Saídas</h3><div class="ex-outputs">${Object.keys(r.fields || {}).map((n) => outputRow(r, ri, n)).join("") || '<p class="lx-help">Nenhuma saída. Marque trechos nas amostras, aceite uma sugestão ou adicione uma saída.</p>'}</div>
      <form class="ex-row" data-form="output"><input name="name" placeholder="nome_da_saida" aria-label="Nome da nova saída"><input name="from" placeholder="vem de (campo extraído ou do registro)" aria-label="Origem da nova saída"><button type="submit" class="btn ghost small">Adicionar saída</button></form></section>` : '<p class="lx-empty">Adicione uma regra para começar.</p>'}`;
    paintDiagnostics(); catalogList();
  }
  function catalogList() {
    const list = q("#ex-catalog-list");
    if (!list || !S.catalog) return;
    const term = (q("#ex-catalog-q")?.value || "").trim().toLowerCase();
    const items = S.catalog.grok.patterns.filter((e) => !term || `${e.name} ${e.category} ${e.description}`.toLowerCase().includes(term)).slice(0, 40);
    list.innerHTML = items.map((e) => `<li><button type="button" class="btn ghost small" data-act="catalog-insert" data-n="${h(e.name)}" title="Inserir na alternativa em edição">${h(e.name)}</button> <small>${h(e.category)} · ${h(e.description)} Ex.: <code>${h(e.example)}</code></small></li>`).join("") || "<li>Nenhum padrão encontrado.</li>";
  }

  // ------------------------------------------------------------------ rendering: results (right)
  const TABS = [["quick", "Amostra"], ["full", "Conjunto completo"], ["diff", "Efeito da edição"], ["indexes", "Índices"], ["json", "Definição"], ["templates", "Modelos"]];
  function paintResultsHeader() {
    const node = q("#ex-tabs");
    if (!node) return;
    const html = TABS.map(([id, label]) => `<button type="button" role="tab" class="${id === S.tab ? "selected" : ""}" aria-selected="${id === S.tab}" data-tab="${id}">${h(label)}${id === "quick" && dirty() ? " •" : ""}</button>`).join("");
    if (node.dataset.html !== html) { node.dataset.html = html; node.innerHTML = html; }
  }
  function quickHtml() {
    const r = S.quick;
    if (!r) return '<p class="lx-empty">Sorteie amostras ou cole linhas para ver, registro a registro, o texto original, o que foi extraído e o valor final.</p>';
    if (r.ok === false) return `<p class="lx-notice" data-error="true">${h(r.error)}</p>`;
    const total = S.samples?.population;
    return `<p class="ex-scope"><i class="fas fa-flask" aria-hidden="true"></i> <strong>Prévia rápida</strong> · ${num(r.inspected)} ${S.samples ? `amostras de ${num(total)} registros` : "linhas coladas"}. Não representa o conjunto: use “Conjunto completo” para a contagem real.</p>
      <table class="lx-table"><thead><tr><th>Regra</th><th>Elegíveis</th><th>Reconhecidas</th><th>Não reconhecidas</th><th>Erros</th></tr></thead><tbody>${r.rules.map((x) => `<tr><td>${h(x.rule)}</td><td>${num(x.eligible)}</td><td>${num(x.recognized)}</td><td>${num(x.notRecognized)}</td><td>${num(x.errors)}</td></tr>`).join("")}</tbody></table>
      ${S.active != null && r.results[S.active] ? sampleDetail(r.results[S.active], S.active) : ""}`;
  }
  function sampleDetail(result, index) {
    const sample = S.samples?.samples?.[index];
    const textValue = sample?.text ?? S.texts[index] ?? "";
    const ranges = [];
    for (const t of result.trace?.rules || []) for (const [field, , range] of t.extracted || []) if (range) ranges.push({ start: range[0], end: range[1], field });
    const body = M.paintPlan(textValue, ranges).map((p) => (p.field ? `<mark class="ex-mark" style="--c:${M.fieldColor(p.field, colors())}">${h(p.text)}</mark>` : h(p.text))).join("");
    const outs = result.trace?.outputs || [];
    return `<h3>Amostra ${index + 1}${sample ? ` · ${h(sample.fileName)} · posição ${h(sample.offset)}` : ""}</h3><pre class="ex-sample-text ex-detail-text">${body}</pre>${result.error ? `<p class="lx-notice" data-error="true">${h(result.error)}</p>` : ""}
      <table class="lx-table ex-values"><thead><tr><th>Campo</th><th>Original</th><th>Extraído</th><th>Normalizado</th><th>Estado</th></tr></thead><tbody>${outs.map((o) => { const extracted = (result.trace.rules.flatMap((t) => t.extracted || []).find((e) => `extract:${e[0]}` === o.source) || [])[1]; return `<tr><td>${swatch(o.field)}${h(o.field)}</td><td>${h(o.original ?? "")}</td><td>${h(extracted ?? "")}${o.range ? ` <small>[${o.range[0]}–${o.range[1]}]</small>` : ""}</td><td>${o.normalized === undefined || o.normalized === null ? "—" : `<code>${h(typeof o.normalized === "string" ? o.normalized : JSON.stringify(o.normalized))}</code>`}</td><td><span class="lx-badge" data-state="${h(o.status)}" title="${h(o.reason || "")}">${h(STATUS[o.status] || o.status)}</span>${o.reason ? `<small> ${h(o.reason)}</small>` : ""}</td></tr>`; }).join("") || '<tr><td colspan="5">Nenhuma saída foi calculada para esta amostra (a regra não se aplica ou não reconheceu o texto).</td></tr>'}</tbody></table>
      <p class="lx-help">Proveniência de cada valor: regra, origem e original ficam em <code>${h(S.name)}._provenance</code> depois de salvar.</p>`;
  }
  function fullHtml() {
    const r = S.scan;
    const head = `<div class="ex-row"><button type="button" class="btn primary small" data-act="scan" ${r?.running ? "disabled" : ""}><i class="fas fa-magnifying-glass-chart" aria-hidden="true"></i> Conferir no conjunto</button><button type="button" class="btn ghost small" data-act="scan-cancel" ${r?.running ? "" : "disabled"}>Cancelar</button><label class="lx-inline"><input type="checkbox" id="ex-scan-filters"> Só o recorte atual de Explorar</label></div>`;
    if (!r) return `${head}<p class="lx-help">Passa a definição por todos os registros carregados (ou pelo recorte), em lotes, sem alterar nada, e mostra contagens exatas por arquivo, regra e saída.</p>`;
    if (r.running) return `${head}<p class="lx-help" role="status"><i class="fas fa-circle-notch spin" aria-hidden="true"></i> Conferindo o conjunto… o progresso aparece na barra de tarefas; você pode cancelar.</p>`;
    if (r.error) return `${head}<p class="lx-notice" data-error="true">${h(r.error)}</p>`;
    const ex = (items, label) => items.length ? `<h4>${label}</h4><ul class="ex-examples">${items.map((x) => `<li><code>${h(x.text)}</code><div><small>${h(x.file)}${x.offset != null ? ` · posição ${h(x.offset)}` : ""}${x.reason ? ` · ${h(x.reason)}` : ""}</small> <button type="button" class="btn ghost small" data-act="open-event" data-id="${x.id}" data-ref="${h(x.eventRef)}">Abrir registro</button> <button type="button" class="btn ghost small" data-act="open-file" data-file="${h(x.file)}">Ver arquivo</button></div></li>`).join("")}</ul>` : "";
    return `${head}<p class="ex-scope"><i class="fas fa-circle-check" aria-hidden="true"></i> <strong>Contagem completa</strong> · ${num(r.inspected)} registros conferidos em ${num(r.elapsedMs)} ms · <strong>${num(r.affected)}</strong> ganham saídas · ${num(r.conflicts)} conflito(s) por precedência.</p>
      <h4>Por regra</h4><table class="lx-table"><thead><tr><th>Regra</th><th>Elegíveis</th><th>Reconhecidos</th><th>Não reconhecidos</th><th>Erros</th></tr></thead><tbody>${r.rules.map((x) => `<tr><td>${h(x.rule)}</td><td>${num(x.eligible)}</td><td>${num(x.recognized)}</td><td>${num(x.notRecognized)}</td><td>${num(x.errors)}${x.firstError ? ` <small>${h(x.firstError)}</small>` : ""}</td></tr>`).join("")}</tbody></table>
      <h4>Por saída</h4><table class="lx-table"><thead><tr><th>Saída</th><th>Estados</th></tr></thead><tbody>${r.outputs.map((o) => `<tr><td>${swatch(o.field)}${h(o.rule)} › ${h(o.field)}</td><td>${Object.entries(o.states).map(([k, v]) => `${num(v)} ${h(STATUS[k] || k)}`).join(" · ")}</td></tr>`).join("")}</tbody></table>
      <h4>Por arquivo</h4><table class="lx-table"><thead><tr><th>Arquivo</th><th>Conferidos</th><th>Com saídas</th><th>Reconhecidos</th><th>Não reconhecidos</th></tr></thead><tbody>${r.files.map((f) => `<tr><td title="${h(f.file)}">${h(f.file.split(/[\\/]/).pop())}</td><td>${num(f.inspected)}</td><td>${num(f.affected)}</td><td>${num(f.recognized)}</td><td>${num(f.notRecognized)}</td></tr>`).join("")}</tbody></table>${ex(r.failures, "Exemplos que falharam")}${ex(r.successes, "Exemplos que funcionaram")}`;
  }
  function diffHtml() {
    const d = S.diff;
    const head = '<div class="ex-row"><button type="button" class="btn primary small" data-act="diff">Calcular o efeito</button></div>';
    if (!d) return `${head}<p class="lx-help">Compara a definição em edição com a salva no Caso: o que muda nos campos e quais índices precisam ser recriados.</p>`;
    const list = (label, items) => `<div><strong>${label}</strong> ${items.length ? items.map((f) => `<code>${h(f)}</code>`).join(", ") : "—"}</div>`;
    return `${head}${d.metadataOnly ? '<p class="ex-scope"><i class="fas fa-circle-info" aria-hidden="true"></i> Só título, notas, cores ou preferências de índice mudaram: nenhum valor é recalculado e nenhum índice é invalidado.</p>' : ""}${list("Campos novos", d.added)}${list("Campos removidos", d.removed)}${list("Campos alterados", d.changed)}${list("Campos iguais", d.unchanged)}${list("Regras novas", d.rulesAdded)}${list("Regras removidas", d.rulesRemoved)}${list("Regras alteradas", d.rulesChanged)}${d.global ? '<p class="lx-help">A alteração atinge o mapa inteiro (ativação, padrões compartilhados ou identidade).</p>' : ""}<div><strong>Índices a recriar</strong> ${d.indexesToRebuild.length ? d.indexesToRebuild.map((f) => `<code>${h(f)}</code>`).join(", ") : "nenhum"}</div>`;
  }
  const bytesLabel = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n >= 1e3 ? `${(n / 1e3).toFixed(1)} KB` : `${n} B`);
  function indexesHtml() {
    const saved = installedMaps().find((d) => d.name === S.installed);
    const list = S.index?.indexes || [];
    const outputs = saved ? M && saved.advanced.map.rules.flatMap((r) => Object.entries(r.fields).map(([n, m]) => ({ name: n, type: m.type, pref: m.index }))) : [];
    const seen = new Set();
    const rows = outputs.filter((o) => !seen.has(o.name) && seen.add(o.name)).map((o) => {
      const column = `${S.installed}.${o.name}`;
      const kinds = [["exact", "Igualdade"], ...(["integer", "decimal", "datetime"].includes(o.type) ? [["range", "Intervalos"]] : []), ...(!o.type || o.type === "text" ? [["terms", "Palavras"]] : [])];
      return `<tr><td>${swatch(o.name)}${h(column)}</td><td>${h(TYPES.find((t) => t[0] === (o.type || ""))?.[1] || "")}</td><td>${h(o.pref || "—")}</td><td>${kinds.map(([k, l]) => { const built = list.find((i) => i.field === column && i.kind === k); return `<div class="ex-index">${h(l)}: ${built ? `${bytesLabel(built.bytes)} <button type="button" class="btn ghost small" data-act="index-remove" data-c="${h(column)}" data-k="${k}">Remover</button>` : `<button type="button" class="btn ghost small" data-act="index-build" data-c="${h(column)}" data-k="${k}" ${S.building ? "disabled" : ""}>Criar</button>`}</div>`; }).join("")}</td></tr>`;
    });
    return `<p class="lx-help">Índices são opcionais, locais ao Caso e à fonte, e só aceleram buscas: com ou sem eles o resultado é o mesmo. Igualdade serve para <code>=</code>; Intervalos para &gt;, &lt; e entre (números e datas); Palavras para o operador “tem as palavras”. Consultas SQL não usam estes índices.</p>
      ${!saved ? '<p class="lx-notice">Salve o mapa no Caso para criar índices de suas saídas: eles são calculados sobre a definição salva.</p>' : dirty() ? '<p class="lx-notice">Há alterações não salvas. Os índices abaixo valem para a definição salva.</p>' : ""}
      ${S.building ? `<p role="status"><i class="fas fa-circle-notch spin" aria-hidden="true"></i> Criando índice de ${h(S.building)}… <button type="button" class="btn ghost small" data-act="index-cancel">Cancelar</button></p>` : ""}
      <table class="lx-table"><thead><tr><th>Campo</th><th>Tipo</th><th>Preferência</th><th>Índices (tamanho em disco)</th></tr></thead><tbody>${rows.join("") || '<tr><td colspan="4">Nenhuma saída salva.</td></tr>'}</tbody></table>`;
  }
  function jsonHtml() {
    return `<p class="lx-help">Esta é a mesma definição que as telas editam. Alterar aqui atualiza as telas e vice-versa.</p><textarea id="ex-json" class="lx-code" rows="24" spellcheck="false" aria-label="Definição em JSON">${h(S.json || text())}</textarea><div class="ex-row"><button type="button" class="btn primary small" data-act="json-apply">Aplicar JSON</button><button type="button" class="btn ghost small" data-act="json-reset">Restaurar do editor visual</button></div><div id="ex-json-msg" class="lx-help" role="status"></div>`;
  }
  function templatesHtml() {
    const list = S.catalog?.templates || [];
    return `<p class="lx-help">Pontos de partida completos. Escolher um modelo substitui a definição em edição (você pode desfazer) e experimenta suas linhas de exemplo na hora.</p><ul class="ex-templates">${list.map((t, i) => `<li><div><strong>${h(t.title)}</strong><div>${h(t.description)}</div></div><button type="button" class="btn ghost small" data-act="template" data-i="${i}">Usar este modelo</button></li>`).join("") || "<li>Carregando modelos…</li>"}</ul>`;
  }
  function paintRight() {
    const node = q("#ex-right-body");
    if (!node) return;
    paintResultsHeader();
    node.innerHTML = { quick: quickHtml, full: fullHtml, diff: diffHtml, indexes: indexesHtml, json: jsonHtml, templates: templatesHtml }[S.tab]();
  }
  function paintAll() { paintHeader(); paintLeft(); paintCenter(); paintRight(); }

  // ------------------------------------------------------------------ actions
  function selectedSample() { const s = S.selection; return s ? { ...s, text: (S.samples?.samples?.[s.sample]?.text ?? S.texts[s.sample] ?? "") } : null; }
  function ensureGrok(r) {
    if (!r.extract || r.extract.kind !== "grok") r.extract = { kind: "grok", patterns: [""], anchor: "search", source: "message" };
    return r.extract;
  }
  function addOutputsFor(r, segments) {
    for (const s of segments) if (s.kind === "field" && s.name && !r.fields[s.name]) M.ensureOutput(r, s.name, s.name, M.typeForToken(s.token, s.conv));
  }
  function regenerate(sampleIndex) {
    const r = rule(), seg = S.segs[S.ruleId];
    const sampleText = S.samples?.samples?.[sampleIndex]?.text ?? S.texts[sampleIndex] ?? "";
    const grok = ensureGrok(r);
    grok.patterns[0] = M.compile(sampleText, seg.list);
    addOutputsFor(r, seg.list);
    // Segments that no longer exist stop owning their outputs only if the user removes them; nothing is deleted silently.
    commit(); paintCenter(); paintSamples();
  }
  function visualAction(id) {
    const sel = selectedSample();
    if (!sel || !rule()) return;
    const r = rule();
    const current = S.segs[S.ruleId]?.sample === sel.sample ? S.segs[S.ruleId].list : [];
    if (id === "split") {
      const delimiter = M.delimiterOf(sel.text, sel);
      if (!delimiter) { say("Selecione o delimitador (até 8 caracteres).", true); return; }
      const columns = sel.text.split(delimiter).length;
      r.extract = { kind: "delimited", delimiter, quote: '"', columns: Array.from({ length: columns }, (_, i) => `coluna_${i + 1}`), source: S.samples?.field === "raw" ? "raw" : "message" };
      for (const name of r.extract.columns) M.ensureOutput(r, name, name, "text");
      commit(); paintCenter(); say(`Texto separado por “${delimiter}” em ${columns} colunas. Dê nomes às colunas ao lado.`); return;
    }
    let options = {};
    if (id === "between") {
      const open = prompt("Marcador inicial (texto que vem antes do valor):", sel.text.slice(sel.start, sel.end));
      const close = open == null ? null : prompt("Marcador final (texto que vem depois do valor):", "");
      if (!open || !close) return;
      options = { open, close };
    }
    if (id === "extract" || id === "recognize") {
      const picked = sel.text.slice(sel.start, sel.end);
      options.hint = M.nameBefore(sel.text, sel.start);
      if (id === "recognize") {
        void recognize(picked).then((token) => { if (token) apply(id, sel, current, { ...options, token }); else apply("extract", sel, current, options); });
        return;
      }
    }
    apply(id, sel, current, options);
  }
  async function recognize(picked) {
    try {
      const r = await call({ action: "map_suggest", texts: [picked] }, { latest: "map-recognize" });
      const best = (r.suggestions || []).filter((s) => s.kind === "value").sort((a, b) => b.found - a.found)[0];
      if (best) { say(`Reconhecido como ${best.label.toLowerCase()}.`); return best.expression; }
    } catch { /* falls back to the character-based choice */ }
    say("Formato não reconhecido; usando o padrão que combina com os caracteres.");
    return null;
  }
  function apply(id, sel, current, options) {
    const result = M.apply(sel.text, current, id, sel, options);
    if (result.error) { say(result.error, true); return; }
    S.segs[S.ruleId] = { sample: sel.sample, list: result.segments };
    say();
    regenerate(sel.sample);
    void evaluateQuick();
  }
  function acceptSuggestion(group, i, adjust) {
    const r = rule();
    if (!r) return;
    if (group === "x") {
      const s = S.suggest.extractions[i];
      r.extract = structuredClone(s.extraction);
      const names = s.extraction.kind === "key_value" ? Object.keys(s.extraction.keys) : s.extraction.kind === "delimited" ? s.extraction.columns : Object.keys(s.extraction.paths);
      for (const n of names) M.ensureOutput(r, n, n, "text");
      commit(); paintCenter(); void evaluateQuick(); say(`Extração “${s.label}” aplicada à regra ${r.id}. Ajuste nomes e tipos ao lado.`); return;
    }
    const s = S.suggest.suggestions.filter((x) => x.kind === "value")[i];
    const at = (s.positions || []).find((p) => p.sample === S.active) || s.positions?.[0];
    if (!at) return;
    const sampleText = S.samples?.samples?.[at.sample]?.text ?? S.texts[at.sample] ?? "";
    const taken = new Set((S.segs[S.ruleId]?.list || []).map((x) => x.name));
    const base = S.segs[S.ruleId]?.sample === at.sample ? S.segs[S.ruleId].list : [];
    const added = M.addSegment(base, { kind: "field", start: at.start, end: at.end, token: s.expression, name: M.uniqueName(adjust ? prompt("Nome do campo:", s.field) || s.field : s.field, taken), conv: "" });
    if (added.error) { say(added.error, true); return; }
    S.segs[S.ruleId] = { sample: at.sample, list: added.segments };
    S.active = at.sample;
    regenerate(at.sample); void evaluateQuick();
  }
  function importText(raw) {
    try {
      const parsed = JSON.parse(raw);
      const def = parsed.advanced?.map ?? parsed.map ?? parsed;
      if (!def || !Array.isArray(def.rules)) throw Error("O arquivo não contém uma definição de mapa.");
      setDef(def); commit(); paintAll(); say("Definição importada. Revise e salve para aplicar ao Caso.");
    } catch (e) { say(`Importação recusada: ${e.message || e}`, true); }
  }
  function exportDef() {
    const blob = new Blob([text()], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob); a.download = `${S.def.id || "mapa"}.json`; a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    navigator.clipboard?.writeText(text()).then(() => say("Definição exportada e copiada para a área de transferência.")).catch(() => say("Definição exportada."));
  }
  // Leaving a field first lets its pending change land, so it cannot overwrite the restored text.
  function travel(direction) { document.activeElement?.blur?.(); applyHistory(S.hist[direction]()); }
  function applyHistory(value) { if (value == null) return; S.def = JSON.parse(value); S.json = value; if (!rule()) S.ruleId = S.def.rules[0]?.id ?? null; saveDraft(); refreshDerived(); paintAll(); S.onChange?.(); }
  /** Renames a key of an object inside the definition, keeping the order of the others. */
  function renameField(path, oldKey, value) {
    let t = S.def;
    for (const p of path) t = t[p];
    if (!value || value === oldKey || value in t) return;
    const entries = Object.entries(t).map(([k, v]) => [k === oldKey ? value : k, v]);
    for (const k of Object.keys(t)) delete t[k];
    for (const [k, v] of entries) t[k] = v;
  }
  async function onClick(event) {
    const target = event.target.closest("[data-act],[data-tab],[data-rule],[data-sel],.ex-sample");
    if (!target || !root.contains(target)) return;
    const g = generation;
    try {
      if (target.dataset.tab) { S.tab = target.dataset.tab; if (S.tab === "indexes") void refreshIndexes(); paintRight(); return; }
      if (target.dataset.rule) { S.ruleId = target.dataset.rule; S.chip = null; paintCenter(); paintSamples(); return; }
      if (target.dataset.sel) { visualAction(target.dataset.sel); return; }
      if (target.classList.contains("ex-sample") && !target.dataset.act) { const i = +target.dataset.i; if (i !== S.active) { S.active = i; paintSamples(); paintRight(); } return; }
      await act(target, g);
    } catch (e) { if (alive(g) && !window.LocalNative.superseded(e)) say(String(e.message || e), true); }
  }
  async function act(t, g) {
    const d = t.dataset, r = rule(), ri = ruleIndex();
    const mutate = (fn) => { fn(); commit(); paintCenter(); };
    switch (d.act) {
      case "view-easy": setView("easy"); break;
      case "undo": travel("undo"); break;
      case "redo": travel("redo"); break;
      case "ex-save": await save(); break;
      case "discard": if (confirm("Descartar o rascunho e voltar ao mapa salvo?")) { dropDraft(); loadInstalled(S.installed, { ignoreDraft: true }); paintAll(); void check(); void evaluateQuick(); } break;
      case "duplicate": { const name = prompt("Nome do novo campo:", `${S.installed}_copia`); if (name) { S.installed = null; S.name = name; S.base = ""; S.def.id = `${S.def.id}-copia`; S.def.title = `${S.def.title} (cópia)`; commit(); paintAll(); say("Cópia criada como rascunho. Salve para aplicá-la ao Caso."); } break; }
      case "toggle": { S.def.enabled = S.def.enabled === false ? true : false; if (S.def.enabled) delete S.def.enabled; commit(); await save(); break; }
      case "remove": if (confirm(`Remover o mapa “${S.installed}” do Caso? Os campos ${S.installed}.* deixam de existir. Os arquivos originais não são tocados.`)) { await api("delete_derived_field", { name: S.installed }, { analysisOwner: window.AnalysisContexts.capture() }); await loadDerivedFields(); dropDraft(); loadInstalled(null); paintAll(); say("Mapa removido do Caso."); } break;
      case "import": q("#ex-import-file").click(); break;
      case "export": exportDef(); break;
      case "sample": await drawSamples(!!S.samples); break;
      case "paste": { S.texts = (q("#ex-paste").value || "").split(/\r?\n/).filter(Boolean).slice(0, 200); S.samples = null; S.active = 0; S.quick = null; paintLeft(); void suggestNow(); void evaluateQuick(); break; }
      case "accept": acceptSuggestion(d.g, +d.i, false); break;
      case "adjust": acceptSuggestion(d.g, +d.i, true); break;
      case "dismiss": { const list = d.g === "x" ? S.suggest.extractions : S.suggest.suggestions.filter((x) => x.kind === "value"); const gone = list[+d.i]; if (d.g === "x") S.suggest.extractions.splice(+d.i, 1); else S.suggest.suggestions = S.suggest.suggestions.filter((x) => x !== gone); paintSuggestions(); break; }
      case "rule-add": mutate(() => { const rr = M.blankRule(M.uniqueRuleId(S.def)); rr.extract = { kind: "grok", patterns: [""], anchor: "search", source: "message" }; S.def.rules.push(rr); S.ruleId = rr.id; }); break;
      case "rule-up": mutate(() => { [S.def.rules[ri - 1], S.def.rules[ri]] = [S.def.rules[ri], S.def.rules[ri - 1]]; }); break;
      case "rule-down": mutate(() => { [S.def.rules[ri + 1], S.def.rules[ri]] = [S.def.rules[ri], S.def.rules[ri + 1]]; }); break;
      case "rule-toggle": mutate(() => { if (r.enabled === false) delete r.enabled; else r.enabled = false; }); break;
      case "rule-dup": mutate(() => { const copy = structuredClone(r); copy.id = M.uniqueRuleId(S.def, r.id); S.def.rules.splice(ri + 1, 0, copy); S.ruleId = copy.id; }); break;
      case "rule-remove": if (confirm(`Remover a regra ${r.id}?`)) mutate(() => { S.def.rules.splice(ri, 1); S.ruleId = S.def.rules[0]?.id ?? null; }); break;
      case "when-add": mutate(() => { (r.when ||= []).push({ column: "message", op: "contains", value: "" }); }); break;
      case "when-remove": mutate(() => { r.when.splice(+d.w, 1); if (!r.when.length) delete r.when; }); break;
      case "alt-add": mutate(() => { r.extract.patterns.push(""); }); break;
      case "alt-remove": mutate(() => { r.extract.patterns.splice(+d.a, 1); }); break;
      case "alt-up": mutate(() => { const p = r.extract.patterns, a = +d.a; [p[a - 1], p[a]] = [p[a], p[a - 1]]; }); break;
      case "alt-down": mutate(() => { const p = r.extract.patterns, a = +d.a; [p[a + 1], p[a]] = [p[a], p[a + 1]]; }); break;
      case "chip": S.chip = { rule: r.id, alt: +d.a, k: +d.k }; paintCenter(); break;
      case "chip-close": S.chip = null; paintCenter(); break;
      case "diag": { const dg = (S.error && !S.diagnostics.length ? [{ message: S.error }] : []).concat(S.diagnostics)[+d.i]; const area = q(`[data-grok="${dg.alternative ?? 0}"]`); if (area && dg.end > dg.start) { area.focus(); area.setSelectionRange(dg.start, dg.end); } break; }
      case "catalog-insert": { const area = q("[data-grok]:focus") || q(`[data-grok="${S.chip?.alt ?? 0}"]`); if (r?.extract?.kind === "grok") { const a = +(area?.dataset.grok ?? 0); const p = r.extract.patterns[a] || ""; const at = area && area.selectionStart != null ? area.selectionStart : p.length; mutate(() => { r.extract.patterns[a] = `${p.slice(0, at)}%{${d.n}}${p.slice(at)}`; }); } break; }
      case "custom-remove": mutate(() => { delete S.def.grokPatterns[d.n]; if (!Object.keys(S.def.grokPatterns).length) delete S.def.grokPatterns; }); break;
      case "map-add": mutate(() => { let t = S.def; for (const p of JSON.parse(d.path)) t = t[p]; let n = 1; while (`campo_${n}` in t) n++; t[`campo_${n}`] = ""; }); break;
      case "map-remove": mutate(() => { let t = S.def; for (const p of JSON.parse(d.path)) t = t[p]; delete t[d.k]; }); break;
      case "step-add": mutate(() => { (r.fields[d.n].steps ||= []).push({ op: "trim" }); }); break;
      case "step-remove": mutate(() => { r.fields[d.n].steps.splice(+d.s, 1); if (!r.fields[d.n].steps.length) delete r.fields[d.n].steps; }); break;
      case "step-up": mutate(() => { const s = r.fields[d.n].steps, i = +d.s; [s[i - 1], s[i]] = [s[i], s[i - 1]]; }); break;
      case "output-remove": mutate(() => { delete S.def.rules[+d.ri].fields[d.n]; }); break;
      case "scan": await scan(); break;
      case "scan-cancel": window.LocalNative.cancel("map-scan"); break;
      case "diff": await computeDiff(); paintRight(); break;
      case "open-event": await openDetail(+d.id, { eventRef: d.ref }); break;
      case "open-file": await window.FileViewer?.open(d.file); break;
      case "template": { const tpl = S.catalog.templates[+d.i]; setDef(structuredClone(tpl.map)); S.name = S.installed || tpl.map.id; S.texts = tpl.samples.slice(); S.samples = null; S.quick = null; S.segs = {}; commit(); S.tab = "quick"; paintAll(); void suggestNow(); void evaluateQuick(); say(`Modelo “${tpl.title}” carregado como rascunho.`); break; }
      case "json-apply": { try { const parsed = JSON.parse(q("#ex-json").value); if (!Array.isArray(parsed.rules)) throw Error("A definição precisa de uma lista de regras."); S.def = parsed; S.json = text(); commit(); paintAll(); S.tab = "json"; paintRight(); q("#ex-json-msg").textContent = "Definição aplicada."; } catch (e) { q("#ex-json-msg").textContent = `JSON recusado: ${e.message || e}`; } break; }
      case "json-reset": S.json = text(); paintRight(); break;
      case "index-build": { S.building = d.c; paintRight(); try { S.index = await api("field_index_create", { column: d.c, kind: d.k }, { analysisOwner: window.AnalysisContexts.capture(), latest: "field-index" }); } catch (e) { if (!window.LocalNative.superseded(e)) say(String(e.message || e), true); } S.building = null; if (alive(g)) paintRight(); window.FieldIndexes?.refresh(); break; }
      case "index-cancel": window.Tasks?.cancelLatest("field-index"); break;
      case "index-remove": S.index = await api("field_index_remove", { column: d.c, kind: d.k }, { analysisOwner: window.AnalysisContexts.capture() }); paintRight(); window.FieldIndexes?.refresh(); break;
      default: break;
    }
  }
  function onChange(event) {
    const el = event.target;
    if (el.id === "ex-installed") { loadInstalled(el.value || null); paintAll(); void check(); void evaluateQuick(); return; }
    if (el.id === "ex-name") { S.name = el.value.trim(); const help = q("#ex-name-help"); if (help) paintHeader(); return; }
    if (el.dataset.src !== undefined) { const picked = S.pickedSources || new Set(S.sources.map((s) => s.path)); if (el.checked) picked.add(el.dataset.src); else picked.delete(el.dataset.src); S.pickedSources = picked.size === S.sources.length ? null : picked; return; }
    if (el.id === "ex-catalog-q") { catalogList(); return; }
    if (el.dataset.rename) { renameField(JSON.parse(el.dataset.rename), el.dataset.old, el.value.trim()); commit(); paintCenter(); return; }
    if (el.dataset.renameOutput !== undefined) { const [, ri, old] = JSON.parse(el.dataset.p); const value = M.clean(el.value); const r = S.def.rules[ri]; if (value && !(value in r.fields)) { r.fields = Object.fromEntries(Object.entries(r.fields).map(([k, v]) => [k === old ? value : k, v])); S.openOutput = `${r.id}/${value}`; commit(); paintCenter(); } return; }
    if (!el.dataset.p) return;
    const path = JSON.parse(el.dataset.p);
    if (path[0] === "__ruleid") { const old = S.def.rules[path[1]].id, value = el.value.trim(); if (value && !S.def.rules.some((r) => r.id === value)) { S.def.rules[path[1]].id = value; S.ruleId = value; commit(); paintCenter(); } else el.value = old; return; }
    if (path[2] === "__kind") { const kind = el.value; const r = S.def.rules[path[1]]; const defaults = { grok: { kind, patterns: [""], anchor: "search", source: "message" }, dissect: { kind, pattern: "", source: "message" }, json: { kind, paths: { campo_1: "" }, source: "message" }, xml: { kind, paths: { campo_1: "" }, source: "raw" }, delimited: { kind, delimiter: ",", quote: '"', columns: ["coluna_1"], source: "message" }, key_value: { kind, separator: "=", quote: '"', keys: { campo_1: "campo_1" }, source: "message" }, regex: { kind, pattern: "", engine: "pcre2", source: "message" } }; if (kind) r.extract = defaults[kind]; else delete r.extract; commit(); paintCenter(); return; }
    const kind = el.dataset.kind;
    let value = el.value;
    if (kind === "bool") value = el.checked ? true : undefined;
    else if (kind === "bool-true") value = el.checked ? undefined : false;
    else if (kind === "csv") value = value.split(",").map((s) => s.trim()).filter(Boolean);
    else if (kind === "lines") value = value.split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
    else if (kind === "int") value = value === "" ? undefined : Number.parseInt(value, 10);
    else if (path[path.length - 1] === "priority") value = Number.parseInt(value || "0", 10) || 0;
    if (path.at(-1) === "literal" && value === "") value = "";
    const chain = [S.def];
    for (let i = 0; i < path.length - 1; i++) {
      if (chain.at(-1)[path[i]] === undefined) chain.at(-1)[path[i]] = typeof path[i + 1] === "number" ? [] : {};
      chain.push(chain.at(-1)[path[i]]);
    }
    const target = chain.at(-1), last = path.at(-1);
    if (value === undefined || (value === "" && !["literal", "value", "to"].includes(last))) delete target[last];
    else target[last] = value;
    if (last === "type") {
      if (value !== "datetime") delete target.datetime;
      if (target.index === "range" && !["integer", "decimal", "datetime"].includes(value)) delete target.index;
      if (target.index === "terms" && value && value !== "text") delete target.index;
    }
    // An emptied sub-object (the date format, the selector) leaves the file instead of staying as {}.
    for (let i = chain.length - 1; i > 0; i--) {
      if (["datetime", "selector"].includes(path[i - 1]) && !Object.keys(chain[i]).length) delete chain[i - 1][path[i - 1]];
      else break;
    }
    commit();
    if (["type", "onMissing", "kind", "extract"].includes(last) || kind === "bool") paintCenter();
  }
  function onInput(event) {
    const el = event.target;
    if (el.dataset.grok !== undefined) {
      const path = JSON.parse(el.dataset.p);
      S.def.rules[path[1]].extract.patterns[+el.dataset.grok] = el.value;
      S.hist.push(text()); S.json = text(); saveDraft(); refreshDerived(); paintHeader(); paintResultsHeader();
      const chips = el.closest(".ex-alt")?.querySelector(".ex-chips");
      if (chips) chips.innerHTML = chipsHtml(el.value, +el.dataset.grok);
      return;
    }
    if (el.id === "ex-catalog-q") catalogList();
  }
  function onSubmit(event) {
    const form = event.target.closest("form[data-form]");
    if (!form) return;
    event.preventDefault();
    const data = Object.fromEntries(new FormData(form).entries()), r = rule(), ri = ruleIndex();
    if (form.dataset.form === "chip" && S.chip) {
      const p = r.extract.patterns[S.chip.alt], chip = M.chips(p)[S.chip.k];
      r.extract.patterns[S.chip.alt] = p.slice(0, chip.start) + M.chipText({ type: "token", pattern: data.pattern.trim(), field: M.clean(data.field), conv: data.conv }) + p.slice(chip.end);
      const name = M.clean(data.field);
      if (name && !r.fields[name]) M.ensureOutput(r, name, name, M.typeForToken(data.pattern.trim(), data.conv));
      S.chip = null; commit(); paintCenter(); void evaluateQuick();
    } else if (form.dataset.form === "custom") {
      const name = (data.name || "").trim();
      if (!name || !data.def) return;
      (S.def.grokPatterns ||= {})[name] = data.def; commit(); paintCenter();
    } else if (form.dataset.form === "output") {
      const name = M.clean(data.name);
      if (!name || r.fields[name]) { say("Informe um nome novo para a saída.", true); return; }
      M.ensureOutput(r, name, (data.from || name).trim(), ""); r.fields[name].from = (data.from || name).split(",").map((s) => s.trim()).filter(Boolean);
      S.openOutput = `${r.id}/${name}`; commit(); paintCenter();
    }
    void ri;
  }
  function onSelect() {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !root) { if (S.selection) { S.selection = null; paintActions(); } return; }
    const pre = sel.anchorNode?.parentElement?.closest?.(".ex-sample-text");
    if (!pre || !pre.closest("#ex-sample-list")) return;
    const within = (node, offset) => { const range = document.createRange(); range.selectNodeContents(pre); range.setEnd(node, offset); return range.toString().length; };
    let start = within(sel.anchorNode, sel.anchorOffset), end = within(sel.focusNode, sel.focusOffset);
    if (start > end) [start, end] = [end, start];
    if (start === end) return;
    S.selection = { sample: +pre.dataset.i, start, end };
    S.active = +pre.dataset.i;
    paintActions();
  }
  function onContext(event) {
    const pre = event.target.closest?.("#ex-sample-list .ex-sample-text");
    if (!pre || !menu) return;
    onSelect();
    if (!S.selection) return;
    event.preventDefault();
    menu.open(event.clientX, event.clientY, ACTIONS.map(([id, label, icon]) => ({ icon, label, onClick: () => visualAction(id) })), { trigger: pre, label: "Ações sobre o trecho selecionado" });
  }
  function onKey(event) {
    if (!(event.ctrlKey || event.metaKey) || event.target.closest("textarea,input,select")) return;
    if (event.key === "z") { event.preventDefault(); travel("undo"); }
    if (event.key === "y") { event.preventDefault(); travel("redo"); }
  }

  // ------------------------------------------------------------------ lifecycle
  async function mount(container) {
    root = container; generation++;
    const current = window.LocalNative.stamp();
    // A different Case or source never inherits the previous one's samples, results or definition.
    const switched = !S.def || (ownerKey && ownerKey !== current);
    if (switched) { S.samples = null; S.quick = null; S.scan = null; S.diff = null; S.texts = []; S.segs = {}; S.pickedSources = null; S.index = null; S.suggest = null; }
    ownerKey = current;
    root.innerHTML = `<div id="ex-easy" class="exe" ${S.view === "adv" ? "hidden" : ""}></div><section class="ex" id="ex-adv" ${S.view === "adv" ? "" : "hidden"} aria-label="Extração e normalização — avançado"><div id="ex-head" class="ex-head"></div><div class="ex-grid"><section id="ex-left" class="lx-pane ex-pane" aria-label="Amostras"></section><section id="ex-center" class="lx-pane ex-pane" aria-label="Regras e saídas"></section><section class="lx-pane ex-pane" aria-label="Resultado e verificação"><header><h2>3 · Resultado e verificação</h2></header><div id="ex-tabs" class="ex-tabs" role="tablist"></div><div id="ex-right-body" class="ex-right"></div></section></div></section>`;
    const g = generation;
    root.onclick = onClick; root.onchange = onChange; root.oninput = onInput; root.onsubmit = onSubmit; root.oncontextmenu = onContext; root.onkeydown = onKey;
    document.removeEventListener("selectionchange", onSelect);
    document.addEventListener("selectionchange", onSelect);
    try { await window.LocalNative.ready(); } catch (e) { say(String(e.message || e), true); }
    if (!alive(g)) return;
    try { S.sources = (await api("list_sources", {}, { silent: true })) || []; } catch { S.sources = []; }
    if (!alive(g)) return;
    await loadDerivedFields().catch(() => {});
    if (!alive(g)) return;
    if (switched) {
      const wanted = prefs().current && prefs().current !== "__novo__" ? prefs().current : null;
      loadInstalled(wanted && installedMaps().some((d) => d.name === wanted) ? wanted : installedMaps()[0]?.name ?? null);
    }
    paintAll();
    window.ExtractionEasy?.mount($("#ex-easy"), core);
    void check(); void refreshIndexes();
    call({ action: "map_catalog" }, { latest: "map-catalog" }).then((c) => { if (alive(g)) { S.catalog = c; paintCenter(); if (S.tab === "templates") paintRight(); } }).catch(() => {});
    if (S.samples || S.texts.length) void evaluateQuick();
  }
  function unmount() {
    document.removeEventListener("selectionchange", onSelect);
    window.LocalNative.cancel("map-sample"); window.LocalNative.cancel("map-quick");
    Object.values(timers).forEach(clearTimeout); timers = {};
    window.ExtractionEasy?.unmount?.();
    root = null; generation++;
  }
  function setView(view) {
    S.view = view;
    const easy = q("#ex-easy"), adv = q("#ex-adv");
    if (!easy || !adv) return;
    easy.hidden = view !== "easy"; adv.hidden = view !== "adv";
    if (view === "adv") { paintAll(); void check(); } else window.ExtractionEasy?.refresh?.();
  }
  const core = { S, M, call, alive: () => alive(generation), text, commit, save, check, scan, computeDiff, installedMaps, say, paintHeader, setView, refreshIndexes, saveDraft, loadInstalled, get generation() { return generation; }, get root() { return root; } };
  document.addEventListener("analysis-context-change", () => { if (root && ownerKey && ownerKey !== window.LocalNative.stamp()) void mount(root); });
  return { mount, unmount, state: S, _model: M, core };
})();
