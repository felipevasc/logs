/* Presentation only. Classification is always supplied by the local backend. */
window.EvidenceUI = (() => {
  const names = ["Nível não avaliado", "Pista exploratória", "Suspeita contextual", "Suspeita relevante", "Suspeita forte", "Evidência muito forte"];
  const rigidity = ["", "Exploratória", "Ampliada", "Intermediária", "Alta", "Máxima"];
  const level = value => Number.isInteger(value) && value >= 1 && value <= 5 ? value : 0;
  const label = value => { const n = level(value); return n ? `E${n} · ${names[n]}` : names[0]; };
  const badge = value => {
    const n = level(value);
    return `<span class="evidence-badge evidence-e${n}"><span>${esc(label(n))}</span>${n ? `<span class="evidence-marks" aria-hidden="true">${"●".repeat(n)}${"○".repeat(5 - n)}</span>` : ""}</span>`;
  };
  const outcome = value => ({ success: "operação bem-sucedida", failure: "falha", blocked: "bloqueado", unknown: "resultado desconhecido", mixed: "resultados distintos" }[value] || "resultado desconhecido");
  const claim = value => ({attempt:"Tentativa observada",execution:"Execução observada",effect:"Efeito observado",activity:"Atividade observada"}[value] || "Atividade observada");
  function explanation(d) {
    const list = (title, values) => values?.length ? `<div class="evidence-explanation"><strong>${title}</strong><ul>${values.map(v => `<li>${esc(redact(v))}</li>`).join("")}</ul></div>` : "";
    return `<p><strong>${claim(d.claim)}</strong> · ${outcome(d.outcome)}. O nível não confirma comprometimento.</p>` + (d.maturity === "experimental" ? '<p class="muted small">Regra experimental: a precisão em ambiente real ainda exige validação representativa.</p>' : "") + list("Por que recebeu este nível", d.evidence_reasons)
      + list("Evidências faltantes e limitações", d.missing_evidence)
      + list("Condições satisfeitas", d.conditions_satisfied)
      + list("Medidas e cobertura utilizadas", d.measurements ? [JSON.stringify(d.measurements)] : [])
      + list("Alternativas benignas", d.benign_alternatives)
      + list("Relações demonstradas", d.relationships?.map(r => `${r.description} (${r.fields.join(", ")})`));
  }
  function control(data, minimum) {
    const counts = data.counts_by_level || [0, 0, 0, 0, 0];
    const total = counts.reduce((sum, n) => sum + n, 0), visible = counts.slice(minimum - 1).reduce((sum, n) => sum + n, 0);
    return `<section class="evidence-control" aria-label="Rigidez da análise"><div class="evidence-control-heading"><strong>Rigidez da análise</strong><span aria-live="polite">${fmtNum(visible)} indícios visíveis · ${fmtNum(total)} disponíveis</span></div>
      <div class="evidence-segments" role="group" aria-label="Nível mínimo de evidência">${[5, 4, 3, 2, 1].map(n => {
        const cumulative = counts.slice(n - 1).reduce((sum, v) => sum + v, 0);
        return `<button type="button" data-evidence-min="${n}" aria-pressed="${n === minimum}" title="${n === 5 ? "Somente E5" : `E${n} até E5`} · ${cumulative} indícios${n < minimum ? ` · acrescenta ${cumulative - visible}` : ""}"><strong>${n} — ${rigidity[n]}</strong><small>${n === 5 ? "Somente E5" : `E${n} até E5`} · ${fmtNum(cumulative)}</small></button>`;
      }).join("")}</div><div class="evidence-counts">${[5, 4, 3, 2, 1].map(n => `<span>E${n}: ${fmtNum(counts[n - 1])}</span>`).join("")}${minimum < 5 ? '<button type="button" class="text-button" data-evidence-min="5">Somente E5</button>' : ""}</div></section>`;
  }
  function exportMetadata(data, minimum, scope) {
    return { minimum_evidence: minimum, scope, analysis_id: data.analysis_id, policy_version: data.policy_version, normalization_version: data.normalization_version, attack_version: data.attack_version, limitations: structuredClone(data.limitations || []) };
  }
  function redact(value) {
    if (typeof value === "string") return value.replace(/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g, "[chave privada oculta]").replace(/(password|passwd|access[_-]?token|refresh[_-]?token|secretAccessKey|client[_-]?secret|token|secret|authorization|cookie|api[_-]?key)(["']?\s*[:=]\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:Bearer|Basic)\s+[^\s",;]+|[^\s",;]+)/gi, '$1$2"[oculto]"');
    if (Array.isArray(value)) return value.map(redact);
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, val]) => [key, /(?:^|[.@])(password|passwd|token|secret|authorization|cookie|apikey|accesstoken|refreshtoken|secretaccesskey|clientsecret)$/i.test(key.replace(/[_-]/g, "")) ? "[oculto]" : redact(val)]));
    return value;
  }
  const eventContext = (item, event) => (item.detection?.detections || []).filter(d => d.event_refs?.includes(event.event_ref)).map(d => `${label(d.evidence_level)}: ${d.name}`).join("; ");
  function report(item) {
    if (!item.detection) return "";
    const analysis = item.detection;
    return [`Rigidez registrada: ${analysis.minimum_evidence || "não avaliada"} · Escopo: ${analysis.scope || "histórico"} · Política: ${analysis.policy_version || "legada"} · ATT&CK: ${analysis.attack_version || "histórico"}`,
      `Avaliação do analista: ${analysis.analyst_state || "unreviewed"}`,
      ...(analysis.detections || []).map(d => `${label(d.evidence_level)} — ${d.name}\n${claim(d.claim)} · ${outcome(d.outcome)}\nImpacto potencial: ${d.severity}\nRegra: ${d.rule_version || "legada"} · Normalização: ${d.normalization_version || "legada"} · Avaliação: ${d.evaluation || "não avaliada"}\n${(d.evidence_reasons || []).join("; ")}\nLimitações: ${(d.missing_evidence || []).join("; ") || "não registradas"}\nAlternativas benignas: ${(d.benign_alternatives || []).join("; ")}\nRelações: ${(d.relationships || []).map(r => r.description).join("; ")}\nEventos: ${(d.event_refs || []).join(", ")}`)].join("\n\n");
  }
  function mountCase(host, item) {
    if (!item?.detection || !host) return;
    const box = el("section", "case-detection-evidence");
    box.innerHTML = (item.detection.detections || []).map(d => `<details><summary>${badge(d.evidence_level)} ${esc(d.name)}</summary><small>Impacto potencial: ${esc(d.severity)} · ${esc(outcome(d.outcome))}</small>${explanation(d)}</details>`).join("");
    const field = el("label", "small", "Avaliação do analista: ");
    const select = el("select");
    for (const [value, text] of [["unreviewed", "Não avaliado"], ["confirmed", "Comprometimento confirmado pelo analista"], ["benign", "Atividade benigna"], ["inconclusive", "Inconclusivo"]]) { const option = el("option", "", text); option.value = value; select.append(option); }
    select.value = item.detection.analyst_state || "unreviewed";
    select.onchange = async () => { item.detection.analyst_state = select.value; item.detection.reviewed_at = Date.now(); await saveCases(); };
    field.append(select); box.append(field); host.append(box);
  }
  return { names, rigidity, level, label, badge, outcome, claim, explanation, control, exportMetadata, redact, eventContext, report, mountCase };
})();
