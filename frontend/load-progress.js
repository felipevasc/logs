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

  let timing = null;
  function stat(key, label, value) {
    const list = node("load-stats");
    if (!list) return;
    let row = list.querySelector(`[data-stat="${key}"]`);
    if (value == null || value === "") { row?.remove(); return; }
    if (!row) {
      row = document.createElement("div"); row.className = "load-stat"; row.dataset.stat = key;
      const term = document.createElement("dt"), detail = document.createElement("dd");
      term.textContent = label; row.append(term, detail); list.append(row);
    }
    const detail = row.querySelector("dd");
    if (detail.textContent !== value) detail.textContent = value;
  }
  function clear() {
    for (const id of ["load-note", "load-amount", "load-of", "load-percent", "load-error"]) { const el = node(id); if (el) el.textContent = ""; }
    for (const id of ["load-note", "load-figures", "load-error"]) { const el = node(id); if (el) el.hidden = true; }
    node("load-stats")?.replaceChildren();
    const bar = node("load-bar-fill");
    if (bar) { bar.style.width = "0%"; bar.parentElement.hidden = true; }
    timing = null;
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
    count();
  }

  /* metrics (optional, native receipts): { completed, total, unit, percent, rate,
     eta, elapsed, phaseSeconds, resumed, checkpoint, selected, error }. Without
     metrics, a plain detail text (other operations) is shown as the amount line. */
  function render({ label, detail = "", progress = null, metrics = null } = {}) {
    const title = node("load-title");
    if (!title) return;
    const phase = split(label || "Processando");
    if (title.textContent !== phase.title) title.textContent = phase.title;
    const note = node("load-note");
    note.textContent = phase.note; note.hidden = !phase.note;

    const figures = node("load-figures"), amount = node("load-amount"), of = node("load-of"), percent = node("load-percent");
    const known = progress != null && Number.isFinite(progress);
    if (metrics && (metrics.completed > 0 || metrics.total > 0)) {
      amount.textContent = quantity(metrics.completed || 0, metrics.unit);
      of.textContent = metrics.total > 0 ? `de ${quantity(metrics.total, metrics.unit)}` : "processados";
      figures.hidden = false;
    } else if (!metrics && detail) {
      amount.textContent = detail; of.textContent = ""; figures.hidden = false;
    } else { amount.textContent = ""; of.textContent = ""; figures.hidden = true; }
    percent.textContent = known && !figures.hidden ? `${Math.floor(Math.max(0, Math.min(100, progress)))}%` : "";
    const bar = node("load-bar-fill");
    bar.parentElement.hidden = !known;
    if (known) bar.style.width = `${Math.max(0, Math.min(100, progress))}%`;

    if (metrics) {
      timing = Number.isFinite(metrics.elapsed) ? { elapsed: metrics.elapsed, at: performance.now() } : timing;
      stat("elapsed", "Tempo total", timing ? clock(timing.elapsed) : null);
      stat("phase", "Nesta etapa", Number.isFinite(metrics.phaseSeconds) ? clock(metrics.phaseSeconds) : null);
      stat("eta", "Restante", Number.isFinite(metrics.eta) ? `≈ ${clock(metrics.eta)}` : null);
      stat("rate", "Velocidade", metrics.rate > 0 ? `${isBytes(metrics.unit) ? bytes(metrics.rate) : number(Math.round(metrics.rate))}${isBytes(metrics.unit) ? "/s" : ` ${metrics.unit || "itens"}/s`}` : null);
      stat("selected", "Selecionados", Number.isFinite(metrics.selected) ? number(metrics.selected) : null);
      stat("resumed", "Retomados", metrics.resumed > 0 ? number(metrics.resumed) : null);
      stat("checkpoint", "Salvos p/ retomar", metrics.checkpoint > 0 ? number(metrics.checkpoint) : null);
      const error = node("load-error");
      error.textContent = metrics.error || ""; error.hidden = !metrics.error;
    } else {
      // A phase without a receipt must not inherit figures from previous work.
      timing = null;
      node("load-stats")?.replaceChildren();
      const error = node("load-error");
      error.textContent = ""; error.hidden = true;
    }
  }

  // The total time keeps counting between receipts; no other value moves on its own.
  function tick() {
    if (timing) stat("elapsed", "Tempo total", clock(timing.elapsed + (performance.now() - timing.at) / 1000));
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
