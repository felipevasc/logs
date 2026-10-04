/* Measured, operation-owned verification progress shared by the page and task dialog. */
window.CompromiseProgress = (() => {
  "use strict";
  function merge(previous, incoming) {
    if (!incoming || !Number.isSafeInteger(incoming.revision) || incoming.revision <= (previous?.revision || 0)) return previous;
    const items = new Map(incoming.reset ? [] : (previous?.items || []).map(item => [item.id, item]));
    for (const item of incoming.items || []) if (typeof item.id === "string") items.set(item.id, item);
    return { ...incoming, items: [...items.values()] };
  }
  const coverage = { complete: "Cobertura completa", partial: "Cobertura parcial", not_applicable: "Sem registros aplicáveis", missing_fields: "Campos necessários ausentes", missing_coverage: "Cobertura necessária não comprovada" };
  const states = { pending: "Aguardando", running: "Verificando", completed: "Verificada" };
  const findingCount = count => `${fmtNum(count)} ${count === 1 ? "indício" : "indícios"}`;
  function mount(host, { expanded = false } = {}) {
    host.classList.add("compromise-progress");
    host.innerHTML = `<p class="cp-current" role="status" aria-live="polite"></p><div class="cp-totals"></div><progress class="cp-bar" aria-label="Verificações concluídas"></progress><p class="small muted">Prévia do cálculo em andamento. Os achados e a cobertura podem mudar até a conclusão.</p><details class="cp-details" ${expanded ? "open" : ""}><summary>Ver verificações e achados parciais</summary><label class="cp-filter">Mostrar <select aria-label="Filtrar verificações"><option value="all">Todas</option><option value="completed">Verificadas</option><option value="pending">Restantes</option><option value="findings">Com indícios</option></select></label><div class="cp-items"></div><button type="button" class="btn ghost small" data-cp-more>Ver mais verificações</button><p class="cp-page small muted"></p></details>`;
    const current = host.querySelector(".cp-current"), totals = host.querySelector(".cp-totals"), bar = host.querySelector(".cp-bar"), list = host.querySelector(".cp-items"), filter = host.querySelector("select"), more = host.querySelector("[data-cp-more]"), page = host.querySelector(".cp-page");
    let entry, revision = -1, limit = 40, destroyed = false;
    const rows = new Map();
    function drawItems() {
      const data = entry?.triage;
      const items = (data?.items || []).filter(item => filter.value === "all" || (filter.value === "pending" ? item.state !== "completed" : filter.value === "findings" ? item.findings > 0 : item.state === "completed"));
      const shown = items.slice(0, limit), keep = new Set(shown.map(item => item.id));
      for (const [id, row] of rows) if (!keep.has(id)) { row.remove(); rows.delete(id); }
      for (const [index, item] of shown.entries()) {
        let row = rows.get(item.id);
        if (!row) { row = document.createElement("details"); row.className = "cp-item"; row.innerHTML = '<summary><span class="cp-state"></span><span class="cp-name"></span><strong class="cp-count"></strong></summary><div class="cp-item-detail small"></div>'; rows.set(item.id, row); }
        row.dataset.state = item.state;
        row.querySelector(".cp-state").textContent = states[item.state] || "Aguardando";
        row.querySelector(".cp-name").textContent = item.name;
        row.querySelector(".cp-count").textContent = findingCount(item.findings || 0);
        const metrics = [];
        if (item.coverage) metrics.push(coverage[item.coverage] || item.coverage);
        if (Number.isSafeInteger(item.eligible)) metrics.push(`${fmtNum(item.eligible)} / ${fmtNum(item.applicable)} registros elegíveis`);
        for (let n = 5; n >= 1; n--) if (item.countsByLevel?.[n - 1]) metrics.push(`${window.EvidenceUI?.label(n) || `Nível ${n}`}: ${fmtNum(item.countsByLevel[n - 1])}`);
        const unassessed = (item.findings || 0) - (item.countsByLevel || []).reduce((sum, count) => sum + count, 0);
        if (unassessed > 0) metrics.push(`Nível não avaliado: ${fmtNum(unassessed)}`);
        row.querySelector(".cp-item-detail").textContent = metrics.join(" · ") || "Aguardando a verificação desta regra.";
        // Preserve open details and focus; only move a row if its order changed.
        if (list.children[index] !== row) list.insertBefore(row, list.children[index] || null);
      }
      more.hidden = shown.length >= items.length;
      page.textContent = data ? `${fmtNum(shown.length)} de ${fmtNum(items.length)} verificações neste filtro` : "Aguardando informações das verificações.";
    }
    filter.onchange = () => { limit = 40; drawItems(); };
    more.onclick = () => { limit += 40; drawItems(); };
    function update(next) {
      if (destroyed) return;
      entry = next;
      const data = entry?.triage, active = data?.items.find(item => item.id === data.current);
      const phase = entry?.progress?.phase || "Preparando verificações…";
      current.textContent = entry?.cancelled ? "Cancelando cálculo…" : active ? `Verificando: ${active.name}` : phase;
      if (!data) { totals.textContent = "Aguardando as contagens do cálculo."; bar.removeAttribute("value"); return; }
      totals.textContent = `${fmtNum(data.completed)} de ${fmtNum(data.total)} verificadas · ${fmtNum(Math.max(0, data.total - data.completed))} restantes · ${findingCount(data.findings)} até agora`;
      bar.max = Math.max(1, data.total); bar.value = data.completed;
      if (revision !== data.revision) { revision = data.revision; drawItems(); }
    }
    update(null);
    return { update, destroy() { destroyed = true; rows.clear(); filter.onchange = null; more.onclick = null; } };
  }
  return Object.freeze({ merge, mount });
})();
