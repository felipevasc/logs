/* Visualizar arquivos (Estrutura): the original text of a file, read in steps,
 * without parsing or preparing events. The header describes only this file. The
 * analysis is never touched until the user chooses "Abrir na análise". */
window.FileViewer = (() => {
  "use strict";
  const e = (t) => String(t ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const number = (v) => Number(v || 0).toLocaleString("pt-BR");
  const bytes = (v) => { const n = Number(v || 0); return n >= 2 ** 30 ? `${(n / 2 ** 30).toFixed(2)} GiB` : n >= 2 ** 20 ? `${(n / 2 ** 20).toFixed(1)} MiB` : n >= 1024 ? `${(n / 1024).toFixed(1)} KiB` : `${n} B`; };
  const button = (id, label, icon = "", cls = "ghost") => `<button type="button" id="${id}" class="btn ${cls} small">${icon ? `<i class="fas ${icon}" aria-hidden="true"></i> ` : ""}${label}</button>`;
  let root = null, active = false, generation = 0, loop = 0, intent = 0, ownerKey = "", bench = null;
  const raw = { status: null, source: [], hits: [], offset: 0, selected: null, line: 1, history: [], running: false, path: "", byteInLine: 0, pattern: null };
  const q = (s) => root?.querySelector(s);
  const call = (request, latest = "viewer-action") => window.LocalNative.call(request, { latest });
  function notice(text = "", error = false) {
    const n = q("#fv-notice");
    if (n) { n.hidden = !text; n.dataset.error = String(error); n.textContent = text; }
  }
  const rules = () => window.LocalExplorer?.rules() || [];
  function stop() { loop++; raw.running = false; window.LocalNative.cancel("viewer-loop"); }
  function reset() {
    stop(); window.LocalNative.cancelAll();
    Object.assign(raw, { status: null, source: [], hits: [], offset: 0, selected: null, line: 1, history: [], running: false, path: "", byteInLine: 0, pattern: null });
  }
  const text = (action, extra = {}, latest = "viewer-action") => call({ action: "text", request: { action, session: raw.status?.session, rules: rules(), ...extra } }, latest);

  function paint(row, selected) { return window.LocalExplorer.paint(row.text || "", row.spans, row.highlights, row.line === selected); }
  function sourceRows(rows, selected) {
    return rows.map((row, i) => `<div class="lx-source-row" data-selected="${row.line === selected}"><button type="button" class="lx-line-no" data-row-index="${i}" title="Abrir o evento desta posição">${number(row.line)}</button><pre>${row.truncatedBefore ? "… " : ""}${paint(row, selected)}${row.truncatedAfter ? " …" : ""}</pre>${row.lossy || row.matchesComplete === false ? '<div class="lx-source-meta">Decodificação com substituições ou busca limitada neste registro; consulte o original.</div>' : ""}</div>`).join("") || '<div class="lx-empty">O conteúdo do arquivo aparecerá aqui.</div>';
  }

  // ---- header and progress
  function renderHead() {
    const node = q("#fv-head");
    if (!node) return;
    const s = raw.status;
    if (!raw.path) { node.innerHTML = ""; return; }
    const name = raw.path.split(/[\\/]/).pop();
    const indexed = s ? Number(s.indexedBytes) / Math.max(1, Number(s.bytes)) : 0, searched = s && s.searchedBytes ? Number(s.searchedBytes) / Math.max(1, Number(s.bytes)) : 0;
    const lossy = raw.source.some((r) => r.lossy);
    node.innerHTML = `<div class="fv-title"><strong>${e(name)}</strong><span class="lx-file-path" title="${e(raw.path)}">${e(raw.path)}</span></div><div class="fv-facts">${s ? `<span>Tamanho <b>${bytes(s.bytes)}</b></span><span>Linhas lidas <b>${number(s.indexedLines)}${s.indexComplete ? "" : "+"}</b></span><span>Leitura <b>${Math.round(indexed * 100)}%</b>${s.indexComplete ? " · concluída" : raw.running ? " · em andamento" : " · pausada"}</span>${raw.pattern ? `<span>Pesquisa <b>${s.searchComplete ? "concluída" : `${Math.round(searched * 100)}%`}</b></span>` : ""}<span>Codificação <b>UTF-8</b>${lossy ? " · bytes inválidos substituídos" : ""}</span>` : "<span>Abrindo…</span>"}</div>`;
  }
  function renderProgress() {
    renderHead();
    const node = q("#lx-progress");
    if (!node) return;
    const s = raw.status;
    if (!s) { node.innerHTML = ""; return; }
    const total = Number(s.bytes), done = s.searchComplete, patterned = !!raw.pattern;
    const scanned = patterned ? Number(s.searchedBytes) : Number(s.indexedBytes), coverage = total ? Math.min(1, scanned / total) : 1;
    node.innerHTML = `<span>${patterned ? `${number(s.matches)} linhas com correspondência · ` : ""}${Math.round(coverage * 100)}% ${patterned ? "pesquisado" : "lido"}${(patterned ? done : s.indexComplete) ? " · concluído" : " · resultados parciais"}</span><span>${raw.running ? "Em andamento" : (patterned ? done : s.indexComplete) ? "" : "Pausado"}</span><progress value="${coverage}" max="1" aria-label="Cobertura da leitura"></progress>`;
    if (s.skippedRecords || s.limitedRecords) notice(`${number(s.skippedRecords)} registros não verificados (tamanho, tempo ou limite do motor); ${number(s.limitedRecords)} registros com ocorrências limitadas. Os resultados estão incompletos.`, true);
    const minimap = q("#lx-minimap");
    if (minimap) {
      minimap.innerHTML = (s.buckets || Array(128).fill(0)).map((n, i) => `<button type="button" data-bucket="${i}" data-covered="${i / 128 < coverage}" data-hit="${n > 0}" title="${number(n)} resultados · posição aproximada ${Math.round((i / 128) * 100)}%" aria-label="Posição ${i + 1}, ${number(n)} resultados"></button>`).join("");
      minimap.querySelectorAll("button").forEach((b) => (b.onclick = () => void showRaw(Math.max(1, Math.round(Number(raw.status.indexedLines) * (+b.dataset.bucket / 128)))).catch((x) => notice(String(x.message || x), true))));
    }
  }
  const pageButton = (label, action, disabled = false) => `<button type="button" class="btn ghost small" data-lx-page="${action}" ${disabled ? "disabled" : ""}>${label}</button>`;
  function renderHits() {
    const node = q("#lx-hits");
    if (!node) return;
    node.innerHTML = raw.hits.map((hit, i) => `<button type="button" class="lx-hit" role="option" aria-selected="${raw.selected === raw.offset + i}" data-hit="${i}"><small>Linha ${number(hit.line)} · byte ${e(hit.offset)}</small><code>${hit.truncatedBefore ? "… " : ""}${paint(hit, hit.line === (raw.hits[raw.selected - raw.offset]?.line))}${hit.truncatedAfter ? " …" : ""}</code></button>`).join("") || '<div class="lx-empty">Pesquise para ver as correspondências. A leitura é progressiva.</div>';
    node.querySelectorAll("[data-hit]").forEach((b) => (b.onclick = () => void selectHit(+b.dataset.hit).catch((x) => notice(String(x.message || x), true))));
    const total = raw.status?.matches, footer = q("#lx-hit-pages");
    if (footer) {
      footer.innerHTML = `${pageButton("← Anterior", "prev", raw.offset === 0)}<span>${number(raw.hits.length ? raw.offset + 1 : 0)}–${number(raw.offset + raw.hits.length)} de ${number(total)}</span>${pageButton("Próxima →", "next", raw.offset + raw.hits.length >= Number(total || 0))}`;
      footer.querySelectorAll("button").forEach((b) => (b.onclick = () => void hitPage(b.dataset.lxPage === "prev" ? -1 : 1).catch((x) => notice(String(x.message || x), true))));
    }
  }
  async function hitPage(direction) {
    raw.offset = Math.max(0, raw.offset + direction * 80);
    const result = await text("page", { matchesOnly: true, offset: raw.offset });
    raw.hits = result.lines; raw.status = result.status;
    renderHits(); renderProgress();
  }
  async function selectHit(index) {
    const hit = raw.hits[index];
    if (!hit) return;
    raw.selected = raw.offset + index;
    renderHits();
    await showRaw(Math.max(1, hit.line - 10), hit.line);
  }
  async function nextHit(direction) {
    let next = (raw.selected ?? (direction > 0 ? -1 : 1)) + direction;
    if (next < 0 || next >= Number(raw.status?.matches || 0)) return;
    if (next < raw.offset || next >= raw.offset + raw.hits.length) {
      raw.offset = Math.floor(next / 80) * 80 - (direction > 0 ? 80 : -80);
      await hitPage(direction);
    }
    await selectHit(next - raw.offset);
    q("#lx-hits [aria-selected=true]")?.scrollIntoView({ block: "nearest" });
  }
  async function showRaw(line, selected = null, remember = true, byteInLine = 0) {
    if (!raw.status) return;
    if (line > Number(raw.status.indexedLines)) { notice("Esta linha ainda não foi indexada. Continue a leitura para alcançá-la."); return; }
    if (remember) raw.history.push({ line: raw.line, byte: raw.byteInLine });
    raw.history = raw.history.slice(-80);
    raw.line = line; raw.byteInLine = byteInLine;
    const result = await text("page", { line, byteInLine });
    raw.source = result.lines;
    const node = q("#lx-source");
    if (!node) return;
    const scroll = node.scrollTop;
    node.innerHTML = sourceRows(raw.source, selected);
    node.scrollTop = selected == null ? scroll : 0;
    node.querySelectorAll("[data-row-index]").forEach((b) => (b.onclick = () => void resolveLine(raw.source[+b.dataset.rowIndex]).catch((x) => notice(String(x.message || x), true))));
    q("#lx-source-title").textContent = `Arquivo · linhas ${number(raw.source[0]?.line || line)}–${number(raw.source.at(-1)?.line || line)}`;
    renderHead();
  }
  /** Link a position of the file to its structured event, when the file is part of the analysis. */
  async function resolveLine(line) {
    if (!state.loaded) { notice("Este arquivo ainda não está na análise. Use “Abrir na análise” para ligar linhas a eventos."); return; }
    const event = await call({ action: "resolve_text", path: raw.path, offset: line.offset }, "viewer-resolve");
    await openDetail(event.id, { eventRef: event.event_ref });
  }

  // ---- reading and searching
  async function pump() {
    const version = ++loop;
    raw.running = true; renderProgress();
    try {
      while (version === loop && active && root) {
        if (!raw.status) break;
        const result = await text("step", { matchesOnly: true, offset: raw.offset }, "viewer-loop");
        if (version !== loop) break;
        raw.status = result.status; raw.hits = result.lines;
        renderHits(); renderProgress();
        if (raw.pattern ? raw.status.searchComplete : raw.status.indexComplete) break;
        await new Promise((resolve) => requestAnimationFrame(resolve));
      }
    } catch (error) { if (version === loop && !window.LocalNative.superseded(error)) notice(String(error.message || error), true); }
    finally { if (version === loop) { raw.running = false; renderProgress(); } }
  }
  const quickPattern = () => ({ text: q("#lx-pattern").value, engine: q("#lx-engine").value, caseSensitive: q("#lx-case").checked, multiline: bench?.getDraft().multiline || false, dotAll: bench?.getDraft().dotAll || false });
  async function search(pattern = quickPattern()) {
    stop(); notice();
    if (!pattern.text) { notice("Informe o texto ou padrão para pesquisar."); return; }
    if (!raw.status) { notice("Escolha um arquivo primeiro."); return; }
    const response = await text("search", { pattern, matchesOnly: true });
    raw.pattern = pattern; raw.status = response.status; raw.hits = []; raw.offset = 0; raw.selected = null;
    renderHits(); void pump();
  }

  // ---- analysis transition
  function askPlacement(path) {
    return new Promise((resolve) => {
      const dialog = document.createElement("dialog");
      dialog.className = "lx-dialog";
      dialog.innerHTML = `<header><h2>Abrir na análise</h2></header><p class="lx-help">Já existe uma fonte em análise. O arquivo <b>${e(path.split(/[\\/]/).pop())}</b> não está nela. Escolha como continuar; nada é trocado sem a sua escolha.</p><div class="lx-actions"><button type="button" class="btn primary small" data-v="merge">Unir à análise atual</button><button type="button" class="btn ghost small" data-v="replace">Substituir a análise</button><button type="button" class="btn ghost small" data-v="cancel">Cancelar</button></div>`;
      document.body.append(dialog);
      dialog.querySelectorAll("[data-v]").forEach((b) => (b.onclick = () => dialog.close(b.dataset.v)));
      dialog.onclose = () => { const v = dialog.returnValue || "cancel"; dialog.remove(); resolve(v); };
      dialog.showModal();
    });
  }
  async function openInAnalysis() {
    const path = raw.path;
    if (!path) return;
    const position = raw.hits[(raw.selected ?? raw.offset) - raw.offset] || raw.source.find((r) => r.line === raw.line) || raw.source[0];
    const owner = await window.AnalysisContexts.prepare(window.AnalysisContexts.capture(), { metadata: true });
    const current = state.currentArtifact?.source, paths = current?.paths?.length ? current.paths : current?.path ? [current.path] : [];
    const inside = state.loaded && paths.includes(path);
    let merge = false;
    if (state.loaded && !inside) {
      const choice = await askPlacement(path);
      if (choice === "cancel") return;
      merge = choice === "merge";
    }
    window.AnalysisContexts.assertOwner(owner, { revisions: false });
    stop();
    const saved = { line: raw.line, byteInLine: raw.byteInLine };
    if (!inside && !await loadData({ kind: "file", paths: [path], path, format: "auto" }, { merge })) return;
    await Workspace.showPage("explore");
    if (position) {
      try {
        const event = await call({ action: "resolve_text", path, offset: position.offset }, "viewer-resolve");
        await openDetail(event.id, { eventRef: event.event_ref });
      } catch (error) { toast(`O arquivo foi aberto na análise, mas esta posição não tem um evento visível: ${String(error.message || error)}`, "info"); }
    }
    void saved;
  }

  // ---- page
  function render() {
    generation++;
    const has = !!raw.path;
    root.innerHTML = `<section class="lx lx-text"><div class="lx-notice" id="fv-notice" role="status" hidden></div><div class="fv-top">${button("lx-open", "Escolher arquivo…", "fa-folder-open", has ? "ghost" : "primary")}${button("lx-prepare", "Abrir na análise", "fa-layer-group", "primary")}<span class="lx-help fv-hint">Lê o texto original, sem interpretar os eventos. A análise só muda quando você pedir.</span></div><div id="fv-head" class="fv-head"></div>${has ? `<div class="lx-toolbar"><label class="lx-grow">Padrão<input id="lx-pattern" autocomplete="off" spellcheck="false" placeholder="Texto ou expressão regular" value="${e(raw.pattern?.text || "")}"></label><label>Motor<select id="lx-engine"><option value="literal">Texto literal</option><option value="rust">Regex rápida (Rust)</option><option value="pcre2">PCRE2 avançado</option></select></label><label class="lx-inline"><input type="checkbox" id="lx-case">Maiúsculas</label>${button("lx-search", "Pesquisar", "fa-magnifying-glass", "primary")}${button("lx-pause", "Pausar / continuar", "fa-pause")}${button("lx-assist", "Expressão assistida…", "fa-wand-magic-sparkles")}</div><details id="fv-bench" class="expr-panel" hidden><summary>Editor de expressão <small>teste em amostra; a pesquisa no arquivo começa ao aplicar</small></summary><div class="expr-body"></div></details><p class="lx-help">F3 / Shift+F3: próximo / anterior · Ctrl+G: linha · Alt+←: voltar. A expressão é aplicada à linha física. A área hachurada no mapa ainda não foi lida.</p><div class="lx-progress" id="lx-progress" aria-live="polite"></div><div class="lx-toolbar">${button("lx-prev", "← Correspondência")}${button("lx-next", "Correspondência →")}<label class="lx-inline">Linha<input id="lx-go-line" type="number" min="1" value="${raw.line}" style="width:100px"></label>${button("lx-go", "Ir")}${button("lx-back", "Voltar")}<label class="lx-inline"><input type="checkbox" id="lx-wrap">Quebrar linhas</label>${button("lx-more-text", "Mais desta linha")}</div><div class="lx-split"><section class="lx-pane"><header><h2>Correspondências</h2><span class="lx-status-pill">Resultados parciais até concluir</span></header><div id="lx-hits" class="lx-list" role="listbox" aria-label="Resultados da busca"></div><div id="lx-hit-pages" class="lx-pagination"></div></section><section class="lx-pane"><header><h2 id="lx-source-title">Conteúdo do arquivo</h2></header><div id="lx-source" class="lx-source" tabindex="0"></div></section><div id="lx-minimap" class="lx-map" aria-label="Mapa de resultados e cobertura"></div></div><p class="lx-help">Linhas acima de 8 MiB são navegáveis por janelas e ficam fora da cobertura da busca. Bytes inválidos em UTF-8 recebem aviso. UTF-16 e formatos binários ou compactados são abertos em Eventos, que já converte. Clique no número de uma linha para abrir o evento correspondente, quando o arquivo estiver na análise.</p><details class="lx-rules"><summary>Regras de destaque · cores, severidade e padrões salvos neste Caso</summary><div id="lx-rules"></div><div class="lx-actions">${button("lx-rule-add", "Adicionar regra")}${button("lx-rule-apply", "Aplicar cores")}</div></details>` : `<div class="lx-empty fv-empty"><i class="fas fa-file-lines" aria-hidden="true"></i><p>Escolha um arquivo para examinar o texto original, pesquisar e ver o contexto das linhas, sem preparar os eventos.</p></div>`}</section>`;
    const bind = (id, fn, busyLabel) => {
      const n = q(`#${id}`);
      if (!n) return;
      n.onclick = async () => {
        const restore = busyLabel && typeof btnBusy === "function" ? btnBusy(n, busyLabel) : () => {};
        try { await fn(); } catch (error) { if (!window.LocalNative.superseded(error)) notice(String(error.message || error), true); } finally { restore(); }
      };
    };
    bind("lx-open", chooseFile);
    bind("lx-prepare", openInAnalysis, "Abrindo…");
    q("#lx-prepare").disabled = !has;
    if (!has) return;
    q("#lx-engine").value = raw.pattern?.engine || "rust"; q("#lx-case").checked = !!raw.pattern?.caseSensitive;
    bind("lx-search", () => search(), "Iniciando…");
    bind("lx-pause", () => { if (raw.running) stop(); else void pump(); renderProgress(); });
    bind("lx-prev", () => nextHit(-1)); bind("lx-next", () => nextHit(1));
    bind("lx-go", () => showRaw(Math.max(1, +q("#lx-go-line").value)));
    bind("lx-back", async () => { const previous = raw.history.pop(); if (previous) await showRaw(previous.line, null, false, previous.byte); });
    bind("lx-more-text", () => showRaw(raw.line, null, true, raw.byteInLine + 16384));
    bind("lx-assist", () => toggleBench());
    q("#lx-pattern").onkeydown = (ev) => { if (ev.key === "Enter") { ev.preventDefault(); q("#lx-search").click(); } };
    q("#lx-wrap").onchange = () => (q("#lx-source").dataset.wrap = q("#lx-wrap").checked);
    bind("lx-rule-add", () => { if (!window.LocalExplorer.addRule()) notice("O limite é de 16 regras visuais."); else rulesEditor(); });
    bind("lx-rule-apply", async () => { window.LocalExplorer.keep(); await showRaw(raw.line, null, false, raw.byteInLine); });
    rulesEditor(); renderHits(); renderProgress();
    q("#lx-source").innerHTML = sourceRows(raw.source, null);
    if (raw.status) void showRaw(raw.line, null, false, raw.byteInLine).catch((x) => notice(String(x.message || x), true));
  }
  function toggleBench() {
    const panel = q("#fv-bench");
    if (!panel) return;
    panel.hidden = !panel.hidden; panel.open = !panel.hidden;
    if (panel.hidden) return;
    if (!bench) {
      bench = window.RegexWorkbench.create(panel.querySelector(".expr-body"), {
        mode: "viewer", applyLabel: "Pesquisar no arquivo", applyNote: "Cada linha física é testada separadamente.", sampleOpen: true, scope: "line",
        pattern: q("#lx-pattern").value, engine: q("#lx-engine").value, caseSensitive: q("#lx-case").checked,
        sample: { text: raw.source.slice(0, 20).map((r) => r.text).join("\n"), label: `Linhas ${raw.source[0]?.line || 1}–${raw.source.at(-1)?.line || 1} do arquivo` },
        onApply: async (draft) => {
          q("#lx-pattern").value = draft.pattern; q("#lx-engine").value = draft.engine; q("#lx-case").checked = draft.caseSensitive;
          await search({ text: draft.pattern, engine: draft.engine, caseSensitive: draft.caseSensitive, multiline: draft.multiline, dotAll: draft.dotAll });
        },
      });
    }
    bench.focus();
  }
  function rulesEditor() {
    const node = q("#lx-rules");
    if (!node) return;
    const list = rules();
    node.innerHTML = list.map((r, i) => `<div class="lx-rule" data-rule="${i}"><input type="checkbox" aria-label="Ativar ${e(r.label)}" data-rule-field="enabled" ${r.enabled ? "checked" : ""}><input aria-label="Rótulo da regra" data-rule-field="label" value="${e(r.label)}"><input aria-label="Padrão de destaque" data-rule-field="text" value="${e(r.pattern.text)}" spellcheck="false"><select aria-label="Motor da regra" data-rule-field="engine">${[["literal", "Literal"], ["rust", "Rust"], ["pcre2", "PCRE2"]].map(([v, l]) => `<option value="${v}" ${r.pattern.engine === v ? "selected" : ""}>${l}</option>`).join("")}</select><input type="color" aria-label="Cor do destaque" data-rule-field="color" value="${r.color}"><button type="button" class="icon-btn" data-rule-remove="${i}" title="Remover regra" aria-label="Remover regra">×</button></div>`).join("");
    node.querySelectorAll("[data-rule-field]").forEach((input) => (input.onchange = () => {
      const r = rules()[+input.closest("[data-rule]").dataset.rule], key = input.dataset.ruleField;
      if (key === "enabled") r.enabled = input.checked; else if (key === "text" || key === "engine") r.pattern[key] = input.value; else r[key] = input.value;
      window.LocalExplorer.keep();
    }));
    node.querySelectorAll("[data-rule-remove]").forEach((b) => (b.onclick = () => { rules().splice(+b.dataset.ruleRemove, 1); rulesEditor(); window.LocalExplorer.keep(); }));
  }
  async function chooseFile() {
    const owner = await window.AnalysisContexts.prepare(window.AnalysisContexts.capture(), { metadata: true });
    const path = await dialogApi.open({ multiple: false, directory: false });
    if (!path) return;
    window.AnalysisContexts.assertOwner(owner);
    await openFile(path);
  }
  /** Open `path` for reading. Never loads it into the analysis. */
  async function openFile(path) {
    await window.LocalNative.ready();
    stop();
    Object.assign(raw, { status: null, pattern: null, source: [], hits: [], path: String(path), offset: 0, line: 1, byteInLine: 0, selected: null, history: [] });
    bench?.destroy(); bench = null;
    const mine = ++intent;
    await Workspace.showPage("file-viewer");
    notice("Abrindo o arquivo…");
    try {
      const result = await call({ action: "text", request: { action: "open", path: String(path), rules: rules() } }, "viewer-open");
      if (mine !== intent) return;
      ownerKey = window.LocalNative.stamp();
      raw.path = result.status.path; raw.status = result.status; raw.source = result.lines;
      if (active) { notice(); render(); void pump(); }
    } catch (error) {
      if (mine === intent && active) { notice(String(error.message || error), true); }
      throw error;
    }
  }
  function mount(container) {
    stop(); root = container; active = true;
    const stamp = window.LocalNative.stamp();
    if (ownerKey && ownerKey !== stamp) reset();
    ownerKey = stamp;
    render();
    if (raw.status && !raw.status.indexComplete) void pump();
  }
  function unmount() { stop(); active = false; root = null; generation++; bench?.destroy(); bench = null; }
  document.addEventListener("keydown", (ev) => {
    if (!active || !root) return;
    if (ev.key === "F3") { ev.preventDefault(); void nextHit(ev.shiftKey ? -1 : 1).catch((x) => notice(String(x.message || x), true)); }
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "g" && q("#lx-go-line")) { ev.preventDefault(); q("#lx-go-line").focus(); }
    if (ev.altKey && ev.key === "ArrowLeft" && q("#lx-back")) { ev.preventDefault(); ev.stopImmediatePropagation(); q("#lx-back").click(); }
  }, true);
  document.addEventListener("analysis-context-change", () => {
    if (ownerKey && ownerKey !== window.LocalNative.stamp()) {
      reset(); ownerKey = window.LocalNative.stamp();
      if (root && active) render();
    }
  });
  return { mount, unmount, open: openFile, path: () => raw.path };
})();
