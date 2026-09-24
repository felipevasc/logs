/* Workspace navigation and task-oriented analysis. */
(() => {
  "use strict";
  const home = $("#workspace-home"), content = $("#ws-content"), empty = $("#ws-empty");
  let page = "summary", overview = null, sourceList = [], serial = 0, cacheKey = "", pendingOverview = null, lastExploredKey = "";
  const history = [];
  let lastFilters = "[]", importing = false, removedEvidence = null;
  let previousSelection = { filters: [], quick: "" };
  function rememberSelection() {
    const current = { filters: structuredClone(state.filters), quick: state.quick };
    const signature = JSON.stringify(current);
    if (signature !== lastFilters) { history.push(previousSelection); if (history.length > 30) history.shift(); previousSelection = current; lastFilters = signature; cacheKey = ""; lastExploredKey = ""; }
  }
  const titles = { summary: "Resumo", timeline: "Linha do tempo", "case-timeline": "Linha do tempo", "case-trails": "Trilhas", journeys: "Possíveis trilhas", explore: "Explorar", compare: "Comparar", evidence: "Evidências", sources: "Arquivos" };
  const fmtBytes = n => n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${fmtNum(Math.ceil(n / 1000))} KB`;
  const pct = n => `${(n * 100).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}%`;
  const localInput = t => { const d = new Date(t); return new Date(t - d.getTimezoneOffset() * 60000).toISOString().slice(0, 23); };
  const sourceKey = () => JSON.stringify([workspaceScope(), workspaceScope() === "case" ? caseSig() : state.currentArtifact?.id, state.currentArtifact?.loadedAt, backendFilters(), state.derivedFields]);
  const note = text => `<div class="notice">${esc(text)}</div>`;
  const iconButton = (action, icon, title, index) => `<button class="icon-btn" data-action="${action}" data-index="${index}" title="${esc(title)}" aria-label="${esc(title)}"><i class="fas ${icon}"></i></button>`;
  const metric = (label, value, foot, cls = "") => `<div class="metric ${cls}"><div class="metric-label">${label}</div><div class="metric-value">${value}</div><div class="metric-foot">${foot}</div></div>`;

  function markPage(next) {
    page = next; document.body.dataset.page = next;
    const navPage = next === "timeline" ? "case-timeline" : next;
    document.querySelectorAll("[data-page]").forEach(b => { if (b.tagName === "BUTTON") { b.classList.toggle("selected", b.dataset.page === navPage); b.setAttribute("aria-current", b.dataset.page === navPage ? "page" : "false"); } });
    $("#ws-title").textContent = titles[next] || next;
    $("#ws-reload").hidden = ["sources", "evidence", "case-timeline", "case-trails", "journeys"].includes(next) || !state.loaded;
    $("#ws-clear-scope").hidden = !backendFilters().length || next === "evidence";
    updateCounts();
  }
  function updateCounts() {
    $("#ws-evidence-count").textContent = activeCase()?.items?.length || "";
    $("#ws-source-count").textContent = sourceList.length || "";
    $("#ws-subtitle").textContent = page === "evidence" ? (activeCase()?.name || "") : page === "journeys" ? "" : workspaceScope() === "case" ? `${fmtNum(state.total)} registros do Caso${backendFilters().length ? " no recorte" : ""}` : state.loaded ? `${fmtNum(state.total)} eventos${backendFilters().length ? " no recorte" : ""} · ${sourceList.length || 1} ${sourceList.length === 1 ? "fonte" : "fontes"}` : "";
  }
  async function showPage(next) {
    if (next === "case-timeline" && workspaceScope() === "case" && activeCase()?.workspace?.timelineMode === "volume") next = "timeline";
    if (workspaceScope() === "case" && next === "sources") { await window.WorkspaceContext.setScope("dataset", { page: "sources" }); return; }
    if (workspaceScope() === "dataset" && ["evidence", "case-trails"].includes(next) && window.WorkspaceContext && !window.WorkspaceContext.changing) { await window.WorkspaceContext.setScope("case", { page: next }); return; }
    markPage(next); closeDrawer();
    if (next === "case-timeline" && workspaceScope() === "case") {
      home.hidden = true;
      const savedView = activeCase()?.workspace?.analysisView;
      setAnalysisView([savedView, state.analysisView].find(view => ["timeline", "vtimeline", "timeline-table"].includes(view)) || "vtimeline");
      switchView("caso");
      return;
    }
    if (next === "explore") {
      if (workspaceScope() === "dataset" && !state.loaded) { await showPage("summary"); return; }
      const contextKey = sourceKey();
      switchView("viz", { deferAnalytics: true }); home.hidden = true;
      if (lastExploredKey === contextKey && state.rows?.length > 0 && !state.queryError) {
        switchTab(state.activeDatasetTab, { deferAnalytics: false });
        return;
      }
      const refreshed = await refresh({ analytics: false });
      if (refreshed && page === "explore" && contextKey === sourceKey()) {
        lastExploredKey = contextKey;
        switchTab(state.activeDatasetTab);
      }
      return;
    }
    switchView("workspace"); home.hidden = false;
    if (next === "journeys") { empty.hidden = true; content.hidden = false; await window.Journeys.render(content); return; }
    empty.hidden = workspaceScope() === "case" || state.loaded || next === "evidence" || (next === "sources" && sourceList.length > 0);
    content.hidden = !empty.hidden;
    if (!empty.hidden) return;
    if (next === "evidence") { renderEvidence(); return; }
    if (next === "case-trails") { window.CaseTrails.render(content, activeCase()); return; }
    if (next === "sources") { await renderSources(); return; }
    if (next === "compare") { await renderCompare(); return; }
    if (next === "timeline" || next === "case-timeline") { await timeline.load(); return; }
    await renderSummary();
  }
  async function getOverview(force = false) {
    const key = sourceKey();
    if (!force && overview && key === cacheKey) return overview;
    if (!force && pendingOverview?.key === key) return pendingOverview.promise;
    const promise = api("dataset_overview", analyticsRequest(workspaceScope())).then(result => {
      if (key === sourceKey()) { overview = result; cacheKey = key; }
      return result;
    }).finally(() => { if (pendingOverview?.key === key) pendingOverview = null; });
    pendingOverview = { key, promise }; return promise;
  }
  function loading(label = "Analisando os eventos…") { content.innerHTML = `<div class="ws-loading" role="status"><i class="fas fa-circle-notch spin"></i>${esc(label)}</div>`; }
  function failed(error) { content.innerHTML = `<div class="notice">${esc(String(error))}</div><button class="btn ghost" data-action="retry">Tentar novamente</button>`; }
  async function renderSummary() {
    const version = ++serial, contextKey = sourceKey(); loading();
    startOperation("summary", "Calculando resumo", "Você pode continuar explorando os eventos.");
    try {
      const data = await getOverview(); if (version !== serial || contextKey !== sourceKey() || page !== "summary") return;
      state.queryError = null; state.total = data.total; state.dataPeriod = data.start == null ? null : { min: data.start, max: data.end }; updateContextBar();
      if (state.activeOperation?.kind === "summary") finishOperation("Resumo atualizado", `${fmtNum(data.total)} eventos analisados`);
      updateCounts();
      const dated = data.total - data.undated;
      content.innerHTML = `<div class="metric-grid">
        ${metric("Eventos", fmtNum(data.total), data.start == null ? "Sem horário reconhecido" : `${esc(fmtTs(data.start).slice(0, 11))} — ${esc(fmtTs(data.end).slice(0, 11))}`)}
        ${metric("Erros", fmtNum(data.errors), `${pct(data.errors / Math.max(1, data.total))} do recorte`, "error")}
        ${metric("Avisos", fmtNum(data.warnings), `${pct(data.warnings / Math.max(1, data.total))} do recorte`, "warning")}
        ${metric("Com horário", pct(dated / Math.max(1, data.total)), data.undated ? `${fmtNum(data.undated)} sem data` : "Todos os eventos datados")}
        </div>
        ${!data.complete ? note("Análise interrompida. Atualize para calcular o conjunto completo.") : ""}
        <section class="ws-card"><div class="card-heading"><h2>Eventos no tempo</h2><button class="text-button" data-action="timeline">Ver atividade <i class="fas fa-arrow-right"></i></button></div><div class="timeline-chart" id="ws-timeline"></div></section>
        <div class="overview-grid"><div>
        <section class="ws-card"><div class="card-heading"><h2>Ocorrências em destaque</h2><span>${data.findings.length ? data.findings.length + (data.findings.length === 1 ? " indicação" : " indicações") : ""}</span></div>
        ${data.findings.length ? data.findings.map((f, i) => `<article class="finding"><i class="finding-icon ${esc(f.kind)} fas ${f.kind === "spike" ? "fa-arrow-trend-up" : f.kind === "quality" ? "fa-clock" : f.kind === "gap" ? "fa-arrows-left-right" : "fa-layer-group"}"></i><div><h3>${esc(f.title)}</h3><p>${esc(f.detail)}</p><button class="text-button" data-action="finding" data-index="${i}">${f.kind === "quality" ? "Revisar data/hora" : "Investigar"}<i class="fas fa-arrow-right"></i></button></div><div class="finding-actions">${iconButton("save-finding", "fa-bookmark", "Salvar evidência", i)}</div></article>`).join("") : '<p class="quiet-empty">Nenhuma concentração de erros ou lacuna destacada neste recorte. Você pode explorar os padrões e comparar períodos.</p>'}</section>
        <section class="ws-card"><div class="card-heading"><h2>Padrões de mensagem</h2><span>Mais frequentes</span></div>${data.patterns_limited ? note("O limite de 20.000 padrões foi atingido. Refine o período; padrões adicionais não aparecem nesta lista.") : ""}<table class="ws-table"><thead><tr><th>Mensagem</th><th class="num">Eventos</th></tr></thead><tbody>${data.patterns.slice(0, 14).map((p, i) => `<tr><td><button class="pattern-button" data-action="pattern" data-index="${i}">${esc(p.pattern || "Mensagem vazia")}</button><div class="pattern-meta">${esc(p.example.source || "Sem origem")}${p.errors ? ` · ${fmtNum(p.errors)} erros` : ""}</div></td><td class="num">${fmtNum(p.count)}<br><span class="count-bar" style="width:${90 * p.count / Math.max(1, data.patterns[0].count)}px"></span></td></tr>`).join("")}</tbody></table></section>
        </div><div><section class="ws-card"><div class="card-heading"><h2>Origens</h2><button class="text-button" data-action="sources">Ver fontes <i class="fas fa-arrow-right"></i></button></div>${data.sources.map(([source, count]) => `<button class="source-row source-filter" data-source="${esc(source)}"><i class="fas fa-server"></i><span class="source-name"><strong>${esc(source)}</strong><small>${pct(count / Math.max(1, data.total))} do recorte</small></span><span class="source-amount">${fmtNum(count)}</span></button>`).join("")}${data.sources_other ? `<div class="source-row"><span class="source-name">Outras origens</span><span>${fmtNum(data.sources_other)}</span></div>` : ""}</section>
        ${data.latency ? `<section class="ws-card"><div class="card-heading"><h2>${esc(colLabel(data.latency.field))}</h2><span>${fmtNum(data.latency.count)} valores · ${esc(data.latency.unit || "unidade da fonte")}</span></div><div class="latency-values">${["p50", "p95", "p99"].map(k => `<div><span>${k.toUpperCase()}</span><strong>${fmtNum(Math.round(data.latency[k] * 100) / 100)}</strong></div>`).join("")}</div><p class="quiet-empty">${data.latency.sampled < data.latency.count ? `Percentis estimados em ${fmtNum(data.latency.sampled)} valores distribuídos.` : "Percentis de todos os valores disponíveis."}${data.latency.incompatible ? ` ${fmtNum(data.latency.incompatible)} valores com outra unidade foram excluídos.` : ""}</p></section>` : ""}
        <section class="ws-card"><div class="card-heading"><h2>Investigar</h2></div><div class="recipe-list"><button data-action="errors"><i class="fas fa-circle-exclamation"></i>Ver erros e falhas<i class="fas fa-arrow-right"></i></button><button data-action="compare"><i class="fas fa-code-compare"></i>Comparar períodos<i class="fas fa-arrow-right"></i></button><button data-action="explore"><i class="fas fa-list"></i>Explorar todos os eventos<i class="fas fa-arrow-right"></i></button></div></section></div></div>`;
      drawTimeline(data);
    } catch (e) { if (version === serial && contextKey === sourceKey() && page === "summary") { if (state.activeOperation?.kind === "summary") finishOperation("Resumo não concluído"); failed(e); } }
    finally { if (version === serial && contextKey === sourceKey() && state.activeOperation?.kind === "summary") finishOperation("Pronto"); }
  }
  function drawTimeline(data) {
    const box = $("#ws-timeline");
    if (!data.buckets.length) { box.innerHTML = '<p class="quiet-empty">Nenhum evento com horário neste recorte.</p>'; return; }
    const max = Math.max(1, ...data.buckets.map(b => b.count)); const n = data.buckets.length, step = 1000 / n;
    box.innerHTML = `<svg viewBox="0 0 1000 145" preserveAspectRatio="none" role="img" aria-label="Volume de eventos e erros ao longo do tempo"><path d="M0 35H1000 M0 75H1000 M0 115H1000" stroke="var(--border)" stroke-width=".7"/>${data.buckets.map((b, i) => `<g data-bucket="${i}" tabindex="0" role="button" aria-label="${esc(fmtTs(b.timestamp))}: ${fmtNum(b.count)} eventos, ${fmtNum(b.errors)} erros. Filtrar intervalo."><rect x="${i * step + 1}" y="${140 - 125 * b.count / max}" width="${Math.max(1, step - 3)}" height="${Math.max(1, 125 * b.count / max)}" rx="1.5" fill="var(--accent)" opacity=".6"><title>${esc(fmtTs(b.timestamp))} · ${fmtNum(b.count)} eventos · ${fmtNum(b.errors)} erros</title></rect><rect x="${i * step + 1}" y="${140 - 125 * b.errors / max}" width="${Math.max(1, step - 3)}" height="${125 * b.errors / max}" rx="1" fill="var(--lv-erro)"/></g>`).join("")}</svg><div class="time-labels"><span>${esc(fmtTs(data.start))}</span><span>${esc(fmtTs(data.end))}</span></div>`;
    box.onkeydown = e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.target.dispatchEvent(new MouseEvent("click", { bubbles: true })); } };
    box.onclick = e => { const g = e.target.closest("[data-bucket]"); if (!g) return; const i = +g.dataset.bucket; applyRange(data.buckets[i].timestamp, data.buckets[i + 1]?.timestamp - 1 || data.end); };
  }
  function applyFilters(filters, replace = false) {
    state.filters = replace ? filters : [...state.filters, ...filters]; state.page = 0; renderChips(); syncCurrentSavedFilter();
    rememberSelection();
    return showPage("explore");
  }
  function applyRange(start, end) {
    applyFilters([...state.filters.filter(f => f.column !== "timestamp"), { column: "timestamp", op: "between", value: String(start), value2: String(end) }], true);
  }
  const timeline = window.createTimelineView({
    content, getOverview, sourceKey,
    scope: () => workspaceScope(),
    filters: () => backendFilters(),
    isActive: () => page === "timeline" || page === "case-timeline" && workspaceScope() === "dataset",
    applyRange,
    onMode: mode => { setAnalysisView(mode === "table" ? "timeline-table" : mode === "horizontal" ? "timeline" : "vtimeline"); if (activeCase()) { activeCase().workspace.analysisView = state.analysisView; activeCase().workspace.timelineMode = mode; } showPage("case-timeline"); },
  });
  function undo() { const previous = history.pop(); if (!previous) return; Object.assign(state, previous); state.page = 0; previousSelection = structuredClone(previous); lastFilters = JSON.stringify(previous); $("#quick-search").value = state.quick; renderChips(); syncCurrentSavedFilter(); refresh(); }
  async function renderCompare() {
    const version = ++serial, contextKey = sourceKey(); loading();
    try {
      const data = await getOverview(); if (version !== serial || contextKey !== sourceKey() || page !== "compare") return;
      if (data.start == null || data.start === data.end) { content.innerHTML = '<div class="quiet-empty">Carregue eventos com horários distintos para comparar períodos.</div>'; return; }
      const middle = data.start + Math.floor((data.end - data.start) / 2) + 1;
      content.innerHTML = `<section class="ws-card"><form id="ws-compare-form" class="compare-form"><div class="compare-period"><label>Período de referência</label><div><input id="ws-before-start" aria-label="Início da referência" type="datetime-local" required value="${localInput(data.start)}"><input id="ws-before-end" aria-label="Fim da referência" type="datetime-local" required value="${localInput(middle - 1)}"></div></div><div class="compare-period"><label>Período de análise</label><div><input id="ws-after-start" aria-label="Início da análise" type="datetime-local" required value="${localInput(middle)}"><input id="ws-after-end" aria-label="Fim da análise" type="datetime-local" required value="${localInput(data.end)}"></div></div><button class="btn primary" type="submit">Comparar</button></form></section><div id="ws-comparison"></div>`;
      $("#ws-compare-form").onsubmit = async e => { e.preventDefault(); await runComparison(); };
      for (const input of content.querySelectorAll('input[type="datetime-local"]')) input.step = "0.001";
      await runComparison();
    } catch (e) { if (version === serial && contextKey === sourceKey() && page === "compare") failed(e); }
  }
  let comparison = null, comparisonPeriods = null;
  async function runComparison() {
    const contextKey = sourceKey(), version = serial;
    const area = $("#ws-comparison"), button = $("#ws-compare-form button");
    const before = { start: +new Date($("#ws-before-start").value), end: +new Date($("#ws-before-end").value) };
    const after = { start: +new Date($("#ws-after-start").value), end: +new Date($("#ws-after-end").value) };
    if (![before.start, before.end, after.start, after.end].every(Number.isFinite) || before.start > before.end || after.start > after.end || (before.start <= after.end && after.start <= before.end)) { area.innerHTML = note("Escolha dois períodos válidos, sem sobreposição."); return; }
    button.disabled = true; area.innerHTML = '<div class="ws-loading"><i class="fas fa-circle-notch spin"></i>Comparando…</div>';
    try {
      const res = await api("compare_periods", { ...analyticsRequest(workspaceScope()), filters: backendFilters().filter(f => f.column !== "timestamp"), before, after });
      if (page !== "compare" || version !== serial || contextKey !== sourceKey()) return; comparison = res; comparisonPeriods = { before, after };
      area.innerHTML = `<div class="metric-grid">${metric("Referência", fmtNum(res.before_total), "eventos")}${metric("Análise", fmtNum(res.after_total), "eventos")}${metric("Erros · referência", pct(res.before_errors / Math.max(1, res.before_total)), fmtNum(res.before_errors) + " erros", "error")}${metric("Erros · análise", pct(res.after_errors / Math.max(1, res.after_total)), fmtNum(res.after_errors) + " erros", "error")}</div>${res.limited ? note("Limite de padrões atingido. Refine os períodos para detalhar as diferenças.") : ""}${!res.before_total || !res.after_total ? note("Um dos períodos não contém eventos. Revise a cobertura antes de interpretar as diferenças.") : ""}<section class="ws-card"><div class="card-heading"><h2>O que mudou</h2><span>Participação no volume de cada período</span></div><table class="ws-table"><thead><tr><th>Padrão</th><th class="num">Referência</th><th class="num">Análise</th><th class="num">Diferença</th></tr></thead><tbody>${res.changes.map((c, i) => `<tr><td><button class="pattern-button" data-action="change" data-index="${i}">${esc(c.pattern)}</button>${!c.before && c.after ? '<div class="pattern-meta">Aparece somente no período de análise</div>' : ""}</td><td class="num">${fmtNum(c.before)}<div class="pattern-meta">${pct(c.before_rate)}</div></td><td class="num">${fmtNum(c.after)}<div class="pattern-meta">${pct(c.after_rate)}</div></td><td class="num ${c.delta > 0 ? "delta-plus" : "delta-minus"}">${c.delta > 0 ? "+" : ""}${(c.delta * 100).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} pp</td></tr>`).join("")}</tbody></table></section>`;
    } catch (e) { if (version === serial && contextKey === sourceKey() && area.isConnected) area.innerHTML = note(String(e)); } finally { button.disabled = false; }
  }
  async function renderSources() {
    const version = ++serial, contextKey = sourceKey();
    loading("Lendo fontes…");
    try {
      const sources = await api("list_sources", {}, { silent: true }); if (page !== "sources" || version !== serial || contextKey !== sourceKey()) return; sourceList = sources; updateCounts();
      content.innerHTML = `<div class="source-controls"><button id="ws-source-add" class="btn primary"><i class="fas fa-plus"></i> Adicionar arquivos</button><button id="ws-source-folder" class="btn ghost">Adicionar pasta</button><button id="ws-source-config" class="btn ghost">Formato e data/hora</button><div class="spacer"></div><button id="ws-source-clear" class="btn ghost danger" title="Descarregar todas as fontes da análise"><i class="fas fa-trash"></i> Limpar análise</button><button id="ws-source-apply" class="btn ghost" ${sourceList.length < 2 ? "disabled" : ""}>Explorar seleção</button></div><section class="ws-card"><table class="ws-table"><thead><tr><th></th><th>Arquivo</th><th>Formato</th><th class="num">Tamanho</th><th class="num">Eventos</th><th>Leitura</th><th class="num">Ação</th></tr></thead><tbody>${sourceList.map((s, i) => `<tr><td><input class="source-check" aria-label="Selecionar ${esc(s.name)}" type="checkbox" data-source-index="${i}" checked></td><td><strong>${esc(s.name)}</strong><div class="pattern-meta" title="${esc(s.path)}">${s.start != null ? `${esc(fmtTs(s.start))} — ${esc(fmtTs(s.end))}` : "Sem horário reconhecido"}</div></td><td><span class="tag">${esc(s.format)}</span></td><td class="num">${s.bytes ? fmtBytes(s.bytes) : "—"}</td><td class="num">${fmtNum(s.count)}</td><td>${s.unparsed ? `<span class="tag">${s.unparsed}/${s.sampled} não interpretados na amostra</span>` : '<span class="tag good">Disponível</span>'}${s.undated ? `<div class="pattern-meta">${fmtNum(s.undated)} sem horário</div>` : ""}</td><td class="num"><button class="icon-btn danger source-remove-btn" title="Remover da análise" data-remove-index="${i}"><i class="fas fa-trash-can"></i></button></td></tr>`).join("")}</tbody></table></section>`;
      const reload = el("button", "btn ghost", "Recarregar fontes");
      $("#ws-source-config").after(reload);
      reload.onclick = async () => { if (state.currentArtifact?.source && await loadData(state.currentArtifact.source)) await showPage("sources"); };
      content.querySelectorAll("tbody tr").forEach((row, i) => {
        const button = el("button", "text-button", "Data/hora");
        button.onclick = async () => { await loadTsConfig(sourceList[i].path); openTsModal(sourceList[i].path); };
        row.cells[1].append(button);
      });
      content.querySelectorAll(".source-remove-btn").forEach(btn => {
        btn.onclick = async () => { await removeSource(+btn.dataset.removeIndex); };
      });
      $("#ws-source-clear").onclick = async () => {
        if (!confirm("Deseja realmente descarregar todas as fontes e limpar a análise atual?")) return;
        await clearAnalysis();
      };
      $("#ws-source-add").onclick = () => openFiles(false, true);
      $("#ws-source-folder").onclick = () => openFiles(true, true);
      $("#ws-source-config").onclick = () => { home.hidden = true; switchView("source"); showSourceMode("load"); };
      $("#ws-source-apply").onclick = () => {
        const selected = [...document.querySelectorAll("[data-source-index]:checked")].map(c => sourceList[+c.dataset.sourceIndex].path).filter(Boolean);
        if (!selected.length) { toast("Selecione ao menos uma fonte.", "info"); return; }
        const all = selected.length === sourceList.length;
        applyFilters([...state.filters.filter(f => f.column !== "caminho"), ...(all ? [] : [{ column: "caminho", op: "regex", value: `^(?:${selected.map(escapeRegex).join("|")})$` }])], true);
      };
    } catch (e) { if (page === "sources" && version === serial && contextKey === sourceKey()) failed(e); }
  }
  async function removeSource(index) {
    const target = sourceList[index];
    if (!target) return;
    if (!confirm(`Remover "${target.name}" da análise?`)) return;
    const currentSource = state.currentArtifact?.source;
    if (!currentSource) return;

    if (currentSource.kind === "bundle") {
      const remainingMembers = [];
      for (const m of currentSource.members) {
        if (m.kind === "file") {
          const allPaths = m.paths?.length ? m.paths : (m.path ? [m.path] : []);
          const remainingPaths = allPaths.filter(p => p !== target.path && p !== target.name);
          if (remainingPaths.length > 0) {
            remainingMembers.push({ ...m, path: remainingPaths[0], paths: remainingPaths });
          }
        } else if (m.kind === "eventlog") {
          if (m.channel !== target.name && m.channel !== target.path) {
            remainingMembers.push(m);
          }
        }
      }
      if (remainingMembers.length === 0) {
        await clearAnalysis();
        return;
      }
      let newSource;
      if (remainingMembers.length === 1 && remainingMembers[0].kind === "file" && (remainingMembers[0].paths?.length || 1) === 1) {
        const single = remainingMembers[0];
        newSource = { kind: "file", path: single.path, paths: [single.path], format: single.format || "auto" };
      } else {
        newSource = { kind: "bundle", members: remainingMembers, path: remainingMembers[0].path || remainingMembers[0].channel };
      }
      if (await loadData(newSource)) {
        toast(`Fonte "${target.name}" removida da análise.`, "ok");
        await showPage("sources");
      }
    } else if (currentSource.kind === "file") {
      const allPaths = currentSource.paths?.length ? currentSource.paths : (currentSource.path ? [currentSource.path] : []);
      const remainingPaths = allPaths.filter(p => p !== target.path && p !== target.name);
      if (remainingPaths.length === 0) {
        await clearAnalysis();
        return;
      }
      const newSource = { kind: "file", path: remainingPaths[0], paths: remainingPaths, format: currentSource.format || "auto" };
      if (await loadData(newSource)) {
        toast(`Fonte "${target.name}" removida da análise.`, "ok");
        await showPage("sources");
      }
    } else {
      await clearAnalysis();
    }
  }
  async function clearAnalysis() {
    state.loaded = false;
    state.currentArtifact = null;
    state.columns = [];
    state.visibleCols = [];
    state.filters = [];
    state.quick = "";
    state.rows = [];
    state.total = 0;
    sourceList = [];
    sessionStorage.removeItem("loginsight_current_artifact");
    applySourceSpec({ kind: "file", path: "", paths: [], format: "auto" });
    toast("Análise limpa.", "info");
    await showPage("summary");
  }
  function escapeRegex(text) { return String(text).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }
  async function openFiles(folder = false, merge = state.loaded, dropped = null) {
    if (importing) return; importing = true;
    try {
      let chosen = dropped || await dialogApi.open({ multiple: !folder, directory: folder });
      if (!chosen) return;
      let paths = Array.isArray(chosen) ? chosen : [chosen];
      paths = await api("expand_paths", { paths });
      if (await loadData({ kind: "file", paths, path: paths[0], format: "auto" }, { merge })) await showPage("summary");
    } catch (e) { if (!String(e).includes("cancelada")) toast(String(e), "err"); }
    finally { importing = false; }
  }
  async function loaded() {
    cacheKey = ""; overview = null;
    history.length = 0; previousSelection = { filters: structuredClone(state.filters), quick: state.quick }; lastFilters = JSON.stringify(previousSelection);
    try { sourceList = await api("list_sources", {}, { silent: true }); } catch { sourceList = []; }
    updateCounts();
  }
  async function saveFinding(finding) {
    if (workspaceScope() === "case") { toast("Estes registros já pertencem ao Caso.", "info"); return; }
    const c = ensureCase();
    const row = finding.event_id != null ? await api("event_detail", { id: finding.event_id }) : null;
    const filters = [...backendFilters().filter(f => f.column !== "timestamp")];
    if (finding.start != null && finding.end != null) filters.push({ column: "timestamp", op: "between", value: String(finding.start), value2: String(finding.end) });
    c.items.push({ id: nid(), kind: "grupo", label: finding.title, note: finding.detail, createdAt: Date.now(), rows: row ? [row] : [], sourceFilters: filters, sourceSpec: structuredClone(state.currentArtifact?.source), foundCount: null, includedCount: row ? 1 : 0, tags: [], relevance: "normal", origin: state.currentOrigin, artifactId: state.currentArtifact?.id, stationId: null });
    if (await saveCases()) { updateCounts(); toast("Evidência salva.", "ok"); }
  }
  async function saveEvent(ev) {
    if (!ev) return;
    if (workspaceScope() === "case") { toast("Este registro já pertence ao Caso.", "info"); return; }
    const c = ensureCase(); const key = caseRecordKey(ev, state.currentArtifact?.id, state.currentOrigin);
    if (c.items.some(it => it.rows?.some(r => caseRecordKey(r, it.artifactId, it.origin) === key))) { toast("Este evento já está nas evidências.", "info"); return; }
    c.items.push({ id: nid(), kind: "evento", label: ev.message.split("\n")[0].slice(0, 140), note: "", createdAt: Date.now(), rows: [structuredClone(ev)], sourceFilters: backendFilters(), sourceSpec: structuredClone(state.currentArtifact?.source), foundCount: 1, includedCount: 1, tags: [], relevance: "normal", origin: state.currentOrigin, artifactId: state.currentArtifact?.id, stationId: null });
    if (await saveCases()) { updateCounts(); window.WorkspaceContext?.refreshMembership(); toast("Evento salvo.", "ok"); }
  }
  let evidencePage = 0, evidenceSearch = "", evidenceCaseId = null;
  function renderEvidence() {
    const c = activeCase(), items = c?.items || []; updateCounts();
    if (evidenceCaseId !== c?.id) { evidenceCaseId = c?.id; evidencePage = 0; evidenceSearch = ""; }
    const query = evidenceSearch.toLocaleLowerCase("pt-BR"), selected = items.map((item,index)=>({item,index})).filter(({item})=>!query||[item.label,...Object.values(CaseContent.narrative(item))].join(" ").toLocaleLowerCase("pt-BR").includes(query));
    const pages = Math.max(1, Math.ceil(selected.length / 40)); evidencePage = Math.min(evidencePage, pages-1);
    const visible = selected.slice(evidencePage*40,evidencePage*40+40);
    content.innerHTML = `<div class="source-controls"><button class="btn primary" id="ws-evidence-pdf"><i class="fas fa-file-pdf"></i> Relatório PDF</button><button class="btn ghost" id="ws-evidence-trails"><i class="fas fa-route"></i> Trilhas</button><button class="btn ghost" id="ws-evidence-advanced">Linha do tempo</button><button class="btn ghost" id="ws-evidence-export" title="Exportar a investigação com seus textos e imagens"><i class="fas fa-arrow-up-from-bracket"></i> Exportar</button></div><div class="evidence-list-controls"><input id="ws-evidence-search" type="search" placeholder="Buscar nos itens…" aria-label="Buscar nos itens do Caso" value="${esc(evidenceSearch)}"><span class="muted small">${fmtNum(selected.length)} itens</span><div class="spacer"></div><button class="icon-btn" id="ws-evidence-prev" aria-label="Página anterior" ${evidencePage===0?'disabled':''}><i class="fas fa-chevron-left"></i></button><span class="muted small">${evidencePage+1} / ${pages}</span><button class="icon-btn" id="ws-evidence-next" aria-label="Próxima página" ${evidencePage+1===pages?'disabled':''}><i class="fas fa-chevron-right"></i></button></div>${visible.length ? visible.map(({item:it,index:i}) => { const narrative=CaseContent.narrative(it);return `<article class="ws-card evidence-card" data-item-index="${i}"><div class="evidence-top"><div><h2>${esc(it.label || "Item do Caso")}</h2><span class="subtle">${esc(fmtTs(it.createdAt))}${it.rows?.length ? ` · ${fmtNum(it.rows.length)} registros preservados` : " · Recorte salvo"}</span></div>${iconButton("remove-evidence", "fa-trash-can", "Remover item do Caso", i)}</div>${narrative.summary?`<p class="case-narrative-summary">${esc(narrative.summary)}</p>`:''}${narrative.details?`<details class="case-item-details"><summary>Detalhes</summary><p>${esc(narrative.details)}</p></details>`:''}<div data-item-images></div><div class="evidence-actions"><button class="btn ghost small" data-action="explain-evidence" data-index="${i}"><i class="fas fa-pen-to-square"></i> Explicação e imagens</button>${it.rows?.length ? `<button class="btn ghost small" data-action="evidence-event" data-index="${i}">${it.rows.length>1?'Primeiro registro':'Ver registro'}</button>` : ""}<button class="btn ghost small" data-action="reopen-evidence" data-index="${i}">Reabrir recorte</button>${it.relevance&&it.relevance!=='normal'?`<span class="tag">${esc(it.relevance)}</span>`:''}</div></article>`; }).join("") : '<section class="ws-card"><p class="quiet-empty">Nenhum item encontrado. Salve registros da Análise para reuni-los no Caso.</p></section>'}`;
    $("#ws-evidence-pdf").onclick = () => CaseReport.open();
    $("#ws-evidence-trails").onclick = () => showPage("case-trails");
    $("#ws-evidence-export").onclick = () => { openExport(); $("#ws-export-kind").value = "case"; $("#ws-export-kind").dispatchEvent(new Event("change")); };
    $("#ws-evidence-advanced").onclick = () => showPage("case-timeline");
    $("#ws-evidence-prev").onclick = () => { evidencePage--; renderEvidence(); };
    $("#ws-evidence-next").onclick = () => { evidencePage++; renderEvidence(); };
    $("#ws-evidence-search").oninput = event => { evidenceSearch=event.target.value;evidencePage=0;renderEvidence();const search=$("#ws-evidence-search");search.focus(); };
    for(const card of content.querySelectorAll("[data-item-index]")){
      const index = +card.dataset.itemIndex;
      const item = items[index];
      if(item?.attachments?.length)CaseContent.mountAttachments(card.querySelector("[data-item-images]"),item);
      card.oncontextmenu = (e) => {
        e.preventDefault();
        showCtxMenu(e.clientX, e.clientY, [
          {
            icon: "fa-trash-can",
            label: "Remover item do Caso",
            danger: true,
            onClick: async () => {
              removedEvidence = { item, caseId: c.id, index };
              c.items.splice(index, 1);
              await saveCases();
              renderEvidence();
              toast("Item removido do Caso.", "ok");
            }
          },
          { sep: true },
          {
            icon: "fa-pen-to-square",
            label: "Explicação e imagens",
            onClick: () => CaseContent.editItem(item.id)
          },
          ...(item.rows?.length ? [{
            icon: "fa-eye",
            label: item.rows.length > 1 ? "Primeiro registro" : "Ver registro",
            onClick: () => showDetail(item.rows[0], item.sourceSpec)
          }] : []),
          {
            icon: "fa-rotate-left",
            label: "Reabrir recorte",
            onClick: async () => {
              await window.WorkspaceContext?.setScope("dataset", { animate: false });
              if (item.sourceSpec && JSON.stringify(item.sourceSpec) !== JSON.stringify(state.currentArtifact?.source) && !await loadData(item.sourceSpec)) return;
              if (!state.loaded) { toast("Abra a fonte original para reexecutar este recorte.", "info"); return; }
              applyFilters(structuredClone(item.sourceFilters || []), true);
            }
          }
        ]);
      };
    }
    if (removedEvidence?.caseId === c?.id) {
      const restore = el("button", "btn ghost", "Desfazer remoção");
      $("#ws-evidence-advanced").after(restore);
      restore.onclick = async () => { c.items.splice(removedEvidence.index, 0, removedEvidence.item); removedEvidence = null; await saveCases(); renderEvidence(); };
    }
    const importButton = el("button", "btn ghost", "Importar investigação");
    $("#ws-evidence-advanced").after(importButton);
    importButton.onclick = async () => {
      try {
        const path = await dialogApi.open({ multiple: false, filters: [{ name: "Investigação", extensions: ["json"] }] });
        if (!path) return;
        const data = normalizeCaseStore(await api("import_investigation", { path }));
        for (const c of data.cases) { if (state.cases.cases.some(saved => saved.id === c.id)) c.id = nid(); state.cases.cases.push(c); }
        if (await saveCases()) { renderCaseBar(); toast("Investigação importada. Selecione-a no menu de casos.", "ok"); }
      } catch (error) { toast(String(error), "err"); }
    };
  }
  function report() {
    const c = activeCase();
    return `# ${c?.name || "Investigação"}\n\n${(c?.items || []).map(it => `## ${it.label}\n\n${Object.values(CaseContent.narrative(it)).filter(Boolean).join("\n\n")}\n\n${it.rows?.length || 0} eventos preservados.\n\n${(it.rows || []).map(e => `- ${fmtTsFull(e.timestamp)} · ${e.source} · ${e.message.replaceAll("\n", " ")}\n  Referência: ${e.event_ref || e.id}`).join("\n")}\n\nFiltros: ${JSON.stringify(it.sourceFilters || [])}`).join("\n\n")}`;
  }
  function openExport() { $("#ws-export-modal").hidden = false; $("#ws-export-kind").focus(); $("#ws-export-kind").dispatchEvent(new Event("change")); $("#ws-export-scope").textContent = `${fmtNum(state.total)} eventos no recorte atual. A exportação de eventos inclui todos os resultados.`; }
  async function exportFile() {
    const kind = $("#ws-export-kind").value, mask = $("#ws-mask").checked;
    if (kind === "case-pdf") { $("#ws-export-modal").hidden = true; window.CaseReport.open(); return; }
    const extension = kind === "report" ? "md" : kind === "case" ? "json" : kind;
    const path = await dialogApi.save({ defaultPath: `loginsight.${extension}`, filters: [{ name: extension.toUpperCase(), extensions: [extension] }] });
    if (!path) return; const button = $("#ws-export-save"), restore = btnBusy(button, "Exportando…");
    try {
      if (kind === "report" || kind === "case") {
        const data = kind === "report" ? report() : { schemaVersion: 2, active: activeCase()?.id, cases: activeCase() ? [activeCase()] : [] };
        if (kind === "case") await api("export_investigation", { path, data, mask });
        else await api("export_document", { path, content: mask ? redactValue(data) : data });
      } else { await api("export_events", { path, format: kind, ...analyticsRequest(workspaceScope()), mask }); }
      toast("Arquivo exportado.", "ok"); $("#ws-export-modal").hidden = true;
    } catch {} finally { restore(); }
  }
  function redactValue(value) {
    if (typeof value === "string") return value.replace(/(password|passwd|token|secret|authorization|api[_-]?key)(["']?\s*[:=]\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:Bearer|Basic)\s+[^\s",;]+|[^\s",;]+)/gi, '$1$2"[oculto]"');
    if (Array.isArray(value)) return value.map(redactValue);
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, val]) => [key, /^(password|passwd|token|secret|authorization|apikey|accesstoken|refreshtoken)$/i.test(key.replace(/[_-]/g, "")) ? "[oculto]" : redactValue(val)]));
    return value;
  }
  content.addEventListener("click", async e => {
    const source = e.target.closest("[data-source]"); if (source) { applyFilters([{ column: "source", op: "equals_exact", value: source.dataset.source }]); return; }
    const b = e.target.closest("[data-action]"); if (!b) return;
    const i = +b.dataset.index, action = b.dataset.action;
    if (["sources", "compare", "explore", "timeline"].includes(action)) { await showPage(action); return; }
    if (action === "retry") { await showPage(page); return; }
    if (action === "errors") { applyFilters([{ column: "level", op: "regex", value: "^(Erro|Crítico)$" }]); return; }
    if (action === "pattern") { applyFilters([{ column: "message", op: "pattern", value: overview.patterns[i].pattern }]); return; }
    if (action === "finding") { const f = overview.findings[i]; if (f.kind === "quality") openTsModal(); else if (f.kind === "pattern") { await showPage("explore"); await openDetail(f.event_id); } else if (f.start != null) applyRange(f.start, f.end); return; }
    if (action === "save-finding") { await saveFinding(overview.findings[i]); return; }
    if (action === "change") { const c = comparison.changes[i], period = c.after ? comparisonPeriods.after : comparisonPeriods.before; applyFilters([...state.filters.filter(f => f.column !== "timestamp"), { column: "message", op: "pattern", value: c.pattern }, { column: "timestamp", op: "between", value: String(period.start), value2: String(period.end) }], true); return; }
    const item = activeCase()?.items?.[i]; if (!item) return;
    if (action === "remove-evidence") { removedEvidence = { item, caseId: activeCase().id, index: i }; activeCase().items.splice(i, 1); await saveCases(); renderEvidence(); }
    if (action === "evidence-event") { showDetail(item.rows[0], item.sourceSpec); }
    if (action === "explain-evidence") { await CaseContent.editItem(item.id); }
    if (action === "reopen-evidence") {
      await window.WorkspaceContext?.setScope("dataset", { animate: false });
      if (item.sourceSpec && JSON.stringify(item.sourceSpec) !== JSON.stringify(state.currentArtifact?.source) && !await loadData(item.sourceSpec)) return;
      if (!state.loaded) { toast("Abra a fonte original para reexecutar este recorte.", "info"); return; }
      applyFilters(structuredClone(item.sourceFilters || []), true);
    }
  });
  document.querySelectorAll(".workspace-nav [data-page]").forEach(b => b.onclick = () => showPage(b.dataset.page));
  $(".workspace-brand").onclick = e => { e.preventDefault(); showPage("summary"); };
  $("#ws-open").onclick = () => openFiles(false, state.loaded);
  $("#case-timeline-volume").onclick = () => { if (activeCase()) activeCase().workspace.timelineMode = "volume"; saveCases(); showPage("timeline"); };
  $("#ws-empty-open").onclick = () => openFiles(); $("#ws-folder").onclick = () => openFiles(true);
  $("#ws-windows").onclick = () => { home.hidden = true; switchView("source"); showSourceMode("load"); setSource("eventlog"); };
  $("#ws-preferences").onclick = () => openSettings();
  $("#ws-reload").onclick = async () => { cacheKey = ""; timeline.invalidate(); await showPage(page); };
  $("#ws-clear-scope").onclick = () => { state.filters = []; state.quick = ""; $("#quick-search").value = ""; state.page = 0; renderChips(); syncCurrentSavedFilter(); refresh().then(() => showPage(page)); };
  $("#ws-export").onclick = openExport; $("#ws-export-close").onclick = () => { $("#ws-export-modal").hidden = true; }; $("#ws-export-save").onclick = exportFile;
  $("#ws-export-kind").onchange = () => { const wholeCase=["case", "case-pdf", "report"].includes($("#ws-export-kind").value);$("#ws-export-scope").textContent=wholeCase?`Caso completo · ${fmtNum(activeCase()?.items?.length||0)} itens. Imagens acompanham a investigação JSON e o relatório PDF.`:`${fmtNum(state.total)} registros no recorte atual.`;$("#ws-mask").closest("label").hidden=$("#ws-export-kind").value==="case-pdf"; };
  $("#ws-export-modal").onclick = e => { if (e.target.id === "ws-export-modal") e.target.hidden = true; };
  $("#btn-load").onclick = async () => { await loadData(); if (state.loaded) { await loaded(); await showPage("summary"); } };
  $("#btn-merge").onclick = async () => { await loadData(null, { merge: true }); if (state.loaded) { await loaded(); await showPage("summary"); } };
  $("#workbar-cancel").onclick = async () => { await api("cancel_operation", {}, { silent: true }); state.refreshVersion++; serial++; finishOperation("Operação cancelada"); };
  $("#tabbtn-group").innerHTML = '<i class="fas fa-layer-group"></i> Agrupar';
  $("#tabbtn-dashboard").innerHTML = '<i class="fas fa-chart-line"></i> Gráficos';
  $("#tabbtn-cube").innerHTML = '<i class="fas fa-table-cells"></i> Tabela dinâmica';
  $("#btn-back-drive").onclick = () => showPage("sources"); $("#btn-back-drive").innerHTML = '<i class="fas fa-layer-group"></i> Fontes';
  const tools = el("div", "explore-tools"); tools.innerHTML = '<button class="icon-btn" id="ws-undo" title="Desfazer filtro (Alt + ←)" aria-label="Desfazer filtro"><i class="fas fa-rotate-left"></i></button><button class="icon-btn" id="ws-wrap" title="Quebrar linhas" aria-label="Quebrar linhas"><i class="fas fa-align-left"></i></button><button class="icon-btn" id="ws-density" title="Alternar densidade" aria-label="Alternar densidade"><i class="fas fa-grip-lines"></i></button>';
  $(".viewbar").appendChild(tools); $("#ws-undo").onclick = undo;
  $("#ws-wrap").onclick = () => { document.body.dataset.wrap = document.body.dataset.wrap !== "true"; };
  $("#ws-density").onclick = () => { const d = document.body.dataset.density === "compact" ? "comfortable" : "compact"; document.body.dataset.density = d; localStorage.setItem("workspace.density", d); };
  document.body.dataset.density = localStorage.getItem("workspace.density") || "comfortable";
  try { state.favoriteFields = JSON.parse(localStorage.getItem("workspace.fields") || "[]"); } catch { state.favoriteFields = []; }
  const detailActions = el("div", "detail-quick-actions"); detailActions.innerHTML = '<button class="btn ghost small" id="ws-detail-save"><i class="fas fa-bookmark"></i> Salvar</button><button class="btn ghost small" id="ws-detail-context">Ver contexto</button><button class="btn ghost small" id="ws-detail-follow">Seguir requisição</button>';
  $(".drawer-tabs").after(detailActions);
  $("#ws-detail-save").onclick = () => saveEvent(state.currentDetailEv);
  async function restoreDetailSource() {
    const source = state.detailSourceSpec;
    return !source || JSON.stringify(source) === JSON.stringify(state.currentArtifact?.source) || await loadData(source);
  }
  $("#ws-detail-context").onclick = async () => { if (!state.currentDetailEv) return; const ev = state.currentDetailEv; if (!await restoreDetailSource()) return; if (ev.timestamp != null) applyRange(ev.timestamp - 120000, ev.timestamp + 120000); else toast("Este evento não tem horário reconhecido.", "info"); };
  $("#ws-detail-follow").onclick = async () => {
    const ev = state.currentDetailEv; if (!ev) return;
    const field = ["trace_id", "trace.id", "request_id", "requestId", "correlation_id", "session_id"].find(k => ev.fields?.[k]);
    if (!field) { toast("Nenhum identificador de requisição neste evento.", "info"); return; }
    if (!await restoreDetailSource()) return;
    applyFilters([{ column: field, op: "equals_exact", value: String(ev.fields[field]) }], true);
  };
  document.addEventListener("keydown", e => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "o") { e.preventDefault(); openFiles(); }
    if (e.altKey && e.key === "ArrowLeft") { e.preventDefault(); undo(); }
    if (e.key === "Tab") {
      const modal = [...document.querySelectorAll(".modal-overlay:not([hidden])")].at(-1);
      const controls = modal ? [...modal.querySelectorAll("button,input,select,textarea,a[href],[tabindex='0']")].filter(n => !n.disabled && n.getClientRects().length) : [];
      if (controls.length) { const first=controls[0], last=controls.at(-1); if (e.shiftKey && (document.activeElement===first || !modal.contains(document.activeElement))) {e.preventDefault();last.focus();} else if (!e.shiftKey && (document.activeElement===last || !modal.contains(document.activeElement))) {e.preventDefault();first.focus();} }
    }
    if (e.key === "Escape") { $("#ws-export-modal").hidden = true; $("#ws-drop").hidden = true; }
  });
  let dragDepth = 0;
  document.addEventListener("dragenter", e => { if (e.dataTransfer?.types?.includes("Files")) { e.preventDefault(); dragDepth++; $("#ws-drop").hidden = false; } });
  document.addEventListener("dragover", e => { if (e.dataTransfer?.types?.includes("Files")) e.preventDefault(); });
  document.addEventListener("dragleave", () => { if (--dragDepth <= 0) $("#ws-drop").hidden = true; });
  document.addEventListener("drop", e => { e.preventDefault(); dragDepth = 0; $("#ws-drop").hidden = true; });
  window.__TAURI__.event?.listen("tauri://drag-drop", ({ payload }) => { $("#ws-drop").hidden = true; if (payload?.paths?.length) openFiles(false, state.loaded, payload.paths); }).catch(() => {});
  window.Workspace = {
    loaded, showPage, saveEvent,
    openItem: itemId => { const item=activeCase()?.items?.find(item=>item.id===itemId);if(item?.rows?.length)showDetail(item.rows[0],item.sourceSpec); },
    page: () => page,
    capture: () => ({ history: structuredClone(history), previousSelection: structuredClone(previousSelection), lastFilters, timeline: timeline.capture() }),
    restore: snapshot => { serial++; cacheKey = ""; overview = null; pendingOverview = null; history.splice(0, history.length, ...(snapshot?.history || [])); previousSelection = snapshot?.previousSelection || { filters: structuredClone(state.filters), quick: state.quick }; lastFilters = snapshot?.lastFilters || JSON.stringify(previousSelection); timeline.restore(snapshot?.timeline); },
    onFiltersChanged: () => { lastExploredKey = ""; if (["summary", "timeline", "case-timeline", "compare", "journeys"].includes(page)) showPage(page); },
    onView(which) {
      if (which === "workspace") return;
      home.hidden = true;
      markPage(which === "viz" || which === "trail" ? "explore" : which === "source" ? "sources" : which === "caso" && ["timeline", "vtimeline", "timeline-table"].includes(state.analysisView) ? "case-timeline" : "evidence");
    },
    onRefresh() {
      rememberSelection();
      lastExploredKey = sourceKey();
      updateCounts();
      if (page === "journeys") window.Journeys?.refresh();
    },
  };
  // Existing workspaces restore asynchronously; an empty launch remains immediately useful.
  showPage("summary");
})();
