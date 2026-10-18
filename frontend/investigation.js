/* Population investigation. The native analysis owns scope and membership;
   every late response is rejected before touching a replacement view. */
window.InvestigationUI = (() => {
  const controllers = new WeakMap();
  const names = { new_relationship: 'Relações novas', contextual_rarity: 'Raridade por entidade', volume_change: 'Mudança de volume', password_spraying: 'Falhas distribuídas', session_context_change: 'Mudança de sessão', beaconing: 'Comunicação periódica', dns_tunneling_context: 'Padrões DNS', process_parent_rarity: 'Relação pai e filho', telemetry_parse_quality: 'Qualidade da fonte' };
  const node = (tag, text, className) => { const item = document.createElement(tag); if (text != null) item.textContent = text; if (className) item.className = className; return item; };
  names.local_learning = 'Comportamento aprendido';
  function learningPanel(analysis, options, current, navigate) {
    const report = analysis.investigation.learning;
    if (!report?.enabled) return null;
    const panel = node('details'); panel.dataset.learning = '';
    if (report.available === false) {
      panel.append(node('summary', 'Aprendizado local indisponível'), node('p', String(report.error || 'Não foi possível aplicar esta referência.'), 'error'), node('p', 'A análise foi concluída sem esta etapa. Confira o perfil local e recalcule.', 'small muted'));
    }
    if (report.available !== false) panel.append(node('summary', 'Aprendizado local sem rótulos'), node('p', `${report.scored || 0} registros comparados · ${report.unusual || 0} desvios · ${report.incidents || 0} incidentes para investigar.`));
    const states = { accepted: 'Atualização validada e incorporada ao perfil local.', withheld: 'Atualização retida: histórico, qualidade ou validação insuficientes.', disabled: 'Atualizações automáticas desativadas.', receipt_budget_reached: 'O perfil atingiu o limite de observações registradas.', concurrent_update_deferred: 'Outro cálculo atualizou o perfil; esta contribuição foi adiada.', update_failed: 'A análise foi concluída; a atualização do perfil falhou.' };
    if (report.available !== false) panel.append(node('p', states[report.update?.status] || 'Referência local disponível para esta análise.'), node('p', `${report.cold_start || 0} registros sem referência suficiente. Modelos aprendem diferenças estatísticas; as hipóteses precisam de confirmação.`, 'small muted'));
    if (report.contexts_limited) panel.append(node('p', 'O limite de contextos foi atingido. A cobertura do aprendizado é parcial.', 'small muted'));
    const details = node('details'); details.append(node('summary', 'Referência, cobertura e validação'), node('pre', JSON.stringify({ revision: report.model_revision, valid: report.valid, total: report.total, validation: report.validation, shared_revision: report.shared_revision }, null, 2))); panel.append(details);
    const actions = node('div', null, 'investigation-navigation');
    const diagnostics = node('button', 'Ver diagnósticos aprendidos', 'btn ghost small'); diagnostics.type = 'button'; diagnostics.disabled = report.available === false; diagnostics.onclick = navigate; actions.append(diagnostics);
    const status = node('pre'); status.setAttribute('aria-live', 'polite');
    const run = async (button, request) => {
      button.disabled = true;
      try {
        const result = await options.request('investigation_learning', { analysisId: analysis.analysis_id, request });
        if (!current()) return null;
        status.textContent = JSON.stringify(result, null, 2);
        if (['import', 'download', 'reset', 'rollback'].includes(request.action)) options.onLearningChanged?.();
        return result;
      } catch (error) { if (current()) status.textContent = String(error); return null; }
      finally { if (button.isConnected) button.disabled = false; }
    };
    for (const [action, label] of [['rollback', 'Voltar à revisão anterior'], ['reset', 'Reiniciar perfil local']]) {
      const button = node('button', label, 'btn ghost small'); button.type = 'button'; button.onclick = () => run(button, { action }); actions.append(button);
    }
    panel.append(actions);
    const share = node('details'); share.append(node('summary', 'Base compartilhada e contribuições'), node('p', 'O perfil com mensagens permanece neste computador. A contribuição contém somente contagens de estados numéricos de um vocabulário fechado. Contagens também descrevem atividade; publique apenas quando esse compartilhamento for adequado. Não há envio automático.'));
    const exportButton = node('button', 'Exportar contribuição agregada', 'btn ghost small'); exportButton.type = 'button'; exportButton.disabled = report.update?.applied !== true;
    exportButton.onclick = async () => {
      const result = await run(exportButton, { action: 'export' }); if (!result || !current()) return;
      const blob = new Blob([JSON.stringify({ schema_version: result.schema_version, contributions: result.contributions }, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob), link = node('a'); link.href = url; link.download = 'contribuicao-aprendizado.json'; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
    }; share.append(exportButton);
    const file = node('input'); file.type = 'file'; file.accept = 'application/json,.json'; file.setAttribute('aria-label', 'Arquivo da base compartilhada');
    const importButton = node('button', 'Importar base compartilhada', 'btn ghost small'); importButton.type = 'button';
    importButton.onclick = async () => {
      const selected = file.files?.[0]; if (!selected) { status.textContent = 'Escolha o arquivo da base compartilhada.'; return; }
      if (selected.size > 16 * 1024 * 1024) { status.textContent = 'A base deve ter até 16 MiB.'; return; }
      try { await run(importButton, { action: 'import', document: JSON.parse(await selected.text()) }); } catch (error) { if (current()) status.textContent = String(error); }
    }; share.append(file, importButton);
    if (report.repository) {
      share.append(node('p', `Repositório: ${report.repository} · branch ${report.branch}. O token é usado somente nesta operação.`));
      const token = node('input'); token.type = 'password'; token.autocomplete = 'off'; token.maxLength = 256; token.setAttribute('aria-label', 'Token temporário do GitHub'); share.append(token);
      for (const [action, label] of [['download', 'Baixar base do Git'], ['publish', 'Publicar contribuição no Git']]) {
        const button = node('button', label, 'btn ghost small'); button.type = 'button'; if (action === 'publish') button.disabled = report.update?.applied !== true;
        button.onclick = async () => { const credential = token.value; token.value = ''; await run(button, { action, ...(credential ? { token: credential } : {}) }); }; share.append(button);
      }
    } else share.append(node('p', 'Para sincronizar pelo Git, configure o repositório nas Regras → Análise comportamental.', 'small muted'));
    panel.append(share, status); return panel;
  }
  function mount(parent, analysis, options) {
    if (!analysis?.investigation?.enabled || !analysis.investigation.complete) return;
    parent.querySelector('[data-investigation]')?.remove();
    const host = node('section', null, 'investigation-card'); host.dataset.investigation = '';
    const controller = { active: true, revision: 0, section: 'queue', offset: 0, namespace: null, entity: null, signalId: null };
    controllers.set(host, controller); parent.append(host);
    const current = () => host.isConnected && controller.active && options.isCurrent();
    const title = node('h2', 'Investigar comportamento');
    const description = node('p', `${analysis.investigation.population.toLocaleString('pt-BR')} registros analisados · ${analysis.investigation.signals} sinais · ${analysis.investigation.edges} relações. Prioridade e força da evidência são independentes.`, 'small muted');
    const navigation = node('div', null, 'investigation-navigation');
    const content = node('div'); content.setAttribute('aria-live', 'polite');
    const pagination = node('div', null, 'investigation-navigation');
    for (const [section, label] of [['queue', 'Fila de investigação'], ['signals', 'Sinais'], ['graph', 'Relações'], ['story', 'Cronologia'], ['narrative', 'Preparar síntese assistida'], ['hunts', 'Caças guiadas'], ['coverage', 'Cobertura'], ['schema', 'Campos e interpretação'], ['revision', 'Mudanças nesta revisão'], ['ioc_matches', 'IOCs confirmados']]) {
      const button = node('button', label, 'btn ghost small'); button.type = 'button';
      button.onclick = () => { controller.section = section; if(section==='schema')controller.entity=null;controller.signalId = null; controller.offset = 0; void load(); };
      navigation.append(button);
    }
    const clear = node('button', 'Todas as entidades', 'btn ghost small'); clear.type = 'button';
    clear.onclick = () => { controller.namespace = null; controller.entity = null; controller.signalId = null; controller.offset = 0; controller.section = 'queue'; void load(); };
    navigation.append(clear);
    host.append(title, description, navigation, content, pagination);
    if (analysis.investigation.learning?.enabled) {
      const navigate = () => { controller.section = 'learning'; controller.namespace = null; controller.entity = null; controller.offset = 0; void load(); };
      if (analysis.investigation.learning.available !== false) { const button = node('button', 'Diagnósticos aprendidos', 'btn ghost small'); button.type = 'button'; button.onclick = navigate; navigation.append(button); }
      const panel = learningPanel(analysis, options, current, navigate); if (panel) host.insertBefore(panel, navigation);
    }
    if (analysis.investigation.historical_profile?.status && analysis.investigation.historical_profile.status !== 'disabled') {
      const training = node('details'); training.append(node('summary', 'Referência histórica do ambiente'), node('p', 'O perfil fica congelado neste Caso. Usar este universo substitui a referência, excluindo membros de E3+ e hipóteses com prioridade a partir de 35. A próxima análise poderá comparar relações, hora da semana em UTC e pares declarados.'), node('pre', JSON.stringify(analysis.investigation.historical_profile, null, 2)));
      const button = node('button', 'Usar este universo como referência', 'btn ghost small'); button.type = 'button';
      button.onclick = async () => { button.disabled = true; try { const result = await options.request('investigation_profile_train', { analysisId: analysis.analysis_id }); options.onProfileSaved?.(result); if (current()) training.append(node('p', `Referência salva: ${result.eligible} eventos elegíveis, ${result.withheld} excluídos. Execute a análise para aplicar.`)); } catch (error) { if (current()) training.append(node('p', String(error), 'error')); } finally { if (current()) button.disabled = false; } }; training.append(button); host.append(training);
    }
    function chooseEntity(item, section) {
      controller.namespace = item.namespace; controller.entity = item.value;
      controller.section = section; controller.signalId = null; controller.offset = 0; void load();
    }
    async function load() {
      const revision = ++controller.revision;
      content.replaceChildren(node('p', 'Carregando investigação…')); pagination.replaceChildren();
      try {
        const page = await options.request('investigation_page', { analysisId: analysis.analysis_id, section: controller.section, namespace: controller.namespace, entity: controller.entity, signalId: controller.signalId, offset: controller.offset, limit: 20 });
        if (!current() || revision !== controller.revision) return;
        if (page.analysis_id !== analysis.analysis_id || page.complete !== true) throw Error('Resultado incompleto ou de outra análise.');
        const fragment = document.createDocumentFragment();
        if (page.playbook) {
          const playbook = node('details'); playbook.open = true;
          playbook.append(node('summary', page.playbook.title), node('p', `Dados necessários: ${page.playbook.required_fields.join(' · ')}`), node('p', `Hipóteses concorrentes: ${page.playbook.competing_hypotheses.join(' · ')}`));
          const steps = node('ol'); for (const text of page.playbook.next_queries) steps.append(node('li', text)); playbook.append(steps); fragment.append(playbook);
        }
        if (page.story) fragment.append(node('p', page.story.text), node('p', page.story.limitations.join(' · '), 'small muted'));
        if (page.story?.claims?.length) { const claims = node('details'); claims.append(node('summary', 'Hipóteses e referências para conferir')); const safe = window.EvidenceUI?.redact ? window.EvidenceUI.redact(page.story.claims) : []; claims.append(node('pre', JSON.stringify(safe, null, 2))); fragment.append(claims); }
        if (controller.section === 'narrative' && page.story?.assistant_contract) { const contract = node('details'); contract.append(node('summary', 'Orientação para síntese via cliente MCP'), node('p', 'Uma síntese assistida pode usar esta página como pacote de evidências. Confira as citações e mantenha as hipóteses separadas das observações.'), node('pre', JSON.stringify(page.story.assistant_contract, null, 2))); fragment.append(contract); }
        if (page.story?.recommendations?.items?.length) {
          const recommendations = node('details'); recommendations.open = true; recommendations.append(node('summary', 'Consultas que ajudam a distinguir as hipóteses'));
          for (const item of page.story.recommendations.items) {
            const row = node('div'); row.append(node('p', `${item.purpose} · valor ordinal ${item.priority_value}; sem probabilidade atribuída`), node('p', item.discriminating_queries.join(' · ')));
            const next = node('button', 'Abrir próximo passo', 'btn ghost small'); next.onclick = () => { controller.section = item.query_plan.section; controller.signalId = item.query_plan.signal_id; controller.offset = 0; void load(); }; row.append(next); recommendations.append(row);
          } fragment.append(recommendations);
        }
        if (controller.section === 'narrative') {
          const draft = node('textarea'); draft.setAttribute('aria-label', 'Síntese assistida com citações (JSON)'); draft.placeholder = JSON.stringify({ claims: [{ kind: 'observation', text: 'Descreva uma observação sustentada pelo original.', event_refs: [], signal_ids: [] }] }, null, 2);
          const review = node('button', 'Conferir citações da síntese', 'btn ghost small'); const reviewed = node('pre'); reviewed.setAttribute('aria-live', 'polite');
          review.onclick = async () => { review.disabled = true; try {
            const result = await options.request('investigation_narrative_review', { analysisId: analysis.analysis_id, draft: JSON.parse(draft.value), namespace: controller.namespace, entity: controller.entity });
            if (!current() || revision !== controller.revision) return;
            if (result.analysis_id !== analysis.analysis_id || result.classification_updated !== false) throw Error('Revisão da síntese incompleta ou de outra análise.');
            reviewed.textContent = JSON.stringify(window.EvidenceUI?.redact ? window.EvidenceUI.redact(result) : result, null, 2);
          } catch (error) { if (current()) reviewed.textContent = String(error); } finally { if (current()) review.disabled = false; } };
          fragment.append(node('p', 'Cole o rascunho produzido pelo seu cliente de IA para conferir suas referências no recorte atual. A validação das citações exige também revisão humana do significado do texto.'), draft, review, reviewed);
        }
        if (!page.items.length) fragment.append(node('p', controller.section === 'signals' ? 'Nenhum sinal calculado neste recorte. Confira o histórico e a cobertura disponíveis.' : 'Nenhuma relação ou evento disponível neste recorte.'));
        for (const item of page.items) {
          const article = node('article', null, 'investigation-item');
          if (controller.section === 'hunts') {
            article.append(node('h3', item.title), node('p', `Dados necessários: ${item.required_fields.join(' · ')}`));
            const run = node('button', 'Abrir caça', 'btn ghost small'); run.type = 'button';
            run.onclick = () => { controller.section = 'hunt'; controller.signalId = item.id; controller.offset = 0; void load(); }; article.append(run);
          } else if (controller.section === 'queue') {
            article.append(node('h3', `${item.value} · prioridade ${item.priority}/100`), node('p', `${item.column} · ${item.namespace}`, 'small muted'));
            const reasoning = node('details'); reasoning.append(node('summary', 'Como a prioridade foi calculada'), node('p', item.ranking.meaning), node('pre', JSON.stringify(item.ranking, null, 2))); article.append(reasoning);
            const actions = node('div', null, 'investigation-navigation');
            for (const [label, section] of [['Sinais da entidade', 'signals'], ['Perfil', 'profile'], ['Cronologia', 'story'], ['Relações', 'graph'], ['Caças guiadas', 'hunts'], ['Cobertura', 'coverage']]) {
              const button = node('button', label, 'btn ghost small'); button.type = 'button'; button.onclick = () => chooseEntity(item, section); actions.append(button);
            }
            article.append(actions);
          } else if (controller.section === 'signals' || controller.section === 'hunt' || controller.section === 'learning') {
            article.append(node('h3', `${names[item.kind] || names[item.kind.split(':')[0]] || item.kind} · ${item.value}`));
            article.append(node('p', `Prioridade ${item.priority}/100 · evidência ainda não avaliada · ${item.namespace}`, 'small muted'));
            article.append(node('p', item.explanation));
            const metrics = node('details'); metrics.append(node('summary', 'Por que investigar'), node('pre', JSON.stringify(item.measurements, null, 2)));
            article.append(metrics);
            if (item.kind === 'local_learning') {
              const comparisons = item.measurements?.comparisons || [];
              const learn = node('button', 'Aprender mensagens e medidas deste recorte', 'btn ghost small'); learn.type = 'button';
              const learnedStatus = node('p'); learnedStatus.setAttribute('aria-live', 'polite');
              learn.onclick = async () => { learn.disabled = true; try { const result = await options.request('investigation_learning', { analysisId: analysis.analysis_id, request: { action: 'accept_expected', signal_id: item.id } }); if (!current() || revision !== controller.revision) return; learnedStatus.textContent = result.applied ? 'Padrões incorporados ao perfil local. Recalcule para comparar; as evidências de segurança permanecem independentes.' : 'A atualização foi retida: são necessários eventos elegíveis e controles independentes suficientes.'; if (result.applied) options.onLearningChanged?.(); } catch (error) { if (current()) learnedStatus.textContent = String(error); } finally { if (learn.isConnected) learn.disabled = false; } };
              article.append(node('p', 'Depois de conferir os originais, você pode incorporar mensagens e medidas esperadas ao perfil. A revisão é opcional e outros desvios ainda podem ser apontados.', 'small muted'), learn, learnedStatus);
              const observations = [...new Set((item.measurements?.deviations || []).flat().map(row => row.detail).filter(Boolean))].slice(0, 6);
              const list = node('ul'); for (const text of observations) list.append(node('li', text)); if (observations.length) article.append(list);
              article.append(node('p', 'As comparações usam a mesma operação e intervalo. Associação não demonstra causa.', 'small muted'));
              if (!comparisons.length) article.append(node('p', 'Não há grupos comparáveis suficientes para distinguir uma explicação.'));
              for (const comparison of comparisons) {
                const row = node('div'); row.append(node('p', comparison.hypothesis), node('p', `${comparison.field}=${comparison.value}: ${comparison.unusual}/${comparison.observations} registros incomuns; outros grupos: ${comparison.comparison_unusual}/${comparison.comparison_observations}.`, 'small muted')); article.append(row);
              }
            }
            const actions = node('div', null, 'investigation-navigation');
            for (const [label, section] of [['Ver eventos de apoio', 'members'], ['Ver cronologia', 'timeline'], ['Ver relações', 'graph'], ['Propor regra', 'proposal']]) {
              if (item.kind === 'local_learning' && section === 'proposal') continue;
              const button = node('button', label, 'btn ghost small'); button.type = 'button';
              button.onclick = () => { if (section === 'members' || section === 'proposal') { controller.section = section; controller.signalId = item.id; controller.entity = null; controller.namespace = item.namespace; controller.offset = 0; void load(); } else chooseEntity(item, section); };
              actions.append(button);
            }
            article.append(actions);
            const alternatives = node('details'); alternatives.append(node('summary', 'Alternativas e dados que faltam'), node('p', [...(item.benign_alternatives || []), ...(item.missing_evidence || [])].join(' · '))); article.append(alternatives);
          } else if (controller.section === 'proposal') {
            article.append(node('h3', 'Proposta de regra para revisão'));
            if (!item.rule) article.append(node('p', item.reason));
            else {
              article.append(node('p', item.limitation), node('pre', JSON.stringify(item.rule, null, 2)));
              const controls = node('textarea'); controls.setAttribute('aria-label', 'Controles positivos e negativos da proposta (JSON)'); controls.value = JSON.stringify({ positive: [], negative: [], holdout_declaration: '' }, null, 2);
              const rationale = node('textarea'); rationale.setAttribute('aria-label', 'Motivo da revisão da proposta'); rationale.placeholder = 'Explique a revisão dos controles e do contexto.';
              const status = node('pre'); status.setAttribute('aria-live', 'polite'); const evaluate = node('button', 'Avaliar com controles', 'btn ghost small'), accept = node('button', 'Ativar regra revisada neste Caso', 'btn ghost small'); accept.disabled = true;
              controls.oninput = () => { accept.disabled = true; };
              evaluate.onclick = async () => { evaluate.disabled = true; accept.disabled = true; try {
                const result = await options.request('investigation_proposal_evaluate', { analysisId: analysis.analysis_id, signalId: item.signal_id, controls: JSON.parse(controls.value) });
                if (!current() || revision !== controller.revision) return;
                if (result.analysis_id !== analysis.analysis_id || result.rule_hash !== item.rule_hash || result.complete !== true) throw Error('Avaliação incompleta ou desatualizada.');
                status.textContent = JSON.stringify(result, null, 2); accept.disabled = result.activation_gate !== true;
              } catch (error) { if (current()) status.textContent = String(error); } finally { if (current()) evaluate.disabled = false; } };
              accept.onclick = async () => { accept.disabled = true; try { await options.request('investigation_proposal_accept', { analysisId: analysis.analysis_id, signalId: item.signal_id, rationale: rationale.value }); options.onRuleSaved?.(); } catch (error) { if (current()) status.textContent = String(error); } };
              article.append(node('p', 'Rotule referências dos eventos originais e declare a separação por origem/tempo. Avaliar executa o seletor no universo inteiro. A regra aceita permanece com nível não avaliado.'), controls, rationale, evaluate, accept, status);
            }
          } else if (controller.section === 'revision') {
            const labels={added:'Novo achado',retracted:'Achado retirado da análise corrente',revised_classification:'Classificação revisada'};
            article.append(node('h3',labels[item.kind]||item.kind),node('p',`${item.finding_id} · ${item.namespace}`),node('pre',JSON.stringify(window.EvidenceUI?.redact?window.EvidenceUI.redact(item.classification):{},null,2)),node('p','A retirada indica mudança entre cálculos completos. Ela não demonstra, por si só, que a atividade foi benigna.','small muted'));
          } else if (controller.section === 'schema') {
            article.append(node('h3', `${item.source} · ${item.field}`), node('p', `${item.namespace} · ${item.present}/${item.population} registros · formato sugerido: ${item.suggested_type || 'indefinido'}`), node('p', `Consistência do formato: ${Math.round((item.confidence.value || 0) * 100)}% em ${item.confidence.support} valores. Isso não atribui origem, destino, unidades ou papel de identidade. Confirme o mapeamento nas regras.`), node('pre', JSON.stringify(item.types, null, 2)));
          } else if (controller.section === 'profile') {
            article.append(node('h3', `Perfil de ${item.entity || 'todo o universo'}`), node('p', item.meaning), node('pre', JSON.stringify(item.statistics, null, 2)), node('p', `Referência histórica: ${item.history?.status || 'disabled'}`));
          } else if (controller.section === 'coverage') {
            article.append(node('h3', item.title), node('p', `${item.population} registros · ${item.status}`)); const fields = node('ul');
            for (const field of item.fields) fields.append(node('li', `${field.field}: ${field.observed}/${field.population} · ${field.status}`)); article.append(fields, node('p', item.meaning, 'small muted'));
          } else if (controller.section === 'ioc_matches') {
            const match = window.EvidenceUI?.redact ? window.EvidenceUI.redact(item.match) : {};
            article.append(node('h3', item.indicator_id), node('p', 'Correspondência exata no campo declarado. Confira procedência, revisão e validade temporal.'), node('pre', JSON.stringify(match, null, 2)));
          } else if (controller.section === 'graph') {
            article.append(node('p', `${item.left_kind}: ${item.left_value} → ${item.right_kind}: ${item.right_value}`));
            article.append(node('p', `${item.relation} · vínculo ${{explicit:"explícito",contextual:"contextual",hypothetical:"hipotético"}[item.quality]||item.quality} · ${item.namespace}`, 'small muted'));
          } else {
            article.append(node('p', `${item.timestamp == null ? 'Horário não disponível' : new Date(item.timestamp).toLocaleString('pt-BR')} · ${item.actor || item.source || item.event_ref}`));
            if (item.facts) article.append(node('pre', JSON.stringify(window.EvidenceUI?.redact ? window.EvidenceUI.redact(item.facts) : { product: item.facts.product, limitations: item.facts.limitations }, null, 2)));
          }
          if (item.event_ref) {
            const button = node('button', 'Abrir evento original', 'btn ghost small'); button.type = 'button';
            button.onclick = async () => {
              button.disabled = true;
              try {
                const event = await options.request('triage_evidence_event', { analysisId: analysis.analysis_id, eventRef: item.event_ref, eventId: item.event_id });
                if (!current() || revision !== controller.revision) return;
                const safe = window.EvidenceUI?.redact ? window.EvidenceUI.redact(event) : { event_ref: event.event_ref, source: event.source, timestamp: event.timestamp };
                const detail = node('details'); detail.open = true; detail.append(node('summary', 'Evento original'), node('pre', JSON.stringify(safe, null, 2))); article.append(detail);
              } catch (error) { if (current()) article.append(node('p', String(error), 'error')); }
              finally { if (button.isConnected) button.disabled = false; }
            };
            article.append(button);
          }
          fragment.append(article);
        }
        content.replaceChildren(fragment);
        const info = node('span', `${page.total} itens · ${controller.section}`, 'small muted'); pagination.append(info);
        for (const [label, offset] of [['Anterior', controller.offset > 0 ? Math.max(0, controller.offset - 20) : null], ['Próxima', page.next_offset]]) {
          const button = node('button', label, 'btn ghost small'); button.type = 'button'; button.disabled = offset == null;
          button.onclick = () => { controller.offset = offset; void load(); }; pagination.append(button);
        }
      } catch (error) {
        if (!current() || revision !== controller.revision) return;
        content.replaceChildren(node('p', `Não foi possível abrir a investigação: ${error}`));
        const retry = node('button', 'Tentar novamente', 'btn ghost small'); retry.type = 'button'; retry.onclick = () => void load(); content.append(retry);
      }
    }
    void load();
  }
  return { mount };
})();
