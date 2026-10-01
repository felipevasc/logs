/* Exact temporal grouping for Explore. Presentation choices never change its filters. */
window.ExplorerTimeline = (() => {
  "use strict";
  const COLORS = ["#2f6fed", "#18a999", "#d18c19", "#a46ee8", "#ec6d63", "#40a9db", "#d960a3", "#66b96a", "#cf7240", "#8393e8", "#b2a42b", "#d571d7"];
  const panel = $(".hist-panel"), box = $("#chart");
  const heading = el("div", "explorer-timeline-heading"), title = el("strong", "", "Timeline");
  const fieldLabel = el("span", "explorer-timeline-field"), clear = el("button", "btn ghost small", "Limpar agrupamento");
  const fieldControl = el("select", "explorer-timeline-field-select");
  fieldControl.setAttribute("aria-label", "Agrupar Timeline por campo");
  fieldControl.disabled = true;
  const control = el("button", "btn ghost small");
  const legend = el("div", "explorer-timeline-legend"), note = el("div", "explorer-timeline-note muted small");
  const countControl = el("select", "explorer-timeline-limit");
  countControl.setAttribute("aria-label", "Máximo de grupos na Timeline");
  for (const count of [6, 12, 24]) { const option = el("option", "", `${count} grupos`); option.value = String(count); countControl.append(option); }
  clear.type = control.type = "button"; note.setAttribute("role", "status");
  heading.append(title, fieldLabel, fieldControl, countControl, clear, control); panel.prepend(heading); panel.append(legend, note);
  panel.classList.add("explorer-timeline");
  let preferences = { field: null, limit: 12, hidden: [] };
  let stats = null, base = "", grouped = null, pending = null, serial = 0, status = "idle", error = null, manualPause = false, plotted = "";
  let fieldOptionsKey = "";
  const copy = value => structuredClone(value);
  const sum = values => values.reduce((total, value) => total + value, 0);
  const safeCount = value => Number.isSafeInteger(value) && value >= 0;
  const color = index => index < COLORS.length ? COLORS[index] : `hsl(${(index * 137.5) % 360} 68% 60%)`;
  function syncFieldControl() {
    if (window.AnalysisFields) {
      window.AnalysisFields.control(fieldControl, { value: preferences.field || "", fixed: [["", "Sem agrupamento"]],
        choose: field => { if ((field || null) !== preferences.field) select(field || null); } });
      fieldControl.disabled = !state.loaded || !!state.analysisDefinitionsPending || !!state.loadOverlay;
      return;
    }
    const fields = [...new Set((state.columns || []).filter(field => typeof field === "string" && field))];
    const missing = preferences.field && !fields.includes(preferences.field) ? preferences.field : null;
    const key = JSON.stringify([fields, missing]);
    if (key !== fieldOptionsKey) {
      fieldOptionsKey = key; fieldControl.replaceChildren();
      const none = el("option", "", "Sem agrupamento"); none.value = ""; fieldControl.append(none);
      for (const field of fields) {
        const option = el("option", "", trunc(colLabel(field), 80)); option.value = field; option.title = field; fieldControl.append(option);
      }
      if (missing) { const option = el("option", "", `${trunc(colLabel(missing), 64)} (campo indisponível)`); option.value = missing; option.disabled = true; fieldControl.append(option); }
    }
    fieldControl.value = preferences.field || "";
    fieldControl.disabled = !state.loaded || !!state.analysisDefinitionsPending || !!state.loadOverlay;
    const owner = window.AnalysisContexts?.capture(), scope = workspaceScope(), signature = scope === "case" ? caseSig() : null;
    fieldControl.onchange = () => {
      if (fieldControl.disabled) return;
      if (owner && !window.AnalysisContexts.isCurrent(owner) || scope !== workspaceScope() || signature !== null && signature !== caseSig()) {
        toast("O contexto mudou. Atualize a Timeline antes de escolher um campo.", "info"); return;
      }
      const field = fieldControl.value || null;
      if (field && !fields.includes(field) || field === preferences.field) return;
      select(field);
    };
  }
  function gridFor(value) {
    const buckets = value?.buckets || [];
    if (!buckets.length) return { start: 0, bucketMs: 0, bucketCount: 0 };
    const start = buckets[0][0], bucketMs = value.bucketMs ?? value.bucket_ms ?? (buckets[1]?.[0] - start || 1);
    const bucketCount = Math.round((buckets.at(-1)[0] - start) / bucketMs) + 1;
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(bucketMs) || bucketMs < 1 || !Number.isSafeInteger(bucketCount) || bucketCount < 1 || bucketCount > 240) throw Error("Grade temporal inválida.");
    if (!Number.isSafeInteger(start + bucketMs * bucketCount)
      || buckets.some(([time, count], index) => !Number.isSafeInteger(time) || !safeCount(count) || (time - start) % bucketMs !== 0 || index > 0 && time <= buckets[index - 1][0])) throw Error("Grade temporal inválida.");
    return { start, bucketMs, bucketCount };
  }
  const contextKey = grid => JSON.stringify([workspaceScope(), window.AnalysisContexts?.capture(), state.datasetRevision,
    workspaceScope() === "case" ? caseSig() : [state.currentArtifact?.id, state.currentArtifact?.loadedAt], backendFilters(), grid]);
  const summaryEntry = () => explorerAnalytics.get(explorerKey());
  const isPaused = () => manualPause || ["paused", "failed"].includes(summaryEntry()?.status);
  function validate(result, expected) {
    const { field, grid, owner, caseKey, scope } = expected;
    const sameIdentity = (a, b) => JSON.stringify(a ? [a.caseId, a.analysisId, a.configRevision, a.visibilityRevision] : null) === JSON.stringify(b ? [b.caseId, b.analysisId, b.configRevision, b.visibilityRevision] : null);
    const sameGrid = value => value && value.start === grid.start && value.bucketMs === grid.bucketMs && value.bucketCount === grid.bucketCount;
    if (result?.field !== field || !sameGrid(result.grid) || result.selection !== "top" || result.limit !== expected.limit
      || !result.context || !sameIdentity(result.context.analysis ?? null, owner?.identity ?? null) || (result.context?.caseKey ?? null) !== caseKey
      || scope === "case" && result.context?.sourceGeneration != null
      || scope !== "case" && owner?.sourceGeneration != null && result.context?.sourceGeneration !== owner.sourceGeneration) throw Error("A resposta temporal pertence a outro contexto.");
    if (!Array.isArray(result.series) || result.series.length > expected.limit || !safeCount(result.untimed) || !safeCount(result.outsideGrid)) throw Error("Resposta temporal incompleta.");
    const groups = [result.total, ...result.series, result.other, result.missing];
    for (const group of groups) if (!safeCount(group?.count) || !Array.isArray(group.buckets) || group.buckets.length !== grid.bucketCount || !group.buckets.every(safeCount) || sum(group.buckets) !== group.count) throw Error("Contagens temporais inconsistentes.");
    const keys = result.series.map(series => series.key);
    if (!keys.every(key => typeof key === "string") || new Set(keys).size !== keys.length) throw Error("Grupos temporais inválidos.");
    const parts = [...result.series, result.other, result.missing];
    if (sum(parts.map(group => group.count)) !== result.total.count || result.total.buckets.some((count, index) => sum(parts.map(group => group.buckets[index])) !== count)) throw Error("Há registros sem representação na Timeline.");
    return result;
  }
  function totalSeries() {
    const grid = gridFor(stats), buckets = Array(grid.bucketCount).fill(0);
    for (const [time, count] of stats?.buckets || []) {
      const index = Math.round((time - grid.start) / grid.bucketMs);
      if (index >= 0 && index < buckets.length) buckets[index] = count;
    }
    return { grid, total: { count: sum(buckets), buckets } };
  }
  function displaySeries() {
    const result = grouped?.base === base ? grouped.result : null;
    const data = result || totalSeries();
    const series = [{ id: "total", label: "Total no período", count: data.total.count, buckets: data.total.buckets, color: () => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim(), total: true }];
    if (preferences.field && result) {
      result.series.forEach((group, index) => series.push({ ...group, id: JSON.stringify(["value", result.field, group.key]), label: group.key === "" ? "(valor vazio)" : group.key, color: color(index) }));
      if (result.other.count) series.push({ ...result.other, id: "other", label: "Outros valores", color: "#9c8b76" });
      if (result.missing.count) series.push({ ...result.missing, id: "missing", label: "Campo ausente", color: "#8693a9" });
    }
    return { data, series, result };
  }
  function paint() {
    if (stats && contextKey(gridFor(stats)) !== base) invalidate();
    panel.hidden = !state.loaded;
    syncFieldControl();
    clear.hidden = countControl.hidden = !preferences.field; countControl.value = String(preferences.limit);
    fieldLabel.textContent = preferences.field ? `por ${colLabel(preferences.field)}` : "Volume de eventos";
    fieldLabel.title = preferences.field || "";
    control.hidden = !preferences.field || status === "done";
    control.textContent = status === "loading" ? "Pausar" : "Retomar";
    control.onclick = () => status === "loading" ? pause() : resume();
    if (!stats) return;
    const { data, series, result } = displaySeries(), grid = data.grid;
    panel.classList.toggle("analytics-placeholder", !grid.bucketCount);
    const plotKey = JSON.stringify([base, result?.field ?? null, preferences.field != null, series.map(item => item.id)]);
    const xs = Array.from({ length: grid.bucketCount }, (_, index) => (grid.start + index * grid.bucketMs) / 1000);
    if (!grid.bucketCount) {
      if (chart) { chart.destroy(); chart = null; } plotted = "";
      box.replaceChildren(el("div", "hint-empty", "Sem dados temporais."));
    } else if (chart && plotted === plotKey) chart.setData([xs, ...series.map(item => item.buckets)]);
    else {
      if (chart) chart.destroy(); box.replaceChildren();
      const plotOwner = base;
      chart = new uPlot({
        width: Math.max(280, box.clientWidth - 4), height: 96, legend: { show: false },
        cursor: { drag: { x: true, y: false, setScale: false }, focus: { prox: 24 } }, scales: { x: { time: true } },
        axes: [{ stroke: () => isLight() ? "#5b6678" : "#8d99ae", grid: { show: false }, ticks: { show: false }, size: 20, font: "10px sans-serif", values: (u, values) => values.map(value => new Date(value * 1000).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })) },
          { stroke: () => isLight() ? "#5b6678" : "#8d99ae", grid: { stroke: () => isLight() ? "rgba(19,81,180,0.08)" : "rgba(255,255,255,0.06)" }, ticks: { show: false }, size: 30 }],
        series: [{}, ...series.map(item => ({ label: item.label, stroke: item.color, width: item.total ? 2 : 1.4, points: { show: false }, show: !preferences.hidden.includes(item.id) }))],
        hooks: { setSelect: [plot => {
          if (plot.select.width < 8 || plotOwner !== contextKey(grid)) return;
          const start = Math.round(plot.posToVal(plot.select.left, "x") * 1000), end = Math.round(plot.posToVal(plot.select.left + plot.select.width, "x") * 1000);
          plot.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
          state.filters = state.filters.filter(filter => !(filter.column === "timestamp" && filter.op === "between"));
          addFilter({ column: "timestamp", op: "between", value: String(start), value2: String(end) });
        }] },
      }, [xs, ...series.map(item => item.buckets)], box);
      plotted = plotKey;
    }
    legend.replaceChildren();
    // Aggregate coverage stays visible before a long, scrollable list of keys.
    // Retain each plot index so rearranging the legend cannot toggle another line.
    const legendPriority = item => item.id === "total" ? 0 : item.id === "other" ? 1 : item.id === "missing" ? 2 : 3;
    const legendSeries = [...series.entries()].sort(([, a], [, b]) => legendPriority(a) - legendPriority(b));
    for (const [index, item] of legendSeries) {
      const label = el("label", "explorer-timeline-series"), checkbox = el("input"), swatch = el("span", "explorer-timeline-swatch");
      checkbox.type = "checkbox"; checkbox.checked = !preferences.hidden.includes(item.id);
      checkbox.setAttribute("aria-label", `Mostrar ${item.label}`); swatch.style.background = typeof item.color === "function" ? item.color() : item.color;
      label.title = `${item.label} · ${fmtNum(item.count)} eventos no período`; label.classList.toggle("is-hidden", !checkbox.checked);
      const caption = el("span", "explorer-timeline-series-label", trunc(item.label, 40));
      label.append(checkbox, swatch, caption, el("small", "", fmtNum(item.count)));
      checkbox.onchange = () => {
        if (!stats || contextKey(gridFor(stats)) !== base) { invalidate(); return; }
        const hidden = new Set(preferences.hidden); if (checkbox.checked) hidden.delete(item.id); else hidden.add(item.id);
        preferences.hidden = [...hidden]; label.classList.toggle("is-hidden", !checkbox.checked);
        chart?.setSeries(index + 1, { show: checkbox.checked }); persist();
      };
      legend.append(label);
    }
    const previous = preferences.field && result && result.field !== preferences.field;
    const messages = [];
    if (previous) messages.push(`Séries anteriores: ${colLabel(result.field)}`);
    if (preferences.field && status === "loading") messages.push(`Calculando ${colLabel(preferences.field)}…`);
    if (preferences.field && status === "paused") messages.push("Agrupamento pausado. Retome para atualizar as séries.");
    if (preferences.field && status === "failed") messages.push(`Agrupamento não concluído: ${error}`);
    if (preferences.field && status === "done") messages.push(`${result?.series.length || 0} grupos de maior volume no período${result?.other.count ? " + Outros valores" : ""}`);
    if (result?.untimed) messages.push(`${fmtNum(result.untimed)} sem horário`);
    if (result?.outsideGrid) messages.push(`${fmtNum(result.outsideGrid)} fora da grade`);
    messages.push("Arraste no gráfico para selecionar um período");
    note.textContent = messages.join(" · ");
  }
  function cancel() { serial++; pending = null; window.Tasks?.cancelLatest("explore-timeline"); }
  async function requestGrouped() {
    if (!preferences.field || !stats) return;
    if (isPaused()) { status = "paused"; paint(); return; }
    if (window.AnalysisFields && !window.AnalysisFields.available(preferences.field)) {
      cancel(); grouped = null; status = "failed"; error = "Campo salvo indisponível neste contexto. Escolha outro campo."; paint(); return;
    }
    const grid = gridFor(stats), expectedBase = contextKey(grid), field = preferences.field, limit = preferences.limit;
    if (expectedBase !== base) { invalidate(); return; }
    if (grouped?.base === base && grouped.result.field === field && grouped.result.limit === limit) { status = "done"; paint(); return; }
    const signature = JSON.stringify([base, field, limit]);
    if (pending?.signature === signature) return pending.promise;
    cancel(); const version = serial, owner = window.AnalysisContexts?.capture(), scope = workspaceScope(), capturedEvents = scope === "case" ? caseEvents() : null;
    const current = () => version === serial && base === expectedBase && contextKey(grid) === expectedBase && preferences.field === field && preferences.limit === limit;
    status = "loading"; error = null; paint();
    const promise = (async () => {
      try {
        const args = await caseArgs({ field, grid, filters: copy(backendFilters()), limit, analysisContext: owner?.identity ?? null, sourceGeneration: owner?.sourceGeneration ?? null, ...(scope === "case" ? { caseEvents: capturedEvents } : {}) });
        if (!current()) return;
        const result = await api("grouped_timeline", args, { silent: true, latest: "explore-timeline", analysisOwner: owner, caseEvents: capturedEvents });
        if (!current()) return;
        validate(result, { field, grid, owner, scope, limit, caseKey: args.caseKey ?? null });
        grouped = { base, result }; status = "done";
      } catch (reason) {
        if (!current()) return;
        status = /cancelad|substituíd/i.test(String(reason)) ? "paused" : "failed"; error = String(reason);
      } finally { if (current()) { pending = null; paint(); } }
    })();
    pending = { signature, promise }; return promise;
  }
  function render(value) {
    const next = contextKey(gridFor(value));
    if (next !== base) { cancel(); grouped = null; manualPause = false; error = null; base = next; status = "idle"; }
    stats = value; paint(); if (preferences.field) void requestGrouped();
  }
  function summary(entry) {
    if (entry?.stats) {
      if (["paused", "failed"].includes(entry.status) && pending) { cancel(); status = "paused"; }
      render(entry.stats); return;
    }
    cancel(); stats = null; grouped = null; base = ""; plotted = "";
    if (chart) { chart.destroy(); chart = null; }
    panel.hidden = !state.loaded; panel.classList.add("analytics-placeholder"); legend.replaceChildren();
    syncFieldControl();
    fieldLabel.textContent = preferences.field ? `por ${colLabel(preferences.field)}` : "Volume de eventos";
    clear.hidden = countControl.hidden = !preferences.field; control.hidden = true; note.textContent = "";
    const label = entry?.status === "paused" ? "Timeline pausada. Os registros continuam disponíveis."
      : entry?.status === "failed" ? "Timeline não concluída. Retome o resumo para tentar novamente."
      : entry?.status === "stats" ? "Calculando Timeline…" : "Timeline aguardando o resumo.";
    box.replaceChildren(el("p", "analytics-placeholder-text", label));
  }
  function pause() { manualPause = true; cancel(); status = "paused"; paint(); }
  function resume() { manualPause = false; const entry = summaryEntry(); if (["paused", "failed"].includes(entry?.status)) entry.resume(); else void requestGrouped(); }
  function persist() { window.WorkspaceContext?.capture(); void saveCases(); }
  function select(field) {
    cancel(); preferences.field = typeof field === "string" && field ? field : null; preferences.hidden = []; manualPause = false; error = null;
    status = preferences.field ? "idle" : "done";
    if (!preferences.field) grouped = null;
    persist(); paint(); if (preferences.field) void requestGrouped();
  }
  clear.onclick = () => select(null);
  countControl.onchange = () => { preferences.limit = Number(countControl.value); cancel(); grouped = null; persist(); paint(); void requestGrouped(); };
  function invalidate() { cancel(); stats = null; grouped = null; base = ""; plotted = ""; error = null; status = "idle"; fieldControl.disabled = true; if (chart) { chart.destroy(); chart = null; } box.replaceChildren(); legend.replaceChildren(); fieldLabel.textContent = ""; note.textContent = ""; control.hidden = true; }
  function restore(value) {
    invalidate(); manualPause = false;
    preferences = { field: typeof value?.field === "string" && value.field ? value.field : null, limit: [6, 12, 24].includes(value?.limit) ? value.limit : 12,
      hidden: Array.isArray(value?.hidden) ? value.hidden.filter(item => typeof item === "string").slice(0, 27) : [] };
    syncFieldControl();
  }
  function menuItem(field) {
    const owner = window.AnalysisContexts?.capture(), scope = workspaceScope();
    return { icon: "fa-chart-line", label: `Representar ${colLabel(field)} na Timeline`, onClick: () => {
      if (owner && !window.AnalysisContexts.isCurrent(owner) || scope !== workspaceScope()) { toast("O contexto mudou. Abra o menu novamente.", "info"); return; }
      select(field); panel.scrollIntoView?.({ block: "nearest" });
    } };
  }
  document.addEventListener?.("analysis-fields-change", () => syncFieldControl());
  return { render, summary, select, pause, resume, invalidate, restore, menuItem, capture: () => copy(preferences), gridFor, validate };
})();
