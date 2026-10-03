/* Explicit historical reads. Native text/summary DTOs never become saved Events. */
window.CaseEvidencePreserved = (() => {
  "use strict";
  const tickets = new WeakMap(), histories = new WeakMap();
  const types = new Set(["null", "boolean", "number", "string", "array", "object"]);
  const fail = () => { throw Error("EVIDENCE_HISTORY_INVALID: A resposta das evidências preservadas não corresponde ao recorte solicitado."); };
  const stale = () => Error("EVIDENCE_VIEW_CHANGED: A evidência ou o Caso mudou. Reabra o registro preservado.");
  const integer = (value, maximum = 4294967295) => Number.isSafeInteger(value) && value >= 0 && value <= maximum;
  const millis = value => Number.isSafeInteger(value) && Math.abs(value) <= 8640000000000000;
  const text = (value, maximum) => typeof value === "string" && window.CaseEvidence.validate.utf8Within(value, maximum);
  const memberKey = value => JSON.stringify([value?.containerId, value?.manifestId, value?.occurrenceId]);
  const referenceKey = value => JSON.stringify([value.kind, value.owner.storeId, value.owner.caseId, value.owner.analysisId, value.containerId, value.manifestId, value.manifestSha256, value.memberCount, value.token ?? null]);
  const ownerKey = value => JSON.stringify([value?.storeId, value?.caseId, value?.analysisId]);
  const storeKey = value => JSON.stringify([value?.storeId, value?.epoch]);
  // Count JSON wire bytes without allocating a second potentially large string.
  function wireWithin(value, maximum) {
    let bytes = 0;
    const string = value => {
      bytes += 2;
      for (let index = 0; index < value.length && bytes <= maximum; index++) {
        const code = value.charCodeAt(index);
        if (code === 34 || code === 92 || [8, 9, 10, 12, 13].includes(code)) bytes += 2;
        else if (code < 32) bytes += 6;
        else if (code < 128) bytes++;
        else if (code < 2048) bytes += 2;
        else if (code >= 0xd800 && code <= 0xdbff && value.charCodeAt(index + 1) >= 0xdc00 && value.charCodeAt(index + 1) <= 0xdfff) { bytes += 4; index++; }
        else bytes += code >= 0xd800 && code <= 0xdfff ? 6 : 3;
      }
    };
    const walk = (value, depth = 0) => {
      if (bytes > maximum) return;
      if (depth > 64) { bytes = maximum + 1; return; }
      if (typeof value === "string") string(value);
      else if (value === null || typeof value !== "object") bytes += String(value).length;
      else {
        bytes += 2;
        if (Array.isArray(value)) {
          for (let index = 0; index < value.length && bytes <= maximum; index++) { if (index) bytes++; walk(value[index], depth + 1); }
        } else {
          let count = 0;
          for (const key in value) {
            if (bytes > maximum) break;
            if (!Object.hasOwn(value, key) || value[key] === undefined) continue;
            if (count++) bytes++; string(key); bytes++; walk(value[key], depth + 1);
          }
        }
      }
    };
    walk(value); return bytes <= maximum;
  }
  function preview(value, maximum) { if (!value || typeof value.complete !== "boolean" || !text(value.text, maximum)) fail(); }
  function references(item, analytical = false) {
    const values = analytical ? [] : [item?.rows, item?.events];
    for (const entry of item?.items || []) values.push(entry.rows, ...(analytical ? [] : [entry.events]));
    return values.filter(value => value?.kind === "native_evidence_container").map(value => value.reference);
  }
  function detailResponse(value, member) {
    if (value?.kind !== "preserved_member_detail" || memberKey(value.member) !== memberKey(member) || !Array.isArray(value.fields)
      || value.fields.length > 128 || !integer(value.fieldsTotal) || value.fieldsTotal < value.fields.length
      || value.nextCursor != null && !text(value.nextCursor, 4096) || typeof value.javaTraceAvailable !== "boolean") fail();
    const columns = new Set();
    for (const field of value.fields) {
      if (!text(field.column, 1024) || !field.column || columns.has(field.column) || !types.has(field.type) && field.type !== "missing") fail();
      if (field.type === "missing" && (field.text !== "" || field.complete !== true)) fail();
      preview(field, 4096); columns.add(field.column);
    }
    preview(value.raw, 262144); preview(value.envelope, 262144); if (!wireWithin(value, 1048576)) fail(); return value;
  }
  function fieldResponse(value, member, column) {
    if (value?.kind !== "preserved_field_text" || memberKey(value.member) !== memberKey(member) || value.column !== column
      || typeof value.present !== "boolean" || value.complete !== true) fail();
    if (value.present ? !types.has(value.type) || !text(value.text, 1048576) : value.type !== null || value.text !== null) fail();
    if (!wireWithin(value, 1048576)) fail(); return value;
  }
  function create({ client, invoke, getStore, currentCase }) {
    function ownerCurrent(captured) {
      const store = getStore(), item = currentCase();
      return store === captured.store && item === captured.item && storeKey(store?.store) === captured.storeKey
        && item?.id === captured.owner.caseId && (item.analysisContext?.analysisId || store.caseEvidence?.find(entry => entry.owner?.caseId === item.id)?.owner?.analysisId) === captured.owner.analysisId
        && captured.isCurrent();
    }
    function capture({ reference, member, isCurrent = () => true }) {
      window.CaseEvidence.validate.reference(reference); window.CaseEvidence.validate.member(member, reference);
      const store = getStore(), item = currentCase();
      if (store?.evidenceViewVersion !== 1 || item?.kind === "preserved_case_unavailable" || item?.id !== reference.owner.caseId || reference.owner.storeId !== store.store.storeId) throw stale();
      const ticket = Object.freeze({ kind: "preserved_member", member: Object.freeze({ ...member }) });
      const captured = { store, item, owner: structuredClone(reference.owner), storeKey: storeKey(store.store), isCurrent,
        request: { store: { storeId: store.store.storeId, epoch: store.store.epoch }, reference: structuredClone(reference), member: structuredClone(member) } };
      const current = () => ownerCurrent(captured) && references(item).some(ref => referenceKey(ref) === referenceKey(captured.request.reference));
      if (!current()) throw stale(); captured.current = current; tickets.set(ticket, captured); return ticket;
    }
    async function read(ticket, command, extra, validate) {
      const captured = tickets.get(ticket); if (!captured?.current()) throw stale();
      const request = { ...structuredClone(captured.request), ...extra }, result = await invoke(command, { request });
      if (!captured.current()) throw stale(); validate(result, captured.request.member); return structuredClone(result);
    }
    function detail(ticket, { cursor = null } = {}) {
      if (cursor !== null && (!text(cursor, 4096) || !cursor)) fail();
      return read(ticket, "case_evidence_member_detail", { cursor }, detailResponse);
    }
    function fieldText(ticket, column) {
      if (!text(column, 1024) || !column || column === "*") fail();
      return read(ticket, "case_evidence_member_field_text", { column }, (value, member) => fieldResponse(value, member, column));
    }
    function java(ticket) {
      return read(ticket, "case_evidence_member_java_trace", {}, (value, member) => {
        if (memberKey(value?.member) !== memberKey(member) || !["available", "unavailable"].includes(value?.state)) fail();
        if (value.state === "available" ? value.reason !== null || !value.trace || typeof value.trace !== "object"
          : value.trace !== null || !["raw_unavailable", "not_java"].includes(value.reason)) fail();
        if (!wireWithin(value, 1048576)) fail();
      });
    }
    function captureHistory({ stationId = null, filters = [], fromMs = null, toMs = null, includeUntimed = true, aliases = [], authoredViewJson = null, isCurrent = () => true } = {}) {
      const store = getStore(), item = currentCase(), state = store?.caseEvidence?.find(entry => entry.owner?.caseId === item?.id);
      if (store?.evidenceViewVersion !== 1 || item?.kind === "preserved_case_unavailable" || state?.state !== "ready") throw stale();
      if (stationId !== null && (!text(stationId, 4096) || !stationId) || fromMs !== null && !millis(fromMs) || toMs !== null && !millis(toMs)
        || fromMs !== null && toMs !== null && fromMs > toMs || typeof includeUntimed !== "boolean" || !Array.isArray(filters) || filters.length > 200
        || !Array.isArray(aliases) || aliases.length > 256 || new Set(aliases).size !== aliases.length || aliases.some(alias => !alias || !text(alias, 2048))) fail();
      for (const filter of filters) if (!filter || !text(filter.column, 1024) || !filter.column || !text(filter.op, 32) || !filter.op
        || filter.value != null && !text(filter.value, 1000000) || filter.value2 != null && !text(filter.value2, 1000000)) fail();
      const liveAuthored = () => JSON.stringify({ timeline: item.timeline || {}, manual: item.manual || [] });
      const currentAuthorship = authoredViewJson === null;
      authoredViewJson = currentAuthorship ? liveAuthored() : authoredViewJson;
      if (!text(authoredViewJson, 1048576)) fail();
      const shape = window.CaseEvidence.analysisShape(item);
      if (shape !== client.analysisShape(item.id)) throw Error("EVIDENCE_SAVE_PENDING: Salve a composição do Caso antes de abrir sua cronologia preservada.");
      const captured = { store, item, owner: structuredClone(state.owner), storeKey: storeKey(store.store), isCurrent, shape, preservedCount: state.preservedCount, authoredViewSignature: null,
        request: { store: { storeId: store.store.storeId, epoch: store.store.epoch }, owner: structuredClone(state.owner), evidenceSignature: state.evidenceSignature,
          stationId, authoredViewJson, filters: filters.map(filter => ({ column: filter.column, op: filter.op, value: filter.value ?? "", value2: filter.value2 ?? null })), fromMs, toMs, includeUntimed, aliases: aliases.slice() },
        references: new Map(references(item, true).map(ref => [ref.containerId, structuredClone(ref)])),
        locations: new Map((item.items || []).flatMap((entry, index) => entry.rows?.kind === "native_evidence_container" ? [[entry.rows.reference.containerId, { index, id: typeof entry.id === "string" ? entry.id : null }]] : [])) };
      captured.current = () => ownerCurrent(captured) && (!currentAuthorship || liveAuthored() === authoredViewJson) && window.CaseEvidence.analysisShape(item) === captured.shape
        && client.analysisShape(item.id) === captured.shape && getStore().caseEvidence.some(entry => ownerKey(entry.owner) === ownerKey(captured.owner) && entry.evidenceSignature === captured.request.evidenceSignature);
      if (!captured.current()) throw stale(); if (!wireWithin({ ...captured.request, cursor: null }, 2097152)) fail();
      const ticket = Object.freeze({ kind: "preserved_timeline_scope" }); histories.set(ticket, captured); return ticket;
    }
    async function timeline(ticket, { cursor = null } = {}) {
      const captured = histories.get(ticket); if (!captured?.current()) throw stale();
      if (cursor !== null && (!text(cursor, 512) || !cursor)) fail();
      const request = { ...captured.request, cursor }; if (!wireWithin(request, 2097152)) fail();
      const result = await invoke("case_evidence_timeline", { request: structuredClone(request) });
      if (!captured.current()) throw stale();
      timelineResponse(result, captured); captured.authoredViewSignature ||= result.authoredViewSignature; return structuredClone(result);
    }
    return { capture, detail, fieldText, java, captureHistory, timeline,
      current: ticket => (tickets.get(ticket) || histories.get(ticket))?.current() === true };
  }
  function timelineResponse(value, captured) {
    if (value?.kind !== "preserved_timeline" || ownerKey(value.owner) !== ownerKey(captured.owner) || value.evidenceSignature !== captured.request.evidenceSignature
      || !/^[0-9a-f]{64}$/.test(value.authoredViewSignature || "") || captured.authoredViewSignature && value.authoredViewSignature !== captured.authoredViewSignature
      || !Array.isArray(value.entries) || value.entries.length > 128 || !Array.isArray(value.aliases) || value.aliases.length !== captured.request.aliases.length
      || value.nextCursor != null && (!text(value.nextCursor, 512) || !value.nextCursor)) fail();
    for (const name of ["preservedCount", "scopeCount", "timedCount", "untimedCount", "unavailableCount"]) if (!integer(value[name])) fail();
    if (value.preservedCount !== captured.preservedCount || value.scopeCount > value.preservedCount || value.entries.length > value.scopeCount || value.timedCount + value.untimedCount + value.unavailableCount !== value.scopeCount) fail();
    if (value.timedCount ? !millis(value.minTimestampMs) || !millis(value.maxTimestampMs) || value.minTimestampMs > value.maxTimestampMs : value.minTimestampMs !== null || value.maxTimestampMs !== null) fail();
    const seen = new Set(); let previous = null;
    for (const entry of value.entries) {
      const ref = captured.references.get(entry.containerId), location = captured.locations.get(entry.containerId); if (!ref || !location) fail(); window.CaseEvidence.validate.member(entry.member, ref);
      if (entry.entryId !== `n:${entry.containerId}:${entry.member.occurrenceId}` || seen.has(entry.entryId) || !integer(entry.itemIndex) || !integer(entry.originalRowIndex)
        || entry.itemIndex !== location.index || entry.itemId !== location.id || entry.laneKey !== entry.containerId
        || entry.itemId !== null && !text(entry.itemId, 4096) || !["timed", "untimed", "unavailable"].includes(entry.timing) || typeof entry.groupingUsesTitle !== "boolean"
        || !text(entry.laneKey, 4096) || !text(entry.groupingKey, 4096) || entry.eventRef !== null && !text(entry.eventRef, 4096)) fail();
      if (entry.timing === "timed" ? !millis(entry.timestampMs) || entry.timestampMs < value.minTimestampMs || entry.timestampMs > value.maxTimestampMs : entry.timestampMs !== null) fail();
      for (const name of ["title", "detail", "source", "laneLabel"]) preview(entry[name], 4096);
      const order = [entry.timing === "timed" ? 0 : entry.timing === "untimed" ? 1 : 2, entry.timestampMs ?? 0, entry.itemIndex, entry.originalRowIndex];
      if (previous && order.some((part, index) => part < previous[index] && order.slice(0, index).every((prior, i) => prior === previous[i]))) fail();
      previous = order; seen.add(entry.entryId);
    }
    const aliases = new Set();
    for (const alias of value.aliases) {
      if (!captured.request.aliases.includes(alias.alias) || aliases.has(alias.alias) || !["unique", "missing", "ambiguous"].includes(alias.state)
        || (alias.state === "unique" ? !text(alias.entryId, 4096) || !alias.entryId : alias.entryId !== null)) fail();
      aliases.add(alias.alias);
    }
    if (!wireWithin(value, 1048576)) fail(); return value;
  }
  return { create, validate: { detail: detailResponse, field: fieldResponse } };
})();
