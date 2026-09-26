/* Interactive timeline for the active log selection. */
window.createTimelineView = ({ content, getOverview, sourceKey, filters, isActive, applyRange, onMode, scope = () => window.WorkspaceContext?.scope() || "dataset" }) => {
  const HOUR = 3600000;
  const contexts = new Map();
  let view, requestSerial = 0;
  const contextKey = () => JSON.stringify([scope(), typeof state === "undefined" ? null : state.cases?.active, scope() === "dataset" && typeof state !== "undefined" ? state.currentArtifact?.id : null]);
  function syncContext() {
    const key = contextKey();
    if (!contexts.has(key)) {
      contexts.set(key, { key: "", bounds: null, range: null, history: [], result: null, selected: null });
      if (contexts.size > 12) contexts.delete(contexts.keys().next().value);
    }
    view = contexts.get(key);
  }
  syncContext();
  // An outstanding query belongs to the old context even if its page remains open.
  document.addEventListener("workspace-context-change", () => { requestSerial++; syncContext(); });
  const shortTime = (time, span) => new Intl.DateTimeFormat("pt-BR", span < 24 * HOUR
    ? { hour: "2-digit", minute: "2-digit", ...(span < 5 * 60000 ? { second: "2-digit" } : {}) }
    : span < 45 * 24 * HOUR
      ? { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }
      : { day: "2-digit", month: "2-digit", year: "2-digit" }).format(new Date(time));
  const fullTime = (time, precise = false) => new Intl.DateTimeFormat("pt-BR", {
    day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit",
    ...(precise ? { fractionalSecondDigits: 3 } : {}),
  }).format(new Date(time));
  const intervalLabel = (start, end) => `${fullTime(start, end - start < 1000)} — ${fullTime(end, end - start < 1000)}`;
  // Apply the fallback before subtracting: undefined - 1 is NaN, not nullish.
  const bucketEnd = (data, index) => Math.min(data.end, (data.buckets[index + 1]?.timestamp ?? data.end + 1) - 1);
  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
  const validRange = range => Number.isFinite(range?.start) && Number.isFinite(range?.end) && range.start <= range.end;
  function restore(snapshot) {
    requestSerial++; syncContext();
    if (!validRange(snapshot?.range)) return;
    view.restore = { range: { start: snapshot.range.start, end: snapshot.range.end }, history: (Array.isArray(snapshot.history) ? snapshot.history : []).slice(-40).filter(validRange), selected: snapshot.selected };
    view.key = ""; view.result = null;
  }
  const capture = () => { syncContext(); return { range: view.range && { ...view.range }, history: view.history.slice(-40).map(range => ({ ...range })), selected: view.selected && { ...view.selected } }; };
  function modeControls() {
    if (scope() !== "case" || !onMode) return;
    const controls = document.createElement("div"); controls.className = "seg tl-mode-switch";
    controls.setAttribute("role", "group"); controls.setAttribute("aria-label", "Visualização da linha do tempo");
    controls.innerHTML = '<button type="button" class="seg-btn active" aria-pressed="true"><i class="fas fa-chart-column" aria-hidden="true"></i> Volume</button><button type="button" class="seg-btn" data-timeline-mode="horizontal" aria-pressed="false"><i class="fas fa-arrows-left-right" aria-hidden="true"></i> Horizontal</button><button type="button" class="seg-btn" data-timeline-mode="vertical" aria-pressed="false"><i class="fas fa-arrows-up-down" aria-hidden="true"></i> Vertical</button><button type="button" class="seg-btn" data-timeline-mode="table" aria-pressed="false"><i class="fas fa-table-list" aria-hidden="true"></i> Tabela</button>';
    controls.querySelectorAll("[data-timeline-mode]").forEach(button => { button.onclick = () => onMode(button.dataset.timelineMode); });
    content.prepend(controls);
  }

  async function load() {
    syncContext();
    const request = ++requestSerial;
    content.innerHTML = '<div class="ws-loading" role="status"><i class="fas fa-circle-notch spin"></i>Calculando timeline…</div>';
    try {
      const overview = await getOverview();
      if (request !== requestSerial || !isActive()) return;
      if (overview.start == null || overview.end == null) {
        view.result = null;
        content.innerHTML = `<section class="ws-card"><p class="quiet-empty">Nenhum evento com horário neste recorte.${overview.undated ? ` ${fmtNum(overview.undated)} eventos não entram na timeline; confira a configuração de data/hora da fonte.` : ""}</p></section>`;
        modeControls();
        return;
      }
      const key = sourceKey();
      if (view.key !== key) {
        view.key = key;
        view.bounds = { start: overview.start, end: overview.end };
        view.range = { ...view.bounds };
        view.history = [];
        view.result = null;
        view.selected = null;
        if (view.restore) {
          const trim = range => ({ start: clamp(range.start, view.bounds.start, view.bounds.end), end: clamp(range.end, view.bounds.start, view.bounds.end) });
          if (view.restore.range.end >= view.bounds.start && view.restore.range.start <= view.bounds.end) {
            view.range = trim(view.restore.range);
            view.history = view.restore.history.map(trim);
            const selected = view.restore.selected;
            if (Number.isInteger(selected?.first) && Number.isInteger(selected?.last) && selected.first >= 0 && selected.last >= selected.first && selected.last < 120) view.restoreSelection = { first: selected.first, last: selected.last };
          }
          view.restore = null;
        }
      }
      if (view.result) { draw(overview); return; }
      await fetchRange(overview, request);
    } catch (error) {
      if (request !== requestSerial || !isActive()) return;
      content.innerHTML = `<div class="notice">${esc(String(error))}</div><button class="btn ghost" id="tl-retry">Tentar novamente</button>`;
      modeControls();
      content.querySelector("#tl-retry")?.addEventListener("click", load);
    }
  }

  async function fetchRange(overview, request = ++requestSerial) {
    const { start, end } = view.range;
    content.innerHTML = '<div class="ws-loading" role="status"><i class="fas fa-circle-notch spin"></i>Lendo o período…</div>';
    try {
      const selection = typeof analyticsRequest === "function" ? analyticsRequest(scope()) : { filters: filters() };
      const data = await api("timeline_range", { ...selection, start, end, bucketCount: 120 });
      if (request !== requestSerial || !isActive()) return;
      view.result = { ...data, buckets: data.buckets.filter(bucket => bucket.timestamp >= data.start && bucket.timestamp <= data.end) };
      view.selected = view.restoreSelection || null; view.restoreSelection = null;
      draw(overview);
    } catch (error) {
      if (request !== requestSerial || !isActive()) return;
      content.innerHTML = `<div class="notice">${esc(String(error))}</div><button class="btn ghost" id="tl-retry">Tentar novamente</button>`;
      modeControls();
      content.querySelector("#tl-retry")?.addEventListener("click", () => fetchRange(overview));
    }
  }

  async function navigate(start, end, overview, { remember = true } = {}) {
    start = clamp(Math.round(start), view.bounds.start, view.bounds.end);
    end = clamp(Math.round(end), start, view.bounds.end);
    if (!Number.isFinite(start) || !Number.isFinite(end) || start > end || start === view.range.start && end === view.range.end) return;
    if (remember) { view.history.push({ ...view.range }); if (view.history.length > 40) view.history.shift(); }
    view.range = { start, end };
    view.result = null;
    view.selected = null;
    await fetchRange(overview);
  }

  function draw(overview) {
    const data = view.result;
    if (!data.buckets?.length) {
      content.innerHTML = '<section class="ws-card"><p class="quiet-empty">Não há intervalos para exibir neste período.</p></section>';
      modeControls();
      return;
    }
    const span = Math.max(1, data.end - data.start);
    const max = Math.max(1, ...data.buckets.map(bucket => bucket.count));
    const n = data.buckets.length;
    const step = 1000 / n;
    const height = 226;
    const bottom = 252;
    const grid = [0, 0.25, 0.5, 0.75, 1].map(fraction => `<line x1="0" x2="1000" y1="${bottom - height * fraction}" y2="${bottom - height * fraction}" class="tl-grid"/>`).join("");
    const bars = data.buckets.map((bucket, i) => {
      const h = bucket.count ? Math.max(1, height * bucket.count / max) : 0;
      const red = height * bucket.errors / max;
      const amber = height * bucket.warnings / max;
      const x = i * step + 1.3;
      const width = Math.max(1.5, step - 2.6);
      return `<g class="tl-bin"><rect x="${x}" y="${bottom - h}" width="${width}" height="${h}" rx="1.5" class="tl-bar-total"/>${red ? `<rect x="${x}" y="${bottom - red}" width="${width}" height="${red}" rx="1" class="tl-bar-error"/>` : ""}${amber ? `<rect x="${x}" y="${bottom - red - amber}" width="${width}" height="${amber}" rx="1" class="tl-bar-warning"/>` : ""}</g>`;
    }).join("");
    const timeTicks = [0, 0.25, 0.5, 0.75, 1].map(fraction => `<span>${shortTime(data.start + span * fraction, span)}</span>`).join("");
    // Only report complete silent stretches between observed events. Empty edges
    // can simply be the bounds of the user's filter and are not a useful signal.
    let gap = null;
    for (let i = 1; i < n - 1; i++) {
      if (data.buckets[i].count || !data.buckets[i - 1].count) continue;
      const first = i;
      while (i < n && !data.buckets[i].count) i++;
      if (i < n && (!gap || i - first > gap.last - gap.first + 1)) gap = { first, last: i - 1 };
    }
    const peak = data.buckets.reduce((best, bucket, index) => bucket.count > data.buckets[best].count ? index : best, 0);
    const errorPeak = data.buckets.reduce((best, bucket, index) => bucket.errors > data.buckets[best].errors ? index : best, 0);
    const signal = (index, label, number, suffix, last = index) => `<button type="button" class="tl-signal" data-signal="${index}" data-signal-end="${last}"><span>${label}</span><strong>${number}</strong><small>${shortTime(data.buckets[index].timestamp, span)} · ${suffix}</small></button>`;
    content.innerHTML = `<div class="tl-summary"><div><span>Período visível</span><strong>${intervalLabel(data.start, data.end)}</strong></div><div class="tl-summary-numbers"><span><b>${fmtNum(data.total)}</b> eventos</span><span><b>${fmtNum(data.errors)}</b> erros</span><span><b>${fmtNum(data.warnings)}</b> avisos</span></div></div>
      <section class="ws-card tl-main"><div class="tl-main-head"><div class="tl-legend"><span><i class="tl-key total"></i> Demais eventos</span><span><i class="tl-key error"></i> Erros</span><span><i class="tl-key warning"></i> Avisos</span></div><div class="tl-controls"><button class="btn ghost small tl-swim-button" data-lanes aria-haspopup="menu" title="Separar a atividade por origem, usuário, IP…"><i class="fas fa-layer-group"></i> Separar</button><button class="btn ghost small" data-tl="back" ${view.history.length ? "" : "disabled"}><i class="fas fa-arrow-left"></i> Voltar</button><button class="btn ghost small" data-tl="hour" ${span > HOUR ? "" : "disabled"}>1 h</button><button class="btn ghost small" data-tl="day" ${span > 24 * HOUR ? "" : "disabled"}>24 h</button><button class="btn ghost small" data-tl="all" ${data.start !== view.bounds.start || data.end !== view.bounds.end ? "" : "disabled"}>Tudo</button></div></div>
      <div class="tl-chart-layout"><div class="tl-y-axis"><span>${fmtNum(max)}</span><span>${fmtNum(max / 2)}</span><span>0</span></div><div class="tl-plot-column"><div class="tl-interactive" id="tl-interactive" tabindex="0" role="group" aria-label="Volume ao longo do tempo" aria-describedby="tl-instructions tl-selection-status"><svg viewBox="0 0 1000 260" preserveAspectRatio="none" role="img" aria-label="Volume de eventos, erros e avisos no período">${grid}${bars}</svg><div class="tl-selected-band" id="tl-selected-band" hidden></div><div class="tl-drag-band" id="tl-drag-band" hidden></div><div class="tl-hover" id="tl-hover" hidden></div></div><div class="tl-x-axis">${timeTicks}</div></div></div><div class="tl-swims" id="tl-swims" hidden></div>
      <div class="tl-chart-foot"><span id="tl-instructions">Clique ou arraste para selecionar · Setas para navegar · Shift + setas para ampliar</span><span>Horário local · intervalos de ${formatDuration(data.bucketMs)}</span></div><p id="tl-selection-status" class="tl-sr-only" aria-live="polite" aria-atomic="true"></p></section>
      <div class="tl-bottom"><section class="ws-card tl-selection" id="tl-selection"></section><section class="ws-card tl-signals"><div class="card-heading"><h2>Destaques do período</h2></div>${data.total ? signal(peak, "Maior volume", fmtNum(data.buckets[peak].count), "eventos") : ""}${data.errors ? signal(errorPeak, "Mais erros", fmtNum(data.buckets[errorPeak].errors), "erros") : ""}${gap ? signal(gap.first, "Maior pausa", formatDuration(bucketEnd(data, gap.last) - data.buckets[gap.first].timestamp + 1), "sem registros entre eventos", gap.last) : ""}${!data.total ? '<p class="quiet-empty">Não há eventos com horário neste período.</p>' : ""}</section></div>
      ${overview.undated ? `<p class="tl-undated">${fmtNum(overview.undated)} eventos sem horário não aparecem na timeline.</p>` : ""}`;
    modeControls();
    const plot = content.querySelector("#tl-interactive");
    window.TimelineExport?.attach(content.querySelector(".tl-controls"), { source: content, type: "activity", title: scope() === "case" ? "Atividade do caso" : "Atividade dos registros", subtitle: `${intervalLabel(data.start, data.end)} · ${fmtNum(data.total)} eventos no recorte`, filename: scope() === "case" ? "caso-atividade-timeline" : "atividade-timeline" });
    const position = event => clamp((event.clientX - plot.getBoundingClientRect().left) / plot.clientWidth, 0, 0.999999);
    const indexAt = event => Math.floor(position(event) * n);
    const hover = content.querySelector("#tl-hover");
    const dragBand = content.querySelector("#tl-drag-band");
    let drag = null;
    plot.onpointerdown = event => {
      if (event.button !== 0) return;
      plot.focus({ preventScroll: true });
      drag = { first: indexAt(event), x: event.clientX };
      plot.setPointerCapture(event.pointerId);
    };
    plot.onpointermove = event => {
      if (drag) {
        const from = Math.min(drag.first, indexAt(event)), to = Math.max(drag.first, indexAt(event));
        dragBand.hidden = false;
        dragBand.style.left = `${from * 100 / n}%`;
        dragBand.style.width = `${(to - from + 1) * 100 / n}%`;
        hover.hidden = true;
      } else {
        const i = indexAt(event), bucket = data.buckets[i];
        hover.textContent = `${shortTime(bucket.timestamp, span)} · ${fmtNum(bucket.count)} eventos · ${fmtNum(bucket.errors)} erros · ${fmtNum(bucket.warnings)} avisos`;
        hover.style.left = `${clamp(position(event) * 100, 8, 78)}%`;
        hover.hidden = false;
      }
    };
    plot.onpointerleave = () => { if (!drag) hover.hidden = true; };
    plot.onpointercancel = () => { drag = null; dragBand.hidden = true; hover.hidden = true; };
    plot.onpointerup = event => {
      if (!drag) return;
      const first = Math.min(drag.first, indexAt(event)), last = Math.max(drag.first, indexAt(event));
      select(first, last);
      drag = null;
      dragBand.hidden = true;
      hover.hidden = true;
    };
    plot.onkeydown = event => {
      if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        const current = view.selected?.cursor ?? view.selected?.first ?? peak;
        const next = event.key === "Home" ? 0 : event.key === "End" ? n - 1 : clamp(current + (event.key === "ArrowLeft" ? -1 : 1), 0, n - 1);
        const anchor = event.shiftKey ? view.selected?.anchor ?? current : next;
        select(Math.min(anchor, next), Math.max(anchor, next), anchor, next);
      } else if (event.key === "Enter" && view.selected) {
        event.preventDefault();
        content.querySelector('[data-tl="zoom"]')?.click();
      }
    };
    content.querySelectorAll("[data-signal]").forEach(button => button.onclick = () => select(+button.dataset.signal, +button.dataset.signalEnd));
    content.querySelectorAll("[data-tl]").forEach(button => button.onclick = async () => {
      if (button.dataset.tl === "back") {
        const previous = view.history.pop();
        if (previous) await navigate(previous.start, previous.end, overview, { remember: false });
      } else if (button.dataset.tl === "all") {
        await navigate(view.bounds.start, view.bounds.end, overview);
      } else {
        const duration = button.dataset.tl === "hour" ? HOUR : 24 * HOUR;
        const center = view.selected ? (data.buckets[view.selected.first].timestamp + bucketEnd(data, view.selected.last)) / 2 : (data.start + data.end) / 2;
        const from = clamp(Math.round(center - (duration - 1) / 2), view.bounds.start, Math.max(view.bounds.start, view.bounds.end - duration + 1));
        await navigate(from, Math.min(view.bounds.end, from + duration - 1), overview);
      }
    });
    select(view.selected?.first ?? peak, view.selected?.last ?? peak);
    // Detections become marks under the axis; a mark selects its interval.
    window.Security?.markers(content.querySelector(".tl-plot-column"), data.start, data.end, (from, to) => {
      const first = clamp(Math.floor((from - data.start) / data.bucketMs), 0, n - 1);
      const last = clamp(Math.floor((to - data.start) / data.bucketMs), first, n - 1);
      select(first, last);
    }, content.querySelector(".tl-x-axis"));
    content.querySelector("[data-lanes]").onclick = event => {
      const r = event.currentTarget.getBoundingClientRect();
      const current = view.lanes;
      const options = laneOptions().map(([column, label]) => ({ icon: column === current ? "fa-check" : window.EntityMenu?.icon(column) || "fa-tag", label, onClick: () => { view.lanes = column; drawLanes(data, select); } }));
      showCtxMenu(r.left, r.bottom + 4, current ? [{ icon: "fa-minus", label: "Não separar", onClick: () => { view.lanes = null; drawLanes(data, select); } }, { sep: true }, ...options] : options);
    };
    drawLanes(data, select);

    function select(first, last = first, anchor = first, cursor = last) {
      first = clamp(first, 0, n - 1);
      last = clamp(last, first, n - 1);
      view.selected = { first, last, anchor, cursor };
      const selectedBuckets = data.buckets.slice(first, last + 1);
      const count = selectedBuckets.reduce((sum, bucket) => sum + bucket.count, 0);
      const errors = selectedBuckets.reduce((sum, bucket) => sum + bucket.errors, 0);
      const warnings = selectedBuckets.reduce((sum, bucket) => sum + bucket.warnings, 0);
      const start = data.buckets[first].timestamp, end = bucketEnd(data, last);
      content.querySelector("#tl-selection-status").textContent = `${intervalLabel(start, end)}. ${fmtNum(count)} eventos, ${fmtNum(errors)} erros, ${fmtNum(warnings)} avisos.`;
      content.querySelectorAll("[data-signal]").forEach(button => button.classList.toggle("is-selected", +button.dataset.signal === first && +button.dataset.signalEnd === last));
      const band = content.querySelector("#tl-selected-band");
      band.hidden = false;
      band.style.left = `${first * 100 / n}%`;
      band.style.width = `${(last - first + 1) * 100 / n}%`;
      markLanes(first, last, n);
      const target = content.querySelector("#tl-selection");
      target.innerHTML = `<div class="card-heading"><h2>Intervalo selecionado</h2><span>${intervalLabel(start, end)}</span></div><div class="tl-selection-stats"><div><strong>${fmtNum(count)}</strong><span>eventos</span></div><div><strong>${fmtNum(errors)}</strong><span>erros${count ? ` · ${pctLocal(errors, count)}` : ""}</span></div><div><strong>${fmtNum(warnings)}</strong><span>avisos</span></div></div><div class="tl-selection-actions"><button class="btn primary small" data-tl="explore" ${count ? "" : "disabled"}>Explorar eventos <i class="fas fa-arrow-right"></i></button><button class="btn ghost small" data-tl="zoom" ${end > start && end - start < data.end - data.start ? "" : "disabled"}>Aproximar período</button></div>`;
      target.querySelector('[data-tl="explore"]').onclick = () => applyRange(start, end);
      target.querySelector('[data-tl="zoom"]').onclick = () => navigate(start, end, overview);
    }
  }

  // ------------------------------------------------------------ lanes (optional, off by default)
  const laneLabel = column => column === "source" ? "Origem" : column === "code" ? "Código" : window.EntityMenu?.label(column) || column;
  function laneOptions() {
    const coverage = (window.Security?.last()?.coverage || []).filter(c => c.count > 0).map(c => [c.column, c.label]);
    const roles = coverage.length ? coverage : [["@user", "Usuário"], ["@src_ip", "IP de origem"], ["@host", "Host"]];
    return [["source", "Origem"], ...roles, ["code", "Código"]];
  }
  function markLanes(first, last, n) {
    const step = 1000 / n;
    content.querySelectorAll(".tl-swim-sel").forEach(rect => { rect.setAttribute("x", first * step); rect.setAttribute("width", (last - first + 1) * step); });
  }
  async function drawLanes(data, select) {
    const host = content.querySelector("#tl-swims"), button = content.querySelector("[data-lanes]");
    if (!host || !button) return;
    const column = view.lanes;
    button.classList.toggle("active", !!column);
    button.innerHTML = `<i class="fas fa-layer-group"></i> ${column ? esc(laneLabel(column)) : "Separar"}`;
    if (!column) { host.hidden = true; host.innerHTML = ""; return; }
    host.hidden = false;
    const key = JSON.stringify([column, data.start, data.end, view.key]);
    if (view.lanesResult?.key !== key) {
      host.innerHTML = '<div class="tl-swim-note"><i class="fas fa-circle-notch spin"></i></div>';
      try {
        const selection = typeof analyticsRequest === "function" ? analyticsRequest(scope()) : { filters: filters() };
        const lanes = await api("timeline_lanes", { ...selection, start: data.start, end: data.end, bucketCount: data.buckets.length, column, limit: 8 }, { silent: true });
        if (view.result !== data || view.lanes !== column || !host.isConnected) return;
        view.lanesResult = { key, lanes };
      } catch (error) {
        if (view.result === data && host.isConnected) host.innerHTML = `<p class="tl-swim-note">${esc(String(error))}</p>`;
        return;
      }
    }
    renderLanes(host, data, view.lanesResult.lanes, select);
  }
  function renderLanes(host, data, lanes, select) {
    const n = data.buckets.length, step = 1000 / n, span = Math.max(1, data.end - data.start);
    const rows = [...lanes.lanes, ...(lanes.others ? [{ ...lanes.others, others: true }] : [])];
    const label = laneLabel(lanes.column);
    if (!rows.length) { host.innerHTML = `<p class="tl-swim-note">Nenhum valor de ${esc(label)} neste período.</p>`; return; }
    // Buckets use the chart's layout; a lane bucket maps onto the bar that covers its start.
    const cells = rows.map(lane => {
      const merged = new Map();
      lane.counts.forEach((count, j) => {
        if (!count) return;
        const i = clamp(Math.floor((lanes.start + j * lanes.bucketMs - data.start) / data.bucketMs), 0, n - 1);
        const cell = merged.get(i) || { count: 0, errors: 0 };
        cell.count += count; cell.errors += lane.error_counts[j] || 0;
        merged.set(i, cell);
      });
      return merged;
    });
    host.innerHTML = rows.map((lane, k) => {
      const max = Math.max(1, ...[...cells[k].values()].map(c => c.count));
      const rects = [...cells[k]].map(([i, c]) => `<rect x="${i * step + 0.6}" y="0" width="${Math.max(1, step - 1.2)}" height="12" class="${c.errors * 2 >= c.count ? "err" : ""}" style="opacity:${(0.2 + 0.8 * Math.sqrt(c.count / max)).toFixed(2)}"/>`).join("");
      const name = lane.others ? `Outros valores` : lane.value;
      return `<div class="tl-swim${lane.others ? " others" : ""}" data-lane="${k}"><div class="tl-swim-head"><button type="button" class="tl-swim-label" ${lane.others ? "disabled" : ""} title="${lane.others ? "" : "Ações para este valor"}"><span></span></button><small>${fmtNum(lane.total)}${lane.errors ? ` · <b>${fmtNum(lane.errors)} erros</b>` : ""}</small></div><svg class="tl-swim-strip" viewBox="0 0 1000 12" preserveAspectRatio="none" role="img" aria-label="${esc(name)}: ${fmtNum(lane.total)} eventos"><rect class="tl-swim-sel" y="0" height="12" x="0" width="0"/>${rects}</svg></div>`;
    }).join("") + `${lanes.missing ? `<p class="tl-swim-note">${fmtNum(lanes.missing)} eventos sem ${esc(label.toLowerCase())}</p>` : ""}<div class="tl-swim-hover" hidden></div>`;
    rows.forEach((lane, k) => { host.querySelector(`[data-lane="${k}"] .tl-swim-label span`).textContent = lane.others ? "Outros valores" : lane.value; });
    const hover = host.querySelector(".tl-swim-hover");
    const at = (event, strip) => clamp(Math.floor((event.clientX - strip.getBoundingClientRect().left) / strip.clientWidth * n), 0, n - 1);
    const entity = (lane, i) => i == null ? { column: lanes.column, value: lane.value, first: data.start, last: data.end } : { column: lanes.column, value: lane.value, first: data.buckets[i].timestamp, last: bucketEnd(data, i) };
    host.querySelectorAll(".tl-swim").forEach(row => {
      const k = +row.dataset.lane, lane = rows[k], strip = row.querySelector(".tl-swim-strip");
      const menu = (event, i) => { if (lane.others) return; event.preventDefault(); window.EntityMenu?.open(event.clientX, event.clientY, entity(lane, i)); };
      row.querySelector(".tl-swim-label").onclick = event => { const r = event.currentTarget.getBoundingClientRect(); menu({ preventDefault() {}, clientX: r.left, clientY: r.bottom + 4 }); };
      strip.onpointermove = event => {
        const i = at(event, strip), cell = cells[k].get(i);
        hover.textContent = `${lane.others ? "Outros valores" : lane.value} · ${shortTime(data.buckets[i].timestamp, span)} · ${fmtNum(cell?.count || 0)} eventos${cell?.errors ? ` · ${fmtNum(cell.errors)} erros` : ""}`;
        hover.style.left = `${clamp((event.clientX - host.getBoundingClientRect().left) / host.clientWidth * 100, 6, 74)}%`;
        hover.style.top = `${row.offsetTop - 26}px`;
        hover.hidden = false;
      };
      strip.onpointerleave = () => { hover.hidden = true; };
      strip.onclick = event => select(at(event, strip));
      strip.oncontextmenu = event => menu(event, at(event, strip));
    });
    if (view.selected) markLanes(view.selected.first, view.selected.last, n);
  }

  function formatDuration(ms) {
    if (ms < 1000) return `${ms} ms`;
    const number = value => value.toLocaleString("pt-BR", { maximumFractionDigits: 1 });
    if (ms < 60000) return `${number(ms / 1000)} s`;
    if (ms < HOUR) return `${number(ms / 60000)} min`;
    if (ms < 24 * HOUR) return `${number(ms / HOUR)} h`;
    return `${number(ms / (24 * HOUR))} d`;
  }
  const pctLocal = (count, total) => `${(count * 100 / total).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}%`;
  return { load, capture, restore, invalidate: () => { requestSerial++; view.result = null; } };
};
