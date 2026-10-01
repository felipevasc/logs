/* Work in progress, per menu and tab. Loading continues when the user navigates away; returning to a
   tab joins the same request. The status bar summarizes what is loading and lists it for cancelling. */
window.Tasks = (() => {
  "use strict";
  const base = api;
  // Commands that only read: identical requests in flight are shared and survive a selective cancel.
  const READS = new Set(["threat_catalog", "threat_scan", "threat_events", "journey_fields", "journey_index", "journey_events", "dataset_overview", "triage", "triage_evidence_event", "event_insights", "detection_rules", "timeline_lanes", "entity_summary", "ioc_sightings", "source_hashes", "timeline_range", "compare_periods", "list_sources", "source_summary", "source_snapshot", "query_page", "query_events", "explore_snapshot", "aggregate_events", "trail_events", "count_filtered", "tree_aggs", "stats_events", "event_detail", "profile_fields", "discover_patterns", "compute_series", "pivot", "list_channels", "list_formats", "list_derived_fields", "get_codes", "get_codes_path", "system_codes_count", "remote_list", "get_ts_config"]);
  // Bookkeeping calls never show as work.
  const QUIET = new Set(["validate_filters", "case_sync", "ui_zoom", "cancel_operation", "cancel_task", "engine_status", "cases_save", "cases_load", "mcp_status", "get_codes_path", "system_codes_count", "list_formats"]);
  const WHAT = { dataset_overview: "Visão geral", triage: "Comprometimentos", triage_evidence_event: "Evento da evidência", timeline_range: "Volume no tempo", timeline_lanes: "Faixas", query_page: "Registros", engine_retry: "Preparando índices", query_events: "Registros", explore_snapshot: "Registros e campos", tree_aggs: "Campos", aggregate_events: "Grupos", profile_fields: "Perfil dos campos", compute_series: "Gráficos", pivot: "Tabela dinâmica", discover_patterns: "Padrões", threat_scan: "Ameaças", threat_events: "Registros de ameaça", compare_periods: "Comparação de períodos", journey_index: "Possíveis trilhas", journey_events: "Registros da trilha", journey_fields: "Campos de ligação", entity_summary: "Entidades", ioc_sightings: "Indicadores nas fontes", source_hashes: "SHA-256 das fontes", stats_events: "Histograma", count_filtered: "Contagem", trail_events: "Vizinhança do registro", load_file: "Abrindo logs", load_files: "Abrindo logs", load_bundle: "Abrindo logs", load_event_log: "Lendo o Event Log", remote_import: "Importação remota", remote_test: "Teste de conexão", export_events: "Exportação", export_investigation: "Exportação", import_investigation: "Importação da investigação", harvest_codes: "Catálogo do sistema", sigma_import: "Importação Sigma", expand_paths: "Lendo pastas", event_detail: "Detalhe do registro", list_sources: "Fontes", event_insights: "Detalhe do registro" };
  const ZONES = { analysis: "Análise", case: "Caso", structure: "Estrutura" };
  const PAGES = { summary: "Resumo", compromises: "Comprometimentos", evidence: "Evidências", sources: "Arquivos", connections: "Conexões", import: "Abrir logs", "case-timeline": "Linha do tempo", "case-trails": "Trilhas", journeys: "Possíveis trilhas" };
  const TABS = { table: "Registros", group: "Resumir", dashboard: "Descobrir", cube: "Cruzar dados" };
  const VISIBLE_AFTER = 300;
  const tasks = new Map(), inflight = new Map(), ids = new WeakMap();
  let serial = 0, arrays = 0, dialog = null, ticker = null;
  const named = new Set(["set_ts_config", "remote_import", "remote_test", "load_bundle", "journey_fields", "journey_index", "journey_events", "discover_patterns", "timeline_range", "query_page", "query_events", "explore_snapshot", "count_filtered", "stats_events", "tree_aggs", "aggregate_events", "compute_series", "pivot", "load_file", "load_files", "load_event_log", "engine_retry"]);
  const sourceMutations = new Set(["load_file", "load_files", "load_bundle", "load_event_log", "clear_events"]);
  named.add("clear_events");
  const latest = new Map();
  const background = window.PerformanceTools.queue(1);

  // Where the work was asked from: the menu item, its tab and its area.
  function origin() {
    const page = document.body.dataset.page || "summary", zone = document.documentElement.dataset.zone || "analysis";
    const marks = [`.zone-switch [data-zone="${zone}"]`];
    let label, key;
    if (page === "explore") {
      const tab = state.activeDatasetTab || "table";
      marks.push('.nav-pages [data-page="explore"]', `#tabbtn-${tab}`);
      label = `Explorar · ${TABS[tab] || tab}`; key = `explore:${tab}`;
      if (tab === "dashboard" && window.Discovery?.mode) { const mode = window.Discovery.mode(); marks.push(`.discovery-mode[data-mode="${mode}"]`); label += ` · ${window.Discovery.modeLabel()}`; key += `:${mode}`; }
    } else if (["timeline", "case-timeline", "case-trails", "journeys"].includes(page)) {
      const sub = page === "timeline" ? "case-timeline" : page;
      marks.push('.nav-pages [data-page="case-timeline"]', `#page-tabs [data-subpage="${sub}"]`);
      label = PAGES[sub]; key = `timeline:${sub}`;
    } else { marks.push(`.nav-pages [data-page="${page}"]`); label = PAGES[page] || page; key = page; }
    return { key: `${zone}:${key}`, label: zone === "analysis" ? label : `${label} (${ZONES[zone]})`, marks };
  }
  const signature = (cmd, args) => cmd + JSON.stringify(args, (k, v) => {
    if (Array.isArray(v) && v.length > 64) { if (!ids.has(v)) ids.set(v, ++arrays); return `#${ids.get(v)}:${v.length}`; }
    return v;
  });

  async function run(entry) {
    const execute = async () => {
      if (entry.cancelled) throw new Error("Operação cancelada.");
      entry.status = "running";
      return base(entry.cmd, entry.args, { ...entry.opts, silent: true, cancelled: () => entry.cancelled });
    };
    return entry.opts.background ? background.add(execute, () => !entry.cancelled) : execute();
  }
  function start(cmd, args, opts) {
    const from = origin(), id = ++serial, operationId = named.has(cmd) ? `ui-${Date.now()}-${id}` : null;
    const entry = { id, cmd, args: operationId ? { ...args, operationId } : args, operationId, opts, read: READS.has(cmd), cancelled: false, owners: new Set(opts.latest ? [opts.latest] : []), keepAlive: !opts.latest, status: opts.background ? "queued" : "running", started: performance.now(), ...from };
    tasks.set(entry.id, entry);
    entry.promise = run(entry).then(result => {
      // A successful source mutation already crossed the native commit boundary.
      // Cancellation may have arrived too late; retain the committed result so the
      // source intent guard can reconcile it instead of pretending it rolled back.
      if (entry.cancelled && !sourceMutations.has(entry.cmd)) throw new Error("Operação cancelada.");
      return result;
    }, error => { if (!entry.cancelled && !opts.silent) toast(String(error), "err"); throw error; })
      .finally(() => { tasks.delete(entry.id); schedule(); if (inflight.get(entry.key2) === entry) inflight.delete(entry.key2); for (const [key, owner] of latest) if (owner === entry) latest.delete(key); });
    if (opts.latest) latest.set(opts.latest, entry);
    setTimeout(schedule, VISIBLE_AFTER + 10);
    return entry;
  }
  api = function(cmd, args = {}, opts = {}) {
    if (QUIET.has(cmd)) return base(cmd, args, opts);
    // Dataset identity belongs to the signature; identical arguments on a replacement source are different work.
    const context = JSON.stringify([state.datasetRevision, state.currentArtifact?.id, state.currentArtifact?.loadedAt, state.cases?.active, state.derivedFields]);
    const key = READS.has(cmd) ? context + signature(cmd, args) : null, shared = key && inflight.get(key);
    if (shared && !shared.cancelled) { if (opts.latest) { if (latest.get(opts.latest) !== shared) cancelLatest(opts.latest); latest.set(opts.latest, shared); shared.owners.add(opts.latest); } else shared.keepAlive = true; return shared.promise; }
    if (opts.latest) cancelLatest(opts.latest);
    const entry = start(cmd, args, opts); entry.key2 = key;
    if (entry.read) inflight.set(key, entry);
    return entry.promise;
  };

  const visible = () => [...tasks.values()].filter(t => performance.now() - t.started >= VISIBLE_AFTER);
  function groups() {
    const map = new Map();
    for (const t of visible()) { const g = map.get(t.key) || map.set(t.key, { key: t.key, label: t.label, marks: t.marks, tasks: [] }).get(t.key); g.tasks.push(t); }
    return [...map.values()];
  }
  function summary(list) {
    const names = list.map(g => g.label);
    if (names.length === 1) return `Carregando ${names[0]}`;
    if (names.length === 2) return `Carregando ${names[0]} e ${names[1]}`;
    return `Carregando ${names[0]}, ${names[1]} e mais ${names.length - 2}`;
  }
  let pending = false;
  function schedule() { if (pending) return; pending = true; requestAnimationFrame(() => { pending = false; render(); }); }
  function render() {
    const list = groups();
    const marked = new Set(list.flatMap(g => g.marks.flatMap(selector => [...document.querySelectorAll(selector)])));
    // One compact spinner per busy control; preserve any pre-existing busy state.
    document.querySelectorAll('[data-loading="task"]').forEach(node => { if (!marked.has(node)) { node.removeAttribute("data-loading"); if(node.__taskBusyBefore == null)node.removeAttribute("aria-busy");else node.setAttribute("aria-busy",node.__taskBusyBefore); delete node.__taskBusyBefore; node.querySelector(":scope > .li-loader")?.remove(); } });
    marked.forEach(node => { if(node.dataset.loading!=="task")node.__taskBusyBefore=node.getAttribute("aria-busy"); node.setAttribute("aria-busy","true"); node.setAttribute("data-loading", "task"); if (!node.querySelector(":scope > .li-loader")) node.append(loader()); });
    const bar = $("#workbar"), button = $("#workbar-tasks");
    bar.classList.toggle("has-tasks", list.length > 0);
    if (list.length) { button.querySelector("span").textContent = summary(list); button.title = "Ver e cancelar o que está carregando"; }
    const selectedId = state.loadOverlay
      ? latest.get(state.loadOverlayProgressKey || "source-load")?.operationId
      : state.progressOperationId;
    const selected = selectedId ? [...tasks.values()].find(t => t.operationId === selectedId) : null;
    if (state.progressOperationId && !selected && !state.loadOverlay) {
      state.progressOperationId = null;
      setWorkbar(tasks.size ? "Tarefas em andamento" : "Nenhuma tarefa em andamento", "", null, false);
    }
    if (selected) {
      const timing = `${detail(selected)} · ${elapsed(selected)} decorridos`;
      $("#workbar-detail").textContent = timing;
      if (state.loadOverlay) $("#load-eta").textContent = timing;
    }
    if (dialog) drawDialog();
    // Menus and tabs are redrawn by their pages; marks are refreshed while work goes on.
    clearInterval(ticker); ticker = list.length || tasks.size ? setInterval(schedule, 500) : null;
  }

  function cancel(entry) {
    if (!entry || entry.cancelled) return;
    entry.cancelled = true; entry.status = "cancelling";
    if (inflight.get(entry.key2) === entry) inflight.delete(entry.key2);
    // Never cancel unrelated engine work. Keep the row until the invocation settles.
    if (entry.operationId) base("cancel_task", { operationId: entry.operationId }, { silent: true }).catch(error => { entry.cancelError = String(error); schedule(); });
    schedule();
  }
  function cancelLatest(key) { const entry = latest.get(key); latest.delete(key); if (!entry) return; entry.owners.delete(key); if (!entry.keepAlive && !entry.owners.size) cancel(entry); }
  function cancelAll() { for (const task of tasks.values()) cancel(task); inflight.clear(); latest.clear(); schedule(); }
  function progress(payload) {
    if (!payload?.operationId) return null;
    const entry = [...tasks.values()].find(t => t.operationId === payload.operationId);
    if (!entry || entry.cancelled) return null;
    entry.progress = payload;
    entry.estimate = window.PerformanceTools.estimate(entry.estimate, payload);
    schedule(); return entry;
  }
  function detail(t) {
    const p = t.progress, estimate = t.estimate;
    if (t.cancelError) return `Falha ao cancelar: ${t.cancelError}`;
    if (t.cancelled) return "Cancelando · aguardando confirmação";
    if (t.status === "queued") return "Na fila";
    if (!p) return "Em execução";
    const items = [p.phase || "Em execução"];
    if (p.total > 0) items.push(`${fmtNum(p.completed)} / ${fmtNum(p.total)} ${p.unit || "itens"}`);
    else if (p.completed > 0) items.push(`${fmtNum(p.completed)} ${p.unit || "itens"}`);
    const phaseSeconds = window.PerformanceTools.phaseSeconds(p, estimate);
    if (phaseSeconds != null) items.push(`${window.PerformanceTools.duration(phaseSeconds)} nesta etapa`);
    if (p.unit === "candidatos" && Number.isFinite(p.selected)) items.push(`${fmtNum(p.selected)} selecionados`);
    const fresh = estimate && performance.now() - estimate.updated <= 10000;
    if (fresh && estimate.rate > 0) items.push(`${fmtNum(Math.round(estimate.rate))} ${p.unit || "itens"}/s`);
    if (fresh && estimate.eta != null) items.push(`≈ ${window.PerformanceTools.duration(estimate.eta)} nesta etapa`);
    if (p.resumedRows > 0) items.push(`${fmtNum(p.resumedRows)} retomados`);
    if (p.checkpointRows > 0) items.push(`${fmtNum(p.checkpointRows)} salvos`);
    if (p.error) items.push(p.error);
    if (estimate && performance.now() - estimate.updated > 10000) items.push("Aguardando atualização de progresso");
    return items.join(" · ");
  }

  function loader() { const node = document.createElement("span"); node.className = "li-loader"; node.setAttribute("aria-hidden", "true"); return node; }
  const elapsed = t => { const s = Math.round((performance.now() - t.started) / 1000); return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`; };
  let drawn = "";
  function drawDialog() {
    const list = groups(), body = dialog.querySelector(".tasks-list");
    // Same work as before: only the elapsed times change, so buttons stay put under the pointer.
    const shape = list.map(g => g.key + ":" + g.tasks.map(t => t.id).join(",")).join("|");
    if (shape === drawn && list.length) { for (const g of list) for (const t of g.tasks) { const cell = body.querySelector(`[data-elapsed="${t.id}"]`); if (cell) cell.textContent = elapsed(t); const status = body.querySelector(`[data-status="${t.id}"]`); if (status) status.textContent = detail(t); const cancelButton = body.querySelector(`[data-cancel="${t.id}"]`); if (cancelButton) cancelButton.disabled = t.cancelled; } return; }
    drawn = shape;
    body.innerHTML = list.length ? list.map(g => `<div class="task-group"><div class="task-origin" data-loading>${esc(g.label)}<span class="li-loader" aria-hidden="true"></span></div>${g.tasks.map(t => `<div class="task-row"><span>${esc(WHAT[t.cmd] || t.cmd)}<small data-status="${t.id}" style="display:block;max-width:36rem">${esc(detail(t))}</small></span><small data-elapsed="${t.id}">${elapsed(t)}</small>${t.operationId ? `<button type="button" class="btn ghost small" data-cancel="${t.id}" ${t.cancelled ? "disabled" : ""}>Cancelar</button>` : ""}</div>`).join("")}</div>`).join("") : '<p class="quiet-empty">Nada carregando agora.</p>';
    body.querySelectorAll("[data-cancel]").forEach(b => { b.onclick = () => { const t = tasks.get(+b.dataset.cancel); if (t) cancel(t); }; });
    dialog.querySelector("[data-cancel-all]").hidden = !list.length;
  }
  function openDialog() {
    if (dialog) return;
    dialog = el("div", "modal-overlay tasks-overlay");
    dialog.innerHTML = `<section class="modal tasks-modal" role="dialog" aria-modal="true" aria-labelledby="tasks-title"><div class="modal-head"><h3 id="tasks-title">Em andamento</h3><button class="icon-btn" type="button" data-close aria-label="Fechar"><i class="fas fa-xmark"></i></button></div><div class="modal-body"><p class="muted small">Trocar de menu ou aba não interrompe o carregamento.</p><div class="tasks-list"></div><div class="modal-actions"><button type="button" class="btn ghost" data-cancel-all>Cancelar tudo</button></div></div></section>`;
    const close = () => { dialog?.remove(); dialog = null; drawn = ""; };
    dialog.onclick = e => { if (e.target === dialog) close(); };
    dialog.querySelector("[data-close]").onclick = close;
    dialog.querySelector("[data-cancel-all]").onclick = () => { cancelAll(); base("cancel_operation", {}, { silent: true }).catch(() => {}); };
    dialog.addEventListener("keydown", e => { if (e.key === "Escape") { e.stopPropagation(); close(); } });
    document.body.append(dialog); drawDialog(); dialog.querySelector("[data-close]").focus();
  }

  const button = el("button", "workbar-tasks"); button.id = "workbar-tasks"; button.type = "button";
  button.innerHTML = '<i class="li-pulse" aria-hidden="true"></i><span></span>';
  button.onclick = openDialog;
  $("#workbar-label").before(button);
  return { cancelAll, cancelLatest, cancelOperation: id => cancel([...tasks.values()].find(t => t.operationId === id)), progress, detail, operationFor: key => latest.get(key)?.operationId || null, pendingSources: () => [...tasks.values()].some(task => sourceMutations.has(task.cmd)), pending: () => tasks.size, open: openDialog, running: () => visible().length, groups };
})();
