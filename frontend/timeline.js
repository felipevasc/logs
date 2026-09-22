/* Interactive timeline for the active log selection. */
window.createTimelineView = ({ content, getOverview, sourceKey, filters, isActive, applyRange }) => {
  const HOUR = 3600000;
  const view = { key: "", bounds: null, range: null, history: [], result: null, selected: null, request: 0 };
  const shortTime = (time, span) => new Intl.DateTimeFormat("pt-BR", span < 24 * HOUR
    ? { hour: "2-digit", minute: "2-digit" }
    : span < 45 * 24 * HOUR
      ? { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }
      : { day: "2-digit", month: "2-digit", year: "2-digit" }).format(new Date(time));
  const fullTime = time => new Intl.DateTimeFormat("pt-BR", {
    day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit",
  }).format(new Date(time));
  const intervalLabel = (start, end) => `${fullTime(start)} — ${fullTime(end)}`;
  const bucketEnd = (data, index) => Math.min(data.end, data.buckets[index + 1]?.timestamp - 1 ?? data.end);
  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

  async function load() {
    const request = ++view.request;
    content.innerHTML = '<div class="ws-loading" role="status"><i class="fas fa-circle-notch spin"></i>Calculando timeline…</div>';
    try {
      const overview = await getOverview();
      if (request !== view.request || !isActive()) return;
      if (overview.start == null || overview.end == null) {
        content.innerHTML = `<section class="ws-card"><p class="quiet-empty">Nenhum evento com horário neste recorte.${overview.undated ? ` ${fmtNum(overview.undated)} eventos não entram na timeline; confira a configuração de data/hora da fonte.` : ""}</p></section>`;
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
      }
      if (view.result) { draw(overview); return; }
      await fetchRange(overview, request);
    } catch (error) {
      if (request === view.request && isActive()) content.innerHTML = `<div class="notice">${esc(String(error))}</div><button class="btn ghost" id="tl-retry">Tentar novamente</button>`;
      content.querySelector("#tl-retry")?.addEventListener("click", load);
    }
  }

  async function fetchRange(overview, request = ++view.request) {
    const { start, end } = view.range;
    content.innerHTML = '<div class="ws-loading" role="status"><i class="fas fa-circle-notch spin"></i>Lendo o período…</div>';
    try {
      const data = await api("timeline_range", { filters: filters(), start, end, bucketCount: 120 });
      if (request !== view.request || !isActive()) return;
      view.result = data;
      view.selected = null;
      draw(overview);
    } catch (error) {
      if (request !== view.request || !isActive()) return;
      content.innerHTML = `<div class="notice">${esc(String(error))}</div><button class="btn ghost" id="tl-retry">Tentar novamente</button>`;
      content.querySelector("#tl-retry")?.addEventListener("click", () => fetchRange(overview));
    }
  }

  async function navigate(start, end, overview, { remember = true } = {}) {
    if (start >= end) return;
    if (remember) view.history.push({ ...view.range });
    view.range = { start, end };
    view.result = null;
    view.selected = null;
    await fetchRange(overview);
  }

  function draw(overview) {
    const data = view.result;
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
    const gap = data.buckets.findIndex((bucket, index) => index > 0 && index < n - 1 && bucket.count === 0);
    const peak = data.buckets.reduce((best, bucket, index) => bucket.count > data.buckets[best].count ? index : best, 0);
    const errorPeak = data.buckets.reduce((best, bucket, index) => bucket.errors > data.buckets[best].errors ? index : best, 0);
    const signal = (index, label, number, suffix) => `<button class="tl-signal" data-signal="${index}"><span>${label}</span><strong>${number}</strong><small>${shortTime(data.buckets[index].timestamp, span)} · ${suffix}</small></button>`;
    content.innerHTML = `<div class="tl-summary"><div><span>Período visível</span><strong>${intervalLabel(data.start, data.end)}</strong></div><div class="tl-summary-numbers"><span><b>${fmtNum(data.total)}</b> eventos</span><span><b>${fmtNum(data.errors)}</b> erros</span><span><b>${fmtNum(data.warnings)}</b> avisos</span></div></div>
      <section class="ws-card tl-main"><div class="tl-main-head"><div class="tl-legend"><span><i class="tl-key total"></i> Demais eventos</span><span><i class="tl-key error"></i> Erros</span><span><i class="tl-key warning"></i> Avisos</span></div><div class="tl-controls"><button class="btn ghost small" data-tl="back" ${view.history.length ? "" : "disabled"}><i class="fas fa-arrow-left"></i> Voltar</button><button class="btn ghost small" data-tl="hour" ${span > HOUR ? "" : "disabled"}>1 h</button><button class="btn ghost small" data-tl="day" ${span > 24 * HOUR ? "" : "disabled"}>24 h</button><button class="btn ghost small" data-tl="all" ${data.start !== view.bounds.start || data.end !== view.bounds.end ? "" : "disabled"}>Tudo</button></div></div>
      <div class="tl-chart-layout"><div class="tl-y-axis"><span>${fmtNum(max)}</span><span>${fmtNum(Math.round(max / 2))}</span><span>0</span></div><div class="tl-plot-column"><div class="tl-interactive" id="tl-interactive" tabindex="0" role="group" aria-label="Timeline. Clique em uma barra para selecionar um intervalo; arraste para selecionar vários. Use as setas para mudar a seleção."><svg viewBox="0 0 1000 260" preserveAspectRatio="none" role="img" aria-label="Volume de eventos, erros e avisos no período">${grid}${bars}</svg><div class="tl-selected-band" id="tl-selected-band" hidden></div><div class="tl-drag-band" id="tl-drag-band" hidden></div><div class="tl-hover" id="tl-hover" hidden></div></div><div class="tl-x-axis">${timeTicks}</div></div></div>
      <div class="tl-chart-foot"><span>Arraste para marcar um período. Clique em uma barra para examiná-la.</span><span>Horário local · intervalos de ${formatDuration(data.bucketMs)}</span></div></section>
      <div class="tl-bottom"><section class="ws-card tl-selection" id="tl-selection"></section><section class="ws-card tl-signals"><div class="card-heading"><h2>Pontos do período</h2></div>${data.total ? signal(peak, "Maior volume", fmtNum(data.buckets[peak].count), "eventos") : ""}${data.errors ? signal(errorPeak, "Mais erros", fmtNum(data.buckets[errorPeak].errors), "erros") : ""}${gap >= 0 ? signal(gap, "Sem registros", "—", "intervalo vazio") : ""}${!data.total ? '<p class="quiet-empty">Não há eventos com horário neste período.</p>' : ""}</section></div>
      ${overview.undated ? `<p class="tl-undated">${fmtNum(overview.undated)} eventos sem horário não aparecem na timeline.</p>` : ""}`;
    const plot = content.querySelector("#tl-interactive");
    const position = event => clamp((event.clientX - plot.getBoundingClientRect().left) / plot.clientWidth, 0, 0.999999);
    const indexAt = event => Math.floor(position(event) * n);
    const hover = content.querySelector("#tl-hover");
    const dragBand = content.querySelector("#tl-drag-band");
    let drag = null;
    plot.onpointerdown = event => {
      if (event.button !== 0) return;
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
        hover.textContent = `${shortTime(bucket.timestamp, span)} · ${fmtNum(bucket.count)} eventos · ${fmtNum(bucket.errors)} erros`;
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
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        const current = view.selected?.first ?? peak;
        select(clamp(current + (event.key === "ArrowLeft" ? -1 : 1), 0, n - 1));
      } else if (event.key === "Enter" && view.selected) {
        event.preventDefault();
        content.querySelector('[data-tl="zoom"]')?.click();
      }
    };
    content.querySelectorAll("[data-signal]").forEach(button => button.onclick = () => select(+button.dataset.signal));
    content.querySelectorAll("[data-tl]").forEach(button => button.onclick = async () => {
      if (button.dataset.tl === "back") {
        const previous = view.history.pop();
        if (previous) await navigate(previous.start, previous.end, overview, { remember: false });
      } else if (button.dataset.tl === "all") {
        await navigate(view.bounds.start, view.bounds.end, overview);
      } else {
        const duration = button.dataset.tl === "hour" ? HOUR : 24 * HOUR;
        await navigate(Math.max(view.bounds.start, data.end - duration), data.end, overview);
      }
    });
    select(view.selected?.first ?? peak, view.selected?.last ?? peak);

    function select(first, last = first) {
      first = clamp(first, 0, n - 1);
      last = clamp(last, first, n - 1);
      view.selected = { first, last };
      const selectedBuckets = data.buckets.slice(first, last + 1);
      const count = selectedBuckets.reduce((sum, bucket) => sum + bucket.count, 0);
      const errors = selectedBuckets.reduce((sum, bucket) => sum + bucket.errors, 0);
      const warnings = selectedBuckets.reduce((sum, bucket) => sum + bucket.warnings, 0);
      const start = data.buckets[first].timestamp, end = bucketEnd(data, last);
      const band = content.querySelector("#tl-selected-band");
      band.hidden = false;
      band.style.left = `${first * 100 / n}%`;
      band.style.width = `${(last - first + 1) * 100 / n}%`;
      const target = content.querySelector("#tl-selection");
      target.innerHTML = `<div class="card-heading"><h2>Intervalo selecionado</h2><span>${intervalLabel(start, end)}</span></div><div class="tl-selection-stats"><div><strong>${fmtNum(count)}</strong><span>eventos</span></div><div><strong>${fmtNum(errors)}</strong><span>erros${count ? ` · ${pctLocal(errors, count)}` : ""}</span></div><div><strong>${fmtNum(warnings)}</strong><span>avisos</span></div></div><div class="tl-selection-actions"><button class="btn primary small" data-tl="explore" ${count ? "" : "disabled"}>Explorar eventos <i class="fas fa-arrow-right"></i></button><button class="btn ghost small" data-tl="zoom" ${end > start && end - start < data.end - data.start ? "" : "disabled"}>Aproximar período</button></div>`;
      target.querySelector('[data-tl="explore"]').onclick = () => applyRange(start, end);
      target.querySelector('[data-tl="zoom"]').onclick = () => navigate(start, end, overview);
    }
  }

  function formatDuration(ms) {
    if (ms < 1000) return `${ms} ms`;
    if (ms < 60000) return `${Math.round(ms / 1000)} s`;
    if (ms < HOUR) return `${Math.round(ms / 60000)} min`;
    if (ms < 24 * HOUR) return `${Math.round(ms / HOUR)} h`;
    return `${Math.round(ms / (24 * HOUR))} d`;
  }
  const pctLocal = (count, total) => `${(count * 100 / total).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}%`;
  return { load, invalidate: () => { view.result = null; } };
};
