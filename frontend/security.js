/* Security triage on the summary: what deserves attention first, with the records behind it one click away. */
window.Security = (() => {
  "use strict";
  const SEVERITY = { critical: ["Crítica", 4], high: ["Alta", 3], medium: ["Média", 2], low: ["Baixa", 1], info: ["Informativa", 0] };
  const sevLabel = s => SEVERITY[s]?.[0] || s;
  const results = new Map(), analysisRequests = new WeakMap();
  // A calculation is admitted only by an explicit action. Rendering never starts it.
  const pending = new Map(), previous = new Map(), failures = new Map(), resultOwners = new Map(), views = new Map();
  const scopeKey = () => JSON.stringify([window.AnalysisContexts?.capture()?.caseId, window.AnalysisContexts?.capture()?.instance, workspaceScope()]);
  const notify = () => document.dispatchEvent(new CustomEvent("security-analysis-change"));
  const names = new Map();
  let tacticFilter = null;
  let shownEpisodes = 3;
  let minimumEvidence = 5, universe = "", summarySlots = null;
  const evidence = () => window.EvidenceUI;
  const universeKey = () => JSON.stringify([window.AnalysisContexts?.capture(), workspaceScope(), workspaceScope() === "case" ? caseSig(true) : state.currentArtifact?.id, state.currentArtifact?.loadedAt]);
  function syncUniverse() { const next = universeKey(); if (next !== universe) { universe = next; minimumEvidence = 5; } }
  function setMinimum(n) {
    if (!Number.isInteger(n) || n < 1 || n > 5) return;
    syncUniverse(); minimumEvidence = n;
    const data = cached();
    if (data?.storage?.kind === "sqlite") {
      loadStoredPage(0).catch(error => toast(String(error), "err"));
      document.dispatchEvent(new CustomEvent("evidence-rigidity-change", { detail: { minimum: n } }));
      return;
    }
    if (data && summarySlots?.attention?.isConnected) {
      drawAttention(summarySlots.attention, data);
      if (summarySlots.entities) drawEntities(summarySlots.entities, data);
      if (summarySlots.rare) drawRare(summarySlots.rare, data);
    }
    document.dispatchEvent(new CustomEvent("evidence-rigidity-change", { detail: { minimum: n } }));
  }
  const visibleDetection = d => (d.evidence_level || 0) >= minimumEvidence && !d.context_only;
  const COLLAPSED = 3;

  const key = () => JSON.stringify([universeKey(), state.derivedFields]);
  let fullCaseKey = "", fullCaseRows = [];
  function fullRequest() {
    if (workspaceScope() !== "case") return { filters: [] };
    if (window.CaseEvidence?.active === true) return { filters: [], caseEvents: caseEvents("analysis-all") };
    const sig = caseSig(true);
    if (sig !== fullCaseKey) { fullCaseKey = sig; fullCaseRows = caseEventsCompute(true); }
    return { filters: [], caseEvents: fullCaseRows };
  }
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
  function status() {
    const scope = scopeKey(), run = pending.get(scope), failure = failures.get(scope);
    if (run) return { state: run.cancelled || run.key !== key() ? "cancelling" : "calculating" };
    if (failure?.key === key()) return failure;
    if (cached()) return { state: "ready" };
    return { state: previous.has(scope) ? "stale" : "idle" };
  }
  function cancel() {
    const run = pending.get(scopeKey());
    if (!run || run.cancelled) return;
    run.cancelled = true;
    window.Tasks?.cancelLatest(run.latest);
    notify();
  }
  function invalidate() {
    const caseId = window.AnalysisContexts?.capture()?.caseId;
    for (const [k, owner] of resultOwners) if (owner.caseId === caseId) { results.delete(k); resultOwners.delete(k); }
    for (const run of pending.values()) if (run.owner?.caseId === caseId && !(run.preparing && !run.owner?.identity)) {
      run.cancelled = true; window.Tasks?.cancelLatest(run.latest);
    }
    failures.clear(); pageGeneration++; notify();
  }
  function get({ force = false } = {}) {
    syncUniverse();
    const scope = scopeKey(), k = key();
    if (pending.has(scope)) return pending.get(scope).promise;
    if (!force && results.has(k)) return Promise.resolve(results.get(k));
    const run = { key: k, scope, owner: window.AnalysisContexts?.capture(), latest: `security:${scope}`, preparing: true, cancelled: false };
    failures.delete(scope); pageGeneration++;
    // Start in a microtask so repeated clicks join the same admitted request.
    run.promise = Promise.resolve().then(async () => {
      if (run.owner) run.owner = await window.AnalysisContexts.prepare(run.owner);
      if (run.cancelled || scope !== scopeKey()) throw Error("Operação cancelada.");
      run.key = key(); run.preparing = false; notify();
      const request = fullRequest();
      const data = await api("triage", { ...request, force, minimumEvidence, episodeLimit: 20 }, { silent: true, analysisOwner: run.owner, latest: run.latest });
      if (run.cancelled || run.key !== key() || scope !== scopeKey()) throw Error("Operação cancelada: o Caso, os dados ou as regras mudaram.");
      if (run.owner) window.AnalysisContexts.assertOwner(run.owner);
      analysisRequests.set(data, { request, owner: run.owner }); remember(data);
      results.set(run.key, data); resultOwners.set(run.key, run.owner || {}); previous.set(scope, true);
      if (results.size > 8) { const oldest = results.keys().next().value; results.delete(oldest); resultOwners.delete(oldest); }
      return data;
    }).catch(error => {
      if (run.key === key() && scope === scopeKey()) failures.set(scope, { key: run.key, state: run.cancelled || /cancelad|ANALYSIS_CONTEXT_CHANGED/i.test(String(error)) ? "cancelled" : "failed", error: String(error) });
      throw error;
    }).finally(() => { if (pending.get(scope) === run) pending.delete(scope); notify(); });
    pending.set(scope, run); notify();
    return run.promise;
  }
  const cached = () => results.get(key()) || null;
  let pageGeneration = 0;
  async function loadStoredPage(offset) {
    if (!cached() || pending.has(scopeKey())) return;
    const generation=++pageGeneration, context=key(), level=minimumEvidence;
    const request=fullRequest(),owner=window.AnalysisContexts?.capture();
    const data=await api("triage",{...request,minimumEvidence:level,episodeOffset:offset,episodeLimit:20,tactic:tacticFilter,cacheOnly:true},{silent:true,analysisOwner:owner});
    analysisRequests.set(data,{request,owner});
    if(generation!==pageGeneration || context!==key() || level!==minimumEvidence)return;
    results.set(context,data);resultOwners.set(context,owner || {});remember(data);
    if(summarySlots?.attention?.isConnected){drawAttention(summarySlots.attention,data);if(summarySlots.entities)drawEntities(summarySlots.entities,data);}
  }
  function episodeRequest(data) {
    const captured = analysisRequests.get(data);
    if (!captured && window.AnalysisContexts) throw new Error("Reabra a análise antes de consultar suas evidências.");
    if (captured?.owner) window.AnalysisContexts.assertOwner(captured.owner);
    return captured || { request: fullRequest(), owner: null };
  }
  async function completeEpisode(data,episode) {
    if(episode.members_complete!==false)return episode.detections.map(i=>data.detections[i]);
    const captured=episodeRequest(data),detections=[];let offset=0;
    do {const page=await api("triage_episode",{...captured.request,analysisId:data.analysis_id,episodeId:episode.id,offset,limit:100},{silent:true,analysisOwner:captured.owner});
      if(page.analysis_id!==data.analysis_id)throw new Error("A análise mudou durante a leitura das evidências");
      detections.push(...page.detections);offset=page.next_offset;
    } while(offset!=null);
    return detections;
  }
  document.addEventListener("workspace-context-change", () => {
    tacticFilter = null; shownEpisodes = 3; minimumEvidence = 5; universe = universeKey();
    for (const run of pending.values()) if (run.key !== key()) { run.cancelled = true; window.Tasks?.cancelLatest(run.latest); }
    notify();
  });
  document.addEventListener("task-state-change", event => {
    if (!event.detail?.operationId || event.detail.state !== "cancelling") return;
    for (const run of pending.values()) if (window.Tasks?.operationFor(run.latest) === event.detail.operationId) { run.cancelled = true; notify(); }
  });

  // ---------------------------------------------------------------- evidence
  function detectionFilters(d) {
    return d.event_refs?.length ? [{ column: "event_ref", op: "in_exact", value: d.event_refs.join("\n") }] : [...d.filters];
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
    const refs = episode.event_refs || [...new Set(episode.detections.flatMap(i => data.detections[i].event_refs || []))];
    return refs.length ? [{ column: "event_ref", op: "in_exact", value: refs.join("\n") }] : episode.detections.flatMap(i => detectionFilters(data.detections[i]));
  }
  const keepContext = () => state.filters.filter(f => f.column !== "timestamp");
  function showRecords(filters) { window.Workspace.applyFilters(filters, true); }
  function showTimeline(filters, start, end) {
    if (start != null) window.Workspace.focusTimeline?.(start, end ?? start);
    window.Workspace.applyFilters(filters, true, "timeline");
  }
  async function saveToCase(data, detections, title, summary, grouping) {
    try { episodeRequest(data); } catch (error) { toast(String(error), "err"); return; }
    if (workspaceScope() === "case") { toast("Estes registros já pertencem ao Caso.", "info"); return; }
    const contextBefore = universeKey();
    const ids = [...new Set(detections.flatMap(d => d.event_ids))];
    if (window.CaseEvidence?.active === true && (!ids.length || ids.length > 10000)) { toast("Selecione um achado com 1 a 10.000 registros de apoio para preservar.", "info"); return; }
    const membersById=new Map(), ambiguousIds=new Set();
    for(const d of detections)for(const member of d.evidence_members || []){if(!membersById.has(member.event_id))membersById.set(member.event_id,member);else if(membersById.get(member.event_id).event_ref!==member.event_ref)ambiguousIds.add(member.event_id);}
    if (window.CaseEvidence?.active === true) {
      try {
        const c = ensureCase(), owner = await window.AnalysisContexts.prepare(window.AnalysisContexts.capture(), { metadata: true });
        const current = () => activeCase() === c && workspaceScope() === "dataset" && universeKey() === contextBefore && window.AnalysisContexts.isCurrent(owner);
        if (!current()) throw Error("O conjunto mudou. Reabra o achado antes de preservá-lo.");
        const handles = ids.map(id => {
          const member = membersById.get(id);
          if (ambiguousIds.has(id) || !member?.event_ref) throw Error("A referência original do achado está ausente ou é ambígua.");
          return { id, eventRef: member.event_ref };
        });
        const actions = nativeEvidenceServices().actions, selected = await actions.selection(handles, { guard: current });
        if (!current()) throw Error("O conjunto mudou durante a preparação do achado.");
        const references = [...new Set(handles.map(row => row.eventRef))], filters = [{ column: "event_ref", op: "in_exact", value: references.join("\n") }];
        let start = Infinity, end = -Infinity; for (const detection of detections) { start = Math.min(start, detection.start ?? Infinity); end = Math.max(end, detection.end ?? -Infinity); }
        const artifact = registerCurrentArtifact(c);
        await actions.add(selected, { id: "i" + Date.now().toString(36) + Math.floor(Math.random() * 1e4), kind: "grupo", label: title, note: summary, createdAt: Date.now(),
          sourceFilters: filters, sourceSpec: structuredClone(state.currentArtifact?.source), foundCount: handles.length, tags: ["detecção"], relevance: detections.some(d => SEVERITY[d.severity]?.[1] >= 3) ? "importante" : "normal", origin: state.currentOrigin, artifactId: artifact?.id || state.currentArtifact?.id, stationId: null,
          detection: { ...evidence().exportMetadata(data, minimumEvidence, workspaceScope()), analyst_state: "unreviewed", ...(grouping ? { grouping: structuredClone(grouping) } : {}), detections: structuredClone(detections), start: Number.isFinite(start) ? start : null, end: Number.isFinite(end) ? end : null } }, { guard: current });
        if (artifact) queueCustody(artifact); updateCountsSafe(); window.WorkspaceContext?.refreshMembership?.(); toast("Achado salvo com suas ocorrências preservadas no Caso.", "ok");
      } catch (error) { toast(`Não foi possível preservar o achado: ${String(error.message || error)}`, "err"); }
      return;
    }
    const rows = [];
    for (const id of ids) { try {
      const member=membersById.get(id);
      if(!member)throw Error('Referência original não preservada neste achado histórico');
      const row = await api("triage_evidence_event", { analysisId:data.analysis_id,eventId:id,eventRef:member.event_ref }, { silent: true });
      if (row) rows.push(row);
    } catch (error) { toast("Nao foi possivel preservar todas as evidencias: " + String(error), "error"); return; } }
    if (universeKey() !== contextBefore) { toast("O conjunto mudou durante a coleta de evidencias.", "error"); return; }
    if (rows.length !== ids.length) { toast("Evidencias incompletas; achado nao salvo.", "error"); return; }
    const c = ensureCase();
    queueCustody(registerCurrentArtifact(c));
    let start=Infinity,end=-Infinity;
    for(const d of detections){start=Math.min(start,d.start??Infinity);end=Math.max(end,d.end??-Infinity);}
    const references = [...new Set(detections.flatMap(d => d.event_refs || []))];
    const filters = references.length ? [{ column: "event_ref", op: "in_exact", value: references.join("\n") }] : detections.flatMap(detectionFilters);
    c.items.push({
      id: "i" + Date.now().toString(36) + Math.floor(Math.random() * 1e4), kind: "grupo", label: title, note: summary, createdAt: Date.now(),
      rows, sourceFilters: filters, sourceSpec: structuredClone(state.currentArtifact?.source),
      foundCount: ids.length, includedCount: rows.length, tags: ["detecção"], relevance: detections.some(d => SEVERITY[d.severity]?.[1] >= 3) ? "importante" : "normal",
      origin: state.currentOrigin, artifactId: state.currentArtifact?.id, stationId: null,
      detection: { ...evidence().exportMetadata(data, minimumEvidence, workspaceScope()), analyst_state: "unreviewed", ...(grouping?{grouping:structuredClone(grouping)}:{}), detections: structuredClone(detections), start: Number.isFinite(start) ? start : null, end: Number.isFinite(end) ? end : null },
    });
    if (await saveCases()) { window.Workspace?.loaded && updateCountsSafe(); window.WorkspaceContext?.refreshMembership?.(); toast("Salvo no Caso com os registros de apoio.", "ok"); }
  }
  const updateCountsSafe = () => { try { document.querySelector("#ws-evidence-count").textContent = activeCase()?.items?.length || ""; } catch { /* navigation not ready */ } };
  async function suppress(detection, data) {
    let owner;
    try { owner = await window.AnalysisContexts.prepare(episodeRequest(data).owner, { metadata: true }); } catch (error) { toast(String(error), "err"); return; }
    const settings = structuredClone((await rules(owner)).settings);
    const entity = detection.entities[0];
    const exception = await new Promise(resolve => {
      const dialog = document.createElement("dialog"); dialog.className = "evidence-exception";
      dialog.innerHTML = `<form><h3>Exceção de detecção</h3><p></p><label>Justificativa<textarea required maxlength="1000" name="reason"></textarea></label><label>Validade<select name="days"><option value="1">1 dia</option><option value="7" selected>7 dias</option><option value="30">30 dias</option></select></label><div class="modal-actions"><button type="button" class="btn ghost" data-cancel>Cancelar</button><button class="btn primary" type="submit">Aplicar exceção</button></div></form>`;
      dialog.querySelector("p").textContent = `${detection.name} · ${entity?.value || "todos os achados desta regra"} · escopo ${detection.namespace || "fonte sem namespace"}`;
      const close = value => { dialog.close(); dialog.remove(); resolve(value); };
      dialog.querySelector("[data-cancel]").onclick = () => close(null);
      dialog.oncancel = event => { event.preventDefault(); close(null); };
      dialog.querySelector("form").onsubmit = event => { event.preventDefault(); const form = new FormData(event.target), note = String(form.get("reason")).trim(); if (!note) return; close({ note, expires: Date.now() + Number(form.get("days")) * 86400000 }); };
      document.body.append(dialog); dialog.showModal(); dialog.querySelector("textarea").focus();
    });
    if (!exception) return;
    settings.suppress = [...(settings.suppress || []), { rule: detection.rule, column: entity?.column || null, value: entity?.value || null, scope: detection.namespace || "", created: Date.now(), ...exception }];
    window.AnalysisContexts.assertOwner(owner);
    await api("detection_settings_save", { settings }, { analysisOwner: owner });
    window.AnalysisContexts.assertOwner(owner, { revisions: false });
    invalidate(); rulesCache = null;
    toast(entity ? `Ocultado para ${entity.value}.` : "Detecção ocultada.", "ok");
    window.Workspace.showPage(window.Workspace.page());
  }

  // ---------------------------------------------------------------- attention
  function openInlineEvents(node, detection, data) {
    const host=node.querySelector('.sec-inline-events');
    host.hidden=!host.hidden;
    node.querySelector('[data-act="d-records"]').setAttribute('aria-expanded',String(!host.hidden));
    if(host.hidden)return;
    const article=node.closest('.sec-episode');
    article.__previewDetection=detection;
    article.querySelector('.sec-evidence-preview').innerHTML=preview(detection,article.dataset.patternGroup==='true',3);
    if(host.childNodes.length)return;
    host.innerHTML=`<h4>Eventos originais</h4><div class="sec-event-list"></div>`;
    const members=[...new Map((detection.evidence_members || []).map(m=>[m.event_ref,m])).values()];
    if(!members.length) {host.querySelector('.sec-event-list').textContent='Este achado histórico não preservou referências completas aos eventos.';return;}
    const context=key(), request=fullRequest(), list=host.querySelector('.sec-event-list'); let offset=0;
    const appendPage=()=>{
      host.querySelector('[data-more-events]')?.remove();
      for(const member of members.slice(offset,offset+20)) {
        const details=el('details','sec-event');
        details.innerHTML=`<summary>Evento ${esc(member.event_id)} <code>${esc(member.event_ref)}</code></summary><div class="sec-event-content"></div>`;
        const body=details.querySelector('.sec-event-content');let loading=false,loaded=false;
        details.addEventListener('toggle',async()=>{
          if(!details.open || loading || loaded)return;
          loading=true;body.textContent='Carregando evento original…';
          try {
            const event=await api('triage_evidence_event',{analysisId:data.analysis_id,eventRef:member.event_ref,eventId:member.event_id,...request},{silent:true});
            if(!details.isConnected || context!==key())return;
            if(!event)throw Error('Evento original indisponível');
            const safe=evidence().redact(event);
            const fields=Object.entries(safe.fields || {});
            const meta=Object.entries(safe).filter(([k])=>!['fields','raw'].includes(k));
            const rows=entries=>entries.map(([k,v])=>`<div class="sec-event-field"><dt>${esc(k)}</dt><dd><pre>${esc(typeof v==='string'?v:JSON.stringify(v,null,2))}</pre></dd></div>`).join('');
            body.innerHTML=`<h5>Evento</h5><dl>${rows(meta)}</dl><h5>Campos (${fields.length})</h5><dl>${rows(fields)}</dl><details class="sec-raw"><summary>Conteúdo bruto original</summary><pre>${esc(safe.raw || '(não registrado)')}</pre></details>`;
            loaded=true;
          }catch(error){body.textContent=`Não foi possível abrir este evento: ${error}`;const retry=el('button','text-button','Tentar novamente');retry.onclick=()=>{details.open=false;requestAnimationFrame(()=>details.open=true);};body.append(retry);}
          finally{loading=false;}
        });
        details.oncontextmenu=e=>{e.preventDefault();e.stopPropagation();showCtxMenu(e.clientX,e.clientY,[{icon:'fa-filter',label:'Filtrar este evento no Explorar',onClick:()=>showRecords([{column:'event_ref',op:'in_exact',value:member.event_ref}])}]);};
        list.append(details);
      }
      offset+=20;
      if(offset<members.length){const more=el('button','btn ghost',`Mais eventos (${members.length-offset} restantes)`);more.dataset.moreEvents='';more.onclick=appendPage;host.append(more);}
    };
    appendPage();
  }

  function tacticsStrip(data) {
    const tactics = data.tactics.map(t => ({ ...t, count: t.counts_by_level ? t.counts_by_level.slice(minimumEvidence-1).reduce((a,b)=>a+b,0) : data.detections.filter(d => visibleDetection(d) && d.tactics.includes(t.key)).length }));
    const max = Math.max(1, ...tactics.map(t => t.count));
    return `<div class="sec-tactics" role="group" aria-label="Táticas MITRE ATT&CK">${tactics.map(t => {
      const level = t.count ? Math.ceil(4 * t.count / max) : 0;
      const tips = t.techniques.slice(0, 4).map(x => `${x.id} ${x.name}`).join("\n");
      return `<button type="button" class="sec-tactic l${level}${tacticFilter === t.key ? " on" : ""}" data-tactic="${esc(t.key)}" ${t.count ? "" : "disabled"} title="${esc(`${t.label}${t.count ? ` · ${t.count} ${t.count === 1 ? "detecção" : "detecções"}` : ""}${tips ? `\n${tips}` : ""}`)}" aria-label="${esc(t.label)}"></button>`;
    }).join("")}</div>`;
  }
  function preview(detection, grouped, limit=1) {
    return `${grouped ? `<small class="sec-preview-source">Trecho da ocorrência · Evento ${esc(detection.event_ids?.[0] ?? '—')} · ${esc(range(detection.start,detection.end))}</small>` : ''}${evidence().excerpts(detection,limit)}`;
  }
  function episodeRow(data, episode, index) {
    const detections = episode.detections.map(i => data.detections[i]);
    const lead=detections.slice().sort((a,b)=>b.evidence_level-a.evidence_level)[0] || {};
    const records = episode.record_count ?? episode.event_refs?.length ?? new Set(detections.flatMap(d => d.event_ids)).size;
    const chain = episode.tactics.map(t => tacticLabel(data, t)).join(" → ");
    const grouped=episode.grouping?.kind==='pattern',occurrences=episode.grouping?.occurrence_count;
    const content=`${evidence().badge(episode.evidence_level)}${icon("fa-circle-info", "Entender a classificação e ver verificações", "explain")}<small class="evidence-impact">Impacto potencial: ${esc(sevLabel(episode.severity))} · ${esc(evidence().claim(lead.claim))} · ${esc(evidence().outcome(lead.outcome))}</small><h3>${esc(episode.title)}</h3>${grouped ? `<button class="sec-pattern-count" data-act="expand" title="Mesmo padrão e contexto. A repetição não aumenta o nível nem demonstra vínculo entre os eventos."><i class="fas fa-layer-group" aria-hidden="true"></i>${fmtNum(occurrences)} ocorrências semelhantes <span>Ver ocorrências</span></button>` : ''}<div class="sec-evidence-preview">${preview(lead,grouped)}</div>
      <div class="sec-meta"><time>${esc(range(episode.start, episode.end))}</time><span>${fmtNum(episode.detection_count ?? detections.length)} ${(episode.detection_count ?? detections.length) === 1 ? "detecção" : "detecções"} · ${fmtNum(records)} ${records === 1 ? "registro" : "registros"}</span>${chain ? `<span class="sec-chain">${esc(chain)}</span>` : ""}<span class="sec-entities"></span></div>`;
    return `<article class="sec-episode sev-${esc(episode.severity)}" data-episode="${index}" data-pattern-group="${grouped}" tabindex="0" aria-expanded="false">
      <span class="sec-bar" title="Severidade ${esc(sevLabel(episode.severity))}"></span>
      <div class="sec-main">${ParticipantsUI.cards(episode.participants,content)}
        <div class="sec-detections" hidden></div></div>
      <div class="sec-actions">${icon("fa-chevron-down", "Abrir indícios e eventos", "expand")}${workspaceScope() === "dataset" ? icon("fa-bookmark", "Salvar no Caso", "save") : ""}</div></article>`;
  }
  function detectionRows(data, episode) {
    return episode.detections.map(i => {
      const d = data.detections[i];
      if(episode.grouping?.kind==='pattern') {
        const severalRules=episode.detection_count>episode.grouping.occurrence_count;
        return `<div class="sec-detection sec-occurrence sev-${esc(d.severity)}" data-detection="${i}">
          <span class="sec-dot"></span><div><strong>Evento ${esc(d.event_ids?.[0] ?? '—')}</strong>${severalRules ? `<small>${evidence().badge(d.evidence_level)}${esc(d.name)}</small>` : ''}${!visibleDetection(d) ? '<small>Fora do recorte de exibição</small>' : ''}</div>
          <time>${esc(range(d.start,d.end))}</time><span class="sec-row-actions">${icon("fa-circle-info", "Verificações desta ocorrência", "d-explain")}<button class="btn ghost" data-act="d-records" aria-expanded="false">Ver evento</button></span><div class="sec-inline-events" hidden></div></div>`;
      }
      return `<div class="sec-detection sev-${esc(d.severity)}" data-detection="${i}">
        <span class="sec-dot"></span><div>${evidence().badge(d.evidence_level)}${icon("fa-circle-info", "Entender a classificação e ver verificações", "d-explain")}${!visibleDetection(d) ? '<small>Contexto da correlação</small>' : ""}<strong>${esc(d.name)}</strong><small>${esc(evidence().claim(d.claim))} · ${esc(evidence().outcome(d.outcome))}</small></div>
        <span class="sec-count">${fmtNum(d.count)}</span><time>${esc(range(d.start, d.end))}</time>
        <span class="sec-row-actions"><button class="btn ghost" data-act="d-records" aria-expanded="false">Ver eventos</button>${icon("fa-eye-slash", "Ocultar esta detecção", "d-hide")}</span><div class="sec-inline-events" hidden></div></div>`;
    }).join("");
  }
  function drawAttention(slot, data) {
    syncUniverse();
    const eligibleEpisodes = data.episodes.filter(e => e.detections.some(i => visibleDetection(data.detections[i])));
    const matching = data.episodes.map((e, i) => ({ e, i })).filter(({ e }) => eligibleEpisodes.includes(e) && (!tacticFilter || e.tactics.includes(tacticFilter)));
    const controls = previousNotice() + '<div class="sec-calculation-actions"><button type="button" class="btn ghost small" data-calculate-compromises>Recalcular comprometimentos</button></div>' + evidence().control(data, minimumEvidence);
    slot.dataset.analysisState = "ready"; slot.removeAttribute("aria-busy");
    const bindRigidity = () => { bindCalculation(slot); slot.querySelectorAll("[data-evidence-min]").forEach(b => b.onclick = () => setMinimum(+b.dataset.evidenceMin)); };
    const stored=data.storage?.kind==="sqlite";
    const episodes = stored ? matching : matching.slice(0, shownEpisodes);
    const hidden = matching.length - episodes.length;
    const rulesText = `${fmtNum(data.rules)} regras${data.threat_rules ? ` · ${fmtNum(data.threat_rules)} sinais` : ""}${data.sigma_rules ? ` · ${fmtNum(data.sigma_rules)} Sigma` : ""}`;
    if (!eligibleEpisodes.length) {
      slot.className = "sec-clear";
      const gaps = (data.rule_coverage || []).filter(r => ["partial", "missing_fields", "missing_coverage"].includes(r.status));
      slot.innerHTML = `${controls}<div class="evidence-empty"><strong>Nenhum indício no nível ${evidence().label(minimumEvidence)}${minimumEvidence < 5 ? " ou acima" : ""} encontrado neste conjunto.</strong><p>${(data.counts_by_level || []).slice(0, minimumEvidence - 1).some(Boolean) ? "Existem indícios em outros níveis. Selecione um nível abaixo para investigar." : "A ausência de indícios não demonstra ausência de comprometimento."}</p><small>Cobertura: ${data.limited || gaps.length ? "parcial" : "conforme os campos disponíveis"}${gaps.length ? ` · ${gaps.length} regras com dados insuficientes` : ""}. ${esc((data.limitations || []).join(" · "))}</small></div><small>${esc(rulesText)} · ${fmtNum(data.total)} registros analisados no universo completo</small>`;
      bindRigidity();
      if(tacticFilter){const clear=el('button','text-button',`Remover seleção de tática: ${tacticLabel(data,tacticFilter)}`);clear.onclick=()=>{tacticFilter=null;data.storage?.kind==='sqlite'?loadStoredPage(0).catch(e=>toast(String(e),'err')):drawAttention(slot,data);};slot.append(clear);}
      slot.hidden = false;
      slot.querySelector("[data-open-rules]")?.addEventListener("click", () => openSettings("detection"));
      return;
    }
    slot.className = "ws-card sec-attention";
    slot.innerHTML = `${controls}<div class="card-heading"><h2>Indícios de comprometimento</h2>${tacticsStrip(data)}<span class="sec-heading-count">${stored ? `Cartões ${data.page.episode_offset+1}–${data.page.episode_offset+data.page.returned_episodes} de ${fmtNum(data.page.total_episodes)}` : `${fmtNum(eligibleEpisodes.length)} cartões visíveis · ${fmtNum(data.episodes.length)} disponíveis`}</span></div>
      ${tacticFilter ? `<div class="sec-filtering">${esc(tacticLabel(data, tacticFilter))} <button type="button" class="text-button" data-clear-tactic>mostrar todos</button></div>` : ""}
      <div class="sec-episodes">${episodes.map(({ e, i }) => episodeRow(data, e, i)).join("")}</div>
      ${stored ? `<div class="sec-pagination"><button class="btn ghost" data-page-offset="${Math.max(0,data.page.episode_offset-data.page.episode_limit)}" ${data.page.episode_offset===0?"disabled":""}>Anterior</button><button class="btn ghost" data-page-offset="${data.page.next_offset??0}" ${data.page.next_offset==null?"disabled":""}>Próxima</button></div>` : ""}
      ${hidden > 0 ? `<button type="button" class="sec-more-episodes" data-more="expand">Mostrar mais ${fmtNum(Math.min(20, hidden))} ${hidden === 1 ? "episódio" : "episódios"}</button>` : shownEpisodes > COLLAPSED && matching.length > COLLAPSED ? '<button type="button" class="sec-more-episodes" data-more="collapse">Mostrar menos</button>' : ""}
      <div class="sec-foot"><span>${esc(rulesText)} · ${fmtNum(data.total)} registros${data.elapsed_ms ? ` em ${(data.elapsed_ms / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} s` : ""}</span>${data.limited ? '<span class="sec-limited" title="Algum orçamento de memória foi atingido; refine o período para análise completa.">parcial</span>' : ""}${data.suppressed ? `<button type="button" class="text-button" data-open-rules>${fmtNum(data.suppressed)} ocultas</button>` : '<button type="button" class="text-button" data-open-rules>Regras</button>'}</div>`;
    slot.hidden = false;
    for (const article of slot.querySelectorAll("[data-episode]")) {
      const episode = data.episodes[+article.dataset.episode];
      const holder = article.querySelector(".sec-entities");
      for (const entity of episode.entities.slice(0, 3)) holder.append(window.EntityMenu.chip({ ...entity, first: episode.start, last: episode.end }, "small"));
    }
    bindRigidity();
    bindAttention(slot, data);
  }
  function bindAttention(slot, data) {
    slot.querySelectorAll("[data-page-offset]").forEach(b=>b.onclick=()=>loadStoredPage(+b.dataset.pageOffset).catch(e=>toast(String(e),"err")));
    slot.querySelectorAll("[data-tactic]").forEach(b => b.onclick = () => { tacticFilter = tacticFilter === b.dataset.tactic ? null : b.dataset.tactic; if(data.storage?.kind==="sqlite")loadStoredPage(0).catch(e=>toast(String(e),"err"));else drawAttention(slot, data); });
    slot.querySelector("[data-clear-tactic]")?.addEventListener("click", () => { tacticFilter = null; if(data.storage?.kind==="sqlite")loadStoredPage(0).catch(e=>toast(String(e),"err"));else drawAttention(slot, data); });
    slot.querySelectorAll("[data-open-rules]").forEach(b => b.onclick = () => openSettings("detection"));
    slot.querySelector("[data-more]")?.addEventListener("click", () => { shownEpisodes = slot.querySelector("[data-more]").dataset.more === "expand" ? shownEpisodes + 20 : COLLAPSED; drawAttention(slot, data); });
    const episodeOf = node => data.episodes[+node.closest("[data-episode]").dataset.episode];
    const loadMembers = async (article,offset) => {
      const episode=episodeOf(article),limit=episode.grouping?.kind==='pattern'?20:100;
      const captured=episodeRequest(data);
      const page=await api("triage_episode",{...captured.request,analysisId:data.analysis_id,episodeId:episode.id,offset,limit},{silent:true,analysisOwner:captured.owner});
      if (!article.isConnected) return;
      article.__detailMembers=page.detections.map(d=>({...d,context_only:(d.evidence_level||0)<minimumEvidence || !!(tacticFilter&&!d.tactics.includes(tacticFilter))}));
      article.querySelector(".sec-detections").innerHTML=detectionRows({...data,detections:article.__detailMembers},{...episode,detections:page.detections.map((_,i)=>i)})+`<p class="small">Indícios ${offset+1}–${offset+page.detections.length} de ${page.total}</p>${offset>0?`<button class="btn ghost" data-members-offset="${Math.max(0,offset-limit)}">Anteriores</button>`:""}${page.next_offset!=null?`<button class="btn ghost" data-members-offset="${page.next_offset}">Próximos indícios</button>`:""}`;
    };
    const toggle = async article => {
      const list = article.querySelector(".sec-detections"), open = list.hidden;
      list.hidden = !open; article.setAttribute("aria-expanded", String(open));
      article.querySelectorAll('[data-act="expand"]').forEach(b=>b.setAttribute('aria-expanded',String(open)));
      const groupLabel=article.querySelector('.sec-pattern-count span');
      if(groupLabel)groupLabel.textContent=open?'Ocultar ocorrências':'Ver ocorrências';
      if (open && !list.innerHTML) {
        const episode=episodeOf(article);
        if(episode.members_complete===false || (episode.grouping?.kind==='pattern' && data.storage?.kind==='sqlite' && episode.detection_count>20)){list.textContent="Carregando ocorrências…";try{await loadMembers(article,0);}catch(error){list.innerHTML="";toast(String(error),"err");}}
        else list.innerHTML = detectionRows(data, episode);
      }
    };
    slot.querySelector(".sec-episodes")?.addEventListener("click", async event => {
      const article = event.target.closest("[data-episode]");
      if (!article || event.target.closest(".entity-chip, .evidence-why, .sec-inline-events, .sec-excerpts") || window.getSelection()?.toString()) return;
      const action = event.target.closest("[data-act]")?.dataset.act;
      let episode = episodeOf(article);
      if(action==='participants') {
        ParticipantsUI.inspect(episode.participants,event.target.closest('[data-side]').dataset.side,{analysisId:data.analysis_id,request:fullRequest()});
        return;
      }
      const membersPage=event.target.closest("[data-members-offset]");
      if(membersPage){
        try {await loadMembers(article,+membersPage.dataset.membersOffset);}catch(error){toast(String(error),"err");}return;
      }
      let actionData=data;
      if(action && episode.members_complete===false && ["records","timeline","save"].includes(action)) {
        try {const members=await completeEpisode(data,episode);actionData={...data,detections:members};analysisRequests.set(actionData,episodeRequest(data));episode={...episode,detections:members.map((_,i)=>i),event_refs:[...new Set(members.flatMap(d=>d.event_refs||[]))],members_complete:true};}
        catch(error){toast(String(error),"err");return;}
      }
      const detection = event.target.closest("[data-detection]") && (article.__detailMembers || data.detections)[+event.target.closest("[data-detection]").dataset.detection];
      if (action === "explain") evidence().inspect(article.__previewDetection || episode.detections.map(i=>data.detections[i]).sort((a,b)=>b.evidence_level-a.evidence_level)[0]);
      else if (action === "d-explain" && detection) evidence().inspect(detection);
      else if (action === "records") showRecords(episodeFilters(actionData, episode));
      else if (action === "timeline") showTimeline(episodeFilters(actionData, episode), episode.start, episode.end);
      else if (action === "save") await saveToCase(actionData, episode.detections.map(i => actionData.detections[i]), episode.title, episode.summary, episode.grouping);
      else if (action === "d-records" && detection) openInlineEvents(event.target.closest('[data-detection]'), detection, data);
      else if (action === "d-hide" && detection) await suppress(detection, data);
      else if (!detection) toggle(article);
      else openInlineEvents(event.target.closest('[data-detection]'), detection, data);
    });
    slot.querySelector(".sec-episodes")?.addEventListener("keydown", event => {
      const article = event.target.closest("[data-episode]");
      if (article && event.target === article && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); toggle(article); }
    });
    slot.querySelector(".sec-episodes")?.addEventListener("contextmenu", async event => {
      if (event.target.closest('.sec-event')) return;
      const article = event.target.closest("[data-episode]");
      if (!article || event.target.closest(".entity-chip, .evidence-why")) return;
      event.preventDefault();
      let episode = episodeOf(article), actionData=data;
      const detectionNode = event.target.closest("[data-detection]");
      const detection = detectionNode && (article.__detailMembers || data.detections)[+detectionNode.dataset.detection];
      if(!detection && episode.members_complete===false){
        try {const members=await completeEpisode(data,episode);actionData={...data,detections:members};analysisRequests.set(actionData,episodeRequest(data));episode={...episode,detections:members.map((_,i)=>i),event_refs:[...new Set(members.flatMap(d=>d.event_refs||[]))]};}
        catch(error){toast(String(error),"err");return;}
      }
      const target = detection ? { title: detection.name, summary: detection.summary, list: [detection], filters: detectionFilters(detection), start: detection.start, end: detection.end } : { title: episode.title, summary: episode.summary, list: episode.detections.map(i => actionData.detections[i]), filters: episodeFilters(actionData, episode), start: episode.start, end: episode.end };
      showCtxMenu(event.clientX, event.clientY, [
        { icon: "fa-filter", label: "Filtrar eventos no Explorar", onClick: () => showRecords(target.filters) },
        { icon: "fa-timeline", label: "Ver na linha do tempo", onClick: () => showTimeline(target.filters, target.start, target.end) },
        ...(workspaceScope() === "dataset" ? [{ icon: "fa-bookmark", label: "Salvar no Caso", onClick: () => saveToCase(actionData, target.list, target.title, target.summary, detection?undefined:episode.grouping) }] : []),
        { icon: "fa-copy", label: "Copiar resumo", onClick: () => navigator.clipboard?.writeText(`${target.title}\n${target.summary}\n${range(target.start, target.end)}\n${target.list.map(d => `- ${d.name}: ${d.summary} (${d.count})`).join("\n")}`) },
        ...(detection ? [{ sep: true }, { icon: "fa-eye-slash", label: "Ocultar esta detecção", onClick: () => suppress(detection, data) }] : []),
      ]);
    });
  }

  // ---------------------------------------------------------------- entities and rarity
  function drawEntities(slot, data) {
    data = { ...data, entities: data.entities.filter(e => (e.evidence_level || 0) >= minimumEvidence) };
    if (!data.entities.length) { slot.hidden = true; return; }
    slot.className = "ws-card sec-entities-card";
    slot.innerHTML = `<div class="card-heading"><h2>Entidades em destaque</h2><span>por força da evidência</span></div><div class="sec-entity-list"></div>`;
    const list = slot.querySelector(".sec-entity-list");
    for (const e of data.entities.slice(0, 8)) {
      const row = el("button", `sec-entity risk-${e.level === "alto" ? "high" : e.level === "médio" ? "medium" : "low"}`);
      row.type = "button";
      row.innerHTML = `<i class="fas ${window.EntityMenu.icon(e.column)}" aria-hidden="true"></i><span class="sec-entity-name"><strong></strong><small></small></span><span class="sec-risk" aria-hidden="true"><b style="width:${Math.max(6, e.score)}%"></b></span><span class="sec-score">${e.score}</span>`;
      row.querySelector("strong").textContent = e.value;
      row.querySelector("small").textContent = `${evidence().label(e.evidence_level)} · ${e.namespace || e.scope || e.label} · ${e.detections} ${e.detections === 1 ? "detecção" : "detecções"}`;
      row.title = `${e.label}: ${e.value}\nRisco ${e.score}/100 (${e.level})\n${fmtNum(e.events)} registros${e.failures ? ` · ${fmtNum(e.failures)} falhas` : ""}${e.first != null ? `\n${range(e.first, e.last)}` : ""}${e.tactics.length ? `\n${e.tactics.map(t => tacticLabel(data, t)).join(" · ")}` : ""}`;
      const act = event => { event.preventDefault(); window.EntityMenu.open(event.clientX, event.clientY, e); };
      row.onclick = act; row.oncontextmenu = act;
      list.append(row);
    }
    slot.hidden = false;
  }
  function drawRare(slot, data, expanded = false) {
    if (minimumEvidence !== 1 || !data.rare.length) { slot.hidden = true; return; }
    const shown = expanded ? data.rare : data.rare.slice(0, 6);
    slot.className = "ws-card sec-rare-card";
    slot.innerHTML = `<div class="card-heading"><h2>Contexto estatístico</h2><span title="Valores que quase nunca aparecem entre muitos registros do mesmo tipo">raridade não é indício</span></div>${shown.map((r, i) => `<button type="button" class="sec-rare" data-rare="${i}" title="${esc(`${r.label}: ${r.value}\n${r.count} de ${fmtNum(r.role_events)} registros · ${fmtNum(r.role_distinct)} valores distintos${r.first != null ? `\nPrimeira vez: ${shortTime(r.first)}` : ""}`)}"><code></code><small>${esc(r.label)}</small><span>${fmtNum(r.count)}</span></button>`).join("")}${shown.length < data.rare.length ? `<button type="button" class="text-button sec-more">Mais ${data.rare.length - shown.length}</button>` : ""}`;
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
    let revision=0;
    const draw = async data => {
      const mine=++revision,context=key(),level=minimumEvidence;
      host.querySelectorAll(".sec-markers").forEach(n => n.remove());
      if (!host.isConnected || !data || !(end > start)) return;
      if(data.storage?.kind==="sqlite") {
        try {
          const timeline=await api("triage_timeline",{...analyticsRequest(workspaceScope()),minimumEvidence:level,start,end},{silent:true});
          if(mine!==revision || context!==key() || level!==minimumEvidence || !host.isConnected)return;
          if(!timeline.count)return;
          const strip=el("div","sec-markers");strip.setAttribute("aria-label",`${fmtNum(timeline.count)} indícios no período — resultado completo`);
          const groups=new Map();for(const bin of timeline.bins)(groups.get(bin.slot)||groups.set(bin.slot,[]).get(bin.slot)).push(bin);
          for(const [slot,bins] of groups){
            const strength=Math.max(...bins.map(b=>b.evidence_level)),from=Math.min(...bins.map(b=>b.start)),to=Math.max(...bins.map(b=>b.end));
            const mark=el("button",`sec-marker evidence-marker-e${strength}`);mark.type="button";mark.style.left=`${slot/2}%`;
            mark.title=bins.map(b=>`${evidence().label(b.evidence_level)}: ${fmtNum(b.count)} indícios`).join("\n");mark.setAttribute("aria-label",mark.title);
            mark.onclick=()=>onPick(from,to,[]);mark.oncontextmenu=event=>{event.preventDefault();showCtxMenu(event.clientX,event.clientY,[{icon:"fa-timeline",label:"Selecionar intervalo dos indícios",onClick:()=>onPick(from,to,[])}]);};strip.append(mark);
          }
          if(before?.parentElement===host)host.insertBefore(strip,before);else host.append(strip);
        }catch(error){if(mine===revision && host.isConnected){const failure=el("div","sec-markers","Não foi possível consultar os indícios do período");failure.title=String(error);host.append(failure);}}
        return;
      }
      if(!data.detections?.length)return;
      const visible = data.detections.filter(d => visibleDetection(d) && d.start != null && d.end >= start && d.start <= end);
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
        list.sort((a, b) => b.evidence_level - a.evidence_level || (SEVERITY[b.severity]?.[1] || 0) - (SEVERITY[a.severity]?.[1] || 0));
        const mark = el("button", `sec-marker evidence-marker-e${list[0].evidence_level}`);
        mark.type = "button";
        mark.style.left = `${slot / 2}%`;
        mark.title = list.slice(0, 6).map(d => `${evidence().label(d.evidence_level)} · ${d.name} · ${d.summary}`).join("\n") + (list.length > 6 ? `\n+${list.length - 6}` : "");
        mark.setAttribute("aria-label", mark.title);
        mark.onclick = () => onPick(Math.min(...list.map(d => d.start)), Math.max(...list.map(d => d.end ?? d.start)), list);
        mark.oncontextmenu = event => { event.preventDefault(); showCtxMenu(event.clientX, event.clientY, list.slice(0, 8).map(d => ({ icon: "fa-list", label: `${d.name} (${fmtNum(d.count)})`, onClick: () => showRecords(detectionFilters(d)) }))); };
        strip.append(mark);
      }
      if (before && before.parentElement === host) host.insertBefore(strip, before); else host.append(strip);
    };
    const refresh = () => { if (host.isConnected) draw(cached()); else { document.removeEventListener("evidence-rigidity-change", refresh); document.removeEventListener("security-analysis-change", refresh); } };
    document.addEventListener("evidence-rigidity-change", refresh);
    document.addEventListener("security-analysis-change", refresh);
    draw(cached());
  }

  // ---------------------------------------------------------------- summary
  function bindCalculation(slot) {
    slot.querySelector("[data-calculate-compromises]")?.addEventListener("click", () => { get({ force: !!cached() }).catch(() => {}); });
    slot.querySelector("[data-cancel-compromises]")?.addEventListener("click", cancel);
  }
  function previousNotice() {
    const current = status();
    if (!cached() || !["failed", "cancelled"].includes(current.state)) return "";
    const message = current.state === "cancelled" ? "Recálculo cancelado." : `O recálculo falhou: ${current.error}`;
    return `<p class="small muted" role="status">${esc(message)} Exibindo o resultado válido anterior.</p>`;
  }
  function drawStatus(slot, summary = false) {
    const current = status();
    if (current.state === "ready" || cached() && ["failed", "cancelled"].includes(current.state)) return false;
    const busy = ["calculating", "cancelling"].includes(current.state);
    const labels = { idle: "Comprometimentos ainda não calculados", stale: "Resultado desatualizado", calculating: "Calculando comprometimentos…", cancelling: "Cancelando cálculo…", cancelled: "Cálculo cancelado", failed: "Não foi possível calcular os comprometimentos" };
    const detail = current.state === "stale" ? "Os dados ou as regras mudaram. Calcule novamente para ver os indícios atuais."
      : busy ? "Analisando o conjunto completo. Você pode continuar usando as outras páginas."
      : current.state === "failed" ? current.error : current.state === "cancelled" ? "Nenhum resultado parcial foi aplicado. Você pode iniciar o cálculo novamente."
      : "O cálculo só começa quando você pedir. Importar logs ou abrir esta página não inicia a análise.";
    slot.className = summary ? "ws-card sec-summary sec-calculation-state" : "ws-card sec-calculation-state";
    slot.dataset.analysisState = current.state; slot.setAttribute("aria-busy", String(busy)); slot.hidden = false;
    slot.innerHTML = `<div class="card-heading"><h2>${summary ? "Comprometimentos" : labels[current.state]}</h2></div><p role="status">${summary ? `<strong>${labels[current.state]}</strong><br>` : ""}${esc(detail)}</p>${busy ? `<button type="button" class="btn ghost small" data-cancel-compromises ${current.state === "cancelling" ? "disabled" : ""}>${current.state === "cancelling" ? "Aguardando cancelamento" : "Cancelar cálculo"}</button>` : `<button type="button" class="btn primary small" data-calculate-compromises>${["stale", "failed", "cancelled"].includes(current.state) ? "Calcular novamente" : "Calcular comprometimentos"}</button>`}`;
    bindCalculation(slot); return true;
  }
  function registerView(attention, summary, extras = {}) {
    for (const slot of views.keys()) if (!slot.isConnected) views.delete(slot);
    views.set(attention, { scope: scopeKey(), summary, ...extras });
    drawView(attention, views.get(attention));
  }
  function drawView(attention, view) {
    if (!attention.isConnected || view.scope !== scopeKey()) return;
    syncUniverse();
    if (drawStatus(attention, view.summary)) return;
    const data = cached();
    if (view.summary) drawSummary(attention, data);
    else {
      summarySlots = { attention, entities: view.entities, rare: view.rare };
      drawAttention(attention, data);
      if (view.entities) drawEntities(view.entities, data);
      if (view.rare) drawRare(view.rare, data);
    }
    updateContextBar();
  }
  document.addEventListener("security-analysis-change", () => {
    for (const [slot, view] of views) { if (!slot.isConnected) views.delete(slot); else drawView(slot, view); }
  });
  async function renderPage(host) {
    host.innerHTML = `<section class="sec-intro"><div><p>Calcule os indícios quando quiser investigar o conjunto completo.<br>Abra um indício para percorrer as evidências até os eventos originais.</p></div><small>Força da evidência, impacto potencial e resultado são informações distintas. “Quase confirmado” também pode descrever uma tentativa muito específica, inclusive bloqueada.</small></section><div class="sec-page-results" data-compromises-results></div>`;
    registerView(host.querySelector('[data-compromises-results]'), false);
  }
  async function fillSummary({ attention }) {
    if (attention) registerView(attention, true);
  }
  function drawSummary(attention, data) {
    attention.hidden = false; attention.className = 'ws-card sec-summary'; attention.dataset.analysisState = "ready"; attention.removeAttribute("aria-busy");
    const counts = data.universe_counts_by_level || data.counts_by_level || [0,0,0,0,0], max = Math.max(1,...counts);
    attention.innerHTML = `${previousNotice()}<div class="card-heading"><h2>Comprometimentos</h2><button class="text-button" data-open-compromises>Abrir análise <i class="fas fa-arrow-right"></i></button></div><p class="small muted">Indícios por força da evidência · ${fmtNum(data.total)} registros no conjunto completo · filtros do Explorar não se aplicam</p><div class="sec-count-chart" role="group" aria-label="Número de indícios por força da evidência">${[5,4,3,2,1].map(n=>`<button class="sec-chart-row evidence-e${n}" data-chart-level="${n}" aria-label="${evidence().label(n)}: ${counts[n-1]} indícios"><span>${evidence().label(n)}</span><span class="sec-chart-track"><span style="width:${100*counts[n-1]/max}%"></span></span><strong>${fmtNum(counts[n-1])}</strong></button>`).join('')}</div>`;
    const open=async level=>{const hadTactic=!!tacticFilter;tacticFilter=null;await Workspace.showPage('compromises');if(level)setMinimum(level);else if(hadTactic&&cached()?.storage?.kind==='sqlite')loadStoredPage(0).catch(error=>toast(String(error),'err'));};
    attention.querySelector('[data-open-compromises]').onclick=()=>open();
    attention.querySelectorAll('[data-chart-level]').forEach(b=>b.onclick=()=>open(+b.dataset.chartLevel));
  }

  // ---------------------------------------------------------------- rules and settings
  let rulesCache = null;
  const ruleOwners = new WeakMap(), rulePanes = new WeakMap(), rulePaneStates = new WeakMap(), liveRulePanes = new Set(), controlDisabled = new WeakMap();
  function updateRulePaneState(pane) {
    const view = rulePaneStates.get(pane); if (!view) return;
    let stale = false; try { window.AnalysisContexts.assertOwner(view.owner); } catch { stale = true; }
    view.notice.innerHTML = `<p class="small muted">Segurança do Caso ${esc(view.owner.caseId || "sem identificação")}</p>${stale ? '<p role="alert">O Caso ou sua configuração mudou. Este formulário anterior está bloqueado.</p><button type="button" class="btn ghost small">Reabrir regras do Caso ativo</button>' : ""}`;
    const reopen = stale ? view.notice.querySelector("button") : null;
    if (reopen) reopen.onclick = () => renderRulesPane(pane);
    for (const control of pane.querySelectorAll("input, textarea, button")) {
      if (control === reopen) continue;
      if (stale) { if (!controlDisabled.has(control)) controlDisabled.set(control, control.disabled); control.disabled = true; }
      else if (controlDisabled.has(control)) { control.disabled = controlDisabled.get(control); controlDisabled.delete(control); }
    }
  }
  async function rules(owner) {
    owner ||= await window.AnalysisContexts.prepare(window.AnalysisContexts.capture(), { metadata: true });
    window.AnalysisContexts.assertOwner(owner);
    const signature = JSON.stringify(owner);
    if (!rulesCache || rulesCache.signature !== signature) {
      const value = await api("detection_rules", {}, { silent: true, analysisOwner: owner });
      window.AnalysisContexts.assertOwner(owner);
      ruleOwners.set(value, owner); rulesCache = { signature, value };
    }
    for (const r of rulesCache.value.rules) names.set(r.id, r.name);
    return rulesCache.value;
  }
  for (const event of ["workspace-context-change", "analysis-context-change"]) document.addEventListener(event, () => { rulesCache = null; names.clear(); for (const pane of liveRulePanes) { if (!pane.isConnected) liveRulePanes.delete(pane); else updateRulePaneState(pane); } });
  async function renderRulesPane(pane) {
    const token = {}; rulePanes.set(pane, token);
    pane.innerHTML = '<div class="ws-loading"><i class="fas fa-circle-notch spin"></i>Carregando regras…</div>';
    let overview;
    try { rulesCache = null; overview = await rules(); if (rulePanes.get(pane) !== token) return; } catch (error) { if (rulePanes.get(pane) !== token) return; pane.innerHTML = `<p class="muted small">${esc(String(error))}</p>`; return; }
    const settings = structuredClone(overview.settings);
    let owner = ruleOwners.get(overview);
    const mutate = async (command, args) => {
      window.AnalysisContexts.assertOwner(owner);
      const result = await api(command, structuredClone(args), { analysisOwner: owner });
      window.AnalysisContexts.assertOwner(owner, { revisions: false });
      owner = window.AnalysisContexts.capture(); if (rulePaneStates.has(pane)) { rulePaneStates.get(pane).owner = owner; updateRulePaneState(pane); } invalidate(); rulesCache = null;
      return result;
    };
    const save = async (message) => { await mutate("detection_settings_save", { settings }); if (message) toast(message, "ok"); };
    const groups = [["builtin", "Modelos internos"], ["case", "Regras deste Caso"], ["sigma", "Sigma deste Caso"]];
    pane.innerHTML = `<div class="rules-top"><label class="check-line"><input type="checkbox" id="rules-threats" ${settings.threats ? "checked" : ""}> Incluir sinais do catálogo de ameaças na triagem</label>
      <div class="rules-actions"><button type="button" class="btn ghost small" id="rules-import"><i class="fas fa-file-import"></i> Importar Sigma</button><button type="button" class="btn ghost small" id="rules-import-folder">Pasta Sigma</button><button type="button" class="btn ghost small" id="rules-clear" title="Remove apenas as fontes Sigma deste Caso; também permite redefinir uma importação legada inválida">Limpar Sigma do Caso</button></div></div>
      <input type="search" class="rules-search" placeholder="Filtrar regras…" aria-label="Filtrar regras">
      ${settings.suppress?.length ? `<details class="rules-suppress"><summary>${settings.suppress.length} ${settings.suppress.length === 1 ? "detecção oculta" : "detecções ocultas"}</summary>${settings.suppress.map((s, i) => `<div class="rules-suppressed"><span>${esc(names.get(s.rule) || s.rule)}${s.value ? ` · ${esc(s.value)}` : ""}</span><button type="button" class="text-button" data-unsuppress="${i}">Mostrar novamente</button></div>`).join("")}</details>` : ""}
      ${overview.sigma_errors.length ? `<details class="rules-errors"><summary>${overview.sigma_errors.length} regras Sigma não convertidas</summary>${overview.sigma_errors.slice(0, 50).map(e => `<div>${esc(e)}</div>`).join("")}</details>` : ""}
      <div class="rules-list">${groups.map(([origin, label]) => { const list = overview.rules.filter(r => r.origin === origin); return list.length ? `<h4>${label} <span>${list.filter(r => r.enabled).length}/${list.length}</span></h4>${list.map(r => `<label class="rule-row" data-search="${esc(`${r.name} ${r.id} ${r.attack.map(a => a.id + " " + a.name).join(" ")}`.toLowerCase())}" title="${esc(r.description)}"><input type="checkbox" data-rule="${esc(r.id)}" ${r.enabled ? "checked" : ""} ${r.retired ? 'disabled title="Retirada da triagem: comportamento insuficiente isoladamente"' : ""}><span class="sec-dot sev-${esc(r.severity)}"></span><span class="rule-name">${esc(r.name)}</span><small>${r.evidence?.maturity === "unassessed" ? "Nível não avaliado" : evidence().label(r.evidence_level ?? r.evidence?.level)} · ${esc(r.evidence?.maturity || "legada")}</small><small>${esc(r.attack.map(a => a.id).join(" "))}</small></label>`).join("")}` : ""; }).join("")}</div>`;
    const custom = el("details", "rules-mappings");
    custom.innerHTML = '<summary>Regras personalizadas deste Caso</summary><p class="small muted">JSON com version: 1 e rules. IDs iguais substituem o modelo somente neste Caso. Deixe vazio para usar somente os modelos internos.</p><textarea class="source-mapping-editor" aria-label="Regras personalizadas do Caso (JSON)"></textarea><button type="button" class="btn ghost small">Salvar regras do Caso</button><pre aria-live="polite"></pre>';
    custom.querySelector("textarea").value = overview.custom_rules_json || "";
    custom.querySelector("button").onclick = async () => { try { await mutate("detection_settings_save", { settings, customRulesJson: custom.querySelector("textarea").value }); if (pane.isConnected) await renderRulesPane(pane); } catch (error) { custom.querySelector("pre").textContent = String(error); } };
    pane.prepend(custom);
    const mappingsPane = el("details", "rules-mappings");
    mappingsPane.innerHTML = `<summary>Mapeamento de fontes e procedência</summary><p class="muted small">Cada fonte usa seu nome exato. Campos: timestamp, actor, target, namespace, host, service, action, outcome, request, session, connection, process, parent, file, command, credential, created_credential, resource, request_command, url, persistence_target, application, grant, token, repository, pipeline, run, revision, secret, certificate, certificate_issuer, certificate_subject, requester, beneficiary, delegator, resource_spn, destination, artifact, logon, remote_session, principal, source_address. Resultados: success, failure, blocked, unknown.</p><textarea class="source-mapping-editor" aria-label="Mapeamentos por fonte (JSON)"></textarea><button type="button" class="btn ghost small" data-preview-map>Prévia no evento aberto</button><button type="button" class="btn ghost small" data-save-map>Salvar mapeamentos</button><pre class="mapping-preview" aria-live="polite"></pre>`;
    pane.prepend(mappingsPane);
    const editor = mappingsPane.querySelector("textarea"), preview = mappingsPane.querySelector("pre");
    editor.value = JSON.stringify(settings.mappings || [], null, 2);
    const coveragePane=el("details","rules-mappings");
    coveragePane.innerHTML=`<summary>Cobertura explícita para regras por ausência</summary><p class="small muted">Declare somente intervalos completos comprovados. Cada declaração exige dataset_fingerprint, source, namespace, category, start/end em epoch ms, complete e justification. Uma declaração não vale para outro conjunto ou Caso.</p><code class="coverage-fingerprint"></code><textarea class="source-mapping-editor" aria-label="Cobertura por fonte (JSON)"></textarea><button type="button" class="btn ghost small" data-save-coverage>Salvar cobertura</button><pre aria-live="polite"></pre>`;
    coveragePane.querySelector("code").textContent=`Conjunto atual: ${cached()?.dataset_fingerprint || "Execute a triagem para obter a identificação"}`;
    coveragePane.querySelector("textarea").value=JSON.stringify(settings.coverage||[],null,2);
    coveragePane.querySelector("button").onclick=async()=>{try{const coverage=JSON.parse(coveragePane.querySelector("textarea").value);if(!Array.isArray(coverage))throw Error("Informe uma lista de declarações");await mutate("detection_settings_save",{settings:{...settings,coverage}});settings.coverage=coverage;invalidate();rulesCache=null;coveragePane.querySelector("pre").textContent="Cobertura salva para o conjunto identificado.";}catch(e){coveragePane.querySelector("pre").textContent=String(e);}};
    pane.prepend(coveragePane);
    mappingsPane.querySelector("[data-preview-map]").onclick = async () => {
      try { window.AnalysisContexts.assertOwner(owner); if (!state.currentDetailEv) throw Error("Abra um evento para conferir os campos reais da fonte."); const data = await api("normalization_preview", { event: state.currentDetailEv, mappings: JSON.parse(editor.value) }); window.AnalysisContexts.assertOwner(owner); preview.textContent = JSON.stringify(data, null, 2); } catch (e) { preview.textContent = String(e); }
    };
    mappingsPane.querySelector("[data-save-map]").onclick = async () => {
      try { const next = JSON.parse(editor.value); if (!Array.isArray(next)) throw Error("Informe uma lista de mapeamentos."); await mutate("detection_settings_save", { settings: { ...settings, mappings: next } }); settings.mappings = next; invalidate(); rulesCache = null; preview.textContent = "Mapeamentos salvos. A proxima analise usara a nova versao."; } catch (e) { preview.textContent = String(e); }
    };
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
      const result = await mutate("sigma_import", { paths: Array.isArray(chosen) ? chosen : [chosen] });
      invalidate();
      toast(`${fmtNum(result.rules)} regras Sigma importadas${result.failed.length ? ` · ${result.failed.length} arquivos não convertidos` : ""}.`, result.rules ? "ok" : "info");
      renderRulesPane(pane);
    };
    pane.querySelector("#rules-import").onclick = () => importFrom(false);
    pane.querySelector("#rules-import-folder").onclick = () => importFrom(true);
    pane.querySelector("#rules-clear")?.addEventListener("click", async () => { if (!confirm("Remover todas as regras Sigma importadas deste Caso?")) return; await mutate("sigma_clear", {}); invalidate(); renderRulesPane(pane); });
    const notice = el("div", "security-case-owner"); pane.prepend(notice);
    rulePaneStates.set(pane, { owner, notice }); liveRulePanes.add(pane); updateRulePaneState(pane);
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

  async function openEpisode(index) {
    const data = cached(), episode = data?.episodes?.[index];
    if (episode) { await Workspace.showPage('compromises'); document.querySelector(`[data-episode="${index}"] [data-act="expand"]`)?.click(); }
  }
  return { setMinimum, minimum: () => { syncUniverse(); return minimumEvidence; }, get, cached, fillSummary, renderPage, markers, renderRulesPane, recipes, recipesMenu, openEpisode, ruleName: id => names.get(id) || null, rules: async () => (await rules()).rules, invalidate, cancel, status, last: cached };
})();
