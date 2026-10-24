/* Context of a prepared event, shown as a tab of the detail drawer so it stays next
 * to the investigation. Neighbours come from the source, a time window or an
 * identifier. Presentation filters are ignored; Case exclusions and visibility are not. */
window.EventContext = (() => {
  "use strict";
  const e = (t) => String(t ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const number = (v) => Number(v || 0).toLocaleString("pt-BR");
  const tab = document.createElement("button");
  tab.className = "dtab"; tab.type = "button"; tab.dataset.pane = "context"; tab.textContent = "Contexto";
  document.querySelector("#drawer .drawer-tabs")?.append(tab);
  const pane = document.createElement("div");
  pane.id = "pane-context"; pane.className = "ec-pane"; pane.hidden = true;
  pane.innerHTML = `<div class="ec-bar"><label>Relação<select data-f="mode" aria-label="Relação com o evento"><option value="physical">Vizinhança na fonte</option><option value="temporal">Janela de tempo</option><option value="identifier">Mesmo identificador</option></select></label><label data-for="temporal">Segundos ±<input data-f="seconds" type="number" min="1" max="86400" value="120"></label><label data-for="identifier">Campo<input data-f="field" value="trace_id" spellcheck="false"></label><label>Texto<select data-f="scope" aria-label="Texto exibido"><option value="message">Mensagem</option><option value="event">Registro completo</option><option value="line">Linha física</option></select></label><button type="button" class="btn ghost small" data-act="reload">Atualizar</button></div><p class="ec-help">Os filtros de apresentação são ignorados. Exclusões do Caso e a visibilidade obrigatória continuam valendo.</p><div class="ec-status" role="status" aria-live="polite"></div><div class="ec-rows" tabindex="0" aria-label="Eventos do contexto"></div>`;
  document.querySelector("#drawer .drawer-body")?.append(pane);
  const q = (s) => pane.querySelector(s), f = (n) => pane.querySelector(`[data-f="${n}"]`);
  let serial = 0, shownFor = null;
  const sync = () => {
    pane.querySelectorAll("[data-for]").forEach((n) => (n.hidden = n.dataset.for !== f("mode").value));
  };
  function status(text, error = false) { const n = q(".ec-status"); n.textContent = text; n.dataset.error = String(error); n.hidden = !text; }
  async function load() {
    const ev = state.currentDetailEv;
    if (!ev?.event_ref) { q(".ec-rows").innerHTML = ""; status("Abra um evento preparado para ver o seu contexto."); return; }
    const request = ++serial, stamp = window.LocalNative.stamp();
    const current = () => request === serial && !pane.hidden && state.currentDetailEv === ev && window.LocalNative.stamp() === stamp;
    status("Buscando eventos vizinhos…");
    q(".ec-rows").setAttribute("aria-busy", "true");
    try {
      if (state.detailSourceSpec && JSON.stringify(state.detailSourceSpec) !== JSON.stringify(state.currentArtifact?.source) && !await loadData(state.detailSourceSpec)) { status("Não foi possível reabrir a fonte deste registro.", true); return; }
      if (!current()) return;
      const result = await window.LocalNative.call({ action: "context", id: ev.id, event_ref: ev.event_ref, mode: f("mode").value, radius: 12, seconds: Math.max(1, +f("seconds").value || 120), field: f("field").value || null, pattern: null, subjectScope: f("scope").value }, { latest: "event-context" });
      if (!current()) return;
      const marks = await window.LocalNative.call({ action: "highlights", rules: window.LocalExplorer?.rules() || [], texts: result.rows.map((r) => r.text) }, { latest: "event-context-colors" });
      if (!current()) return;
      result.rows.forEach((r, i) => (r.highlights = marks[i]));
      shownFor = ev;
      status(result.more ? "Mostrando os registros mais próximos; há mais fora deste recorte." : `${number(result.rows.length)} registro${result.rows.length === 1 ? "" : "s"} no contexto.`);
      q(".ec-rows").innerHTML = result.rows.map((row, i) => `<div class="ec-row" data-center="${row.id === ev.id}"><button type="button" class="ec-id" data-i="${i}" title="Abrir este evento nos detalhes">#${number(row.id)}</button><div class="ec-body"><small>${row.timestamp != null ? e(new Date(row.timestamp).toLocaleString("pt-BR")) : "sem horário"} · ${e(row.level)} · ${e(row.source)}</small><pre>${row.truncatedBefore ? "… " : ""}${window.LocalExplorer.paint(row.text || "", row.spans, row.highlights, row.id === ev.id)}${row.truncatedAfter ? " …" : ""}</pre></div></div>`).join("") || '<div class="ec-empty">Nenhum registro encontrado com esta relação.</div>';
      q(".ec-rows").querySelectorAll("[data-i]").forEach((b) => (b.onclick = () => { const row = result.rows[+b.dataset.i]; if (row.id !== ev.id) void openDetail(row.id, { eventRef: row.event_ref }); }));
      q(".ec-rows [data-center=true]")?.scrollIntoView({ block: "center" });
    } catch (error) {
      if (current() && !window.LocalNative.superseded(error)) { q(".ec-rows").innerHTML = ""; status(String(error.message || error), true); }
    } finally { if (request === serial) q(".ec-rows").removeAttribute("aria-busy"); }
  }
  f("mode").onchange = () => { sync(); void load(); };
  for (const n of ["seconds", "field", "scope"]) f(n).onchange = () => void load();
  q('[data-act="reload"]').onclick = () => void load();
  sync();
  return {
    /** Called whenever the drawer shows another event. */
    onDetail(ev) {
      tab.hidden = !ev?.event_ref;
      serial++; shownFor = null;
      q(".ec-rows").innerHTML = ""; status("");
      if (!pane.hidden) { if (ev?.event_ref) void load(); else window.switchDetailTab?.("overview"); }
    },
    show: () => { window.switchDetailTab("context"); },
    /** Called by the drawer when the Contexto tab becomes visible. */
    activated() { if (shownFor !== state.currentDetailEv) void load(); },
    deactivated() { serial++; window.LocalNative.cancel("event-context"); },
  };
})();
