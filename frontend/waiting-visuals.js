/* One small, decorative scene for a real operation. No timers, progress estimation or IPC.
   Load as a classic script and pair with waiting-visuals.css.

   const view = WaitingVisuals.mount(host, {
     operationId, phaseId, state: 'running', label: 'Indexando metadados',
     completed, total, unit, elapsedMs
   }, { showDetails: false, motionEnabled: true });
   view.update(nextSnapshot); // Replaces, rather than merges, the previous receipt.
   view.setVisible(false);    // Call when the host's panel is hidden by the application.
   view.destroy();            // Call before removing/replacing the host.

   phaseIndex is one-based. phaseCount, elapsedMs and estimateMs are optional measured/
   supplied values; estimateMs is remaining time for THIS phase. No value is inferred.
   Long scenes loop only when supplied elapsedMs >= 4000. Short scenes make one gesture.
   An unknown phase/state is static, even if its human label sounds like a known operation.
*/
window.WaitingVisuals = (() => {
  "use strict";

  // Exact protocol IDs, not fuzzy matches against localized labels or opaque operation IDs.
  const PHASES = Object.freeze({
    'metadata-scan': 'reading', 'metadata-json-boundaries': 'reading',
    'metadata-columns': 'reading', 'metadata-validate': 'reading',
    'metadata-map-validate': 'reading', 'analytics-time-index': 'reading',
    'pivot-values': 'reading', 'canonical-verify': 'reading',
    'metadata-checkpoint-write': 'checkpoint', 'metadata-checkpoint-sync': 'checkpoint',
    'metadata-checkpoint-publish': 'checkpoint', 'metadata-checkpoint-committed': 'checkpoint',
    'analytics-select': 'calculation', 'analytics-verify': 'calculation',
    'analytics-sql': 'calculation',
    // Explicit adapter IDs for genuinely pending commands, not native subphases.
    'command:aggregate_events': 'calculation', 'command:pivot': 'calculation'
  });
  const STATES = Object.freeze({
    running: 'Em andamento', queued: 'Na fila', paused: 'Pausado',
    cancelling: 'Cancelando', cancelled: 'Cancelado', error: 'Não foi possível continuar',
    completed: 'Concluído', idle: 'Aguardando', unknown: 'Aguardando informações'
  });
  const numberFormat = new Intl.NumberFormat('pt-BR', { maximumFractionDigits: 2 });
  const text = value => typeof value === 'string' ? value.trim() : '';
  const measured = value => typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= Number.MAX_SAFE_INTEGER ? value : null;
  const ordinal = value => Number.isSafeInteger(value) && value > 0 ? value : null;
  function duration(milliseconds) {
    const seconds = Math.floor(milliseconds / 1000);
    return seconds < 60 ? `${seconds} s` : seconds < 3600
      ? `${Math.floor(seconds / 60)} min ${seconds % 60} s`
      : `${Math.floor(seconds / 3600)} h ${Math.floor(seconds / 60) % 60} min`;
  }

  function variantFor(operationId, family) {
    // Two quiet compositions per family, fixed for the life of an operation.
    let hash = 7;
    for (const character of `${operationId}:${family}`) hash = (Math.imul(hash, 31) + character.charCodeAt(0)) >>> 0;
    return hash % 2 ? 'b' : 'a';
  }

  function derive(snapshot = {}) {
    const input = snapshot && typeof snapshot === 'object' ? snapshot : {};
    const operationId = text(input.operationId), phaseId = text(input.phaseId);
    const state = Object.hasOwn(STATES, input.state) ? input.state : 'unknown';
    const partialCheckpoint = phaseId === 'metadata-checkpoint-committed';
    const knownFamily = Object.hasOwn(PHASES, phaseId) ? PHASES[phaseId] : 'neutral';
    const family = operationId && state === 'running' ? knownFamily : 'neutral';
    const canAnimate = family !== 'neutral' && !partialCheckpoint;
    const suppliedLabel = text(input.label);
    const label = suppliedLabel || (partialCheckpoint && state === 'running' ? 'Checkpoint preservado' : STATES[state]);
    const status = suppliedLabel && state !== 'running' ? `${STATES[state]} · ${suppliedLabel}` : label;
    const completed = measured(input.completed), suppliedTotal = measured(input.total);
    // Zero/absent/contradictory totals are not usable denominators. Equality never means success.
    const total = suppliedTotal > 0 && completed !== null && suppliedTotal >= completed ? suppliedTotal : null;
    const unit = text(input.unit);
    const metric = completed === null ? '' : `${numberFormat.format(completed)}${total === null ? '' : ` / ${numberFormat.format(total)}`}${unit ? ` ${unit}` : ''}`;
    const phaseIndex = ordinal(input.phaseIndex), suppliedPhaseCount = ordinal(input.phaseCount);
    const phaseCount = phaseIndex !== null && suppliedPhaseCount >= phaseIndex ? suppliedPhaseCount : null;
    const elapsedMs = measured(input.elapsedMs), estimateMs = measured(input.estimateMs);
    const details = [];
    if (phaseIndex !== null) details.push(`Etapa ${phaseIndex}${phaseCount === null ? '' : ` de ${phaseCount}`}`);
    if (elapsedMs !== null) details.push(`${duration(elapsedMs)} decorridos`);
    if (estimateMs !== null && state === 'running' && !partialCheckpoint) details.push(`≈ ${duration(estimateMs)} restantes nesta etapa`);
    return { operationId, phaseId, state, family, canAnimate, partialCheckpoint, label, status,
      metric, details, variant: variantFor(operationId, family), motionMode: elapsedMs !== null && elapsedMs >= 4000 ? 'loop' : 'gesture' };
  }

  // Only these constant SVG strings enter innerHTML. All receipt text uses textContent.
  // No external assets, SVG IDs, clip paths or filters: scenes can coexist safely.
  const helper = `<g class="wv-helper"><rect x="139" y="35" width="15" height="13" rx="4"/><path class="wv-visor" d="M144 40h5"/><path d="M143 49v6m7-6v6m-10 0h5m3 0h5"/><path class="wv-arm" d="m138 44-8 5-5-2"/></g>`;
  const rail = `<path class="wv-rail" d="M32 78h128M38 75v3m116-3v3"/>`;
  const readerSheet = `<rect class="wv-reader-paper" x="90" y="34" width="18" height="28" rx="2"/><path class="wv-reader-paper-lines" d="M102 34v6h6m-13 5h8m-8 4h6m-6 4h8"/>`;
  const scenes = Object.freeze({
    // The carried sheet is a child of the forearm/hand, never an independently moving prop.
    // Source/destination transforms match the hand's 18° + 30° − 48° contact pose exactly.
    reading: `<path class="wv-rail" d="M23 83h148m-140-3v3m132-3v3"/>
      <g class="wv-reader-station"><path d="M82 70h30l-3 5H85Zm48 0h30l-3 5h-24Z"/>
        <path class="wv-reader-stack-edge" d="M86 70h21m-19 2h17m30-2h21m-19 2h17"/>
        <g class="wv-reader-source" transform="translate(-4.401963 8.196673)">${readerSheet}</g>
        <g class="wv-reader-filed" transform="translate(43.598037 8.196673)">${readerSheet}</g>
      </g>
      <g class="wv-reader">
        <g class="wv-reader-leg-back"><path class="wv-reader-limb" d="M61 61v13.5"/>
          <g class="wv-reader-knee-back"><path class="wv-reader-limb" d="M61 74.5V88"/><circle class="wv-reader-joint" cx="61" cy="74.5" r="1.6"/>
            <g class="wv-reader-foot-back"><path class="wv-reader-boot" d="M58 86h5l4 2v2h-10v-2Z"/></g>
          </g>
        </g>
        <g class="wv-reader-leg-front"><path class="wv-reader-limb" d="M68 61v13.5"/>
          <g class="wv-reader-knee-front"><path class="wv-reader-limb" d="M68 74.5V88"/><circle class="wv-reader-joint" cx="68" cy="74.5" r="1.6"/>
            <g class="wv-reader-foot-front"><path class="wv-reader-boot" d="M65 86h5l4 2v2h-10v-2Z"/></g>
          </g>
        </g>
        <g class="wv-reader-body">
          <g class="wv-reader-arm-back"><path class="wv-reader-limb" d="m56 51-7 9 5 5"/><circle class="wv-reader-palm" cx="54" cy="65" r="2.5"/></g>
          <path class="wv-reader-shell" d="M59 47h10l6 7-3 10H57l-4-10Z"/>
          <path class="wv-reader-seam" d="M59 59h10m-7-10v3h5v-3"/>
          <g class="wv-reader-head"><path class="wv-reader-neck" d="M61 45v5h7v-5"/>
            <rect class="wv-reader-shell" x="50" y="23" width="28" height="24" rx="7"/>
            <path class="wv-reader-ear" d="M48 32v6m32-6v6"/>
            <rect class="wv-reader-visor-bed" x="55" y="29" width="18" height="12" rx="5"/>
            <g class="wv-reader-gaze"><path class="wv-reader-visor" d="M59 35h10"/><path class="wv-reader-smile" d="m59 34 3 2h4l3-2"/></g>
            <path class="wv-reader-brow" d="M57 26h7"/>
          </g>
          <g class="wv-reader-arm"><path class="wv-reader-limb" d="m72 52 10 6"/><circle class="wv-reader-joint" cx="72" cy="52" r="2"/>
            <g class="wv-reader-hand"><g class="wv-read-card">${readerSheet}</g>
              <path class="wv-reader-limb" d="m82 58 9-3"/><circle class="wv-reader-joint" cx="82" cy="58" r="1.8"/>
              <path class="wv-reader-palm" d="M90 53h3v5h-3l-2-2Z"/>
              <path class="wv-reader-fingers" d="M91 55h2"/>
            </g>
          </g>
        </g>
      </g>`,
    checkpoint: `${rail}<path class="wv-guide" d="M71 27h52m-52 0v7m52-7v7"/><g class="wv-save-card"><rect x="84" y="34" width="24" height="29" rx="3"/><path d="M90 41h12m-12 6h9"/></g><g class="wv-tray"><path d="m74 58 4 13h40l4-13m-48 0h10l3 6h22l3-6h10"/><path class="wv-tray-line" d="M88 69h18"/></g>${helper}`,
    calculation: `${rail}<g class="wv-guide"><rect x="51" y="30" width="30" height="37" rx="8"/><rect x="96" y="30" width="30" height="37" rx="8"/></g><g class="wv-point wv-point-a"><circle cx="62" cy="41" r="3"/></g><g class="wv-point wv-point-b"><circle cx="72" cy="54" r="3"/></g><g class="wv-point wv-point-c"><circle cx="106" cy="42" r="3"/></g><g class="wv-point wv-point-d"><circle cx="117" cy="54" r="3"/></g>${helper}`,
    neutral: `<g class="wv-neutral"><rect x="77" y="32" width="38" height="34" rx="7"/><path d="M87 44h18m-18 9h11"/><circle cx="113" cy="64" r="3"/></g>`
  });

  function mount(host, initialSnapshot = {}, options = {}) {
    if (!host?.ownerDocument?.createElement || typeof host.append !== 'function') throw new TypeError('WaitingVisuals.mount precisa de um elemento.');
    const document = host.ownerDocument, view = document.defaultView || window;
    const root = document.createElement('div'); root.className = 'waiting-visual';
    const art = document.createElement('div'); art.className = 'wv-art'; art.setAttribute('aria-hidden', 'true');
    const status = document.createElement('div'); status.className = 'wv-status';
    status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite'); status.setAttribute('aria-atomic', 'true');
    const metric = document.createElement('div'); metric.className = 'wv-metric';
    const details = document.createElement('div'); details.className = 'wv-details';
    const control = document.createElement('button'); control.className = 'wv-motion-toggle'; control.type = 'button';
    root.append(art, status, metric, details, control); host.append(root);
    let current, sceneKey = '', destroyed = false, visible = true, intersecting = false;
    let motionEnabled = options.motionEnabled !== false;
    const media = typeof view.matchMedia === 'function' ? view.matchMedia('(prefers-reduced-motion: reduce)') : null;

    function syncMotion() {
      if (destroyed || !current) return;
      const reduced = !!media?.matches;
      const moving = current.canAnimate && motionEnabled && !reduced && visible && intersecting && !document.hidden && root.isConnected;
      root.dataset.motion = moving ? 'running' : 'static';
      root.dataset.pace = current.motionMode;
      control.hidden = !current.canAnimate || reduced;
      control.textContent = motionEnabled ? 'Pausar animação' : 'Retomar animação';
      control.setAttribute('aria-label', motionEnabled ? 'Pausar apenas a animação; a operação continua' : 'Retomar apenas a animação; a operação continua');
      control.setAttribute('aria-pressed', String(!motionEnabled));
    }
    function update(snapshot = {}) {
      if (destroyed) return false;
      current = derive(snapshot);
      // Counters/elapsed receipts do not rebuild SVG or restart its CSS timeline.
      const key = `${current.operationId}\u0000${current.family}`;
      if (key !== sceneKey) {
        art.innerHTML = `<svg viewBox="0 0 192 96" aria-hidden="true" focusable="false" xmlns="http://www.w3.org/2000/svg">${scenes[current.family]}</svg>`;
        sceneKey = key;
      }
      root.dataset.family = current.family;
      root.dataset.variant = current.variant;
      root.dataset.state = current.state;
      root.dataset.checkpoint = current.partialCheckpoint ? 'preserved' : 'pending';
      // Announce concise semantic changes only, never every count or elapsed update.
      if (status.textContent !== current.status) status.textContent = current.status;
      if (metric.textContent !== current.metric) metric.textContent = current.metric;
      metric.hidden = !current.metric;
      const detailText = current.details.join(' · ');
      if (details.textContent !== detailText) details.textContent = detailText;
      details.hidden = options.showDetails !== true || !detailText;
      syncMotion();
      return true;
    }
    function setMotionEnabled(enabled) { if (destroyed) return; motionEnabled = !!enabled; syncMotion(); }
    function toggleMotion() { setMotionEnabled(!motionEnabled); }
    function setVisible(shown) { if (destroyed) return; visible = !!shown; root.hidden = !visible; syncMotion(); }
    control.addEventListener('click', toggleMotion);
    document.addEventListener('visibilitychange', syncMotion);
    if (media?.addEventListener) media.addEventListener('change', syncMotion);
    else if (media?.addListener) media.addListener(syncMotion);
    // No IntersectionObserver means static, not an unbounded offscreen fallback loop.
    const observer = typeof view.IntersectionObserver === 'function' ? new view.IntersectionObserver(entries => {
      if (destroyed) return;
      for (const entry of entries) if (entry.target === root) intersecting = entry.isIntersecting && entry.intersectionRatio > 0;
      syncMotion();
    }, { threshold: 0 }) : null;
    observer?.observe(root);
    update(initialSnapshot);
    return Object.freeze({
      element: root, update, setVisible, setMotionEnabled,
      destroy() {
        if (destroyed) return;
        destroyed = true;
        root.dataset.motion = 'static';
        observer?.disconnect();
        document.removeEventListener('visibilitychange', syncMotion);
        if (media?.removeEventListener) media.removeEventListener('change', syncMotion);
        else if (media?.removeListener) media.removeListener(syncMotion);
        control.removeEventListener('click', toggleMotion);
        root.remove();
      }
    });
  }
  return Object.freeze({ derive, mount });
})();
