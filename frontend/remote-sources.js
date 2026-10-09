/* Global endpoint vault; Case-owned selection/query drafts and bounded local snapshots. */
(() => {
  "use strict";
  const ui = { connections: [], selected: null, persistentSecrets: null, busy: false, action: null, listVersion: 0, lastImport: null, returnFocus: null, item: null, connectionId: null, epoch: 0, libraryOpen: false, request: null };
  const sessions = new WeakMap();
  let requestSerial = 0;
  const defaults = () => ({ name: "", kind: "elasticsearch", kibanaVersion: "auto", url: "", index: "logs-*", timeField: "@timestamp", username: "", keyPath: "", maxRecords: 100000, query: null, auth: "basic", caPath: "", insecureTls: false });
  // Wazuh data sets: [index, time field, label]. 4.x writes alerts with "timestamp", 5.x data streams with "@timestamp".
  const wazuhIndexer = [["wazuh-alerts-*", "timestamp", "Alertas · Wazuh 4.x"], ["wazuh-archives-*", "timestamp", "Todos os eventos (archives) · Wazuh 4.x"], ["wazuh-findings-v5*", "@timestamp", "Achados · Wazuh 5.x"], ["wazuh-events-v5*", "@timestamp", "Eventos normalizados · Wazuh 5.x"], ["wazuh-states-vulnerabilities*", "vulnerability.detected_at", "Vulnerabilidades · Wazuh 4.8+ e 5.x"]];
  const wazuhApi = [["agents", "", "Agentes · inventário"]];
  const kindLabels = { ssh: "SSH", winrm: "WinRM", kibana: "Kibana", elasticsearch: "Elasticsearch", wazuh: "Wazuh indexer", wazuhapi: "Wazuh API" };
  const modal = el("div", "modal-overlay remote-overlay"); modal.id = "remote-modal"; modal.hidden = true;
  modal.innerHTML = `<section class="modal remote-modal" role="dialog" aria-modal="true" aria-labelledby="rs-title">
    <header class="modal-head"><div><h3 id="rs-title">Conexões</h3><p>Importe registros de Elasticsearch, Kibana e Wazuh ou colete arquivos por SSH e WinRM.</p></div><button class="icon-btn" type="button" id="rs-close" aria-label="Fechar conexões"><i class="fas fa-xmark"></i></button></header>
    <div class="remote-body"><aside class="remote-saved"><div class="remote-list-head"><strong>Biblioteca global de conexões</strong><button class="btn ghost small" id="rs-new" type="button">+ Nova</button></div><p class="remote-help">Compartilhada neste computador. Abra explicitamente para escolher um acesso para este Caso.</p><div id="rs-list" role="listbox" aria-label="Biblioteca global de conexões" hidden></div><button class="text-button" id="rs-reload" type="button">Abrir biblioteca global</button></aside>
      <form id="rs-form" class="remote-form" novalidate><fieldset id="rs-fields"><div class="remote-form-heading"><h4 id="rs-form-title">Nova conexão</h4><span id="rs-saved-state"></span></div>
        <div class="remote-grid"><label class="remote-wide" for="rs-name">Nome<input id="rs-name" autocomplete="off" maxlength="120" placeholder="Ex.: Produção · API" required /></label>
          <label for="rs-kind">Serviço<select id="rs-kind"><option value="elasticsearch">Elasticsearch</option><option value="kibana">Kibana</option><option value="wazuh">Wazuh · indexer (alertas e eventos)</option><option value="wazuhapi">Wazuh · API do servidor (agentes)</option><option value="ssh">SSH · Linux</option><option value="winrm">WinRM · Windows</option></select></label><label for="rs-wazuh-data" id="rs-wazuh-data-wrap" hidden>Dados do Wazuh<select id="rs-wazuh-data"></select></label><label for="rs-kibana-version" id="rs-kibana-version-wrap" hidden>Versão do Kibana<select id="rs-kibana-version"><option value="auto">Automático (Kibana 7, 8 ou 9+)</option><option value="v7_8">Kibana 7.x / 8.x</option><option value="v9">Kibana 9+ / Serverless</option></select></label><label for="rs-index" id="rs-index-wrap">Índice ou padrão<input id="rs-index" autocomplete="off" placeholder="logs-*" required /></label>
          <label class="remote-wide" for="rs-url">URL base<input id="rs-url" type="url" autocomplete="off" placeholder="https://elastic.exemplo:9200" required /><span id="rs-url-hint" class="remote-help"></span></label>
          <label class="remote-wide" for="rs-auth" id="rs-auth-wrap" hidden>Autenticação<select id="rs-auth"><option value="basic">Usuário e senha</option><option value="token">Token (Bearer)</option></select></label>
          <label for="rs-username" id="rs-username-wrap">Usuário <span class="remote-optional" id="rs-username-optional">opcional</span><input id="rs-username" autocomplete="off" spellcheck="false" /></label><label for="rs-password" id="rs-password-wrap"><span id="rs-password-label">Senha</span><input id="rs-password" type="password" autocomplete="new-password" placeholder="Senha de acesso" /><span id="rs-password-hint" class="remote-help"></span></label>
          <div class="remote-wide remote-tls" id="rs-tls"><label for="rs-ca">Certificado CA <span class="remote-optional">opcional · PEM ou DER, como o root-ca.pem do Wazuh</span><div class="remote-actions"><input id="rs-ca" autocomplete="off" spellcheck="false" placeholder="Caminho local do certificado da autoridade" /><button type="button" class="btn ghost small" id="rs-ca-browse">Escolher</button></div></label><label class="remote-check" for="rs-insecure"><input type="checkbox" id="rs-insecure" />Não validar o certificado do servidor (somente em rede confiável)</label></div>
          <label class="remote-wide" id="rs-key-wrap" hidden>Chave privada SSH <span class="remote-optional">opcional com agente configurado</span><div class="remote-actions"><input id="rs-key" autocomplete="off" placeholder="Caminho local da chave privada" /><button type="button" class="btn ghost small" id="rs-key-browse">Escolher</button></div></label>
        </div>
        <section id="rs-files" class="remote-options" hidden><strong>Arquivos e pastas · deste Caso</strong><p class="remote-help">Um caminho absoluto por linha. Pastas incluem seus arquivos e subpastas. A coleta preserva os arquivos para nossa indexação local.</p><div id="rs-presets" class="remote-actions"></div><label for="rs-paths">Caminhos selecionados<textarea id="rs-paths" rows="5" spellcheck="false"></textarea></label><label for="rs-mib">Limite da coleta (MiB)<input id="rs-mib" type="number" min="1" max="4096" value="512" /></label></section>
        <label class="remote-check"><input type="checkbox" id="rs-remember" /><span id="rs-remember-label">Guardar senha com proteção do sistema</span></label><p class="remote-help" id="rs-secret-note"></p>
        <details class="remote-options" id="rs-query-options" open><summary>Recorte da importação · deste Caso</summary><div class="remote-grid"><label for="rs-time-field" id="rs-time-field-wrap">Campo de data/hora<input id="rs-time-field" autocomplete="off" placeholder="Ex.: @timestamp (opcional)" /></label><label for="rs-limit">Máximo de registros<input id="rs-limit" type="number" min="1" max="5000000" step="1" value="100000" /></label><label for="rs-from" id="rs-from-wrap">De <span class="remote-optional">opcional</span><input id="rs-from" type="datetime-local" step="1" /></label><label for="rs-to" id="rs-to-wrap">Até <span class="remote-optional">opcional</span><input id="rs-to" type="datetime-local" step="1" /></label></div><p class="remote-help" id="rs-query-help">Seleção, índice, filtro e período são salvos somente neste Caso. Dados de acesso ficam na biblioteca global. Horários no fuso deste computador. Com campo de data/hora, os registros mais recentes vêm primeiro. Sem esse campo, a consulta usa a ordem interna do índice.</p></details>
        <details class="remote-options" id="rs-query-advanced"><summary>Filtro avançado · Query DSL</summary><label for="rs-query" class="remote-help">Somente a cláusula de consulta, por exemplo <code>{"match":{"service.name":"api"}}</code>.</label><textarea id="rs-query" rows="5" spellcheck="false" placeholder='{"match_all":{}}'></textarea></details>
      </fieldset><div class="remote-actions"><button class="btn ghost" id="rs-delete" type="button" hidden>Excluir da biblioteca global</button><span class="spacer"></span><button class="btn ghost" id="rs-test" type="button">Testar acesso</button><button class="btn ghost" id="rs-save" type="button">Salvar acesso na biblioteca global</button><button class="btn primary" id="rs-import" type="button"><i class="fas fa-cloud-arrow-down"></i> Importar registros</button></div>
      <div class="remote-status" id="rs-status" role="status" aria-live="polite"><span id="rs-status-text">Salvar a conexão guarda suas opções para a próxima consulta.</span><button class="btn ghost small" id="rs-open-result" type="button" hidden>Abrir arquivo importado</button><button class="btn ghost small" id="rs-cancel" type="button" hidden>Cancelar</button></div>
      </form></div></section>`;
  document.body.append(modal);
  // The panel may be docked outside the overlay: look elements up by id.
  const q = id => document.getElementById(`rs-${id}`);
  const setStatus = (text, kind = "neutral") => { q("status-text").textContent = text; q("status").dataset.kind = kind; };
  function setBusy(action = null) {
    ui.action = action; ui.busy = !!action; q("fields").disabled = ui.busy;
    for (const name of ["new", "reload", "delete", "test", "save", "import", "close", "open-result"]) q(name).disabled = ui.busy;
    for (const item of q("list").querySelectorAll("button")) item.disabled = ui.busy;
    q("cancel").hidden = !["import", "test"].includes(action); q("cancel").disabled = !!ui.request?.cancelled || !["import", "test"].includes(action);
    q("form").setAttribute("aria-busy", String(ui.busy));
  }
  const isWazuh = kind => kind === "wazuh" || kind === "wazuhapi";
  const authMode = () => isWazuh(q("kind").value) ? q("auth").value : "basic";
  function credentialReusable() {
    return !!ui.selected?.hasPassword && q("kind").value === ui.selected.kind && q("url").value.trim().replace(/\/$/, "") === ui.selected.url.replace(/\/$/, "") && authMode() === (ui.selected.auth || "basic") && (authMode() === "token" || q("username").value.trim() === (ui.selected.username || ""));
  }
  // The data selector only fills the Case-owned index and time field.
  function syncWazuhData() {
    const kind = q("kind").value, presets = kind === "wazuhapi" ? wazuhApi : wazuhIndexer, select = q("wazuh-data");
    q("wazuh-data-wrap").hidden = !isWazuh(kind);
    if (!isWazuh(kind)) return;
    select.replaceChildren(...presets.map(([index, , label]) => new Option(label, index)), ...(kind === "wazuh" ? [new Option("Outro índice ou padrão", "")] : []));
    select.value = presets.some(([index]) => index === q("index").value.trim()) ? q("index").value.trim() : kind === "wazuh" ? "" : presets[0][0];
  }
  function applyWazuhData(index) {
    const preset = [...wazuhIndexer, ...wazuhApi].find(([value]) => value === index);
    if (!preset) { q("index").focus(); return; }
    q("index").value = preset[0]; q("time-field").value = preset[1];
  }
  function syncHints() {
    const kind = q("kind").value, kibana = kind === "kibana", wazuh = kind === "wazuh", api = kind === "wazuhapi";
    const ssh = kind === "ssh", winrm = kind === "winrm", files = ssh || winrm, token = authMode() === "token";
    q("files").hidden = !files; q("key-wrap").hidden = !ssh; q("password-wrap").hidden = ssh;
    for (const id of ["index-wrap", "query-options", "query-advanced"]) q(id).hidden = files;
    // The server API serves fixed inventories: no index, Query DSL or period.
    for (const id of ["time-field-wrap", "from-wrap", "to-wrap"]) q(id).hidden = api;
    if (api) { q("index-wrap").hidden = true; q("query-advanced").hidden = true; }
    q("query-help").textContent = api ? "Dados e limite são salvos somente neste Caso. Dados de acesso ficam na biblioteca global. A API do servidor não filtra por período." : "Seleção, índice, filtro e período são salvos somente neste Caso. Dados de acesso ficam na biblioteca global. Horários no fuso deste computador. Com campo de data/hora, os registros mais recentes vêm primeiro. Sem esse campo, a consulta usa a ordem interna do índice.";
    q("auth-wrap").hidden = !isWazuh(kind); q("username-wrap").hidden = token; q("tls").hidden = files;
    q("username-optional").hidden = isWazuh(kind);
    q("password-label").textContent = token ? "Token" : "Senha";
    q("remember-label").textContent = token ? "Guardar token com proteção do sistema" : "Guardar senha com proteção do sistema";
    syncWazuhData();
    q("import").innerHTML = `<i class="fas fa-cloud-arrow-down"></i> ${files ? "Coletar e indexar arquivos" : "Importar registros"}`;
    q("url").type = "text";
    q("kibana-version-wrap").hidden = !kibana;
    q("url").placeholder = ssh ? "ssh://servidor:22" : winrm ? "https://servidor:5986/wsman" : kibana ? "https://kibana.exemplo/s/meu-espaco" : wazuh ? "https://wazuh-indexer:9200" : api ? "https://wazuh-servidor:55000" : "https://elastic.exemplo:9200";
    q("url-hint").textContent = ssh ? "OpenSSH por chave ou agente. O host deve estar verificado em known_hosts. Python 3 no Linux remoto." : winrm ? "WinRM com HTTPS e certificado válido. Cliente disponível no Windows. Usuário vazio usa autenticação integrada." : kibana ? "Use a URL base, com /s/espaco se necessário. Requer permissão no Console do Kibana; login SSO do navegador não é reutilizado." : wazuh ? "Wazuh indexer, onde o Wazuh grava alertas e eventos (porta 9200). Use um usuário do indexer com leitura em wazuh-*, como o do painel. Compatível com Wazuh 4.x e 5.x." : api ? "API do servidor Wazuh 4.x ou 5.x (porta 55000). Usuário e senha geram um token JWT renovado automaticamente. Alertas ficam no Wazuh indexer." : "Endereço da API Elasticsearch. Use usuário e senha nos campos próprios, se exigidos.";
    q("presets").replaceChildren();
    if (files) {
      const presets = ssh ? [["Sistema", "/var/log/syslog\n/var/log/messages"], ["Autenticação", "/var/log/auth.log\n/var/log/secure"], ["Nginx", "/var/log/nginx"], ["Apache", "/var/log/apache2\n/var/log/httpd"], ["Auditoria", "/var/log/audit/audit.log"]] : [["EVTX principais", "C:\\Windows\\System32\\winevt\\Logs\\Application.evtx\nC:\\Windows\\System32\\winevt\\Logs\\System.evtx\nC:\\Windows\\System32\\winevt\\Logs\\Security.evtx"], ["Todos os EVTX", "C:\\Windows\\System32\\winevt\\Logs"], ["IIS", "C:\\inetpub\\logs\\LogFiles"]];
      for (const [label, paths] of presets) { const button = el("button", "btn ghost small", `+ ${label}`); button.type = "button"; button.onclick = () => { q("paths").value = [...new Set([...q("paths").value.split(/\r?\n/).filter(Boolean), ...paths.split("\n")])].join("\n"); rememberDraft(); }; q("presets").append(button); }
    }
    q("password").placeholder = credentialReusable() ? (token ? "Em branco mantém o token existente" : "Em branco mantém a senha existente") : token ? "Cole o token de acesso" : "Senha de acesso";
    q("password-hint").textContent = credentialReusable() ? (ui.selected.passwordSaved ? `${token ? "Token protegido" : "Senha protegida"} disponível neste computador.` : `${token ? "Token disponível" : "Senha disponível"} nesta sessão.`) : token ? (api ? "Token JWT da API, que expira em 15 minutos por padrão. Para importações longas, prefira usuário e senha." : "Enviado como Authorization: Bearer.") : "Deixe em branco para acesso sem senha.";
    q("remember").disabled = !ui.persistentSecrets;
    q("secret-note").textContent = ui.persistentSecrets == null ? "Abra a biblioteca global para verificar a proteção de senhas disponível. Senhas nunca entram no Caso ou nas exportações." : ui.persistentSecrets ? "Desmarcado: senha apenas na sessão atual. Marcado: protegida pelo sistema neste computador. Senhas ficam fora dos casos e das exportações." : "Neste sistema, senhas ficam apenas na sessão atual; serão solicitadas ao reabrir o aplicativo.";
  }
  // Persist only the query controls. Never spread a connection/vault response into a Case.
  const draftFields = [["index", "index"], ["time-field", "timeField"], ["limit", "maxRecords"], ["query", "queryText"], ["from", "from"], ["to", "to"], ["paths", "pathsText"], ["mib", "maxMib"]];
  const endpointFields = [["name", "name"], ["kind", "kind"], ["url", "url"], ["username", "username"], ["kibana-version", "kibanaVersion"], ["key", "keyPath"], ["auth", "auth"], ["ca", "caPath"], ["insecure", "insecureTls"]];
  const readField = input => q(input).type === "checkbox" ? q(input).checked : q(input).value;
  const writeField = (input, value) => { if (q(input).type === "checkbox") q(input).checked = !!value; else q(input).value = value; };
  function normalizeDraft(raw) {
    const value = raw && typeof raw === "object" ? raw : {}, draft = value.draft || {};
    const string = (key, fallback = "") => typeof draft[key] === "string" ? draft[key] : fallback;
    return { schemaVersion: 1, connectionId: typeof value.connectionId === "string" && value.connectionId ? value.connectionId : null,
      draft: { index: string("index", "logs-*"), timeField: string("timeField", "@timestamp"), maxRecords: string("maxRecords", "100000"),
        queryText: string("queryText"), from: string("from"), to: string("to"), pathsText: string("pathsText"), maxMib: string("maxMib", "512") } };
  }
  function sessionFor(item) {
    if (!sessions.has(item)) sessions.set(item, { version: 0, endpoint: null, lastImport: null });
    return sessions.get(item);
  }
  function ownerValid(captured) {
    return !!captured?.item && captured.item.kind !== "preserved_case_unavailable" && state.cases.cases.includes(captured.item)
      && (!captured.owner || window.AnalysisContexts.owns(captured.owner));
  }
  const captureImportOwner = () => ({ item: activeCase(), owner: window.AnalysisContexts?.capture() });
  const importOwnerCurrent = captured => ownerValid(captured) && activeCase() === captured.item;
  const panelCurrent = captured => importOwnerCurrent(captured) && ui.item === captured.item && ui.epoch === captured.epoch;
  const capturePanel = () => ({ ...captureImportOwner(), epoch: ui.epoch });
  function persistDraft(item = ui.item, value = null) {
    if (!item || item.kind === "preserved_case_unavailable" || !state.cases.cases.includes(item)) return;
    const draft = value || { connectionId: ui.connectionId, draft: Object.fromEntries(draftFields.map(([input, field]) => [field, q(input).value])) };
    item.workspace ||= {}; item.workspace.remoteSource = normalizeDraft(draft);
    void saveCases();
  }
  function rememberDraft() {
    if (!ui.item || ui.item !== activeCase()) return;
    const session = sessionFor(ui.item); session.version++;
    // Unsaved endpoint edits are in-memory only, tied to the exact Case instance.
    session.endpoint = Object.fromEntries(endpointFields.map(([input, field]) => [field, readField(input)]));
    persistDraft();
  }
  function paintEndpoint(connection, endpoint = connection || defaults()) {
    ui.selected = connection ? { ...connection } : null; ui.connectionId = connection?.id || null;
    for (const [input, field] of endpointFields) writeField(input, endpoint[field] ?? defaults()[field]);
    // A generic recorte follows a Wazuh access to its usual data; an authored index is kept.
    if (isWazuh(q("kind").value) && ["", "logs-*"].includes(q("index").value.trim())) applyWazuhData(q("kind").value === "wazuh" ? "wazuh-alerts-*" : "agents");
    q("password").value = ""; q("remember").checked = ui.persistentSecrets && !!connection?.passwordSaved;
    q("form-title").textContent = connection?.name || "Novo acesso";
    q("saved-state").textContent = connection ? "Acesso da biblioteca global · recorte deste Caso" : "Acesso não salvo · recorte deste Caso";
    q("delete").hidden = !connection; syncHints();
  }
  function renderList() {
    const box = q("list"); box.replaceChildren(); box.hidden = !ui.libraryOpen;
    q("reload").textContent = ui.libraryOpen ? "Atualizar biblioteca global" : "Abrir biblioteca global";
    if (!ui.libraryOpen) return;
    if (!ui.connections.length) box.append(el("p", "remote-list-empty", "Nenhum acesso salvo na biblioteca global."));
    for (const connection of ui.connections) {
      const item = el("button", "remote-connection"); item.type = "button"; item.setAttribute("role", "option"); item.setAttribute("aria-selected", String(ui.selected?.id === connection.id)); item.disabled = ui.busy;
      const head = el("span", "remote-connection-name", connection.name); const kind = el("span", "remote-connection-kind", kindLabels[connection.kind] || connection.kind);
      const detail = el("span", "remote-connection-detail", connection.url); detail.title = detail.textContent;
      item.append(head, kind, detail); item.onclick = () => selectConnection(connection); box.append(item);
    }
  }
  function selectConnection(connection = null) {
    caseChanged(); if (ui.busy || !ui.item || ui.item.kind === "preserved_case_unavailable") return;
    const session = sessionFor(ui.item); session.lastImport = null; session.importRequest = null; ui.lastImport = null; q("open-result").hidden = true;
    paintEndpoint(connection); rememberDraft(); renderList();
    setStatus(connection ? "Acesso selecionado para este Caso. Seu índice, filtro e período foram mantidos." : "Configure um acesso ou abra a biblioteca global. O recorte pertence somente a este Caso.");
  }
  function caseChanged() {
    const item = activeCase(); if (ui.item === item) return;
    ui.item = item; ui.epoch++; ui.listVersion++; ui.request = null; ui.libraryOpen = false; ui.connections = []; ui.persistentSecrets = null;
    setBusy(); paintEndpoint(null);
    const saved = normalizeDraft(item?.workspace?.remoteSource), session = item ? sessionFor(item) : null;
    ui.connectionId = saved.connectionId;
    for (const [input, field] of draftFields) q(input).value = saved.draft[field];
    // The vault is not restored here. Resolution happens only for this Case's saved ID on mount.
    if (!saved.connectionId && session?.endpoint) paintEndpoint(null, session.endpoint);
    ui.lastImport = session?.lastImport || null; q("open-result").hidden = !ui.lastImport;
    renderList(); setStatus(item ? "Recorte deste Caso. Abra a biblioteca global para escolher um acesso." : "Abra um Caso para preparar uma importação.");
  }
  async function refreshConnections({ reveal = false, resolveSelection = false } = {}) {
    caseChanged(); const captured = capturePanel(), version = ++ui.listVersion;
    if (!ownerValid(captured)) return false;
    const draftVersion = sessionFor(captured.item).version, selectedId = normalizeDraft(captured.item.workspace?.remoteSource).connectionId;
    const response = await api("remote_list", {}, { silent: true });
    if (!panelCurrent(captured) || version !== ui.listVersion) return false;
    ui.persistentSecrets = !!response.persistentSecrets;
    if (reveal) ui.libraryOpen = true;
    ui.connections = ui.libraryOpen ? response.connections || [] : [];
    const saved = normalizeDraft(captured.item.workspace?.remoteSource), id = saved.connectionId;
    let missing = false;
    if (id && id === selectedId && draftVersion === sessionFor(captured.item).version) {
      const connection = (response.connections || []).find(item => item.id === id);
      if (!connection) {
        missing = true; paintEndpoint(null); sessionFor(captured.item).endpoint = null;
        persistDraft(captured.item, { ...saved, connectionId: null });
        setStatus("O acesso selecionado não está mais na biblioteca. Escolha outro explicitamente; o recorte deste Caso foi mantido.", "warning");
      } else if (draftVersion === sessionFor(captured.item).version && (resolveSelection || !ui.selected)) {
        paintEndpoint(connection, sessionFor(captured.item).endpoint || connection);
      }
    }
    renderList(); syncHints(); return !missing;
  }
  function readConnection() {
    const connection = {
      ...(ui.selected?.id ? { id: ui.selected.id } : {}),
      name: q("name").value.trim(), kind: q("kind").value, url: q("url").value.trim(), index: q("index").value.trim(),
      kibanaVersion: q("kind").value === "kibana" ? q("kibana-version").value : "auto",
      username: authMode() === "token" ? "" : q("username").value.trim(), timeField: q("time-field").value.trim(), maxRecords: Number(q("limit").value), query: null,
      auth: authMode(), caPath: q("ca").value.trim(), insecureTls: q("insecure").checked,
    };
    if (!connection.name) { q("name").focus(); throw Error("Dê um nome à conexão."); }
    let parsed;
    try { parsed = new URL(connection.url); } catch { q("url").focus(); throw Error("Informe uma URL válida, começando com https:// ou http://."); }
    if (["ssh", "winrm"].includes(connection.kind)) {
      if (parsed.username || parsed.password) throw Error("Use os campos próprios para credenciais.");
      if (connection.kind === "ssh" ? parsed.protocol !== "ssh:" : parsed.protocol !== "https:") throw Error("Use ssh:// para SSH ou https:// para WinRM.");
      connection.paths = q("paths").value.split(/\r?\n/).map(p => p.trim()).filter(Boolean);
      connection.keyPath = connection.kind === "ssh" ? q("key").value.trim() : "";
      connection.maxBytes = Number(q("mib").value) * 1024 * 1024;
      if (!connection.paths.length) throw Error("Escolha locais sugeridos ou informe um caminho por linha.");
      if (!Number.isInteger(connection.maxBytes) || connection.maxBytes < 1048576 || connection.maxBytes > 4294967296) throw Error("Defina um limite entre 1 e 4096 MiB.");
      connection.timeField = ""; connection.index = ""; connection.auth = "basic"; connection.caPath = ""; connection.insecureTls = false;
      return connection;
    }
    if (!["http:", "https:"].includes(parsed.protocol)) throw Error("A URL precisa usar https:// ou http://.");
    if (parsed.username || parsed.password) throw Error("Preencha usuário e senha nos campos próprios, sem incluir credenciais na URL.");
    if (!Number.isInteger(connection.maxRecords) || connection.maxRecords < 1 || connection.maxRecords > 5000000) throw Error("Escolha um limite entre 1 e 5.000.000 registros.");
    if (connection.kind === "wazuhapi") {
      if (connection.auth === "basic" && !connection.username) { q("username").focus(); throw Error("Informe o usuário da API do servidor Wazuh ou escolha autenticação por token."); }
      connection.index = q("wazuh-data").value; connection.timeField = "";
      return connection;
    }
    if (!connection.index) { q("index").focus(); throw Error(`Informe um índice ou padrão, como ${connection.kind === "wazuh" ? "wazuh-alerts-*" : "logs-*"}.`); }
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
  function passwordArgs() { const password = q("kind").value === "ssh" ? "" : q("password").value; return password ? { password } : {}; }
  async function openSnapshot(result, captured, request = null) {
    const opening = { ...captured, epoch: ui.epoch };
    const current = () => panelCurrent(opening) && (!request || !request.cancelled && sessionFor(captured.item).importRequest === request);
    if (!current()) return false;
    setStatus("Preparando os registros para análise…");
    if(window.WorkspaceContext?.scope()==='case')await window.WorkspaceContext.setScope('dataset');
    if (!current()) return false;
    if(window.WorkspaceContext?.scope()==='case') { setStatus("Aguarde a operação atual e abra o arquivo importado na Análise.", "warning"); q("open-result").hidden = false; return false; }
    const loaded = await loadData({ kind: "file", path: result.path, paths: result.paths || [result.path], format: result.format || "jsonl" }, { merge: state.loaded, caseId: captured.item.id });
    if (!current()) return false;
    if (!loaded) {
      q("open-result").hidden = false; setStatus("O arquivo foi importado, mas não pôde ser aberto. Use Abrir arquivo importado para tentar novamente.", "error"); return false;
    }
    const navigation = window.Workspace?.showPage("summary"), navigationEpoch = ui.epoch;
    await navigation;
    if (!importOwnerCurrent(captured) || ui.epoch !== navigationEpoch || request?.cancelled) return false;
    const noun = result.paths ? "arquivos coletados e indexados" : "registros importados";
    setStatus(`${fmtNum(result.count)} ${noun}.${result.limited ? " Limite atingido; ajuste o período ou o máximo para buscar mais." : ""}${result.warning ? ` ${result.warning}` : ""}`, result.limited || result.warning ? "warning" : "success");
    toast(`${fmtNum(result.count)} ${noun}${result.limited ? " · limite atingido" : ""}.${result.warning ? ` ${result.warning}` : ""}`, result.limited || result.warning ? "info" : "ok");
    return true;
  }
  async function act(action) {
    caseChanged(); if (ui.busy || !ui.item || ui.item.kind === "preserved_case_unavailable") return;
    let connection, range;
    try {
      connection = readConnection();
      if (action === "import" && !["ssh", "winrm", "wazuhapi"].includes(connection.kind)) {
        range = readRange();
        if ((range.from || range.to) && !connection.timeField) { q("time-field").focus(); throw Error("Informe o campo de data/hora para usar um período."); }
      }
    } catch (error) { setStatus(error.message, "error"); return; }
    rememberDraft();
    const captured = capturePanel(), session = sessionFor(captured.item), draftVersion = session.version;
    const request = { captured, key: `remote-request:${++requestSerial}`, cancelled: false, action, phase: "query" };
    if (action === "save") ui.listVersion++;
    if (action === "import") { session.importRequest = request; session.lastImport = null; ui.lastImport = null; }
    ui.request = request; setBusy(action); q("open-result").hidden = true;
    const current = () => panelCurrent(captured) && ui.request === request;
    setStatus(action === "save" ? "Salvando acesso na biblioteca global…" : action === "test" ? ({ wazuh: "Verificando o Wazuh indexer, a versão e os índices…", wazuhapi: "Verificando a API do servidor Wazuh…" }[connection.kind] || "Verificando acesso ao índice…") : "Consultando e importando registros…");
    let closeOnSuccess = false;
    const credentials = passwordArgs();
    try {
      if (action === "save") {
        // The global vault stores access, never a Case's authored query/recorte.
        const [index, timeField] = { wazuh: ["wazuh-alerts-*", "timestamp"], wazuhapi: ["agents", ""] }[connection.kind] || ["logs-*", "@timestamp"];
        const libraryConnection = { ...connection, index, timeField, maxRecords: 100000, query: null };
        const saved = await api("remote_save", { connection: libraryConnection, ...credentials, rememberPassword: ui.persistentSecrets && q("remember").checked }, { silent: true });
        if (!ownerValid(captured) || session.version !== draftVersion) return;
        persistDraft(captured.item, { ...normalizeDraft(captured.item.workspace?.remoteSource), connectionId: saved.id });
        session.endpoint = Object.fromEntries(endpointFields.map(([, field]) => [field, saved[field]]));
        // A remounted, unchanged draft must retain its saved ID on the next edit.
        // Reconcile only its internal binding; never repaint a retired panel intent.
        if (activeCase() === captured.item && ui.item === captured.item) { ui.listVersion++; ui.connectionId = saved.id; ui.selected = { ...saved }; }
        if (!current()) return;
        paintEndpoint(saved);
        if (ui.libraryOpen) { ui.connections = ui.connections.filter(item => item.id !== saved.id); ui.connections.push(saved); renderList(); }
        setStatus("Acesso salvo na biblioteca global. Seleção e recorte mantidos somente neste Caso.", "success");
      } else if (action === "test") {
        const result = await api("remote_test", { connection, ...credentials }, { silent: true, latest: request.key });
        if (current()) setStatus(request.cancelled ? "Teste cancelado. Seu recorte foi mantido." : result.message || "Acesso confirmado.", request.cancelled ? "neutral" : "success");
      } else {
        startOperation("remote", "Importando registros", connection.name);
        const result = await api("remote_import", { connection, ...credentials, ...range }, { silent: true, latest: request.key });
        if (request.cancelled || !ownerValid(captured) || session.importRequest !== request) return;
        session.lastImport = { result, captured };
        if (!current()) {
          // The same Case may have been remounted while its query continued.
          // Offer an explicit open only; never revive the old automatic intent.
          if (importOwnerCurrent(captured) && ui.item === captured.item && !ui.request) {
            ui.lastImport = session.lastImport; q("open-result").hidden = !result.count;
            setStatus(result.count ? "O arquivo importado deste Caso está pronto. Use Abrir arquivo importado para analisá-lo." : "Nenhum registro encontrado na consulta anterior deste Caso.");
          }
          return;
        }
        ui.lastImport = session.lastImport;
        if (!result.count) { setStatus("Nenhum registro encontrado nesse recorte. Ajuste o período, o índice ou o filtro."); finishOperation("Consulta concluída", "Nenhum registro encontrado"); return; }
        // The remote query has finished. Its Cancel no longer owns the local
        // source load, whose overlay offers its own task-scoped cancellation.
        request.phase = "opening"; setBusy("open");
        closeOnSuccess = await openSnapshot(result, captured, request);
      }
    } catch (error) {
      if (current()) {
        setStatus(request.cancelled ? "Operação cancelada. Seu recorte foi mantido." : `Não foi possível ${action === "save" ? "salvar" : action === "test" ? "testar o acesso" : "importar"}: ${String(error)}`, request.cancelled ? "neutral" : "error");
        if (action === "import") finishOperation(request.cancelled ? "Importação cancelada" : "Falha na importação remota");
      }
    } finally {
      if (current()) {
        if (request.cancelled) { setStatus("Operação cancelada. Seu recorte foi mantido."); if (action === "import") finishOperation("Importação cancelada"); }
        ui.request = null; setBusy(); syncHints(); if (closeOnSuccess) close();
      }
    }
  }
  // Conexões is a page of the Estrutura area; the panel is docked into it.
  const panel = modal.querySelector(".remote-modal");
  async function open() { await window.Workspace.showPage("connections"); }
  async function mount(host) {
    caseChanged(); panel.classList.add("docked"); panel.removeAttribute("aria-modal"); host.append(panel);
    const captured = capturePanel();
    ui.lastImport = ui.item ? sessionFor(ui.item).lastImport : null; q("open-result").hidden = !ui.lastImport;
    // A previously chosen ID may resolve itself; opening the page never enumerates the vault.
    if (!normalizeDraft(ui.item?.workspace?.remoteSource).connectionId) return;
    try { await refreshConnections({ resolveSelection: true }); }
    catch (error) { if (panelCurrent(captured)) setStatus(`Não foi possível resolver o acesso deste Caso. Abra a biblioteca para tentar novamente. ${String(error)}`, "error"); }
  }
  function unmount() {
    if (!panel.classList.contains("docked")) return;
    ui.epoch++; ui.listVersion++; ui.request = null; ui.libraryOpen = false; ui.connections = []; setBusy(); renderList();
    q("password").value = ""; panel.classList.remove("docked"); panel.setAttribute("aria-modal", "true"); modal.append(panel);
  }
  function close() {
    if (ui.busy || panel.classList.contains("docked")) return;
    q("password").value = ""; modal.hidden = true; ui.returnFocus?.focus?.();
  }
  q("form").onsubmit = event => event.preventDefault();
  q("key-browse").onclick = async () => { const captured = capturePanel(); const path = await dialogApi.open({ multiple: false, directory: false }); if (path && panelCurrent(captured)) { q("key").value = path; rememberDraft(); } };
  q("ca-browse").onclick = async () => { const captured = capturePanel(); const path = await dialogApi.open({ multiple: false, directory: false, filters: [{ name: "Certificado CA", extensions: ["pem", "crt", "cer", "der"] }, { name: "Todos os arquivos", extensions: ["*"] }] }); if (path && panelCurrent(captured)) { q("ca").value = path; rememberDraft(); } };
  // Switching to or from Wazuh replaces only a default or preset recorte, never an authored index.
  q("kind").addEventListener("change", () => {
    const kind = q("kind").value, index = q("index").value.trim();
    if (index && index !== "logs-*" && ![...wazuhIndexer, ...wazuhApi].some(([value]) => value === index)) return;
    if (kind === "wazuh") applyWazuhData(wazuhIndexer.some(([value]) => value === index) ? index : "wazuh-alerts-*");
    else if (kind === "wazuhapi") applyWazuhData(wazuhApi.some(([value]) => value === index) ? index : "agents");
    else if (!["ssh", "winrm"].includes(kind)) { q("index").value = "logs-*"; q("time-field").value = "@timestamp"; }
    syncHints(); rememberDraft();
  });
  q("wazuh-data").addEventListener("change", () => { if (q("wazuh-data").value) applyWazuhData(q("wazuh-data").value); else q("index").focus(); });
  q("index").addEventListener("input", syncWazuhData);
  for (const action of ["save", "test", "import"]) q(action).onclick = () => act(action);
  q("new").onclick = () => { selectConnection(); q("name").focus(); };
  q("close").onclick = close;
  q("reload").onclick = async () => {
    caseChanged(); const captured = capturePanel();
    try { if (await refreshConnections({ reveal: true }) && panelCurrent(captured)) setStatus("Biblioteca global aberta. Selecione explicitamente um acesso para este Caso."); }
    catch (error) { if (panelCurrent(captured)) setStatus(String(error), "error"); }
  };
  q("delete").onclick = async () => {
    caseChanged(); if (!ui.selected || ui.busy) return;
    const captured = capturePanel(), id = ui.selected.id, request = { captured, action: "delete" };
    ui.listVersion++; ui.request = request; setBusy("delete");
    const current = () => panelCurrent(captured) && ui.request === request;
    try {
      await api("remote_delete", { id }, { silent: true });
      if (!ownerValid(captured)) return;
      const saved = normalizeDraft(captured.item.workspace?.remoteSource);
      if (saved.connectionId === id) { persistDraft(captured.item, { ...saved, connectionId: null }); sessionFor(captured.item).endpoint = null; }
      if (activeCase() === captured.item && ui.item === captured.item && ui.connectionId === id) {
        ui.listVersion++; ui.connectionId = null; ui.selected = null;
        q("delete").hidden = true; q("saved-state").textContent = "Acesso removido da biblioteca global · recorte mantido neste Caso";
      }
      if (!current()) return;
      paintEndpoint(null); ui.connections = ui.connections.filter(item => item.id !== id); renderList();
      setStatus("Acesso removido da biblioteca global. Nenhum outro foi selecionado; o recorte e os arquivos importados foram mantidos.");
    } catch (error) { if (current()) setStatus(`Não foi possível remover: ${String(error)}`, "error"); }
    finally { if (current()) { ui.request = null; setBusy(); } }
  };
  q("cancel").onclick = () => {
    const request = ui.request;
    if (!request?.key || !["import", "test"].includes(request.action) || request.phase !== "query" || request.cancelled || !panelCurrent(request.captured)) return;
    request.cancelled = true; q("cancel").disabled = true; setStatus("Cancelando… Aguardando a consulta em andamento encerrar.");
    try { window.Tasks?.cancelLatest(request.key); }
    catch { if (ui.request === request) { request.cancelled = false; q("cancel").disabled = false; setStatus("Não foi possível solicitar o cancelamento. A consulta continua; você pode tentar novamente.", "error"); } }
  };
  q("open-result").onclick = async () => {
    caseChanged(); if (!ui.lastImport || ui.busy) return;
    const captured = capturePanel(), result = ui.lastImport;
    setBusy("open"); let opened = false;
    try { opened = await openSnapshot(result.result, result.captured); }
    finally { if (panelCurrent(captured)) { setBusy(); if (opened) close(); } }
  };
  for (const id of ["url", "username", "kind", "kibana-version", "auth"]) {
    q(id).addEventListener("input", syncHints);
    q(id).addEventListener("change", syncHints);
  }
  for (const event of ["input", "change"]) q("fields").addEventListener(event, () => {
    rememberDraft(); q("saved-state").textContent = "Recorte salvo neste Caso · acesso só é alterado ao salvar na biblioteca";
  });
  modal.addEventListener("click", event => { if (event.target === modal) close(); });
  document.addEventListener("keydown", event => { if (event.key === "Escape" && !modal.hidden) { event.preventDefault(); close(); } });
  window.__TAURI__.event?.listen("operation-progress", ({ payload }) => {
    if (ui.action !== "import" || !ui.request || ui.request.cancelled || !panelCurrent(ui.request.captured) || payload?.operation !== "remoto") return;
    const operationId = window.Tasks?.operationFor(ui.request.key);
    if (!operationId || payload.operationId !== operationId) return;
    const count = Number(payload.completed) || 0, total = Number(payload.total) || 0;
    setStatus(`${payload.phase || "Importando"}${count ? ` · ${fmtNum(count)}${total ? ` de ${fmtNum(total)}` : ""} registros` : ""}…`);
  }).catch(() => {});
  const empty = el("button", "text-button", "Elasticsearch / Kibana / Wazuh"); empty.id = "ws-remote-empty"; empty.type = "button"; empty.onclick = open; $("#ws-windows").after(empty);
  window.RemoteSources = { open, mount, unmount, caseChanged };
  caseChanged();
})();
