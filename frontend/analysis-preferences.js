/* Analysis preferences of this computer: light mode for basic PCs, automatic compromise
   calculation after an import, and the offline IP location base used by the analyses. */
window.AnalysisPreferences = (() => {
  "use strict";
  const KEY = "loginsight.preferences.v1";
  const DEFAULTS = { lowPower: "auto", autoCompromises: false, autoCompromisesLimit: 2_000_000 };
  const LIMITS = [250_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000];
  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
  const html = value => String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const number = value => Number(value).toLocaleString("pt-BR");

  function read() {
    let saved = {};
    try { saved = JSON.parse(localStorage.getItem(KEY) || "{}") || {}; } catch { saved = {}; }
    return {
      lowPower: ["auto", "on", "off"].includes(saved.lowPower) ? saved.lowPower : DEFAULTS.lowPower,
      autoCompromises: saved.autoCompromises === true,
      autoCompromisesLimit: LIMITS.includes(saved.autoCompromisesLimit) ? saved.autoCompromisesLimit : DEFAULTS.autoCompromisesLimit,
    };
  }
  let preferences = read();
  function write(next) {
    preferences = { ...preferences, ...next };
    try { localStorage.setItem(KEY, JSON.stringify(preferences)); } catch { /* The choice still applies to this session. */ }
    applyLowPower();
  }

  // ------------------------------------------------------------ light mode
  /** A basic PC: few processor threads or little memory reported by the browser engine. */
  function weakMachine() {
    const threads = Number(navigator.hardwareConcurrency) || 0;
    const memory = Number(navigator.deviceMemory) || 0;
    return (threads > 0 && threads <= 4) || (memory > 0 && memory <= 4);
  }
  const lowPower = () => preferences.lowPower === "on" || (preferences.lowPower === "auto" && weakMachine());
  function applyLowPower() {
    const root = document.documentElement;
    if (!root?.dataset) return;
    if (lowPower()) root.dataset.lowPower = "1"; else delete root.dataset.lowPower;
    if (typeof document.dispatchEvent === "function") document.dispatchEvent(new CustomEvent("loginsight:motion-change", { detail: { lowPower: lowPower() } }));
  }
  /** Animations should stay still: the system asks for reduced motion or the PC is basic. */
  function reducedMotion() {
    const media = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
    return media || lowPower();
  }
  applyLowPower();

  // ------------------------------------------------- automatic calculation
  if (typeof document.addEventListener === "function") document.addEventListener("loginsight:source-loaded", event => {
    if (!preferences.autoCompromises) return;
    const count = Number(event.detail?.count) || 0, artifact = event.detail?.artifact ?? null;
    if (count > preferences.autoCompromisesLimit) {
      window.toast?.(`Comprometimentos não calculados automaticamente: ${number(count)} registros, acima do limite de ${number(preferences.autoCompromisesLimit)}.`, "info");
      return;
    }
    // Let the page finish adopting the new source before asking for the analysis.
    setTimeout(async () => {
      try {
        if (!state.loaded || (state.currentArtifact?.id ?? null) !== artifact) return;
        if (window.Security?.cached?.()) return;
        if (await window.Security?.restore?.()) return;
        if (window.Security?.cached?.() || window.Security?.status?.()?.state === "running") return;
        await window.Security?.get?.();
      } catch { /* The page shows the failure and the manual button. */ }
    }, 800);
  });

  // ------------------------------------------------------------------ pane
  let geo = null, geoBusy = "", geoMessage = "", geoFailed = false, pane = null;
  async function loadGeo() {
    try { geo = await invoke("geoip_status"); geoFailed = false; geoMessage = geo.error || ""; if (geo.error) geoFailed = true; }
    catch (error) { geo = null; geoFailed = true; geoMessage = `Não foi possível ler a base de localização: ${error}`; }
    render();
  }
  async function geoAction(label, command, args) {
    if (geoBusy) return;
    geoBusy = label; geoMessage = ""; geoFailed = false; render();
    try {
      geo = await invoke(command, args);
      geoMessage = geo.loaded ? "Base instalada. Calcule os comprometimentos novamente para usar a localização." : "Base removida. As regras de país e distância deixam de ser avaliadas.";
      window.Security?.invalidate?.();
    } catch (error) { geoFailed = true; geoMessage = String(error); }
    finally { geoBusy = ""; render(); }
  }
  async function importGeo() {
    const path = await window.__TAURI__.dialog.open({ multiple: false, directory: false, title: "Base de IPs (ip2asn TSV ou DB-IP CSV)", filters: [{ name: "Base de IPs", extensions: ["tsv", "csv", "gz"] }] });
    if (typeof path === "string" && path) await geoAction("Importando…", "geoip_import", { path });
  }
  function geoSection() {
    const files = geo?.files || [];
    const state = geoBusy ? `<span class="muted">${html(geoBusy)}</span>` : geo?.loaded ? `<strong class="pref-ok">Instalada</strong> · ${files.map(f => `${html(f.name)} (${number(f.ranges)} faixas)`).join(", ")}` : "<strong>Não instalada</strong>";
    return `<section class="pref-block" aria-labelledby="pref-geo-title">
      <h4 id="pref-geo-title">Localização de IPs (offline)</h4>
      <p class="small muted">Com uma base de faixas de IP, cada acesso recebe país, sistema autônomo (AS) e a indicação de provedor de nuvem ou hospedagem. Isso habilita as regras de país novo, viagem impossível, sessão em vários países e login a partir de hospedagem. Nada é consultado na internet durante a análise.</p>
      <p class="small">${state}</p>
      <div class="pref-actions">
        <button type="button" class="btn ghost small" data-geo-download ${geoBusy ? "disabled" : ""}><i class="fas fa-download" aria-hidden="true"></i> Baixar base gratuita (iptoasn.com)</button>
        <button type="button" class="btn ghost small" data-geo-import ${geoBusy ? "disabled" : ""}><i class="fas fa-file-import" aria-hidden="true"></i> Importar arquivo</button>
        ${geo?.loaded ? `<button type="button" class="btn ghost small" data-geo-remove ${geoBusy ? "disabled" : ""}>Remover base</button>` : ""}
      </div>
      <p class="small muted">Formatos aceitos: ip2asn (TSV, pode estar compactado em .gz) e DB-IP country/ASN (CSV). ${geo?.directory ? `Pasta: <code>${html(geo.directory)}</code>` : ""}</p>
      ${geoMessage ? `<p class="small ${geoFailed ? "pref-error" : "pref-ok"}" role="status">${html(geoMessage)}</p>` : ""}
    </section>`;
  }
  function render() {
    if (!pane?.isConnected) return;
    const weak = weakMachine();
    pane.innerHTML = `<section class="pref-block" aria-labelledby="pref-power-title">
        <h4 id="pref-power-title">Modo leve</h4>
        <p class="small muted">Desliga animações e efeitos visuais para deixar a interface mais fluida em computadores básicos. ${weak ? "Este computador foi identificado como básico." : "Este computador não foi identificado como básico."}</p>
        <div class="pref-choices" role="radiogroup" aria-label="Modo leve">
          ${[["auto", "Automático"], ["on", "Sempre ligado"], ["off", "Desligado"]].map(([value, label]) => `<label class="check-line"><input type="radio" name="pref-low-power" value="${value}" ${preferences.lowPower === value ? "checked" : ""}> ${label}</label>`).join("")}
        </div>
        <p class="small">${lowPower() ? "Modo leve ativo." : "Modo leve inativo."}</p>
      </section>
      <section class="pref-block" aria-labelledby="pref-auto-title">
        <h4 id="pref-auto-title">Comprometimentos após importar</h4>
        <label class="check-line"><input type="checkbox" data-auto-compromises ${preferences.autoCompromises ? "checked" : ""}> Calcular os comprometimentos automaticamente quando uma fonte terminar de carregar</label>
        <label class="pref-inline">Somente até <select data-auto-limit aria-label="Limite de registros para o cálculo automático">${LIMITS.map(v => `<option value="${v}" ${v === preferences.autoCompromisesLimit ? "selected" : ""}>${number(v)}</option>`).join("")}</select> registros</label>
        <p class="small muted">Fontes maiores continuam com o botão manual, para não ocupar o computador sem você pedir. Um resultado já salvo é reaberto em vez de recalculado.</p>
      </section>
      ${geoSection()}`;
    pane.querySelectorAll('input[name="pref-low-power"]').forEach(input => input.onchange = () => { write({ lowPower: input.value }); render(); });
    pane.querySelector("[data-auto-compromises]").onchange = event => write({ autoCompromises: event.target.checked });
    pane.querySelector("[data-auto-limit]").onchange = event => write({ autoCompromisesLimit: Number(event.target.value) });
    pane.querySelector("[data-geo-download]").onclick = () => geoAction("Baixando e instalando a base…", "geoip_download");
    pane.querySelector("[data-geo-import]").onclick = () => importGeo().catch(error => { geoFailed = true; geoMessage = String(error); render(); });
    const remove = pane.querySelector("[data-geo-remove]");
    if (remove) remove.onclick = () => geoAction("Removendo…", "geoip_remove");
  }
  function renderPane(target) {
    pane = target; render();
    if (!geoBusy) void loadGeo();
  }

  return { renderPane, reducedMotion, lowPower, preferences: () => ({ ...preferences }) };
})();
