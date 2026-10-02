/* Case indicators (IOCs) and hypotheses: compact until used, part of the case and its report. */
window.CaseIntel = (() => {
  "use strict";
  const STATUS = ["a verificar", "suspeito", "malicioso", "benigno"];
  const HYPOTHESIS = ["aberta", "confirmada", "descartada"];
  const KIND_ICON = { ip: "fa-network-wired", domain: "fa-globe", hash: "fa-fingerprint", url: "fa-link", user: "fa-user", host: "fa-server", file: "fa-file", other: "fa-tag" };
  const COLUMN_KIND = { "@src_ip": "ip", "@dst_ip": "ip", "@domain": "domain", "@hash": "hash", "@url": "url", "@user": "user", "@host": "host", "@file": "file" };
  let open = null, associationUndo = null;
  const nativeItems = () => window.CaseEvidence?.active === true ? window.CaseEvidenceItems : null;
  function linkedItems(c, h, bindings = nativeItems()?.index(c)) {
    if (bindings) return bindings.associations(h, { legacyField: "items" });
    return (h.items || []).map(id => { const matches = (c.items || []).filter(item => item.id === id); return { state: matches.length === 1 ? "unique" : matches.length ? "ambiguous" : "missing", item: matches.length === 1 ? matches[0] : null }; });
  }
  const associationLabel = entry => entry.item?.label || entry.item?.name || (entry.state === "ambiguous" ? "Associação ambígua" : "Associação indisponível");
  async function editAssociations(c, h, references) {
    try {
      associationUndo = await nativeItems().edit(c, h, references, { legacyField: "items", save: saveCases, owns: () => activeCase() === c && (c.intel?.hypotheses || []).includes(h) });
      refresh(); return true;
    } catch (error) { toast(String(error.message || error), "err"); return false; }
  }
  async function linkItem(c, h, item) {
    if (activeCase() !== c || !(c.items || []).includes(item) || !(c.intel?.hypotheses || []).includes(h)) return false;
    if (nativeItems()) {
      const refs = linkedItems(c, h).map(entry => entry.reference), reference = nativeItems().forItem(c, item);
      if (refs.some(entry => nativeItems().key(entry) === nativeItems().key(reference))) return false;
      return editAssociations(c, h, [...refs, reference]);
    }
    if (!(h.items || []).includes(item.id)) { (h.items ||= []).push(item.id); await persist(c, "Evidência vinculada à hipótese."); return true; }
    return false;
  }
  function associationMenu(c, h, anchor) {
    const entries = linkedItems(c, h), rectangle = anchor.getBoundingClientRect();
    showCtxMenu(rectangle.left, rectangle.bottom, entries.map((entry, index) => ({ icon: "fa-link", label: `${index + 1}. ${associationLabel(entry)}`, onClick: () => {
      if (activeCase() !== c || !(c.intel?.hypotheses || []).includes(h)) return;
      const refs = entries.map(value => value.reference), actions = [];
      if (entry.state === "unique") actions.push({ icon: "fa-eye", label: "Abrir item", onClick: () => nativeItems().open(entry.item) });
      actions.push({ icon: "fa-link-slash", label: "Desassociar item", onClick: () => editAssociations(c, h, refs.filter((_, position) => position !== index)) });
      for (const direction of [-1, 1]) if (index + direction >= 0 && index + direction < refs.length) actions.push({ icon: direction < 0 ? "fa-arrow-up" : "fa-arrow-down", label: direction < 0 ? "Mover acima" : "Mover abaixo", onClick: () => { const next = refs.slice(); [next[index], next[index + direction]] = [next[index + direction], next[index]]; return editAssociations(c, h, next); } });
      showCtxMenu(rectangle.left, rectangle.bottom, actions);
    } })));
  }
  const uid = prefix => prefix + Date.now().toString(36) + Math.floor(Math.random() * 1e4);
  const intel = c => (c.intel ||= { indicators: [], hypotheses: [] });
  const kindOf = (value, column) => COLUMN_KIND[column] || COLUMN_KIND[window.EntityMenu?.guess(value)] || "other";

  async function persist(c, message) {
    if (await saveCases()) { if (message) toast(message, "ok"); refresh(); }
  }
  function refresh() {
    if (document.body.dataset.page === "evidence") window.Workspace?.showPage("evidence");
  }

  function addIndicator({ value, column = "" }, silent = false) {
    const c = ensureCase();
    const clean = String(value || "").trim();
    if (!clean) return false;
    const list = intel(c).indicators;
    if (list.some(i => i.value.toLowerCase() === clean.toLowerCase())) { if (!silent) toast("Este indicador já está no Caso.", "info"); return false; }
    list.push({ id: uid("ioc"), value: clean, kind: kindOf(clean, column), status: "a verificar", note: "", createdAt: Date.now(), sightings: null });
    if (!silent) persist(c, "Indicador adicionado ao Caso.");
    return true;
  }

  async function extract(c) {
    let groups;
    try {
      const events = caseEvents("analysis");
      if (Array.isArray(events) && !events.length) { toast("Inclua registros no Caso para extrair indicadores.", "info"); return; }
      groups = await api("entity_summary", { filters: [], caseEvents: events, limit: 200 });
    } catch { return; }
    let added = 0;
    for (const group of groups) {
      if (!["@src_ip", "@dst_ip", "@domain", "@hash", "@url"].includes(group.column)) continue;
      for (const v of group.values) {
        if ((group.column === "@src_ip" || group.column === "@dst_ip") && v.scope && v.scope !== "público") continue;
        if (addIndicator({ value: v.value, column: group.column }, true)) added++;
      }
    }
    await persist(c, added ? `${fmtNum(added)} indicadores extraídos dos registros do Caso.` : "Nenhum indicador novo nos registros do Caso.");
  }

  async function sightings(c, only = null) {
    const list = intel(c).indicators.filter(i => !only || i.id === only.id);
    if (!list.length) return;
    // Sightings are computed on the loaded sources, not on the case copy.
    const values = list.map(i => i.value);
    let found;
    try { found = await api("ioc_sightings", { values, filters: [] }); } catch { return; }
    const byValue = new Map(found.map(s => [s.value.toLowerCase(), s]));
    for (const indicator of list) {
      const s = byValue.get(indicator.value.toLowerCase());
      indicator.sightings = s ? { count: s.count, first: s.first, last: s.last, sources: s.sources, checkedAt: Date.now() } : { count: 0, sources: [], checkedAt: Date.now() };
    }
    const hits = list.filter(i => i.sightings?.count).length;
    await persist(c, `${fmtNum(hits)} de ${fmtNum(list.length)} indicadores aparecem nas fontes abertas.`);
  }
  async function showInDataset(values) {
    if (workspaceScope() === "case" && window.WorkspaceContext) await window.WorkspaceContext.setScope("dataset", { animate: false });
    if (!state.loaded) { toast("Abra as fontes na Análise para procurar os indicadores.", "info"); return; }
    window.Workspace.search(values.map(v => `"${v.replace(/"/g, '\\"')}"`).join(" OR "));
  }

  function indicatorsPanel(c) {
    const list = intel(c).indicators;
    const panel = el("div", "intel-panel");
    panel.innerHTML = `<div class="intel-add"><input type="text" placeholder="Adicionar indicador: IP, domínio, hash, usuário…" aria-label="Novo indicador"><button type="button" class="btn ghost small" data-intel="extract" title="Endereços públicos, domínios, URLs e hashes dos registros do Caso">Extrair dos registros</button>${list.length ? '<button type="button" class="btn ghost small" data-intel="sightings" title="Procurar todos os indicadores nas fontes abertas">Onde aparecem</button><button type="button" class="icon-btn" data-intel="copy" title="Copiar lista" aria-label="Copiar lista"><i class="fas fa-copy"></i></button>' : ""}</div>
      <div class="intel-list">${list.map((i, index) => `<div class="intel-row status-${esc(i.status.replace(/\s/g, "-"))}" data-index="${index}"><i class="fas ${KIND_ICON[i.kind] || KIND_ICON.other}" aria-hidden="true"></i><code></code><button type="button" class="intel-status" title="Clique para mudar a classificação">${esc(i.status)}</button><span class="intel-seen">${i.sightings ? i.sightings.count ? `${fmtNum(i.sightings.count)} registros${i.sightings.sources?.length ? ` · ${i.sightings.sources.length} ${i.sightings.sources.length === 1 ? "fonte" : "fontes"}` : ""}` : "não encontrado" : ""}</span><span class="intel-actions"><button type="button" class="icon-btn" data-row="find" title="Ver onde aparece" aria-label="Ver onde aparece"><i class="fas fa-magnifying-glass"></i></button><button type="button" class="icon-btn" data-row="remove" title="Remover" aria-label="Remover"><i class="fas fa-xmark"></i></button></span></div>`).join("")}</div>`;
    panel.querySelectorAll(".intel-row").forEach(row => { row.querySelector("code").textContent = list[+row.dataset.index].value; });
    const input = panel.querySelector("input");
    input.onkeydown = event => { if (event.key === "Enter" && input.value.trim()) { addIndicator({ value: input.value }); input.value = ""; } };
    panel.onclick = async event => {
      const action = event.target.closest("[data-intel]")?.dataset.intel;
      if (action === "extract") return extract(c);
      if (action === "sightings") return sightings(c);
      if (action === "copy") return navigator.clipboard?.writeText(list.map(i => `${i.value}\t${i.kind}\t${i.status}`).join("\n")).then(() => toast("Lista copiada.", "ok"));
      const row = event.target.closest(".intel-row");
      if (!row) return;
      const indicator = list[+row.dataset.index];
      if (event.target.closest(".intel-status")) { indicator.status = STATUS[(STATUS.indexOf(indicator.status) + 1) % STATUS.length]; return persist(c); }
      const rowAction = event.target.closest("[data-row]")?.dataset.row;
      if (rowAction === "remove") { list.splice(+row.dataset.index, 1); return persist(c); }
      if (rowAction === "find") return showInDataset([indicator.value]);
    };
    panel.oncontextmenu = event => {
      const row = event.target.closest(".intel-row");
      if (!row) return;
      event.preventDefault();
      const indicator = list[+row.dataset.index];
      window.EntityMenu.open(event.clientX, event.clientY, { value: indicator.value, column: Object.keys(COLUMN_KIND).find(k => COLUMN_KIND[k] === indicator.kind) || "" });
    };
    return panel;
  }

  function hypothesesPanel(c) {
    const list = intel(c).hypotheses, bindings = nativeItems()?.index(c);
    const items = c.items || [];
    const panel = el("div", "intel-panel");
    panel.innerHTML = `<div class="intel-add"><input type="text" placeholder="Nova hipótese: o que pode ter acontecido?" aria-label="Nova hipótese"></div>
      <div class="intel-list">${list.map((h, index) => { const linked = linkedItems(c, h, bindings); return `<div class="hyp-row hyp-${esc(h.status)}" data-index="${index}"><button type="button" class="hyp-status" title="Clique para mudar o estado">${esc(h.status)}</button><span class="hyp-text" title="Duplo clique para editar"></span><button type="button" class="hyp-links" title="${esc(linked.map(associationLabel).join("\n") || "Arraste evidências para cá ou use o botão direito nelas")}">${linked.length ? `${linked.length} ${linked.length === 1 ? "evidência" : "evidências"}` : "sem evidências"}</button><button type="button" class="icon-btn" data-row="remove" title="Remover" aria-label="Remover"><i class="fas fa-xmark"></i></button></div>`; }).join("")}</div>`;
    panel.querySelectorAll(".hyp-row").forEach(row => { row.querySelector(".hyp-text").textContent = list[+row.dataset.index].text; });
    if (associationUndo?.case === c) {
      const undo = el("button", "btn ghost small", "Desfazer associação ou ordem"); undo.type = "button";
      undo.onclick = async () => { const receipt = associationUndo; undo.disabled = true; try { await nativeItems().undo(receipt, { save: saveCases, owns: () => activeCase() === c }); if (associationUndo === receipt) associationUndo = null; refresh(); } catch (error) { toast(String(error.message || error), "err"); undo.disabled = false; } }; panel.append(undo);
    }
    const input = panel.querySelector("input");
    input.onkeydown = event => {
      if (event.key !== "Enter" || !input.value.trim()) return;
      list.push({ id: uid("h"), text: input.value.trim(), status: "aberta", items: [], createdAt: Date.now() });
      input.value = ""; persist(c);
    };
    panel.onclick = event => {
      const row = event.target.closest(".hyp-row");
      if (!row) return;
      const h = list[+row.dataset.index];
      if (nativeItems() && event.target.closest(".hyp-links")) return associationMenu(c, h, event.target.closest(".hyp-links"));
      if (event.target.closest(".hyp-status")) { h.status = HYPOTHESIS[(HYPOTHESIS.indexOf(h.status) + 1) % HYPOTHESIS.length]; return persist(c); }
      if (event.target.closest("[data-row='remove']")) { list.splice(+row.dataset.index, 1); return persist(c); }
    };
    panel.ondblclick = event => {
      const text = event.target.closest(".hyp-text");
      if (!text) return;
      const h = list[+text.closest(".hyp-row").dataset.index];
      const editor = el("input", "hyp-edit"); editor.value = h.text;
      text.replaceWith(editor); editor.focus(); editor.select();
      const done = commit => { if (commit && editor.value.trim()) h.text = editor.value.trim(); persist(c); };
      editor.onkeydown = e => { if (e.key === "Enter") done(true); if (e.key === "Escape") refresh(); };
      editor.onblur = () => done(true);
    };
    // Evidence cards dropped on a hypothesis become linked evidence.
    panel.addEventListener("dragover", event => { const row = event.target.closest(".hyp-row"); if (row && event.dataTransfer.types.includes("text/case-item")) { event.preventDefault(); row.classList.add("drop"); } });
    panel.addEventListener("dragleave", event => event.target.closest?.(".hyp-row")?.classList.remove("drop"));
    panel.addEventListener("drop", event => {
      const row = event.target.closest(".hyp-row");
      const id = event.dataTransfer.getData("text/case-item");
      if (!row || !id) return;
      event.preventDefault();
      const h = list[+row.dataset.index];
      if (nativeItems()) {
        try { const payload = JSON.parse(id); if (payload.caseId !== c.id || payload.storeId !== state.cases.store.storeId) return; const entry = nativeItems().resolve(c, payload.reference); if (entry.state === "unique") void linkItem(c, h, entry.item); } catch { toast("A associação mudou. Arraste o item novamente.", "info"); }
      } else { const matches = items.filter(item => item.id === id); if (matches.length === 1) void linkItem(c, h, matches[0]); }
    });
    return panel;
  }

  /** Compact bar on the evidence page; panels open on demand. */
  function mount(container, c) {
    if (!c) return;
    const data = intel(c);
    const bar = el("div", "intel-bar");
    const tab = (key, icon, label, count) => `<button type="button" class="intel-tab${open === key ? " open" : ""}" data-tab="${key}" aria-expanded="${open === key}"><i class="fas ${icon}" aria-hidden="true"></i>${label}${count ? `<span>${fmtNum(count)}</span>` : ""}</button>`;
    bar.innerHTML = tab("iocs", "fa-crosshairs", "Indicadores", data.indicators.length) + tab("hyp", "fa-lightbulb", "Hipóteses", data.hypotheses.length);
    const holder = el("div", "intel-holder");
    if (open === "iocs") holder.append(indicatorsPanel(c));
    if (open === "hyp") holder.append(hypothesesPanel(c));
    bar.onclick = event => { const b = event.target.closest("[data-tab]"); if (!b) return; open = open === b.dataset.tab ? null : b.dataset.tab; refresh(); };
    const anchor = container.querySelector(".evidence-list-controls") || container.firstElementChild;
    anchor?.before(bar, holder);
    for (const card of container.querySelectorAll("[data-item-index]")) {
      const item = c.items[+card.dataset.itemIndex];
      if (!item) continue;
      card.draggable = true;
      card.addEventListener("dragstart", event => { if (activeCase() !== c || !c.items.includes(item)) { event.preventDefault(); return; } let data = item.id; if (nativeItems()) { try { data = JSON.stringify({ storeId: state.cases.store.storeId, caseId: c.id, reference: nativeItems().forItem(c, item) }); } catch { event.preventDefault(); return; } } event.dataTransfer.setData("text/case-item", data); event.dataTransfer.effectAllowed = "link"; });
    }
  }
  /** Extra context-menu entries for an evidence item. */
  function menuItems(item) {
    const c = activeCase(), bindings = nativeItems()?.index(c);
    const list = c ? intel(c).hypotheses.filter(h => !linkedItems(c, h, bindings).some(entry => entry.item === item)) : [];
    return list.slice(0, 6).map(h => ({ icon: "fa-lightbulb", label: `Vincular a: ${h.text.slice(0, 48)}`, onClick: () => linkItem(c, h, item) }));
  }
  /** What the reports state about the investigation; `ref(item)` names an item. */
  function synthesis(c, ref = item => item.label || "Item") {
    const items = c?.items || [], bindings = nativeItems()?.index(c);
    const data = c?.intel || {};
    const techniques = new Map();
    for (const item of items) for (const d of item.detection?.detections || []) for (const a of d.attack || []) {
      const t = techniques.get(a.id) || techniques.set(a.id, { id: a.id, name: a.name, tactics: a.tactics || [], refs: [] }).get(a.id);
      const r = ref(item);
      if (r && !t.refs.includes(r)) t.refs.push(r);
    }
    return {
      hypotheses: (data.hypotheses || []).map(h => ({ status: h.status, text: h.text, refs: linkedItems(c, h, bindings).map(entry => entry.state === "unique" ? ref(entry.item) : associationLabel(entry)) })),
      techniques: [...techniques.values()].sort((a, b) => a.id.localeCompare(b.id)),
      indicators: (data.indicators || []).map(i => ({ value: i.value, kind: i.kind, status: i.status, note: i.note || "", sightings: i.sightings || null })),
      custody: (c?.artifacts || []).filter(a => a.hashes?.files?.length).map(a => ({ label: a.label || a.path || "Artefato", at: a.hashes.at, files: a.hashes.files })),
    };
  }
  const sightingText = s => !s ? "não verificado" : s.count ? `${s.count} ocorrências nas fontes` : "sem ocorrências nas fontes verificadas";

  return { addIndicator, mount, menuItems, intel, synthesis, sightingText };
})();
