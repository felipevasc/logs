/* Explicitly opted-in browser fixture. Native tests own disk and transaction semantics. */
window.createMockExclusions = ({ contextFor, identity, generation, rows, filter, persist, sourceAvailable }) => {
  const enabled = () => window.__mockExclusionsEnabled === true;
  const copy = value => structuredClone(value), signature = value => JSON.stringify(identity(value));
  const namespace = value => JSON.stringify([value.caseId, value.analysisId]);
  const reference = event => event.event_ref || `preview:${event.id}`;
  const key = member => JSON.stringify(member.key);
  const archives = new Map(), previews = new Map(); let sequence = 0;
  const requireEnabled = () => { if (!enabled()) throw Error('Arquivo de exclusões indisponível no preview padrão.'); };
  const batches = analysis => archives.get(namespace(analysis)) || [];
  const getBatch = (analysis, id) => {
    const value = batches(analysis).find(batch => batch.id === id);
    if (!value) throw Error('Lote não encontrado neste Caso.'); return value;
  };
  const summary = ({ records, restored, ...batch }) => copy(batch);
  const hidden = analysis => new Set(batches(analysis).filter(batch => batch.active)
    .flatMap(batch => batch.records.filter(member => !batch.restored.has(key(member))).map(member => member.eventRef)));
  const visible = (analysis, source) => enabled() && analysis ? source.filter(event => !hidden(analysis).has(reference(event))) : source;
  const receipt = (analysis, batchId, selectedMembers) => {
    const context = contextFor({ analysisContext: analysis }); context.visibilityRevision++; persist(context);
    return { analysisContext: copy(context), batchId, selectedMembers, newlyVisible: null };
  };
  const source = { version: 'synthetic-v1', recordSpace: 'preview-records-v1', label: 'Fonte sintética do preview', eventRefPrefix: 'preview:' };
  const member = event => ({ key: { sourceKey: 'preview-source-v1', locator: { stable_record: reference(event) } }, eventRef: reference(event) });
  const handlers = {
    exclusion_visibility: args => {
      contextFor(args); const caseScope = Array.isArray(args.caseEvents), available = caseScope || sourceAvailable();
      const pool = caseScope ? args.caseEvents : rows();
      return { analysis: copy(args.analysisContext), sourceGeneration: caseScope ? null : generation(), scope: caseScope ? 'case' : 'dataset',
        sourceAvailable: available, totalRows: available ? pool.length : null,
        excludedRows: available ? pool.length - visible(args.analysisContext, pool).length : null,
        unavailableMembers: caseScope || !available ? null : enabled() ? window.__mockUnavailableMembers ?? 0 : 0 };
    },
    exclusion_capabilities: args => { contextFor(args); return { available: enabled(), reason: enabled() ? null : 'O arquivo de exclusões ainda não está disponível nesta versão.' }; },
    exclusion_preview: args => {
      requireEnabled(); contextFor(args);
      const caseScope = Array.isArray(args.caseEvents), admittedGeneration = caseScope ? null : generation();
      if (!caseScope && args.sourceGeneration !== admittedGeneration) throw Error('A fonte da prévia mudou.');
      const pool = visible(args.analysisContext, caseScope ? args.caseEvents : rows());
      let selected;
      if (args.scope?.kind === 'selected') {
        const ids = new Set(args.scope.ids), candidates = pool.filter(event => ids.has(event.id));
        if (candidates.length !== ids.size) throw Error('A seleção contém registros indisponíveis.'); selected = candidates;
      } else if (args.scope?.kind === 'filtered') selected = filter(args.scope.filters, pool);
      else throw Error('Seleção inválida.');
      for (const [token, preview] of previews) if (preview.expires < Date.now()) previews.delete(token);
      if ([...previews.values()].filter(value => !value.receipt).length >= 2) throw Error('Há duas seleções preparadas.');
      const previewToken = `preview-exclusion-${++sequence}`, records = [...new Map(selected.map(event => { const value = member(event); return [key(value), value]; })).values()];
      previews.set(previewToken, { analysis: copy(args.analysisContext), sourceGeneration: admittedGeneration, records,
        scope: copy(args.scope), expires: Date.now() + 600000 });
      return { previewToken, selectedMembers: records.length, analysisContext: copy(args.analysisContext), sourceGeneration: admittedGeneration };
    },
    exclusion_discard: args => {
      const preview = previews.get(args.previewToken); if (!preview || preview.receipt) return false;
      if (signature(preview.analysis) !== signature(args.analysisContext) || preview.sourceGeneration !== args.sourceGeneration) throw Error('O Caso ou a fonte da prévia mudou.');
      previews.delete(args.previewToken); return true;
    },
    exclusion_commit: args => {
      requireEnabled();
      const preview = previews.get(args.previewToken);
      if (!preview || preview.expires < Date.now()) throw Error('A prévia expirou. Prepare a seleção novamente.');
      if (signature(preview.analysis) !== signature(args.analysisContext) || preview.sourceGeneration !== args.sourceGeneration) throw Error('O Caso ou a fonte da prévia mudou.');
      if (preview.receipt) return copy(preview.receipt);
      contextFor(args);
      if (preview.sourceGeneration !== null && preview.sourceGeneration !== generation()) throw Error('A fonte da prévia mudou.');
      if (!preview.records.length) throw Error('A prévia não contém registros.');
      const batch = { id: `batch-${String(++sequence).padStart(6, '0')}`, createdAtMs: Date.now(), label: args.label || '', reason: args.reason || '',
        scope: preview.scope, sourceReceipt: { sourceGeneration: preview.sourceGeneration }, members: preview.records.length,
        active: true, restoredAtMs: null, records: preview.records, restored: new Set() };
      const list = batches(args.analysisContext); list.unshift(batch); archives.set(namespace(args.analysisContext), list);
      preview.receipt = receipt(args.analysisContext, batch.id, batch.members); return copy(preview.receipt);
    },
    exclusion_list: args => {
      requireEnabled(); contextFor(args);
      const all = batches(args.analysisContext), start = args.cursor ? all.findIndex(batch => batch.id === args.cursor) + 1 : 0;
      const selected = all.slice(start, start + Math.min(99, args.limit || 50));
      return { analysis: copy(args.analysisContext), batches: selected.map(summary), nextCursor: start + selected.length < all.length ? selected.at(-1).id : null };
    },
    exclusion_archive_page: args => {
      requireEnabled(); contextFor(args); const batch = getBatch(args.analysisContext, args.batchId);
      const start = args.cursor ? batch.records.findIndex(record => key(record) === JSON.stringify(args.cursor)) + 1 : 0;
      const selected = batch.records.slice(start, start + Math.min(500, args.limit || 50));
      const available = new Map([...rows(), ...args.caseEvents || []].map(event => [reference(event), event]));
      return { analysis: copy(args.analysisContext), batch: summary(batch), sources: { 'preview-source-v1': copy(source) },
        activeMembers: !batch.active ? 0 : batch.restored.size ? null : batch.members,
        rows: selected.map(member => ({ member: copy(member), activeInBatch: batch.active && !batch.restored.has(key(member)),
          ...(available.has(member.eventRef) ? { event: copy(available.get(member.eventRef)) } : { unavailableReason: 'Conteúdo indisponível: a fonte não está aberta. A procedência foi preservada.' }) })),
        nextCursor: start + selected.length < batch.records.length ? copy(selected.at(-1).key) : null };
    },
    exclusion_restore_selected: args => {
      requireEnabled(); contextFor(args); const batch = getBatch(args.analysisContext, args.batchId);
      if (!args.members?.length || args.members.length > 500 || !batch.active) throw Error('Selecione registros ativos da página atual.');
      for (const member of args.members) if (!batch.records.some(value => JSON.stringify(value) === JSON.stringify(member)) || batch.restored.has(key(member))) throw Error('O registro não está mais ativo neste lote.');
      for (const member of args.members) batch.restored.add(key(member));
      return receipt(args.analysisContext, batch.id, args.members.length);
    },
    exclusion_restore_batch: args => {
      requireEnabled(); contextFor(args); const batch = getBatch(args.analysisContext, args.batchId);
      batch.active = false; batch.restoredAtMs = Date.now(); return receipt(args.analysisContext, batch.id, batch.members);
    },
  };
  return { handlers, visible, expirePreviews: () => { for (const preview of previews.values()) if (!preview.receipt) preview.expires = 0; } };
};
