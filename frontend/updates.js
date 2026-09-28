/* Updates published on GitHub (docs/atualizacoes.md). The backend checks when the app opens and installs only
   after the user agrees; this module shows the dialog, the download and Settings → Atualizações. */
window.Updates = (() => {
  "use strict";
  const call = (command, args) => window.__TAURI__.core.invoke(command, args);
  const KINDS = { nsis: "instalador do Windows (.exe)", msi: "pacote MSI do Windows", appimage: "AppImage", deb: "pacote .deb", rpm: "pacote .rpm", other: "cópia sem instalador" };
  const STARTUP_DELAY = 2500;
  let status = null, notice = null, overlay = null;

  const node = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text != null) n.textContent = text; return n; };
  const megabytes = bytes => `${(bytes / 1048576).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} MB`;
  const moment = iso => { const date = new Date(iso); return Number.isNaN(date.getTime()) ? "" : date.toLocaleString("pt-BR", { dateStyle: "short", timeStyle: "short" }); };
  function button(label, cls, action, icon) {
    const b = node("button", cls);
    b.type = "button";
    if (icon) b.append(node("i", `fas ${icon}`), " ");
    b.append(label);
    b.onclick = async () => { b.disabled = true; try { await action(); } catch (error) { toast(String(error), "err"); } finally { b.disabled = false; } };
    return b;
  }
  const row = (...buttons) => { const r = node("div", "modal-actions update-actions"); r.append(...buttons.filter(Boolean)); return r; };
  const warn = text => node("p", "update-warn", text);

  // Release notes are Markdown written in docs/releases; shown as plain text, headings and bullets only.
  function notes(text) {
    const box = node("div", "update-notes");
    for (const line of String(text).split(/\r?\n/)) {
      const heading = line.match(/^#{1,6}\s+(.*)/), bullet = line.match(/^\s*[-*]\s+(.*)/);
      if (heading) box.append(node("strong", "", heading[1].replace(/\*\*/g, "")));
      else if (bullet) box.append(node("div", "update-bullet", bullet[1].replace(/\*\*|`/g, "")));
      else if (line.trim()) box.append(node("p", "", line.replace(/\*\*|`/g, "")));
    }
    return box;
  }

  function set(next) {
    const previous = status?.phase;
    status = next;
    document.querySelector("#btn-settings")?.classList.toggle("has-update", ["available", "downloading", "ready"].includes(next.phase) && next.available?.version !== next.skippedVersion);
    if (previous === "downloading" && next.phase === "ready") open();
    render();
    const pane = document.querySelector("#settings-pane-updates");
    if (pane && !pane.hidden) renderPane(pane);
  }

  function dialog() {
    if (overlay) return overlay;
    overlay = node("div", "modal-overlay update-overlay");
    overlay.hidden = true;
    const modal = node("div", "modal compact-modal update-modal");
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-labelledby", "update-title");
    const head = node("div", "modal-head");
    head.append(node("h3", "", "Atualização do LogInsight"));
    head.lastChild.id = "update-title";
    const close = node("button", "icon-btn");
    close.type = "button"; close.title = "Fechar"; close.dataset.close = "";
    close.append(node("i", "fas fa-xmark"));
    close.onclick = hide;
    head.append(close);
    modal.append(head, node("div", "modal-body"));
    overlay.append(modal);
    overlay.addEventListener("click", event => { if (event.target === overlay) hide(); });
    document.body.append(overlay);
    return overlay;
  }
  function hide() { if (overlay && status?.phase !== "installing") overlay.hidden = true; }
  function open() { dialog().hidden = false; render(); }

  async function download() { set(await call("update_download")); }
  async function skip() {
    const version = status.available.version;
    set(await call("update_skip", { version }));
    hide();
    toast(`A versão ${version} não será mais oferecida ao abrir. Ela continua disponível em Configurações → Atualizações.`, "info");
  }
  async function install() {
    // Pending case edits are written before the app closes.
    try {
      if (typeof caseSaveTimer !== "undefined" && caseSaveTimer && typeof saveCases === "function") await saveCases();
      else if (typeof casesSaveQueue !== "undefined") await casesSaveQueue.catch(() => {});
    } catch { /* the save error was already shown */ }
    await call("update_install");
  }
  async function installOnClose() {
    set(await call("update_install_on_close", { enabled: !status.installOnClose }));
    if (status.installOnClose) { hide(); toast("A atualização será instalada quando você fechar o LogInsight.", "ok"); }
  }
  const openPage = () => call("update_open_page", { version: status?.available?.version || null });

  function render() {
    if (!overlay || overlay.hidden || !status) return;
    const s = status, next = s.available, body = overlay.querySelector(".modal-body");
    overlay.querySelector("[data-close]").hidden = s.phase === "installing";
    body.replaceChildren();
    if (s.phase === "checking") { body.append(node("p", "muted", "Verificando se há uma versão nova…")); return; }
    if (!next || s.phase === "idle") {
      body.append(node("p", "", s.unavailable || s.lastCheck?.message || "Você está usando a versão mais recente."));
      body.append(row(button("Fechar", "btn primary", hide)));
      return;
    }
    const lead = node("p", "update-lead");
    if (s.phase === "installing") {
      lead.textContent = `Instalando o LogInsight ${next.version}…`;
      body.append(lead, node("p", "muted", s.needsAdmin ? "Confirme a senha de administrador quando o sistema pedir. O LogInsight será aberto novamente." : "O LogInsight será fechado e aberto novamente em seguida."));
      return;
    }
    if (s.phase === "downloading") {
      lead.textContent = `Baixando o LogInsight ${next.version}…`;
      const bar = node("div", "update-progress"), fill = node("span");
      const percent = s.total ? Math.min(100, (s.downloaded / s.total) * 100) : null;
      if (percent == null) bar.classList.add("indeterminate"); else fill.style.width = `${percent.toFixed(1)}%`;
      bar.append(fill);
      body.append(lead, bar, node("p", "muted small", `${megabytes(s.downloaded)}${s.total ? ` de ${megabytes(s.total)}` : ""}. Você pode continuar trabalhando; avisaremos quando estiver pronto.`));
      body.append(row(button("Cancelar download", "btn ghost", async () => set(await call("update_cancel"))), button("Continuar trabalhando", "btn primary", hide)));
      return;
    }
    if (s.phase === "ready") {
      lead.textContent = `O LogInsight ${next.version} está pronto para instalar.`;
      body.append(lead, node("p", "", "O LogInsight será fechado, a nova versão será instalada e ele abrirá de novo. Os casos são salvos antes."));
      if (window.Tasks?.running?.()) body.append(warn("Há tarefas em andamento; elas serão interrompidas ao reiniciar."));
      if (s.needsAdmin) body.append(warn("Durante a instalação, o sistema vai pedir a senha de administrador."));
      if (s.installOnClose) body.append(node("p", "muted small", "Programada para quando você fechar o LogInsight."));
      if (s.blocker) body.append(warn(s.blocker));
      if (s.error) body.append(node("p", "update-error", s.error));
      body.append(row(
        s.blocker ? button("Baixar manualmente", "btn ghost", openPage, "fa-arrow-up-right-from-square") : null,
        button(s.installOnClose ? "Não instalar ao fechar" : "Instalar ao fechar", "btn ghost", installOnClose),
        button("Reiniciar e instalar", "btn primary", install, "fa-rotate"),
      ));
      if (s.blocker) body.querySelector(".update-actions .btn.primary").disabled = true;
      return;
    }
    // Available
    lead.textContent = `O LogInsight ${next.version} está disponível. Você usa a versão ${s.currentVersion}.`;
    body.append(lead);
    if (next.date && moment(next.date)) body.append(node("p", "muted small", `Publicada em ${moment(next.date)}.`));
    if (next.notes) body.append(notes(next.notes));
    if (s.needsAdmin) body.append(warn("Durante a instalação, o sistema vai pedir a senha de administrador."));
    if (s.blocker) body.append(warn(s.blocker));
    if (s.error) body.append(node("p", "update-error", s.error));
    body.append(row(
      button("Pular esta versão", "btn ghost", skip),
      button("Mais tarde", "btn ghost", hide),
      s.blocker ? button("Baixar manualmente", "btn primary", openPage, "fa-arrow-up-right-from-square") : button("Atualizar agora", "btn primary", download, "fa-download"),
    ));
  }

  function renderPane(pane) {
    pane.replaceChildren();
    if (!status) { pane.append(node("p", "muted small", "Carregando…")); return; }
    const s = status, next = s.available;
    const version = node("div", "ui-pref");
    const head = node("div", "ui-pref-head");
    head.append(node("strong", "", `LogInsight ${s.currentVersion}`), node("span", "", KINDS[s.installKind] || s.installKind));
    version.append(head);
    if (s.unavailable) version.append(node("p", "muted small", s.unavailable));
    else if (s.phase === "checking") version.append(node("p", "muted small", "Verificando…"));
    else if (s.lastCheck) version.append(node("p", s.lastCheck.outcome === "error" ? "update-error" : "muted small", `${s.lastCheck.message} ${moment(s.lastCheck.at) ? `(${moment(s.lastCheck.at)})` : ""}`.trim()));
    if (notice) version.append(node("p", notice.kind === "failed" ? "update-error" : "muted small", notice.kind === "failed"
      ? `A atualização para ${notice.to} não foi concluída. Tente de novo ou baixe a versão manualmente.`
      : `Atualizado de ${notice.from || "uma versão anterior"} para ${notice.to}.${notice.backup ? ` Cópia dos dados anteriores: ${notice.backup}` : ""}`));
    const actions = node("div", "update-pane-actions");
    if (next && ["available", "downloading", "ready"].includes(s.phase)) actions.append(button(`Ver versão ${next.version}`, "btn primary small", async () => open(), "fa-circle-arrow-up"));
    if (!s.unavailable) actions.append(button("Verificar agora", "btn ghost small", async () => { set(await call("update_check")); if (status.phase === "available") open(); }, "fa-arrows-rotate"));
    actions.append(button("Versões publicadas", "btn ghost small", () => call("update_open_page", { version: null }), "fa-arrow-up-right-from-square"));
    version.append(actions);

    const start = node("div", "ui-pref");
    start.append(node("div", "ui-pref-head"));
    start.firstChild.append(node("strong", "", "Ao abrir o LogInsight"));
    const label = node("label", "update-toggle"), box = node("input");
    box.type = "checkbox"; box.checked = s.checkOnStart; box.disabled = Boolean(s.unavailable);
    box.onchange = async () => { box.disabled = true; try { set(await call("update_set_check_on_start", { enabled: box.checked })); } catch (error) { toast(String(error), "err"); } };
    label.append(box, " Verificar se há uma versão nova");
    start.append(label, node("p", "muted small", "A verificação consulta as versões publicadas no GitHub. Nada é baixado ou instalado sem a sua confirmação."));
    if (s.skippedVersion) {
      const skipped = node("p", "muted small", `A versão ${s.skippedVersion} foi pulada. `);
      const undo = node("a", "", "Voltar a oferecer");
      undo.href = "#";
      undo.onclick = async event => { event.preventDefault(); set(await call("update_skip", { version: null })); };
      skipped.append(undo);
      start.append(skipped);
    }
    pane.append(version, start);
  }

  async function startup() {
    try { set(await call("update_startup")); } catch { return; }
    notice = status.notice;
    if (notice?.kind === "updated") toast(`LogInsight atualizado para a versão ${notice.to}.`, "ok");
    if (notice?.kind === "failed") toast(`A atualização para ${notice.to} não foi concluída. Veja Configurações → Atualizações.`, "err");
    if (status.phase === "available" && status.available && status.available.version !== status.skippedVersion) open();
  }

  window.__TAURI__?.event?.listen("update-state", ({ payload }) => set(payload)).catch(() => {});
  if (window.__TAURI__?.core) setTimeout(startup, STARTUP_DELAY);
  return {
    open: async () => { if (!status) set(await call("update_status")); open(); },
    check: async () => { open(); set(await call("update_check")); },
    renderPane: async pane => { if (!status) { try { set(await call("update_status")); } catch { /* shown as loading */ } } renderPane(pane); },
  };
})();
