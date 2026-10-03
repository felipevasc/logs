/* Restart-only native resource preferences. This small control surface invokes
   its own commands: saving must not invalidate the current dataset or query. */
window.ResourceSettings = (() => {
  "use strict";
  const call = async (command, args) => window.__TAURI__.core.invoke(command, args);
  const node = (tag, cls, text) => {
    const item = document.createElement(tag);
    if (cls) item.className = cls;
    if (text != null) item.textContent = text;
    return item;
  };
  const mib = value => `${Number(value).toLocaleString("pt-BR")} MiB`;
  let status = null, draft = null, loading = null, saving = false, message = "", failed = false, pane = null;
  let caseOwner = null, caseStatus = null, caseDraft = null, caseLoading = null, caseSaving = false, caseMessage = "", caseFailed = false;
  const contexts = () => window.AnalysisContexts;
  const caseKey = owner => JSON.stringify([owner?.instance, owner?.identity || null]);
  function casePreferences(mode, text, maximum) {
    if (mode === "inherit") return { schemaVersion: 1, mode, workLimitMib: null };
    const value = Number(text);
    if (mode !== "custom" || !/^[1-9]\d*$/.test(String(text)) || !Number.isSafeInteger(value) || value < 8 || !Number.isSafeInteger(maximum) || value > maximum) {
      throw Error(`Informe uma cota inteira entre 8 e ${maximum} MiB.`);
    }
    return { schemaVersion: 1, mode, workLimitMib: value };
  }
  function resetCaseDraft() {
    caseDraft = { mode: caseStatus.preferences.mode, work: String(caseStatus.preferences.workLimitMib ?? Math.min(caseStatus.maximumWorkMib, caseStatus.effective.accountedLimitMib)) };
  }
  async function syncCase() {
    const owner = contexts()?.capture?.();
    if (caseKey(owner) === caseKey(caseOwner) && (caseStatus || caseLoading)) return caseLoading;
    caseOwner = owner || null; caseStatus = null; caseDraft = null; caseMessage = ""; caseFailed = false;
    if (!owner?.identity) { caseLoading = null; render(); return; }
    const key = caseKey(owner);
    const pending = (async () => {
      try {
        const result = await call("case_resource_settings_status", { identity: owner.identity });
        if (caseKey(caseOwner) !== key || !contexts()?.isCurrent(owner)) return;
        caseStatus = result; resetCaseDraft();
      } catch (error) {
        if (caseKey(caseOwner) === key) { caseMessage = `Não foi possível ler os recursos do Caso: ${error}`; caseFailed = true; }
      } finally {
        if (caseKey(caseOwner) === key) { caseLoading = null; render(); }
      }
    })();
    caseLoading = pending; render(); return pending;
  }
  async function saveCase() {
    if (caseSaving || !caseStatus || !caseOwner?.identity) return;
    const owner = caseOwner;
    let submitted;
    try { contexts().assertOwner(owner); submitted = casePreferences(caseDraft.mode, caseDraft.work, caseStatus.maximumWorkMib); }
    catch (error) { caseMessage = String(error.message || error); caseFailed = true; render(); return; }
    caseSaving = true; caseMessage = "Salvando recursos do Caso…"; caseFailed = false; render();
    try {
      const result = await call("case_resource_settings_save", { expected: owner.identity, preferences: submitted });
      await contexts().adopt(result.analysisContext, { owner });
      if (contexts().owns(owner) && contexts().capture().caseId === owner.caseId) {
        caseOwner = contexts().capture(); caseStatus = result; resetCaseDraft();
        caseMessage = "Cota salva neste Caso. Novas consultas usam a nova configuração; trabalhos já admitidos mantêm sua captura.";
      }
    } catch (error) {
      if (caseKey(caseOwner) === caseKey(owner)) { caseMessage = `Não foi possível salvar os recursos do Caso: ${error}`; caseFailed = true; }
    } finally { caseSaving = false; render(); }
  }
  function renderCase() {
    const box = node("section", "ui-pref");
    box.append(node("strong", "", "Cota lógica deste Caso"));
    box.append(node("p", "muted small", "Cota independente de volume lógico contabilizado: agregações e rankings, materialização analítica de evidências e IDs de seleções, inclusive IDs em tabelas ou spill. Não configura RAM do DuckDB, disco temporário ou threads; esses motores seguem os padrões do aplicativo. Paginação e hidratação conservam seus limites específicos. Histórico, prévias, recuperação e importação/exportação de arquivos preservados continuam sob as cotas globais."));
    if (!caseOwner?.identity) { box.append(node("p", "muted small", "Abra e salve um Caso para configurar sua cota independente.")); return box; }
    if (!caseStatus) {
      box.append(node("p", caseFailed ? "update-error" : "muted small", caseMessage || "Carregando recursos do Caso…"));
      if (caseFailed) { const retry = button("Reler recursos do Caso", () => { caseOwner = null; return syncCase(); }); box.append(retry); }
      return box;
    }
    const effective = caseStatus.effective;
    box.append(node("p", "", `Cota lógica efetiva do Caso: ${mib(effective.accountedLimitMib)} · Trabalho: até ${mib(effective.workLiveMib)} · Seleção lógica: até ${mib(effective.selectionMib)}`));
    box.append(node("p", "muted small", `Materialização de evidências: ${mib(effective.materializedMib)} · Valores analíticos: ${mib(effective.analyticsMib)} · Lista completa de IDs: ${mib(effective.collectedIdsMib)} · Cache de seleção: ${mib(effective.selectionCacheMib)}`));
    box.append(node("p", "muted small", `As revisões deste Caso compartilham a cota enquanto houver consumidores. Limites agregados do aplicativo: trabalho ${mib(effective.applicationWorkMib)} e seleções lógicas ${mib(effective.applicationSelectionMib)}. A soma contabilizada não representa memória física.`));
    if (caseStatus.preferences.workLimitMib != null) box.append(node("p", "muted small", `Preferência salva do Caso: ${mib(caseStatus.preferences.workLimitMib)}. O valor efetivo acima respeita os pools globais e a máquina atual.`));
    if (caseStatus.clamped) box.append(node("p", "update-warn", "A preferência salva excede os pools globais efetivos ou a memória desta máquina. A execução usa a cota efetiva mostrada acima; o valor salvo foi preservado. Aumentar esta cota não amplia os motores nem os limites individuais."));
    const form = node("form"); form.noValidate = true;
    const mode = node("select"); mode.id = "case-resource-mode"; mode.disabled = caseSaving;
    for (const [value, label] of [["inherit", "Herdar limites do aplicativo"], ["custom", "Cota deste Caso"]]) { const option = node("option", "", label); option.value = value; mode.append(option); }
    mode.value = caseDraft.mode;
    mode.onchange = () => { caseDraft.mode = mode.value; caseMessage = ""; render(); };
    const work = node("input"); work.id = "case-resource-work"; work.type = "number"; work.min = "8"; work.max = String(caseStatus.maximumWorkMib); work.step = "1";
    work.value = caseDraft.work; work.disabled = caseSaving || caseDraft.mode !== "custom";
    work.oninput = () => { caseDraft.work = work.value; caseMessage = "Alterações deste Caso ainda não salvas."; feedback.textContent = caseMessage; };
    const feedback = node("p", caseFailed ? "update-error" : "muted small", caseMessage); feedback.id = "case-resource-feedback"; feedback.setAttribute("role", "status"); feedback.setAttribute("aria-live", "polite");
    work.setAttribute("aria-describedby", feedback.id);
    form.append(field("Modo do Caso", mode), field("Volume lógico contabilizado (MiB)", work), node("p", "muted small", `Entre 8 e ${caseStatus.maximumWorkMib} MiB. Herdar mantém os limites individuais do aplicativo. O máximo configurável acompanha a memória total detectada. A cota efetiva fica limitada aos pools globais, atualmente até ${mib(caseStatus.maximumEffectiveWorkMib ?? Math.min(caseStatus.maximumWorkMib, effective.applicationWorkMib + effective.applicationSelectionMib))}. O orçamento de memória do aplicativo pode ser ajustado abaixo para o próximo início, mas esta cota não amplia limites individuais nem reserva RAM.`));
    const actions = node("div", "modal-actions");
    const discard = button("Descartar alterações do Caso", () => { resetCaseDraft(); caseMessage = ""; render(); }); discard.disabled = caseSaving;
    const submit = button(caseSaving ? "Salvando Caso…" : "Salvar recursos deste Caso", () => {}, "btn primary small"); submit.type = "submit"; submit.id = "case-resource-save"; submit.disabled = caseSaving;
    actions.append(discard, submit); form.append(actions, feedback); form.onsubmit = event => { event.preventDefault(); void saveCase(); }; box.append(form); return box;
  }

  function parallelismPreference(text, maximum = 64) {
    if (text == null) return null;
    const value = Number(text);
    if (!/^[1-9]\d*$/.test(String(text)) || !Number.isSafeInteger(value) || value > maximum) {
      throw Error(`Informe um limite de paralelismo inteiro entre 1 e ${maximum}.`);
    }
    return value;
  }
  function preferences(mode, text, maximum, parallelism = null, maximumParallelism = 64) {
    const parallelismLimit = parallelismPreference(parallelism, maximumParallelism);
    if (mode === "automatic") return { schemaVersion: 1, mode, memoryLimitMib: null, parallelismLimit };
    const value = Number(text);
    if (mode !== "custom" || !/^[1-9]\d*$/.test(String(text)) || !Number.isSafeInteger(value) || value < 128 || !Number.isSafeInteger(maximum) || value > maximum) {
      throw Error(`Informe um orçamento inteiro entre 128 e ${maximum} MiB.`);
    }
    return { schemaVersion: 1, mode, memoryLimitMib: value, parallelismLimit };
  }
  function resetDraft() {
    draft = { mode: status.saved.mode, memory: String(status.saved.memoryLimitMib ?? status.active.memoryBudgetMib),
      parallelismMode: status.saved.parallelismLimit == null ? "automatic" : "custom",
      parallelism: String(status.saved.parallelismLimit ?? status.active.globalParallelism) };
  }
  function submittedPreferences() {
    return preferences(draft.mode, draft.memory, status.maximumMemoryMib,
      draft.parallelismMode === "automatic" ? null : draft.parallelism, status.maximumParallelism);
  }
  function changed() {
    try { const current = submittedPreferences(); return current.mode !== status.saved.mode || current.memoryLimitMib !== status.saved.memoryLimitMib || current.parallelismLimit !== (status.saved.parallelismLimit ?? null); }
    catch { return true; }
  }
  function notice(text, error = false) { message = text; failed = error; }
  async function load() {
    if (loading) return loading;
    loading = (async () => {
      try { status = await call("resource_settings_status"); resetDraft(); notice(""); }
      catch (error) { notice(`Não foi possível ler a configuração de recursos: ${error}`, true); }
      finally { loading = null; render(); }
    })();
    return loading;
  }
  async function save() {
    if (saving || !status) return;
    let submitted;
    try { submitted = submittedPreferences(); }
    catch (error) { notice(String(error.message || error), true); render(); return; }
    saving = true; notice("Salvando…"); render();
    try {
      status = await call("resource_settings_save", { preferences: submitted });
      resetDraft();
      notice(status.restartRequired ? "Preferência salva. Feche e abra o LogInsight quando concluir as tarefas em andamento." : "Preferência salva. Não há mudança de preferência pendente para o próximo início.");
    } catch (error) { notice(`Não foi possível salvar: ${error}`, true); }
    finally { saving = false; render(); }
  }
  function button(text, action, cls = "btn ghost small") {
    const item = node("button", cls, text); item.type = "button";
    item.disabled = saving; item.onclick = action; return item;
  }
  function field(label, control) {
    const box = node("div", "fld"), caption = node("label", "", label);
    caption.htmlFor = control.id; box.append(caption, control); return box;
  }
  function render() {
    if (!pane) return;
    const focused = document.activeElement?.id;
    pane.replaceChildren();
    const feedback = node("p", failed ? "update-error" : "muted small", message);
    feedback.id = "resource-settings-feedback"; feedback.setAttribute("role", "status");
    feedback.tabIndex = -1;
    feedback.setAttribute("aria-live", "polite");
    if (!status) {
      pane.append(node("p", "muted", failed ? "Configuração de recursos indisponível." : "Carregando configuração de recursos…"), feedback);
      if (failed) pane.append(button("Tentar novamente", () => { notice(""); render(); return load(); }));
      return;
    }
    const active = status.active;
    const summary = node("div", "ui-pref");
    summary.append(node("strong", "", "Orçamento de memória do aplicativo nesta sessão"));
    summary.append(node("p", "", `Orçamento de referência ativo: ${mib(active.memoryBudgetMib)} · Memória total detectada: ${mib(active.memoryAvailableMib)}`));
    summary.append(node("p", "muted small", "Os buffers e o cache são usados sob demanda. Este orçamento não reserva RAM e não é um teto de memória total do aplicativo: arquivos mapeados, metadados, bibliotecas e sessões simultâneas podem usar memória adicional."));
    summary.append(node("p", "muted small", `DuckDB: ${mib(active.duckdbPerInstanceMib)} por instância · Índice textual: ${mib(active.textIndexMib)} · Cache de seleções: ${mib(active.selectionCacheMib)}`));
    summary.append(node("p", "muted small", `Paralelismo global ativo: ${active.globalParallelism} unidades de trabalho CPU gerenciado. Parsers: até ${active.parserThreads} trabalhadores · SQL: até ${active.queryThreadsPerSession} por sessão · Índice textual: até ${active.textThreads}, sujeitos à admissão compartilhada.`));
    summary.append(node("p", "muted small", "O limite coordena os trabalhos nativos cobertos; não é a contagem total de threads do sistema operacional, da interface ou das bibliotecas. Página e detalhe têm prioridade de admissão; cancelar continua disponível enquanto uma tarefa aguarda."));
    if (active.conservativeBuilder) summary.append(node("p", "muted small", "Preparação em modo conservador: um gravador e lotes menores."));
    if (active.environmentOverrideMib != null) summary.append(node("p", "update-warn", `LOGINSIGHT_MEMORY_LIMIT_MB tem prioridade sobre a preferência salva (${mib(active.environmentOverrideMib)}, sujeito ao teto da máquina). Enquanto essa variável estiver definida, reiniciar mantém a prioridade dela.`));
    if (active.invalidEnvironmentOverride) summary.append(node("p", "update-warn", "LOGINSIGHT_MEMORY_LIMIT_MB é inválido e foi ignorado nesta sessão."));
    if (active.memoryBudgetMib >= status.maximumMemoryMib) summary.append(node("p", "update-warn", "O orçamento ativo usa o total de memória detectada. Outras alocações do aplicativo e do sistema continuam precisando de RAM; pode haver lentidão ou encerramento por falta de memória."));
    if (status.activePreferences.mode === "custom" && status.activePreferences.memoryLimitMib > status.maximumMemoryMib) {
      summary.append(node("p", "update-warn", "A preferência foi criada para uma máquina com mais memória. O valor ativo foi reduzido ao teto desta máquina."));
    }
    if (status.activePreferences.parallelismLimit > status.maximumParallelism) summary.append(node("p", "update-warn", "O paralelismo salvo foi reduzido ao limite desta máquina; a preferência original foi preservada."));
    if (contexts()) pane.append(renderCase());
    pane.append(summary);

    const form = node("form", "ui-pref"); form.noValidate = true;
    form.append(node("strong", "", "Orçamento de memória do aplicativo para o próximo início"));
    const mode = node("select"); mode.id = "resource-memory-mode";
    for (const [value, label] of [["automatic", "Automático (recomendado)"], ["custom", "Personalizado"]]) {
      const option = node("option", "", label); option.value = value; mode.append(option);
    }
    mode.value = draft.mode; mode.disabled = saving;
    mode.onchange = () => { draft.mode = mode.value; notice(""); render(); };
    const memory = node("input"); memory.id = "resource-memory-limit"; memory.type = "number";
    memory.min = String(status.minimumMemoryMib); memory.max = String(status.maximumMemoryMib); memory.step = "1";
    memory.value = draft.memory; memory.disabled = saving || draft.mode !== "custom";
    memory.setAttribute("aria-describedby", "resource-memory-help resource-memory-warning resource-settings-feedback");
    memory.oninput = () => {
      draft.memory = memory.value; notice(""); feedback.className = "muted small";
      feedback.textContent = "Alterações ainda não salvas."; discard.disabled = !changed(); updateMemoryWarning();
    };
    const warning = node("p", "update-warn"); warning.id = "resource-memory-warning"; warning.setAttribute("aria-live", "polite");
    const updateMemoryWarning = () => {
      warning.textContent = draft.mode === "custom" && Number(draft.memory) >= status.maximumMemoryMib
        ? "Usar toda a memória detectada não deixa folga neste orçamento para o sistema e outras alocações. Pode causar lentidão ou encerramento por falta de memória. A escolha é permitida; o valor não reserva RAM nem limita o consumo total do aplicativo."
        : "";
    };
    updateMemoryWarning();
    const help = node("p", "muted small", `Entre ${status.minimumMemoryMib} e ${status.maximumMemoryMib} MiB. O máximo corresponde à memória total detectada, respeitando o limite do contêiner quando houver; não é a RAM livre. Automático mantém folga para o sistema e outras alocações. As mudanças só valem após fechar e abrir o aplicativo; as consultas atuais continuam com os valores desta sessão.`);
    help.id = "resource-memory-help";
    form.append(field("Modo de memória", mode), field("Orçamento personalizado de processamento (MiB)", memory), help, warning);
    const parallelismMode = node("select"); parallelismMode.id = "resource-parallelism-mode"; parallelismMode.disabled = saving;
    for (const [value, label] of [["automatic", "Automático (recomendado)"], ["custom", "Limite personalizado"]]) {
      const option = node("option", "", label); option.value = value; parallelismMode.append(option);
    }
    parallelismMode.value = draft.parallelismMode;
    parallelismMode.onchange = () => { draft.parallelismMode = parallelismMode.value; notice(""); render(); };
    const parallelism = node("input"); parallelism.id = "resource-parallelism-limit"; parallelism.type = "number";
    parallelism.min = "1"; parallelism.max = String(status.maximumParallelism); parallelism.step = "1";
    parallelism.value = draft.parallelism; parallelism.disabled = saving || draft.parallelismMode !== "custom";
    parallelism.setAttribute("aria-describedby", "resource-parallelism-help resource-settings-feedback");
    parallelism.oninput = () => { draft.parallelism = parallelism.value; notice(""); feedback.textContent = "Alterações ainda não salvas."; discard.disabled = !changed(); };
    const parallelismHelp = node("p", "muted small", `Entre 1 e ${status.maximumParallelism} unidades de trabalho simultâneo. O limite é compartilhado por Casos e operações; não se multiplica a cada sessão. Aplica-se no próximo início, sem interromper o trabalho atual.`);
    parallelismHelp.id = "resource-parallelism-help";
    form.append(field("Modo de paralelismo", parallelismMode), field("Limite global de paralelismo", parallelism), parallelismHelp);
    if (status.restartRequired) form.append(node("p", "update-warn", "Há uma preferência salva aguardando o próximo início."));
    const actions = node("div", "modal-actions");
    const discard = button("Descartar alterações não salvas", () => { resetDraft(); notice(""); render(); });
    discard.id = "resource-settings-discard";
    discard.disabled = saving || !changed(); actions.append(discard);
    const submit = button(saving ? "Salvando…" : "Salvar para próximo início", () => {}, "btn primary small");
    submit.id = "resource-settings-save";
    submit.type = "submit"; actions.append(submit);
    form.onsubmit = event => { event.preventDefault(); void save(); };
    form.append(actions, feedback);
    for (const warning of new Set([status.startupWarning, status.savedWarning].filter(Boolean))) form.append(node("p", "update-warn", warning));
    pane.append(form, node("p", "muted small", "Pastas de índices/cache, temporários e limites de disco seguem a configuração existente. A troca de pastas e cotas globais de disco estão fora deste painel."));
    if (focused?.startsWith("resource-") || focused?.startsWith("case-resource-")) {
      const target = document.getElementById(focused);
      (target?.disabled ? feedback : target)?.focus({ preventScroll: true });
    }
  }
  async function renderPane(target) {
    pane = target; render();
    await Promise.all([!status ? load() : Promise.resolve(), syncCase()]);
  }
  for (const event of ["workspace-context-change", "analysis-context-change"]) {
    document.addEventListener?.(event, () => { if (pane && !pane.hidden) void syncCase(); });
  }
  return { renderPane, preferences, parallelismPreference, casePreferences };
})();
