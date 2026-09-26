/* Entity pivots: one menu for users, addresses, hosts, processes and indicators, wherever they appear. */
window.EntityMenu = (() => {
  "use strict";
  const ICONS = { "@user": "fa-user", "@src_ip": "fa-network-wired", "@dst_ip": "fa-network-wired", "@host": "fa-server", "@process": "fa-gear", "@parent_process": "fa-gears", "@cmdline": "fa-terminal", "@url": "fa-link", "@domain": "fa-globe", "@hash": "fa-fingerprint", "@file": "fa-file", "@dst_port": "fa-plug", "@user_agent": "fa-robot", "@tool": "fa-toolbox" };
  const LABELS = { "@user": "Usuário", "@src_ip": "IP de origem", "@dst_ip": "IP de destino", "@host": "Host", "@process": "Processo", "@parent_process": "Processo pai", "@cmdline": "Linha de comando", "@url": "URL", "@domain": "Domínio", "@hash": "Hash", "@dst_port": "Porta de destino", "@user_agent": "User agent", "@file": "Arquivo", "@status": "Status", "@action": "Ação", "@outcome": "Resultado", "@src_scope": "Rede de origem", "@dst_scope": "Rede de destino", "@tool": "Ferramenta" };
  const icon = column => ICONS[column] || "fa-tag";
  const label = column => LABELS[column] || (typeof colLabel === "function" ? colLabel(column) : column);
  const TIME_WINDOW = 30 * 60000;

  // Kind of a free value (table text, indicators): decides the column used for pivots.
  function guess(value) {
    const v = String(value || "").trim();
    if (/^(?:\d{1,3}\.){3}\d{1,3}$/.test(v) || (/^[0-9a-f:]+$/i.test(v) && v.split(":").length > 2)) return "@src_ip";
    if (/^[a-f0-9]{32}$|^[a-f0-9]{40}$|^[a-f0-9]{64}$/i.test(v)) return "@hash";
    if (/^https?:\/\//i.test(v)) return "@url";
    if (/^[a-z0-9-]+(\.[a-z0-9-]+)+$/i.test(v) && /[a-z]/i.test(v.split(".").pop())) return "@domain";
    return "";
  }
  const isAddress = column => column === "@src_ip" || column === "@dst_ip";

  async function inDataset() {
    if (workspaceScope() === "case" && window.WorkspaceContext) await window.WorkspaceContext.setScope("dataset", { animate: false });
    return state.loaded;
  }
  function filtersFor(column, value) {
    // An address may be the origin in one source and the destination in another.
    if (isAddress(column)) return [{ column: "_all", op: "query", value: `@src_ip:"${value.replace(/"/g, '\\"')}" OR @dst_ip:"${value.replace(/"/g, '\\"')}"` }];
    if (column) return [{ column, op: "equals_exact", value }];
    return [{ column: "_all", op: "query", value: `"${value.replace(/"/g, '\\"')}"` }];
  }
  function copy(value) {
    navigator.clipboard?.writeText(value).then(() => toast("Copiado.", "ok"), () => toast("Não foi possível copiar.", "err"));
  }

  /** Menu entries for an entity: {column, value, first?, last?}. */
  function items(entity) {
    const value = String(entity.value ?? "");
    const column = entity.column || guess(value);
    const list = [
      { icon: "fa-filter", label: "Filtrar por este valor", onClick: () => window.Workspace.applyFilters(filtersFor(column, value)) },
      { icon: "fa-filter-circle-xmark", label: "Ocultar este valor", onClick: () => window.Workspace.applyFilters([{ column: column || "_all", op: column ? "not_equals_exact" : "query", value: column ? value : `-"${value.replace(/"/g, '\\"')}"` }]) },
      { icon: "fa-timeline", label: "Ver na linha do tempo", onClick: () => window.Workspace.applyFilters(filtersFor(column, value), false, "timeline") },
      { icon: "fa-magnifying-glass", label: "Procurar em todas as fontes", onClick: async () => { if (await inDataset()) window.Workspace.search(`"${value.replace(/"/g, '\\"')}"`); } },
    ];
    if (["@user", "@src_ip", "@dst_ip", "@host"].includes(column) && window.Journeys?.openValue) {
      list.push({ icon: "fa-code-branch", label: "Seguir entre fontes", onClick: () => {
        const around = entity.first != null ? { from: entity.first - TIME_WINDOW, to: (entity.last ?? entity.first) + TIME_WINDOW } : {};
        window.Journeys.openValue({ field: column, value, ...around });
      } });
    }
    list.push({ sep: true });
    if (window.CaseIntel) list.push({ icon: "fa-crosshairs", label: "Adicionar aos indicadores do Caso", onClick: () => window.CaseIntel.addIndicator({ value, column }) });
    list.push({ icon: "fa-copy", label: "Copiar valor", onClick: () => copy(value) });
    return list;
  }
  function open(x, y, entity) { showCtxMenu(x, y, items(entity)); }

  /** Compact chip that opens the pivots on click or right-click. */
  function chip(entity, extra = "") {
    const column = entity.column || guess(entity.value);
    const node = el("button", `entity-chip ${extra}`.trim());
    node.type = "button";
    node.innerHTML = `<i class="fas ${icon(column)}" aria-hidden="true"></i><span></span>`;
    node.querySelector("span").textContent = entity.display || entity.value;
    node.title = `${label(column)}: ${entity.value}${entity.scope ? ` · rede ${entity.scope}` : ""}\nClique para ações`;
    const act = event => { event.preventDefault(); event.stopPropagation(); const r = node.getBoundingClientRect(); open(event.clientX || r.left, event.clientY || r.bottom, { ...entity, column }); };
    node.onclick = act; node.oncontextmenu = act;
    return node;
  }

  // Values inside table text are pivots on right-click, invisible until hovered.
  const TOKEN = /\b(?:(?:\d{1,3}\.){3}\d{1,3})\b|\b[a-f0-9]{64}\b|\b[a-f0-9]{40}\b|\b[a-f0-9]{32}\b|https?:\/\/[^\s"'<>]{4,200}/gi;
  function highlight(text) {
    const source = String(text ?? "");
    let out = "", last = 0;
    for (const match of source.matchAll(TOKEN)) {
      const value = match[0];
      if (/^(?:\d{1,3}\.){3}\d{1,3}$/.test(value) && value.split(".").some(part => +part > 255)) continue;
      out += esc(source.slice(last, match.index)) + `<span class="ent" data-entity="${esc(value)}">${esc(value)}</span>`;
      last = match.index + value.length;
    }
    return last ? out + esc(source.slice(last)) : null;
  }
  document.addEventListener("contextmenu", event => {
    const token = event.target.closest?.(".ent[data-entity]");
    if (!token) return;
    event.preventDefault(); event.stopImmediatePropagation();
    open(event.clientX, event.clientY, { value: token.dataset.entity });
  }, true);

  return { open, items, chip, icon, label, guess, highlight, filtersFor };
})();
