/* Security triage on the summary: what deserves attention first, with the records behind it one click away. */
window.Security = (() => {
  "use strict";
  const SEVERITY = { critical: ["Crítica", 4], high: ["Alta", 3], medium: ["Média", 2], low: ["Baixa", 1], info: ["Informativa", 0] };
  const sevLabel = s => SEVERITY[s]?.[0] || s;
  const results = new Map();
  const pending = new Map();
  const names = new Map();
  let tacticFilter = null;
  let showAll = false;
  let lastData = null;
  const COLLAPSED = 3;

  const key = () => JSON.stringify([workspaceScope(), window.Workspace?.sourceKey?.() || ""]);
  const two = n => String(n).padStart(2, "0");
  const shortTime = ms => { const d = new Date(ms); return `${two(d.getDate())}/${two(d.getMonth() + 1)} ${two(d.getHours())}:${two(d.getMinutes())}`; };
  const range = (a, b) => {
    if (a == null) return "sem horário";
    if (b == null || b === a) return shortTime(a);
    const da = new Date(a), db = new Date(b);
    return da.toDateString() === db.toDateString() ? `${shortTime(a)} – ${two(db.getHours())}:${two(db.getMinutes())}` : `${shortTime(a)} – ${shortTime(b)}`;
  };
  const tacticLabel = (data, k) => data?.tactics?.find(t => t.key === k)?.label || k;
  const icon = (name, label, action) => `<button type="button" class="icon-btn sec-act" data-act="${action}" title="${esc(label)}" aria-label="${esc(label)}"><i class="fas ${name}" aria-hidden="true"></i></button>`;

  function remember(data) {
    for (const d of data?.detections || []) names.set(d.rule, d.name);
  }
  async function get({ force = false } = {}) {
    const k = key();
    if (!force && results.has(k)) return results.get(k);
    if (!force && pending.has(k)) return pending.get(k);
    const request = typeof analyticsRequest === "function" ? analyticsRequest(workspaceScope()) : { filters: backendFilters() };
    const promise = api("triage", { ...request, force }, { silent: true }).then(data => {
      remember(data);
      if (k === key()) { results.set(k, data); lastData = data; if (results.size > 8) results.delete(results.keys().next().value); }
      return data;
    }).finally(() => pending.delete(k));
    pending.set(k, promise);
    return promise;
  }
  const cached = () => results.get(key()) || null;
  document.addEventListener("workspace-context-change", () => { tacticFilter = null; showAll = false; });

  // ---------------------------------------------------------------- evidence
  function detectionFilters(d) {
    const time = d.start != null ? [{ column: "timestamp", op: "between", value: String(d.start), value2: String(d.end ?? d.start) }] : [];
    return [...d.filters, ...time];
  }
  const FIELD_NAME = /^@?[\p{L}\p{N}_.-]+$/u;
  const quoted = value => `"${String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  // One detection as a query clause: the rule itself plus the entity it grouped by.
  function detectionClause(d) {
    const parts = [];
    for (const f of d.filters) {
      if (f.op === "detection") parts.push(`deteccao:${quoted(f.value)}`);
      else if (f.op === "query") parts.push(`(${f.value})`);
      else if (f.op === "equals_exact" && FIELD_NAME.test(f.column)) parts.push(`${f.column}:${quoted(f.value)}`);
    }
    return parts.join(" AND ");
  }
  function episodeFilters(data, episode) {
    const detections = episode.detections.map(i => data.detections[i]);
    if (detections.length === 1) return detectionFilters(detections[0]);
    const clauses = detections.map(detectionClause).filter(Boolean);
    const time = episode.start != null ? [{ column: "timestamp", op: "between", value: String(episode.start), value2: String(episode.end ?? episode.start) }] : [];
    return [...(clauses.length ? [{ column: "_all", op: "query", value: clauses.map(c => `(${c})`).join(" OR "), label: `Episódio: ${episode.title}` }] : []), ...time];
  }
  const keepContext = () => state.filters.filter(f => f.column !== "timestamp");
  function showRecords(filters) { window.Workspace.applyFilters([...keepContext(), ...filters], true); }
  function showTimeline(filters, start, end) {
    if (start != null) window.Workspace.focusTimeline?.(start, end ?? start);
    window.Workspace.applyFilters([...keepContext(), ...filters.filter(f => f.column !== "timestamp")], true, "timeline");
  }
  async function saveToCase(data, detections, title, summary) {
    if (workspaceScope() === "case") { toast("Estes registros já pertencem ao Caso.", "info"); return; }
    const ids = [...new Set(detections.flatMap(d => d.event_ids))].slice(0, 40);
    const rows = [];
    for (const id of ids) { try { const row = await api("event_detail", { id }, { silent: true }); if (row) rows.push(row); } catch { /* record no longer available */ } }
    const c = ensureCase();
    const start = Math.min(...detections.map(d => d.start ?? Infinity)), end = Math.max(...detections.map(d => d.end ?? -Infinity));
    const filters = detections.length === 1 ? detectionFilters(detections[0]) : [];
    c.items.push({
      id: "i" + Date.now().toString(36) + Math.floor(Math.random() * 1e4), kind: "grupo", label: title, note: summary, createdAt: Date.now(),
      rows, sourceFilters: [...backendFilters().filter(f => f.column !== "timestamp" && !f._quick), ...filters], sourceSpec: structuredClone(state.currentArtifact?.source),
      foundCount: detections.reduce((n, d) => n + d.count, 0), includedCount: rows.length, tags: ["detecção"], relevance: detections.some(d => SEVERITY[d.severity]?.[1] >= 3) ? "importante" : "normal",
      origin: state.currentOrigin, artifactId: state.currentArtifact?.id, stationId: null,
      detection: { detections: detections.map(d => ({ rule: d.rule, name: d.name, severity: d.severity, attack: d.attack, tactics: d.tactics, summary: d.summary, start: d.start, end: d.end, count: d.count, entities: d.entities })), start: Number.isFinite(start) ? start : null, end: Number.isFinite(end) ? end : null },
    });
    if (await saveCases()) { window.Workspace?.loaded && updateCountsSafe(); window.WorkspaceContext?.refreshMembership?.(); toast("Salvo no Caso com os registros de apoio.", "ok"); }
  }
  const updateCountsSafe = () => { try { document.querySelector("#ws-evidence-count").textContent = activeCase()?.items?.length || ""; } catch { /* navigation not ready */ } };
  async function suppress(detection) {
    const settings = (await rules()).settings;
    const entity = detection.entities[0];
    settings.suppress = [...(settings.suppress || []), { rule: detection.rule, column: entity?.column || null, value: entity?.value || null, note: "", created: Date.now() }];
    await api("detection_settings_save", { settings });
    results.clear(); rulesCache = null;
    toast(entity ? `Ocultado para ${entity.value}.` : "Detecção ocultada.", "ok");
    window.Workspace.showPage(window.Workspace.page());
  }

  // ---------------------------------------------------------------- attention
  function tacticsStrip(data) {
    const max = Math.max(1, ...data.tactics.map(t => t.count));
    return `<div class="sec-tactics" role="group" aria-label="Táticas MITRE ATT&CK">${data.tactics.map(t => {
      const level = t.count ? Math.ceil(4 * t.count / max) : 0;
      const tips = t.techniques.slice(0, 4).map(x => `${x.id} ${x.name}`).join("\n");
      return `<button type="button" class="sec-tactic l${level}${tacticFilter === t.key ? " on" : ""}" data-tactic="${esc(t.key)}" ${t.count ? "" : "disabled"} title="${esc(`${t.label}${t.count ? ` · ${t.count} ${t.count === 1 ? "detecção" : "detecções"}` : ""}${tips ? `\n${tips}` : ""}`)}" aria-label="${esc(t.label)}"></button>`;
    }).join("")}</div>`;
  }
  function episodeRow(data, episode, index) {
    const detections = episode.detections.map(i => data.detections[i]);
    const records = detections.reduce((n, d) => n + d.count, 0);
    const chain = episode.tactics.map(t => tacticLabel(data, t)).join(" → ");
    return `<article class="sec-episode sev-${esc(episode.severity)}" data-episode="${index}" tabindex="0" aria-expanded="false">
      <span class="sec-bar" title="Severidade ${esc(sevLabel(episode.severity))}"></span>
      <div class="sec-main"><h3>${esc(episode.title)}</h3><p>${esc(episode.summary)}</p>
        <div class="sec-meta"><time>${esc(range(episode.start, episode.end))}</time><span>${fmtNum(detections.length)} ${detections.length === 1 ? "detecção" : "detecções"} · ${fmtNum(records)} ${records === 1 ? "registro" : "registros"}</span>${chain ? `<span class="sec-chain">${esc(chain)}</span>` : ""}<span class="sec-entities"></span></div>
        <div class="sec-detections" hidden></div></div>
      <div class="sec-actions">${icon("fa-list", "Ver registros", "records")}${icon("fa-timeline", "Ver na linha do tempo", "timeline")}${workspaceScope() === "dataset" ? icon("fa-bookmark", "Salvar no Caso", "save") : ""}</div></article>`;
  }
  function detectionRows(data, episode) {
    return episode.detections.map(i => {
      const d = data.detections[i];
      const attack = d.attack.map(a => `${a.id} ${a.name}`).join(" · ");
      return `<div class="sec-detection sev-${esc(d.severity)}" data-detection="${i}" title="${esc(`${d.description}${attack ? `\n\nATT&CK: ${attack}` : ""}`)}">
        <span class="sec-dot"></span><div><strong>${esc(d.name)}</strong><small>${esc(d.summary)}</small></div>
        <span class="sec-count">${fmtNum(d.count)}</span><time>${esc(range(d.start, d.end))}</time>
        <span class="sec-row-actions">${icon("fa-list", "Ver registros", "d-records")}${icon("fa-eye-slash", "Ocultar esta detecção", "d-hide")}</span></div>`;
    }).join("");
  }
  function drawAttention(slot, data) {
    const matching = data.episodes.map((e, i) => ({ e, i })).filter(({ e }) => !tacticFilter || e.tactics.includes(tacticFilter));
    const episodes = showAll || tacticFilter ? matching : matching.slice(0, COLLAPSED);
    const hidden = matching.length - episodes.length;
    const rulesText = `${fmtNum(data.rules)} regras${data.threat_rules ? ` · ${fmtNum(data.threat_rules)} sinais` : ""}${data.sigma_rules ? ` · ${fmtNum(data.sigma_rules)} Sigma` : ""}`;
    if (!data.episodes.length) {
      slot.className = "sec-clear";
      slot.innerHTML = `<i class="fas fa-shield-halved" aria-hidden="true"></i><span>Nenhum padrão de ataque identificado</span><small>${esc(rulesText)} · ${fmtNum(data.total)} registros</small>${data.suppressed ? `<button type="button" class="text-button" data-open-rules>${fmtNum(data.suppressed)} ocultas</button>` : ""}`;
      slot.hidden = false;
      slot.querySelector("[data-open-rules]")?.addEventListener("click", () => openSettings("detection"));
      return;
    }
    slot.className = "ws-card sec-attention";
    slot.innerHTML = `<div class="card-heading"><h2>Atenção</h2>${tacticsStrip(data)}<span class="sec-heading-count">${fmtNum(data.episodes.length)} ${data.episodes.length === 1 ? "episódio" : "episódios"}</span></div>
      ${tacticFilter ? `<div class="sec-filtering">${esc(tacticLabel(data, tacticFilter))} <button type="button" class="text-button" data-clear-tactic>mostrar todos</button></div>` : ""}
      <div class="sec-episodes">${episodes.map(({ e, i }) => episodeRow(data, e, i)).join("")}</div>
      ${hidden > 0 ? `<button type="button" class="sec-more-episodes" data-more>Mostrar mais ${fmtNum(hidden)} ${hidden === 1 ? "episódio" : "episódios"}</button>` : showAll && matching.length > COLLAPSED ? '<button type="button" class="sec-more-episodes" data-more>Mostrar menos</button>' : ""}
      <div class="sec-foot"><span>${esc(rulesText)} · ${fmtNum(data.total)} registros${data.elapsed_ms ? ` em ${(data.elapsed_ms / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} s` : ""}</span>${data.limited ? '<span class="sec-limited" title="Algum orçamento de memória foi atingido; refine o período para análise completa.">parcial</span>' : ""}${data.suppressed ? `<button type="button" class="text-button" data-open-rules>${fmtNum(data.suppressed)} ocultas</button>` : '<button type="button" class="text-button" data-open-rules>Regras</button>'}</div>`;
    slot.hidden = false;
    for (const article of slot.querySelectorAll("[data-episode]")) {
      const episode = data.episodes[+article.dataset.episode];
      const holder = article.querySelector(".sec-entities");
      for (const entity of episode.entities.slice(0, 3)) holder.append(window.EntityMenu.chip({ ...entity, first: episode.start, last: episode.end }, "small"));
    }
    bindAttention(slot, data);
  }
  function bindAttention(slot, data) {
    slot.querySelectorAll("[data-tactic]").forEach(b => b.onclick = () => { tacticFilter = tacticFilter === b.dataset.tactic ? null : b.dataset.tactic; drawAttention(slot, data); });
    slot.querySelector("[data-clear-tactic]")?.addEventListener("click", () => { tacticFilter = null; drawAttention(slot, data); });
    slot.querySelectorAll("[data-open-rules]").forEach(b => b.onclick = () => openSettings("detection"));
    slot.querySelector("[data-more]")?.addEventListener("click", () => { showAll = !showAll; drawAttention(slot, data); });
    const episodeOf = node => data.episodes[+node.closest("[data-episode]").dataset.episode];
    const toggle = article => {
      const list = article.querySelector(".sec-detections"), open = list.hidden;
      if (open && !list.innerHTML) list.innerHTML = detectionRows(data, episodeOf(article));
      list.hidden = !open; article.setAttribute("aria-expanded", String(open));
    };
    slot.querySelector(".sec-episodes")?.addEventListener("click", async event => {
      const article = event.target.closest("[data-episode]");
      if (!article || event.target.closest(".entity-chip")) return;
      const action = event.target.closest("[data-act]")?.dataset.act;
      const episode = episodeOf(article);
      const detection = event.target.closest("[data-detection]") && data.detections[+event.target.closest("[data-detection]").dataset.detection];
      if (action === "records") showRecords(episodeFilters(data, episode));
      else if (action === "timeline") showTimeline(episodeFilters(data, episode), episode.start, episode.end);
      else if (action === "save") await saveToCase(data, episode.detections.map(i => data.detections[i]), episode.title, episode.summary);
      else if (action === "d-records" && detection) showRecords(detectionFilters(detection));
      else if (action === "d-hide" && detection) await suppress(detection);
      else if (!detection) toggle(article);
      else showRecords(detectionFilters(detection));
    });
    slot.querySelector(".sec-episodes")?.addEventListener("keydown", event => {
      const article = event.target.closest("[data-episode]");
      if (article && event.target === article && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); toggle(article); }
    });
    slot.querySelector(".sec-episodes")?.addEventListener("contextmenu", event => {
      const article = event.target.closest("[data-episode]");
      if (!article || event.target.closest(".entity-chip")) return;
      event.preventDefault();
      const episode = episodeOf(article);
      const detectionNode = event.target.closest("[data-detection]");
      const detection = detectionNode && data.detections[+detectionNode.dataset.detection];
      const target = detection ? { title: detection.name, summary: detection.summary, list: [detection], filters: detectionFilters(detection), start: detection.start, end: detection.end } : { title: episode.title, summary: episode.summary, list: episode.detections.map(i => data.detections[i]), filters: episodeFilters(data, episode), start: episode.start, end: episode.end };
      showCtxMenu(event.clientX, event.clientY, [
        { icon: "fa-list", label: "Ver registros", onClick: () => showRecords(target.filters) },
        { icon: "fa-timeline", label: "Ver na linha do tempo", onClick: () => showTimeline(target.filters, target.start, target.end) },
        ...(workspaceScope() === "dataset" ? [{ icon: "fa-bookmark", label: "Salvar no Caso", onClick: () => saveToCase(data, target.list, target.title, target.summary) }] : []),
        { icon: "fa-copy", label: "Copiar resumo", onClick: () => navigator.clipboard?.writeText(`${target.title}\n${target.summary}\n${range(target.start, target.end)}\n${target.list.map(d => `- ${d.name}: ${d.summary} (${d.count})`).join("\n")}`) },
        ...(detection ? [{ sep: true }, { icon: "fa-eye-slash", label: "Ocultar esta detecção", onClick: () => suppress(detection) }] : []),
      ]);
    });
  }

  // ---------------------------------------------------------------- entities and rarity
  function drawEntities(slot, data) {
    if (!data.entities.length) { slot.hidden = true; return; }
    slot.className = "ws-card sec-entities-card";
    slot.innerHTML = `<div class="card-heading"><h2>Entidades em destaque</h2><span>por risco</span></div><div class="sec-entity-list"></div>`;
    const list = slot.querySelector(".sec-entity-list");
    for (const e of data.entities.slice(0, 8)) {
      const row = el("button", `sec-entity risk-${e.level === "alto" ? "high" : e.level === "médio" ? "medium" : "low"}`);
      row.type = "button";
      row.innerHTML = `<i class="fas ${window.EntityMenu.icon(e.column)}" aria-hidden="true"></i><span class="sec-entity-name"><strong></strong><small></small></span><span class="sec-risk" aria-hidden="true"><b style="width:${Math.max(6, e.score)}%"></b></span><span class="sec-score">${e.score}</span>`;
      row.querySelector("strong").textContent = e.value;
      row.querySelector("small").textContent = `${e.label}${e.scope && e.scope !== "privado" ? ` · ${e.scope}` : ""} · ${e.detections} ${e.detections === 1 ? "detecção" : "detecções"}`;
      row.title = `${e.label}: ${e.value}\nRisco ${e.score}/100 (${e.level})\n${fmtNum(e.events)} registros${e.failures ? ` · ${fmtNum(e.failures)} falhas` : ""}${e.first != null ? `\n${range(e.first, e.last)}` : ""}${e.tactics.length ? `\n${e.tactics.map(t => tacticLabel(data, t)).join(" · ")}` : ""}`;
      const act = event => { event.preventDefault(); window.EntityMenu.open(event.clientX, event.clientY, e); };
      row.onclick = act; row.oncontextmenu = act;
      list.append(row);
    }
    slot.hidden = false;
  }
  function drawRare(slot, data, expanded = false) {
    if (!data.rare.length) { slot.hidden = true; return; }
    const shown = expanded ? data.rare : data.rare.slice(0, 6);
    slot.className = "ws-card sec-rare-card";
    slot.innerHTML = `<div class="card-heading"><h2>Raridades</h2><span title="Valores que quase nunca aparecem entre muitos registros do mesmo tipo">pouco comuns</span></div>${shown.map((r, i) => `<button type="button" class="sec-rare" data-rare="${i}" title="${esc(`${r.label}: ${r.value}\n${r.count} de ${fmtNum(r.role_events)} registros · ${fmtNum(r.role_distinct)} valores distintos${r.first != null ? `\nPrimeira vez: ${shortTime(r.first)}` : ""}`)}"><code></code><small>${esc(r.label)}</small><span>${fmtNum(r.count)}</span></button>`).join("")}${shown.length < data.rare.length ? `<button type="button" class="text-button sec-more">Mais ${data.rare.length - shown.length}</button>` : ""}`;
    slot.querySelectorAll("[data-rare]").forEach(b => {
      const r = data.rare[+b.dataset.rare];
      b.querySelector("code").textContent = r.value;
      b.onclick = () => showRecords([r.filter]);
      b.oncontextmenu = event => { event.preventDefault(); window.EntityMenu.open(event.clientX, event.clientY, { column: r.column, value: r.value, first: r.first }); };
    });
    slot.querySelector(".sec-more")?.addEventListener("click", () => drawRare(slot, data, true));
    slot.hidden = false;
  }

  // ---------------------------------------------------------------- timeline markers
  /** Detection marks under a time axis: hover names them, click selects their interval. */
  function markers(host, start, end, onPick, before = null) {
    if (!host) return;
    host.querySelector(".sec-markers")?.remove();
    const draw = data => {
      if (!host.isConnected || !data?.detections?.length || !(end > start)) return;
      const visible = data.detections.filter(d => d.start != null && d.end >= start && d.start <= end);
      if (!visible.length) return;
      const strip = el("div", "sec-markers");
      strip.setAttribute("aria-label", "Detecções no período");
      const groups = new Map();
      for (const d of visible) {
        const x = Math.max(0, Math.min(1, (Math.max(d.start, start) - start) / (end - start)));
        const slot = Math.round(x * 200);
        (groups.get(slot) || groups.set(slot, []).get(slot)).push(d);
      }
      for (const [slot, list] of groups) {
        list.sort((a, b) => (SEVERITY[b.severity]?.[1] || 0) - (SEVERITY[a.severity]?.[1] || 0));
        const mark = el("button", `sec-marker sev-${list[0].severity}`);
        mark.type = "button";
        mark.style.left = `${slot / 2}%`;
        mark.title = list.slice(0, 6).map(d => `${d.name} · ${d.summary}`).join("\n") + (list.length > 6 ? `\n+${list.length - 6}` : "");
        mark.setAttribute("aria-label", mark.title);
        mark.onclick = () => onPick(Math.min(...list.map(d => d.start)), Math.max(...list.map(d => d.end ?? d.start)), list);
        mark.oncontextmenu = event => { event.preventDefault(); showCtxMenu(event.clientX, event.clientY, list.slice(0, 8).map(d => ({ icon: "fa-list", label: `${d.name} (${fmtNum(d.count)})`, onClick: () => showRecords(detectionFilters(d)) }))); };
        strip.append(mark);
      }
      if (before && before.parentElement === host) host.insertBefore(strip, before); else host.append(strip);
    };
    const now = cached();
    if (now) draw(now); else get().then(draw, () => {});
  }

  // ---------------------------------------------------------------- summary
  async function fillSummary({ attention, entities, rare }) {
    if (!attention) return;
    attention.className = "sec-loading";
    attention.innerHTML = '<i class="fas fa-circle-notch spin" aria-hidden="true"></i><span>Procurando padrões de ataque…</span>';
    attention.hidden = false;
    const k = key();
    try {
      const data = await get();
      if (!attention.isConnected || k !== key()) return;
      drawAttention(attention, data);
      if (entities) drawEntities(entities, data);
      if (rare) drawRare(rare, data);
    } catch (error) {
      if (!attention.isConnected) return;
      attention.className = "sec-clear sec-failed";
      attention.innerHTML = `<i class="fas fa-triangle-exclamation" aria-hidden="true"></i><span>Triagem indisponível</span><small></small><button type="button" class="text-button">Tentar novamente</button>`;
      attention.querySelector("small").textContent = String(error).slice(0, 200);
      attention.querySelector("button").onclick = () => { results.delete(k); fillSummary({ attention, entities, rare }); };
    }
  }

  // ---------------------------------------------------------------- rules and settings
  let rulesCache = null;
  async function rules() {
    if (!rulesCache) rulesCache = await api("detection_rules", {}, { silent: true });
    for (const r of rulesCache.rules) names.set(r.id, r.name);
    return rulesCache;
  }
  async function renderRulesPane(pane) {
    pane.innerHTML = '<div class="ws-loading"><i class="fas fa-circle-notch spin"></i>Carregando regras…</div>';
    let overview;
    try { rulesCache = null; overview = await rules(); } catch (error) { pane.innerHTML = `<p class="muted small">${esc(String(error))}</p>`; return; }
    const settings = overview.settings;
    const save = async (message) => { await api("detection_settings_save", { settings }); results.clear(); rulesCache = null; if (message) toast(message, "ok"); };
    const groups = [["builtin", "Regras embutidas"], ["sigma", "Sigma importadas"]];
    pane.innerHTML = `<div class="rules-top"><label class="check-line"><input type="checkbox" id="rules-threats" ${settings.threats ? "checked" : ""}> Incluir sinais do catálogo de ameaças na triagem</label>
      <div class="rules-actions"><button type="button" class="btn ghost small" id="rules-import"><i class="fas fa-file-import"></i> Importar Sigma</button><button type="button" class="btn ghost small" id="rules-import-folder">Pasta Sigma</button>${overview.rules.some(r => r.origin === "sigma") ? '<button type="button" class="btn ghost small" id="rules-clear">Remover Sigma</button>' : ""}</div></div>
      <input type="search" class="rules-search" placeholder="Filtrar regras…" aria-label="Filtrar regras">
      ${settings.suppress?.length ? `<details class="rules-suppress"><summary>${settings.suppress.length} ${settings.suppress.length === 1 ? "detecção oculta" : "detecções ocultas"}</summary>${settings.suppress.map((s, i) => `<div class="rules-suppressed"><span>${esc(names.get(s.rule) || s.rule)}${s.value ? ` · ${esc(s.value)}` : ""}</span><button type="button" class="text-button" data-unsuppress="${i}">Mostrar novamente</button></div>`).join("")}</details>` : ""}
      ${overview.sigma_errors.length ? `<details class="rules-errors"><summary>${overview.sigma_errors.length} regras Sigma não convertidas</summary>${overview.sigma_errors.slice(0, 50).map(e => `<div>${esc(e)}</div>`).join("")}</details>` : ""}
      <div class="rules-list">${groups.map(([origin, label]) => { const list = overview.rules.filter(r => r.origin === origin); return list.length ? `<h4>${label} <span>${list.filter(r => r.enabled).length}/${list.length}</span></h4>${list.map(r => `<label class="rule-row" data-search="${esc(`${r.name} ${r.id} ${r.attack.map(a => a.id + " " + a.name).join(" ")}`.toLowerCase())}" title="${esc(r.description)}"><input type="checkbox" data-rule="${esc(r.id)}" ${r.enabled ? "checked" : ""}><span class="sec-dot sev-${esc(r.severity)}"></span><span class="rule-name">${esc(r.name)}</span><small>${esc(r.attack.map(a => a.id).join(" "))}</small></label>`).join("")}` : ""; }).join("")}</div>`;
    pane.querySelector("#rules-threats").onchange = async e => { settings.threats = e.target.checked; await save(); };
    pane.querySelectorAll("[data-rule]").forEach(box => box.onchange = async () => {
      const id = box.dataset.rule;
      settings.disabled = box.checked ? settings.disabled.filter(x => x !== id) : [...new Set([...settings.disabled, id])];
      await save();
    });
    pane.querySelectorAll("[data-unsuppress]").forEach(b => b.onclick = async () => { settings.suppress.splice(+b.dataset.unsuppress, 1); await save("A detecção voltará a aparecer."); renderRulesPane(pane); });
    pane.querySelector(".rules-search").oninput = e => { const q = e.target.value.toLowerCase(); pane.querySelectorAll(".rule-row").forEach(r => { r.hidden = !!q && !r.dataset.search.includes(q); }); };
    const importFrom = async folder => {
      const chosen = await dialogApi.open({ multiple: !folder, directory: folder, filters: folder ? undefined : [{ name: "Sigma", extensions: ["yml", "yaml"] }] });
      if (!chosen) return;
      const result = await api("sigma_import", { paths: Array.isArray(chosen) ? chosen : [chosen] });
      results.clear();
      toast(`${fmtNum(result.rules)} regras Sigma importadas${result.failed.length ? ` · ${result.failed.length} arquivos não convertidos` : ""}.`, result.rules ? "ok" : "info");
      renderRulesPane(pane);
    };
    pane.querySelector("#rules-import").onclick = () => importFrom(false);
    pane.querySelector("#rules-import-folder").onclick = () => importFrom(true);
    pane.querySelector("#rules-clear")?.addEventListener("click", async () => { if (!confirm("Remover todas as regras Sigma importadas?")) return; await api("sigma_clear", {}); results.clear(); renderRulesPane(pane); });
  }

  // ---------------------------------------------------------------- hunting recipes
  const RECIPES = [
    { icon: "fa-user-lock", label: "Falhas de autenticação", query: "@action:logon @outcome:failure" },
    { icon: "fa-right-to-bracket", label: "Acessos a partir da internet", query: "@action:logon @outcome:success @src_scope:público" },
    { icon: "fa-user-plus", label: "Contas criadas ou promovidas", query: "@action:(account_create OR group_member_add OR privilege_grant OR credential_create)" },
    { icon: "fa-anchor", label: "Persistência: serviços, tarefas e inicialização", query: "@action:(service_install OR task_create OR registry_change)" },
    { icon: "fa-terminal", label: "Execução de PowerShell e shells", query: "@process:/(^|[\\\\/])(powershell|pwsh|cmd|bash|sh)(\\.exe)?$/ OR @action:script_execution" },
    { icon: "fa-eraser", label: "Registros apagados ou auditoria alterada", query: "@action:(log_clear OR audit_policy_change OR protection_disabled)" },
    { icon: "fa-toolbox", label: "Ferramentas de ataque e varredura", query: "@tool:*" },
    { icon: "fa-shield-virus", label: "Conteúdo compatível com ameaças", query: "regra:*" },
    { icon: "fa-server", label: "Erros de servidor (5xx)", query: "@status:5*" },
  ];
  function recipes() { return RECIPES.map(r => ({ ...r, run: () => window.Workspace.search(r.query) })); }
  function recipesMenu(x, y) { showCtxMenu(x, y, recipes().map(r => ({ icon: r.icon, label: r.label, onClick: r.run }))); }

  function openEpisode(index) {
    const data = lastData, episode = data?.episodes?.[index];
    if (episode) showRecords(episodeFilters(data, episode));
  }
  return { get, cached, fillSummary, markers, renderRulesPane, recipes, recipesMenu, openEpisode, ruleName: id => names.get(id) || null, rules: async () => (await rules()).rules, invalidate: () => results.clear(), last: () => lastData };
})();
