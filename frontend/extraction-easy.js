/* Extração e normalização, modo simples: a linha de exemplo é a interface. Etiquetas e seleção
 * viram colunas; o que aparece na tabela e na cobertura vem sempre do motor nativo. A definição
 * continua sendo uma só (a do modo avançado): este modo apenas a escreve e a lê. */
window.ExtractionEasy = (() => {
  "use strict";
  const M = window.ExtractionModel;
  const h = (v) => esc(String(v ?? ""));
  const num = (n) => Number(n || 0).toLocaleString("pt-BR");
  let core = null, root = null, gen = 0, timers = {};
  const E = { groups: [], gid: null, idx: {}, results: {}, chips: {}, error: "", loading: false, origin: null, focus: -1, pasted: [], serial: {}, lineKind: "message" };
  const S = () => core.S;
  const ICON = {
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>', check: '<path d="M5 12l5 5 9-10"/>', x: '<path d="M6 6l12 12M18 6L6 18"/>',
    moon: '<path d="M20 14a8 8 0 1 1-9-10 6 6 0 0 0 9 10z"/>', code: '<path d="M8 7l-5 5 5 5M16 7l5 5-5 5"/>', plus: '<path d="M12 5v14M5 12h14"/>',
    warn: '<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17h.01"/>', left: '<path d="M15 6l-6 6 6 6"/>', right: '<path d="M9 6l6 6-6 6"/>',
    undo: '<path d="M9 14L4 9l5-5"/><path d="M4 9h10a6 6 0 0 1 0 12h-3"/>', redo: '<path d="M15 14l5-5-5-5"/><path d="M20 9H10a6 6 0 0 0 0 12h3"/>', col: '<rect x="4" y="5" width="16" height="14" rx="2"/><path d="M10 5v14M4 11h16"/>',
    hide: '<path d="M3 3l18 18M10 6.2A9 9 0 0 1 21 12a13 13 0 0 1-3 3.8M6.3 7.5A13 13 0 0 0 3 12s3.5 6 9 6a9 9 0 0 0 3-.5"/>', lock: '<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
    edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', copy: '<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>', zap: '<path d="M13 3L5 14h6l-1 7 8-11h-6z"/>',
    trash: '<path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13"/>', eye: '<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>', type: '<path d="M5 6V4h14v2M12 4v16M9 20h6"/>',
    clean: '<path d="M5 19l9-9M14 5l5 5M13 6l-4-2 1 4"/>', list: '<path d="M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01"/>', opt: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .8-1 1.7M12 17h.01"/>',
    split: '<path d="M12 4v16M5 8l-2 4 2 4M19 8l2 4-2 4"/>',
  };
  const svg = (n) => `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[n]}</svg>`;
  const INFO = {
    geral: ["Como usar", "<p>Ensine pelo exemplo: marque pedaços da linha e a tabela se monta sozinha, calculada pelo motor nativo a partir das suas amostras reais.</p><ul><li>Clique numa <b>etiqueta</b> para virar coluna; clique de novo para ignorar.</li><li>Selecione qualquer trecho e use o <b>botão direito</b>.</li><li>Botão direito no <b>cabeçalho</b> ou numa <b>célula</b> da tabela.</li></ul><p>Na linha de exemplo: <kbd>Tab</kbd> percorre as etiquetas, <kbd>C</kbd> coluna, <kbd>I</kbd> ignora, <kbd>Ctrl</kbd>+<kbd>Z</kbd> desfaz.</p>"],
    tipos: ["Tipos de linha", "<p>As linhas dos seus arquivos são agrupadas pela aparência. Cada tipo é ensinado uma vez e vira uma regra da definição.</p><p>O anel mostra quantas das amostras do tipo já são reconhecidas. Um traço no anel significa que o tipo ainda não foi ensinado.</p>"],
    linha: ["Linha de exemplo", "<p>Etiquetas sublinhadas são pedaços que o aplicativo reconheceu (IP, datas, valores de <code>chave=valor</code>). Passe o mouse para ver o tipo.</p><p>As colunas são escritas a partir desta linha. As outras linhas do tipo só mostram o resultado; para editar, volte à linha de exemplo.</p>"],
    cobertura: ["Cobertura", "<p>Parte das amostras do tipo que a definição reconhece. É uma estimativa sobre as amostras, não uma contagem do arquivo inteiro: a contagem completa aparece ao aplicar.</p><p>O selo vermelho lista as linhas que sobraram; use o botão direito nelas para ensiná-las.</p>"],
  };

  // ------------------------------------------------------------------ definição
  const def = () => S().def;
  const easyInfo = (gid) => def().ui?.easy?.[gid] || null;
  const ruleOf = (gid) => def().rules.find((r) => r.id === gid) || null;
  function setInfo(gid, value) {
    const ui = (def().ui ||= {}), easy = (ui.easy ||= {});
    if (value) easy[gid] = value; else delete easy[gid];
    if (!Object.keys(easy).length) delete ui.easy;
    if (!Object.keys(ui).length) delete def().ui;
  }
  const group = () => E.groups.find((g) => g.id === E.gid) || null;
  const lineIndex = (g) => Math.max(0, Math.min(E.idx[g.id] ?? 0, g.lines.length - 1));
  const lineOf = (g) => g.lines[lineIndex(g)];
  /** The line the columns are written from: fixed once the first column exists. */
  const pinnedText = (g) => easyInfo(g.id)?.text ?? null;
  const editable = (g) => { const pin = pinnedText(g); return !pin || pin === lineOf(g)?.text; };
  const fieldNames = (g) => Object.keys(ruleOf(g.id)?.fields || {});
  const segsOf = (g) => easyInfo(g.id)?.segments || [];
  const colorOf = (name) => M.fieldColor(name, def().ui?.colors || {});
  const GROK_TYPES = { TIMESTAMP_ISO8601: ["datetime", { format: "iso8601" }], HTTPDATE: ["datetime", { format: "%d/%b/%Y:%H:%M:%S %z" }], INT: ["integer"], POSINT: ["integer"], NONNEGINT: ["integer"], NUMBER: ["decimal"], BASE10NUM: ["decimal"], IP: ["ip"] };
  const NAME_HINT = { TIMESTAMP_ISO8601: "quando", HTTPDATE: "quando", SYSLOGTIMESTAMP: "quando", IP: "ip", UUID: "id", MAC: "mac", EMAILADDRESS: "email", URI: "url", WINPATH: "caminho", UNIXPATH: "caminho", QUOTEDSTRING: "texto" };

  /** A catalog name for an expression; compositions get a reusable pattern of their own. */
  function tokenFor(expression) {
    if (!expression.includes("%{")) return expression;
    const name = "DATA_E_HORA_BARRAS";
    (def().grokPatterns ||= {})[name] = expression;
    return name;
  }
  function outputFor(token, conv) {
    const [type, datetime] = GROK_TYPES[token] || (conv === "int" ? ["integer"] : conv === "float" ? ["decimal"] : ["text"]);
    return { type, datetime };
  }
  /** Rewrites the rule of a group from the marks made on its example line. */
  function regenerate(g, segments, text) {
    const rule = ensureRule(g);
    const info = { text, segments, group: g.name };
    setInfo(g.id, info);
    // Recognized pieces that sit between marked ones (an IP, a date) are kept as unnamed patterns, not
    // as the literal text of this one line, so the other lines of the type still match.
    const marked = segments.filter((s) => s.kind === "field" || s.kind === "ignore");
    const first = Math.min(...marked.map((s) => s.start)), last = Math.max(...marked.map((s) => s.end));
    const generalized = marked.length ? (E.chips[text] || []).filter((c) => c.s >= first && c.e <= last && !segments.some((s) => s.start < c.e && c.s < s.end)).map((c) => ({ kind: "ignore", auto: true, start: c.s, end: c.e, token: c.token })) : [];
    rule.extract = { kind: "grok", patterns: [M.compile(text, [...segments, ...generalized])], anchor: rule.extract?.anchor || "search", source: "message" };
    const wanted = [];
    for (const s of segments) if (s.kind === "field" && s.name && !wanted.includes(s.name)) wanted.push(s.name);
    const fields = {};
    for (const name of wanted) {
      const seg = segments.find((s) => s.name === name && s.kind === "field");
      const keep = rule.fields?.[name];
      if (keep) { fields[name] = { ...keep, from: [name] }; continue; }
      const { type, datetime } = outputFor(seg.token, seg.conv);
      fields[name] = { from: [name], type, ...(datetime ? { datetime } : {}) };
    }
    rule.fields = fields;
    if (!rule.extract.patterns[0]) rule.extract.patterns[0] = "";
  }
  function ensureRule(g) {
    let rule = ruleOf(g.id);
    if (rule) return rule;
    // The empty starter rule of a brand new map makes way for the first real one.
    const d = def();
    d.rules = d.rules.filter((r) => !(r.id === "regra-1" && !Object.keys(r.fields || {}).length && !(r.extract?.patterns || []).some(Boolean)));
    rule = { id: g.id, description: g.name, selector: {}, priority: 0, fields: {}, extract: { kind: "grok", patterns: [""], anchor: "search", source: "message" } };
    d.rules.push(rule);
    return rule;
  }
  function useKnown(g) {
    const tpl = g.known.map, src = tpl.rules.find((r) => r.extract) || tpl.rules[0];
    remove(g, false);
    const rule = structuredClone(src);
    rule.id = g.id; rule.description = g.name; rule.selector = {}; rule.priority = 0; delete rule.enabled;
    const d = def();
    d.rules = d.rules.filter((r) => !(r.id === "regra-1" && !Object.keys(r.fields || {}).length && !(r.extract?.patterns || []).some(Boolean)));
    d.rules.push(rule);
    if (tpl.grokPatterns) d.grokPatterns = { ...(d.grokPatterns || {}), ...tpl.grokPatterns };
    setInfo(g.id, { template: g.known.id, text: lineOf(g).text, group: g.name });
    changed();
  }
  function remove(g, record = true) {
    const d = def();
    d.rules = d.rules.filter((r) => r.id !== g.id);
    setInfo(g.id, null);
    delete E.results[g.id];
    if (record) changed();
  }
  // commit() notifies this view through S.onChange, which repaints and asks the engine again.
  const changed = () => core.commit();

  // ------------------------------------------------------------------ dados do motor
  async function load() {
    const g = gen;
    E.loading = true; E.error = ""; paint();
    let groups = [];
    try {
      const r = await core.call({ action: "map_groups", field: "message" }, { latest: "map-groups" });
      if (g !== gen) return;
      groups = r.groups.map((x) => ({ id: x.id, name: x.name, known: x.known, population: x.population, lines: x.samples.map((s) => ({ text: s.text, file: s.fileName || "", path: s.file || "", id: s.id, eventRef: s.eventRef })) }));
      E.population = r.population;
    } catch (e) {
      if (g !== gen) return;
      if (!window.LocalNative.superseded(e)) E.error = String(e.message || e);
    }
    // Types saved in an earlier session come back even when the draw no longer shows them.
    for (const rule of def().rules) {
      const inf = easyInfo(rule.id);
      if (inf?.text && !groups.some((x) => x.id === rule.id)) groups.push({ id: rule.id, name: inf.group || rule.description || rule.id, known: null, population: null, lines: [{ text: inf.text, file: "", path: "" }] });
    }
    if (E.pasted.length) groups.push({ id: "gcolado", name: "Linhas coladas", known: null, population: null, lines: E.pasted.map((t) => ({ text: t, file: "", path: "" })) });
    for (const x of groups) { const inf = easyInfo(x.id); if (inf?.group) x.name = inf.group; }
    E.groups = groups;
    if (!groups.some((x) => x.id === E.gid)) E.gid = groups[0]?.id ?? null;
    E.loading = false;
    paint();
    void evaluate();
    void fetchChips();
  }
  async function evaluate() {
    const g = gen;
    for (const grp of E.groups) {
      const rule = ruleOf(grp.id);
      if (!rule || !Object.keys(rule.fields || {}).length) { delete E.results[grp.id]; continue; }
      const mine = (E.serial[grp.id] = (E.serial[grp.id] || 0) + 1);
      try {
        const r = await core.call({ action: "map_try", map_json: core.text(), texts: grp.lines.map((l) => l.text).slice(0, 200) }, { latest: `map-easy-${grp.id}` });
        if (g !== gen || mine !== E.serial[grp.id]) continue;
        E.results[grp.id] = r.ok === false ? { error: r.error } : r;
      } catch (e) { if (!window.LocalNative.superseded(e) && g === gen) E.results[grp.id] = { error: String(e.message || e) }; }
    }
    if (g === gen) paint();
  }
  async function fetchChips() {
    const g = group();
    if (!g) return;
    const text = pinnedText(g) ?? lineOf(g)?.text;
    if (!text || E.chips[text]) return;
    const mine = gen;
    try {
      const r = await core.call({ action: "map_suggest", texts: [text] }, { latest: "map-easy-chips" });
      if (mine !== gen) return;
      const found = [];
      for (const s of r.suggestions || []) {
        if (!["value", "key_value"].includes(s.kind)) continue;
        for (const p of s.positions || []) {
          if (p.sample !== 0) continue;
          found.push({ s: p.start, e: p.end, token: tokenFor(s.expression), name: s.kind === "key_value" ? M.clean(s.field) : NAME_HINT[s.expression] || M.clean(s.field), kv: s.kind === "key_value", label: s.label });
        }
      }
      // Overlaps: a named key=value beats a bare recognizer; otherwise the longer one wins.
      found.sort((a, b) => (b.kv - a.kv) || ((b.e - b.s) - (a.e - a.s)) || a.s - b.s);
      const kept = [];
      for (const c of found) if (!kept.some((k) => k.s < c.e && c.s < k.e)) kept.push(c);
      E.chips[text] = kept.sort((a, b) => a.s - b.s);
    } catch (e) { if (!window.LocalNative.superseded(e)) E.chips[text] = []; }
    if (mine === gen) paintLine();
  }

  // ------------------------------------------------------------------ resultado por tipo
  function stats(g) {
    const res = E.results[g.id];
    if (!res || res.error || !res.results) return null;
    let ok = 0;
    const leftovers = [];
    res.results.forEach((r, i) => {
      const t = r.trace.rules.find((x) => x.rule === g.id);
      if (t?.extraction === "recognized") ok++; else leftovers.push(i);
    });
    return { ok, total: res.results.length, ratio: res.results.length ? ok / res.results.length : 0, leftovers };
  }
  function cell(g, row, name) {
    const r = E.results[g.id]?.results?.[row];
    const o = r?.trace?.outputs?.find((x) => x.rule === g.id && x.field === name);
    return { o, r };
  }

  // ------------------------------------------------------------------ pintura
  const q = (s) => root?.querySelector(s);
  function paint() {
    if (!root) return;
    paintTop(); paintGroups(); paintSuggest(); paintLine(); paintTable(); paintCover();
  }
  function paintTop() {
    const s = S();
    const files = s.sources.length;
    const advanced = def().rules.filter((r) => !easyInfo(r.id)).length;
    q("#e-src").textContent = files ? `${s.sources.slice(0, 2).map((f) => f.name).join(" + ")}${files > 2 ? ` +${files - 2}` : ""}${E.population ? ` · ${num(E.population)} linhas` : ""}` : "sem arquivos abertos";
    q("#e-undo").disabled = !s.hist.canUndo; q("#e-redo").disabled = !s.hist.canRedo;
    const adv = q("#e-adv-count");
    adv.hidden = !advanced; adv.title = advanced ? `${advanced} regra(s) só editáveis no modo avançado` : "";
    adv.textContent = advanced ? `+${advanced}` : "";
    const pending = JSON.stringify(s.def) !== (s.base || JSON.stringify(s.def));
    q("#e-apply").disabled = !Object.keys(def().ui?.easy || {}).length && !pending && !s.base;
    q("#e-dirty").hidden = !(s.base ? JSON.stringify(s.def) !== s.base : Object.keys(def().ui?.easy || {}).length > 0);
  }
  function paintGroups() {
    const box = q("#e-groups");
    if (E.loading && !E.groups.length) { box.innerHTML = '<div class="e-skel"></div><div class="e-skel"></div>'; return; }
    box.innerHTML = E.groups.map((g) => {
      const st = stats(g), has = !!ruleOf(g.id), pct = st && has ? Math.round(st.ratio * 100) : 0;
      return `<button class="e-group" type="button" data-e-group="${h(g.id)}" aria-pressed="${g.id === E.gid}" title="${g.population ? `${num(g.population)} linhas` : ""}"><b>${h(g.name)}</b><span class="e-ring ${has && pct < 90 ? "low" : ""} ${has ? "" : "idle"}" style="--p:${pct}"><i>${has ? pct : ""}</i></span></button>`;
    }).join("") || `<p class="e-empty">${svg("list")}</p>`;
    box.querySelectorAll("[data-e-group]").forEach((b) => {
      b.onclick = () => { E.gid = b.dataset.eGroup; E.focus = -1; E.origin = null; paint(); void fetchChips(); };
      b.oncontextmenu = (ev) => { ev.preventDefault(); groupMenu(E.groups.find((x) => x.id === b.dataset.eGroup), ev.clientX, ev.clientY); };
    });
  }
  function paintSuggest() {
    const g = group(), box = q("#e-suggest");
    if (!g || ruleOf(g.id) || !g.known) { box.innerHTML = ""; return; }
    box.innerHTML = `<div class="e-suggest">${svg("zap")}<span>${h(g.known.title)}</span><button class="e-ib" id="e-use" type="button" title="Usar" aria-label="Usar este formato">${svg("check")}</button><button class="e-ib" id="e-skip" type="button" title="Dispensar" aria-label="Dispensar">${svg("x")}</button></div>`;
    q("#e-use").onclick = () => useKnown(g);
    q("#e-skip").onclick = () => { g.known = null; paintSuggest(); };
  }
  function marksFor(g) {
    const text = lineOf(g)?.text || "";
    const marks = [];
    if (editable(g)) {
      const segs = segsOf(g);
      segs.forEach((s, i) => { if (s.kind === "field" || s.kind === "ignore") marks.push({ s: s.start, e: s.end, kind: s.kind, name: s.name, seg: i }); });
      for (const c of E.chips[text] || []) if (!marks.some((m) => m.s < c.e && c.s < m.e)) marks.push({ s: c.s, e: c.e, kind: "chip", chip: c, name: c.name });
    } else {
      const r = E.results[g.id]?.results?.[lineIndex(g)];
      for (const t of r?.trace?.rules || []) if (t.rule === g.id) for (const [name, , range] of t.extracted || []) if (range) marks.push({ s: range[0], e: range[1], kind: "result", name });
    }
    return marks.sort((a, b) => a.s - b.s);
  }
  function paintLine() {
    const g = group(), lv = q("#e-line");
    if (!g) { lv.textContent = ""; q("#e-pos").textContent = ""; q("#e-title").firstChild.textContent = ""; return; }
    const l = lineOf(g), text = l?.text || "", marks = marksFor(g);
    q("#e-title").firstChild.textContent = `${g.name} `;
    q("#e-pos").textContent = `${lineIndex(g) + 1}/${g.lines.length}`;
    q("#e-prev").disabled = lineIndex(g) === 0; q("#e-next").disabled = lineIndex(g) >= g.lines.length - 1;
    let html = "", cur = 0;
    marks.forEach((m, i) => {
      html += h(text.slice(cur, m.s));
      const cls = ["e-chip"];
      if (m.kind === "field" || m.kind === "result") cls.push("col"); else if (m.kind === "ignore") cls.push("ignored");
      if (E.focus === i) cls.push("focus");
      if (E.origin && E.origin.s === m.s) cls.push("origin");
      const color = m.name ? colorOf(m.name) : "var(--text-2)";
      const label = m.kind === "field" || m.kind === "result" ? m.name : "";
      html += `<span class="${cls.join(" ")}" data-m="${i}" data-name="${h(label)}" style="--c:${color}" ${m.kind === "chip" ? `title="${h(m.chip.label || "")}"` : ""}>${h(text.slice(m.s, m.e))}</span>`;
      cur = m.e;
    });
    html += h(text.slice(cur));
    lv.innerHTML = html; lv.dataset.text = text; lv._marks = marks;
    lv.classList.toggle("readonly", !editable(g));
    lv.querySelectorAll(".e-chip").forEach((el) => { el.onclick = (ev) => { ev.stopPropagation(); clickMark(marks[+el.dataset.m]); }; });
  }
  function clickMark(m) {
    const g = group();
    if (!g || !editable(g)) { say("Volte à linha de exemplo para editar"); return; }
    const text = lineOf(g).text, segs = structuredClone(segsOf(g));
    if (m.kind === "chip") {
      const c = m.chip, taken = new Set(segs.filter((s) => s.name).map((s) => s.name));
      const name = M.uniqueName(c.name || "campo", taken);
      const added = M.addSegment(segs, { kind: "field", start: c.s, end: c.e, token: c.token, name, conv: "" });
      if (added.error) return say(added.error);
      regenerate(g, added.segments, text);
    } else if (m.kind === "field") {
      segs[m.seg] = { ...segs[m.seg], kind: "ignore" };
      regenerate(g, segs, text);
    } else if (m.kind === "ignore") {
      const s = segs[m.seg], taken = new Set(segs.filter((x) => x.name).map((x) => x.name));
      segs[m.seg] = { ...s, kind: "field", name: M.uniqueName(s.name || nameBeforeOf(text, s.start), taken) };
      regenerate(g, segs, text);
    }
    changed();
  }
  const nameBeforeOf = (text, at) => M.nameBefore(text, at) || "campo";
  function paintTable() {
    const g = group(), t = q("#e-table");
    if (!g || !ruleOf(g.id) || !fieldNames(g).length) { t.innerHTML = `<tbody><tr><td class="e-empty">${svg("col")}</td></tr></tbody>`; return; }
    const names = fieldNames(g), rule = ruleOf(g.id), res = E.results[g.id];
    if (res?.error) { t.innerHTML = `<tbody><tr><td class="e-empty" title="${h(res.error)}">${svg("warn")}</td></tr></tbody>`; return; }
    const head = `<thead><tr>${names.map((n) => `<th tabindex="0" data-e-col="${h(n)}" aria-haspopup="menu"><span class="e-dot" style="--c:${colorOf(n)}"></span>${h(n)}<span class="e-ty">${h(typeLabel(rule.fields[n]))}</span></th>`).join("")}</tr></thead>`;
    const rows = g.lines.slice(0, 200).map((l, r) => `<tr class="${r === lineIndex(g) ? "current" : ""}">${names.map((n) => {
      const { o } = cell(g, r, n);
      const bad = o?.status === "invalid", miss = !o || ["missing", "empty"].includes(o.status);
      const val = o?.normalized == null ? "" : typeof o.normalized === "string" ? o.normalized : JSON.stringify(o.normalized);
      return `<td data-r="${r}" data-n="${h(n)}" class="${bad ? "bad" : miss ? "miss" : ""} ${["integer", "decimal"].includes(rule.fields[n].type) ? "num" : ""}" title="${h(bad ? o.reason || "" : "")}">${bad ? "" : miss ? "—" : h(val)}</td>`;
    }).join("")}</tr>`).join("");
    t.innerHTML = `${head}<tbody>${rows}</tbody>`;
    t.querySelectorAll("th").forEach((th) => {
      const open = (x, y) => headerMenu(g, th.dataset.eCol, x, y);
      th.oncontextmenu = (ev) => { ev.preventDefault(); open(ev.clientX, ev.clientY); };
      th.onclick = (ev) => { const b = th.getBoundingClientRect(); open(b.left, b.bottom + 4); ev.stopPropagation(); };
      th.onkeydown = (ev) => { if (["Enter", "ContextMenu"].includes(ev.key) || (ev.shiftKey && ev.key === "F10")) { ev.preventDefault(); const b = th.getBoundingClientRect(); open(b.left, b.bottom); } };
    });
    t.querySelectorAll("td[data-r]").forEach((td) => {
      td.oncontextmenu = (ev) => { ev.preventDefault(); cellMenu(g, +td.dataset.r, td.dataset.n, ev.clientX, ev.clientY); };
      td.onclick = () => { E.idx[g.id] = +td.dataset.r; paintLine(); paintTable(); };
    });
  }
  const TYPE_LABEL = { text: "texto", integer: "inteiro", decimal: "decimal", boolean: "booleano", datetime: "data/hora", ip: "IP" };
  const typeLabel = (m) => (m?.type ? TYPE_LABEL[m.type] || m.type : "texto") + (m?.steps?.length ? " ·" : "");
  function paintCover() {
    const g = group(), bar = q("#e-cov-bar"), txt = q("#e-cov"), btn = q("#e-left");
    const st = g && ruleOf(g.id) ? stats(g) : null;
    if (!st) { bar.className = "e-bar"; bar.firstElementChild.style.width = "0"; txt.textContent = "—"; txt.title = ""; btn.hidden = true; return; }
    const pct = Math.round(st.ratio * 1000) / 10;
    bar.className = `e-bar ${st.ratio < 0.9 ? "low" : ""}`; bar.firstElementChild.style.width = `${st.ratio * 100}%`;
    txt.textContent = `${pct}%`;
    txt.title = g.population ? `Estimativa: ${num(Math.round(g.population * st.ratio))} de ${num(g.population)} linhas (a partir de ${st.total} amostras)` : `${st.ok} de ${st.total} linhas`;
    btn.hidden = !st.leftovers.length;
    btn.innerHTML = st.leftovers.length ? `${svg("warn")}${st.leftovers.length}` : "";
    btn.title = "Linhas que sobraram";
  }

  // ------------------------------------------------------------------ menus
  const menu = () => q("#e-menu");
  function showMenu(items, x, y, title) {
    const m = menu();
    m.innerHTML = (title ? `<div class="e-mt">${h(title)}</div>` : "") + items.map((it, i) => it === "-" ? "<hr>" : `<button type="button" role="menuitem" data-i="${i}" class="${it.first ? "first" : ""}" ${it.disabled ? "disabled" : ""}>${svg(it.icon || "col")}<span>${h(it.label)}</span>${it.check ? svg("check") : ""}${it.key ? `<kbd>${it.key}</kbd>` : ""}</button>`).join("");
    m.hidden = false;
    m.style.left = `${Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8))}px`; m.style.top = `${Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8))}px`;
    m.querySelectorAll("button").forEach((b) => { b.onclick = () => { const it = items[+b.dataset.i]; hideMenu(); it.run(); }; });
    m.querySelector("button:not([disabled])")?.focus();
  }
  const hideMenu = () => { const m = menu(); if (m) m.hidden = true; };
  let toastTimer;
  function say(message) { const t = q("#e-toast"); if (!t) return; t.textContent = message; t.hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => { t.hidden = true; }, 2200); }

  function selection() {
    const lv = q("#e-line"), sel = getSelection();
    if (!sel || sel.isCollapsed || !lv.contains(sel.anchorNode) || !lv.contains(sel.focusNode)) return null;
    const at = (n, o) => { const r = document.createRange(); r.selectNodeContents(lv); r.setEnd(n, o); return r.toString().length; };
    let s = at(sel.anchorNode, sel.anchorOffset), e = at(sel.focusNode, sel.focusOffset);
    if (s > e) [s, e] = [e, s];
    return s === e ? null : { start: s, end: e };
  }
  /** Right-click without a selection picks the mark or the word under the pointer. */
  function selectUnder(ev) {
    const lv = q("#e-line"), s = getSelection();
    const chip = ev.target.closest?.(".e-chip");
    s.removeAllRanges();
    if (chip?.firstChild) { const r = document.createRange(); r.selectNodeContents(chip.firstChild); s.addRange(r); return; }
    const pos = document.caretPositionFromPoint?.(ev.clientX, ev.clientY), range = document.caretRangeFromPoint?.(ev.clientX, ev.clientY);
    const node = pos?.offsetNode ?? range?.startContainer, off = pos?.offset ?? range?.startOffset;
    if (!node || node.nodeType !== 3 || !lv.contains(node)) return;
    const t = node.textContent; let a = off, b = off;
    while (a > 0 && /\S/.test(t[a - 1])) a--;
    while (b < t.length && /\S/.test(t[b])) b++;
    if (a === b) return;
    const r = document.createRange(); r.setStart(node, a); r.setEnd(node, b); s.addRange(r);
  }
  function lineMenu(ev) {
    const g = group();
    if (!g) return;
    let sel = selection();
    if (!sel) { selectUnder(ev); sel = selection(); }
    if (!sel) return;
    ev.preventDefault();
    const text = lineOf(g).text, picked = text.slice(sel.start, sel.end);
    if (!editable(g)) {
      showMenu([{ label: "Voltar à linha de exemplo", icon: "left", first: true, run: () => { E.idx[g.id] = Math.max(0, g.lines.findIndex((l) => l.text === pinnedText(g))); paint(); } }], ev.clientX, ev.clientY);
      return;
    }
    const segs = structuredClone(segsOf(g));
    const chip = (E.chips[text] || []).find((c) => c.s === sel.start && c.e === sel.end);
    const delimiter = M.delimiterOf(text, sel);
    const kindOf = M.classify(picked.trim());
    const isSep = delimiter && !/[\p{L}\p{N}]/u.test(delimiter);
    const apply = (action, options = {}) => () => {
      const out = M.apply(text, segs, action, sel, options);
      if (out.error) return say(out.error);
      regenerate(g, out.segments, text); changed();
    };
    const items = [];
    const pretty = chip?.name || M.nameBefore(text, sel.start) || "campo";
    if (isSep) items.push({ label: `Separar a linha por “${delimiter}”`, icon: "split", first: true, run: () => splitBy(g, text, delimiter) });
    items.push({ label: `Coluna “${pretty}”${kindOf === "ip" ? "" : ""}`, icon: "col", first: !isSep, key: "C", run: apply("extract", chip ? { token: chip.token, name: chip.name } : {}) });
    items.push({ label: "O que vem depois", icon: "right", run: apply("after") }, { label: "O que vem antes", icon: "left", run: apply("before") },
      { label: "Entre marcadores…", icon: "code", run: async () => { const v = await ask("Entre marcadores", [["open", "Começa em"], ["close", "Termina em"]]); if (v) apply("between", { open: v.open, close: v.close, name: M.clean(v.name || "") || undefined })(); } }, "-",
      { label: "Ignorar", icon: "hide", key: "I", run: apply("ignore") }, { label: "Texto fixo", icon: "lock", run: apply("fixed") },
      { label: "Permitir variação", icon: "opt", run: apply("variation") }, { label: "Opcional", icon: "opt", run: apply("optional") }, "-",
      { label: "Desfazer", icon: "undo", key: "Ctrl+Z", run: () => core.S.hist.canUndo && undo() });
    showMenu(items, ev.clientX, ev.clientY, `“${picked.slice(0, 30)}${picked.length > 30 ? "…" : ""}”`);
  }
  function splitBy(g, text, delimiter) {
    const parts = text.split(delimiter).length;
    const rule = ensureRule(g);
    rule.extract = { kind: "delimited", delimiter, quote: '"', columns: Array.from({ length: parts }, (_, i) => `coluna_${i + 1}`), source: "message" };
    rule.fields = Object.fromEntries(rule.extract.columns.map((n) => [n, { from: [n], type: "text" }]));
    setInfo(g.id, { text, segments: [], delimited: true, group: g.name });
    changed();
  }
  function headerMenu(g, name, x, y) {
    const rule = ruleOf(g.id), field = rule.fields[name];
    const steps = field.steps || [];
    const has = (op) => steps.some((s) => s.op === op);
    const toggle = (op) => () => { if (has(op)) field.steps = steps.filter((s) => s.op !== op); else (field.steps ||= []).push({ op }); if (!field.steps.length) delete field.steps; changed(); };
    showMenu([
      { label: "Renomear", icon: "edit", first: true, run: async () => { const v = await ask("Renomear coluna", [["name", "Nome", name]]); if (v?.name) rename(g, name, M.clean(v.name)); } },
      { label: "Tipo…", icon: "type", run: () => typeDialog(g, name) }, "-",
      { label: "Minúsculas", icon: "clean", check: has("lowercase"), run: toggle("lowercase") }, { label: "MAIÚSCULAS", icon: "clean", check: has("uppercase"), run: toggle("uppercase") },
      { label: "Sem espaços nas pontas", icon: "clean", check: has("trim"), run: toggle("trim") }, { label: "Só o nome do arquivo", icon: "clean", check: has("basename"), run: toggle("basename") }, { label: "Hex → decimal", icon: "clean", check: has("hex_to_int"), run: toggle("hex_to_int") }, "-",
      { label: "Valores frequentes", icon: "list", run: () => frequent(g, name, x, y) },
      { label: "Criar índice", icon: "zap", run: () => makeIndex(g, name) },
      { label: "Duplicar", icon: "copy", run: () => { const copy = M.uniqueName(`${name}_2`, new Set(Object.keys(rule.fields))); rule.fields[copy] = { ...structuredClone(field), from: [name] }; changed(); } },
      { label: "Remover", icon: "trash", run: () => removeColumn(g, name) },
    ], x, y, name);
  }
  function rename(g, from, to) {
    const rule = ruleOf(g.id);
    if (!to || to === from || rule.fields[to]) { if (to && to !== from) say("Já existe uma coluna com esse nome"); return; }
    const segs = structuredClone(segsOf(g)), text = pinnedText(g);
    if (text && segs.length && !easyInfo(g.id).delimited) {
      segs.forEach((s) => { if (s.name === from) s.name = to; });
      rule.fields = Object.fromEntries(Object.entries(rule.fields).map(([k, v]) => [k === from ? to : k, v]));
      regenerate(g, segs, text);
    } else {
      rule.fields = Object.fromEntries(Object.entries(rule.fields).map(([k, v]) => [k === from ? to : k, k === from ? { ...v, from: [to] } : v]));
    }
    changed();
  }
  function removeColumn(g, name) {
    const rule = ruleOf(g.id), text = pinnedText(g), segs = structuredClone(segsOf(g));
    if (text && segs.length && !easyInfo(g.id).delimited) {
      // The text stays in the expression as an unnamed piece, so the other lines still match.
      segs.forEach((s) => { if (s.kind === "field" && s.name === name) s.kind = "ignore"; });
      delete rule.fields[name];
      regenerate(g, segs, text);
    } else delete rule.fields[name];
    changed();
  }
  function frequent(g, name, x, y) {
    const counts = {};
    (E.results[g.id]?.results || []).forEach((r) => { const o = r.trace.outputs.find((z) => z.rule === g.id && z.field === name); if (o && o.normalized != null) { const v = String(o.normalized); counts[v] = (counts[v] || 0) + 1; } });
    const rows = Object.entries(counts).sort((a, b) => b[1] - a[1]).slice(0, 8);
    const p = q("#e-pop");
    p.innerHTML = `<ul>${rows.map(([v, n]) => `<li><span>${h(v)}</span><b>${n}</b></li>`).join("") || "<li>—</li>"}</ul>`;
    p.hidden = false; p.style.left = `${Math.max(8, Math.min(x, innerWidth - 340))}px`; p.style.top = `${Math.max(8, Math.min(y, innerHeight - 260))}px`;
  }
  async function makeIndex(g, name) {
    const s = S();
    if (!s.installed || JSON.stringify(s.def) !== s.base) { say("Aplique ao Caso antes de criar índices"); return; }
    const type = ruleOf(g.id).fields[name].type;
    const kind = ["integer", "decimal", "datetime"].includes(type) ? "range" : !type || type === "text" ? "terms" : "exact";
    try { await api("field_index_create", { column: `${s.installed}.${name}`, kind }, { analysisOwner: window.AnalysisContexts.capture(), latest: "field-index" }); say(`Índice de ${name} criado`); }
    catch (e) { if (!window.LocalNative.superseded(e)) say(String(e.message || e)); }
  }
  function cellMenu(g, row, name, x, y) {
    const { o } = cell(g, row, name), items = [];
    if (o?.status === "invalid") {
      items.push({ label: o.reason || "Valor inválido", icon: "warn", first: true, run: () => {} });
      if (/,/.test(o.original || "") && ruleOf(g.id).fields[name].type === "decimal") items.push({ label: "Trocar vírgula por ponto", icon: "check", run: () => { const f = ruleOf(g.id).fields[name]; (f.steps ||= []).push({ op: "replace", from: ",", to: "." }); changed(); } });
      items.push("-");
    }
    items.push({ label: "Copiar", icon: "copy", run: () => { navigator.clipboard?.writeText(String(o?.normalized ?? "")); say("Copiado"); } },
      { label: "Ver na linha", icon: "eye", run: () => { E.idx[g.id] = row; E.origin = o?.range ? { s: o.range[0] } : null; paint(); setTimeout(() => { E.origin = null; paintLine(); }, 1800); } });
    showMenu(items, x, y, `${name} · linha ${row + 1}`);
  }
  function groupMenu(g, x, y) {
    if (!g) return;
    showMenu([
      { label: "Renomear tipo", icon: "edit", first: true, run: async () => { const v = await ask("Renomear tipo", [["name", "Nome", g.name]]); if (v?.name) { g.name = v.name.trim(); const inf = easyInfo(g.id); if (inf) { inf.group = g.name; ruleOf(g.id).description = g.name; core.commit(); } paint(); } } },
      { label: "Recomeçar este tipo", icon: "undo", disabled: !ruleOf(g.id), run: () => remove(g) },
    ], x, y, g.name);
  }
  function leftoversMenu(g, i, x, y) {
    showMenu([{ label: "Ensinar esta linha", icon: "edit", first: true, run: () => teach(g, i) }, { label: "Copiar", icon: "copy", run: () => { navigator.clipboard?.writeText(g.lines[i].text); say("Copiado"); } }], x, y, "Linha que sobrou");
  }
  /** A line the type does not recognize becomes the example of a type of its own. */
  function teach(g, i) {
    const l = g.lines.splice(i, 1)[0];
    const id = `gl${(Math.abs(hash(l.text)) >>> 0).toString(16)}`;
    const existing = E.groups.find((x) => x.id === id);
    if (existing) { E.gid = id; } else { E.groups.push({ id, name: `Tipo ${E.groups.length + 1}`, known: null, population: null, lines: [l] }); E.gid = id; }
    E.idx[id] = 0;
    delete E.results[g.id];
    paint(); void evaluate(); void fetchChips();
  }
  const hash = (t) => [...t].reduce((a, c) => (Math.imul(a, 31) + c.charCodeAt(0)) | 0, 7);
  function showLeftovers(ev) {
    const g = group(), st = g && stats(g);
    if (!st) return;
    ev.stopPropagation();
    const p = q("#e-pop"), b = ev.currentTarget.getBoundingClientRect();
    p.innerHTML = `<ul>${st.leftovers.map((i) => `<li data-i="${i}"><span title="${h(g.lines[i].text)}">${h(g.lines[i].text)}</span><button class="e-ib" type="button" data-teach aria-label="Ensinar esta linha">${svg("edit")}</button></li>`).join("")}</ul>`;
    p.hidden = false; p.style.width = `${Math.min(560, innerWidth - 24)}px`;
    p.style.left = `${Math.max(8, Math.min(b.right - 560, innerWidth - 580))}px`; p.style.top = `${b.bottom + 6}px`;
    p.querySelectorAll("li").forEach((li) => {
      li.oncontextmenu = (e) => { e.preventDefault(); leftoversMenu(g, +li.dataset.i, e.clientX, e.clientY); };
      li.querySelector("[data-teach]").onclick = () => { p.hidden = true; teach(g, +li.dataset.i); };
    });
  }

  // ------------------------------------------------------------------ diálogos
  /** A small form: fields are [key, label, initial]. Resolves to an object or null. */
  function ask(title, fields) {
    return new Promise((resolve) => {
      const d = q("#e-dlg");
      d.innerHTML = `<form method="dialog"><h2>${h(title)}</h2>${fields.map(([k, l, v]) => `<label>${h(l)}<input name="${k}" value="${h(v || "")}" autocomplete="off"></label>`).join("")}<div class="e-row"><button class="e-btn" value="no">Cancelar</button><button class="e-btn primary" value="ok">OK</button></div></form>`;
      d.onclose = () => { const f = new FormData(d.querySelector("form")); resolve(d.returnValue === "ok" ? Object.fromEntries(f.entries()) : null); };
      d.showModal(); d.querySelector("input")?.select();
    });
  }
  function typeDialog(g, name) {
    const field = ruleOf(g.id).fields[name], d = q("#e-dlg");
    d.innerHTML = `<form method="dialog"><h2>Tipo de “${h(name)}”</h2><div class="e-types">${Object.entries(TYPE_LABEL).map(([k, l]) => `<label><input type="radio" name="type" value="${k}" ${(field.type || "text") === k ? "checked" : ""}> ${h(l)}</label>`).join("")}</div><div id="e-dt" ${field.type === "datetime" ? "" : "hidden"}><label>Formato<input name="format" value="${h(field.datetime?.format || "iso8601")}" list="e-fmts"></label><label>Fuso horário<input name="timezone" value="${h(field.datetime?.timezone || "utc")}" placeholder="utc ou -03:00"></label><datalist id="e-fmts"><option value="iso8601"><option value="epoch_s"><option value="epoch_ms"><option value="%d/%b/%Y:%H:%M:%S %z"><option value="%Y-%m-%d %H:%M:%S"><option value="%d/%m/%Y %H:%M:%S"></datalist></div><div class="e-row"><button class="e-btn" value="no">Cancelar</button><button class="e-btn primary" value="ok">OK</button></div></form>`;
    d.querySelectorAll("[name=type]").forEach((r) => { r.onchange = () => { d.querySelector("#e-dt").hidden = r.value !== "datetime" || !r.checked; }; });
    d.onclose = () => {
      if (d.returnValue !== "ok") return;
      const f = Object.fromEntries(new FormData(d.querySelector("form")).entries());
      if (f.type === "text" || !f.type) delete field.type; else field.type = f.type;
      if (f.type === "datetime") field.datetime = { format: f.format || "iso8601", ...(f.timezone ? { timezone: f.timezone } : {}) }; else delete field.datetime;
      if (field.index === "range" && !["integer", "decimal", "datetime"].includes(f.type)) delete field.index;
      changed();
    };
    d.showModal();
  }
  function showInfo(topic) {
    const [title, body] = INFO[topic], d = q("#e-dlg");
    d.innerHTML = `<form method="dialog"><h2>${h(title)}</h2>${body}<div class="e-row"><button class="e-btn">Fechar</button></div></form>`;
    d.onclose = null; d.showModal();
  }
  async function applyDialog() {
    const s = S(), d = q("#e-dlg");
    if (!s.valid) { say(s.error || "A definição tem problemas"); return; }
    const easy = Object.keys(def().ui?.easy || {});
    if (!easy.length) { say("Ensine ao menos um tipo de linha"); return; }
    const prefix = s.name || "norm";
    const lines = easy.map((gid) => { const g = E.groups.find((x) => x.id === gid), st = g && stats(g); return `<li><b>${h(g?.name || gid)}</b>${st ? ` · ${Math.round(st.ratio * 100)}%` : ""}<br>${Object.keys(ruleOf(gid)?.fields || {}).map((n) => `<code>${h(prefix)}.${h(n)}</code>`).join(" ")}</li>`; }).join("");
    d.innerHTML = `<form method="dialog"><h2>Aplicar ao Caso</h2><label>Nome do campo<input id="e-prefix" value="${h(prefix)}" autocomplete="off"></label><ul>${lines}</ul><div id="e-scan"></div><div class="e-row"><button type="button" class="e-btn" id="e-do-scan">Conferir no conjunto</button><span class="e-grow"></span><button class="e-btn" value="no">Voltar</button><button class="e-btn primary" value="ok">Aplicar</button></div></form>`;
    d.querySelector("#e-do-scan").onclick = async (ev) => {
      const b = ev.currentTarget; b.disabled = true; d.querySelector("#e-scan").innerHTML = `<p class="e-note">…</p>`;
      try {
        await core.scan();
        const r = core.S.scan;
        d.querySelector("#e-scan").innerHTML = r?.error ? `<p class="e-note">${h(r.error)}</p>` : r?.inspected != null ? `<p class="e-note"><b>${num(r.affected)}</b> de ${num(r.inspected)} registros ganham campos · ${(r.rules || []).map((x) => `${h(x.rule === "r" ? "" : "")}${num(x.recognized)}/${num(x.eligible)}`).join(" · ")}</p>` : "";
      } catch (e) { d.querySelector("#e-scan").innerHTML = `<p class="e-note">${h(String(e.message || e))}</p>`; }
      b.disabled = false;
    };
    d.onclose = async () => {
      if (d.returnValue !== "ok") return;
      const name = (d.querySelector("#e-prefix")?.value || "").trim() || "norm";
      core.S.name = name; core.paintHeader();
      try { await core.save(); paint(); say("Aplicado ao Caso"); } catch (e) { say(String(e.message || e)); }
    };
    d.showModal();
  }
  function undo() { core.root.querySelector("#ex-adv")?.querySelector('[data-act="undo"]')?.click(); }

  // ------------------------------------------------------------------ montagem
  function shell() {
    root.innerHTML = `<div class="exe-top"><span class="e-pill" id="e-src"></span><span class="e-dirty" id="e-dirty" hidden title="Alterações não aplicadas"></span><span class="e-grow"></span>
      <span class="e-pill" id="e-adv-count" hidden></span>
      <button class="e-ib" id="e-undo" type="button" title="Desfazer" aria-label="Desfazer">${svg("undo")}</button><button class="e-ib" id="e-redo" type="button" title="Refazer" aria-label="Refazer">${svg("redo")}</button>
      <button class="e-ib" data-e-info="geral" type="button" title="Como usar" aria-label="Como usar">${svg("info")}</button>
      <button class="e-ib" id="e-adv" type="button" title="Avançado" aria-label="Modo avançado">${svg("code")}</button>
      <button class="e-btn primary" id="e-apply" type="button">${svg("check")}Aplicar</button></div>
    <div class="exe-wrap"><aside class="exe-types" aria-label="Tipos de linha"><div class="e-sec">Tipos <button class="e-i" data-e-info="tipos" type="button" aria-label="Mais informações">${svg("info")}</button></div><div id="e-groups"></div>
      <div class="e-add"><input id="e-paste" placeholder="Colar linha…" aria-label="Colar uma linha"><button class="e-ib" id="e-paste-add" type="button" aria-label="Adicionar linha">${svg("plus")}</button></div></aside>
    <section class="exe-stage"><div id="e-suggest"></div>
      <div class="e-card"><div class="e-lvhead"><div class="e-sec" id="e-title">x <button class="e-i" data-e-info="linha" type="button" aria-label="Mais informações">${svg("info")}</button></div><button class="e-ib" id="e-prev" type="button" aria-label="Linha anterior">${svg("left")}</button><small id="e-pos"></small><button class="e-ib" id="e-next" type="button" aria-label="Próxima linha">${svg("right")}</button></div>
        <div class="e-line" id="e-line" tabindex="0" role="textbox" aria-readonly="true" aria-label="Linha de exemplo"></div></div>
      <div class="e-cov"><span class="e-sec" style="margin:0">Cobertura <button class="e-i" data-e-info="cobertura" type="button" aria-label="Mais informações">${svg("info")}</button></span><div class="e-bar" id="e-cov-bar"><i></i></div><strong id="e-cov"></strong><button class="e-left" id="e-left" type="button" hidden></button></div>
      <div class="e-card flush"><div class="e-tablewrap"><table id="e-table"></table></div></div>
      <p class="e-error" id="e-error" hidden></p></section></div>
    <div class="e-menu" id="e-menu" role="menu" hidden></div><div class="e-pop" id="e-pop" hidden></div><div class="e-toast" id="e-toast" role="status" hidden></div><dialog class="e-dlg" id="e-dlg"></dialog>`;
    root.querySelectorAll("[data-e-info]").forEach((b) => { b.onclick = () => showInfo(b.dataset.eInfo); });
    q("#e-adv").onclick = () => core.setView("adv");
    q("#e-undo").onclick = () => undo(); q("#e-redo").onclick = () => core.root.querySelector('[data-act="redo"]')?.click();
    q("#e-apply").onclick = applyDialog;
    q("#e-prev").onclick = () => { const g = group(); E.idx[g.id] = lineIndex(g) - 1; paint(); void fetchChips(); };
    q("#e-next").onclick = () => { const g = group(); E.idx[g.id] = lineIndex(g) + 1; paint(); void fetchChips(); };
    q("#e-left").onclick = showLeftovers;
    q("#e-paste").onkeydown = (e) => { if (e.key === "Enter") q("#e-paste-add").click(); };
    q("#e-paste-add").onclick = () => {
      const t = q("#e-paste").value.trim();
      if (!t) return;
      E.pasted.push(t); q("#e-paste").value = "";
      const existing = E.groups.find((x) => x.id === "gcolado");
      if (existing) existing.lines.push({ text: t, file: "", path: "" }); else E.groups.push({ id: "gcolado", name: "Linhas coladas", known: null, population: null, lines: [{ text: t, file: "", path: "" }] });
      E.gid = "gcolado"; E.idx.gcolado = E.groups.find((x) => x.id === "gcolado").lines.length - 1;
      paint(); void evaluate(); void fetchChips();
    };
    q("#e-line").oncontextmenu = lineMenu;
    q("#e-line").onkeydown = lineKeys;
    root.onclick = (e) => { if (!menu().contains(e.target)) hideMenu(); if (!q("#e-pop").contains(e.target)) q("#e-pop").hidden = true; };
  }
  function lineKeys(e) {
    const g = group(), lv = q("#e-line"), marks = lv._marks || [];
    if (!g) return;
    if (e.key === "Tab" && !e.shiftKey && E.focus < marks.length - 1) { e.preventDefault(); E.focus++; paintLine(); lv.focus(); }
    else if (e.key === "Tab" && e.shiftKey && E.focus > 0) { e.preventDefault(); E.focus--; paintLine(); lv.focus(); }
    else if (E.focus >= 0 && ["c", "C"].includes(e.key)) { e.preventDefault(); const m = marks[E.focus]; if (m && m.kind !== "field") clickMark(m); }
    else if (E.focus >= 0 && ["i", "I"].includes(e.key)) { e.preventDefault(); const m = marks[E.focus]; if (m?.kind === "field") clickMark(m); }
  }
  function mount(container, c) {
    core = c; root = container; gen++; E.groups = []; E.results = {}; E.chips = {}; E.idx = {}; E.focus = -1;
    shell(); paintTop();
    core.S.onChange = () => { if (root) { paint(); void evaluate(); } };
    void load();
  }
  function refresh() { if (root) { paint(); void evaluate(); } }
  function unmount() { gen++; clearTimeout(toastTimer); Object.values(timers).forEach(clearTimeout); if (core) core.S.onChange = null; root = null; }
  return { mount, unmount, refresh, state: E };
})();
