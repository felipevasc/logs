/* Exact, reversible removal of preserved occurrences. Event values are never edited. */
window.CaseRemovals = (() => {
  const timelineId = (item, row, index) => `e:${item.id}:${row.event_ref || (row.id ?? index)}`;
  const timelineStamp = c => JSON.stringify({ groups: c.timeline?.groups || [], annotations: c.timeline?.annotations || [] });
  const shape = c => (c.items || []).map(item => ({ item, rows: item.rows, members: Array.isArray(item.rows) ? item.rows.slice() : null }));
  const matches = (c, snapshot) => (c.items || []).length === snapshot.length && snapshot.every((entry, index) =>
    c.items[index] === entry.item && c.items[index].rows === entry.rows && (!entry.members
      || entry.members.length === entry.rows.length && entry.members.every((row, i) => row === entry.rows[i])));
  const equal = (left, right) => JSON.stringify(left) === JSON.stringify(right);
  function restoreFields(current, changed, original) {
    if (equal(current, changed)) return original;
    if (!current || !changed || !original || [current, changed, original].some(value => typeof value !== "object" || Array.isArray(value))) return current;
    const result = { ...current };
    for (const key of new Set([...Object.keys(changed), ...Object.keys(original)])) {
      const hasCurrent = Object.hasOwn(current, key), hasChanged = Object.hasOwn(changed, key), hasOriginal = Object.hasOwn(original, key);
      if (hasCurrent === hasChanged && equal(current[key], changed[key])) {
        if (hasOriginal) result[key] = original[key]; else delete result[key];
      } else if (hasCurrent && hasChanged && hasOriginal) {
        const lists = [current[key], changed[key], original[key]];
        if (key === "links" && lists.every(value => Array.isArray(value) && value.every(link => typeof link?.targetId === "string"))) {
          result[key] = restoreTimeline(current[key], changed[key], original[key], entry => entry.targetId);
        } else if (key === "ids" && lists.every(value => Array.isArray(value) && value.every(id => typeof id === "string"))) {
          result[key] = restoreTimeline(current[key], changed[key], original[key], entry => entry);
        } else result[key] = restoreFields(current[key], changed[key], original[key]);
      }
    }
    return result;
  }
  function restoreTimeline(current, changed, original, identity = entry => entry.id) {
    const changedById = new Map(changed.map(entry => [identity(entry), entry])), originalById = new Map(original.map(entry => [identity(entry), entry]));
    const result = current.filter(entry => !changedById.has(identity(entry)) || originalById.has(identity(entry)) || !equal(entry, changedById.get(identity(entry)))).map(entry => {
      const previous = originalById.get(identity(entry)), next = changedById.get(identity(entry));
      return previous && next ? restoreFields(entry, next, previous) : entry;
    });
    const present = new Set(result.map(entry => identity(entry))), insertions = new Map(); let following = null;
    for (let i = original.length - 1; i >= 0; i--) {
      const entry = original[i];
      if (present.has(identity(entry))) { following = identity(entry); continue; }
      if (changedById.has(identity(entry))) continue;
      if (!insertions.has(following)) insertions.set(following, []);
      insertions.get(following).push(entry);
    }
    return result.flatMap(entry => [...(insertions.get(identity(entry)) || []).reverse(), entry]).concat((insertions.get(null) || []).reverse());
  }

  function resolve(c, event, { key, stationId = null } = {}) {
    const result = [];
    const ref = typeof event?.event_ref === "string" && event.event_ref ? event.event_ref : null;
    let wanted = null;
    if (!ref) {
      if (!Number.isSafeInteger(event?.id) || event.id < 0 || typeof key !== "function") return result;
      const seen = new Set(); let dense = 0;
      for (const item of c.items || []) {
        if (stationId && item.stationId !== stationId) continue;
        for (const row of item.rows || []) {
          if (!Number.isInteger(row.id)) continue;
          const value = key(row, item); if (seen.has(value)) continue; seen.add(value);
          if (dense++ === event.id) wanted = value;
        }
      }
      if (wanted === null) return result;
    }
    (c.items || []).forEach((item, itemIndex) => (item.rows || []).forEach((row, index) => {
      if (ref ? row.event_ref === ref : key(row, item) === wanted) result.push({ item, itemIndex, index, row });
    }));
    return result;
  }
  function prepare(c, targets, { entryIds = [] } = {}) {
    if (!c || !Array.isArray(targets) || !targets.length) throw Error("Nenhuma ocorrência foi selecionada.");
    const before = shape(c), grouped = new Map(), selectedIds = new Set();
    for (const target of targets) {
      if (!Number.isInteger(target.itemIndex) || !Number.isInteger(target.index) || target.index < 0
        || c.items?.[target.itemIndex] !== target.item || target.item.rows?.[target.index] !== target.row) throw Error("O Caso mudou. Selecione a ocorrência novamente.");
      let indices = grouped.get(target.itemIndex); if (!indices) { indices = new Set(); grouped.set(target.itemIndex, indices); }
      indices.add(target.index); selectedIds.add(timelineId(target.item, target.row, target.index));
    }
    const nextItems = [], replacements = new Map(), membersAfter = new Map();
    before.forEach((entry, itemIndex) => {
      const indices = grouped.get(itemIndex);
      if (!indices) { nextItems.push(entry.item); return; }
      const rows = entry.members.filter((_, index) => !indices.has(index)); membersAfter.set(entry.item, rows);
      if (rows.length) { const next = { ...entry.item, rows, includedCount: rows.length }; nextItems.push(next); replacements.set(entry.item, next); }
    });
    const remainingIds = new Set();
    nextItems.forEach(item => (item.rows || []).forEach((row, index) => remainingIds.add(timelineId(item, row, index))));
    const removedIds = new Set([...selectedIds, ...entryIds].filter(id => !remainingIds.has(id)));
    for (const id of removedIds) if (id.startsWith("a:") && remainingIds.has(id.slice(2))) removedIds.delete(id);
    let priorGroups = c.timeline?.groups, priorAnnotations = c.timeline?.annotations, beforeTimeline = timelineStamp(c);
    const retarget = new Map();
    const groups = [];
    for (const group of priorGroups || []) {
      const ids = (group.ids || []).filter(id => !removedIds.has(id));
      if (ids.length === (group.ids || []).length) { groups.push(group); removedIds.delete(group.id); continue; }
      if (ids.length >= 2) groups.push(ids.length === group.ids.length ? group : { ...group, ids });
      else if (ids.length) retarget.set(group.id, ids[0]);
      else removedIds.add(group.id);
    }
    const annotations = [];
    for (const note of priorAnnotations || []) {
      const next = { ...note };
      if (next.links) next.links = next.links.filter(link => !removedIds.has(link.targetId)).map(link => retarget.has(link.targetId) ? { ...link, targetId: retarget.get(link.targetId) } : link);
      if (retarget.has(next.anchor)) next.anchor = retarget.get(next.anchor);
      else if (removedIds.has(next.anchor)) {
        if (!next.links?.length) continue;
        const link = next.links.shift(); next.anchor = link.targetId;
        for (const name of ["arrow", "lineStyle", "color", "curve", "endpoints"]) next[name] = link[name];
      }
      annotations.push(next);
    }
    let after = null, afterTimeline = null, undone = null, applied = false;
    function apply() {
      if (applied || !matches(c, before) || timelineStamp(c) !== beforeTimeline) throw Error("O Caso mudou. Selecione a ocorrência novamente.");
      c.items = nextItems;
      if (c.timeline) { c.timeline.groups = groups; c.timeline.annotations = annotations; }
      after = shape(c); afterTimeline = timelineStamp(c); applied = true;
    }
    const canUndo = () => applied && matches(c, after) && timelineStamp(c) === afterTimeline;
    function restoreMembers() {
      const current = (c.items || []).slice(), restored = new Map();
      // Only the removed occurrences belong to this mutation. A concurrent
      // appended item or edits to another container must remain untouched.
      for (const [index, entry] of before.entries()) {
        if (!grouped.has(index)) continue;
        const replacement = replacements.get(entry.item);
        if (!replacement) continue;
        const liveIndex = current.indexOf(replacement), expected = after.find(item => item.item === replacement);
        if (liveIndex < 0 || !expected || replacement.rows !== expected.rows || replacement.rows.length !== expected.members.length
          || expected.members.some((row, index) => replacement.rows[index] !== row)) throw Error("As evidências alteradas receberam outra edição; atualize a visualização.");
        const value = { ...replacement, rows: entry.members };
        if (Object.hasOwn(entry.item, "includedCount")) value.includedCount = entry.item.includedCount; else delete value.includedCount;
        current[liveIndex] = value; restored.set(entry.item, value);
      }
      let following = null;
      for (let index = before.length - 1; index >= 0; index--) {
        const entry = before[index], live = restored.get(entry.item) || entry.item;
        if (current.includes(live)) { following = live; continue; }
        if (!grouped.has(index) || replacements.has(entry.item)) continue;
        const at = following ? current.indexOf(following) : current.length;
        current.splice(at < 0 ? current.length : at, 0, entry.item); following = entry.item;
      }
      c.items = current;
      for (const [original, value] of restored) replacements.set(original, value);
    }
    function undo() {
      if (!canUndo()) throw Error("O Caso recebeu outras alterações. Não foi possível desfazer esta remoção com segurança.");
      restoreMembers();
      if (c.timeline) { c.timeline.groups = priorGroups; c.timeline.annotations = priorAnnotations; }
      applied = false; undone = shape(c); return true;
    }
    function rollback() {
      if (!applied) throw Error("Esta remoção já foi revertida.");
      restoreMembers();
      if (c.timeline) {
        const prior = JSON.parse(beforeTimeline), next = JSON.parse(afterTimeline);
        c.timeline.groups = restoreTimeline(c.timeline.groups || [], next.groups, prior.groups);
        c.timeline.annotations = restoreTimeline(c.timeline.annotations || [], next.annotations, prior.annotations);
      }
      applied = false; undone = shape(c); return true;
    }
    function redo() {
      if (applied || !undone) throw Error("O Caso recebeu outras alterações; atualize a visualização.");
      const current = (c.items || []).slice(), updated = new Map();
      for (const [original, rows] of membersAfter) {
        const restored = replacements.get(original) || original, index = current.indexOf(restored), expected = undone.find(entry => entry.item === restored);
        if (index < 0 || !expected || restored.rows !== expected.rows || restored.rows.length !== expected.members.length
          || expected.members.some((row, index) => restored.rows[index] !== row)) throw Error("As evidências alteradas receberam outra edição; atualize a visualização.");
        if (rows.length) { const replacement = { ...restored, rows, includedCount: rows.length }; current[index] = replacement; updated.set(original, replacement); }
        else current.splice(index, 1);
      }
      c.items = current;
      for (const [original, value] of updated) replacements.set(original, value);
      if (c.timeline) {
        const prior = JSON.parse(beforeTimeline), next = JSON.parse(afterTimeline);
        priorGroups = c.timeline.groups; priorAnnotations = c.timeline.annotations; beforeTimeline = timelineStamp(c);
        c.timeline.groups = restoreTimeline(c.timeline.groups || [], prior.groups, next.groups);
        c.timeline.annotations = restoreTimeline(c.timeline.annotations || [], prior.annotations, next.annotations);
      }
      applied = true; after = shape(c); afterTimeline = timelineStamp(c); return true;
    }
    return { apply, undo, rollback, redo, canUndo, count: [...grouped.values()].reduce((total, indices) => total + indices.size, 0) };
  }
  return { resolve, prepare };
})();
