/* Work in progress, per menu and tab. Loading continues when the user navigates away; returning to a
   tab joins the same request. The status bar summarizes what is loading and lists it for cancelling. */
window.Tasks = (() => {
  "use strict";
  const base = api;
  // Commands that only read: identical requests in flight are shared and survive a selective cancel.
  const READS = new Set(["threat_catalog", "threat_scan", "threat_events", "journey_fields", "journey_index", "journey_events", "dataset_overview", "triage", "event_insights", "detection_rules", "timeline_lanes", "entity_summary", "ioc_sightings", "source_hashes", "timeline_range", "compare_periods", "list_sources", "source_summary", "query_events", "explore_snapshot", "aggregate_events", "trail_events", "count_filtered", "tree_aggs", "stats_events", "event_detail", "profile_fields", "discover_patterns", "compute_series", "pivot", "list_channels", "list_formats", "list_derived_fields", "get_codes", "get_codes_path", "system_codes_count", "remote_list", "get_ts_config"]);
  // Bookkeeping calls never show as work.
  const QUIET = new Set(["validate_filters", "case_sync", "ui_zoom", "cancel_operation", "cases_save", "cases_load", "mcp_status", "get_codes_path", "system_codes_count", "list_formats"]);
  const WHAT = { dataset_overview: "Visão geral", triage: "Triagem de segurança", timeline_range: "Volume no tempo", timeline_lanes: "Faixas", query_events: "Registros", explore_snapshot: "Registros e campos", tree_aggs: "Campos", aggregate_events: "Grupos", profile_fields: "Perfil dos campos", compute_series: "Gráficos", pivot: "Tabela dinâmica", discover_patterns: "Padrões", threat_scan: "Ameaças", threat_events: "Registros de ameaça", compare_periods: "Comparação de períodos", journey_index: "Possíveis trilhas", journey_events: "Registros da trilha", journey_fields: "Campos de ligação", entity_summary: "Entidades", ioc_sightings: "Indicadores nas fontes", source_hashes: "SHA-256 das fontes", stats_events: "Histograma", count_filtered: "Contagem", trail_events: "Vizinhança do registro", load_file: "Abrindo logs", load_files: "Abrindo logs", load_bundle: "Abrindo logs", load_event_log: "Lendo o Event Log", remote_import: "Importação remota", remote_test: "Teste de conexão", export_events: "Exportação", export_investigation: "Exportação", import_investigation: "Importação da investigação", harvest_codes: "Catálogo do sistema", sigma_import: "Importação Sigma", expand_paths: "Lendo pastas", event_detail: "Detalhe do registro", list_sources: "Fontes", event_insights: "Detalhe do registro" };
  const ZONES = { analysis: "Análise", case: "Caso", structure: "Estrutura" };
  const PAGES = { summary: "Resumo", evidence: "Evidências", sources: "Arquivos", connections: "Conexões", import: "Abrir logs", "case-timeline": "Linha do tempo", "case-trails": "Trilhas", journeys: "Possíveis trilhas" };
  const TABS = { table: "Registros", group: "Resumir", dashboard: "Descobrir", cube: "Cruzar dados" };
  const VISIBLE_AFTER = 300;
  const tasks = new Map(), inflight = new Map(), ids = new WeakMap();
  let serial = 0, arrays = 0, selectiveUntil = 0, dialog = null, ticker = null;

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
    for (;;) {
      try { return await base(entry.cmd, entry.args, { ...entry.opts, silent: true }); }
      catch (error) {
        // A selective cancel stops every running read in the engine; the ones still wanted start again.
        if (!entry.cancelled && entry.read && entry.retries < 3 && Date.now() < selectiveUntil && /cancelad/i.test(String(error))) { entry.retries++; continue; }
        throw error;
      }
    }
  }
  function start(cmd, args, opts) {
    const from = origin(), entry = { id: ++serial, cmd, args, opts, read: READS.has(cmd), retries: 0, cancelled: false, started: performance.now(), ...from };
    entry.promise = new Promise((resolve, reject) => {
      entry.reject = reject;
      run(entry).then(resolve, error => { if (!entry.cancelled && !opts.silent) toast(String(error), "err"); reject(error); });
    }).finally(() => { if (tasks.get(entry.id) === entry) { tasks.delete(entry.id); schedule(); } if (inflight.get(entry.key2) === entry) inflight.delete(entry.key2); });
    tasks.set(entry.id, entry);
    setTimeout(schedule, VISIBLE_AFTER + 10);
    return entry;
  }
  api = function(cmd, args = {}, opts = {}) {
    if (QUIET.has(cmd)) return base(cmd, args, opts);
    if (!READS.has(cmd)) return start(cmd, args, opts).promise;
    const key = signature(cmd, args), shared = inflight.get(key);
    if (shared && !shared.cancelled) return shared.promise;
    const entry = start(cmd, args, opts); entry.key2 = key; inflight.set(key, entry);
    return entry.promise;
  };

  const visible = () => [...tasks.values()].filter(t => !t.cancelled && performance.now() - t.started >= VISIBLE_AFTER);
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
    document.querySelectorAll("[data-loading]").forEach(node => { if (!marked.has(node)) node.removeAttribute("data-loading"); });
    marked.forEach(node => node.setAttribute("data-loading", ""));
    const bar = $("#workbar"), button = $("#workbar-tasks");
    bar.classList.toggle("has-tasks", list.length > 0);
    if (list.length) { button.querySelector("span").textContent = summary(list); button.title = "Ver e cancelar o que está carregando"; }
    if (dialog) drawDialog();
    // Menus and tabs are redrawn by their pages; marks are refreshed while work goes on.
    clearInterval(ticker); ticker = list.length || tasks.size ? setInterval(schedule, 500) : null;
  }

  function cancel(entry) {
    if (entry.cancelled) return;
    entry.cancelled = true; tasks.delete(entry.id); if (inflight.get(entry.key2) === entry) inflight.delete(entry.key2);
    entry.reject(new Error("Operação cancelada."));
    // The engine cancels everything at once: only do it when the remaining work can simply restart.
    const others = [...tasks.values()].filter(t => !t.cancelled);
    if (others.every(t => t.read)) { selectiveUntil = Date.now() + 15000; base("cancel_operation", {}, { silent: true }).catch(() => {}); }
    schedule();
  }
  function cancelAll() { for (const t of [...tasks.values()]) { t.cancelled = true; t.reject(new Error("Operação cancelada.")); } tasks.clear(); inflight.clear(); selectiveUntil = 0; schedule(); }

  const elapsed = t => { const s = Math.round((performance.now() - t.started) / 1000); return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`; };
  let drawn = "";
  function drawDialog() {
    const list = groups(), body = dialog.querySelector(".tasks-list");
    // Same work as before: only the elapsed times change, so buttons stay put under the pointer.
    const shape = list.map(g => g.key + ":" + g.tasks.map(t => t.id).join(",")).join("|");
    if (shape === drawn && list.length) { for (const g of list) for (const t of g.tasks) { const cell = body.querySelector(`[data-elapsed="${t.id}"]`); if (cell) cell.textContent = elapsed(t); } return; }
    drawn = shape;
    body.innerHTML = list.length ? list.map(g => `<div class="task-group"><div class="task-origin" data-loading>${esc(g.label)}</div>${g.tasks.map(t => `<div class="task-row"><span>${esc(WHAT[t.cmd] || t.cmd)}</span><small data-elapsed="${t.id}">${elapsed(t)}</small><button type="button" class="btn ghost small" data-cancel="${t.id}">Cancelar</button></div>`).join("")}</div>`).join("") : '<p class="quiet-empty">Nada carregando agora.</p>';
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
  return { cancelAll, open: openDialog, running: () => visible().length, groups };
})();
