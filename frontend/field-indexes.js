/* Optional field indexes belong to the exact active Case and loaded source. */
window.FieldIndexes = (() => {
  "use strict";
  let fields = new Set(), owner = null, version = 0, pending = null, loadingKey = null;
  const current = captured => workspaceScope() === "dataset" && state.loaded && captured && window.AnalysisContexts.isCurrent(captured);
  const has = column => current(owner) && fields.has(column);
  function paint() {
    document.querySelectorAll(".field-row[data-column],.detail-tree-name[data-column]").forEach(node => {
      const indexed = has(node.dataset.column);
      node.classList.toggle("field-indexed", indexed);
      const control = node.querySelector(".field-item") || node;
      const hint = "Índice de valores criado neste Caso";
      const baseTitle = (control.title || "").replace(`\n${hint}`, "");
      control.title = indexed ? `${baseTitle}\n${hint}` : baseTitle;
      if (indexed) node.setAttribute("data-index-hint", "Índice de valores criado neste Caso");
      else node.removeAttribute("data-index-hint");
    });
  }
  function reset() { version++; fields = new Set(); owner = null; pending = null; loadingKey = null; paint(); }
  function refresh() {
    if (workspaceScope() !== "dataset" || !state.loaded || state.sourceIdentityUnconfirmed) { reset(); return Promise.resolve(); }
    const captured = window.AnalysisContexts.capture(), key = JSON.stringify(captured);
    if (loadingKey === key) return pending || Promise.resolve();
    const request = ++version; loadingKey = key;
    pending = api("field_index_status", {}, { silent: true, analysisOwner: captured }).then(result => {
      if (request !== version || !current(captured)) return;
      fields = new Set(result.fields); owner = captured; paint();
    }).catch(() => { if (request === version) { fields = new Set(); owner = null; loadingKey = null; paint(); } })
      .finally(() => { if (request === version) pending = null; });
    return pending;
  }
  async function create(column, captured) {
    if (!current(captured)) return;
    try {
      const result = await api("field_index_create", { column }, { analysisOwner: captured, latest: "field-index" });
      if (!current(captured)) return;
      fields = new Set(result.fields); owner = captured; loadingKey = JSON.stringify(captured); paint();
      toast(`Índice criado para ${colLabel(column)}.`, "ok");
    } catch (error) { if (current(captured)) toast(String(error), "err"); }
  }
  function menuItem(column) {
    const captured = window.AnalysisContexts.capture();
    return has(column)
      ? { icon: "fa-check", label: "Índice de valores já criado", disabled: true }
      : { icon: "fa-bolt", label: "Criar índice para este campo/token", disabled: !current(captured) || column === "_all", onClick: () => create(column, captured) };
  }
  document.addEventListener("workspace-context-change", () => { reset(); void refresh(); });
  document.addEventListener("analysis-context-change", () => { reset(); void refresh(); });
  return { refresh, reset, has, menuItem, paint };
})();
