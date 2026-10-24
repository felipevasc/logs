/* Expression search inside Explorar → Eventos. The panel drafts and tests on a
 * sample; only the Apply button adds a `search` filter, so the expression takes part
 * in the real filter state: the page, its count, grouping, exports, SQL with current
 * filters and every other consumer see the same set. */
window.EventPatternSearch = (() => {
  "use strict";
  const ENGINE_LABEL = { literal: "texto", rust: "Rust", pcre2: "PCRE2" };
  let panel = null, bench = null, editing = null;
  const fieldList = () => {
    const seen = new Set(["message", "raw"]);
    const rest = (state.columns || []).filter((c) => !seen.has(c) && !["id", "event_ref", "timestamp"].includes(c));
    return [["message", "Mensagem"], ["raw", "Conteúdo bruto"], ...rest.map((c) => [c, colLabel(c)])];
  };
  const optionsOf = (draft) => JSON.stringify({ engine: draft.engine, caseSensitive: draft.caseSensitive, multiline: draft.multiline, dotAll: draft.dotAll, scope: draft.scope });
  function filterOf(draft) { return { column: draft.field, op: "search", value: draft.pattern, value2: optionsOf(draft) }; }
  function parse(filter) {
    let o = {};
    try { o = JSON.parse(filter.value2 || "{}"); } catch { o = {}; }
    return { field: filter.column, pattern: filter.value, engine: o.engine || "rust", caseSensitive: !!o.caseSensitive, multiline: !!o.multiline, dotAll: !!o.dotAll, scope: o.scope || "event" };
  }
  function chipLabel(filter) {
    const d = parse(filter), text = d.pattern.length > 48 ? `${d.pattern.slice(0, 48)}…` : d.pattern;
    return `${colLabel(d.field)} · ${ENGINE_LABEL[d.engine] || d.engine} /${text}/${d.caseSensitive ? " Aa" : ""}${d.scope === "line" ? " · linha" : ""}`;
  }
  function ensurePanel() {
    if (panel) return panel;
    const host = document.querySelector("#tab-table");
    if (!host) return null;
    panel = document.createElement("details");
    panel.id = "expr-search"; panel.className = "expr-panel";
    panel.innerHTML = '<summary><i class="fas fa-code" aria-hidden="true"></i> Pesquisa por expressão <small>Texto literal, regex Rust ou PCRE2 · aplicada ao conjunto só quando você pedir</small></summary><div class="expr-body"></div>';
    host.prepend(panel);
    panel.addEventListener("toggle", () => { if (panel.open) ensureBench(); });
    return panel;
  }
  function ensureBench() {
    ensurePanel();
    if (bench) return bench;
    const saved = window.LocalExplorer?.expression() || {};
    bench = window.RegexWorkbench.create(panel.querySelector(".expr-body"), {
      mode: "search", fields: fieldList(), applyLabel: "Aplicar ao conjunto de eventos", pattern: saved.pattern || "", engine: saved.engine || "rust",
      field: saved.field || "message", caseSensitive: saved.caseSensitive, multiline: saved.multiline, dotAll: saved.dotAll, scope: saved.scope || "event",
      sampleOpen: false,
      sampleProvider: () => (state.currentDetailEv?.event_ref ? { id: state.currentDetailEv.id, event_ref: state.currentDetailEv.event_ref } : null),
      onChange: (draft) => window.LocalExplorer?.setExpression(draft),
      onSaveCapture: (draft, capture) => window.FieldExtraction?.open({ pattern: draft, capture, source: draft.field }),
      onApply: async (draft) => {
        const filter = filterOf(draft);
        const owner = window.AnalysisContexts?.capture();
        if (owner && !window.AnalysisContexts.isCurrent(owner)) throw Error("O Caso ou a fonte mudou. Revise a expressão e aplique de novo.");
        if (editing && state.filters.includes(editing)) {
          state.filters[state.filters.indexOf(editing)] = filter; editing = null; filtersChanged();
        } else if (!state.filters.some((f) => f.op === "search" && f.column === filter.column && f.value === filter.value && f.value2 === filter.value2)) {
          addFilter(filter);
        } else { toast("Esta expressão já está aplicada.", "info"); return; }
        toast("Expressão aplicada ao conjunto de eventos.", "ok");
      },
    });
    return bench;
  }
  function open({ focus = true } = {}) {
    if (!ensurePanel()) return null;
    if (window.Workspace?.page?.() !== "explore") void window.Workspace?.showPage("explore");
    // The panel lives in the Registros tab; bring that tab forward when another one is active.
    if (typeof switchTab === "function" && state.activeDatasetTab && state.activeDatasetTab !== "table") switchTab("table");
    panel.open = true;
    const b = ensureBench();
    b.setFields(fieldList());
    panel.scrollIntoView?.({ block: "nearest" });
    if (focus) b.focus();
    return b;
  }
  /** Chip → edit: the expression returns to the editor as a draft; nothing is applied until the button. */
  function edit(index) {
    const filter = state.filters[index];
    if (!filter || filter.op !== "search") return;
    editing = filter;
    const b = open();
    b?.setDraft(parse(filter));
  }
  /** A selection made in an event's text: literal example for the assistant. */
  async function fromSelection({ text, column, event }) {
    // The drawer would cover the editor; the event and the selection are already captured.
    if (typeof closeDrawer === "function" && !document.querySelector("#drawer")?.hidden) closeDrawer();
    const b = open({ focus: false });
    if (!b) return;
    editing = null;
    b.setDraft({ field: ["message", "raw"].includes(column) || fieldList().some(([c]) => c === column) ? column : "message" });
    try {
      if (event?.event_ref) {
        const got = await window.LocalNative.call({ action: "sample_text", id: event.id, event_ref: event.event_ref, field: column }, { latest: "rx-sample-selection" });
        b.setSample(got.text, { label: `Evento #${event.id} · ${column}`, truncated: got.truncated });
      }
    } catch (error) { if (!window.LocalNative.superseded(error)) toast(`A amostra completa não pôde ser lida: ${String(error.message || error)}`, "info"); }
    b.useSelection(text);
  }
  function mountButton() {
    const actions = document.querySelector(".filter-actions");
    if (!actions || document.querySelector("#btn-add-expr")) return;
    const button = document.createElement("button");
    button.id = "btn-add-expr"; button.type = "button"; button.className = "btn ghost small";
    button.title = "Pesquisar por texto literal, regex Rust ou PCRE2";
    button.innerHTML = '<i class="fas fa-code" aria-hidden="true"></i> Expressão';
    button.onclick = () => { editing = null; open(); };
    actions.insertBefore(button, document.querySelector("#btn-clear-filters"));
  }
  mountButton();
  document.addEventListener("analysis-context-change", () => {
    // Samples, previews and selections belong to the Case and source that produced them.
    editing = null;
    if (bench) { bench.destroy(); bench = null; if (panel?.open) ensureBench(); }
  });
  return { open, edit, chipLabel, fromSelection, filterOf };
})();
