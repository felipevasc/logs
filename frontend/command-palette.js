/* Command palette (Ctrl+K): every page, action, hunting recipe and finding from the keyboard. */
window.CommandPalette = (() => {
  "use strict";
  const overlay = el("div", "palette-overlay");
  overlay.hidden = true;
  overlay.innerHTML = `<div class="palette" role="dialog" aria-modal="true" aria-label="Comandos"><div class="palette-input"><i class="fas fa-magnifying-glass" aria-hidden="true"></i><input type="text" placeholder="Buscar comando, página ou texto nos registros…" aria-label="Comando" spellcheck="false" autocomplete="off"><kbd>Esc</kbd></div><div class="palette-list" role="listbox"></div><div class="palette-foot"><span>↑↓ navegar</span><span>Enter executar</span><span>Ctrl+K abrir</span></div></div>`;
  document.body.append(overlay);
  const input = overlay.querySelector("input"), list = overlay.querySelector(".palette-list");
  let entries = [], shown = [], active = 0, returnFocus = null;
  const fold = text => String(text).normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
  const go = page => () => window.Workspace.showPage(page);
  const click = selector => () => document.querySelector(selector)?.click();

  function build() {
    const pages = [["summary", "Resumo", "fa-chart-simple"], ["case-timeline", "Linha do tempo", "fa-timeline"], ["explore", "Explorar", "fa-magnifying-glass"], ["compare", "Comparar períodos", "fa-code-compare"], ["evidence", "Evidências do Caso", "fa-bookmark"], ["case-trails", "Trilhas do Caso", "fa-route"], ["journeys", "Possíveis trilhas", "fa-code-branch"], ["sources", "Arquivos e fontes", "fa-folder-open"]];
    const list = [
      ...pages.map(([page, label, icon]) => ({ group: "Ir para", label, icon, run: go(page), keys: page })),
      { group: "Ações", label: "Abrir arquivos…", icon: "fa-file-circle-plus", run: click("#ws-open"), keys: "importar carregar logs" },
      { group: "Ações", label: "Exportar…", icon: "fa-arrow-up-from-bracket", run: click("#ws-export"), keys: "salvar csv jsonl pdf relatorio" },
      { group: "Ações", label: workspaceScope() === "case" ? "Mudar para Análise" : "Mudar para Caso", icon: "fa-right-left", run: () => window.WorkspaceContext?.setScope(workspaceScope() === "case" ? "dataset" : "case"), keys: "alternar contexto" },
      { group: "Ações", label: "Limpar filtros", icon: "fa-filter-circle-xmark", run: click("#ws-clear-scope"), keys: "remover filtros busca" },
      { group: "Ações", label: "Recalcular triagem", icon: "fa-rotate", run: () => { window.Security?.invalidate(); window.Workspace.showPage("summary"); }, keys: "atualizar deteccoes" },
      { group: "Ações", label: "Regras de detecção e Sigma", icon: "fa-shield-halved", run: () => openSettings("detection"), keys: "sigma importar ocultas configuracoes" },
      { group: "Ações", label: "Configurações e MCP", icon: "fa-sliders", run: () => openSettings("mcp"), keys: "preferencias agentes" },
      { group: "Ações", label: "Alternar tema claro/escuro", icon: "fa-circle-half-stroke", run: click("#btn-theme"), keys: "dark light" },
      ...(window.Security?.recipes() || []).map(r => ({ group: "Caçar", label: r.label, icon: r.icon, run: r.run, keys: r.query })),
    ];
    const data = window.Security?.last();
    (data?.episodes || []).slice(0, 12).forEach((episode, index) => {
      list.push({ group: "Atenção", label: episode.title, detail: episode.summary, icon: "fa-triangle-exclamation", keys: episode.entities.map(e => e.value).join(" "), run: () => window.Security.openEpisode(index) });
    });
    return list;
  }
  function score(entry, words) {
    const hay = fold(`${entry.label} ${entry.group} ${entry.keys || ""} ${entry.detail || ""}`);
    let total = 0;
    for (const w of words) { const at = hay.indexOf(w); if (at < 0) return -1; total += at; }
    return total + (fold(entry.label).startsWith(words[0] || "") ? -50 : 0);
  }
  function dynamic(text) {
    const extra = [];
    const trimmed = text.trim();
    if (!trimmed) return extra;
    const kind = window.EntityMenu?.guess(trimmed);
    if (kind) extra.push({ group: "Entidade", label: `Ações para ${trimmed}`, icon: "fa-crosshairs", run: () => { const r = input.getBoundingClientRect(); window.EntityMenu.open(r.left + 40, r.bottom, { value: trimmed, column: kind }); }, pinned: true });
    const problem = window.QueryLang?.validate(trimmed);
    if (!problem) extra.push({ group: "Buscar", label: `Buscar “${trimmed}” nos registros`, icon: "fa-magnifying-glass", run: () => window.Workspace.search(trimmed), pinned: true });
    return extra;
  }
  function draw() {
    const words = fold(input.value).split(/\s+/).filter(Boolean);
    const ranked = entries.map(e => ({ e, s: words.length ? score(e, words) : 0 })).filter(x => x.s >= 0).sort((a, b) => a.s - b.s).map(x => x.e);
    shown = [...dynamic(input.value), ...ranked].slice(0, 40);
    active = Math.min(active, Math.max(0, shown.length - 1));
    let group = null;
    list.innerHTML = shown.map((item, i) => {
      const heading = item.group !== group ? `<div class="palette-group">${esc(item.group)}</div>` : "";
      group = item.group;
      return `${heading}<div class="palette-item${i === active ? " active" : ""}" role="option" data-index="${i}" aria-selected="${i === active}"><i class="fas ${esc(item.icon || "fa-circle")}" aria-hidden="true"></i><span>${esc(item.label)}</span>${item.detail ? `<small>${esc(item.detail)}</small>` : ""}</div>`;
    }).join("") || '<div class="palette-empty">Nada encontrado</div>';
    list.querySelector(".palette-item.active")?.scrollIntoView({ block: "nearest" });
  }
  function open() {
    returnFocus = document.activeElement;
    entries = build();
    input.value = ""; active = 0;
    overlay.hidden = false;
    draw();
    input.focus();
  }
  function close() { overlay.hidden = true; returnFocus?.focus?.(); }
  function run(index = active) {
    const item = shown[index];
    if (!item) return;
    close();
    Promise.resolve().then(item.run).catch(error => toast(String(error), "err"));
  }
  input.addEventListener("input", () => { active = 0; draw(); });
  input.addEventListener("keydown", event => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); if (!shown.length) return; active = (active + (event.key === "ArrowDown" ? 1 : -1) + shown.length) % shown.length; draw(); }
    else if (event.key === "Enter") { event.preventDefault(); run(); }
    else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
  });
  list.addEventListener("mousemove", event => { const item = event.target.closest("[data-index]"); if (item && +item.dataset.index !== active) { active = +item.dataset.index; draw(); } });
  list.addEventListener("click", event => { const item = event.target.closest("[data-index]"); if (item) run(+item.dataset.index); });
  overlay.addEventListener("mousedown", event => { if (event.target === overlay) close(); });
  document.addEventListener("keydown", event => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); overlay.hidden ? open() : close(); }
  });
  return { open, close };
})();
