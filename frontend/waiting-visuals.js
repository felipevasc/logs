/* Decorative scenes for real operations. No timers, progress estimation or IPC.
   Load as a classic script and pair with waiting-visuals.css.

   mount(host, receipt, options); update replaces the receipt.
   setVisible(false) pauses hidden panels; destroy before removing the host.

   phaseIndex is one-based. phaseCount, elapsedMs and estimateMs are optional measured/
   supplied values; estimateMs is remaining time for THIS phase. No value is inferred.
   Long scenes start directly when elapsedMs >= 4000; otherwise the complete finite
   gesture promotes at its own CSS boundary if work is still pending.
   Lock access is always gesture-only: elapsedMs describes the whole operation.
   Reactions are chosen only at CSS cycle boundaries, from supplied elapsed time.
   They never hold results, change receipts or estimate progress. Pauses freeze all tracks.
   Same-operation phase changes coalesce at complete scene boundaries. Receipts
   stay immediate. Unknown and terminal states are static.
*/
window.WaitingVisuals = (() => {
  "use strict";

  // Exact protocol IDs, not fuzzy matches against localized labels or opaque operation IDs.
  const PHASES = Object.freeze({
    'metadata-lock': 'access', 'metadata-restore': 'restoration',
    'metadata-scan': 'reading', 'metadata-json-boundaries': 'reading',
    'metadata-columns': 'reading', 'metadata-validate': 'verification',
    'metadata-map-validate': 'verification', 'analytics-time-index': 'reading',
    'pivot-values': 'reading', 'canonical-verify': 'verification',
    'canonical-convert': 'reading', 'canonical-extract': 'reading',
    'canonical-publish': 'checkpoint', 'canonical-lock': 'access',
    'engine-validate': 'verification', 'engine-validated': 'verification',
    'engine-restore': 'restoration', 'engine-prepare': 'reading',
    'engine-columns': 'reading', 'engine-index': 'reading',
    'engine-checkpoint-write': 'checkpoint', 'engine-text-merge': 'calculation',
    'engine-checkpoint-sync': 'checkpoint', 'engine-checkpoint-publish': 'checkpoint',
    'engine-checkpoint-committed': 'checkpoint', 'engine-open': 'restoration',
    'engine-ready': 'restoration', 'source-prepare': 'reading', 'source-indexed': 'reading',
    'source-activate': 'restoration', 'source-settle': 'restoration',
    'metadata-checkpoint-write': 'checkpoint', 'metadata-checkpoint-sync': 'checkpoint',
    'metadata-checkpoint-publish': 'checkpoint', 'metadata-checkpoint-committed': 'checkpoint',
    'analytics-select': 'calculation', 'analytics-verify': 'verification',
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
    const partialCheckpoint = ['metadata-checkpoint-committed', 'engine-checkpoint-committed'].includes(phaseId);
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
      metric, details, elapsedMs, variant: variantFor(operationId, family), motionMode: !adapters[family]?.gestureOnly && elapsedMs !== null && elapsedMs >= 4000 ? 'loop' : 'gesture' };
  }

  // A shared repertoire with explicit family safe points. Adapters must release
  // or park their tool before the shared reaction starts, and recover it before
  // resuming the exact task pose. Only the long work loop may enter this bridge.
  const repertoire = Object.freeze({
    coffee: Object.freeze({ minElapsedMs: 45000, durationMs: 32000, cooldown: 4, weight: 1, long: true }),
    manual: Object.freeze({ minElapsedMs: 60000, durationMs: 32000, cooldown: 5, weight: 1, long: true }),
    review: Object.freeze({ minElapsedMs: 15000, durationMs: 10000, cooldown: 2, weight: 4 }),
    stretch: Object.freeze({ minElapsedMs: 28000, durationMs: 12000, cooldown: 3, weight: 2 }),
    // Small empty-hand gestures share every safe adapter, with low selection weight.
    visor: Object.freeze({ minElapsedMs: 24000, durationMs: 4800, cooldown: 4, weight: 1 }),
    wave: Object.freeze({ minElapsedMs: 36000, durationMs: 3600, cooldown: 4, weight: 1 })
  });
  const adapters = Object.freeze({
    reading: Object.freeze({ reactions: true, homeX: 0, head: -4, cycleMs: 7200, safePoint: 'filed-sheet-empty-hand' }),
    checkpoint: Object.freeze({ reactions: true, homeX: 20, head: 4, cycleMs: 6800, safePoint: 'closed-drawer-released-handle' }),
    calculation: Object.freeze({ reactions: true, homeX: 26, anchorX: 20, head: 4, cycleMs: 6400, bridgeMs: 1200, safePoint: 'filed-tile-empty-hand' }),
    composition: Object.freeze({ reactions: true, homeX: 24, anchorX: 20, head: 8, cycleMs: 7600, bridgeMs: 3200, safePoint: 'proof-parked-on-existing-bed' }),
    // Empty hand at both endpoints; shares the checkpoint reaction anchor without a bridge.
    verification: Object.freeze({ reactions: true, homeX: 20, head: 4, cycleMs: 9600, safePoint: 'lens-docked-empty-hand' }),
    // The supplied age belongs to the whole operation, not this short lock wait.
    access: Object.freeze({ reactions: false, gestureOnly: true, homeX: 20, head: 4, cycleMs: 2400, safePoint: 'empty-hands-closed-terminal' }),
    restoration: Object.freeze({ reactions: true, homeX: 20, head: 4, cycleMs: 7200, safePoint: 'record-reseated-empty-hand' })
  });
  // Measured decoration age is private. It is never written back to derive(),
  // status/details or the application's operation receipt. A quiet pending promise
  // can become eligible without transport events, polling or a scheduling timer.
  function createElapsedClock(now) {
    let scope = '', base = null, anchor = 0, lastNow = 0;
    function tick() {
      const sampled = measured(now());
      if (sampled !== null) lastNow = Math.max(lastNow, sampled);
      return lastNow;
    }
    return Object.freeze({
      observe(model) {
        const nextScope = `${model.operationId}\u0000${model.phaseId}`;
        const supplied = measured(model.elapsedMs);
        if (nextScope !== scope) {
          scope = nextScope; base = supplied; anchor = tick();
        } else if (supplied !== null) { base = supplied; anchor = tick(); }
        // A sparse same-phase receipt replaces displayed values, but cannot erase
        // an earlier measured anchor used only for decorative eligibility.
      },
      read() { return base === null ? null : Math.min(Number.MAX_SAFE_INTEGER, base + tick() - anchor); }
    });
  }
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
        if (episode !== 'work' || model?.partialCheckpoint || !model?.canAnimate || model.motionMode !== 'loop' || !adapters[model.family]?.reactions) return null;
        cycle++;
        if (cooldown > 0) { cooldown--; return null; }
        const elapsed = measured(model.elapsedMs);
        const previous = history.at(-1);
        const eligible = Object.entries(repertoire).filter(([name, item]) => elapsed !== null && elapsed >= item.minElapsedMs
          && name !== previous && !(item.long && repertoire[previous]?.long));
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
  // The same lens is either docked or carried by the hand. Its handle passes through
  // the existing palm at (91,55); no independent translation can detach the tool.
  const verifyLens = `<path class="wv-verify-handle" d="m91 55 6.5-5.3"/><circle class="wv-verify-lens" cx="102" cy="46" r="6.3"/><path class="wv-verify-glint" d="M98.2 44.7a4 4 0 0 1 3-2.6"/>`;
  // An existing index dot is read, never added as a checkmark or completion cue.
  // The tab is inside the unchanged palm at (91,55); the seated copy is translated
  // by the actor's x20 only, matching body/arm/hand 0° at both transfers.
  const restoreRecord = `<path class="wv-task-paper" d="M94 50h17v12H94v-5h-3v-4h3Z"/><circle class="wv-restore-marker" cx="99" cy="55" r="1.4"/><path class="wv-task-detail" d="M104 54h4m-4 4h4"/>`;
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
    // Station ends at x162, leaving the future coffee hatch region clear.
    // Dock matrix = translate(20,0) · arm(35°) · hand(-5°), using the shared pivots.
    verification: `<path class="wv-verify-rail wv-rail" d="M39 83h123m-115-3v3m110-3v3"/>
      <g class="wv-verify-station">
        <path class="wv-task-machine" d="M106 71h56l-3 5h-50Zm6 5v5m43-5v5"/>
        <path class="wv-task-edge" d="M121 60v11m31-16v16"/>
        <rect class="wv-task-inset" x="117" y="29" width="20" height="32" rx="2"/>
        <g class="wv-verify-document"><path class="wv-task-paper" d="M120 33h10l4 4v21h-14Z"/>
          <path class="wv-task-detail" d="M130 33v4h4m-11 5h8m-8 5h5m-5 5h8"/></g>
        <g class="wv-verify-reference"><rect class="wv-task-inset" x="142" y="30" width="17" height="25" rx="2"/>
          <path class="wv-task-detail" d="M146 36h9m-9 6h7m-7 5h9"/></g>
        <g class="wv-verify-docked" transform="matrix(0.866025403784 0.5 -0.5 0.866025403784 54.735978714537 -28.578796792173)">${verifyLens}</g>
        <path class="wv-verify-cradle" d="M113 65v4h14v-4m-7 4v2"/>
      </g>
      <g class="wv-verify-actor" transform="translate(20 0)">${taskRobot(`<g class="wv-verify-held">${verifyLens}</g>`)}</g>`,
    // A closed local terminal conveys journal contention, never permission/login.
    access: `<path class="wv-rail" d="M39 83h119m-111-3v3m104-3v3"/>
      <g class="wv-access-station">
        <path class="wv-task-machine" d="M120 43h38v31h-38Z"/>
        <path class="wv-task-inset" d="M124 47h30v17h-30Z"/>
        <path class="wv-access-slot" d="M129 55h20"/>
        <path class="wv-task-edge" d="M125 74v7m28-7v7m-26-12h7"/>
      </g>
      <g class="wv-access-actor" transform="translate(20 0)">${taskRobot()}</g>`,
    // Low open archive and a fixed support; no checkpoint drawer or moving press.
    restoration: `<path class="wv-rail" d="M39 83h122m-114-3v3m108-3v3"/>
      <g class="wv-restore-station">
        <path class="wv-task-machine" d="M119 55h39v19h-43V60Z"/>
        <path class="wv-task-inset" d="M121 51h31v15h-31Z"/>
        <path class="wv-task-edge" d="M136 54v9m5-9v9m5-9v9m-26 11v7m34-7v7"/>
        <path class="wv-restore-support" d="M109 58v4h24v-4m-20 4v11"/>
        <g class="wv-restore-seated" transform="matrix(1 0 0 1 20 0)">${restoreRecord}</g>
      </g>
      <g class="wv-restore-actor" transform="translate(20 0)">${taskRobot(`<g class="wv-restore-held">${restoreRecord}</g>`)}</g>`,
    neutral: `<path class="wv-rail" d="M70 83h56"/><g class="wv-idle-actor" transform="translate(29 0)">${taskRobot()}</g>`
  });

  const coffeeCup = `<g class="wv-cup-steam"><path d="M94 47c-1-1 1-2 0-3"/><path d="M97 46c-1-1 1-2 0-3"/></g><path class="wv-cup-shell" d="M92 50h7v6a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2Z"/><path class="wv-cup-handle" d="M92 52h-2v4h2"/><path class="wv-cup-rim" d="M93 50h5"/>`;
  // Closed transport grips the left edge at (64, 58.7). During consultation the
  // original rear palm supports (64, 62.6619), reachable without changing its rig.
  // The right cover and one loose page hinge at x=74.5, following the front hand.
  const manualClosed = `<path class="wv-manual-cover" d="M64 48.5q5.25-1.5 10.5.5v14q-5.25-2-10.5-.5Z"/><path class="wv-manual-detail" d="M66 51.5h5m-5 2.5h4m2.5-5v13"/>`;
  const manualLeft = `<path class="wv-manual-leaf" d="M64 48.5q5.25-1.5 10.5.5v14q-5.25-2-10.5-.5Z"/><path class="wv-manual-lines" d="M66 52h6m-6 3h6m-6 3h4"/>`;
  const manualRight = `<path class="wv-manual-leaf" d="M74.5 49q5.25-2 10.5-.5v14q-5.25-1.5-10.5.5Z"/><path class="wv-manual-lines" d="M77 52h6m-6 3h6m-6 3h4"/>`;
  const manualOpen = `${manualLeft}<g class="wv-manual-cover-fold">${manualRight}<g class="wv-manual-cover-face"><g transform="translate(149 0) scale(-1 1)">${manualClosed}</g></g></g><g class="wv-manual-page">${manualRight}</g><path class="wv-manual-spine" d="M74.5 49v14"/>`;
  function reactionScenery(family) {
    if (!adapters[family]?.reactions) return '';
    // The coffee hatch and cup share measured contact coordinates with the hand.
    // The hatch stays open while the cup is out, and closes under the empty hand.
    const kitchen = `<g class="wv-kitchen"><path class="wv-kitchen-wall" d="M166 34h23v47h-23Z"/>
      <path class="wv-kitchen-recess" d="M169 43h17v21h-17Z"/><path class="wv-kitchen-shelf" d="M168 59h19m-19 6h19"/>
      <g class="wv-cup-shelf" transform="translate(82 0)">${coffeeCup}</g>
      <g class="wv-kitchen-hatch"><path class="wv-kitchen-wall" d="M168 43h19v18h-19Z"/><path class="wv-kitchen-handle" d="M171 55h4"/>
        <path class="wv-kitchen-detail" d="M172 47h11"/></g><path class="wv-kitchen-detail" d="M170 72h13m-13 3h9"/>
      <path class="wv-rail" d="M165 83h25"/></g>`;
    // The manual has its own open shelf, never a door or a coffee fixture.
    const manualKit = `<g class="wv-manual-kit"><path class="wv-manual-shelf-board" d="M167 59.7h22V62h-22Z"/>
      <path class="wv-manual-shelf-bracket" d="M170 62v6h5m11-6v6h-5"/>
      <g class="wv-manual-shelf" transform="translate(109 -3.7)">${manualClosed}</g></g>`;
    const proof = family === 'composition' ? `<g class="wv-proof-held">${composeSheet}</g>` : '';
    const parked = family === 'composition' ? `<g class="wv-proof-parked" transform="translate(26.341973495 1.717610627)">${composeSheet}</g>` : '';
    const backArm = `<g class="wv-task-arm-back"><path class="wv-task-limb" d="m56 51-7 9 5 5"/><circle class="wv-task-palm" cx="54" cy="65" r="2.5"/></g>`;
    const supportingArm = `<g class="wv-task-arm-back"><path class="wv-task-limb" d="m56 51-7 9 5 5"/><g class="wv-manual-supported"><g class="wv-manual-support-wrist"><g transform="translate(-10 2.33809621)">${manualOpen}</g></g></g><circle class="wv-task-palm" cx="54" cy="65" r="2.5"/></g>`;
    // Keep the rear hand's prop in its actual hierarchy, but draw that arm above
    // the torso so the supported book cannot disappear behind the chest shell.
    const actor = taskRobot(`${proof}<g class="wv-cup-held"><g class="wv-cup-wrist">${coffeeCup}</g></g><g class="wv-manual-held"><g class="wv-manual-wrist"><g transform="translate(27 -3.7)">${manualClosed}</g></g></g>`)
      .replace(backArm, '').replace('<g class="wv-task-arm">', `${supportingArm}<g class="wv-task-arm">`).replaceAll('wv-task', 'wv-react');
    return `${kitchen}${manualKit}${parked}<g class="wv-reaction-actor">${actor}</g>`;
  }

  const gestureDuration = family => ['checkpoint', 'composition'].includes(family) ? 2800
    : ['calculation', 'verification'].includes(family) ? 2600 : 2400;
  let dismissCompletion = null;
  const mounted = new Set();
  function mount(host, initialSnapshot = {}, options = {}) {
    if (!host?.ownerDocument?.createElement || typeof host.append !== 'function') throw new TypeError('WaitingVisuals.mount precisa de um elemento.');
    dismissCompletion?.();
    const document = host.ownerDocument, view = document.defaultView || window;
    const root = document.createElement('div'); root.className = 'waiting-visual';
    const art = document.createElement('div'); art.className = 'wv-art'; art.setAttribute('aria-hidden', 'true');
    const status = document.createElement('div'); status.className = 'wv-status';
    status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite'); status.setAttribute('aria-atomic', 'true');
    const metric = document.createElement('div'); metric.className = 'wv-metric';
    const details = document.createElement('div'); details.className = 'wv-details';
    const control = document.createElement('button'); control.className = 'wv-motion-toggle'; control.type = 'button';
    root.append(art, status, metric, details, control); host.append(root); mounted.add(root);
    let current, sceneKey = '', destroyed = false, finishing = false, gestureFinished = false, visible = true, intersecting = false;
    let director, directorOwner = '', workSignal, episodeSignal, adapterSignal, moving = false, deferredCompletion = null;
    let completionWorkEnd = null;
    let motionEnabled = options.motionEnabled !== false;
    const elapsedClock = createElapsedClock(typeof view.performance?.now === 'function' ? () => view.performance.now() : () => 0);
    const media = typeof view.matchMedia === 'function' ? view.matchMedia('(prefers-reduced-motion: reduce)') : null;

    function syncMotion() {
      if (destroyed || !current) return;
      const reduced = !!media?.matches;
      moving = current.canAnimate && motionEnabled && !reduced && visible && intersecting && !document.hidden && root.isConnected;
      if (finishing && !moving) { destroy(); return; }
      root.dataset.motion = moving ? 'running' : 'static';
      // Latch animation assignment. Pausing changes only play-state, never its name.
      if (moving) root.dataset.animated = 'true';
      if (!current.canAnimate) root.dataset.animated = 'false';
      // Receipts cannot cut a gesture, replace paused tracks, or demote a loop.
      control.hidden = finishing || !current.canAnimate || reduced;
      control.textContent = motionEnabled ? 'Pausar animação' : 'Retomar animação';
      control.setAttribute('aria-label', motionEnabled ? 'Pausar apenas a animação; a operação continua' : 'Retomar apenas a animação; a operação continua');
      control.setAttribute('aria-pressed', String(!motionEnabled));
      // CSS can finish just before a pause but deliver its end event afterward.
      // Resume consumes that one already-reached endpoint, never a queue of work
      // iterations or future reactions. Every other paused event is discarded.
      if (moving && deferredCompletion) {
        const completion = deferredCompletion; deferredCompletion = null;
        if (completion.stage === root.dataset.adapter && completion.episode === root.dataset.episode) boundary(completion);
      }
    }
    function selectScene(force = false) {
      const key = `${current.operationId}\u0000${current.family}`;
      if (key === sceneKey) return false;
      // Retain only the latest desired family, never a queue of stale phases.
      if (!force && current.canAnimate && directorOwner === current.operationId
          && root.dataset.animated === 'true' && adapters[root.dataset.family]
          && !(adapters[root.dataset.family].gestureOnly && gestureFinished)) return false;
      deferredCompletion = null; gestureFinished = false;
      art.innerHTML = `<svg viewBox="0 0 192 96" aria-hidden="true" focusable="false" xmlns="http://www.w3.org/2000/svg"><g class="wv-work">${scenes[current.family]}</g>${reactionScenery(current.family)}</svg>`;
      workSignal?.remove(); episodeSignal?.remove(); adapterSignal?.remove();
      workSignal = document.createElement('span'); workSignal.className = 'wv-work-boundary';
      episodeSignal = document.createElement('span'); episodeSignal.className = 'wv-episode-boundary';
      adapterSignal = document.createElement('span'); adapterSignal.className = 'wv-adapter-boundary';
      workSignal.setAttribute('aria-hidden', 'true'); episodeSignal.setAttribute('aria-hidden', 'true'); adapterSignal.setAttribute('aria-hidden', 'true');
      art.append(workSignal, episodeSignal, adapterSignal);
      const sameOperation = director && directorOwner === current.operationId;
      if (!sameOperation) {
        director = createDirector(current.operationId, options.reactionSeed ?? current.operationId);
        directorOwner = current.operationId;
      } else director.finish();
      const wasLooping = sameOperation && root.dataset.pace === 'loop';
      root.dataset.episode = 'work'; root.dataset.adapter = 'work'; root.dataset.reactionVariant = 'a'; root.dataset.animated = 'false';
      root.dataset.family = current.family; root.dataset.variant = current.variant;
      root.dataset.pace = wasLooping && !adapters[current.family]?.gestureOnly ? 'loop' : current.motionMode;
      sceneKey = key;
      return true;
    }
    function finishReaction() {
      if (finishing) { destroy(); return; }
      director.finish(); root.dataset.episode = 'work'; root.dataset.adapter = 'work';
      selectScene(true); syncMotion();
    }
    function update(snapshot = {}) {
      if (destroyed || finishing) return false;
      const previous = current;
      current = derive(snapshot);
      // A committed segment remains inside the pending operation, except on an
      // explicit terminal/cancelling state, which must always stop immediately.
      if (current.state === 'running' && current.partialCheckpoint && previous?.canAnimate && current.operationId === previous.operationId)
        current = { ...current, family: root.dataset.family, canAnimate: true };
      elapsedClock.observe(current);
      selectScene();
      if (!current.canAnimate) deferredCompletion = null;
      if (!current.canAnimate && root.dataset.episode !== 'work') {
        director.finish(); root.dataset.episode = 'work'; root.dataset.adapter = 'work';
      }
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
      if (destroyed || !current?.canAnimate || !root.isConnected || event.pseudoElement) return;
      // Restoring an older WebView's clock can enqueue an iteration that was
      // already consumed. Only the still-pending work boundary may end the tail.
      if (finishing && completionWorkEnd !== null && event.target === workSignal && event.type === 'animationiteration'
          && (measured(event.elapsedTime) ?? 0) * 1000 < completionWorkEnd - 1) return;
      if (!moving || document.hidden || media?.matches || !visible || !intersecting || !motionEnabled) {
        let completedDuration = null;
        if (event.type === 'animationend') {
          if (event.target === workSignal && event.animationName === 'wv-gesture-boundary' && root.dataset.pace === 'gesture') completedDuration = gestureDuration(root.dataset.family);
          else if (event.target === episodeSignal && event.animationName === 'wv-episode-boundary' && root.dataset.adapter === 'react') completedDuration = repertoire[root.dataset.episode]?.durationMs;
          else if (event.target === adapterSignal && ['prepare', 'resume'].includes(root.dataset.adapter) && event.animationName === `wv-${root.dataset.adapter}-boundary`) completedDuration = adapters[root.dataset.family]?.bridgeMs;
        }
        const elapsed = measured(event.elapsedTime);
        if (completedDuration > 0 && elapsed !== null && Math.abs(elapsed * 1000 - completedDuration) < 1) {
          deferredCompletion = { type: event.type, target: event.target, animationName: event.animationName,
            elapsedTime: elapsed, stage: root.dataset.adapter, episode: root.dataset.episode };
        }
        return;
      }
      if (event.target === workSignal && event.type === 'animationend' && event.animationName === 'wv-gesture-boundary'
          && root.dataset.episode === 'work' && root.dataset.pace === 'gesture') {
        if (finishing) { destroy(); return; }
        gestureFinished = true;
        if (!adapters[root.dataset.family]?.gestureOnly) root.dataset.pace = 'loop';
        selectScene(true); syncMotion();
      } else if (event.target === workSignal && event.type === 'animationiteration' && event.animationName === 'wv-work-boundary' && root.dataset.episode === 'work') {
        if (finishing) { destroy(); return; }
        if (selectScene(true)) { syncMotion(); return; }
        const next = director.boundary({ ...current, family: root.dataset.family, motionMode: root.dataset.pace, elapsedMs: elapsedClock.read() });
        if (next) {
          root.dataset.episode = next.episode; root.dataset.reactionVariant = next.variant;
          root.dataset.adapter = adapters[root.dataset.family].bridgeMs ? 'prepare' : 'react';
        }
      } else if (event.target === episodeSignal && event.type === 'animationend' && event.animationName === 'wv-episode-boundary' && root.dataset.adapter === 'react') {
        if (adapters[root.dataset.family].bridgeMs) root.dataset.adapter = 'resume';
        else finishReaction();
      } else if (event.target === adapterSignal && event.type === 'animationend' && root.dataset.episode !== 'work') {
        if (root.dataset.adapter === 'prepare' && event.animationName === 'wv-prepare-boundary') root.dataset.adapter = 'react';
        else if (root.dataset.adapter === 'resume' && event.animationName === 'wv-resume-boundary') {
          finishReaction();
        }
      }
    }
    function setMotionEnabled(enabled) { if (destroyed) return; motionEnabled = !!enabled; syncMotion(); }
    function toggleMotion() { setMotionEnabled(!motionEnabled); }
    function setVisible(shown) {
      if (destroyed) return;
      visible = !!shown; root.dataset.visible = String(visible); root.inert = finishing || !visible;
      root.setAttribute('aria-hidden', String(finishing || !visible)); syncMotion();
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
    function moveCompletion() {
      // Atomic DOM moves retain CSSAnimation identities (Chromium 133+). Older
      // WebViews remove/reinsert on append, so retain their clocks explicitly.
      // https://developer.chrome.com/blog/movebefore-api
      if (typeof document.body.moveBefore === 'function') {
        try { document.body.moveBefore(root, null); return 'atomic'; } catch { /* use the compatible clock transfer */ }
      }
      const tracks = art.getAnimations?.({ subtree: true }).map(animation => ({
        target: animation.effect?.target, name: animation.animationName,
        time: animation.currentTime, start: animation.startTime, timeline: animation.timeline,
        state: animation.playState, duration: animation.effect?.getComputedTiming?.().duration,
      }));
      if (!tracks?.length || tracks.some(track => !track.target || !Number.isFinite(track.time))) return null;
      const work = tracks.find(track => track.target === workSignal && track.name === 'wv-work-boundary');
      if (root.dataset.episode === 'work' && root.dataset.pace === 'loop') {
        if (!work || !(work.duration > 0)) return null;
        completionWorkEnd = (Math.floor(work.time / work.duration) + 1) * work.duration;
      }
      document.body.append(root);
      const remaining = art.getAnimations({ subtree: true });
      const pairs = tracks.map(track => {
        const index = remaining.findIndex(animation => animation.effect?.target === track.target && animation.animationName === track.name);
        return [track, index < 0 ? null : remaining.splice(index, 1)[0]];
      });
      if (remaining.length || pairs.some(([, animation]) => !animation)) return null;
      // All assignments occur before rendering. Running tracks keep the original
      // document-timeline origin; paused work stays parked throughout a reaction.
      for (const [track, animation] of pairs) {
        animation.currentTime = track.time;
        if (track.state === 'running' && Number.isFinite(track.start) && animation.timeline === track.timeline)
          animation.startTime = track.start;
        // Do not call play()/pause(): CSS must retain control of parked tracks.
        if (animation.playState !== track.state) return null;
      }
      return 'clock-transfer';
    }
    function complete() {
      if (destroyed || finishing) return false;
      // Results settle immediately. Only the existing moving sequence may finish
      // outside its blocking host, with no new reaction, focus or input capture.
      if (!moving || !root.isConnected || (gestureFinished && root.dataset.pace === 'gesture') || mounted.size > 1) { destroy(); return false; }
      finishing = true; dismissCompletion = destroy;
      root.dataset.finishing = 'true'; root.dataset.state = 'completed';
      root.inert = true; root.setAttribute('aria-hidden', 'true');
      status.textContent = 'Concluído'; metric.hidden = true; details.hidden = true; control.hidden = true;
      try { root.dataset.completionTransfer = moveCompletion() || ''; } catch { root.dataset.completionTransfer = ''; }
      if (!root.dataset.completionTransfer) { destroy(); return false; }
      document.addEventListener('workspace-context-change', destroy);
      document.addEventListener('analysis-context-change', destroy);
      return true;
    }
    function destroy() {
      if (destroyed) return;
      destroyed = true; deferredCompletion = null; mounted.delete(root);
      if (dismissCompletion === destroy) dismissCompletion = null;
      root.dataset.motion = 'static';
      observer?.disconnect();
      document.removeEventListener('visibilitychange', syncMotion);
      document.removeEventListener('workspace-context-change', destroy);
      document.removeEventListener('analysis-context-change', destroy);
      if (media?.removeEventListener) media.removeEventListener('change', syncMotion);
      else if (media?.removeListener) media.removeListener(syncMotion);
      control.removeEventListener('click', toggleMotion);
      art.removeEventListener('animationiteration', boundary);
      art.removeEventListener('animationend', boundary);
      root.remove();
    }
    return Object.freeze({ element: root, update, setVisible, setMotionEnabled, complete, destroy,
      inspect: () => ({ ...director?.inspect(), adapterStage: root.dataset.adapter, finishing }) });
  }
  return Object.freeze({ derive, mount, repertoire, adapters, createDirector, createElapsedClock });
})();
