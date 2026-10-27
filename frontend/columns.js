/* Extração e normalização, fluxo "Colunas": uma tela só. A amostra mostra o que o aplicativo já
 * reconhece (campos achados na indexação) e o que o usuário já virou coluna; selecionar um trecho
 * propõe padrões verificados (motor em columns-engine.js, confirmação pelo motor nativo PCRE2);
 * a tabela embaixo lista as colunas. Cada coluna é uma regra da mesma definição do modo avançado. */
window.ExtractionColumns = (() => {
  "use strict";
  const E = window.ColumnsEngine;
  const h = (v) => esc(String(v ?? ""));
  const num = (n) => Number(n || 0).toLocaleString("pt-BR");
  let core = null, root = null, gen = 0, toastTimer = null, timers = {};
  const C = { files: [], fi: 0, recs: [], idx: 0, population: 0, loading: false, error: "", framing: null, draft: null, res: null, mode: "grok", resKey: "", serial: 0, sampleSerial: 0 };
  const q = (s) => root?.querySelector(s);
  const alive = (g) => g === gen && !!root;
  const later = (key, fn, wait) => { clearTimeout(timers[key]); timers[key] = setTimeout(fn, wait); };
  const PALETTE = ["#2f7fd1", "#e07a1f", "#2b9a57", "#b254c0", "#c79a1b", "#d4476a", "#2aa79b", "#7a6ee0"];
  const color = (n) => { let x = 7; for (const c of n) x = (x * 31 + c.charCodeAt(0)) >>> 0; return PALETTE[x % PALETTE.length]; };
  const ICON = {
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>', check: '<path d="M5 12l5 5 9-10"/>', x: '<path d="M6 6l12 12M18 6L6 18"/>',
    undo: '<path d="M9 14L4 9l5-5"/><path d="M4 9h10a6 6 0 0 1 0 12h-3"/>', redo: '<path d="M15 14l5-5-5-5"/><path d="M20 9H10a6 6 0 0 0 0 12h3"/>',
    left: '<path d="M15 6l-6 6 6 6"/>', right: '<path d="M9 6l6 6-6 6"/>', edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', col: '<rect x="4" y="5" width="16" height="14" rx="2"/><path d="M10 5v14M4 11h16"/>',
    trash: '<path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13"/>', type: '<path d="M5 6V4h14v2M12 4v16M9 20h6"/>', copy: '<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
    code: '<path d="M8 7l-5 5 5 5M16 7l5 5-5 5"/>', spark: '<path d="M12 3l2.2 5.3L20 10l-5.8 1.7L12 17l-2.2-5.3L4 10l5.8-1.7z"/>',
  };
  const svg = (n) => `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[n]}</svg>`;
  const INFO = {
    geral: ["Como usar", "<p>Selecione um trecho da amostra e clique em <b>Criar coluna</b> (ou clique num pedaço sublinhado). O aplicativo propõe <b>padrões possíveis</b>: escolha um e veja a coluna nova, em destaque, na tabela.</p><p>Dê um nome, confira o tipo e clique em Adicionar. Depois, <b>Aplicar</b> grava as colunas no Caso.</p>"],
    amostra: ["Amostra", "<p>Cinza: campos que o aplicativo já reconheceu ao indexar. Colorido: colunas que você criou. Tracejado: a coluna em criação.</p><p>Use as setas para ver outros registros: os padrões valem para todos.</p>"],
    registro: ["O que é um registro", "<p>Alguns logs têm várias linhas por registro (como a pilha de uma exceção Java). O aplicativo detecta isso sozinho; se precisar, informe uma expressão que marque o <b>começo</b> de cada registro.</p><p>A expressão vale para uma linha física (sintaxe Rust regex) e fica salva por arquivo. O arquivo é lido de novo ao salvar, e os padrões passam a valer para o registro inteiro.</p>"],
    nova: ["Nova coluna", "<p>Cada coluna tem o seu próprio padrão. Troque o padrão pela lista, ou escreva a expressão (Grok ou Regex) e veja o resultado na hora.</p>"],
    padroes: ["Padrões possíveis", "<p>Combinamos <b>o que o valor é</b> (IP, data, palavra, formato do texto…) com <b>onde ele está</b> (começo ou fim da linha, depois de uma palavra, valor de uma chave, entre parênteses, antes de um texto…).</p><p>Cada padrão foi conferido pelo motor do aplicativo: ele devolve exatamente o trecho que você escolheu, na posição escolhida. O número à direita é em quantas amostras ele encontra um valor.</p>"],
    tabela: ["Colunas", "<p>A coluna em criação aparece em destaque, no topo. Depois vêm as suas colunas e as que o aplicativo já reconhecia. Duplo clique no nome para renomear; clique no tipo para trocar.</p><p>Os tipos seguem referências como ECS (Elastic), os padrões Grok e o Splunk CIM.</p>"],
  };

  // ------------------------------------------------------------------ definição
  const def = () => core.S.def;
  const metaAll = () => ((def().ui ||= {}).columns ||= {});
  const gname = (name) => name.replace(/[^\w]/g, "_").replace(/^(\d)/, "_$1");
  const columns = () => (core.S.def?.rules || []).filter((r) => r.id.startsWith("col-") && core.S.def.ui?.columns?.[r.description]);
  const colNames = () => columns().map((r) => r.description);
  const takenNames = (except) => [...detectedCols().map((c) => c.name), ...colNames()].filter((n) => n !== except);
  const uniqName = (n, except) => { const t = new Set(takenNames(except)); let x = n, k = 2; while (t.has(x)) x = `${n}_${k++}`; return x; };

  /** Tipo de conversão do motor para um tipo do catálogo, olhando um valor de exemplo (formato da data). */
  function typeSpec(typeId, example) {
    const base = E.T(typeId).b;
    if (base === "inteiro") return { type: "integer" };
    if (base === "decimal") return { type: "decimal" };
    if (base === "booleano") return { type: "boolean" };
    if (base === "ip") return { type: "ip" };
    if (base !== "data-hora") return { type: "text" };
    const ex = String(example ?? "");
    if (typeId === "httpdate") return { type: "datetime", datetime: { format: "%d/%b/%Y:%H:%M:%S %z" } };
    if (typeId === "epoch_ms" || typeId === "epoch_s") return { type: "datetime", datetime: { format: typeId } };
    if (typeId === "iso" && /^\d{4}-\d\d-\d\d[T ]\d\d:\d\d:\d\d/.test(ex)) {
      const zone = /(Z|[+-]\d\d:\d\d)$/.test(ex);
      if (zone) return /T/.test(ex) && !/,/.test(ex) ? { type: "datetime", datetime: { format: "iso8601" } } : { type: "text" };
      const frac = /,\d+/.test(ex) ? ",%f" : /\.\d+/.test(ex) ? "%.f" : "";
      return { type: "datetime", datetime: { format: `%Y-%m-%d${ex.includes("T") ? "T" : " "}%H:%M:%S${frac}`, timezone: "utc" } };
    }
    return { type: "text" };
  }
  function buildRule(name, m, example) {
    const g = gname(name), spec = typeSpec(m.typeId, example);
    const extract = m.mode === "grok" && m.grok != null
      ? { kind: "grok", patterns: [m.grok], anchor: "search", dot_all: true, source: "raw" }
      : { kind: "regex", pattern: m.regex, engine: "pcre2", case_sensitive: true, source: "raw" };
    return { id: `col-${name}`, description: name, selector: {}, priority: 0, fields: { [name]: { from: [g], type: spec.type, ...(spec.datetime ? { datetime: spec.datetime } : {}) } }, extract };
  }
  const exampleOf = (name, id = `col-${name}`) => { for (let i = 0; i < C.recs.length; i++) { const v = cellOf(id, name, i)?.original; if (v != null && v !== "") return v; } return ""; };
  /** Padrões Grok próprios gerados por colunas: entram e saem da definição junto com elas. */
  function syncGrok() {
    const d = def(), ui = (d.ui ||= {}), owned = new Set(ui.columnPatterns || []), needed = {};
    for (const r of columns()) Object.assign(needed, metaAll()[r.description].custom || {});
    for (const n of owned) if (!(n in needed)) delete d.grokPatterns?.[n];
    if (Object.keys(needed).length) { d.grokPatterns = { ...(d.grokPatterns || {}), ...needed }; ui.columnPatterns = Object.keys(needed); }
    else { delete ui.columnPatterns; if (d.grokPatterns && !Object.keys(d.grokPatterns).length) delete d.grokPatterns; }
  }
  function writeColumn(name, m, replaceName, example) {
    const d = def();
    d.rules = d.rules.filter((r) => !(r.id === "regra-1" && !Object.keys(r.fields || {}).length && !(r.extract?.patterns || []).some(Boolean)));
    const old = replaceName ?? name, at = d.rules.findIndex((r) => r.id === `col-${old}`);
    const rule = buildRule(name, m, example || exampleOf(old));
    if (replaceName && replaceName !== name) delete metaAll()[replaceName];
    metaAll()[name] = m;
    if (at >= 0) d.rules[at] = rule; else d.rules.push(rule);
    syncGrok();
    core.commit();
  }
  function removeColumn(name) {
    const d = def();
    d.rules = d.rules.filter((r) => r.id !== `col-${name}`);
    delete metaAll()[name];
    if (!Object.keys(d.ui.columns).length) delete d.ui.columns;
    if (!Object.keys(d.ui).length) delete d.ui;
    syncGrok();
    core.commit();
  }
  /** O nome do grupo mudou: as expressões guardadas acompanham. */
  function renameExpr(m, from, to) {
    const fix = (s, re, to2) => (s == null ? s : s.replace(re, to2));
    return { ...m, grok: fix(m.grok, new RegExp(`:${from}\\}`, "g"), `:${to}}`), regex: fix(m.regex, new RegExp(`\\(\\?<${from}>`, "g"), `(?<${to}>`) };
  }

  // ------------------------------------------------------------------ amostra e resultados nativos
  async function load() {
    const g = gen, mine = ++C.sampleSerial;
    C.loading = true; C.error = ""; paint();
    try {
      let r = await core.call({ action: "map_sample", count: 60, field: "raw", seed: 1 }, { latest: "map-columns-sample" });
      if (!alive(g) || mine !== C.sampleSerial) return;
      const files = (r.distribution || []).filter((f) => f.records > 0).map((f) => ({ path: f.file, name: f.fileName, records: f.records }));
      if (files.length > 1) {
        C.files = files; C.fi = Math.min(C.fi, files.length - 1);
        r = await core.call({ action: "map_sample", count: 60, field: "raw", seed: 1, sources: [files[C.fi].path] }, { latest: "map-columns-sample" });
        if (!alive(g) || mine !== C.sampleSerial) return;
      } else { C.files = files; C.fi = 0; }
      C.recs = (r.samples || []).map((s) => ({ text: s.text, path: s.file, file: s.fileName, id: s.id, eventRef: s.eventRef, detected: s.detected || {}, truncated: s.truncated }));
      C.population = C.files.length > 1 ? C.files[C.fi].records : r.population;
      C.idx = Math.min(C.idx, Math.max(0, C.recs.length - 1));
      C.draft = null; C.res = null;
    } catch (e) {
      if (!alive(g)) return;
      if (!window.LocalNative.superseded(e)) C.error = String(e.message || e);
    }
    C.loading = false;
    paint();
    void evaluate(); void loadFraming();
  }
  async function loadFraming() {
    const g = gen, path = C.recs[0]?.path;
    C.framing = null; paintRecordRow();
    if (!path) return;
    try {
      const cfg = await api("get_ts_config", { path }, { silent: true, analysisOwner: window.AnalysisContexts?.capture() });
      const r = await core.call({ action: "map_frame", path, pattern: cfg?.record_start || null, count: 3 }, { latest: "map-columns-frame" });
      if (!alive(g)) return;
      C.framing = { path, config: cfg || null, detected: r.detected || null, pattern: r.pattern || null, format: r.format };
    } catch (e) { if (alive(g) && !window.LocalNative.superseded(e)) C.framing = { path, error: String(e.message || e) }; }
    paintRecordRow();
  }
  /** O que o motor nativo diz de todas as colunas (e do rascunho) nas amostras reais. */
  function mapForEvaluation() {
    const d = structuredClone(def());
    if (C.draft) {
      const n = C.draft.name || "rascunho", m = draftMeta();
      if (m) {
        d.rules = d.rules.filter((r) => r.id !== `col-${n}`);
        d.rules.push({ ...buildRule(n, m, exampleOf(n)), id: "col-rascunho", description: n });
        d.grokPatterns = { ...(d.grokPatterns || {}), ...(m.custom || {}) };
      }
    }
    return d;
  }
  async function evaluate() {
    const g = gen, mine = ++C.serial;
    if (!C.recs.length) { paint(); return; }
    const d = mapForEvaluation();
    if (!d.rules.some((r) => r.extract && (r.extract.patterns || [r.extract.pattern]).some(Boolean))) { C.res = null; paintAfterEval(); return; }
    try {
      const r = await core.call({ action: "map_evaluate", map_json: JSON.stringify(d), samples: C.recs.map((x) => ({ id: x.id, event_ref: x.eventRef })) }, { latest: "map-columns-eval" });
      if (!alive(g) || mine !== C.serial) return;
      C.res = r.ok === false ? { error: r.error?.[0]?.message || r.error?.message || (r.diagnostics?.[0]?.message) || "Definição inválida." } : r;
    } catch (e) {
      if (!alive(g) || mine !== C.serial) return;
      if (!window.LocalNative.superseded(e)) C.res = { error: String(e.message || e) };
    }
    paintAfterEval();
  }
  const paintAfterEval = () => { paintRecord(); paintRows(); paintStatus(); };
  /** Resultado de uma regra no registro `i`: valor normalizado, estado e posição no texto. */
  function cellOf(ruleId, name, i) {
    const r = C.res?.results?.[i];
    if (!r) return null;
    const o = r.trace?.outputs?.find((x) => x.rule === ruleId && x.field === name);
    const t = r.trace?.rules?.find((x) => x.rule === ruleId);
    const range = t?.extracted?.[0]?.[2] || null;
    return { o, range, original: o?.original ?? t?.extracted?.[0]?.[1] ?? null, value: o && o.normalized != null ? (typeof o.normalized === "string" ? o.normalized : JSON.stringify(o.normalized)) : null, status: o?.status };
  }
  const valuesOfColumn = (name) => C.recs.map((_, i) => cellOf(`col-${name}`, name, i)?.value ?? null);
  const valuesOfDraft = () => {
    const d = C.draft;
    if (!d) return [];
    const c = d.typed == null ? d.cands[d.pick] : null;
    const native = C.recs.map((_, i) => cellOf("col-rascunho", d.name || "rascunho", i)?.value ?? null);
    // Até o motor responder, valem os valores medidos ao propor o padrão.
    return c && native.every((v) => v == null) ? c.values : native;
  };

  // ------------------------------------------------------------------ colunas já reconhecidas
  function detectedPairs(rec) {
    const out = [];
    for (const [k, v] of Object.entries(rec.detected?.standard || {})) out.push([k, String(v), "standard"]);
    // Campos que as extrações do próprio Caso acrescentam (norm.*, proveniência) não são divisões do registro.
    const own = new Set([core.S.name, core.S.installed, ...core.installedMaps().map((d) => d.name)].filter(Boolean));
    for (const [k, v] of Object.entries(rec.detected?.fields || {})) {
      if (k.includes("._provenance") || own.has(k.split(".")[0])) continue;
      out.push([k, String(v), "field"]);
    }
    return out;
  }
  /** Posição do valor no texto do registro, quando ele aparece por inteiro (não no meio de outra palavra). */
  function locate(text, v) {
    if (!v) return -1;
    let from = 0;
    for (;;) {
      const i = text.indexOf(v, from);
      if (i < 0) return -1;
      const before = text[i - 1], after = text[i + v.length];
      if (!(/^\w/.test(v) && before && /\w/.test(before)) && !(/\w$/.test(v) && after && /\w/.test(after))) return i;
      from = i + 1;
    }
  }
  function detectedCols() {
    const by = new Map();
    for (const rec of C.recs) for (const [name, value, kind] of detectedPairs(rec)) {
      const c = by.get(name) || { name, kind, values: [], located: false };
      c.values.push(value);
      if (locate(rec.text, value) >= 0) c.located = true;
      by.set(name, c);
    }
    return [...by.values()].filter((c) => c.located || c.name === "timestamp");
  }
  function inferType(values) {
    const vs = values.filter((v) => v != null && v !== "");
    if (!vs.length) return "texto";
    for (const id of E.SPECIFIC) {
      const t = E.T(id);
      if (t.re && !["inteiro", "arquivo", "usuario", "porta", "pid", "dominio"].includes(id) && vs.every((v) => new RegExp(`^(?:${t.re})$`).test(v))) return id;
    }
    if (vs.every((v) => /^-?\d+$/.test(v))) return "inteiro";
    if (vs.every((v) => /^-?\d+\.\d+$/.test(v))) return "decimal";
    return "texto";
  }
  const existingCtx = () => detectedPairs(C.recs[C.idx] || { detected: {} }).filter(([, v]) => v.length > 1).map(([name, value]) => ({ name, value }));

  // ------------------------------------------------------------------ rascunho (nova coluna)
  const ECS_NAMES = (c) => E.nameSuggestions(c);
  function startDraft(s, e) {
    const rec = C.recs[C.idx];
    if (!rec || e - s < 1) return;
    const cands = E.candidates(rec.text, s, e, { records: C.recs.map((r) => (r.text.length > 8000 ? r.text.slice(0, 8000) : r.text)), existing: existingCtx() });
    if (!cands.length) return say("Não foi possível propor padrões para este trecho");
    const sg = ECS_NAMES(cands[0]);
    C.draft = { s, e, rec: C.idx, cands, pick: 0, name: uniqName(sg[0] || "campo"), typeId: cands[0].typeId, typed: null, nameTouched: false, typeTouched: false, all: false, native: "pending", editing: null };
    paint(); void verifyNative();
  }
  /** Os padrões nascem aqui (JavaScript, rápido); só ficam os que o motor nativo confirma. */
  async function verifyNative() {
    const d = C.draft, g = gen;
    if (!d) return;
    try {
      const r = await core.call({ action: "map_verify", texts: C.recs.map((x) => x.text), patterns: d.cands.map((c) => c.re) }, { latest: "map-columns-verify" });
      if (!alive(g) || C.draft !== d) return;
      const kept = [];
      d.cands.forEach((c, i) => {
        const res = r.results?.[i];
        if (!res || res.error) return;
        const at = res.ranges[d.rec];
        if (!at || !(c.part ? at[0] >= d.s && at[1] <= d.e : at[0] === d.s && at[1] === d.e)) return;
        c.values = res.ranges.map((x, k) => (x ? C.recs[k].text.slice(x[0], x[1]) : null));
        c.cover = c.values.filter((v) => v != null).length;
        c.confirmed = true;
        kept.push(c);
      });
      if (kept.length) { d.cands = kept; d.pick = 0; d.native = "ok"; if (!d.typeTouched) d.typeId = kept[0].typeId; if (!d.nameTouched) d.name = uniqName(ECS_NAMES(kept[0])[0] || "campo"); }
      else d.native = "unconfirmed";
    } catch (e) { if (alive(g) && C.draft === d && !window.LocalNative.superseded(e)) d.native = "unconfirmed"; }
    if (alive(g) && C.draft === d) { paint(); void evaluate(); }
  }
  const effMode = () => { const d = C.draft; const c = d && d.typed == null ? d.cands[d.pick] : null; return c && c.forms(gname(d.name || "campo")).grok == null ? "regex" : C.mode; };
  const exprOf = (c, name, mode) => { const f = c.forms(gname(name)); return mode === "grok" && f.grok != null ? f.grok : f.regex; };
  /** O que será gravado para o rascunho atual. */
  function draftMeta() {
    const d = C.draft;
    if (!d) return null;
    if (d.typed != null) {
      return C.mode === "grok" ? { typeId: d.typeId, mode: "grok", grok: d.typed, regex: null, custom: {} } : { typeId: d.typeId, mode: "regex", grok: null, regex: d.typed, custom: {} };
    }
    const c = d.cands[d.pick], f = c.forms(gname(d.name || "campo"));
    return { typeId: d.typeId, mode: f.grok != null && C.mode === "grok" ? "grok" : "regex", grok: f.grok, regex: f.regex, custom: f.custom ? { [f.custom.name]: f.custom.src } : {} };
  }
  function pick(i) {
    const d = C.draft;
    d.pick = i; d.typed = null;
    if (!d.typeTouched) d.typeId = d.cands[i].typeId;
    if (!d.nameTouched) d.name = uniqName(ECS_NAMES(d.cands[i])[0] || "campo");
    paint(); later("eval", () => void evaluate(), 80);
  }
  function commitDraft() {
    const d = C.draft;
    if (!d) return;
    const name = (d.name || "").trim();
    if (!name) return say("Dê um nome à coluna");
    if (takenNames(d.editing).includes(name)) return say("Já existe uma coluna com esse nome");
    const m = draftMeta();
    if (d.typed != null && C.res?.error) return say("Corrija a expressão antes de adicionar");
    // A forma Grok só é gravada se o motor devolve a mesma posição que a verificação confirmou.
    if (d.typed == null && m.mode === "grok") {
      const got = cellOf("col-rascunho", name, d.rec)?.range;
      if (got && (got[0] !== d.s || got[1] !== d.e) && !d.cands[d.pick].part) { m.mode = "regex"; say("Gravei como Regex para manter o resultado exato"); }
    }
    writeColumn(name, m, d.editing, exampleOf(d.name || "rascunho", "col-rascunho"));
    C.draft = null;
    paint(); void evaluate();
    say(d.editing ? `Coluna “${name}” atualizada` : `Coluna “${name}” criada`);
  }

  // ------------------------------------------------------------------ pintura
  function say(message) { const t = q("#cx-toast"); if (!t) return; t.textContent = message; t.hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => { t.hidden = true; toastTimer = null; }, 2400); }
  function paint() {
    if (!root) return;
    paintTop(); paintRecord(); paintRecordRow(); paintNew(); paintRows(); paintStatus();
    const err = q("#cx-error");
    err.hidden = !C.error; err.textContent = C.error ? `Não foi possível ler amostras: ${C.error}` : "";
  }
  function paintTop() {
    const s = core.S, dirty = s.base ? JSON.stringify(s.def) !== s.base : colNames().length > 0;
    q("#cx-undo").disabled = !s.hist.canUndo; q("#cx-redo").disabled = !s.hist.canRedo;
    q("#cx-dirty").hidden = !dirty;
    const others = (s.def?.rules || []).filter((r) => !columns().includes(r) && r.extract && Object.keys(r.fields || {}).length).length, pill = q("#cx-adv-count");
    pill.hidden = !others; pill.textContent = others ? `+${others}` : ""; pill.title = others ? `${others} regra(s) criadas fora desta tela: só se editam no modo avançado` : "";
    q("#cx-apply").disabled = !colNames().length && !s.base;
    const tabs = q("#cx-tabs");
    tabs.hidden = C.files.length < 2;
    tabs.innerHTML = C.files.map((f, i) => `<button class="cx-tab" role="tab" data-fi="${i}" aria-selected="${i === C.fi}" title="${h(f.path)}">${h(f.name)}<small>${num(f.records)} registros</small></button>`).join("");
    tabs.querySelectorAll("[data-fi]").forEach((b) => { b.onclick = () => { C.fi = +b.dataset.fi; C.idx = 0; void load(); }; });
    q("#cx-src").textContent = C.files.length < 2 ? (C.recs[0] ? `${C.recs[0].file}${C.population ? ` · ${num(C.population)} registros` : ""}` : (s.sources.length ? "" : "sem arquivos abertos")) : "";
  }
  function marksOf(i) {
    const rec = C.recs[i];
    if (!rec) return [];
    const out = [], text = rec.text;
    for (const [n, v] of detectedPairs(rec)) { const at = locate(text, v); if (at >= 0 && v.length > 0) out.push({ s: at, e: at + v.length, cls: "exists", name: n, c: "var(--cx-muted)" }); }
    for (const r of columns()) { const range = cellOf(r.id, r.description, i)?.range; if (range) out.push({ s: range[0], e: range[1], cls: "col", name: r.description, c: color(r.description) }); }
    const d = C.draft;
    if (d && d.rec === i) {
      let range = null;
      if (d.typed == null) { const c = d.cands[d.pick]; try { const m = new RegExp(c.re, "d").exec(text); range = m && m.indices[1] ? m.indices[1] : null; } catch { range = null; } }
      else range = cellOf("col-rascunho", d.name || "rascunho", i)?.range || null;
      if (range) out.push({ s: range[0], e: range[1], cls: "draft", name: d.name });
    } else if (d) {
      const range = cellOf("col-rascunho", d.name || "rascunho", i)?.range;
      if (range) out.push({ s: range[0], e: range[1], cls: "draft", name: d.name });
    }
    const taken = out.slice();
    for (const t of ["ipv4", "iso", "classe", "excecao", "duracao", "uuid", "email", "url", "status", "semver", "bytes", "hex"]) {
      const re = new RegExp(E.T(t).re, "g");
      for (const m of text.matchAll(re)) {
        const s = m.index, e = s + m[0].length;
        if (!taken.some((x) => x.s < e && s < x.e) && !(t === "status" && !/" \d{3}$/.test(text.slice(0, e)))) { out.push({ s, e, cls: "cand", name: t, c: "var(--cx-line)" }); taken.push({ s, e }); }
      }
    }
    return out.sort((a, b) => a.s - b.s || (a.cls === "draft" ? -1 : 1)).filter((m, k, a) => !a.slice(0, k).some((x) => x.e > m.s));
  }
  function paintRecord() {
    const el = q("#cx-record"), rec = C.recs[C.idx];
    if (!el) return;
    if (!rec) { el.textContent = C.loading ? "" : "Abra um arquivo para ver os registros."; q("#cx-pos").textContent = ""; q("#cx-prev").disabled = q("#cx-next").disabled = true; return; }
    const ms = marksOf(C.idx);
    let html = "", cur = 0;
    ms.forEach((m, k) => { html += h(rec.text.slice(cur, m.s)) + `<span class="cx-m ${m.cls}" data-i="${k}" data-name="${h(m.cls === "cand" ? "" : m.name)}" style="--c:${m.c}">${h(rec.text.slice(m.s, m.e))}</span>`; cur = m.e; });
    html += h(rec.text.slice(cur));
    el.innerHTML = html; el._marks = ms;
    q("#cx-pos").textContent = `${C.idx + 1}/${C.recs.length}`; q("#cx-prev").disabled = C.idx === 0; q("#cx-next").disabled = C.idx >= C.recs.length - 1;
    el.querySelectorAll(".cx-m.cand").forEach((sp) => { sp.onclick = (ev) => { ev.stopPropagation(); const m = ms[+sp.dataset.i]; startDraft(m.s, m.e); }; });
  }
  function paintRecordRow() {
    const code = q("#cx-start"), f = C.framing;
    if (!code) return;
    code.textContent = !f ? "…" : f.error ? "(indisponível)" : f.pattern || f.detected || "(uma linha por registro)";
    code.title = f?.pattern ? "Definido por você para este arquivo" : f?.detected ? "Detectado automaticamente" : "";
    code.dataset.custom = String(!!f?.pattern);
  }
  const covClass = (c) => (c.cover === c.total ? "" : c.cover === 0 ? "low" : "mid");
  function paintNew() {
    const d = C.draft, box = q("#cx-new");
    box.className = "cx-card" + (d ? " draft" : "");
    if (!d) { box.innerHTML = `<div class="cx-head"><div class="cx-sec">Nova coluna <button class="cx-i" data-info="nova" type="button" aria-label="Mais informações">${svg("info")}</button></div></div><div class="cx-empty">${svg("col")}<span>Selecione um trecho da amostra ou clique num pedaço sublinhado</span></div>`; return; }
    const sg = ECS_NAMES(d.cands[d.pick]), ty = E.T(d.typeId), dup = takenNames(d.editing).includes(d.name), mode = effMode();
    const shown = d.cands.map((p, i) => ({ p, i })).filter(({ i }) => i < (d.all ? 40 : 8) || i === d.pick);
    box.innerHTML = `<div class="cx-head"><div class="cx-sec">${d.editing ? "Editar coluna" : "Nova coluna"} <button class="cx-i" data-info="nova" type="button" aria-label="Mais informações">${svg("info")}</button></div>
      <button class="cx-ib" id="cx-d-cancel" type="button" title="Cancelar" aria-label="Cancelar">${svg("x")}</button><button class="cx-btn primary small" id="cx-d-ok" type="button">${svg("check")}${d.editing ? "Salvar" : "Adicionar"}</button></div>
    <div class="cx-namerow"><input class="cx-name ${dup ? "bad" : ""}" id="cx-nameinput" value="${h(d.name)}" spellcheck="false" aria-label="Nome da coluna" placeholder="nome da coluna"><button class="cx-typebtn" id="cx-typebtn" type="button" aria-label="Escolher o tipo">${svg("type")}${h(ty.l)}<small>${h(ty.b)}</small></button></div>
    <div class="cx-sugg"><span>Nomes:</span>${sg.map((n) => `<button type="button" data-n="${h(n)}">${h(n)}</button>`).join("")}</div>
    <div class="cx-head"><div class="cx-sec">Padrões possíveis <span class="cx-sub">· ${d.cands.length} verificados (de ${d.cands.total || d.cands.length} testados)${d.native === "pending" ? " · confirmando…" : d.native === "unconfirmed" ? " · não confirmados pelo motor" : ""}</span> <button class="cx-i" data-info="padroes" type="button" aria-label="Mais informações">${svg("info")}</button></div></div>
    <div class="cx-opts" role="radiogroup" aria-label="Padrões possíveis">${shown.map(({ p, i }) => `<button class="cx-opt" role="radio" aria-checked="${d.typed == null && i === d.pick}" data-p="${i}"><span class="cx-rd"></span><span><b>${h(p.label)}</b>${p.rec ? '<span class="cx-rec">recomendado</span>' : ""}<span class="cx-why">${h(p.why || "")}</span><span class="cx-badges">${p.once && p.cover === p.total ? '<span class="cx-bd ok">estável</span>' : p.once ? "" : '<span class="cx-bd warn">acha mais de um em alguma amostra</span>'}${p.homog ? '<span class="cx-bd">formato parecido nas amostras</span>' : ""}${p.alts ? `<span class="cx-bd">+${p.alts} equivalentes</span>` : ""}</span><code>${h(exprOf(p, d.name || "campo", C.mode))}</code><span class="cx-exv">${h(p.values.filter((v) => v != null).slice(0, 3).join("  ·  "))}</span></span><span class="cx-cov ${covClass(p)}">${p.cover}/${p.total}</span></button>`).join("")}</div>
    ${d.cands.length > 8 ? `<button class="cx-btn small" id="cx-more" type="button" style="margin-bottom:10px">${d.all ? "Mostrar só as melhores" : `Ver todas as ${d.cands.length} possibilidades verificadas`}</button>` : ""}
    <div class="cx-head"><div class="cx-sec">Expressão</div><div class="cx-seg" role="group" aria-label="Tipo de expressão"><button id="cx-m-grok" aria-pressed="${mode === "grok"}" ${effMode() === "regex" && C.mode === "grok" && d.typed == null ? 'title="Este padrão só existe em Regex"' : ""}>Grok</button><button id="cx-m-regex" aria-pressed="${mode === "regex"}">Regex</button></div></div>
    <textarea class="cx-expr" id="cx-expr" spellcheck="false" aria-label="Expressão">${h(d.typed != null ? d.typed : exprOf(d.cands[d.pick], d.name || "campo", mode))}</textarea>
    <div class="cx-status" id="cx-status"></div>`;
    box.querySelector("#cx-d-ok").onclick = commitDraft; box.querySelector("#cx-d-cancel").onclick = () => { C.draft = null; paint(); void evaluate(); };
    const more = box.querySelector("#cx-more"); if (more) more.onclick = () => { d.all = !d.all; paintNew(); };
    const nameInput = box.querySelector("#cx-nameinput");
    nameInput.oninput = (e) => {
      const old = gname(d.name || "campo");
      d.name = e.target.value.trim().replace(/\s+/g, "_"); d.nameTouched = true;
      if (d.typed != null) d.typed = d.typed.replace(new RegExp(`:${old}\\}`, "g"), `:${gname(d.name)}}`).replace(new RegExp(`\\(\\?<${old}>`, "g"), `(?<${gname(d.name)}>`);
      nameInput.classList.toggle("bad", takenNames(d.editing).includes(d.name));
      const x = box.querySelector("#cx-expr"); if (d.typed == null) x.value = exprOf(d.cands[d.pick], d.name || "campo", effMode()); else x.value = d.typed;
      box.querySelectorAll(".cx-opt code").forEach((cd, k) => { const idx = +cd.closest(".cx-opt").dataset.p; cd.textContent = exprOf(d.cands[idx], d.name || "campo", C.mode); });
      paintRows(); later("eval", () => void evaluate(), 250);
    };
    nameInput.onkeydown = (e) => { if (e.key === "Enter") commitDraft(); };
    box.querySelectorAll("[data-n]").forEach((b) => { b.onclick = () => { d.name = uniqName(b.dataset.n, d.editing); d.nameTouched = true; paintNew(); paintRows(); later("eval", () => void evaluate(), 100); }; });
    box.querySelectorAll("[data-p]").forEach((b) => { b.onclick = () => pick(+b.dataset.p); });
    box.querySelector("#cx-typebtn").onclick = () => typePicker(d.typeId, (id) => { d.typeId = id; d.typeTouched = true; paintNew(); paintRows(); later("eval", () => void evaluate(), 80); });
    box.querySelector("#cx-m-grok").onclick = () => { C.mode = "grok"; if (d.typed != null) d.typed = null; paintNew(); later("eval", () => void evaluate(), 80); };
    box.querySelector("#cx-m-regex").onclick = () => { C.mode = "regex"; if (d.typed != null) d.typed = null; paintNew(); later("eval", () => void evaluate(), 80); };
    box.querySelector("#cx-expr").oninput = (e) => {
      d.typed = e.target.value;
      const named = C.mode === "grok" ? /%\{\w+:([\w]+)\}/.exec(d.typed) : /\(\?<(\w+)>/.exec(d.typed);
      if (named && gname(d.name) !== named[1]) { d.name = named[1]; d.nameTouched = true; nameInput.value = d.name; }
      box.querySelectorAll(".cx-opt").forEach((o) => o.setAttribute("aria-checked", "false"));
      box.querySelector("#cx-status").className = "cx-status"; box.querySelector("#cx-status").textContent = "…";
      later("eval", () => void evaluate(), 300);
    };
    paintStatus();
  }
  function paintStatus() {
    const st = q("#cx-status"), d = C.draft;
    if (!st || !d) return;
    if (C.res?.error) { st.className = "cx-status bad"; st.textContent = C.res.error; return; }
    const v = valuesOfDraft(), ok = v.filter((x) => x != null && x !== "").length;
    st.className = "cx-status";
    st.innerHTML = `${svg("check")}<span>Encaixa em <b>${ok} de ${v.length}</b> amostras</span>`;
  }
  function paintRows() {
    const body = q("#cx-rows"), d = C.draft;
    if (!body) return;
    const rows = [], join = (vs) => h(vs.filter((x) => x != null && x !== "").slice(0, 3).join("  ·  "));
    if (d) rows.push(`<tr class="draftrow"><td><span class="dot" style="--c:var(--cx-accent)"></span>${h(d.name || "(sem nome)")}<span class="tag new">${d.editing ? "editando" : "novo"}</span></td><td>${h(E.T(d.typeId).l)}</td><td class="cx-ex">${join(valuesOfDraft())}</td></tr>`);
    for (const r of columns()) {
      if (d?.editing === r.description) continue;
      const m = metaAll()[r.description], vs = valuesOfColumn(r.description), bad = C.res?.results?.some((x, i) => cellOf(r.id, r.description, i)?.status === "invalid");
      rows.push(`<tr data-c="${h(r.description)}"><td class="nm" data-nm="${h(r.description)}"><span class="dot" style="--c:${color(r.description)}"></span>${h(r.description)}</td><td class="ty" data-ty="${h(r.description)}">${h(E.T(m.typeId).l)}${bad ? '<span class="tag warn" title="Alguns valores não cabem neste tipo">valores inválidos</span>' : ""}</td><td class="cx-ex">${join(vs)}</td></tr>`);
    }
    for (const c of detectedCols()) rows.push(`<tr data-ex="1"><td><span class="dot" style="--c:var(--cx-muted)"></span>${h(c.name)}<span class="tag">já existe</span></td><td>${h(E.T(inferType(c.values)).l)}</td><td class="cx-ex">${join(c.values)}</td></tr>`);
    body.innerHTML = rows.join("") || `<tr><td colspan="3" class="cx-empty-row">Nenhuma coluna ainda.</td></tr>`;
    body.querySelectorAll("[data-c]").forEach((tr) => { tr.oncontextmenu = (e) => { e.preventDefault(); rowMenu(tr.dataset.c, e.clientX, e.clientY); }; });
    body.querySelectorAll("[data-nm]").forEach((td) => { td.ondblclick = () => renameInline(td.dataset.nm); });
    body.querySelectorAll("[data-ty]").forEach((td) => { td.onclick = () => { const n = td.dataset.ty; typePicker(metaAll()[n].typeId, (id) => writeColumn(n, { ...metaAll()[n], typeId: id })); }; });
  }
  function renameInline(name) {
    const td = q(`[data-nm="${CSS.escape(name)}"]`);
    if (!td) return;
    td.innerHTML = `<input value="${h(name)}" aria-label="Novo nome">`;
    const inp = td.querySelector("input");
    inp.focus(); inp.select();
    let finished = false;
    const done = (ok) => {
      if (finished) return; finished = true;
      const n = E.clean(inp.value) || name;
      if (ok && n !== name) {
        if (takenNames(name).includes(n)) say("Já existe uma coluna com esse nome");
        else { const m = renameExpr(metaAll()[name], gname(name), gname(n)); writeColumn(n, m, name); }
      }
      paint(); void evaluate();
    };
    inp.onkeydown = (e) => { if (e.key === "Enter") done(true); if (e.key === "Escape") done(false); };
    inp.onblur = () => done(true);
  }

  // ------------------------------------------------------------------ menus, seleção e diálogos
  const menu = () => q("#cx-menu");
  function showMenu(items, x, y, title) {
    const m = menu();
    m.innerHTML = (title ? `<div class="t">${h(title)}</div>` : "") + items.map((it, i) => (it === "-" ? "<hr>" : `<button type="button" data-i="${i}" class="${it.first ? "first" : ""}">${svg(it.icon || "col")}${h(it.label)}</button>`)).join("");
    m.hidden = false;
    m.style.left = `${Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8))}px`; m.style.top = `${Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8))}px`;
    m.querySelectorAll("button").forEach((b) => { b.onclick = () => { const it = items[+b.dataset.i]; m.hidden = true; it.run(); }; });
    m.querySelector("button")?.focus();
  }
  function selection() {
    const sel = getSelection(), el = q("#cx-record");
    if (!sel || sel.isCollapsed || !el || !el.contains(sel.anchorNode) || !el.contains(sel.focusNode)) return null;
    const at = (n, o) => { const r = document.createRange(); r.selectNodeContents(el); r.setEnd(n, o); return r.toString().length; };
    let s = at(sel.anchorNode, sel.anchorOffset), e = at(sel.focusNode, sel.focusOffset);
    if (s > e) [s, e] = [e, s];
    return s === e ? null : { s, e };
  }
  function rowMenu(name, x, y) {
    const m = metaAll()[name];
    if (!m) return;
    showMenu([
      { label: "Renomear", icon: "edit", first: true, run: () => renameInline(name) },
      { label: "Tipo…", icon: "type", run: () => typePicker(m.typeId, (id) => writeColumn(name, { ...m, typeId: id })) },
      { label: "Editar o padrão", icon: "code", run: () => editColumn(name) },
      { label: "Copiar expressão", icon: "copy", run: () => { navigator.clipboard?.writeText(m.mode === "grok" && m.grok != null ? m.grok : m.regex); say("Copiado"); } }, "-",
      { label: "Remover coluna", icon: "trash", run: () => removeColumn(name) },
    ], x, y, name);
  }
  /** Reabre uma coluna criada como rascunho editável, com a expressão como está gravada. */
  function editColumn(name) {
    const m = metaAll()[name];
    C.mode = m.mode === "grok" && m.grok != null ? "grok" : "regex";
    C.draft = { s: 0, e: 0, rec: C.idx, cands: [{ label: "Padrão atual", why: "O padrão gravado", re: "", values: valuesOfColumn(name), cover: valuesOfColumn(name).filter((v) => v != null).length, total: C.recs.length, once: true, homog: true, typeId: m.typeId, names: [name], forms: () => ({ grok: m.grok, regex: m.regex, custom: null }) }], pick: 0, name, typeId: m.typeId, typed: C.mode === "grok" ? m.grok : m.regex, nameTouched: true, typeTouched: true, all: false, native: "ok", editing: name };
    C.draft.cands.total = 1;
    paint(); void evaluate();
  }
  function typePicker(current, done) {
    const d = q("#cx-dlg"), groups = [...new Set(E.TYPES.map((t) => t.g))];
    const draw = (text) => {
      d.querySelector("#cx-tp").innerHTML = groups.map((g) => { const list = E.TYPES.filter((t) => t.g === g && (!text || (t.l + t.id + g).toLowerCase().includes(text))); return list.length ? `<h3>${h(g)}</h3>` + list.map((t) => `<button type="button" data-t="${t.id}" ${t.id === current ? 'aria-current="true"' : ""}><span>${h(t.l)}</span><small>${h(t.ex)}</small></button>`).join("") : ""; }).join("") || "<p>Nenhum tipo encontrado.</p>";
      d.querySelectorAll("[data-t]").forEach((b) => { b.onclick = () => { d.close(); done(b.dataset.t); }; });
    };
    d.innerHTML = `<h2>Tipo da coluna</h2><input class="q" id="cx-tq" placeholder="Buscar tipo…" aria-label="Buscar tipo" autocomplete="off"><div class="tp" id="cx-tp"></div><div class="row"><button class="cx-btn" type="button" id="cx-close">Fechar</button></div>`;
    d.onclose = null; d.showModal(); draw("");
    d.querySelector("#cx-close").onclick = () => d.close();
    d.querySelector("#cx-tq").oninput = (e) => draw(e.target.value.toLowerCase()); d.querySelector("#cx-tq").focus();
  }
  function showInfo(topic) {
    const [title, body] = INFO[topic], d = q("#cx-dlg");
    d.innerHTML = `<form method="dialog"><h2>${h(title)}</h2>${body}<div class="row"><button class="cx-btn">Fechar</button></div></form>`;
    d.onclose = null; d.showModal();
  }
  /** O que é um registro: expressão que marca o começo, com prévia do enquadramento antes de salvar. */
  function recordStartDialog() {
    const f = C.framing, d = q("#cx-dlg");
    if (!f || f.error || !f.path) return say("O arquivo ainda não foi lido");
    d.innerHTML = `<form method="dialog" id="cx-rs"><h2>Cada registro começa com</h2><p>Expressão (Rust regex) aplicada a uma linha do arquivo. Linhas que não a casam continuam o registro anterior. Vazio = automático${f.detected ? ` (<code>${h(f.detected)}</code>)` : " (uma linha por registro)"}.</p>
      <input class="q" name="v" value="${h(f.pattern || "")}" autocomplete="off" spellcheck="false" aria-label="Início do registro" placeholder="ex.: ^\\d{4}-\\d\\d-\\d\\d"><div id="cx-rs-prev" class="cx-rs-prev"></div>
      <div class="row"><button type="button" class="cx-btn" id="cx-rs-try">Ver prévia</button><span class="cx-grow"></span><button class="cx-btn" value="no">Cancelar</button><button class="cx-btn primary" value="ok">Salvar e reler</button></div></form>`;
    const input = d.querySelector("input"), prev = d.querySelector("#cx-rs-prev");
    const preview = async () => {
      prev.textContent = "…";
      try {
        const r = await core.call({ action: "map_frame", path: f.path, pattern: input.value.trim() || null, count: 40 }, { latest: "map-columns-frame-preview" });
        const lines = r.records.map((x) => x.lines);
        prev.innerHTML = `<p><b>${num(r.records.length)}${r.more ? "+" : ""}</b> registros nas primeiras linhas · ${num(lines.filter((n) => n > 1).length)} com várias linhas</p><ol>${r.records.slice(0, 4).map((x) => `<li><code>${h(x.text.split("\n")[0].slice(0, 90))}</code>${x.lines > 1 ? ` <small>+${x.lines - 1} linha(s)</small>` : ""}</li>`).join("")}</ol>`;
      } catch (e) { prev.textContent = String(e.message || e); }
    };
    d.querySelector("#cx-rs-try").onclick = preview; input.onkeydown = (e) => { if (e.key === "Enter") { e.preventDefault(); void preview(); } };
    d.onclose = async () => {
      if (d.returnValue !== "ok") return;
      const value = input.value.trim() || null;
      if (value === (f.pattern || null)) return;
      await saveRecordStart(f, value);
    };
    d.showModal(); input.focus();
  }
  async function saveRecordStart(f, value) {
    try {
      const config = { ...(f.config || {}), record_start: value };
      if (!value) delete config.record_start;
      await commitTsConfig([f.path], Object.keys(config).length ? config : null);
      say("Relendo o arquivo com o novo início de registro…");
      const source = state.currentArtifact?.source;
      if (source) await loadData(source);
      void load();
    } catch (e) { say(String(e.message || e)); }
  }
  async function applyDialog() {
    const s = core.S, d = q("#cx-dlg");
    if (!s.valid) { say(s.error || "A definição tem problemas"); return; }
    if (s.defsReady === false) { say("As definições do Caso ainda estão carregando"); return; }
    const cols = colNames();
    if (!cols.length) { say("Crie ao menos uma coluna"); return; }
    const prefix = s.name || "norm";
    d.innerHTML = `<form method="dialog"><h2>Aplicar ao Caso</h2><label class="cx-lbl">Nome do campo<input class="q" id="cx-prefix" value="${h(prefix)}" autocomplete="off"></label><p>${cols.map((n) => `<code>${h(prefix)}.${h(n)}</code>`).join(" ")}</p><div id="cx-scan"></div><div class="row"><button type="button" class="cx-btn" id="cx-do-scan">Conferir no conjunto</button><span class="cx-grow"></span><button class="cx-btn" value="no">Voltar</button><button class="cx-btn primary" value="ok">Aplicar</button></div></form>`;
    d.querySelector("#cx-do-scan").onclick = async (ev) => {
      const b = ev.currentTarget; b.disabled = true; d.querySelector("#cx-scan").innerHTML = "<p>…</p>";
      try {
        await core.scan();
        const r = core.S.scan;
        d.querySelector("#cx-scan").innerHTML = r?.error ? `<p>${h(r.error)}</p>` : r?.inspected != null ? `<p><b>${num(r.affected)}</b> de ${num(r.inspected)} registros ganham campos.</p>` : "";
      } catch (e) { d.querySelector("#cx-scan").innerHTML = `<p>${h(String(e.message || e))}</p>`; }
      b.disabled = false;
    };
    d.onclose = async () => {
      if (d.returnValue !== "ok") return;
      core.S.name = (d.querySelector("#cx-prefix")?.value || "").trim() || "norm"; core.paintHeader();
      try { const saved = await core.save(); paint(); say(saved ? "Aplicado ao Caso" : (core.S.notice || "Não foi possível aplicar")); } catch (e) { say(String(e.message || e)); }
    };
    d.showModal();
  }

  // ------------------------------------------------------------------ montagem
  function shell() {
    root.innerHTML = `<div class="cx">
    <header class="cx-top"><h1>Colunas</h1><div class="cx-tabs" id="cx-tabs" role="tablist" hidden></div><span class="cx-pill" id="cx-src"></span><span class="cx-dirty" id="cx-dirty" hidden title="Alterações não aplicadas"></span><span class="cx-grow"></span><span class="cx-pill" id="cx-adv-count" hidden></span>
      <button class="cx-ib" id="cx-undo" type="button" title="Desfazer" aria-label="Desfazer">${svg("undo")}</button><button class="cx-ib" id="cx-redo" type="button" title="Refazer" aria-label="Refazer">${svg("redo")}</button>
      <button class="cx-ib" data-info="geral" type="button" title="Como usar" aria-label="Como usar">${svg("info")}</button>
      <button class="cx-ib" id="cx-adv" type="button" title="Avançado" aria-label="Modo avançado">${svg("code")}</button>
      <button class="cx-btn primary" id="cx-apply" type="button">${svg("check")}Aplicar</button></header>
    <main class="cx-main">
      <div class="cx-card"><div class="cx-head"><div class="cx-sec">Amostra <button class="cx-i" data-info="amostra" type="button" aria-label="Mais informações">${svg("info")}</button></div>
        <button class="cx-ib" id="cx-prev" type="button" aria-label="Anterior">${svg("left")}</button><small id="cx-pos" class="cx-sub"></small><button class="cx-ib" id="cx-next" type="button" aria-label="Próxima">${svg("right")}</button></div>
        <div class="cx-record" id="cx-record" tabindex="0" aria-label="Registro de exemplo. Selecione um trecho para criar uma coluna."></div>
        <div class="cx-recrow"><span>Registro começa com</span><code id="cx-start">…</code><button class="cx-ib" id="cx-start-edit" type="button" title="Definir o que é um registro" aria-label="Definir o que é um registro">${svg("edit")}</button><button class="cx-i" data-info="registro" type="button" aria-label="Mais informações">${svg("info")}</button></div></div>
      <div class="cx-card" id="cx-new"></div>
      <p class="cx-error" id="cx-error" hidden></p>
    </main>
    <div class="cx-dock"><div class="cx-in"><div class="cx-head"><div class="cx-sec">Colunas <button class="cx-i" data-info="tabela" type="button" aria-label="Mais informações">${svg("info")}</button></div></div>
      <table><thead><tr><th>Coluna</th><th>Tipo</th><th>Exemplos</th></tr></thead><tbody id="cx-rows"></tbody></table></div></div>
    <button class="cx-seltip" id="cx-seltip" type="button" hidden>${svg("spark")}Criar coluna</button>
    <div class="cx-menu" id="cx-menu" hidden></div><div class="cx-toast" id="cx-toast" role="status" hidden></div><dialog class="cx-dlg" id="cx-dlg"></dialog></div>`;
    root.querySelectorAll("[data-info]").forEach((b) => { b.onclick = () => showInfo(b.dataset.info); });
    q("#cx-adv").onclick = () => core.setView("adv");
    q("#cx-undo").onclick = () => core.travel("undo"); q("#cx-redo").onclick = () => core.travel("redo");
    q("#cx-apply").onclick = applyDialog;
    q("#cx-prev").onclick = () => { C.idx = Math.max(0, C.idx - 1); C.draft = null; paint(); void evaluate(); };
    q("#cx-next").onclick = () => { C.idx = Math.min(C.recs.length - 1, C.idx + 1); C.draft = null; paint(); void evaluate(); };
    q("#cx-start-edit").onclick = recordStartDialog;
    const rec = q("#cx-record"), tip = q("#cx-seltip");
    const hideTip = () => { tip.hidden = true; };
    // Ao selecionar, aparece uma dica sobre o texto; só ao clicar nela os padrões são calculados.
    const showTip = () => setTimeout(() => {
      const sel = selection();
      if (!sel) return hideTip();
      const r = getSelection().getRangeAt(0).getBoundingClientRect();
      tip._sel = sel; tip.hidden = false;
      tip.style.left = `${Math.max(8, Math.min(r.right - 20, innerWidth - tip.offsetWidth - 8))}px`; tip.style.top = `${Math.max(8, r.top - tip.offsetHeight - 8)}px`;
    }, 0);
    rec.addEventListener("mouseup", showTip);
    rec.addEventListener("keyup", (e) => { if (e.shiftKey || e.key === "Shift") showTip(); });
    tip.addEventListener("mousedown", (e) => e.preventDefault());
    tip.onclick = () => { const sel = tip._sel; hideTip(); if (sel) startDraft(sel.s, sel.e); };
    rec.addEventListener("contextmenu", (ev) => {
      let sel = selection();
      if (!sel) { const sp = ev.target.closest(".cx-m"); if (sp?.firstChild) { const r = document.createRange(); r.selectNodeContents(sp.firstChild); getSelection().removeAllRanges(); getSelection().addRange(r); sel = selection(); } }
      if (!sel) return;
      ev.preventDefault();
      const picked = C.recs[C.idx].text.slice(sel.s, sel.e);
      showMenu([{ label: "Criar coluna com este trecho", icon: "col", first: true, run: () => startDraft(sel.s, sel.e) }, { label: "Copiar", icon: "copy", run: () => { navigator.clipboard?.writeText(picked); say("Copiado"); } }, "-", { label: "Desfazer", icon: "undo", run: () => core.travel("undo") }], ev.clientX, ev.clientY, `“${picked.slice(0, 28)}”`);
    });
    root.onclick = (e) => { if (!menu().contains(e.target)) menu().hidden = true; };
    document.addEventListener("selectionchange", onSelectionChange);
    root.onkeydown = (e) => {
      if (e.key === "Escape") menu().hidden = true;
      if ((e.ctrlKey || e.metaKey) && !/INPUT|TEXTAREA|SELECT/.test(document.activeElement?.tagName || "")) {
        if (e.key === "z") { e.preventDefault(); core.travel("undo"); } else if (e.key === "y") { e.preventDefault(); core.travel("redo"); }
      }
    };
  }
  function onSelectionChange() { const tip = q("#cx-seltip"); if (tip && !selection()) tip.hidden = true; }
  function mount(container, c) {
    core = c; root = container; gen++; C.recs = []; C.res = null; C.draft = null; C.framing = null; C.files = []; C.fi = 0; C.idx = 0;
    shell(); C.loading = true; paint();
    const mine = gen;
    Promise.resolve(core.S.ready).then(() => {
      if (mine !== gen || !root) return;
      if (core.S.fatal || !core.S.def) { fail(core.S.fatal || new Error("A definição não pôde ser aberta.")); return; }
      try { core.S.onChange = () => { if (root) { paint(); void evaluate(); } }; void load(); } catch (e) { fail(e); }
    });
  }
  /** Whatever goes wrong while opening is said on the page, never left as an empty one. */
  function fail(error) {
    console.error("extraction", error);
    C.loading = false;
    const box = q("#cx-error");
    if (box) { box.hidden = false; box.textContent = `Não foi possível abrir: ${String(error?.message || error)}`; }
  }
  const reload = () => { if (root && core.S.def && core.S.onChange) void load(); };
  const refresh = () => { if (root && core.S.def && core.S.onChange) { paint(); void evaluate(); } };
  function unmount() { gen++; document.removeEventListener("selectionchange", onSelectionChange); clearTimeout(toastTimer); Object.values(timers).forEach(clearTimeout); if (core) core.S.onChange = null; root = null; }
  return { mount, unmount, refresh, reload, state: C };
})();
