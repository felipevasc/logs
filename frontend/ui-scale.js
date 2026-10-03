/* Interface scale is a preference, independent of compact layout. The existing automatic
   mode fits small/high-DPI windows; an explicit choice is never replaced by resizing. */
window.UiScale = (() => {
  "use strict";
  const KEY = "li-ui-scale";
  const STEPS = [0.7, 0.75, 0.8, 0.85, 0.9, 1, 1.1, 1.25, 1.4, 1.5, 1.75, 2];
  const percent = value => `${Math.round(value * 100)}%`;
  const CHOICES = [["auto", "Automático"], ...STEPS.map(value => [value, percent(value)])];
  // Preserve the automatic default and Ctrl 0 behavior; changing them is a separate decision.
  const REF_WIDTH = 1440, REF_HEIGHT = 820, MIN_AUTO = 0.7;
  let applied = 1, requested = 1, application = "applied", timer = null, resizeDeferred = false;
  let inFlight = false, automaticPending = false, requestId = 0, intentId = 0, queue = Promise.resolve(true);

  function normalize(value) {
    if (value === "auto") return value;
    if (typeof value !== "number" && (typeof value !== "string" || !value.trim())) return null;
    const number = Number(value);
    return STEPS.includes(number) ? number : null;
  }
  function readSetting() {
    try { return normalize(localStorage.getItem(KEY)) ?? "auto"; } catch { return "auto"; }
  }
  // Keep a usable session preference even when persistence is unavailable.
  let preference = readSetting();
  const setting = () => preference;
  const status = () => ({ requested, applied, state: application, setting: setting(), automaticPending });
  function automatic() {
    // Called only outside a native write. Its receipt decides which scale can
    // undo the viewport dimensions observed now; a requested scale is not proof.
    const width = innerWidth * applied, height = innerHeight * applied;
    const fit = Math.min(1, width / REF_WIDTH, height / REF_HEIGHT);
    const density = (window.devicePixelRatio || 1) / applied;
    const floor = Math.max(MIN_AUTO, Math.min(1, 0.9 / density));
    return Math.max(floor, Math.floor(fit * 20 + 1e-6) / 20);
  }
  function syncState() {
    document.documentElement.dataset.uiScale = String(requested);
    document.documentElement.dataset.uiScaleApplied = String(applied);
    document.documentElement.dataset.uiScaleStatus = application;
  }
  function renderVisiblePane() {
    const pane = document.querySelector("#settings-pane-interface");
    if (pane && !pane.closest("[hidden]")) renderPane(pane);
  }
  function scheduleAutomatic() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      if (setting() !== "auto") return;
      if (inFlight) { resizeDeferred = true; return; }
      apply();
    }, 250);
  }
  function apply(choice = setting()) {
    const isAutomatic = choice === "auto";
    const deferAutomatic = isAutomatic && application === "pending";
    // An Auto intent queued behind another write has no reliable target yet.
    // Keep the old requested number as history and expose the pending calculation.
    let scale = isAutomatic ? (deferAutomatic ? requested : automatic()) : choice;
    if (!deferAutomatic && !automaticPending && Math.abs(scale - requested) < 0.001 && application !== "failed") {
      syncState(); renderVisiblePane();
      return application === "pending" ? queue : Promise.resolve(true);
    }
    requested = scale;
    automaticPending = deferAutomatic;
    application = "pending";
    const id = ++requestId;
    syncState(); renderVisiblePane();
    // Serialize native writes, coalescing superseded requests that have not started.
    // Ignoring stale replies alone cannot stop an older native write landing last.
    queue = queue.then(async () => {
      if (id !== requestId) return false;
      if (isAutomatic) {
        // Earlier writes have settled (or were coalesced). Re-read both the
        // confirmed scale and current viewport, even if no resize event fired.
        scale = automatic();
        requested = scale;
        automaticPending = false;
        resizeDeferred = false;
        syncState(); renderVisiblePane();
      }
      inFlight = true;
      let ok = false;
      try { ok = (await invoke("ui_zoom", { scale })) === true; } catch { /* retain the last confirmed scale */ }
      inFlight = false;
      if (ok) applied = scale;
      syncState();
      if (id !== requestId) return false;
      application = ok ? "applied" : "failed";
      syncState(); renderVisiblePane();
      document.dispatchEvent(new CustomEvent("ui-scale-change", { detail: {
        scale, setting: setting(), appliedScale: applied, applied: ok, status: application,
      } }));
      if (resizeDeferred && setting() === "auto") { resizeDeferred = false; scheduleAutomatic(); }
      return ok;
    });
    return queue;
  }
  async function set(value, announce = true) {
    const choice = normalize(value);
    if (choice === null) return false;
    const intent = ++intentId;
    preference = choice;
    clearTimeout(timer); timer = null; resizeDeferred = false;
    try { localStorage.setItem(KEY, String(choice)); } catch { /* session choice remains usable */ }
    const ok = await apply(choice);
    if (intent !== intentId) return false;
    if (announce) {
      if (ok) toast(`Interface ${choice === "auto" ? `automática (${percent(requested)})` : percent(requested)}`, "ok");
      else toast(`Tamanho ${percent(requested)} selecionado, mas não foi possível aplicar nesta janela.`, "info");
    }
    return ok;
  }
  function step(direction) {
    if (!Number.isFinite(direction) || direction === 0) return;
    const next = direction > 0 ? STEPS.find(v => v > requested + 0.001) : [...STEPS].reverse().find(v => v < requested - 0.001);
    if (next !== undefined) return set(next);
  }
  function bindRadios(pane, selector, select) {
    const buttons = [...pane.querySelectorAll(selector)];
    buttons.forEach((button, index) => {
      button.onclick = () => select(button);
      button.onkeydown = event => {
        if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.ctrlKey || event.metaKey || event.altKey) return;
        let next;
        if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (index + 1) % buttons.length;
        else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (index + buttons.length - 1) % buttons.length;
        else if (event.key === "Home") next = 0;
        else if (event.key === "End") next = buttons.length - 1;
        else return; // Enter and Space use the button's native activation.
        event.preventDefault();
        event.stopPropagation();
        buttons[next].focus();
        select(buttons[next]);
      };
    });
  }
  function renderPane(pane) {
    // Capture focus at render time, not request time: a delayed receipt must never
    // steal focus back from another control, tab or a closed settings dialog.
    const focused = pane.contains(document.activeElement) ? document.activeElement : null;
    const focusSelector = focused?.dataset.scale !== undefined ? `[data-scale="${focused.dataset.scale}"]`
      : focused?.dataset.themeChoice ? `[data-theme-choice="${focused.dataset.themeChoice}"]` : null;
    const value = setting(), light = document.documentElement.dataset.theme === "light";
    const message = automaticPending ? "Aguardando o ajuste em andamento para calcular o tamanho automático…"
      : application === "pending" ? `Aplicando ${percent(requested)}…`
      : application === "failed" ? `${percent(requested)} selecionado. Não foi possível aplicar nesta janela. Último tamanho confirmado: ${percent(applied)}.`
      : `Tamanho aplicado: ${percent(applied)}.`;
    pane.innerHTML = `<div class="ui-pref"><div class="ui-pref-head"><strong>Tamanho da interface</strong><span>${value === "auto" ? (automaticPending ? "Automático" : `Automático: ${percent(requested)} nesta janela`) : percent(requested)}</span></div>
      <div class="seg ui-scale-choices" role="radiogroup" aria-label="Tamanho da interface" aria-describedby="ui-scale-status">${CHOICES.map(([v, label]) => `<button type="button" class="seg-btn${v === value ? " active" : ""}" role="radio" aria-checked="${v === value}" tabindex="${v === value ? 0 : -1}" data-scale="${v}">${label}</button>`).join("")}</div>
      <p class="muted small" id="ui-scale-status" role="status" aria-live="polite" aria-atomic="true">${message}</p>
      <p class="muted small">Automático reduz a interface quando a janela é pequena ou a tela usa escala do sistema, para caber mais informação. Uma escolha manual é mantida ao redimensionar. Atalhos: <kbd>Ctrl</kbd> <kbd>+</kbd> e <kbd>Ctrl</kbd> <kbd>−</kbd> ajustam até 200%, <kbd>Ctrl</kbd> <kbd>0</kbd> volta ao automático.</p></div>
      <div class="ui-pref"><div class="ui-pref-head"><strong>Tema</strong></div>
      <div class="seg" role="radiogroup" aria-label="Tema"><button type="button" class="seg-btn${light ? "" : " active"}" role="radio" aria-checked="${!light}" tabindex="${light ? -1 : 0}" data-theme-choice="dark"><i class="fas fa-moon"></i> Escuro</button><button type="button" class="seg-btn${light ? " active" : ""}" role="radio" aria-checked="${light}" tabindex="${light ? 0 : -1}" data-theme-choice="light"><i class="fas fa-sun"></i> Claro</button></div></div>`;
    bindRadios(pane, "[data-scale]", button => set(button.dataset.scale, false));
    bindRadios(pane, "[data-theme-choice]", button => {
      if ((document.documentElement.dataset.theme === "light") !== (button.dataset.themeChoice === "light")) toggleTheme();
      renderPane(pane);
    });
    if (focusSelector && pane.isConnected && !pane.closest("[hidden]")) pane.querySelector(focusSelector)?.focus();
  }

  document.addEventListener("keydown", event => {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || !(event.ctrlKey || event.metaKey) || event.altKey) return;
    if (event.key === "=" || event.key === "+") { event.preventDefault(); step(1); }
    else if (event.key === "-" || event.key === "_") { event.preventDefault(); step(-1); }
    else if (event.key === "0") { event.preventDefault(); set("auto"); }
  });
  window.addEventListener("resize", () => { if (setting() === "auto") scheduleAutomatic(); });
  apply();
  return { set, step, renderPane, current: () => requested, setting, status };
})();
