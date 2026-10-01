/* Authored item/trail identities. References describe membership, never record values. */
window.CaseEvidenceItems = (() => {
  "use strict";
  const own = (value, key) => Object.hasOwn(value, key), locks = new WeakSet();
  const container = value => value?.kind === "native_evidence_container" ? value : null;
  const itemContainers = item => [container(item?.rows), container(item?.events)].filter(Boolean);
  const key = value => JSON.stringify([value?.kind ?? null, value?.containerId ?? null, value?.itemId ?? null, value?.state ?? null]);
  const referenceKey = value => JSON.stringify([value?.kind, value?.owner?.storeId, value?.owner?.caseId, value?.owner?.analysisId,
    value?.containerId, value?.manifestId, value?.manifestSha256, value?.memberCount, value?.token ?? null]);
  function valid(value) {
    return !!value && typeof value === "object" && !Array.isArray(value)
      && (value.kind === "container" ? typeof value.containerId === "string" && !!value.containerId && Object.keys(value).every(name => ["kind", "containerId"].includes(name))
        : value.kind === "item" && typeof value.itemId === "string" && !!value.itemId && Object.keys(value).every(name => ["kind", "itemId"].includes(name)));
  }
  function unresolved(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return false;
    if (value.kind === "unresolved_native_container") return typeof value.containerId === "string" && !!value.containerId && value.state === "missing"
      && Object.keys(value).every(name => ["kind", "containerId", "state"].includes(name));
    return value.kind === "unresolved_legacy_item" && (typeof value.itemId === "string" && ["missing", "ambiguous"].includes(value.state)
      || value.itemId === null && value.state === "unsupported") && Object.keys(value).every(name => ["kind", "itemId", "state"].includes(name));
  }
  function count(item, field = "rows") {
    const value = item?.[field];
    if (container(value)) { window.CaseEvidence.validate.container(value); return value.preservedCount; }
    if (Array.isArray(value) && window.CaseEvidence?.active !== true) return value.length;
    if (value != null && window.CaseEvidence?.active === true) throw Error("EVIDENCE_NATIVE_CONTAINER_REQUIRED: A lista preservada precisa de uma referência nativa; ela não foi tratada como vazia.");
    return 0;
  }
  function index(c, { legacyItemAliases = typeof state === "object" ? state.cases?.caseEvidence?.find(entry => entry.state === "ready" && entry.owner.caseId === c?.id)?.legacyItemAliases || [] : [] } = {}) {
    const blocked = new Map(legacyItemAliases.map(entry => [entry.alias, entry.state]));
    const containers = new Map(), ids = new Map(), members = new Set(c?.items || []);
    for (const [position, item] of (c?.items || []).entries()) {
      const entry = { item, index: position };
      if (!ids.has(item.id)) ids.set(item.id, []); ids.get(item.id).push(entry);
      for (const value of itemContainers(item)) {
        const id = value.reference.containerId; if (!containers.has(id)) containers.set(id, []); containers.get(id).push(entry);
      }
    }
    function resolve(reference, { legacy = false } = {}) {
      if (unresolved(reference)) return { state: reference.state, reference, item: null, index: null };
      if (legacy && blocked.has(reference?.itemId)) return { state: blocked.get(reference.itemId), reference: { kind: "unresolved_legacy_item", itemId: reference.itemId, state: blocked.get(reference.itemId) }, item: null, index: null };
      if (!valid(reference)) return { state: "unavailable", reference, item: null, index: null };
      const matches = reference.kind === "container" ? containers.get(reference.containerId) || []
        : (ids.get(reference.itemId) || []).filter(({ item }) => legacy || !itemContainers(item).length);
      return matches.length === 1 ? { state: "unique", reference, ...matches[0] }
        : { state: matches.length ? "ambiguous" : "missing", reference, item: null, index: null };
    }
    function forItem(item) {
      if (!members.has(item)) throw Error("O item não pertence mais a este Caso.");
      const value = itemContainers(item)[0], reference = value ? { kind: "container", containerId: value.reference.containerId } : { kind: "item", itemId: item.id };
      if (resolve(reference).item !== item) throw Error("A identidade deste item é ambígua. Escolha um item com referência própria.");
      return reference;
    }
    function associations(trail, { legacyField = "itemIds" } = {}) {
      if (own(trail, "itemRefs")) {
        if (!Array.isArray(trail.itemRefs)) return [{ state: "unavailable", reference: null, item: null, index: null }];
        return trail.itemRefs.map(reference => resolve(reference));
      }
      return (Array.isArray(trail[legacyField]) ? trail[legacyField] : []).map(itemId => {
        if (typeof itemId !== "string") return resolve({ kind: "unresolved_legacy_item", itemId: null, state: "unsupported" });
        const result = resolve({ kind: "item", itemId }, { legacy: true });
        return result.state === "unique" ? { ...result, reference: forItem(result.item) }
          : { ...result, reference: { kind: "unresolved_legacy_item", itemId, state: result.state === "ambiguous" ? "ambiguous" : "missing" } };
      });
    }
    return { resolve, forItem, associations };
  }
  const resolve = (c, reference, options) => index(c).resolve(reference, options);
  const forItem = (c, item) => index(c).forItem(item);
  const associations = (c, trail, options) => index(c).associations(trail, options);
  const snapshot = trail => ({ hasRefs: own(trail, "itemRefs"), refs: trail.itemRefs, hasIds: own(trail, "itemIds"), ids: trail.itemIds, updatedAt: trail.updatedAt });
  function install(trail, value) {
    if (value.hasRefs) trail.itemRefs = value.refs; else delete trail.itemRefs;
    if (value.hasIds) trail.itemIds = value.ids; else delete trail.itemIds;
  }
  async function change(c, trail, next, { save, owns = () => true, coordinator = window.CaseEvidence?.active === true && typeof nativeEvidenceServices === "function" ? nativeEvidenceServices().session : null } = {}) {
    if (coordinator && coordinator.status() !== "ready") throw Error("Confirme o salvamento pendente antes de alterar outra associação.");
    if (!owns() || !([...c.caseTrails || [], ...c.intel?.hypotheses || []]).includes(trail)) throw Error("A trilha mudou. Reabra sua edição.");
    if (locks.has(trail)) throw Error("Aguarde a confirmação da alteração anterior desta trilha.");
    const before = snapshot(trail), captured = { ...next, updatedAt: Date.now() };
    locks.add(trail); install(trail, captured); trail.updatedAt = captured.updatedAt;
    try {
      if (await save() === false) throw Error("Não foi possível confirmar o salvamento da associação.");
      return { before, after: captured, case: c, trail };
    } catch (error) {
      // A sent native ticket can have committed despite a missing acknowledgement.
      // Keep the exact intended draft so its immutable retry and live view agree.
      if (coordinator?.hasUnconfirmedSave?.()) throw Error(`A confirmação da associação está pendente; o rascunho foi mantido. ${String(error.message || error)}`);
      if (trail.itemRefs === captured.refs && own(trail, "itemRefs") === captured.hasRefs && trail.itemIds === captured.ids && own(trail, "itemIds") === captured.hasIds) {
        install(trail, before); if (trail.updatedAt === captured.updatedAt) trail.updatedAt = before.updatedAt;
        throw Error(`${String(error.message || error)} A ordem anterior foi mantida.`);
      }
      throw error;
    } finally { locks.delete(trail); }
  }
  function edit(c, trail, references, options) {
    if (!Array.isArray(references)) throw Error("Associações inválidas.");
    const lookup = index(c), prior = new Map();
    // Existing unresolved and repeated slots are authored positions. They may be
    // reordered or removed without manufacturing a new association.
    for (const entry of lookup.associations(trail, options)) prior.set(key(entry.reference), (prior.get(key(entry.reference)) || 0) + 1);
    const seen = new Map();
    for (const reference of references) {
      const identity = key(reference), count = (seen.get(identity) || 0) + 1;
      if (lookup.resolve(reference).state !== "unique" && !((unresolved(reference) || valid(reference)) && count <= (prior.get(identity) || 0))
        || count > Math.max(1, prior.get(identity) || 0)) throw Error("Resolva os itens indisponíveis ou ambíguos antes de aplicar as associações.");
      seen.set(identity, count);
    }
    // Preserve the original legacy array verbatim; itemRefs is authoritative.
    return change(c, trail, { ...snapshot(trail), hasRefs: true, refs: structuredClone(references) }, options);
  }

  function undo(receipt, options) {
    const { case: c, trail, before, after } = receipt;
    if (trail.itemRefs !== after.refs || trail.itemIds !== after.ids || own(trail, "itemRefs") !== after.hasRefs || own(trail, "itemIds") !== after.hasIds)
      throw Error("A associação mudou depois desta alteração. A ordem atual foi mantida.");
    return change(c, trail, before, options);
  }
  function open(item, { field = "rows", guard = () => true } = {}) {
    const c = activeCase(), value = item?.[field];
    if (!(c?.items || []).includes(item)) return Promise.resolve(false);
    if (container(value)) {
      const captured = referenceKey(value.reference);
      return window.CaseEvidenceDetail.open(value.reference, null, { guard: () => guard() && activeCase() === c && c.items.includes(item) && referenceKey(item[field]?.reference) === captured });
    }
    if (window.CaseEvidence?.active === true || !Array.isArray(value) || !value.length) return Promise.resolve(false);
    showDetail(value[0], item.sourceSpec); return Promise.resolve(true);
  }
  function attachRecords(parent, c, item, { field = "rows" } = {}) {
    if (!container(item?.[field])) return;
    const details = el("details", "case-item-detail"), summary = el("summary", "", `Ocorrências preservadas (${count(item, field)})`), host = el("div", "case-item-events"); details.append(summary, host); parent.append(details);
    const reference = structuredClone(item[field].reference), expected = referenceKey(reference), cursors = [null], offsets = [0]; let page = 0, generation = 0, started = false;
    const guard = () => host.isConnected && activeCase() === c && c.items.includes(item) && referenceKey(item[field]?.reference) === expected;
    const cellText = cell => cell.state === "missing" ? "Ausente" : cell.state === "null" ? "null" : cell.state === "unavailable" ? "Indisponível" : cell.state === "preview" ? `${cell.text}${cell.incomplete ? "… [prévia]" : ""}` : typeof cell.value === "string" ? cell.value : JSON.stringify(cell.value);
    async function paint(index) {
      const request = ++generation; host.textContent = "Lendo ocorrências preservadas…";
      try {
        const columns = (typeof state === "object" && state.visibleCols || ["timestamp", "source", "message"]).slice(0, 8), result = await nativeEvidenceServices().client.preview(reference, { cursor: cursors[index], columns, limit: 16, isCurrent: guard });
        if (!guard() || request !== generation) return;
        const end = offsets[index] + result.rows.length;
        if (end > result.total || result.nextCursor && (!result.rows.length || cursors.slice(0, index + 1).includes(result.nextCursor)) || !result.nextCursor && end !== result.total) throw Error("A página de ocorrências está incompleta. Reabra o item.");
        page = index; cursors[index + 1] = result.nextCursor; offsets[index + 1] = end; host.replaceChildren();
        const table = el("table"), head = el("thead"), titles = el("tr"), body = el("tbody"); titles.append(el("th", "", "Ocorrência")); for (const column of result.columns) titles.append(el("th", "", column)); head.append(titles);
        for (const [position, row] of result.rows.entries()) { const tr = el("tr"), action = el("td"), open = el("button", "btn ghost small", String(offsets[index] + position + 1)); open.type = "button"; open.setAttribute("aria-label", `Abrir ocorrência preservada ${offsets[index] + position + 1}`);
          open.onclick = () => guard() && window.CaseEvidenceDetail.open(reference, row.member, { guard }); action.append(open); tr.append(action);
          for (const cell of row.cells) { const value = cellText(cell); tr.append(el("td", "", window.EvidenceUI?.redact(value) ?? value)); } body.append(tr); }
        table.append(head, body); const navigation = el("div", "case-trails-pager"), previous = el("button", "btn ghost small", "Anteriores"), next = el("button", "btn ghost small", "Próximas"); previous.type = next.type = "button"; previous.disabled = index === 0; next.disabled = !result.nextCursor; previous.onclick = () => paint(page - 1); next.onclick = () => paint(page + 1);
        navigation.append(previous, el("span", "", `${offsets[index] + (result.rows.length ? 1 : 0)}–${end} de ${result.total}`), next); host.append(table, navigation);
      } catch (error) { if (guard() && request === generation) { host.textContent = String(error.message || error); const retry = el("button", "btn ghost small", "Tentar novamente"); retry.type = "button"; retry.onclick = () => paint(index); host.append(retry); } }
    }
    details.addEventListener("toggle", () => { if (details.open && !started) { started = true; void paint(0); } });
    return details;
  }
  return { count, index, resolve, forItem, associations, key, edit, undo, open, attachRecords };
})();
