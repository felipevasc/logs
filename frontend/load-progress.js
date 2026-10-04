/* Foreground load card below the waiting animation: one clear title, the measured
   amount over the bar, a few labelled figures and the step history. Presentation
   only: every value comes from a received progress receipt; nothing is estimated here. */
window.LoadProgress = (() => {
  "use strict";
  const node = id => document.getElementById(id);
  const MAX_STEPS = 40;
  const number = value => Number(value).toLocaleString("pt-BR");
  function bytes(value) {
    const units = ["B", "KB", "MB", "GB", "TB"];
    let amount = Number(value) || 0, index = 0;
    while (amount >= 1024 && index < units.length - 1) { amount /= 1024; index++; }
    return `${amount.toLocaleString("pt-BR", { maximumFractionDigits: amount >= 100 || index === 0 ? 0 : 1 })} ${units[index]}`;
  }
  const isBytes = unit => /^(bytes?|b)$/i.test(String(unit || "").trim());
  const quantity = (value, unit) => isBytes(unit) ? bytes(value) : `${number(value)}${unit ? ` ${unit}` : ""}`;
  function clock(seconds) {
    const total = Math.max(0, Math.round(seconds));
    const h = Math.floor(total / 3600), m = Math.floor(total / 60) % 60, s = total % 60;
    return h ? `${h} h ${String(m).padStart(2, "0")} min` : m ? `${m} min ${String(s).padStart(2, "0")} s` : `${s} s`;
  }

  // "Convertendo fonte; interrupção reinicia esta etapa" → title + note.
  // "Verificando metadados · arquivo.json.gz" → title + the file as a secondary line.
  function split(text) {
    let title = String(text || "").trim(), note = "";
    const semicolon = title.indexOf("; ");
    if (semicolon > 0) { note = title.slice(semicolon + 2); title = title.slice(0, semicolon); note = note[0].toLocaleUpperCase("pt-BR") + note.slice(1); }
    const dot = title.indexOf(" · ");
    // File names and identifiers after "·" keep their own spelling.
    if (dot > 0) { note = [title.slice(dot + 3), note].filter(Boolean).join(" · "); title = title.slice(0, dot); }
    return { title, note };
  }

  // Every slot keeps its height: values change in place and missing ones show "—",
  // so the card (and the animation above it) never jumps while work goes on.
  const STATS = [["elapsed", "Tempo total"], ["phase", "Nesta etapa"], ["eta", "Restante"], ["rate", "Velocidade"]];
  let timing = null;
  const set = (id, value) => { const el = node(id); if (el && el.textContent !== value) el.textContent = value; };
  function stat(key, value) {
    const row = node("load-stats")?.querySelector(`[data-stat="${key}"] dd`);
    const shown = value == null || value === "" ? "—" : value;
    if (row && row.textContent !== shown) row.textContent = shown;
    row?.parentElement.classList.toggle("empty", shown === "—");
  }
  function clear() {
    const list = node("load-stats");
    if (list) list.replaceChildren(...STATS.map(([key, label]) => {
      const row = document.createElement("div"), term = document.createElement("dt"), detail = document.createElement("dd");
      row.className = "load-stat empty"; row.dataset.stat = key; term.textContent = label; detail.textContent = "—";
      row.append(term, detail); return row;
    }));
    for (const id of ["load-note", "load-amount", "load-of", "load-percent", "load-extra"]) set(id, "");
    node("load-extra")?.classList.remove("error");
    progressBar(null);
    timing = null;
  }
  function progressBar(progress) {
    const fill = node("load-bar-fill");
    if (!fill) return;
    const known = progress != null && Number.isFinite(progress);
    // Unknown totals keep the same track with a slow sweep instead of disappearing.
    fill.parentElement.classList.toggle("indeterminate", !known);
    fill.style.width = known ? `${Math.max(0, Math.min(100, progress))}%` : "";
  }

  function reset(first) {
    clear();
    node("load-steps")?.replaceChildren();
    count();
    render({ label: first });
    step(first);
  }

  function count() {
    const label = node("load-steps-count"), total = node("load-steps")?.children.length || 0;
    if (label) label.textContent = total ? `(${total})` : "";
  }
  function step(label) {
    const list = node("load-steps");
    if (!list || !label) return;
    for (const item of list.children) {
      item.classList.add("done");
      const icon = item.querySelector(".load-step-ico");
      if (icon && !icon.querySelector(".fa-check")) icon.innerHTML = '<i class="fas fa-check" aria-hidden="true"></i>';
    }
    const { title, note } = split(label), item = document.createElement("li");
    const icon = document.createElement("span"); icon.className = "load-step-ico";
    const spinner = document.createElement("span"); spinner.className = "load-step-spin"; icon.append(spinner);
    const textBox = document.createElement("span"); textBox.className = "load-step-text";
    const main = document.createElement("span"); main.textContent = title;
    textBox.append(main);
    if (note) { const small = document.createElement("small"); small.textContent = note; small.title = note; textBox.append(small); }
    item.append(icon, textBox); list.append(item);
    while (list.children.length > MAX_STEPS) list.firstElementChild.remove();
    list.scrollTop = list.scrollHeight;
    count();
  }

  /* metrics (optional, native receipts): { completed, total, unit, percent, rate,
     eta, elapsed, phaseSeconds, resumed, checkpoint, selected, error }. Without
     metrics, a plain detail text (other operations) is shown as the amount line. */
  function render({ label, detail = "", progress = null, metrics = null } = {}) {
    const title = node("load-title");
    if (!title) return;
    const phase = split(label || "Processando");
    set("load-title", phase.title); title.title = phase.title;
    set("load-note", phase.note); node("load-note").title = phase.note;

    const known = progress != null && Number.isFinite(progress);
    if (metrics && (metrics.completed > 0 || metrics.total > 0)) {
      set("load-amount", quantity(metrics.completed || 0, metrics.unit));
      set("load-of", metrics.total > 0 ? `de ${quantity(metrics.total, metrics.unit)}` : "processados");
    } else if (!metrics && detail) {
      set("load-amount", detail); set("load-of", "");
    } else { set("load-amount", ""); set("load-of", ""); }
    set("load-percent", known ? `${Math.floor(Math.max(0, Math.min(100, progress)))}%` : "");
    progressBar(progress);

    if (metrics) {
      timing = Number.isFinite(metrics.elapsed) ? { elapsed: metrics.elapsed, at: performance.now() } : timing;
      stat("elapsed", timing ? clock(timing.elapsed) : null);
      stat("phase", Number.isFinite(metrics.phaseSeconds) ? clock(metrics.phaseSeconds) : null);
      stat("eta", Number.isFinite(metrics.eta) ? `≈ ${clock(metrics.eta)}` : null);
      stat("rate", metrics.rate > 0 ? `${isBytes(metrics.unit) ? bytes(metrics.rate) : number(Math.round(metrics.rate))}${isBytes(metrics.unit) ? "/s" : ` ${metrics.unit || "itens"}/s`}` : null);
      // Rare figures and errors share one reserved line below the cards.
      const extra = [
        Number.isFinite(metrics.selected) ? `${number(metrics.selected)} selecionados` : "",
        metrics.resumed > 0 ? `${number(metrics.resumed)} retomados` : "",
        metrics.checkpoint > 0 ? `${number(metrics.checkpoint)} salvos para retomar` : "",
      ].filter(Boolean).join(" · ");
      set("load-extra", metrics.error || extra);
      node("load-extra").classList.toggle("error", !!metrics.error);
      node("load-extra").title = metrics.error || extra;
    } else {
      // A phase without a receipt must not inherit figures from previous work.
      timing = null;
      for (const [key] of STATS) stat(key, null);
      set("load-extra", ""); node("load-extra").classList.remove("error"); node("load-extra").title = "";
    }
  }

  // The total time keeps counting between receipts; no other value moves on its own.
  function tick() {
    if (timing) stat("elapsed", clock(timing.elapsed + (performance.now() - timing.at) / 1000));
  }

  function finish(ok) {
    if (!ok) return;
    for (const item of node("load-steps")?.children || []) {
      item.classList.add("done");
      const icon = item.querySelector(".load-step-ico");
      if (icon) icon.innerHTML = '<i class="fas fa-check" aria-hidden="true"></i>';
    }
  }

  return Object.freeze({ reset, step, render, tick, finish, split, quantity, clock });
})();
