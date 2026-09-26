/* Interface scale: on small or high-DPI windows the interface zooms out so more of the investigation fits
   on screen. Adjustable in Settings → Interface and with Ctrl + / Ctrl − / Ctrl 0. */
window.UiScale = (() => {
  "use strict";
  const KEY = "li-ui-scale";
  const STEPS = [0.7, 0.75, 0.8, 0.85, 0.9, 1, 1.1, 1.25, 1.4];
  const CHOICES = [["auto", "Automático"], [0.8, "80%"], [0.9, "90%"], [1, "100%"], [1.1, "110%"], [1.25, "125%"]];
  // Below this workspace (in CSS pixels at 100%), the automatic scale zooms out.
  const REF_WIDTH = 1440, REF_HEIGHT = 820, MIN_AUTO = 0.7;
  let applied = 1, requested = 1, timer = null;

  function setting() {
    let value = "auto";
    try { value = localStorage.getItem(KEY) || "auto"; } catch { /* storage unavailable */ }
    const number = Number(value);
    return STEPS.includes(number) ? number : "auto";
  }
  function automatic() {
    // `applied` undoes the current zoom: the window's own size decides the scale.
    const width = innerWidth * applied, height = innerHeight * applied;
    const fit = Math.min(1, width / REF_WIDTH, height / REF_HEIGHT);
    // Text stays legible: screens without system scaling zoom out less than high-DPI ones.
    const density = (window.devicePixelRatio || 1) / applied;
    const floor = Math.max(MIN_AUTO, Math.min(1, 0.9 / density));
    return Math.max(floor, Math.floor(fit * 20 + 1e-6) / 20);
  }
  const target = () => { const value = setting(); return value === "auto" ? automatic() : value; };
  const percent = value => `${Math.round(value * 100)}%`;

  async function apply(scale = target()) {
    if (Math.abs(scale - requested) < 0.001) return;
    requested = scale;
    // Assume success first: the resize caused by the zoom must not recompute the scale.
    applied = scale;
    document.documentElement.dataset.uiScale = String(scale);
    let ok = false;
    try { ok = await invoke("ui_zoom", { scale }); } catch { ok = false; }
    if (!ok) applied = 1;
    document.dispatchEvent(new CustomEvent("ui-scale-change", { detail: { scale, setting: setting() } }));
  }
  async function set(value, announce = true) {
    try { localStorage.setItem(KEY, String(value)); } catch { /* storage unavailable */ }
    await apply(target());
    if (announce) toast(`Interface ${value === "auto" ? `automática (${percent(requested)})` : percent(requested)}`, "ok");
    const pane = document.querySelector("#settings-pane-interface");
    if (pane && !pane.hidden) renderPane(pane);
  }
  function step(direction) {
    const next = direction > 0 ? STEPS.find(v => v > requested + 0.001) : [...STEPS].reverse().find(v => v < requested - 0.001);
    if (next) set(next);
  }

  function renderPane(pane) {
    const value = setting(), light = document.documentElement.dataset.theme === "light";
    pane.innerHTML = `<div class="ui-pref"><div class="ui-pref-head"><strong>Tamanho da interface</strong><span>${value === "auto" ? `Automático: ${percent(requested)} nesta janela` : percent(requested)}</span></div>
      <div class="seg ui-scale-choices" role="radiogroup" aria-label="Tamanho da interface">${CHOICES.map(([v, label]) => `<button type="button" class="seg-btn${String(v) === String(value) ? " active" : ""}" role="radio" aria-checked="${String(v) === String(value)}" data-scale="${v}">${label}</button>`).join("")}</div>
      <p class="muted small">Automático reduz a interface quando a janela é pequena ou a tela usa escala do sistema, para caber mais informação. Atalhos: <kbd>Ctrl</kbd> <kbd>+</kbd> e <kbd>Ctrl</kbd> <kbd>−</kbd> ajustam, <kbd>Ctrl</kbd> <kbd>0</kbd> volta ao automático.</p></div>
      <div class="ui-pref"><div class="ui-pref-head"><strong>Tema</strong></div>
      <div class="seg" role="radiogroup" aria-label="Tema"><button type="button" class="seg-btn${light ? "" : " active"}" role="radio" aria-checked="${!light}" data-theme-choice="dark"><i class="fas fa-moon"></i> Escuro</button><button type="button" class="seg-btn${light ? " active" : ""}" role="radio" aria-checked="${light}" data-theme-choice="light"><i class="fas fa-sun"></i> Claro</button></div></div>`;
    pane.querySelectorAll("[data-scale]").forEach(button => { button.onclick = () => set(button.dataset.scale === "auto" ? "auto" : Number(button.dataset.scale), false); });
    pane.querySelectorAll("[data-theme-choice]").forEach(button => {
      button.onclick = () => { if ((document.documentElement.dataset.theme === "light") !== (button.dataset.themeChoice === "light")) toggleTheme(); renderPane(pane); };
    });
  }

  document.addEventListener("keydown", event => {
    if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
    if (event.key === "=" || event.key === "+") { event.preventDefault(); step(1); }
    else if (event.key === "-" || event.key === "_") { event.preventDefault(); step(-1); }
    else if (event.key === "0") { event.preventDefault(); set("auto"); }
  });
  window.addEventListener("resize", () => {
    if (setting() !== "auto") return;
    clearTimeout(timer);
    timer = setTimeout(() => apply(), 250);
  });
  apply();
  return { set, step, renderPane, current: () => requested, setting };
})();
