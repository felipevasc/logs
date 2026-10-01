/* Native evidence mutations transport handles only. Failed acknowledgements keep
   the exact pending draft and its private save receipt for retry/reconciliation. */
window.CaseEvidenceActions = (() => {
  "use strict";
  const locks = new WeakSet(), receipts = new WeakMap();
  const clone = value => structuredClone(value);
  const key = ref => JSON.stringify([ref?.kind, ref?.owner, ref?.containerId, ref?.manifestId, ref?.manifestSha256, ref?.memberCount, ref?.token ?? null]);
  const stale = () => Error("EVIDENCE_VIEW_CHANGED: O Caso ou a seleção mudou. Reabra a ação.");
  const pending = () => Error("EVIDENCE_SAVE_RETRY_REQUIRED: A confirmação do salvamento está pendente. O rascunho foi mantido; repita o salvamento antes de outra alteração.");
  const container = reference => ({ kind: "native_evidence_container", reference: clone(reference), preservedCount: reference.memberCount, preview: null });
  function create({ client, session, getStore, currentCase, save, selectionOwner, selectionCurrent, prepareSelectionOwner = async owner => owner, scope, station }) {
    function authority(c = currentCase()) {
      const store = getStore(), entry = store?.caseEvidence?.find(value => value.state === "ready" && value.owner.caseId === c?.id);
      if (c?.kind === "preserved_case_unavailable" || !entry || !store.cases.includes(c)) throw stale();
      const owner = clone(entry.owner), identity = JSON.stringify([store.store.storeId, store.store.epoch, owner]);
      return { c, store, owner, current: () => getStore() === store && currentCase() === c && store.cases.includes(c)
        && JSON.stringify([store.store.storeId, store.store.epoch, store.caseEvidence?.find(value => value.state === "ready" && value.owner.caseId === c.id)?.owner]) === identity };
    }
    function ready() { if (session.status() !== "ready") throw pending(); }
    async function source({ guard = () => true } = {}) {
      const initialCase = currentCase(), initialStore = getStore(), selectedScope = scope(), stationId = station(); let owner = selectionOwner();
      if (!owner?.identity) owner = await prepareSelectionOwner(owner);
      ready(); if (currentCase() !== initialCase || getStore() !== initialStore || scope() !== selectedScope || station() !== stationId) throw stale();
      const target = authority();
      const current = () => target.current() && guard() && selectionCurrent(owner) && scope() === selectedScope && station() === stationId;
      if (!owner?.identity || !current()) throw stale();
      let request = { analysisContext: clone(owner.identity), sourceGeneration: owner.sourceGeneration, caseKey: null, caseContentToken: null };
      if (selectedScope === "case") {
        const opened = await client.openCase({ caseId: target.c.id, analysisContext: owner.identity, stationId, isCurrent: current });
        if (opened.state !== "ready") throw Error(`${opened.code}: O recorte preservado está indisponível para esta ação.`);
        request = { analysisContext: clone(owner.identity), sourceGeneration: null, caseKey: opened.publication.caseKey, caseContentToken: opened.publication.caseContentToken };
      }
      const receipt = await client.captureSource(request, { isCurrent: current });
      if (!current()) throw stale(); return { ...target, receipt, current, stationId };
    }
    function handles(rows) {
      if (!Array.isArray(rows) || !rows.length || rows.length > 10000) throw Error("Selecione de 1 a 10.000 registros.");
      return rows.map(row => { const value = { id: row.id, eventRef: row.eventRef ?? row.event_ref }; window.CaseEvidence.validate.rowHandle(value); return value; });
    }
    async function selection(rows, options) { const selected = handles(rows); return { ...(await source(options)), rows: selected }; }
    async function membership(rows, options) {
      const selected = await selection(rows, options);
      return client.findMembers({ owner: selected.owner, source: selected.receipt, rows: selected.rows, stationId: selected.stationId }, { isCurrent: selected.current });
    }
    async function locked(target, action) {
      ready(); if (!target.current()) throw stale(); if (locks.has(target.c)) throw Error("Aguarde a confirmação da alteração anterior deste Caso.");
      locks.add(target.c); try { return await action(); } finally { locks.delete(target.c); }
    }
    async function commit() { if (await save() === false) throw pending(); }
    async function add(selected, metadata, { guard = () => true, attach = () => {} } = {}) {
      return locked(selected, async () => {
        const current = () => selected.current() && guard(), ticket = client.prepareCapture({ target: selected.owner, source: selected.receipt, rows: selected.rows });
        let saving = false, item = null, rollback = null;
        try {
          const reference = await client.prepare(ticket, { isCurrent: current }); if (!current()) throw stale();
          item = { ...metadata, rows: container(reference), includedCount: reference.memberCount };
          window.CaseEvidence.validate.container(item.rows); selected.c.items ||= []; selected.c.items.push(item);
          rollback = attach(item); saving = true; await commit(); return item;
        } catch (error) { if (!saving) { if (item) selected.c.items = selected.c.items.filter(value => value !== item); rollback?.(); await client.discard(ticket); } throw error; }
      });
    }
    async function changeMembers(c, changes, { guard = () => true } = {}) {
      const target = authority(c);
      return locked(target, async () => {
        if (!Array.isArray(changes) || !changes.length || changes.length > window.CaseEvidence.limits.pending) throw Error("Altere até oito contêineres por operação.");
        const seen = new Set(), captured = changes.map(change => {
          const field = change.field || "rows", value = change.item?.[field];
          if (!(c.items || []).includes(change.item) || value?.kind !== "native_evidence_container" || seen.has(value.reference.containerId)) throw stale();
          seen.add(value.reference.containerId); return { ...change, field, before: value, beforeKey: key(value.reference), ticket: client.prepareMembership({ reference: value.reference, action: change.action }) };
        });
        const current = () => target.current() && guard() && captured.every(change => c.items.includes(change.item) && key(change.item[change.field]?.reference) === change.beforeKey);
        let installed = false;
        try {
          for (const change of captured) change.result = await client.prepare(change.ticket, { isCurrent: current });
          if (!current()) throw stale();
          for (const change of captured) change.item[change.field] = container(change.result.reference);
          installed = true; await commit();
          const receipt = Object.freeze({ kind: "native_membership_edit", count: captured.reduce((sum, change) => sum + change.result.changedCount, 0) });
          receipts.set(receipt, { target, changes: captured.map(change => ({ item: change.item, field: change.field, before: clone(change.result.undo), afterKey: key(change.item[change.field].reference) })) }); return receipt;
        } catch (error) { if (!installed) for (const change of captured) await client.discard(change.ticket); throw error; }
      });
    }
    function remove(c, targets, options) {
      const groups = new Map();
      for (const target of targets) {
        if (!target?.item || !target.member) throw stale();
        const field = target.field || "rows", reference = target.item[field]?.reference;
        window.CaseEvidence.validate.member(target.member, reference);
        let group = groups.get(reference.containerId); if (!group) { group = { item: target.item, field, action: { kind: "remove", members: [] } }; groups.set(reference.containerId, group); }
        if (!group.action.members.some(member => member.occurrenceId === target.member.occurrenceId)) group.action.members.push(clone(target.member));
      }
      return changeMembers(c, [...groups.values()], options);
    }
    function canUndo(receipt) {
      const record = receipts.get(receipt); return !!record && record.target.current() && session.status() === "ready" && !locks.has(record.target.c)
        && (record.detached ? !record.target.c.items.includes(record.detached.item) && !record.target.c.items.some(item => [item.rows, item.events].some(value => value?.reference && record.detached.containers.has(value.reference.containerId)))
          : record.changes.every(change => record.target.c.items.includes(change.item) && key(change.item[change.field]?.reference) === change.afterKey));
    }
    async function undo(receipt) {
      const record = receipts.get(receipt); if (!canUndo(receipt)) throw stale();
      if (record.detached) return locked(record.target, async () => {
        if (!canUndoInside(record)) throw stale(); const { item, index } = record.detached;
        record.target.c.items.splice(Math.min(index, record.target.c.items.length), 0, item); await commit(); receipts.delete(receipt); return true;
      });
      await changeMembers(record.target.c, record.changes.map(change => ({ item: change.item, field: change.field, action: { kind: "restore", target: change.before } })));
      receipts.delete(receipt); return true;
    }
    function canUndoInside(record) { return record.target.current() && !record.target.c.items.includes(record.detached.item) && !record.target.c.items.some(item => [item.rows, item.events].some(value => value?.reference && record.detached.containers.has(value.reference.containerId))); }
    async function detach(c, item) {
      const target = authority(c);
      return locked(target, async () => {
        const index = c.items.indexOf(item); if (index < 0) throw stale();
        const containers = new Set([item.rows, item.events].filter(value => value?.kind === "native_evidence_container").map(value => value.reference.containerId));
        c.items.splice(index, 1); await commit(); const receipt = Object.freeze({ kind: "native_item_removal" });
        receipts.set(receipt, { target, detached: { item, index, containers } }); return receipt;
      });
    }
    return { source, selection, handles, membership, add, remove, detach, canUndo, undo };
  }
  return { create };
})();
