/* Verified Case recovery is a fresh destination selected on the next startup.
   The live Case store, source sessions and original profile remain untouched. */
window.CaseEvidenceRecovery = (() => {
  "use strict";
  const uuid = value => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
  const text = value => typeof value === "string" && value.length <= 16384;
  const path = value => text(value) && !!value;
  const fail = () => { throw Error("CASE_RECOVERY_INVALID: Não foi possível confirmar o destino da recuperação."); };
  const keys = (value, allowed) => { if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some(key => !allowed.includes(key))) fail(); };
  function validateStatus(value) {
    keys(value, ["currentProfile", "restartOnly", "startupWarning", "pending", "recoveries", "scanLimited", "returnOriginalPending"]); keys(value.currentProfile, ["id", "path"]);
    if (value.currentProfile.id !== null && !uuid(value.currentProfile.id) || !path(value.currentProfile.path) || value.restartOnly !== true || value.startupWarning !== null && !text(value.startupWarning)
      || typeof value.returnOriginalPending !== "boolean" || typeof value.scanLimited !== "boolean" || !Array.isArray(value.recoveries) || value.recoveries.length > 128) fail();
    if (value.pending !== null) { const pending = value.pending; keys(pending, ["profileId", "recoveryId", "requestId", "destination", "restartRequired"]);
      if (![pending.profileId, pending.recoveryId, pending.requestId].every(uuid) || !path(pending.destination) || pending.restartRequired !== true) fail(); }
    const seen = new Set(); for (const entry of value.recoveries) { keys(entry, ["recoveryId", "createdAtMs", "state"]);
      if (!uuid(entry.recoveryId) || seen.has(entry.recoveryId) || entry.state !== "requires_verification" || entry.createdAtMs !== null && (!Number.isSafeInteger(entry.createdAtMs) || entry.createdAtMs < 0)) fail(); seen.add(entry.recoveryId); }
    return value;
  }
  function validatePrepared(value, prior) {
    keys(value, ["profileId", "destination", "restartRequired", "originalPreserved", "warning"]);
    if (!uuid(value.profileId) || !path(value.destination) || value.restartRequired !== true || value.originalPreserved !== true || value.warning !== null && !text(value.warning)
      || value.profileId === prior.currentProfile.id || value.destination === prior.currentProfile.path) fail(); return value;
  }
  function create({ invoke, requestId = () => crypto.randomUUID() }) {
    let snapshot = null, attempt = null, serial = 0;
    async function status() {
      const request = ++serial, result = validateStatus(await invoke("case_recovery_status", {}));
      if (request !== serial) { if (snapshot) return structuredClone(snapshot); throw Error("A seleção de recuperação mudou durante a leitura."); } snapshot = structuredClone(result); return structuredClone(snapshot);
    }
    async function prepare(recoveryId) {
      if (!snapshot?.recoveries.some(entry => entry.recoveryId === recoveryId)) throw Error("Atualize a lista e escolha uma recuperação disponível.");
      if (attempt && attempt.request.recoveryId !== recoveryId) throw Error("Confirme a preparação pendente antes de escolher outra recuperação.");
      if (!attempt) attempt = { kind: "prepare", request: { recoveryId, requestId: requestId() }, before: structuredClone(snapshot), running: null };
      if (attempt.running) return attempt.running;
      const captured = attempt; ++serial;
      const work = (async () => {
        const result = validatePrepared(await invoke("case_recovery_prepare_restart", { request: structuredClone(captured.request) }), captured.before);
        // Publish a confirmed pending destination only. Nothing rebinds state.cases
        // or the current profile inside the running process.
        ++serial; snapshot = { ...snapshot, returnOriginalPending: false, pending: { profileId: result.profileId, recoveryId: captured.request.recoveryId, requestId: captured.request.requestId, destination: result.destination, restartRequired: true } };
        if (attempt === captured) attempt = null; return structuredClone(result);
      })().finally(() => { captured.running = null; }); captured.running = work; return work;
    }
    async function returnOriginal() {
      if (!snapshot || snapshot.currentProfile.id === null) throw Error("O perfil original já está em uso.");
      if (attempt && attempt.kind !== "return") throw Error("Confirme a preparação pendente antes de escolher outro destino.");
      if (!attempt) attempt = { kind: "return", request: { requestId: requestId() }, before: structuredClone(snapshot), running: null };
      if (attempt.running) return attempt.running;
      const captured = attempt; ++serial;
      const work = (async () => {
        const result = await invoke("case_recovery_return_original", { request: structuredClone(captured.request) });
        keys(result, ["destination", "restartRequired", "originalPreserved", "warning"]);
        if (!path(result.destination) || typeof result.restartRequired !== "boolean" || result.originalPreserved !== true || result.warning !== null && !text(result.warning)) fail();
        ++serial; snapshot = { ...snapshot, pending: null, returnOriginalPending: result.restartRequired }; if (attempt === captured) attempt = null; return structuredClone(result);
      })().finally(() => { captured.running = null; }); captured.running = work; return work;
    }
    return { status, prepare, returnOriginal, snapshot: () => snapshot && structuredClone(snapshot), pendingRequest: () => attempt && structuredClone(attempt.request) };
  }
  let controller, mount = null, generation = 0, chosen = "", prepared = null, returned = null;
  const control = () => controller ||= create({ invoke: (command, args) => api(command, args, { silent: true }) });
  const button = (label, action, primary = false) => { const node = el("button", `btn ${primary ? "primary" : "ghost"} small`, label); node.type = "button"; node.onclick = action; return node; };
  async function renderPane(pane) {
    mount = pane; const token = ++generation; pane.textContent = "Lendo recuperações de Casos disponíveis…";
    try { const status = await control().status(); if (token !== generation || !pane.isConnected || pane.hidden) return; draw(pane, status); }
    catch (error) { if (token === generation && pane.isConnected) { pane.textContent = String(error.message || error); pane.append(button("Tentar novamente", () => renderPane(pane))); } }
  }
  function draw(pane, status, error = null) {
    pane.replaceChildren();
    const intro = el("section", "ui-pref"); intro.append(el("h3", "", "Recuperação de Casos"), el("p", "", "Verifique uma recuperação de Casos e prepare um destino novo. A recuperação inclui a base dos Casos, registros originais, imagens, referências, exclusões e contextos próprios dos Casos."),
      el("p", "muted small", "Preferências globais do aplicativo, conexões remotas e catálogos globais externos não são copiados. O perfil original permanece disponível. A seleção só muda ao fechar e abrir o LogInsight novamente."),
      el("p", "muted small", `Perfil em uso: ${status.currentProfile.path}`)); pane.append(intro);
    if (status.startupWarning) pane.append(el("p", "update-warn", status.startupWarning));
    if (status.returnOriginalPending) { const notice = el("section", "ui-pref"); notice.append(el("h3", "", "Retorno ao perfil original preparado"), el("p", "", returned?.destination || "O perfil original será verificado na próxima abertura."), el("p", "", "Feche e abra o LogInsight novamente para voltar ao perfil original. As cópias original e recuperada permanecem disponíveis.")); if (returned?.warning) notice.append(el("p", "update-warn", returned.warning)); pane.append(notice); }
    if (status.currentProfile.id !== null) {
      const returnAction = button(control().pendingRequest() && !control().pendingRequest().recoveryId ? "Repetir retorno ao original" : "Voltar ao perfil original", async () => {
        const token = generation; returnAction.disabled = true;
        try { returned = await control().returnOriginal(); if (mount === pane && token === generation && pane.isConnected) draw(pane, control().snapshot()); }
        catch (error) { if (mount === pane && token === generation && pane.isConnected) draw(pane, control().snapshot() || status, String(error.message || error)); }
      }); returnAction.disabled = status.returnOriginalPending; pane.append(returnAction);
    }
    if (status.pending) { const pending = el("section", "ui-pref"); pending.append(el("h3", "", "Destino preparado para a próxima abertura"), el("p", "", status.pending.destination), el("p", "", "Feche e abra o LogInsight novamente para verificar e selecionar esse destino. O perfil atual continua em uso até lá.")); if (prepared?.warning) pending.append(el("p", "update-warn", prepared.warning)); pane.append(pending); }
    const settings = el("section", "ui-pref"), label = el("label", "fld"), select = el("select"); label.append(el("span", "", "Recuperação disponível")); select.setAttribute("aria-label", "Recuperação disponível");
    select.append(el("option", "", "Escolha uma recuperação")); select.firstChild.value = "";
    for (const recovery of status.recoveries) { const date = recovery.createdAtMs === null || recovery.createdAtMs > 8640000000000000 ? "Data indisponível" : new Date(recovery.createdAtMs).toLocaleString("pt-BR"); const option = el("option", "", `${date} · ${recovery.recoveryId}`); option.value = recovery.recoveryId; select.append(option); }
    const retry = control().pendingRequest(); if (retry?.recoveryId) chosen = retry.recoveryId;
    if (!status.recoveries.some(entry => entry.recoveryId === chosen)) chosen = ""; select.value = chosen; select.disabled = !!retry || !!status.pending || status.returnOriginalPending; label.append(select); settings.append(label);
    const message = el("p", "muted small", status.recoveries.length ? "A lista indica candidatos. A integridade completa será verificada antes de preparar o destino." : "Nenhuma recuperação de Casos está disponível neste perfil."); settings.append(message);
    if (status.scanLimited) settings.append(el("p", "update-warn", "A listagem foi limitada aos candidatos encontrados no orçamento de leitura; ela não representa todos os arquivos do perfil."));
    if (error) settings.append(el("p", "update-error", `Não foi possível confirmar a preparação: ${error}. O perfil atual continua em uso. Repita a solicitação para verificar o resultado.`));
    const prepare = button(retry?.recoveryId ? "Repetir preparação" : "Verificar e preparar novo destino", async () => {
      const selected = select.value, token = generation; prepare.disabled = select.disabled = true; message.textContent = "Verificando a recuperação e preparando um destino novo…";
      try { prepared = await control().prepare(selected); if (mount === pane && token === generation && pane.isConnected) draw(pane, control().snapshot()); }
      catch (error) { if (mount === pane && token === generation && pane.isConnected) draw(pane, control().snapshot() || status, String(error.message || error)); }
    }, true); prepare.disabled = !chosen || !!status.pending && !retry || status.returnOriginalPending || !!retry && !retry.recoveryId;
    select.onchange = () => { chosen = select.value; prepare.disabled = !chosen; };
    settings.append(prepare, button("Atualizar lista", () => renderPane(pane))); pane.append(settings);
  }
  return { create, renderPane, validateStatus, validatePrepared };
})();
