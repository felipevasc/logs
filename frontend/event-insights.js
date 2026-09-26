/* Event detail: what the record says in security terms. Shown only when there is something to show. */
window.EventInsights = (() => {
  "use strict";
  const ENTITY_ROLES = new Set(["@user", "@src_ip", "@dst_ip", "@host", "@process", "@parent_process", "@domain", "@hash", "@url", "@file", "@dst_port", "@tool"]);
  const OUTCOME = { success: "sucesso", failure: "falha" };
  const SEV = { critical: "crítica", high: "alta", medium: "média", low: "baixa", info: "informativa" };
  let serial = 0;

  async function render(ev, pane) {
    const mine = ++serial;
    pane.querySelector(".insight-block")?.remove();
    let data;
    try { data = await api("event_insights", { event: ev }, { silent: true }); } catch { return; }
    if (mine !== serial || state.currentDetailEv !== ev || !pane.isConnected) return;
    const entities = data.entities.filter(e => ENTITY_ROLES.has(e.column));
    const rules = [...data.rules.map(r => ({ ...r, snippet: "", source: "detecção" })), ...data.threats.map(t => ({ ...t, source: t.category }))];
    if (!entities.length && !data.action && !rules.length && !data.decoded.length) return;
    const block = el("section", "insight-block");
    if (data.action) {
      const line = el("div", "insight-action");
      line.innerHTML = `<i class="fas fa-bolt" aria-hidden="true"></i><span></span>${data.outcome ? `<b class="outcome-${esc(data.outcome)}">${esc(OUTCOME[data.outcome] || data.outcome)}</b>` : ""}`;
      line.querySelector("span").textContent = data.action_label || data.action;
      line.title = `Ação normalizada: ${data.action}${data.outcome ? ` · ${data.outcome}` : ""}`;
      block.append(line);
    }
    if (entities.length) {
      const chips = el("div", "insight-entities");
      for (const e of entities) chips.append(window.EntityMenu.chip({ column: e.column, value: e.value, scope: e.scope, first: ev.timestamp, last: ev.timestamp }));
      block.append(chips);
    }
    if (rules.length) {
      const list = el("div", "insight-rules");
      for (const r of rules.slice(0, 8)) {
        const row = el("div", `insight-rule sev-${r.severity}`);
        row.innerHTML = `<span class="sec-dot"></span><span class="insight-rule-name"></span><small></small>`;
        row.querySelector(".insight-rule-name").textContent = r.name;
        row.querySelector("small").textContent = r.attack.map(a => a.id).slice(0, 3).join(" ");
        row.title = `${r.source} · severidade ${SEV[r.severity] || r.severity}${r.attack.length ? `\n${r.attack.map(a => `${a.id} ${a.name}`).join("\n")}` : ""}${r.snippet ? `\n\nTrecho${r.normalized ? " (decodificado)" : ""}: ${r.snippet}` : ""}`;
        list.append(row);
      }
      if (rules.length > 8) list.append(el("small", "muted", `+${rules.length - 8} regras`));
      block.append(list);
    }
    for (const d of data.decoded.slice(0, 3)) {
      const details = el("details", "insight-decoded");
      const summary = el("summary", "", `${d.kind} decodificado · ${d.source}`);
      const pre = el("pre", "code-pane", d.text);
      details.append(summary, pre);
      block.append(details);
    }
    pane.prepend(block);
  }
  return { render };
})();
