/* Saved remote endpoints and explicit, bounded local snapshots. Secrets never enter case state. */
(() => {
  "use strict";
  const ui = { connections: [], selected: null, persistentSecrets: false, busy: false, action: null, cancelled: false, listVersion: 0, lastImport: null, returnFocus: null };
  const defaults = () => ({ name: "", kind: "elasticsearch", url: "", index: "logs-*", timeField: "@timestamp", username: "", maxRecords: 100000, query: null });
  const modal = el("div", "modal-overlay remote-overlay"); modal.id = "remote-modal"; modal.hidden = true;
  modal.innerHTML = `<section class="modal remote-modal" role="dialog" aria-modal="true" aria-labelledby="rs-title">
    <header class="modal-head"><div><h3 id="rs-title">Conexões</h3><p>Consulte Elasticsearch ou Kibana e analise uma cópia local dos registros.</p></div><button class="icon-btn" type="button" id="rs-close" aria-label="Fechar conexões"><i class="fas fa-xmark"></i></button></header>
    <div class="remote-body"><aside class="remote-saved"><div class="remote-list-head"><strong>Salvas neste computador</strong><button class="btn ghost small" id="rs-new" type="button">+ Nova</button></div><div id="rs-list" role="listbox" aria-label="Conexões salvas"></div><button class="text-button" id="rs-reload" type="button">Atualizar lista</button></aside>
      <form id="rs-form" class="remote-form" novalidate><fieldset id="rs-fields"><div class="remote-form-heading"><h4 id="rs-form-title">Nova conexão</h4><span id="rs-saved-state"></span></div>
        <div class="remote-grid"><label class="remote-wide" for="rs-name">Nome<input id="rs-name" autocomplete="off" maxlength="120" placeholder="Ex.: Produção · API" required /></label>
          <label for="rs-kind">Serviço<select id="rs-kind"><option value="elasticsearch">Elasticsearch</option><option value="kibana">Kibana</option></select></label><label for="rs-index">Índice ou padrão<input id="rs-index" autocomplete="off" placeholder="logs-*" required /></label>
          <label class="remote-wide" for="rs-url">URL base<input id="rs-url" type="url" autocomplete="off" placeholder="https://elastic.exemplo:9200" required /><span id="rs-url-hint" class="remote-help"></span></label>
          <label for="rs-username">Usuário <span class="remote-optional">opcional</span><input id="rs-username" autocomplete="off" spellcheck="false" /></label><label for="rs-password">Senha<input id="rs-password" type="password" autocomplete="new-password" placeholder="Senha de acesso" /><span id="rs-password-hint" class="remote-help"></span></label>
        </div>
        <label class="remote-check"><input type="checkbox" id="rs-remember" />Guardar senha com proteção do sistema</label><p class="remote-help" id="rs-secret-note"></p>
        <details class="remote-options" open><summary>Recorte da importação</summary><div class="remote-grid"><label for="rs-time-field">Campo de data/hora<input id="rs-time-field" autocomplete="off" placeholder="Ex.: @timestamp (opcional)" /></label><label for="rs-limit">Máximo de registros<input id="rs-limit" type="number" min="1" max="5000000" step="1" value="100000" /></label><label for="rs-from">De <span class="remote-optional">opcional</span><input id="rs-from" type="datetime-local" step="1" /></label><label for="rs-to">Até <span class="remote-optional">opcional</span><input id="rs-to" type="datetime-local" step="1" /></label></div><p class="remote-help">Horários no fuso deste computador. Com campo de data/hora, os registros mais recentes vêm primeiro. Sem esse campo, a consulta usa a ordem interna do índice.</p></details>
        <details class="remote-options"><summary>Filtro avançado · Query DSL</summary><label for="rs-query" class="remote-help">Somente a cláusula de consulta, por exemplo <code>{"match":{"service.name":"api"}}</code>.</label><textarea id="rs-query" rows="5" spellcheck="false" placeholder='{"match_all":{}}'></textarea></details>
      </fieldset><div class="remote-actions"><button class="btn ghost" id="rs-delete" type="button" hidden>Excluir conexão</button><span class="spacer"></span><button class="btn ghost" id="rs-test" type="button">Testar acesso</button><button class="btn ghost" id="rs-save" type="button">Salvar conexão</button><button class="btn primary" id="rs-import" type="button"><i class="fas fa-cloud-arrow-down"></i> Importar registros</button></div>
      <div class="remote-status" id="rs-status" role="status" aria-live="polite"><span id="rs-status-text">Salvar a conexão guarda suas opções para a próxima consulta.</span><button class="btn ghost small" id="rs-open-result" type="button" hidden>Abrir arquivo importado</button><button class="btn ghost small" id="rs-cancel" type="button" hidden>Cancelar</button></div>
      </form></div></section>`;
  document.body.append(modal);
  const q = id => modal.querySelector(`#rs-${id}`);
  const setStatus = (text, kind = "neutral") => { q("status-text").textContent = text; q("status").dataset.kind = kind; };
  function setBusy(action = null) {
    ui.action = action; ui.busy = !!action; q("fields").disabled = ui.busy;
    for (const name of ["new", "reload", "delete", "test", "save", "import", "close", "open-result"]) q(name).disabled = ui.busy;
    for (const item of q("list").querySelectorAll("button")) item.disabled = ui.busy;
    q("cancel").hidden = !["import", "test"].includes(action); q("cancel").disabled = ui.cancelled;
    q("form").setAttribute("aria-busy", String(ui.busy));
  }
  function credentialReusable() {
    return !!ui.selected?.hasPassword && q("kind").value === ui.selected.kind && q("url").value.trim().replace(/\/$/, "") === ui.selected.url.replace(/\/$/, "") && q("username").value.trim() === (ui.selected.username || "");
  }
  function syncHints() {
    const kibana = q("kind").value === "kibana";
    q("url").placeholder = kibana ? "https://kibana.exemplo/s/meu-espaco" : "https://elastic.exemplo:9200";
    q("url-hint").textContent = kibana ? "Use a URL base, com /s/espaco se necessário. Requer permissão no Console do Kibana; login SSO do navegador não é reutilizado." : "Endereço da API Elasticsearch. Use usuário e senha nos campos próprios, se exigidos.";
    q("password").placeholder = credentialReusable() ? "Em branco mantém a senha existente" : "Senha de acesso";
    q("password-hint").textContent = credentialReusable() ? (ui.selected.passwordSaved ? "Senha protegida disponível neste computador." : "Senha disponível nesta sessão.") : "Deixe em branco para acesso sem senha.";
    q("remember").disabled = !ui.persistentSecrets;
    q("secret-note").textContent = ui.persistentSecrets ? "Desmarcado: senha apenas na sessão atual. Marcado: protegida pelo sistema neste computador. Senhas ficam fora dos casos e das exportações." : "Neste sistema, senhas ficam apenas na sessão atual; serão solicitadas ao reabrir o aplicativo.";
  }
  function renderList() {
    const box = q("list"); box.replaceChildren();
    if (!ui.connections.length) box.append(el("p", "remote-list-empty", "Nenhuma conexão salva. Configure o acesso ao lado e use Salvar conexão para reutilizá-lo."));
    for (const connection of ui.connections) {
      const item = el("button", "remote-connection"); item.type = "button"; item.setAttribute("role", "option"); item.setAttribute("aria-selected", String(ui.selected?.id === connection.id)); item.disabled = ui.busy;
      const head = el("span", "remote-connection-name", connection.name); const kind = el("span", "remote-connection-kind", connection.kind === "kibana" ? "Kibana" : "Elasticsearch");
      const detail = el("span", "remote-connection-detail", `${connection.index} · ${connection.url}`); detail.title = detail.textContent;
      item.append(head, kind, detail); item.onclick = () => selectConnection(connection); box.append(item);
    }
  }
  function selectConnection(connection = null) {
    if (ui.busy) return;
    ui.selected = connection ? { ...connection } : null; ui.lastImport = null; q("open-result").hidden = true;
    const value = connection || defaults();
    for (const [input, field] of [["name", "name"], ["kind", "kind"], ["url", "url"], ["index", "index"], ["username", "username"], ["time-field", "timeField"], ["limit", "maxRecords"]]) q(input).value = value[field] ?? "";
    q("query").value = value.query ? JSON.stringify(value.query, null, 2) : "";
    q("from").value = ""; q("to").value = ""; q("password").value = "";
    q("remember").checked = ui.persistentSecrets && !!connection?.passwordSaved;
    q("form-title").textContent = connection ? connection.name : "Nova conexão";
    q("saved-state").textContent = connection ? "Configuração salva" : "Ainda não salva";
    q("delete").hidden = !connection;
    syncHints(); renderList(); setStatus(connection ? "Edite as opções, teste o acesso ou importe um novo recorte." : "Salvar a conexão guarda suas opções para a próxima consulta.");
  }
  async function refreshConnections(preserve = true) {
    const version = ++ui.listVersion;
    const response = await api("remote_list", {}, { silent: true });
    if (version !== ui.listVersion) return;
    ui.connections = response.connections || []; ui.persistentSecrets = !!response.persistentSecrets;
    const current = ui.selected && ui.connections.find(item => item.id === ui.selected.id);
    if (!preserve || (ui.selected && !current)) selectConnection(current || null);
    else { if (current) ui.selected = { ...current }; renderList(); syncHints(); }
  }
  function readConnection() {
    const connection = {
      ...(ui.selected?.id ? { id: ui.selected.id } : {}),
      name: q("name").value.trim(), kind: q("kind").value, url: q("url").value.trim(), index: q("index").value.trim(),
      username: q("username").value.trim(), timeField: q("time-field").value.trim(), maxRecords: Number(q("limit").value), query: null,
    };
    if (!connection.name) { q("name").focus(); throw Error("Dê um nome à conexão."); }
    let parsed;
    try { parsed = new URL(connection.url); } catch { q("url").focus(); throw Error("Informe uma URL válida, começando com https:// ou http://."); }
    if (!["http:", "https:"].includes(parsed.protocol)) throw Error("A URL precisa usar https:// ou http://.");
    if (parsed.username || parsed.password) throw Error("Preencha usuário e senha nos campos próprios, sem incluir credenciais na URL.");
    if (!connection.index) { q("index").focus(); throw Error("Informe um índice ou padrão, como logs-*."); }
    if (!Number.isInteger(connection.maxRecords) || connection.maxRecords < 1 || connection.maxRecords > 5000000) throw Error("Escolha um limite entre 1 e 5.000.000 registros.");
    const query = q("query").value.trim();
    if (query) {
      try { connection.query = JSON.parse(query); } catch { q("query").parentElement.open = true; q("query").focus(); throw Error("O filtro avançado precisa ser um JSON válido."); }
      if (!connection.query || typeof connection.query !== "object" || Array.isArray(connection.query)) throw Error("Informe um objeto Query DSL, como {\"match_all\":{}}.");
      if (Object.hasOwn(connection.query, "query")) throw Error("Use apenas a cláusula Query DSL, sem envolver em \"query\".");
    }
    return connection;
  }
  function readRange() {
    const from = q("from").value ? new Date(q("from").value) : null, to = q("to").value ? new Date(q("to").value) : null;
    if ((from && !Number.isFinite(from.getTime())) || (to && !Number.isFinite(to.getTime()))) throw Error("Confira o período informado.");
    if (from && to && from > to) throw Error("O início do período deve ser anterior ao fim.");
    return { ...(from ? { from: from.toISOString() } : {}), ...(to ? { to: to.toISOString() } : {}) };
  }
  function passwordArgs() { const password = q("password").value; return password ? { password } : {}; }
  async function openSnapshot(result) {
    setStatus("Preparando os registros para análise…");
    if(window.WorkspaceContext?.scope()==='case')await window.WorkspaceContext.setScope('dataset');
    const loaded = await loadData({ kind: "file", path: result.path, paths: [result.path], format: "jsonl" });
    if (!loaded) {
      q("open-result").hidden = false; setStatus("O arquivo foi importado, mas não pôde ser aberto. Use Abrir arquivo importado para tentar novamente.", "error"); return false;
    }
    await window.Workspace?.showPage("summary");
    setStatus(`${fmtNum(result.count)} registros importados.${result.limited ? " Limite atingido; ajuste o período ou o máximo para buscar mais." : ""}${result.warning ? ` ${result.warning}` : ""}`, result.limited || result.warning ? "warning" : "success");
    toast(`${fmtNum(result.count)} registros importados${result.limited ? " · limite atingido" : ""}.${result.warning ? ` ${result.warning}` : ""}`, result.limited || result.warning ? "info" : "ok");
    return true;
  }
  async function act(action) {
    if (ui.busy) return;
    let connection, range;
    try {
      connection = readConnection();
      if (action === "import") {
        range = readRange();
        if ((range.from || range.to) && !connection.timeField) { q("time-field").focus(); throw Error("Informe o campo de data/hora para usar um período."); }
      }
    }
    catch (error) { setStatus(error.message, "error"); return; }
    ui.cancelled = false; setBusy(action); q("open-result").hidden = true;
    setStatus(action === "save" ? "Salvando conexão…" : action === "test" ? "Verificando acesso ao índice…" : "Consultando e importando registros…");
    let closeOnSuccess = false;
    try {
      if (action === "save") {
        const saved = await api("remote_save", { connection, ...passwordArgs(), rememberPassword: ui.persistentSecrets && q("remember").checked }, { silent: true });
        ui.selected = saved; q("password").value = "";
        await refreshConnections(); q("form-title").textContent = saved.name; q("saved-state").textContent = "Configuração salva"; q("delete").hidden = false;
        syncHints(); setStatus("Conexão salva. Suas opções estarão disponíveis na próxima consulta.", "success");
      } else if (action === "test") {
        const result = await api("remote_test", { connection, ...passwordArgs() }, { silent: true });
        setStatus(ui.cancelled ? "Teste cancelado. Sua configuração foi mantida." : result.message || "Acesso confirmado. A conexão está pronta para importar.", ui.cancelled ? "neutral" : "success");
      } else {
        startOperation("remote", "Importando registros", connection.name);
        const result = await api("remote_import", { connection, ...passwordArgs(), ...range }, { silent: true });
        if (ui.cancelled) { setStatus("Importação cancelada. Sua configuração foi mantida."); finishOperation("Importação cancelada"); return; }
        ui.lastImport = result;
        if (!result.count) { setStatus("Nenhum registro encontrado nesse recorte. Ajuste o período, o índice ou o filtro.", "neutral"); finishOperation("Consulta concluída", "Nenhum registro encontrado"); return; }
        closeOnSuccess = await openSnapshot(result);
      }
    } catch (error) {
      setStatus(ui.cancelled ? "Operação cancelada. Sua configuração foi mantida." : `Não foi possível ${action === "save" ? "salvar" : action === "test" ? "testar o acesso" : "importar"}: ${String(error)}`, ui.cancelled ? "neutral" : "error");
      if (action === "import") finishOperation(ui.cancelled ? "Importação cancelada" : "Falha na importação remota");
    } finally {
      setBusy(); syncHints();
      if (closeOnSuccess) close();
    }
  }
  async function open() {
    if(window.WorkspaceContext?.scope()==='case')await window.WorkspaceContext.setScope('dataset');
    ui.returnFocus = document.activeElement; modal.hidden = false;
    q("name").focus();
    try { await refreshConnections(); }
    catch (error) { setStatus(`Não foi possível carregar as conexões. Use Atualizar lista para tentar novamente. ${String(error)}`, "error"); }
  }
  function close() {
    if (ui.busy) return;
    q("password").value = ""; modal.hidden = true; ui.returnFocus?.focus?.();
  }
  q("form").onsubmit = event => event.preventDefault();
  for (const action of ["save", "test", "import"]) q(action).onclick = () => act(action);
  q("new").onclick = () => { selectConnection(); q("name").focus(); };
  q("close").onclick = close;
  q("reload").onclick = async () => { try { await refreshConnections(); setStatus("Lista de conexões atualizada."); } catch (error) { setStatus(String(error), "error"); } };
  q("delete").onclick = async () => {
    if (!ui.selected || ui.busy) return;
    setBusy("delete");
    try { await api("remote_delete", { id: ui.selected.id }, { silent: true }); ui.selected = null; await refreshConnections(); setBusy(); selectConnection(); setStatus("Conexão removida. Os arquivos já importados continuam disponíveis."); }
    catch (error) { setStatus(`Não foi possível remover: ${String(error)}`, "error"); }
    finally { setBusy(); }
  };
  q("cancel").onclick = async () => {
    if (!ui.busy || ui.cancelled) return;
    ui.cancelled = true; q("cancel").disabled = true; setStatus("Cancelando… Aguardando a consulta em andamento encerrar.");
    try { await api("cancel_operation", {}, { silent: true }); }
    catch { ui.cancelled = false; q("cancel").disabled = false; setStatus("Não foi possível solicitar o cancelamento. A consulta continua; você pode tentar novamente.", "error"); }
  };
  q("open-result").onclick = async () => {
    if (!ui.lastImport || ui.busy) return;
    setBusy("open"); let opened = false;
    try { opened = await openSnapshot(ui.lastImport); } finally { setBusy(); if (opened) close(); }
  };
  for (const id of ["url", "username", "kind"]) q(id).addEventListener("input", syncHints);
  q("fields").addEventListener("input", () => { q("saved-state").textContent = ui.selected ? "Alterações ainda não salvas" : "Ainda não salva"; });
  modal.addEventListener("click", event => { if (event.target === modal) close(); });
  document.addEventListener("keydown", event => { if (event.key === "Escape" && !modal.hidden) { event.preventDefault(); close(); } });
  window.__TAURI__.event?.listen("operation-progress", ({ payload }) => {
    if (ui.action !== "import" || ui.cancelled || payload?.operation !== "remoto") return;
    const count = Number(payload.completed) || 0, total = Number(payload.total) || 0;
    setStatus(`${payload.phase || "Importando"}${count ? ` · ${fmtNum(count)}${total ? ` de ${fmtNum(total)}` : ""} registros` : ""}…`);
  }).catch(() => {});
  const nav = el("button"); nav.id = "ws-remote"; nav.type = "button"; nav.innerHTML = '<i class="fas fa-plug"></i>Conexões'; nav.onclick = open;
  $("#ws-export").before(nav);
  const empty = el("button", "text-button", "Elasticsearch / Kibana"); empty.id = "ws-remote-empty"; empty.type = "button"; empty.onclick = open; $("#ws-windows").after(empty);
  const pageButton = el("button", "btn ghost small", "Conexões"); pageButton.id="ws-remote-page";pageButton.type = "button"; pageButton.onclick = open; $(".page-actions").prepend(pageButton);
  const updatePageButton = () => { pageButton.hidden = document.body.dataset.page !== "sources"||window.WorkspaceContext?.scope()==='case'; };
  document.addEventListener('workspace-context-change',updatePageButton);
  new MutationObserver(updatePageButton).observe(document.body, { attributes: true, attributeFilter: ["data-page"] }); updatePageButton();
  selectConnection();
  window.RemoteSources = { open };
})();
