/* Decorative scenes for real operations. No timers, progress estimation or IPC.
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
   Reactions are chosen only at CSS cycle boundaries, from supplied elapsed time.
   They never hold results, change receipts or estimate progress. Pauses freeze all tracks.
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
    'command:aggregate_events': 'calculation', 'command:pivot': 'calculation',
    'command:case_report_render': 'composition', 'command:timeline_export_render': 'composition'
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
      metric, details, elapsedMs, variant: variantFor(operationId, family), motionMode: elapsedMs !== null && elapsedMs >= 4000 ? 'loop' : 'gesture' };
  }

  // A shared repertoire, with explicit family safe points. New adapters must prove
  // an empty hand / parked tool at BOTH cycle endpoints before enabling reactions.
  const repertoire = Object.freeze({
    coffee: Object.freeze({ minElapsedMs: 45000, durationMs: 32000, cooldown: 4, weight: 1 }),
    review: Object.freeze({ minElapsedMs: 15000, durationMs: 10000, cooldown: 2, weight: 4 }),
    stretch: Object.freeze({ minElapsedMs: 28000, durationMs: 12000, cooldown: 3, weight: 2 })
  });
  const adapters = Object.freeze({
    reading: Object.freeze({ reactions: true, homeX: 0, head: -4, cycleMs: 7200, safePoint: 'filed-sheet-empty-hand' }),
    checkpoint: Object.freeze({ reactions: true, homeX: 20, head: 4, cycleMs: 6800, safePoint: 'closed-drawer-released-handle' }),
    calculation: Object.freeze({ reactions: false, safePoint: 'tile-in-hand-needs-parking' }),
    composition: Object.freeze({ reactions: false, safePoint: 'proof-in-hand-needs-parking' })
  });
  function seedFor(value) {
    let hash = 2166136261;
    for (const character of String(value)) hash = Math.imul(hash ^ character.charCodeAt(0), 16777619) >>> 0;
    return hash || 1;
  }
  // Pure, boundary-driven scheduling. Receipts, visibility and count changes never
  // consume randomness. Cooldowns count completed visible work cycles, not timers.
  function createDirector(operationId, seed = operationId) {
    const initialSeed = seedFor(`${operationId}:${seed}`);
    let random = initialSeed, cycle = 0, cooldown = 0, episode = 'work', variant = 'a';
    const history = [];
    function draw() {
      random ^= random << 13; random ^= random >>> 17; random ^= random << 5;
      return random >>> 0;
    }
    function inspect() { return { episode, variant, cycle, cooldown, history: history.slice(), seed: initialSeed }; }
    return Object.freeze({
      inspect,
      boundary(model) {
        if (episode !== 'work' || !model?.canAnimate || model.motionMode !== 'loop' || !adapters[model.family]?.reactions) return null;
        cycle++;
        if (cooldown > 0) { cooldown--; return null; }
        const elapsed = measured(model.elapsedMs);
        const eligible = Object.entries(repertoire).filter(([name, item]) => elapsed !== null && elapsed >= item.minElapsedMs && name !== history.at(-1));
        if (!eligible.length) return null;
        // Work remains dominant; only one in three eligible safe points starts a break.
        if (draw() % 3 !== 0) return null;
        let choice = draw() % eligible.reduce((sum, [, item]) => sum + item.weight, 0);
        episode = eligible.find(([, item]) => (choice -= item.weight) < 0)[0];
        variant = draw() % 2 ? 'b' : 'a';
        history.push(episode); if (history.length > 4) history.shift();
        return inspect();
      },
      finish() {
        if (episode === 'work') return false;
        cooldown = repertoire[episode].cooldown; episode = 'work'; return true;
      }
    });
  }

  // Only these constant SVG strings enter innerHTML. All receipt text uses textContent.
  // No external assets, SVG IDs, clip paths or filters: scenes can coexist safely.
  const readerSheet = `<rect class="wv-reader-paper" x="90" y="34" width="18" height="28" rx="2"/><path class="wv-reader-paper-lines" d="M102 34v6h6m-13 5h8m-8 4h6m-6 4h8"/>`;
  // The approved robot silhouette, scoped away from reading's fixed choreography.
  // held is always constant artwork and remains a direct child of the articulated hand.
  const taskRobot = (held = '') => `      <g class="wv-task">
        <g class="wv-task-leg-back"><path class="wv-task-limb" d="M61 61v13.5"/>
          <g class="wv-task-knee-back"><path class="wv-task-limb" d="M61 74.5V88"/><circle class="wv-task-joint" cx="61" cy="74.5" r="1.6"/>
            <g class="wv-task-foot-back"><path class="wv-task-boot" d="M58 86h5l4 2v2h-10v-2Z"/></g>
          </g>
        </g>
        <g class="wv-task-leg-front"><path class="wv-task-limb" d="M68 61v13.5"/>
          <g class="wv-task-knee-front"><path class="wv-task-limb" d="M68 74.5V88"/><circle class="wv-task-joint" cx="68" cy="74.5" r="1.6"/>
            <g class="wv-task-foot-front"><path class="wv-task-boot" d="M65 86h5l4 2v2h-10v-2Z"/></g>
          </g>
        </g>
        <g class="wv-task-body">
          <g class="wv-task-arm-back"><path class="wv-task-limb" d="m56 51-7 9 5 5"/><circle class="wv-task-palm" cx="54" cy="65" r="2.5"/></g>
          <path class="wv-task-shell" d="M59 47h10l6 7-3 10H57l-4-10Z"/>
          <path class="wv-task-seam" d="M59 59h10m-7-10v3h5v-3"/>
          <g class="wv-task-head"><path class="wv-task-neck" d="M61 45v5h7v-5"/>
            <rect class="wv-task-shell" x="50" y="23" width="28" height="24" rx="7"/>
            <path class="wv-task-ear" d="M48 32v6m32-6v6"/>
            <rect class="wv-task-visor-bed" x="55" y="29" width="18" height="12" rx="5"/>
            <g class="wv-task-gaze"><path class="wv-task-visor" d="M59 35h10"/><path class="wv-task-smile" d="m59 34 3 2h4l3-2"/></g>
            <path class="wv-task-brow" d="M57 26h7"/>
          </g>
          <g class="wv-task-arm"><path class="wv-task-limb" d="m72 52 10 6"/><circle class="wv-task-joint" cx="72" cy="52" r="2"/>
            <g class="wv-task-hand">${held}
              <path class="wv-task-limb" d="m82 58 9-3"/><circle class="wv-task-joint" cx="82" cy="58" r="1.8"/>
              <path class="wv-task-palm" d="M90 53h3v5h-3l-2-2Z"/>
              <path class="wv-task-fingers" d="M91 55h2"/>
            </g>
          </g>
        </g>
      </g>`;
  const groupTile = `<rect class="wv-task-tile" x="93" y="47" width="10" height="10" rx="2.5"/><path class="wv-task-tile-mark" d="M96 50h4v4h-4Z"/>`;
  // One proof is fed into a hand-operated press, then retrieved by the same hand.
  const composeSheet = `<rect class="wv-task-paper" x="93" y="52" width="16" height="11" rx="1.3"/><path class="wv-task-detail" d="M96 55h9m-9 2.5h5m-5 2.5h9"/>`;
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
    checkpoint: `<path class="wv-rail" d="M39 83h132m-124-3v3m116-3v3"/>
      <g class="wv-archive-station">
        <path class="wv-task-machine" d="M124 38h35l5 5v33h-45V43Z"/>
        <path class="wv-task-edge" d="m124 38 4 5h36m-36 0v31m-5 2v5m35-5v5"/>
        <rect class="wv-task-inset" x="131" y="49" width="27" height="21" rx="2"/>
        <g class="wv-archive-label"><rect class="wv-task-paper" x="137" y="40" width="14" height="7" rx="2"/><path class="wv-task-detail" d="M141 43.5h6"/></g>
        <g class="wv-archive-drawer">
          <path class="wv-task-inset" d="m114 55 8-5h32v18h-40Z"/>
          <g class="wv-archive-folder"><path class="wv-task-folder" d="M121 52V45h10l3 3h16v18h-29Z"/><path class="wv-task-detail" d="M125 53h20m-18 4h14"/></g>
          <path class="wv-task-machine" d="M112 54h8v16h-8Zm8 10h34v6h-34Z"/>
          <path class="wv-task-edge" d="M123 67h25"/>
          <path class="wv-archive-handle" d="M112 57h-3v4h3"/>
        </g>
      </g>
      <g class="wv-archive-actor" transform="translate(20 0)">${taskRobot()}</g>`,
    calculation: `<path class="wv-rail" d="M39 83h132m-124-3v3m116-3v3"/>
      <g class="wv-group-station">
        <path class="wv-task-machine" d="M114 68h51l-3 5h-45Zm5 5v8m39-8v8"/>
        <path class="wv-task-inset" d="M118 30h44v31h-44Z"/>
        <path class="wv-task-edge" d="M120 58h40m-20-26v23"/>
        <rect class="wv-task-paper" x="120" y="33" width="10" height="6" rx="2"/><path class="wv-task-detail" d="M124 35h2v2h-2Z"/>
        <rect class="wv-task-paper" x="144" y="33" width="10" height="6" rx="2"/><circle class="wv-task-detail" cx="149" cy="36" r="1.3"/>
        <rect class="wv-task-ghost" x="144" y="43" width="10" height="10" rx="2.5"/><circle class="wv-task-detail" cx="149" cy="48" r="2"/>
        <path class="wv-task-edge" d="M113 66h16m-14-2h12"/>
        <g class="wv-group-source" transform="matrix(1.000000000 0.000000000 -0.000000000 1.000000000 21.598036657 8.196672658)">${groupTile}</g>
        <g class="wv-group-well"><path class="wv-task-guide" d="M121 43v12h12"/>
          <g class="wv-group-filed" transform="matrix(1.000000000 0.000000000 -0.000000000 1.000000000 29.121431799 -3.091967925)">${groupTile}</g>
        </g>
      </g>
      <g class="wv-group-actor" transform="translate(26 0)">${taskRobot(`<g class="wv-group-held">${groupTile}</g>`)}</g>`,
    composition: `<path class="wv-rail" d="M39 83h132m-124-3v3m116-3v3"/>
      <g class="wv-compose-station">
        <path class="wv-task-machine" d="M116 69h47v6h-47Zm5 6v6m37-6v6M148 33h9v36h-9Z"/>
        <path class="wv-task-edge" d="M151 38h3m-3 5h3m-3 5h3m-3 5h3m-3 5h3"/>
        <path class="wv-task-machine" d="M128 27h29v8h-29Z"/>
        <path class="wv-task-inset" d="M130 35h5v13h-5Z"/>
        <g class="wv-compose-bed">
          <path class="wv-task-inset" d="M117 66h28l4 3h-32Z"/>
          <g class="wv-compose-loaded" transform="translate(26.341973495 1.717610627)">${composeSheet}</g>
        </g>
        <g class="wv-compose-press">
          <path class="wv-task-machine" d="M121 46h22v5h-22Z"/>
          <path class="wv-task-limb" d="M132 46V39h-18v7"/>
          <path class="wv-compose-grip" d="M111 46h6"/>
          <path class="wv-task-edge" d="M125 49h14"/>
        </g>
      </g>
      <g class="wv-compose-actor" transform="translate(24 0)">${taskRobot(`<g class="wv-compose-held">${composeSheet}</g>`)}</g>`,
    neutral: `<path class="wv-rail" d="M70 83h56"/><g class="wv-idle-actor" transform="translate(29 0)">${taskRobot()}</g>`
  });

  const coffeeCup = `<path class="wv-cup-shell" d="M92 50h7v6a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2Z"/><path class="wv-cup-handle" d="M92 52h-2v4h2"/><path class="wv-cup-rim" d="M93 50h5"/>`;
  function reactionScenery(family) {
    if (!adapters[family]?.reactions) return '';
    // The hatch and cup share the same measured contact coordinates as the hand.
    // The hatch stays open while the cup is out, and only closes under the empty hand.
    const kitchen = `<g class="wv-kitchen"><path class="wv-kitchen-wall" d="M166 34h23v47h-23Z"/>
      <path class="wv-kitchen-recess" d="M169 43h17v21h-17Z"/><path class="wv-kitchen-shelf" d="M168 59h19m-19 6h19"/>
      <g class="wv-cup-shelf" transform="translate(82 0)">${coffeeCup}</g>
      <g class="wv-kitchen-hatch"><path class="wv-kitchen-wall" d="M168 43h19v18h-19Z"/><path class="wv-kitchen-handle" d="M171 55h4"/>
        <path class="wv-kitchen-detail" d="M172 47h11"/></g><path class="wv-kitchen-detail" d="M170 72h13m-13 3h9"/>
      <path class="wv-rail" d="M165 83h25"/></g>`;
    const actor = taskRobot(`<g class="wv-cup-held"><g class="wv-cup-wrist">${coffeeCup}</g></g>`).replaceAll('wv-task', 'wv-react');
    return `${kitchen}<g class="wv-reaction-actor">${actor}</g>`;
  }

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
    let director, directorOwner = '', workSignal, episodeSignal, moving = false;
    let motionEnabled = options.motionEnabled !== false;
    const media = typeof view.matchMedia === 'function' ? view.matchMedia('(prefers-reduced-motion: reduce)') : null;

    function syncMotion() {
      if (destroyed || !current) return;
      const reduced = !!media?.matches;
      moving = current.canAnimate && motionEnabled && !reduced && visible && intersecting && !document.hidden && root.isConnected;
      root.dataset.motion = moving ? 'running' : 'static';
      // Latch animation assignment. Pausing changes only play-state, never its name.
      if (moving) root.dataset.animated = 'true';
      if (!current.canAnimate) root.dataset.animated = 'false';
      if (root.dataset.episode === 'work') root.dataset.pace = current.motionMode;
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
        art.innerHTML = `<svg viewBox="0 0 192 96" aria-hidden="true" focusable="false" xmlns="http://www.w3.org/2000/svg"><g class="wv-work">${scenes[current.family]}</g>${reactionScenery(current.family)}</svg>`;
        // Actual DOM targets, never selectors from an event or an old SVG instance.
        workSignal?.remove(); episodeSignal?.remove();
        workSignal = document.createElement('span'); workSignal.className = 'wv-work-boundary';
        episodeSignal = document.createElement('span'); episodeSignal.className = 'wv-episode-boundary';
        workSignal.setAttribute('aria-hidden', 'true'); episodeSignal.setAttribute('aria-hidden', 'true');
        art.append(workSignal, episodeSignal);
        if (!director || directorOwner !== current.operationId) {
          director = createDirector(current.operationId, options.reactionSeed ?? current.operationId);
          directorOwner = current.operationId;
        } else director.finish();
        root.dataset.episode = 'work'; root.dataset.reactionVariant = 'a'; root.dataset.animated = 'false';
        sceneKey = key;
      }
      if (!current.canAnimate && root.dataset.episode !== 'work') {
        director.finish();
        root.dataset.episode = 'work';
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
    function boundary(event) {
      if (destroyed || !moving || !current?.canAnimate || !root.isConnected || document.hidden || media?.matches || !visible || !intersecting || !motionEnabled || event.pseudoElement) return;
      if (event.target === workSignal && event.type === 'animationiteration' && event.animationName === 'wv-work-boundary' && root.dataset.episode === 'work') {
        const next = director.boundary(current);
        if (next) { root.dataset.episode = next.episode; root.dataset.reactionVariant = next.variant; }
      } else if (event.target === episodeSignal && event.type === 'animationend' && event.animationName === 'wv-episode-boundary' && root.dataset.episode !== 'work') {
        director.finish(); root.dataset.episode = 'work'; syncMotion();
      }
    }
    function setMotionEnabled(enabled) { if (destroyed) return; motionEnabled = !!enabled; syncMotion(); }
    function toggleMotion() { setMotionEnabled(!motionEnabled); }
    function setVisible(shown) {
      if (destroyed) return;
      visible = !!shown; root.dataset.visible = String(visible); root.inert = !visible;
      root.setAttribute('aria-hidden', String(!visible)); syncMotion();
    }
    control.addEventListener('click', toggleMotion);
    art.addEventListener('animationiteration', boundary);
    art.addEventListener('animationend', boundary);
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
      inspect: () => director?.inspect(),
      destroy() {
        if (destroyed) return;
        destroyed = true;
        root.dataset.motion = 'static';
        observer?.disconnect();
        document.removeEventListener('visibilitychange', syncMotion);
        if (media?.removeEventListener) media.removeEventListener('change', syncMotion);
        else if (media?.removeListener) media.removeListener(syncMotion);
        control.removeEventListener('click', toggleMotion);
        art.removeEventListener('animationiteration', boundary);
        art.removeEventListener('animationend', boundary);
        root.remove();
      }
    });
  }
  return Object.freeze({ derive, mount, repertoire, adapters, createDirector });
})();
