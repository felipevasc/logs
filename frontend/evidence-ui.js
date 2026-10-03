/* Presentation only. Classification is always supplied by the local backend. */
window.EvidenceUI = (() => {
  const names = ["Nível não avaliado", "Inconclusivo", "Suspeita", "Indício", "Forte indício", "Quase confirmado"];
  const rigidity = names;
  const level = value => Number.isInteger(value) && value >= 1 && value <= 5 ? value : 0;
  const label = value => names[level(value)];
  const badge = value => {
    const n = level(value);
    return `<span class="evidence-badge evidence-e${n}"><span>${esc(label(n))}</span>${n ? `<span class="evidence-marks" aria-hidden="true">${"●".repeat(n)}${"○".repeat(5 - n)}</span>` : ""}</span>`;
  };
  const outcome = value => ({ success: "operação bem-sucedida", failure: "falha", blocked: "bloqueado", unknown: "resultado desconhecido", mixed: "resultados distintos" }[value] || "resultado desconhecido");
  const claim = value => ({attempt:"Tentativa observada",execution:"Execução observada",effect:"Efeito observado",activity:"Atividade observada"}[value] || "Atividade observada");
  function excerpts(d, limit = 3) {
    if (!d.excerpts?.length) return '<p class="sec-no-excerpt">Não há trecho textual preservado neste achado. Consulte os eventos, as relações e as condições abaixo.</p>';
    return `<section class="sec-excerpts" aria-label="Trechos que motivaram o indício"><strong>O que chamou atenção</strong>${d.excerpts.slice(0, limit).map(x => {
      // Mask the complete excerpt before splitting so secrets cannot leak across a mark boundary.
      const pieces = [x.before || '', x.matched || '', x.after || ''];
      const combined = pieces.join(''), safe = redact(combined);
      const masked = safe !== combined;
      const body = masked ? `<mark>${esc(safe)}</mark><small>Trecho protegido: destaque ampliado para preservar o segredo.</small>` : `${esc(pieces[0])}<mark>${esc(pieces[1])}${x.match_truncated ? '…' : ''}</mark>${esc(pieces[2])}`;
      return `<figure><figcaption><code>${esc(x.field)}</code><span>${esc(x.transformation === 'original' || !x.transformation ? 'Conteúdo original' : x.transformation)}</span></figcaption><pre>${x.prefix_omitted ? '…' : ''}${body}${x.suffix_omitted ? '…' : ''}</pre></figure>`;
    }).join('')}</section>`;
  }
  function explanation(d, { revealed = false } = {}) {
    const list = (title, values) => values?.length ? `<div class="evidence-explanation"><strong>${title}</strong><ul>${values.map(v => `<li>${esc(revealed ? v : redact(v))}</li>`).join("")}</ul></div>` : "";
    return `<p><strong>${claim(d.claim)}</strong> · ${outcome(d.outcome)}. O nível não confirma comprometimento.</p>` + (d.maturity === "experimental" ? '<p class="muted small">Regra experimental: a precisão em ambiente real ainda exige validação representativa.</p>' : "") + list("Por que recebeu este nível", d.evidence_reasons)
      + list("Evidências faltantes e limitações", d.missing_evidence)
      + list("Condições satisfeitas", d.conditions_satisfied)
      + list("Medidas e cobertura utilizadas", d.measurements ? [JSON.stringify(d.measurements)] : [])
      + list("Alternativas benignas", d.benign_alternatives)
      + list("Relações demonstradas", d.relationships?.map(r => `${r.description} (${r.fields.join(", ")})`));
  }
  function inspect(d) {
    const previous = document.activeElement, dialog = document.createElement('dialog');
    dialog.className = 'evidence-inspector'; dialog.setAttribute('aria-label','Fundamentação do indício');
    const checks = d.checks || [];
    const stateName = c => ({passed:'Verificado',failed:'Operação bloqueada ou falhou',unknown:'Não demonstrado',observed:'Registrado'}[c.status] || 'Não avaliado');
    const checksHtml = checks.length ? checks.map(c => `<article class="evidence-check check-${esc(c.status)}"><div><span class="check-state">${esc(stateName(c))}</span><strong>${esc(c.label)}</strong></div><p>${esc(redact(c.label === 'Resultado da operação' ? outcome(c.observed) : c.observed))}</p>${c.fields?.length ? `<small>Campos: ${c.fields.map(f=>`<code>${esc(f)}</code>`).join(', ')}</small>` : ''}<details><summary>Critério e referências${c.event_refs?.length ? ` · ${c.event_refs.length} amostra(s)` : ''}</summary><pre>${esc(redact(c.expected))}</pre>${(c.event_refs || []).map(r=>`<code class="check-reference">${esc(r)}</code>`).join('')}</details></article>`).join('') : `<p class="muted">Este achado foi salvo sem verificações estruturadas. As condições registradas estão abaixo; uma nova análise produz o detalhamento atual.</p>${(d.conditions_satisfied || []).map(c=>`<pre>${esc(redact(c))}</pre>`).join('')}`;
    const list = values => (values || []).map(v=>`<li>${esc(redact(v))}</li>`).join('');
    dialog.innerHTML = `<header><div><small>Fundamentação do indício</small><h3>${esc(d.name || 'Indício')}</h3></div><button class="icon-btn" type="button" data-close aria-label="Fechar fundamentação"><i class="fas fa-xmark" aria-hidden="true"></i></button></header><div class="evidence-inspector-body">${badge(d.evidence_level)}<p>${esc(claim(d.claim))} · ${esc(outcome(d.outcome))}</p><p class="evidence-rationale">${esc(redact((d.evidence_reasons || []).join(' ')))}</p><h4>Verificações realizadas</h4><div class="evidence-checks">${checksHtml}</div><details class="evidence-secondary"><summary>O que falta demonstrar${d.missing_evidence?.length ? ` · ${d.missing_evidence.length}` : ''}</summary><p class="muted small">Lacunas e requisitos de validação. Não são testes reprovados nem fatos que ocorreram.</p><ul>${list(d.missing_evidence)}</ul></details><details class="evidence-secondary"><summary>Contexto e alternativas</summary><ul>${list(d.benign_alternatives)}</ul>${d.maturity==='experimental'?'<p class="small">Regra experimental: precisão em corpus representativo ainda não avaliada.</p>':''}</details><details class="evidence-secondary"><summary>Relações e versão da regra</summary><ul>${list(d.relationships?.map(r=>`${r.description} · ${r.fields.join(', ')}`))}</ul><p><code>${esc(d.rule || '')}</code> · versão ${esc(d.rule_version || 'histórica')}</p><small>Política ${esc(d.policy_version || 'não avaliada')} · Normalização ${esc(d.normalization_version || 'não avaliada')}</small></details></div>`;
    const close=()=>{dialog.close();dialog.remove();previous?.isConnected&&previous.focus();};
    dialog.querySelector('[data-close]').onclick=close;
    dialog.addEventListener('cancel',e=>{e.preventDefault();close();});
    dialog.addEventListener('click',e=>{if(e.target===dialog){const r=dialog.getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)close();}});
    document.body.append(dialog);dialog.showModal();dialog.querySelector('[data-close]').focus();
  }
  function control(data, minimum) {
    const counts = data.counts_by_level || [0, 0, 0, 0, 0];
    const total = counts.reduce((sum, n) => sum + n, 0), visible = counts.slice(minimum - 1).reduce((sum, n) => sum + n, 0);
    return `<section class="evidence-control" aria-label="Força mínima da evidência"><div class="evidence-control-heading"><strong>Mostrar a partir de</strong><span aria-live="polite">${fmtNum(visible)} indícios visíveis · ${fmtNum(total)} disponíveis</span></div>
      <div class="evidence-segments" role="group" aria-label="Nível mínimo de evidência">${[5, 4, 3, 2, 1].map(n => {
        const cumulative = counts.slice(n - 1).reduce((sum, v) => sum + v, 0);
        return `<button type="button" data-evidence-min="${n}" aria-pressed="${n === minimum}" title="${n === 5 ? 'Somente ' : 'Inclui níveis acima de '}${names[n]} · ${cumulative} indícios${n < minimum ? ` · acrescenta ${cumulative - visible}` : ""}"><strong>${names[n]}</strong><small>${n === 5 ? 'Somente este nível' : 'Este nível e os mais fortes'} · ${fmtNum(cumulative)}</small></button>`;
      }).join("")}</div><div class="evidence-counts">${[5, 4, 3, 2, 1].map(n => `<span>${names[n]}: ${fmtNum(counts[n - 1])}</span>`).join("")}${minimum < 5 ? '<button type="button" class="text-button" data-evidence-min="5">Somente Quase confirmado</button>' : ""}</div></section>`;
  }
  function exportMetadata(data, minimum, scope) {
    return { minimum_evidence: minimum, scope, analysis_id: data.analysis_id, policy_version: data.policy_version, normalization_version: data.normalization_version, attack_version: data.attack_version, limitations: structuredClone(data.limitations || []) };
  }
  const isSensitiveKey = key => /(?:^|[.@])(password|passwd|token|secret|authorization|cookie|apikey|accesstoken|refreshtoken|secretaccesskey|clientsecret)$/i.test(key.replace(/[_-]/g, ""));
  function redact(value) {
    if (typeof value === "string") value = value.replace(/\$(?:[156y]|2[aby])\$[./A-Za-z0-9$=,-]{20,}/g, '[hash protegido]');
    if (typeof value === "string") value = value.replace(/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?(?:-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|$)/g, '[chave privada oculta]').replace(/((?:AWS_SECRET_ACCESS_KEY|DB_PASSWORD|DATABASE_PASSWORD)\s*=\s*)[^\r\n]+/g, '$1[oculto]');
    if (typeof value === "string") return value.replace(/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g, "[chave privada oculta]").replace(/(password|passwd|access[_-]?token|refresh[_-]?token|secretAccessKey|client[_-]?secret|token|secret|authorization|cookie|api[_-]?key)(["']?\s*[:=]\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:Bearer|Basic)\s+[^\s",;]+|[^\s",;]+)/gi, '$1$2"[oculto]"');
    if (Array.isArray(value)) return value.map(redact);
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, val]) => [key, isSensitiveKey(key) ? "[oculto]" : redact(val)]));
    return value;
  }
  const eventContext = (item, event) => (item.detection?.detections || []).filter(d => d.event_refs?.includes(event.event_ref)).map(d => `${label(d.evidence_level)}: ${d.name}`).join("; ");
  function report(item) {
    if (!item.detection) return "";
    const analysis = item.detection;
    return [`Nível mínimo registrado: ${label(analysis.minimum_evidence)} · Escopo: ${analysis.scope || "histórico"} · Política: ${analysis.policy_version || "legada"} · ATT&CK: ${analysis.attack_version || "histórico"}`,
      `Avaliação do analista: ${analysis.analyst_state || "unreviewed"}`,
      ...(analysis.detections || []).map(d=>window.ParticipantsUI?.report(d.participants)||'').filter(Boolean),
      ...(analysis.detections || []).map(d => `${label(d.evidence_level)} — ${d.name}\n${claim(d.claim)} · ${outcome(d.outcome)}\nImpacto potencial: ${d.severity}\nRegra: ${d.rule_version || "legada"} · Normalização: ${d.normalization_version || "legada"} · Avaliação: ${d.evaluation || "não avaliada"}\n${(d.evidence_reasons || []).join("; ")}\nLimitações: ${(d.missing_evidence || []).join("; ") || "não registradas"}\nAlternativas benignas: ${(d.benign_alternatives || []).join("; ")}\nRelações: ${(d.relationships || []).map(r => r.description).join("; ")}\nEventos: ${(d.event_refs || []).join(", ")}`)].join("\n\n");
  }
  function mountCase(host, item) {
    if (!item?.detection || !host) return;
    const box = el("section", "case-detection-evidence");
    box.innerHTML = (item.detection.detections || []).map((d,i) => `<details><summary>${badge(d.evidence_level)} ${esc(d.name)}</summary><small>Impacto potencial: ${esc(d.severity)} · ${esc(outcome(d.outcome))}</small>${excerpts(d)}<button type="button" class="btn ghost" data-case-evidence-inspect="${i}"><i class="fas fa-circle-info" aria-hidden="true"></i> Ver fundamentação</button></details>`).join("");
    box.querySelectorAll('[data-case-evidence-inspect]').forEach(b=>b.onclick=()=>inspect(item.detection.detections[+b.dataset.caseEvidenceInspect]));
    const field = el("label", "small", "Avaliação do analista: ");
    const select = el("select");
    for (const [value, text] of [["unreviewed", "Não avaliado"], ["confirmed", "Comprometimento confirmado pelo analista"], ["benign", "Atividade benigna"], ["inconclusive", "Inconclusivo"]]) { const option = el("option", "", text); option.value = value; select.append(option); }
    select.value = item.detection.analyst_state || "unreviewed";
    select.onchange = async () => { item.detection.analyst_state = select.value; item.detection.reviewed_at = Date.now(); await saveCases(); };
    field.append(select); box.append(field); host.append(box);
  }
  return { names, rigidity, level, label, badge, outcome, claim, excerpts, explanation, inspect, control, exportMetadata, redact, isSensitiveKey, eventContext, report, mountCase };
})();
