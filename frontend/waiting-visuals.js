/* Loading "room" for real operations. Load as a classic script with waiting-visuals.css.

   mount(host, receipt, options); update replaces the receipt.
   setVisible(false) pauses hidden panels; destroy before removing the host.

   The loader is a ring around a small round room. Inside it, a robot occasionally
   performs a short skit. Every skit starts and ends with an empty room: the robot
   walks in, brings its props, acts, takes the props away and walks out. A new skit
   is only chosen while the room is empty, so phase changes never cut a scene.
   Skits are decorative: they never estimate progress, hold results or change receipts.
   phaseIndex is one-based; phaseCount, elapsedMs and estimateMs are optional measured
   values (estimateMs is the remaining time for THIS phase). No value is inferred.
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
  const LIVE_STATES = new Set(['running', 'queued', 'cancelling']);
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

  // canAnimate: the phase has a contextual scene family. live: the loader itself moves.
  function derive(snapshot = {}) {
    const input = snapshot && typeof snapshot === 'object' ? snapshot : {};
    const operationId = text(input.operationId), phaseId = text(input.phaseId);
    const state = Object.hasOwn(STATES, input.state) ? input.state : 'unknown';
    const partialCheckpoint = ['metadata-checkpoint-committed', 'engine-checkpoint-committed'].includes(phaseId);
    const knownFamily = Object.hasOwn(PHASES, phaseId) ? PHASES[phaseId] : 'neutral';
    const family = operationId && state === 'running' ? knownFamily : 'neutral';
    const canAnimate = family !== 'neutral' && !partialCheckpoint;
    const live = !!operationId && LIVE_STATES.has(state);
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
    return { operationId, phaseId, state, family, canAnimate, partialCheckpoint, live, label, status, metric, details, elapsedMs };
  }

  // Measured decoration age is private and only gates long skits (coffee, nap...).
  function createElapsedClock(now) {
    let scope = '', base = null, anchor = 0, lastNow = 0;
    function tick() {
      const sampled = measured(now());
      if (sampled !== null) lastNow = Math.max(lastNow, sampled);
      return lastNow;
    }
    return Object.freeze({
      observe(model) {
        const nextScope = model.operationId;
        const supplied = measured(model.elapsedMs);
        if (nextScope !== scope) { scope = nextScope; base = supplied; anchor = tick(); }
        else if (supplied !== null) { base = supplied; anchor = tick(); }
      },
      read() { return base === null ? null : Math.min(Number.MAX_SAFE_INTEGER, base + tick() - anchor); }
    });
  }

  // ------------------------------------------------------------------ randomness
  function seeded(seed) {
    let state = 2166136261;
    for (const character of String(seed)) state = Math.imul(state ^ character.charCodeAt(0), 16777619) >>> 0;
    state = state || 1;
    return () => {
      state ^= state << 13; state ^= state >>> 17; state ^= state << 5; state >>>= 0;
      return state / 4294967296;
    };
  }
  function tools(random) {
    return Object.freeze({
      next: random,
      range: (low, high) => low + random() * (high - low),
      chance: probability => random() < probability,
      pick: list => list[Math.min(list.length - 1, Math.floor(random() * list.length))],
      int: (low, high) => low + Math.min(high - low, Math.floor(random() * (high - low + 1)))
    });
  }

  // ------------------------------------------------------------------ 2D affine math
  // [a, b, c, d, e, f] maps (x, y) to (a x + c y + e, b x + d y + f), as SVG does.
  const mul = (m, n) => [m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1], m[0] * n[2] + m[2] * n[3],
    m[1] * n[2] + m[3] * n[3], m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5]];
  const translate = (x, y) => [1, 0, 0, 1, x, y];
  const scale = (x, y = x) => [x, 0, 0, y, 0, 0];
  const rad = degrees => degrees * Math.PI / 180, deg = radians => radians * 180 / Math.PI;
  const rotate = degrees => { const c = Math.cos(rad(degrees)), s = Math.sin(rad(degrees)); return [c, s, -s, c, 0, 0]; };
  const invert = m => {
    const det = m[0] * m[3] - m[1] * m[2];
    return [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det];
  };
  const apply = (m, x, y) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

  // CSS-identical cubic-bezier easing, evaluated when a skit is compiled.
  const EASES = Object.freeze({
    linear: [0, 0, 1, 1], io: [0.42, 0, 0.58, 1], in: [0.42, 0, 1, 1], out: [0, 0, 0.58, 1],
    soft: [0.45, 0.05, 0.55, 0.95], walk: [0.3, 0.08, 0.7, 0.92], back: [0.34, 1.5, 0.64, 1], snap: [0.2, 0.9, 0.3, 1]
  });
  const easingCss = name => name === 'linear' ? 'linear' : `cubic-bezier(${EASES[name].join(',')})`;
  const easingFns = Object.fromEntries(Object.entries(EASES).map(([name, [x1, y1, x2, y2]]) => {
    const curve = (t, a, b) => ((1 - 3 * b + 3 * a) * t + (3 * b - 6 * a)) * t * t + 3 * a * t;
    return [name, x => {
      if (x <= 0) return 0;
      if (x >= 1) return 1;
      let low = 0, high = 1, t = x;
      for (let i = 0; i < 24; i++) { t = (low + high) / 2; if (curve(t, x1, x2) < x) low = t; else high = t; }
      return curve(t, y1, y2);
    }];
  }));

  // ------------------------------------------------------------------ robot rig
  // Robot-local units: origin between the feet on the floor, y grows downwards.
  // Every joint is a static positioning group around an animated group drawn from its pivot.
  const UPPER = 7.5, FORE = 7, SHOULDER_Y = -10.5, NECK_Y = -14.5, HIP_Y = -17;
  const REST = Object.freeze({ armF: 4, elbF: -16, armB: -5, elbB: -14 });
  const ROBOT_CHANNELS = [
    ['rx', 'tx', -40], ['ry', 'ty', 0], ['face', 'sx', 1], ['lean', 'rot', 0], ['head', 'rot', 0],
    ['armF', 'rot', REST.armF], ['elbF', 'rot', REST.elbF], ['armB', 'rot', REST.armB], ['elbB', 'rot', REST.elbB],
    ['legF', 'rot', 0], ['kneeF', 'rot', 0], ['legB', 'rot', 0], ['kneeB', 'rot', 0],
    ['gaze', 'txy', { x: 2.4, y: 0 }], ['eyeOpen', 'op', 1], ['eyeHappy', 'op', 0], ['eyeClosed', 'op', 0],
    ['eyeWide', 'op', 0], ['bulb', 'op', 1]
  ];
  const PARTS = new Set(['base', 'root', 'body', 'head', 'handF', 'handB']);
  const joint = (x, y, channel, body, extra = '') => `<g transform="translate(${x} ${y})"><g data-c="${channel}"${extra}>${body}</g></g>`;
  // A chunky, big-headed android in dark gunmetal: rim-lit capsule limbs, a wide
  // visor with red LEDs, a glowing core and neon soles. Caricature first, detail last.
  const bone = length => `<path class="wv-limb-edge" d="M0 0V${length}"/><path class="wv-limb" d="M0 0V${length}"/>`;
  const foot = '<path class="wv-boot" d="M-2.9 5.3h3.8q4.3.4 4.7 2.9v1.3h-8.5Z"/><path class="wv-sole" d="M-2.4 9.5h7.4"/>';
  const leg = side => joint(side === 'F' ? 1.6 : -1.6, HIP_Y, `leg${side}`,
    `${bone(7.5)}${joint(0, 7.5, `knee${side}`, `${bone(6)}${foot}<circle class="wv-joint" r="1.3"/>`)}`,
    side === 'B' ? ' class="wv-far"' : '');
  const arm = side => joint(side === 'F' ? 1 : -1, SHOULDER_Y, `arm${side}`,
    `${bone(UPPER)}${joint(0, UPPER, `elb${side}`, `${bone(FORE)}<circle class="wv-hand" cy="${FORE + 0.4}" r="2.1"/><circle class="wv-joint" r="1.15"/>`)}<circle class="wv-pad" r="2.5"/><path class="wv-shine" d="M-1.6 -1.3a2.1 2.1 0 0 1 2.2-.9"/>`,
    side === 'B' ? ' class="wv-far"' : '');
  // Red LEDs behind one dark visor; gaze slides them as a three-quarter turn.
  const eyes = `<g data-c="bulb"><rect class="wv-eye-glow" x="-5.6" y="-2.3" width="5.6" height="4.6" rx="2.3"/><rect class="wv-eye-glow" x="0" y="-2.3" width="5.6" height="4.6" rx="2.3"/></g>
    <g data-c="eyeOpen"><g class="wv-blink"><rect class="wv-eye" x="-4.2" y="-1" width="3" height="2" rx="1"/><rect class="wv-eye" x="1.2" y="-1" width="3" height="2" rx="1"/></g></g>
    <g data-c="eyeHappy"><path class="wv-eye-line" d="M-4.2 1l1.5-1.8 1.5 1.8M1.2 1l1.5-1.8 1.5 1.8"/></g>
    <g data-c="eyeClosed"><path class="wv-eye-line" d="M-4.2 .3h3M1.2 .3h3"/></g>
    <g data-c="eyeWide"><circle class="wv-eye" cx="-2.7" r="1.75"/><circle class="wv-eye" cx="2.7" r="1.75"/></g>`;
  const head = `<rect class="wv-neck" x="-2.4" y="-2" width="4.8" height="3.6" rx="1"/>
    <path class="wv-antenna" d="M0 -17.4v-3.4"/><circle class="wv-antenna-tip" cy="-21.6" r="1.2"/>
    <path class="wv-shell" d="M-10.2 -9.6C-10.2 -15.3-5.8-17.6 0-17.6S10.2-15.3 10.2-9.6v3.4c0 3.1-2.2 5-5.2 5h-10c-3 0-5.2-1.9-5.2-5Z"/>
    <path class="wv-shine" d="M-7.6 -14.2q3-2.5 7.2-2.6"/>
    <circle class="wv-pod" cx="-10.2" cy="-8.6" r="2"/><circle class="wv-pod" cx="10.2" cy="-8.6" r="2"/>
    <circle class="wv-pod-light" cx="-10.2" cy="-8.6" r=".7"/><circle class="wv-pod-light" cx="10.2" cy="-8.6" r=".7"/>
    <rect class="wv-visor" x="-8.6" y="-12.4" width="17.2" height="7.2" rx="3.6"/><path class="wv-visor-shine" d="M-6.4 -11.4h5.4"/>
    <path class="wv-grille" d="M-2.6 -3.3h5.2"/>
    ${joint(0, -8.8, 'gaze', eyes)}`;
  // The near arm is drawn as a second copy of the same transform chain above the
  // front props, so anything held in that hand sits between the body and the fingers.
  const ROBOT = `<g class="wv-robot" data-c="rx"><ellipse class="wv-floor-glow" cy=".6" rx="14" ry="2.6"/><ellipse class="wv-shadow" cy=".5" rx="9.5" ry="1.7"/><g data-c="ry"><g data-c="face">
    ${leg('B')}
    ${joint(0, HIP_Y, 'lean', `${arm('B')}
      <rect class="wv-waist" x="-3.6" y="-6" width="7.2" height="6" rx="1.6"/>
      <path class="wv-shell" d="M-7.4 -11.2q0-2.8 2.8-2.8h9.2q2.8 0 2.8 2.8l-.9 5.4q-6.4 2.9-12.8 0Z"/>
      <path class="wv-shine" d="M-5.2 -12.6h4.6"/>
      <path class="wv-shell wv-belt" d="M-5.8 -1.8h11.6l-1.3 3.5h-9Z"/>
      <circle class="wv-core-glow" cx="-3" cy="-7.4" r="2.4"/><circle class="wv-core" cx="-3" cy="-7.4" r="1.3"/>
      ${joint(0, NECK_Y, 'head', head)}`)}
    ${leg('F')}
  </g></g></g>`;
  const ROBOT_ARM = `<g class="wv-robot" data-c="rx"><g data-c="ry"><g data-c="face">${joint(0, HIP_Y, 'lean', arm('F'))}</g></g></g>`;

  // ------------------------------------------------------------------ props
  // Prop-local units: origin at the floor contact (or the grip for hand tools).
  // Sub-channels are [kind, initial]. half is the horizontal reach used to keep
  // entrances and exits outside the round window.
  // Comic glyphs: filled shapes and strokes, outlined in ink like the lettering.
  const FX_ICONS = Object.freeze({
    '♥': { fill: ['M0 2.5C-2.7.7-3.3-.6-3.3-1.6a1.75 1.75 0 0 1 3.3-.8 1.75 1.75 0 0 1 3.3.8c0 1-.6 2.3-3.3 4.1Z'] },
    '✦': { fill: ['M0-3.3.9-.9 3.3 0 .9.9 0 3.3-.9.9-3.3 0-.9-.9Z'] },
    '♪': { line: ['M1 1.5V-2.9l2 .7'], dots: [[-0.2, 1.6, 1.25]] },
    '!': { line: ['M0-2.9v3.1'], dots: [[0, 2.3, 0.75]] },
    '?': { line: ['M-1.5-1.6a1.55 1.55 0 1 1 2.3 1.3c-.6.3-.8.6-.8 1.2'], dots: [[0, 2.3, 0.75]] },
    '✓': { line: ['M-2.3 0-.6 1.7 2.4-1.7'] }
  });
  // Long lines break at the space that best balances two rows.
  function rows(text) {
    if ([...text].length <= 10 || !text.includes(' ')) return [text];
    const words = text.split(' ');
    let best = [text], widest = Infinity;
    for (let i = 1; i < words.length; i++) {
      const pair = [words.slice(0, i).join(' '), words.slice(i).join(' ')];
      const longest = Math.max(...pair.map(row => [...row].length));
      if (longest < widest) { widest = longest; best = pair; }
    }
    return best;
  }
  // tone: 'pow' (warm comic yellow, for exclamations), 'love' (pink) or 'neon' (the room's accent).
  function comic(text, angle = -7, tone = 'neon') {
    const icon = FX_ICONS[text], lines = icon ? [text] : rows(text), two = lines.length > 1;
    const longest = Math.max(...lines.map(line => [...line].length));
    const width = icon ? 12 : longest * (two ? 5.5 : 6.4) + 2, height = icon ? 12 : lines.length * 10 + 1;
    // Short action strokes radiate from above and beside the lettering.
    const burst = [-165, -138, -112, -68, -42, -15].map(degrees => {
      const a = degrees * Math.PI / 180, x = Math.cos(a), y = Math.sin(a);
      const r = 1 / Math.sqrt((x / (width / 2 + 2.2)) ** 2 + (y / (height / 2 + 1.6)) ** 2);
      return `M${n3(x * r)} ${n3(y * r)}l${n3(x * 3.2)} ${n3(y * 3.2)}`;
    }).join('');
    const body = icon
      ? `<g transform="scale(2.1)">${(icon.line || []).map(d => `<path class="wv-fx-ink-line" d="${d}"/>`).join('')}
          ${(icon.fill || []).map(d => `<path class="wv-fx-shape" d="${d}"/>`).join('')}${(icon.line || []).map(d => `<path class="wv-fx-line" d="${d}"/>`).join('')}
          ${(icon.dots || []).map(([cx, cy, r]) => `<circle class="wv-fx-shape" cx="${cx}" cy="${cy}" r="${r}"/>`).join('')}</g>`
      : lines.map((line, i) => {
        const y = (i - (lines.length - 1) / 2) * 10;
        return `<text class="wv-fx-drop" x="1.1" y="${y + 1.2}" text-anchor="middle" dominant-baseline="central">${line}</text>
          <text class="wv-fx-text" y="${y}" text-anchor="middle" dominant-baseline="central">${line}</text>`;
      }).join('');
    return `<g class="wv-fx wv-fx-${tone}${two ? ' wv-fx-two' : ''}" transform="translate(0 ${two ? -15 : -10}) rotate(${angle})"><g class="wv-unmirror"><path class="wv-fx-burst" d="${burst}"/>${body}</g></g>`;
  }
  // Digital rain inside a screen: columns of tiny cells whose bright head falls row by row.
  function rain(x, y, width, height, columns, seed) {
    const rows = Math.max(2, Math.floor(height / 2.3));
    let cells = '';
    for (let column = 0; column < columns; column++) {
      const cx = x + (column + 0.5) * width / columns, start = ((seed * 7 + column * 13) % 10) / 10 * 1.6;
      for (let row = 0; row < rows; row++) {
        const phase = (((start - row * 0.11) % 1.6) + 1.6) % 1.6;
        cells += `<rect x="${n3(cx - 0.35)}" y="${n3(y + 0.4 + row * 2.3)}" width=".7" height="1.3" style="animation-delay:-${n3(phase)}s"/>`;
      }
    }
    return `<g class="wv-matrix">${cells}</g>`;
  }
  const wheel = (id, part, x, y, r) => `<g transform="translate(${x} ${y})"><g data-c="${id}.${part}"><circle class="wv-wheel" r="${r}"/><path class="wv-spoke" d="M${-r * 0.6} 0h${r * 1.2}"/></g></g>`;
  const wheels = (r, ...parts) => ({ r, parts });
  const ART = Object.freeze({
    cart: { half: 23, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.4, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="17" ry="1.5"/><path class="wv-line" d="M-11 -13.4V-2.4M11 -13.4V-2.4M-17 -15l-2.6-12h-2.6"/>
        <rect class="wv-prop" x="-17" y="-16" width="34" height="2.8" rx="1"/><path class="wv-neon" d="M-14 -12.6h28"/>
        ${wheel(id, 'wl', -11, -2.4, 2.4)}${wheel(id, 'wr', 11, -2.4, 2.4)}` },
    machine: { half: 9, ch: { light: ['op', 0.2], drip: ['op', 0] },
      svg: id => `<rect class="wv-prop" x="-4.5" y="-17" width="11" height="17" rx="1.8"/><path class="wv-prop" d="M-4.5 -14.5h-4.4v2.6h4.4"/>
        <path class="wv-line" d="M-7.4 -11.9v1.1"/><rect class="wv-inset" x="-2.2" y="-10.5" width="6.6" height="4.6" rx="1"/><path class="wv-neon" d="M-.8 -8.2h3.8"/>
        <path class="wv-coffee" data-c="${id}.drip" d="M-7.4 -10.4V-6"/><circle class="wv-dot" data-c="${id}.light" cx="-6.6" cy="-15.6" r="1"/>` },
    mug: { half: 5, ch: { steam: ['op', 0] },
      svg: id => `<g class="wv-steam" data-c="${id}.steam"><path d="M-.9 -7.4c-.8-1 .8-1.9 0-2.9"/><path d="M1.2 -7.8c-.8-1 .8-1.9 0-2.9"/></g>
        <path class="wv-line" d="M-2.6 -4.6h-1.2a1.2 1.2 0 0 0 0 2.4h1.2"/>
        <path class="wv-mug" d="M-2.6 -5.8h5.2v4.3a1.5 1.5 0 0 1-1.5 1.5h-2.2a1.5 1.5 0 0 1-1.5-1.5Z"/>` },
    sheet: { half: 5,
      svg: () => `<rect class="wv-paper" x="-4.5" y="-11.5" width="9" height="11.5" rx=".8"/><path class="wv-paper-lines" d="M-2.6 -9h5.2M-2.6 -6.8h5.2M-2.6 -4.6h3.4"/>` },
    tray: { half: 6,
      svg: () => `<rect class="wv-prop" x="-5.5" y="-7" width="11" height="7" rx="1.2"/><path class="wv-neon" d="M-4.2 -6.9h8.4"/>` },
    // Comic onomatopoeia: no balloon, just outlined lettering with action strokes.
    // The origin is the point it bursts out of (above the head or a prop).
    bubble: { half: 34, svg: (id, p) => comic(p.text, p.angle, p.tone) },
    zzz: { half: 12,
      svg: () => `<g class="wv-unmirror"><g class="wv-zzz"><path d="M0 -3h3l-3 3h3"/><path d="M4 -9h3.6l-3.6 3.6h3.6"/><path d="M9 -16h4.4l-4.4 4.4h4.4"/></g></g>` },
    cushion: { half: 15,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="14" ry="1.4"/><path class="wv-cushion" d="M-14 0c-1.6-6 3-10 14-10s15.6 4 14 10Z"/><path class="wv-neon wv-dim" d="M-7 -6.5q7 2.4 14 0"/>` },
    board: { half: 20, ch: { wl: ['rot', 0], wr: ['rot', 0], ax: ['dash', 1], b1: ['dash', 1], b2: ['dash', 1], b3: ['dash', 1], tr: ['dash', 1] },
      wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="15" ry="1.5"/><path class="wv-line" d="M-12 -17V-2.3M12 -17V-2.3M-17 -26h-2.6"/>
        <rect class="wv-board" x="-17" y="-42" width="34" height="25" rx="1.8"/><rect class="wv-board-edge" x="-15.4" y="-40.4" width="30.8" height="21.8" rx=".8"/>
        <path class="wv-line" d="M-15 -17.4h30"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.ax" d="M-11 -38v17h22"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b1" d="M-7.5 -21v-5.5h3.6v5.5"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b2" d="M-1.8 -21v-9.5h3.6v9.5"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b3" d="M3.9 -21v-13h3.6v13"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.tr" d="M-6 -30.5 0 -34.5l5.8-4.6m-2.6-.2 2.6.2-.4 2.6"/>
        ${wheel(id, 'wl', -12, -2.3, 2.3)}${wheel(id, 'wr', 12, -2.3, 2.3)}` },
    stand: { half: 15, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="11" ry="1.4"/><path class="wv-line" d="M-8 -4.4h16M0 -4.4V-22M-8 -4.4l-3.5-16h-2.6M-9 -27h-3v-3"/>
        <rect class="wv-paper" x="-8.5" y="-45" width="17" height="22" rx="1"/>
        <path class="wv-paper-lines" d="M-5.4 -41h10.8M-5.4 -37.8h10.8M-5.4 -34.6h7M-5.4 -31.4h10.8M-5.4 -28.2h6"/>
        ${wheel(id, 'wl', -6.5, -2.2, 2.2)}${wheel(id, 'wr', 6.5, -2.2, 2.2)}` },
    lens: { half: 12,
      svg: () => `<path class="wv-lens-handle" d="M0 0l4.4-4.4"/><circle class="wv-lens" cx="7.4" cy="-7.4" r="4.2"/><path class="wv-glint" d="M5.3 -8.4a2.6 2.6 0 0 1 2-2.1"/>` },
    cabinet: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0], led: ['op', 0.2] }, wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="11" ry="1.4"/><path class="wv-line" d="M-10 -24h-2.8v-6h2.8"/>
        <rect class="wv-prop" x="-10" y="-33" width="20" height="28.4" rx="1.6"/>
        <path class="wv-line wv-soft" d="M-10 -24h20M-10 -14.6h20"/><path class="wv-neon wv-dim" d="M-6.5 -28.6h9M-6.5 -19.4h9M-6.5 -10h9"/>
        <path class="wv-slot" d="M-5.5 -33h11"/><circle class="wv-dot" data-c="${id}.led" cx="6.6" cy="-30" r="1"/>
        ${wheel(id, 'wl', -6.5, -2.3, 2.3)}${wheel(id, 'wr', 6.5, -2.3, 2.3)}` },
    folder: { half: 7,
      svg: () => `<rect class="wv-chip" x="-5.6" y="-10.4" width="11.2" height="10.4" rx="1.6"/><path class="wv-neon" d="M-3.2 -7.6h6.4M-3.2 -5h6.4M-3.2 -2.4h3.8"/>` },
    printer: { half: 10, ch: { led: ['op', 0.2] },
      svg: id => `<rect class="wv-prop" x="-6" y="-15.5" width="12" height="3.5" rx="1"/><rect class="wv-prop" x="-9" y="-12.5" width="18" height="12.5" rx="2"/>
        <path class="wv-slot" d="M-5 -12.4h10"/><rect class="wv-inset" x="-5.5" y="-8" width="7" height="3.4" rx=".8"/>
        <circle class="wv-dot" data-c="${id}.led" cx="5.4" cy="-6.4" r="1"/>` },
    frame: { half: 12,
      svg: () => `<rect class="wv-prop" x="-11.5" y="-12.5" width="23" height="12.5" rx="1.4"/>
        <rect class="wv-socket" x="-9.4" y="-9.6" width="5.6" height="5.6" rx=".9"/><rect class="wv-socket" x="-2.8" y="-9.6" width="5.6" height="5.6" rx=".9"/>
        <rect class="wv-socket" x="3.8" y="-9.6" width="5.6" height="5.6" rx=".9"/>` },
    block: { half: 3,
      svg: (id, p) => `<rect class="wv-block wv-block-${p.tone || 1}" x="-2.6" y="-2.6" width="5.2" height="5.2" rx=".9"/>` },
    terminal: { half: 13, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="10" ry="1.4"/><path class="wv-line" d="M-7 -4.4h14M0 -4.4V-24M-8 -31h-3.4v8"/>
        <rect class="wv-prop" x="-8.5" y="-40" width="17" height="16" rx="2"/><rect class="wv-screen" x="-6.4" y="-38" width="12.8" height="9" rx="1"/>
        <path class="wv-lock" d="M-1.8 -33v-1.7a1.8 1.8 0 0 1 3.6 0v1.7"/><rect class="wv-lock-body" x="-2.6" y="-33.2" width="5.2" height="3.4" rx=".7"/>
        <circle class="wv-button" cx="-4" cy="-26.4" r="1.2"/><circle class="wv-led-wait" cx="4" cy="-26.4" r="1.1"/>
        ${wheel(id, 'wl', -5, -2.2, 2.2)}${wheel(id, 'wr', 5, -2.2, 2.2)}` },
    radio: { half: 9, ch: { notes: ['op', 0] },
      svg: id => `<path class="wv-line" d="M-4.5 -9v-2.6h9V-9M5.4 -9l3-6"/><rect class="wv-prop" x="-7.5" y="-9" width="15" height="9" rx="2"/>
        <circle class="wv-speaker" cx="-3" cy="-4.5" r="2.6"/><path class="wv-line wv-soft" d="M1.8 -6.5h3.6M1.8 -4.5h3.6M1.8 -2.5h3.6"/>
        <g transform="translate(-2 -14)"><g data-c="${id}.notes"><g class="wv-unmirror">${[[0, 0, ''], [7, -4, ' wv-note-b'], [-6, -7, ' wv-note-c']].map(([x, y, late]) =>
          `<g transform="translate(${x} ${y})"><g class="wv-note${late}"><path class="wv-note-stem" d="M1.6 0V-5.2l2.4.8"/><ellipse class="wv-note-head" cx=".5" cy=".1" rx="1.35" ry="1"/></g></g>`).join('')}</g></g></g>` },
    ball: { half: 3,
      svg: (id, p) => `<circle class="wv-ball wv-ball-${p.tone || 1}" r="2.3"/><path class="wv-ball-shine" d="M-1 -1.2a1.4 1.4 0 0 1 1.1-.7"/>` },
    plane: { half: 8, ch: { flat: ['op', 1], fold: ['op', 0] },
      svg: id => `<g data-c="${id}.flat"><rect class="wv-paper" x="-4" y="-5.5" width="8" height="11" rx=".7"/><path class="wv-paper-lines" d="M-2.2 -3h4.4M-2.2 -.8h4.4"/></g>
        <g data-c="${id}.fold"><path class="wv-paper" d="M-6 0 6.5-1.4-6-4.6-3.2-1.4Z"/><path class="wv-line wv-soft" d="M-3.2 -1.4 6.5-1.4M-6 0l2.8-1.4"/></g>` },
    skate: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(1.6, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="11" ry="1.3"/><path class="wv-deck" d="M-12.5 -5.6q.6 1.8 2.6 1.8h19.8q2 0 2.6-1.8"/><path class="wv-neon" d="M-9 -2.8h18"/>
        ${wheel(id, 'wl', -6.5, -1.6, 1.6)}${wheel(id, 'wr', 6.5, -1.6, 1.6)}` },
    broom: { half: 8, ch: { dust: ['op', 0] },
      svg: id => `<path class="wv-stick" d="M0 -3V17"/><path class="wv-bristle" d="M-3.6 16.6h7.2l1.6 5.4h-10.4Z"/><path class="wv-line wv-soft" d="M-1.8 18v3.6M0 18v3.6M1.8 18v3.6"/>
        <g data-c="${id}.dust" class="wv-dust"><circle cx="-6" cy="21" r="1.6"/><circle cx="7" cy="20.4" r="1.3"/><circle cx="-9" cy="19" r="1"/></g>` },
    can: { half: 13, ch: { drops: ['op', 0] },
      svg: id => `<path class="wv-line" d="M-3.4 3.4q3.4-6.4 6.8 0"/><path class="wv-prop" d="M-4.4 3h9.4l-.8 7.4h-7.8Z"/><path class="wv-prop" d="M4.8 5.2 11.4 1l.8 1.2-6.8 5"/>
        <g data-c="${id}.drops" class="wv-drops"><circle cx="13" cy="4" r=".75"/><circle cx="12.2" cy="6" r=".65"/><circle cx="13.6" cy="7.2" r=".7"/></g>` },
    plant: { half: 18, ch: { wl: ['rot', 0], wr: ['rot', 0], l3: ['scale', 0], fl: ['scale', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="13" ry="1.4"/><path class="wv-line" d="M-12 -5.2l-2.4-11.6h-2.6"/>
        <rect class="wv-prop" x="-12" y="-6.4" width="24" height="2" rx=".8"/>
        <g transform="translate(4 0)"><path class="wv-stem" d="M0 -16V-27"/>
          <g transform="translate(0 -19)"><path class="wv-leaf" d="M0 0q-6.4-.6-7.6-5.4 5.4-.2 7.6 5.4Z"/></g>
          <g transform="translate(0 -22.5)"><path class="wv-leaf" d="M0 0q6.4-.6 7.6-5.4-5.4-.2-7.6 5.4Z"/></g>
          <g transform="translate(0 -25.6)"><g data-c="${id}.l3"><path class="wv-leaf" d="M0 0q-5.4-.6-6.4-4.6 4.6-.2 6.4 4.6Z"/></g></g>
          <g transform="translate(0 -28)"><g data-c="${id}.fl"><circle class="wv-petal" cx="-2" r="1.9"/><circle class="wv-petal" cx="2" r="1.9"/><circle class="wv-petal" cy="-2" r="1.9"/><circle class="wv-petal" cy="2" r="1.9"/><circle class="wv-flower" r="1.4"/></g></g>
          <path class="wv-pot" d="M-5.4 -16h10.8l-1.6 9.6h-7.6Z"/><path class="wv-neon" d="M-4.6 -13.6h9.2"/></g>
        ${wheel(id, 'wl', -8, -2.2, 2.2)}${wheel(id, 'wr', 8, -2.2, 2.2)}` },
    box: { half: 10,
      svg: () => `<rect class="wv-box" x="-9" y="-15" width="18" height="15" rx="1.4"/><path class="wv-line wv-soft" d="M-9 -11.5h18"/><path class="wv-neon" d="M-6 -6h12"/>` },
    // ---- tech set: desk, chair, ceiling screens, SOC crew, rack, hologram
    pcdesk: { half: 18, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="16" ry="1.5"/><path class="wv-line" d="M-14 -13.6V-2.2M14 -13.6V-2.2"/><path class="wv-line wv-soft" d="M-14 -7h28"/>
        <rect class="wv-prop" x="5.5" y="-12.6" width="7" height="10" rx="1"/><circle class="wv-tower-led" cx="9" cy="-10.6" r=".6"/>
        <rect class="wv-prop" x="-17" y="-16" width="34" height="2.4" rx="1"/>
        <rect class="wv-keys" x="-9.5" y="-17.5" width="12" height="1.5" rx=".5"/><path class="wv-neon wv-dim" d="M-8.5 -16.75h10"/><ellipse class="wv-prop" cx="6" cy="-16.6" rx="1.5" ry=".8"/>
        ${wheel(id, 'wl', -14, -2.2, 2.2)}${wheel(id, 'wr', 14, -2.2, 2.2)}` },
    chair: { half: 10,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="7" ry="1.2"/><path class="wv-line" d="M0 -8.4V-2.6M-4.6 -2.4h9.2"/>
        <circle class="wv-wheel" cx="-4.2" cy="-1.3" r="1.2"/><circle class="wv-wheel" cx="4.2" cy="-1.3" r="1.2"/>
        <rect class="wv-prop" x="-6.5" y="-10" width="13" height="1.8" rx=".9"/><rect class="wv-chair-back" x="-8.6" y="-23" width="2.8" height="14" rx="1.2"/>` },
    // A wall screen hung from the ceiling with a live attack map (origin: bottom edge).
    dash: { half: 28, ch: { alert: ['op', 0], lock: ['op', 0] },
      svg: id => `<path class="wv-line wv-soft" d="M-20 -36V-150M20 -36V-150"/>
        <rect class="wv-screen-bg" x="-27" y="-36" width="54" height="36" rx="1.6"/>
        <path class="wv-screen-grid" d="M-25 -27h50M-25 -18h50M-25 -9h50M-13 -34v32M0 -34v32M13 -34v32"/>
        <path class="wv-land" d="M-23 -27q3-5 9-3t6 4-3 5-8 1-4-7ZM-4 -30q6-3 11 0t3 6-6 3-7-3-1-6ZM9 -20q5-2 9 1t1 6-6 1-4-8ZM-17 -14q3 2 2 6t-4 2-1-8Z"/>
        <path class="wv-arc" pathLength="1" d="M-16 -24Q-6 -38 3 -26"/><path class="wv-arc wv-arc-b" pathLength="1" d="M3 -26Q13 -36 16 -15"/><path class="wv-arc wv-arc-c" pathLength="1" d="M-12 -9Q1 -21 16 -15"/>
        <circle class="wv-pulse" cx="16" cy="-15" r="1.7"/><circle class="wv-node" cx="-16" cy="-24" r=".9"/><circle class="wv-node" cx="3" cy="-26" r=".9"/><circle class="wv-node wv-node-hot" cx="16" cy="-15" r="1"/><circle class="wv-node" cx="-12" cy="-9" r=".9"/>
        <path class="wv-screen-text" d="M-24 -33.6h9M-24 -32h5M19 -33.6h5"/>
        <rect class="wv-bar" x="20" y="-8" width="1.6" height="5"/><rect class="wv-bar wv-bar-b" x="22.4" y="-8" width="1.6" height="5"/>
        <g data-c="${id}.alert"><rect class="wv-alert-frame" x="-26.3" y="-35.3" width="52.6" height="34.6" rx="1.2"/><path class="wv-alert-icon" d="M-1 -23l3.6 6.2h-7.2Z"/><path class="wv-alert-mark" d="M-1 -21.4v2.2"/></g>
        <g data-c="${id}.lock"><rect class="wv-lock-ok-body" x="-3.4" y="-21" width="6.8" height="5" rx="1"/><path class="wv-lock-ok" d="M-2.2 -21v-1.8a2.2 2.2 0 0 1 4.4 0v1.8"/></g>
        <rect class="wv-handle" x="-3" y="-.6" width="6" height="1.2" rx=".6"/>` },
    // A SOC wall: six monitors of digital rain hanging from the ceiling (origin: bottom edge).
    socwall: { half: 34, ch: { alert: ['op', 0] },
      svg: id => {
        const screens = [[-33, -40], [-11, -40], [11, -40], [-33, -20.5], [-11, -20.5], [11, -20.5]]
          .map(([x, y], i) => `<rect class="wv-screen-bg" x="${x}" y="${y}" width="21" height="18.5" rx="1"/>${rain(x + 1, y + 1, 19, 16.5, 6, i + 1)}`).join('');
        return `<path class="wv-line wv-soft" d="M-26 -40V-150M26 -40V-150"/><rect class="wv-prop" x="-34.5" y="-41.5" width="69" height="40" rx="1.6"/>${screens}
          <g data-c="${id}.alert"><rect class="wv-alert-frame" x="-10.6" y="-39.6" width="20.2" height="17.7" rx=".8"/><path class="wv-alert-icon" d="M-.5 -36l4.2 7.2h-8.4Z"/><path class="wv-alert-mark" d="M-.5 -34.2v2.6"/></g>`;
      } },
    // Two operators seen from behind on rolling chairs; heads and a thumb can move.
    // Drawn 30% larger than the robot: they sit closer to the viewer.
    crew: { half: 26, ch: { h1: ['rot', 0], h2: ['rot', 0], thumb: ['rot', 60], thumbo: ['op', 0] },
      svg: id => [[-13, 1], [13, 2]].map(([x, i]) => `<g transform="translate(${x} 0) scale(1.3)">
          <ellipse class="wv-shadow" cy=".4" rx="7" ry="1.2"/><path class="wv-line" d="M0 -8.4V-2.6M-4.6 -2.4h9.2"/>
          <circle class="wv-wheel" cx="-4.2" cy="-1.3" r="1.2"/><circle class="wv-wheel" cx="4.2" cy="-1.3" r="1.2"/>
          <path class="wv-op" d="M-6.8 -10c0-7.6 2-12.6 6.8-12.6s6.8 5 6.8 12.6Z"/><path class="wv-op-rim" d="M-5.2 -19q5.2-4.6 10.4 0"/>
          <g transform="translate(0 -26)"><g data-c="${id}.h${i}"><circle class="wv-op" r="4.2"/><path class="wv-op-rim" d="M-3.4 -2.4a4.2 4.2 0 0 1 6.8 0"/><path class="wv-headset" d="M-4.6 0a4.6 4.6 0 0 1 9.2 0"/><circle class="wv-op-ear" cx="4.5" cy=".4" r="1.2"/></g></g>
          ${i === 2 ? `<g transform="translate(6 -19)"><g data-c="${id}.thumbo"><g data-c="${id}.thumb"><path class="wv-op-arm" d="M0 0 2.6-6.4"/><circle class="wv-op" cx="2.9" cy="-7.4" r="1.4"/><path class="wv-op-rim" d="M2.6 -9.4v-1.4"/></g></g></g>` : ''}
          <rect class="wv-chair-back" x="-5.6" y="-16" width="11.2" height="6.6" rx="2"/><rect class="wv-prop" x="-6.5" y="-10" width="13" height="1.8" rx=".9"/>
        </g>`).join('') + `<path class="wv-line wv-soft" d="M-19 -3.1h38"/>` },
    // Architecture whiteboard: boxes, arrows, a database, a cloud and a flow (origin: floor).
    arch: { half: 26, ch: { wl: ['rot', 0], wr: ['rot', 0], b1: ['dash', 1], a1: ['dash', 1], b2: ['dash', 1], a2: ['dash', 1], db: ['dash', 1], a3: ['dash', 1], cl: ['dash', 1], fl: ['dash', 1], ci: ['dash', 1] },
      wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="18" ry="1.5"/><path class="wv-line" d="M-15 -18V-2.3M15 -18V-2.3M-22 -30h-2.6"/>
        <rect class="wv-board" x="-22" y="-46" width="44" height="28" rx="1.8"/><rect class="wv-board-edge" x="-20.4" y="-44.4" width="40.8" height="24.8" rx=".8"/>
        <path class="wv-line" d="M-20 -18.4h40"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b1" d="M-18 -42h9v6h-9Z"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.a1" d="M-8.4 -39h5m-1.6-1.4 1.6 1.4-1.6 1.4"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b2" d="M-3 -42h9v6h-9Z"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.a2" d="M1.5 -35.4v3.6m-1.3-1.4 1.3 1.4 1.3-1.4"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.db" d="M-2.5 -29.4a4 1.3 0 0 0 8 0 4 1.3 0 0 0-8 0v5a4 1.3 0 0 0 8 0v-5"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.a3" d="M6.6 -39h4.6m-1.6-1.4 1.6 1.4-1.6 1.4"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.cl" d="M12.4 -36h7.2a2.2 2.2 0 0 0-.6-4.3 3 3 0 0 0-5.6-.8 2.4 2.4 0 0 0-1 5.1Z"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.fl" d="M16 -35.4c0 6-4 8.6-9.6 8.6"/>
        <path class="wv-chalk wv-chalk-hot" pathLength="1" data-c="${id}.ci" d="M-5.8 -39a7.3 4.6 0 1 0 14.6 0 7.3 4.6 0 1 0-14.6 0"/>
        ${wheel(id, 'wl', -15, -2.3, 2.3)}${wheel(id, 'wr', 15, -2.3, 2.3)}` },
    // Server rack: running LEDs; one blade slides out and shows a fault.
    rack: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0], blade: ['tx', 0], bad: ['op', 0] }, wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => {
        const unit = (y, leds, delay) => `<rect class="wv-unit" x="-8.4" y="${y}" width="16.8" height="5.6" rx=".6"/>${[0, 1, 2, 3].map(i =>
          `<rect class="${leds}" x="${2 + i * 1.6}" y="${y + 2.2}" width=".9" height="1.2" style="animation-delay:-${n3(((delay + i) * 0.17) % 1.2)}s"/>`).join('')}<path class="wv-line wv-soft" d="M-6.6 ${y + 2.8}h6"/>`;
        return `<ellipse class="wv-shadow" cy=".5" rx="11" ry="1.4"/><path class="wv-line" d="M-10 -24h-2.8v-6h2.8"/>
          <rect class="wv-prop" x="-10" y="-46" width="20" height="41.4" rx="1.6"/>
          ${[-44, -37, -23, -16, -9].map((y, i) => unit(y, 'wv-led-run', i * 2)).join('')}
          <g data-c="${id}.blade">${unit(-30, 'wv-led-run', 5)}<rect class="wv-handle" x="-10.4" y="-28.4" width="1.6" height="2.4" rx=".5"/>
            <g data-c="${id}.bad">${[0, 1, 2, 3].map(i => `<rect class="wv-led-bad" x="${2 + i * 1.6}" y="-27.8" width=".9" height="1.2"/>`).join('')}</g></g>
          ${wheel(id, 'wl', -6.5, -2.3, 2.3)}${wheel(id, 'wr', 6.5, -2.3, 2.3)}`;
      } },
    // A hologram projector puck; the beam and the globe open above it.
    puck: { half: 12, ch: { beam: ['op', 0], globe: ['scale', 0] },
      svg: id => `<g data-c="${id}.beam"><path class="wv-beam" d="M-4.6 -2.6-11.5-26h23L4.6-2.6Z"/></g>
        <g transform="translate(0 -27)"><g data-c="${id}.globe"><circle class="wv-holo" r="10.5"/>
          ${[0, 1, 2, 3].map(i => `<ellipse class="wv-holo-line wv-meridian" rx="10.5" ry="10.5" style="animation-delay:-${i * 0.75}s"/>`).join('')}
          <path class="wv-holo-line" d="M-10 -3.4h20M-9 3.4h18M-6.6 -7.8h13.2M-6.6 7.8h13.2"/>
          <g class="wv-orbit"><circle class="wv-holo-dot" cx="13.5" r=".9"/><circle class="wv-holo-dot" cx="-12" cy="4" r=".7"/></g></g></g>
        <ellipse class="wv-puck" cy="-1.6" rx="5.6" ry="1.9"/><path class="wv-neon" d="M-3.8 -1.7h7.6"/>` }
  });

  // ------------------------------------------------------------------ skit compiler
  const STEP = 1000 / 30, EPS = 1e-6;
  const clone = value => value && typeof value === 'object' ? { ...value } : value;
  const merge = (current, value) => current && typeof current === 'object' ? { ...current, ...value } : value;
  const lerp = (a, b, k) => typeof a === 'number' ? a + (b - a) * k
    : Object.fromEntries(Object.keys(a).map(name => [name, a[name] + ((b[name] ?? a[name]) - a[name]) * k]));
  const same = (a, b) => typeof a === 'number' ? Math.abs(a - b) < 1e-4 : Object.keys(a).every(name => Math.abs(a[name] - b[name]) < 1e-4);
  const n3 = value => Math.round(value * 1000) / 1000;
  const FORMAT = Object.freeze({
    tx: v => ({ transform: `translate(${n3(v)}px, 0px)` }),
    ty: v => ({ transform: `translate(0px, ${n3(v)}px)` }),
    sx: v => ({ transform: `scale(${n3(v)}, 1)` }),
    rot: v => ({ transform: `rotate(${n3(v)}deg)` }),
    scale: v => ({ transform: `scale(${n3(v)})` }),
    txy: v => ({ transform: `translate(${n3(v.x)}px, ${n3(v.y)}px)` }),
    pose: v => ({ transform: `translate(${n3(v.x)}px, ${n3(v.y)}px) rotate(${n3(v.r)}deg) scale(${n3(v.sx)}, ${n3(v.sy)})` }),
    op: v => ({ opacity: String(n3(v)) }),
    dash: v => ({ strokeDashoffset: String(n3(v)) })
  });
  const targetOf = (name, kind) => kind === 'op' && name.endsWith('.o') ? name.slice(0, -2) : name;

  function createClip(props) {
    const channels = new Map(), attachments = [], meta = new Map();
    let t = 0;
    const define = (name, kind, init) => channels.set(name, { name, kind, init: clone(init), cur: clone(init), frames: [] });
    for (const [name, kind, init] of ROBOT_CHANNELS) define(name, kind, init);
    for (const prop of props) {
      const art = ART[prop.art];
      if (!art) throw Error(`Arte desconhecida: ${prop.art}`);
      meta.set(prop.id, { ...prop, art });
      define(prop.id, 'pose', { x: prop.x ?? -90, y: prop.y ?? 0, r: prop.r ?? 0, sx: 1, sy: 1 });
      define(`${prop.id}.o`, 'op', prop.o ?? 1);
      define(`${prop.id}.pop`, 'scale', prop.pop ?? 1);
      define(`${prop.id}.tilt`, 'rot', prop.tilt ?? 0);
      for (const [part, [kind, init]] of Object.entries(art.ch || {})) define(`${prop.id}.${part}`, kind, prop[part] ?? init);
    }
    const channel = name => {
      const found = channels.get(name);
      if (!found) throw Error(`Canal desconhecido: ${name}`);
      return found;
    };
    function hold(c, at) {
      const last = c.frames.at(-1);
      if (last && last.t > at + EPS) throw Error(`${c.name}: quadro em ${Math.round(at)} ms antes de ${Math.round(last.t)} ms`);
      if (!last || last.t < at - EPS) c.frames.push({ t: at, v: clone(c.cur), e: 'linear' });
      return c.frames.at(-1);
    }
    function key(name, at, value, length, ease) {
      const c = channel(name);
      hold(c, at).e = ease;
      c.cur = merge(c.cur, value);
      c.frames.push({ t: at + length, v: clone(c.cur), e: 'linear' });
    }
    function valueAt(c, time) {
      const frames = c.frames;
      if (!frames.length) return clone(c.init);
      if (time < frames[0].t) return clone(frames[0].v);
      let index = frames.length - 1;
      while (index > 0 && frames[index].t > time + EPS) index--;
      const from = frames[index], to = frames[index + 1];
      if (!to) return clone(from.v);
      const span = to.t - from.t;
      return span <= EPS ? clone(to.v) : lerp(from.v, to.v, easingFns[from.e]((time - from.t) / span));
    }
    const value = (name, time = t) => valueAt(channel(name), time);
    function robotMatrix(part, time) {
      const v = name => value(name, time);
      if (part === 'base') return translate(v('rx'), 0);
      let m = mul(translate(v('rx'), v('ry')), scale(v('face'), 1));
      if (part === 'root') return m;
      m = mul(m, mul(translate(0, HIP_Y), rotate(v('lean'))));
      if (part === 'body') return m;
      if (part === 'head') return mul(m, mul(translate(0, NECK_Y), rotate(v('head'))));
      const side = part === 'handF' ? 'F' : 'B';
      return [translate(side === 'F' ? 1 : -1, SHOULDER_Y), rotate(v(`arm${side}`)), translate(0, UPPER), rotate(v(`elb${side}`)), translate(0, FORE)].reduce(mul, m);
    }
    const attachmentAt = (id, time) => attachments.find(a => a.id === id && a.t0 <= time + EPS && time < a.t1 - EPS) || null;
    function anchor(id, time) {
      if (PARTS.has(id)) { const m = robotMatrix(id, time); return [m[4], m[5]]; }
      const pose = poseOf(id, time);
      return [pose.x, pose.y];
    }
    // Attachments are translation-following: a held prop keeps its own rotation, which
    // the skit controls through its tilt channel. This keeps cups and pages upright.
    function attachedPose(a, time) { const [x, y] = anchor(a.parent, time); return { ...a.pose, x: x + a.dx, y: y + a.dy }; }
    function poseOf(id, time) { const a = attachmentAt(id, time); return a ? attachedPose(a, time) : value(id, time); }
    function point(id, local = [0, 0], time = t) {
      if (PARTS.has(id)) return apply(robotMatrix(id, time), local[0], local[1]);
      const p = poseOf(id, time);
      return apply(mul(mul(translate(p.x, p.y), rotate(p.r)), scale(p.sx, p.sy)), local[0], local[1]);
    }
    function solveArm(side, target, over = {}) {
      const v = name => over[name] ?? value(name, t);
      const body = mul(mul(translate(v('rx'), v('ry')), scale(v('face'), 1)), mul(translate(0, HIP_Y), rotate(v('lean'))));
      const [lx, ly] = apply(invert(body), target[0], target[1]);
      const dx = lx - (side === 'F' ? 1 : -1), dy = ly - SHOULDER_Y;
      const distance = clamp(Math.hypot(dx, dy), Math.abs(UPPER - FORE) + 0.3, UPPER + FORE - 0.05);
      const base = Math.atan2(-dx, dy);
      const alpha = Math.acos(clamp((UPPER * UPPER + distance * distance - FORE * FORE) / (2 * UPPER * distance), -1, 1));
      const phi = Math.acos(clamp((UPPER * UPPER + FORE * FORE - distance * distance) / (2 * UPPER * FORE), -1, 1));
      let shoulder = deg(base + alpha), elbow = -deg(Math.PI - phi);
      if (over.elbowUp) { shoulder = deg(base - alpha); elbow = deg(Math.PI - phi); }
      const current = value(`arm${side}`, t);
      while (shoulder - current > 180) shoulder -= 360;
      while (shoulder - current < -180) shoulder += 360;
      return { [`arm${side}`]: shoulder, [`elb${side}`]: elbow };
    }
    const openAttachment = id => attachments.find(a => a.id === id && a.t1 === Infinity);
    const holding = side => attachments.some(a => a.t1 === Infinity && a.parent === `hand${side}`);
    function walkTime(distance, speed) { return Math.max(520, Math.abs(distance) / speed * 1000); }
    function crouchPose(depth) {
      let low = 0, high = 1.45, phi = 0, psi = 0;
      for (let i = 0; i < 30; i++) {
        phi = (low + high) / 2; psi = Math.asin(clamp(7.5 * Math.sin(phi) / 9.5, -1, 1));
        if (7.5 * Math.cos(phi) + 9.5 * Math.cos(psi) > 17 - depth) low = phi; else high = phi;
      }
      return { ry: depth, legF: -deg(phi), legB: -deg(phi), kneeF: deg(phi + psi), kneeB: deg(phi + psi) };
    }

    const api = {
      get t() { return t; },
      wait(ms) { t += ms; return api; },
      at(ms) { t = ms; return api; },
      cur: name => clone(channel(name).cur),
      pose: id => poseOf(id, t),
      point,
      set(map, at = t) { for (const [name, v] of Object.entries(map)) key(name, at, v, 0, 'linear'); return api; },
      tw(map, length, ease = 'io', at = t) { for (const [name, v] of Object.entries(map)) key(name, at, v, length, ease); return api; },
      to(map, length = 360, ease = 'io') { api.tw(map, length, ease); t += length; return api; },
      par(...branches) {
        const start = t; let end = t;
        for (const branch of branches) { t = start; branch(api); end = Math.max(end, t); }
        t = end; return api;
      },
      attach(id, parent, at = t) {
        const pose = poseOf(id, at), open = openAttachment(id);
        if (open) open.t1 = at; else hold(channel(id), at);
        const [x, y] = anchor(parent, at);
        attachments.push({ id, parent, t0: at, t1: Infinity, pose: { r: pose.r, sx: pose.sx, sy: pose.sy }, dx: pose.x - x, dy: pose.y - y });
        return api;
      },
      detach(id, at = t) {
        const open = openAttachment(id);
        if (!open) return api;
        const pose = attachedPose(open, at), c = channel(id);
        open.t1 = at; hold(c, at); c.frames.push({ t: at, v: pose, e: 'linear' }); c.cur = clone(pose);
        return api;
      },
      grab(id, side = 'F') { return api.attach(id, `hand${side}`); },
      drop(id, { onto = null, settle = 0 } = {}) {
        api.detach(id);
        if (settle) api.tw({ [`${id}.tilt`]: 0 }, settle, 'out');
        if (onto) api.attach(id, onto);
        return api;
      },
      // Samples an explicit path for a free prop; k is eased progress in [0, 1].
      path(id, fn, length, ease = 'linear') {
        const c = channel(id), start = t, base = clone(hold(c, start).v), easing = easingFns[ease];
        for (let s = STEP; s < length - EPS; s += STEP) c.frames.push({ t: start + s, v: merge(base, fn(easing(s / length), base)), e: 'linear' });
        c.cur = merge(base, fn(1, base)); c.frames.push({ t: start + length, v: clone(c.cur), e: 'linear' });
        return api;
      },
      reach(side, target, length = 380, ease = 'io', over = {}) {
        const sides = side === 'both' ? ['F', 'B'] : [side];
        const extra = over.with || {};
        const map = { ...extra };
        for (const s of sides) Object.assign(map, solveArm(s, s === 'B' && side === 'both' ? [target[0] - 0.9, target[1] + 0.4] : target, { ...extra, elbowUp: over.elbowUp }));
        return length ? api.to(map, length, ease) : api.set(map);
      },
      reachAt(side, id, local, length, ease, over) { return api.reach(side, point(id, local), length, ease, over); },
      // Puts the prop origin at a world point; held props translate with the hand.
      place(id, side, target, length = 420, ease = 'io', over = {}) {
        const open = openAttachment(id);
        const [hx, hy] = anchor(open.parent, t), pose = attachedPose(open, t);
        return api.reach(side, [target[0] - (pose.x - hx), target[1] - (pose.y - hy)], length, ease, over);
      },
      gait(start, length, { arms = true, stride = 24, lift = 32, bob = 1.1, period = 300 } = {}) {
        const steps = Math.max(2, Math.round(length / period)), p = length / steps;
        const swingF = arms && !holding('F'), swingB = arms && !holding('B');
        const frames = [];
        for (let k = 0; k < steps; k++) {
          const A = k % 2 ? -1 : 1;
          frames.push([start + (k + 0.5) * p, { legF: -stride * A, legB: stride * A, kneeF: A < 0 ? 8 : 2, kneeB: A > 0 ? 8 : 2, ry: 0,
            ...(swingF ? { armF: REST.armF + 17 * A } : {}), ...(swingB ? { armB: REST.armB - 17 * A } : {}) }]);
          if (k < steps - 1) frames.push([start + (k + 1) * p, { legF: 0, legB: 0, kneeF: A < 0 ? lift : 0, kneeB: A > 0 ? lift : 0, ry: -bob }]);
        }
        frames.push([start + length, { legF: 0, legB: 0, kneeF: 0, kneeB: 0, ry: 0, ...(swingF ? { armF: REST.armF } : {}), ...(swingB ? { armB: REST.armB } : {}) }]);
        let previous = start;
        for (const [time, map] of frames) { api.tw(map, time - previous, 'io', previous); previous = time; }
        return api;
      },
      walk(x, { speed = 38, face = true, arms = true, ease = 'walk', period } = {}) {
        const distance = x - channel('rx').cur;
        if (Math.abs(distance) < 0.05) return api;
        const direction = Math.sign(distance);
        if (face && channel('face').cur !== direction) api.turn(direction);
        const length = walkTime(distance, speed), start = t;
        api.gait(start, length, { arms, period: period ?? clamp(10.5 / speed * 1000, 240, 380) });
        api.tw({ rx: x }, length, ease, start);
        t = start + length;
        return api;
      },
      // A cartoon turn: squash to a sliver, flip, and open up facing the other way.
      turn(direction, length = 260) {
        const from = Math.sign(channel('face').cur) || 1;
        if (from === direction) return api;
        api.to({ face: 0.14 * from }, length / 2, 'in'); api.set({ face: -0.14 * from });
        return api.to({ face: direction }, length / 2, 'out');
      },
      goTo(x, face) { api.walk(x); if (face && channel('face').cur !== face) api.turn(face); return api; },
      // Moves props together with the robot, as if pushed or pulled by the handle.
      haul(ids, x, { speed = 30, ease = 'walk' } = {}) {
        const distance = x - channel('rx').cur;
        if (Math.abs(distance) < 0.05) return api;
        for (const id of ids) api.attach(id, 'base');
        const length = walkTime(distance, speed), start = t;
        api.gait(start, length, { arms: false, period: clamp(10.5 / speed * 1000, 260, 380) });
        api.tw({ rx: x }, length, ease, start);
        for (const id of ids) {
          const turns = meta.get(id)?.art.wheels;
          if (turns) for (const part of turns.parts) api.tw({ [`${id}.${part}`]: channel(`${id}.${part}`).cur + deg(distance / turns.r) }, length, ease, start);
        }
        t = start + length;
        for (const id of ids) api.detach(id);
        return api;
      },
      grip(id, local, { lean = 7, length = 0 } = {}) { return api.reach('both', point(id, local), length, 'io', { with: { lean } }); },
      rest(length = 320) { return api.to({ ...REST, lean: 0, head: 0 }, length, 'io'); },
      mood(name, at = t) {
        return api.set({ eyeOpen: name === 'open' ? 1 : 0, eyeHappy: name === 'happy' ? 1 : 0,
          eyeClosed: name === 'closed' ? 1 : 0, eyeWide: name === 'wide' ? 1 : 0 }, at);
      },
      look(x, y = 0, length = 200) { return api.to({ gaze: { x, y } }, length, 'io'); },
      nod(times = 2) { for (let i = 0; i < times; i++) { api.to({ head: 9 }, 160, 'out'); api.to({ head: 0 }, 180, 'io'); } return api; },
      hop(height = 4, length = 380) {
        api.to({ ry: 1.2, kneeF: 14, kneeB: 14, legF: -6, legB: -6 }, 110, 'out');
        api.to({ ry: -height, kneeF: 22, kneeB: 22, legF: -12, legB: 4 }, length / 2, 'out');
        return api.to({ ry: 0, kneeF: 0, kneeB: 0, legF: 0, legB: 0 }, length / 2, 'in');
      },
      wave(times = 3) {
        // The hand waves beside the skull, never across the face.
        api.to({ armF: -100, elbF: -34 }, 260, 'out');
        for (let i = 0; i < times; i++) { api.to({ elbF: -68 }, 170, 'io'); api.to({ elbF: -30 }, 170, 'io'); }
        return api;
      },
      crouch(depth, length = 360, ease = 'io') { return depth ? api.to(crouchPose(depth), length, ease) : api.to({ ry: 0, legF: 0, legB: 0, kneeF: 0, kneeB: 0 }, length, ease); },
      crouchPose,
      // Speech bubbles hang above the head and keep their text readable when mirrored.
      // Comic lettering punches out of the head (or a prop), wobbles, then puffs away.
      say(id, holdMs = 900, { dx = 7, dy = -19, on = 'head' } = {}) {
        api.detach(id);
        const [x, y] = point(on, [dx, dy]);
        // Lettering stays inside the round window, even when the robot peeks from an edge.
        api.set({ [id]: { x: clamp(x, 64, 136), y }, [`${id}.pop`]: 0.2, [`${id}.o`]: 0, [`${id}.tilt`]: -10 });
        api.attach(id, on);
        api.tw({ [`${id}.o`]: 1 }, 90, 'out');
        api.to({ [`${id}.pop`]: 1, [`${id}.tilt`]: 0 }, 260, 'back');
        const wobbles = Math.min(3, Math.floor(holdMs / 240));
        for (let i = 0; i < wobbles; i++) api.to({ [`${id}.tilt`]: i % 2 ? -3 : 3 }, 120, 'io');
        api.to({ [`${id}.tilt`]: 0 }, 120, 'io');
        t += Math.max(0, holdMs - (wobbles + 1) * 120);
        api.tw({ [`${id}.pop`]: 1.2, [`${id}.o`]: 0 }, 200, 'in'); t += 200;
        return api;
      }
    };

    function finish() {
      const total = Math.max(t, 1);
      for (const a of attachments) {
        const end = Math.min(a.t1, total), c = channel(a.id);
        const samples = [];
        for (let s = a.t0; s < end - EPS; s += STEP) samples.push({ t: s, v: attachedPose(a, s), e: 'linear' });
        samples.push({ t: end, v: attachedPose(a, end), e: 'linear' });
        const before = c.frames.filter(f => f.t < a.t0 - EPS), after = c.frames.filter(f => f.t > end + EPS);
        c.frames = [...before, ...samples, ...after];
      }
      const tracks = [], statics = [];
      for (const c of channels.values()) {
        const target = targetOf(c.name, c.kind);
        if (!c.frames.length) { statics.push({ target, style: FORMAT[c.kind](c.init) }); continue; }
        let frames = c.frames.map(f => ({ ...f, t: clamp(f.t, 0, total) }));
        if (frames[0].t > EPS) frames.unshift({ t: 0, v: clone(frames[0].v), e: 'linear' });
        if (frames.at(-1).t < total - EPS) frames.push({ t: total, v: clone(frames.at(-1).v), e: 'linear' });
        // Drop interior frames of constant runs; they only add parse and memory cost.
        frames = frames.filter((f, i) => i === 0 || i === frames.length - 1 || !(same(frames[i - 1].v, f.v) && same(f.v, frames[i + 1].v)));
        if (frames.every(f => same(f.v, frames[0].v))) { statics.push({ target, style: FORMAT[c.kind](frames[0].v) }); continue; }
        let offset = 0;
        tracks.push({ target, name: c.name, keyframes: frames.map(f => {
          offset = Math.max(offset, clamp(f.t / total, 0, 1));
          return { offset, easing: easingCss(f.e), ...FORMAT[c.kind](f.v) };
        }) });
      }
      const layer = name => props.filter(p => (p.layer || 'back') === name).map(p => {
        const art = meta.get(p.id).art;
        return `<g data-c="${p.id}" class="wv-p wv-p-${p.art}"><g data-c="${p.id}.pop"><g data-c="${p.id}.tilt">${art.svg(p.id, p)}</g></g></g>`;
      }).join('');
      const probe = time => ({
        robot: value('rx', time),
        props: props.map(p => ({ id: p.id, x: poseOf(p.id, time).x, y: poseOf(p.id, time).y, half: meta.get(p.id).art.half, opacity: value(`${p.id}.o`, time) }))
      });
      return { duration: total, tracks, statics, markup: `${layer('back')}${ROBOT}${layer('front')}${ROBOT_ARM}`, probe };
    }
    return { api, finish };
  }

  // ------------------------------------------------------------------ skits
  // World units: the round window is 200 wide, the floor is y = 0 and the visible
  // floor spans roughly x 12..188. Robots and props start and end outside it.
  const CART_GRIP = [-21.6, -27];
  const BEHIND = 31.6; // robot x relative to a cart while pushing it
  function cartIn(c, cart, robotX, speed = 32) {
    c.grip(cart, CART_GRIP);
    c.haul([cart], robotX, { speed });
    return c.rest(300);
  }
  function cartOut(c, cart, speed = 42) {
    c.goTo(c.pose(cart).x - BEHIND, 1);
    c.grip(cart, CART_GRIP, { length: 300 });
    return c.haul([cart], 240, { speed });
  }
  const CART_X = -30; // entrance position; riders are declared relative to it
  const onCart = (dx, y = -16) => ({ x: CART_X + dx, y });
  // Screens hang from the ceiling and are called down (or sent back up) with a gesture.
  const summon = (c, id, y, length = 1000) => { const from = c.pose(id).y; c.path(id, k => ({ y: from + (y - from) * k }), length, 'io'); };
  const sitPose = seat => ({ ry: 16 - seat, legF: -86, legB: -80, kneeF: 84, kneeB: 80 });

  const SKITS = {
    greet: { kind: 'fun', opener: true, weight: 3,
      props: r => [{ id: 'hi', art: 'bubble', layer: 'front', text: r.pick(['E AÍ, MACHO VÉI?', 'EI, MÁ, BLZ?', 'FALA, MACHO!', 'EI, BICHO, BLZ?']), o: 0 }],
      play(c, r) {
        c.walk(r.range(84, 104));
        c.wait(160); c.look(0, -0.4, 220); c.mood('happy');
        c.par(a => a.wave(3), a => { a.wait(140); a.say('hi', 900); });
        c.rest(300); c.mood('open'); c.look(2.4, 0, 200); c.wait(200);
        c.walk(236);
      } },
    peek: { kind: 'fun', opener: true, weight: 2,
      props: r => [{ id: 'bang', art: 'bubble', layer: 'front', text: r.pick(['VISH!', 'OXE!']), tone: 'pow', o: 0 }],
      play(c, r) {
        c.set({ rx: -34 });
        c.walk(r.range(17, 22), { speed: 20 });
        c.to({ lean: 13, head: 7 }, 340, 'out');
        c.look(3.8, -0.8, 180); c.wait(420); c.look(3.4, 0.9, 220); c.wait(360);
        c.look(0, 0, 160); c.mood('wide'); c.say('bang', 420, { dx: 9, dy: -17 });
        c.mood('happy'); c.wave(2); c.rest(260); c.mood('open');
        c.walk(-36, { speed: 48 });
      } },
    stretch: { kind: 'fun', opener: true, weight: 2,
      props: [],
      play(c, r) {
        c.walk(r.range(86, 112));
        c.mood('closed');
        c.to({ armF: -172, elbF: -6, armB: -166, elbB: -10, head: -8 }, 560, 'io');
        c.to({ lean: -10 }, 460); c.to({ lean: 12 }, 560); c.to({ lean: 0 }, 380);
        c.wait(520);
        c.rest(400); c.mood('happy'); c.wait(320); c.mood('open');
        c.walk(236);
      } },
    coffee: { kind: 'fun', weight: 3, minElapsedMs: 15000,
      props: r => [{ id: 'cart', art: 'cart', x: CART_X }, { id: 'mug', art: 'mug', ...onCart(5.7), layer: 'front' }, { id: 'mach', art: 'machine', ...onCart(13) },
        { id: 'love', art: 'bubble', layer: 'front', text: r.pick(['SÓ O FILÉ, OH!', 'GOSTOSO, OH!']), o: 0 },
        { id: 'slurp', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA!', 'ARRIÉGUA!']), tone: 'pow', o: 0 }],
      play(c, r) {
        c.set({ rx: CART_X - BEHIND }); c.attach('mug', 'cart'); c.attach('mach', 'cart');
        cartIn(c, 'cart', r.range(50, 60));
        const cart = c.pose('cart').x;
        c.walk(cart - 9, { speed: 24 });
        c.look(3.6, 0.6); c.wait(200);
        c.reachAt('F', 'mach', [-6.6, -15.6], 380, 'io', { with: { lean: 10 } });
        c.to({ elbF: c.cur('elbF') - 8 }, 120); c.set({ 'mach.light': 1, 'mach.drip': 1 });
        c.rest(300); c.wait(1100); c.set({ 'mach.drip': 0 }); c.tw({ 'mug.steam': 1 }, 500); c.wait(250);
        const slot = [c.pose('mug').x, c.pose('mug').y];
        c.reachAt('F', 'mug', [-4.2, -3.4], 360, 'io', { with: { lean: 6 } }); c.grab('mug');
        for (let sip = 0; sip < 2; sip++) {
          c.par(a => a.reach('F', a.point('root', [7.5, -36]), 520, 'io', { with: { lean: -2 } }), a => a.tw({ head: -9, 'mug.tilt': -24 }, 520));
          c.mood('closed'); if (sip) c.wait(500); else c.say('slurp', 420, { dx: 12, dy: -14 }); c.mood('happy');
          c.par(a => a.reach('F', a.point('root', [10, -25]), 420, 'io', { with: { lean: 0 } }), a => a.tw({ head: 0, 'mug.tilt': 0 }, 420));
          if (!sip) c.par(a => a.to({ ry: -1.2 }, 260).to({ ry: 0 }, 260), a => a.say('love', 700));
        }
        c.place('mug', 'F', slot, 440, 'io', { with: { lean: 6 } }); c.drop('mug', { onto: 'cart' });
        c.tw({ 'mug.steam': 0, 'mach.light': 0.2 }, 500); c.mood('open'); c.rest(300);
        cartOut(c, 'cart');
      } },
    nap: { kind: 'fun', weight: 2, minElapsedMs: 45000,
      props: [{ id: 'cush', art: 'cushion', x: -40, y: -49, layer: 'front' }, { id: 'zzz', art: 'zzz', o: 0, layer: 'front' }, { id: 'bang', art: 'bubble', layer: 'front', text: 'OXE!', tone: 'pow', o: 0 },
        { id: 'plof', art: 'bubble', layer: 'front', text: 'PLOF!', tone: 'pow', o: 0, angle: 6 }],
      play(c, r) {
        c.set({ rx: -40, armF: -166, elbF: -18, armB: -150, elbB: -30 });
        c.attach('cush', 'head');
        const spot = r.range(70, 96);
        const wobbles = Math.floor(Math.abs(spot + 40) / 28 * 1000 / 380) - 1;
        c.par(a => a.walk(spot, { arms: false, speed: 28 }), a => { for (let i = 0; i < wobbles; i++) a.to({ head: i % 2 ? -3 : 3 }, 380); a.to({ head: 0 }, 200); });
        // Tip the cushion forward, then let it settle on the floor.
        c.to({ lean: 10, head: 6 }, 220, 'out'); c.detach('cush');
        const from = c.pose('cush'), land = spot + 22;
        c.path('cush', k => ({ x: from.x + (land - from.x) * k, y: from.y * (1 - k) - 14 * Math.sin(Math.PI * k), r: 12 * Math.sin(Math.PI * k) }), 520, 'in');
        c.par(a => a.rest(420), a => a.wait(520)); c.to({ cush: { sy: 0.8 } }, 90, 'out');
        c.par(a => a.to({ cush: { sy: 1 } }, 200, 'back'), a => a.say('plof', 260, { on: 'cush', dx: 0, dy: -10 }));
        c.look(3.4, 0.8); c.mood('happy'); c.wait(300); c.mood('open');
        // Sit on it facing the viewer's left, as if slumping into a bean bag.
        c.turn(-1); c.walk(land + 1, { face: false, speed: 18 });
        c.to(sitPose(9.5), 420, 'io'); c.to({ lean: -14, armF: -36, elbF: -96, armB: -30, elbB: -90, cush: { sy: 0.82 } }, 300, 'out');
        c.mood('closed'); c.to({ armF: -170, elbF: -8, armB: -164, elbB: -14, head: -8 }, 480); c.wait(300);
        c.to({ armF: -36, elbF: -96, armB: -30, elbB: -90, head: 4 }, 520);
        const [zx, zy] = c.point('head', [8, -17]);
        c.set({ zzz: { x: zx, y: zy } }); c.attach('zzz', 'head'); c.tw({ 'zzz.o': 1 }, 400);
        c.tw({ bulb: 0.25 }, 600);
        for (let i = 0; i < 3; i++) { c.to({ lean: -17, ry: 6.9 }, 820, 'io'); c.to({ lean: -14, ry: 6.5 }, 820, 'io'); }
        c.to({ 'zzz.o': 0 }, 180); c.mood('wide'); c.set({ bulb: 1 });
        c.to({ ry: 4.6, lean: -4 }, 140, 'out'); c.to({ ry: 6.5 }, 220, 'in');
        c.look(0, 0, 160); c.say('bang', 380, { dx: -8, dy: -17 }); c.mood('happy');
        c.to({ ry: 0, legF: 0, legB: 0, kneeF: 0, kneeB: 0, lean: 0, head: 0, ...REST, cush: { sy: 1 } }, 420, 'io');
        c.look(2.4, 0); c.mood('open');
        // Step off, turn back, pick the cushion up onto the head and leave.
        c.walk(land - 16, { face: false, speed: 26 }); c.turn(1);
        c.par(a => a.crouch(3, 320), a => a.reachAt('both', 'cush', [-9, -8], 320, 'io', { with: { lean: 26 } }));
        const lifted = c.pose('cush'), rx = c.cur('rx');
        c.par(a => a.crouch(0, 520), a => a.to({ lean: 0, armF: -166, elbF: -18, armB: -150, elbB: -30 }, 520, 'io'),
          a => a.path('cush', k => ({ x: lifted.x + (rx - lifted.x) * k, y: lifted.y + (-49 - lifted.y) * k - 6 * Math.sin(Math.PI * k) }), 520, 'io'));
        c.attach('cush', 'head'); c.wait(160);
        c.walk(240, { arms: false, speed: 30 });
      } },
    reading: { kind: 'work', families: ['reading'], weight: 6,
      props: r => [{ id: 'cart', art: 'cart', x: CART_X },
        ...[0, 1, 2].map(i => ({ id: `s${i}`, art: 'sheet', ...onCart(-1.4 + i * 1.4, -17), layer: 'front' })),
        { id: 'inbox', art: 'tray', ...onCart(0), layer: 'front' }, { id: 'outbox', art: 'tray', ...onCart(12), layer: 'front' },
        { id: 'hmm', art: 'bubble', layer: 'front', text: 'OXENTE?', o: 0 }, { id: 'aha', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA!', 'ARRIÉGUA!']), tone: 'pow', o: 0 }],
      play(c, r) {
        c.set({ rx: CART_X - BEHIND });
        for (const id of ['s0', 's1', 's2', 'inbox', 'outbox']) c.attach(id, 'cart');
        cartIn(c, 'cart', r.range(46, 56));
        const cart = c.pose('cart').x;
        c.walk(cart - 13, { speed: 24 });
        const count = r.int(2, 3);
        for (let i = 0; i < count; i++) {
          const id = `s${2 - i}`;
          c.look(3.2, 1); c.reachAt('F', id, [0, -10.5], 360, 'io', { with: { lean: 8 } }); c.grab(id);
          c.reach('F', c.point('root', [12.5, -31]), 460, 'io', { with: { lean: 0 } });
          c.tw({ head: 7 }, 300);
          for (let line = 0; line < (i ? 2 : 3); line++) { c.look(1.6, 1.2, 160); c.look(4, 1.2, 420); }
          if (i === count - 1) { c.say('hmm', 520); c.to({ head: -4 }, 200); c.mood('happy'); c.say('aha', 520); }
          else c.nod(1);
          c.walk(cart - 4, { speed: 22 });
          c.place(id, 'F', [cart + 12 + i * 1.3 - 1.3, -17], 420, 'io', { with: { lean: 8 } }); c.drop(id, { onto: 'cart' });
          c.rest(260); c.mood('open'); c.look(2.4, 0);
          if (i < count - 1) c.walk(cart - 13, { face: false, speed: 22 });
        }
        cartOut(c, 'cart');
      } },
    archive: { kind: 'work', families: ['checkpoint'], weight: 6,
      props: [{ id: 'fold', art: 'folder', x: -15, y: -33 }, { id: 'cab', art: 'cabinet', x: -14 }],
      play(c, r) {
        const grip = [-12.8, -27];
        c.set({ rx: -36 }); c.attach('fold', 'cab');
        c.grip('cab', grip, { lean: 6 });
        c.haul(['cab'], r.range(62, 80), { speed: 30 });
        c.rest(300);
        const cab = c.pose('cab').x;
        c.walk(cab - 15, { speed: 22 });
        c.look(3, -1); c.reachAt('F', 'fold', [-2, -9], 380, 'io', { with: { lean: 4 } }); c.grab('fold');
        c.reach('F', c.point('root', [13, -34]), 420); c.wait(240);
        c.place('fold', 'F', [cab, -36], 380); c.place('fold', 'F', [cab, -21], 520, 'in');
        c.drop('fold', { onto: 'cab' }); c.rest(280);
        c.set({ 'cab.led': 1 }); c.to({ 'cab.pop': 1.06 }, 120, 'out').to({ 'cab.pop': 1 }, 220, 'back');
        c.mood('happy'); c.nod(1); c.wait(400); c.tw({ 'cab.led': 0.2 }, 600); c.mood('open');
        c.goTo(cab - 12.8 - 10, 1);
        c.grip('cab', grip, { lean: 6, length: 280 });
        c.haul(['cab'], 236, { speed: 40 });
      } },
    chart: { kind: 'work', families: ['calculation'], weight: 6,
      props: r => [{ id: 'board', art: 'board', x: -22 }, { id: 'hmm', art: 'bubble', layer: 'front', text: 'OXENTE?', o: 0 }, { id: 'aha', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA, MACHO!', 'ARRIÉGUA!']), tone: 'pow', o: 0 }],
      play(c, r) {
        const grip = [-19.6, -26];
        c.set({ rx: CART_X - BEHIND });
        c.grip('board', grip, { lean: 6 });
        c.haul(['board'], r.range(36, 50), { speed: 30 });
        c.rest(300);
        const board = c.pose('board').x;
        const draw = (part, local, step) => {
          c.walk(board + local[0] - 11, { speed: 20 });
          c.reachAt('F', 'board', local, 300, 'io', { with: { lean: 2 } });
          c.par(a => a.to({ [`board.${part}`]: 0 }, step, 'io'), a => {
            for (let i = 0; i < 3; i++) a.reach('F', a.point('board', [local[0] + (i % 2 ? 2 : 0), local[1] + (i % 2 ? 3 : 0)]), step / 3, 'io');
          });
        };
        c.look(3.6, -1.4);
        draw('ax', [-10, -32], 700);
        draw('b1', [-6, -25], 420); draw('b2', [0, -27], 460); draw('b3', [6, -30], 500);
        c.rest(240); c.walk(board - 26, { face: false, speed: 22 });
        c.to({ armF: -140, elbF: -112 }, 320); c.look(3.4, -1.6); c.say('hmm', 560);
        c.mood('happy'); c.say('aha', 420);
        c.walk(board - 4, { speed: 24 }); draw('tr', [0, -36], 560);
        c.rest(260); c.nod(1); c.mood('open');
        c.goTo(board + grip[0] - 10, 1);
        c.grip('board', grip, { lean: 6, length: 300 });
        c.haul(['board'], 236, { speed: 40 });
      } },
    inspect: { kind: 'work', families: ['verification'], weight: 6,
      props: r => [{ id: 'stand', art: 'stand', x: -18 }, { id: 'lens', art: 'lens', x: -31, y: -28, layer: 'front' }, { id: 'hmm', art: 'bubble', layer: 'front', text: r.pick(['SEI NÃO, VIU…', 'PERAÍ, MACHO…']), o: 0 }],
      play(c, r) {
        const grip = [-14, -20.4];
        c.set({ rx: -42 }); c.attach('lens', 'stand');
        c.grip('stand', grip, { lean: 6 });
        c.haul(['stand'], r.range(56, 74), { speed: 30 });
        c.rest(300);
        const stand = c.pose('stand').x, hook = [c.pose('lens').x, c.pose('lens').y];
        c.walk(stand - 18, { speed: 22 });
        c.reachAt('F', 'lens', [0, 0], 360); c.grab('lens');
        c.look(4, -1.2);
        for (const [dx, dy] of [[-11, -34], [-6, -40], [-9, -28], [-3, -36]]) {
          c.place('lens', 'F', [stand + dx, dy + 7], 460, 'io', { with: { lean: 9 } });
          c.mood('wide'); c.wait(340); c.mood('open');
        }
        c.say('hmm', 520); c.nod(2); c.mood('happy');
        c.place('lens', 'F', hook, 460, 'io', { with: { lean: 2 } }); c.drop('lens', { onto: 'stand' });
        c.rest(300); c.mood('open');
        c.goTo(stand + grip[0] - 10, 1);
        c.grip('stand', grip, { lean: 6, length: 300 });
        c.haul(['stand'], 236, { speed: 40 });
      } },
    print: { kind: 'work', families: ['composition'], weight: 6,
      props: r => [{ id: 'cart', art: 'cart', x: CART_X }, { id: 'page', art: 'sheet', ...onCart(8, -17.5), layer: 'front' }, { id: 'prn', art: 'printer', ...onCart(8) },
        { id: 'love', art: 'bubble', layer: 'front', text: r.pick(['FICOU MASSA!', 'MASSA!']), o: 0 }],
      play(c, r) {
        c.set({ rx: CART_X - BEHIND }); c.attach('page', 'cart'); c.attach('prn', 'cart');
        cartIn(c, 'cart', r.range(50, 60));
        const cart = c.pose('cart').x;
        c.walk(cart - 10, { speed: 24 });
        c.reachAt('F', 'prn', [3, -15.5], 340, 'io', { with: { lean: 8 } }); c.to({ elbF: c.cur('elbF') - 7 }, 110);
        c.set({ 'prn.led': 1 }); c.rest(260);
        c.detach('page');
        for (let i = 0; i < 3; i++) c.to({ 'prn.pop': 1.05 }, 140, 'out').to({ 'prn.pop': 1 }, 200, 'io');
        const page = c.pose('page');
        c.path('page', k => ({ y: page.y - 10 * k }), 700, 'io'); c.wait(700); c.set({ 'prn.led': 0.2 });
        c.reachAt('F', 'page', [0, -10.5], 360); c.grab('page');
        c.reach('F', c.point('root', [12.5, -31]), 420); c.tw({ head: 6 }, 260); c.look(3.8, 1.2, 380); c.look(2, 1.2, 260); c.look(3.8, 1.2, 380);
        c.mood('happy'); c.say('love', 600);
        c.place('page', 'F', [cart + 1, -27.5], 440); c.drop('page', { onto: 'cart' });
        c.rest(280); c.mood('open');
        cartOut(c, 'cart');
      } },
    restore: { kind: 'work', families: ['restoration'], weight: 6,
      props: [{ id: 'cart', art: 'cart', x: CART_X }, { id: 'frame', art: 'frame', ...onCart(5) },
        { id: 'k1', art: 'block', tone: 1, ...onCart(-11.4, -18.6), layer: 'front' }, { id: 'k2', art: 'block', tone: 2, ...onCart(-5.6, -18.6), layer: 'front' },
        { id: 'k3', art: 'block', tone: 3, ...onCart(-8.5, -23.8), layer: 'front' }, { id: 'basket', art: 'tray', ...onCart(-8.5), layer: 'front' }],
      play(c, r) {
        c.set({ rx: CART_X - BEHIND });
        for (const id of ['frame', 'k1', 'k2', 'k3', 'basket']) c.attach(id, 'cart');
        cartIn(c, 'cart', r.range(46, 56));
        const cart = c.pose('cart').x, frame = c.pose('frame').x;
        c.walk(cart - 12, { speed: 24 });
        const order = r.chance(0.5) ? ['k3', 'k1', 'k2'] : ['k3', 'k2', 'k1'];
        order.forEach((id, i) => {
          c.look(3, 1); c.reachAt('F', id, [-1.5, -1], 340, 'io', { with: { lean: 10 } }); c.grab(id);
          c.reach('F', c.point('root', [10, -30]), 360);
          c.place(id, 'F', [frame - 6.6 + i * 6.6, -22.8], 460, 'io', { with: { lean: 8 } }); c.drop(id, { onto: 'cart' });
          c.to({ [`${id}.pop`]: 1.18 }, 110, 'out').to({ [`${id}.pop`]: 1 }, 220, 'back');
          c.rest(220);
          if (i < 2) c.walk(cart - 12 + (i + 1) * 3, { speed: 18 });
        });
        c.walk(cart - 20, { face: false, speed: 20 }); c.look(3.6, 0.6); c.mood('happy'); c.nod(2); c.wait(260); c.mood('open');
        cartOut(c, 'cart');
      } },
    queue: { kind: 'work', families: ['access'], weight: 6,
      props: r => [{ id: 'term', art: 'terminal', x: -14 }, { id: 'tune', art: 'bubble', layer: 'front', text: r.pick(['AVIA, MACHO!', 'AVIA!']), tone: 'pow', o: 0 }],
      play(c, r) {
        const grip = [-11.4, -27];
        c.set({ rx: -36 });
        c.grip('term', grip, { lean: 6 });
        c.haul(['term'], r.range(66, 82), { speed: 32 });
        c.rest(280);
        const term = c.pose('term').x;
        c.walk(term - 15, { speed: 22 });
        c.reachAt('F', 'term', [-4, -26.4], 320); c.to({ elbF: c.cur('elbF') - 7 }, 110); c.rest(260);
        for (let i = 0; i < 4; i++) c.to({ legF: -14, kneeF: 18 }, 150, 'out').to({ legF: 0, kneeF: 0 }, 150, 'in');
        c.to({ armB: -88, elbB: -94, head: 10 }, 360); c.wait(420); c.to({ ...REST, head: 0 }, 300);
        c.look(0, 0); c.to({ armF: -40, elbF: -70, armB: -40, elbB: -70, ry: -1 }, 260, 'out'); c.to({ ry: 0 }, 220); c.rest(280);
        c.look(2.4, -1.6); c.say('tune', 700, { dx: 8, dy: -18 }); c.look(2.4, 0);
        c.goTo(term + grip[0] - 10, 1);
        c.grip('term', grip, { lean: 6, length: 280 });
        c.haul(['term'], 236, { speed: 40 });
      } },
    juggle: { kind: 'fun', weight: 2, minElapsedMs: 8000,
      props: r => [{ id: 'b1', art: 'ball', tone: 1, layer: 'front' }, { id: 'b2', art: 'ball', tone: 2, layer: 'front' }, { id: 'b3', art: 'ball', tone: 3, layer: 'front' },
        { id: 'oops', art: 'bubble', layer: 'front', text: r.pick(['VIXE!', 'AI, DIACHO!']), tone: 'pow', o: 0, angle: 7 }],
      play(c, r) {
        const hands = { armF: -58, elbF: -46, armB: -34, elbB: -70 };
        c.set({ rx: -40, ...hands });
        const handF = c.point('handF'), handB = c.point('handB');
        c.set({ b1: { x: handF[0] + 1.5, y: handF[1] - 2.6 }, b2: { x: handF[0] - 2.2, y: handF[1] - 2.2 }, b3: { x: handB[0] + 1, y: handB[1] - 2.4 } });
        c.attach('b1', 'handF'); c.attach('b2', 'handF'); c.attach('b3', 'handB');
        const spot = r.range(84, 104);
        c.walk(spot, { arms: false, speed: 30 });
        c.look(1, -2);
        const toss = (id, from, to, height, length) => {
          c.detach(id);
          const a = c.pose(id), b = c.point(to);
          c.path(id, k => ({ x: a.x + (b[0] - a.x) * k, y: a.y + (b[1] - 2.4 - a.y) * k - height * 4 * k * (1 - k) }), length);
        };
        const rounds = r.int(2, 3);
        const balls = ['b1', 'b2', 'b3'];
        for (let i = 0; i < rounds * 3; i++) {
          const id = balls[i % 3];
          const start = c.t;
          toss(id, 'handF', 'handB', 26, 640);
          c.tw({ elbF: hands.elbF - 14 }, 110, 'out').tw({ elbF: hands.elbF }, 160, 'io', start + 110);
          c.at(start + 640); c.attach(id, 'handB');
          c.at(start + 660); c.detach(id);
          const p = c.pose(id), back = c.point('handF');
          c.path(id, k => ({ x: p.x + (back[0] - p.x) * k, y: p.y + (back[1] - 2.6 - p.y) * k - 5 * 4 * k * (1 - k) }), 200);
          c.at(start + 860); c.attach(id, 'handF');
          c.at(start + 340);
        }
        c.at(c.t + 560);
        // The last throw goes too high and bonks the antenna, then rolls away.
        const id = 'b1';
        c.detach(id); const top = c.point('root', [0, -55.4]), p = c.pose(id);
        c.path(id, k => ({ x: p.x + (top[0] - p.x) * k, y: p.y + (top[1] - p.y) * k - 40 * 4 * k * (1 - k) }), 900, 'linear');
        c.look(0, -2.2, 300); c.wait(600); c.mood('closed'); c.wait(300);
        const hit = c.pose(id);
        const bounce = c.t;
        c.path(id, k => ({ x: hit.x + 26 * k, y: -2.3 + (hit.y + 2.3) * (1 - k) * Math.abs(Math.cos(k * Math.PI * 1.5)), r: 540 * k }), 1100, 'linear');
        c.to({ head: 10, ry: 1.2 }, 120, 'out'); c.say('oops', 380, { dx: -9, dy: -16 }); c.to({ head: 0, ry: 0 }, 300);
        c.mood('happy'); c.look(4, 1.5); c.wait(200); c.mood('open');
        c.at(Math.max(c.t, bounce + 1100));
        c.path(id, k => ({ x: hit.x + 26 + 170 * k, r: 540 + 2600 * k }), 2400, 'in');
        c.walk(244, { arms: false, speed: 44 });
      } },
    airplane: { kind: 'fun', weight: 2, minElapsedMs: 6000,
      props: r => [{ id: 'plane', art: 'plane', layer: 'front' }, { id: 'yay', art: 'bubble', layer: 'front', text: r.pick(['VOA, BICHIM!', 'ÉGUA, VOOU!', 'ARRIÉGUA!']), tone: 'pow', o: 0 }],
      play(c, r) {
        c.set({ rx: -40, armF: -62, elbF: -40 });
        const hand = c.point('handF');
        c.set({ plane: { x: hand[0] + 2, y: hand[1] - 4.5 } }); c.attach('plane', 'handF');
        c.walk(r.range(56, 72), { speed: 32 });
        c.look(4, 1);
        for (let i = 0; i < 2; i++) c.to({ 'plane.pop': 0.8 }, 160, 'out').to({ 'plane.pop': 1 }, 160, 'io');
        c.set({ 'plane.flat': 0, 'plane.fold': 1 }); c.rest(200);
        c.reach('F', c.point('root', [-4, -36]), 300, 'io', { with: { lean: -8 } }); c.look(3, -1.2); c.wait(200);
        c.reach('F', c.point('root', [12, -34]), 180, 'out', { with: { lean: 8 } }); c.detach('plane');
        const start = c.pose('plane');
        // A gentle climb with one loop-the-loop; the nose follows the path tangent.
        const at = k => {
          // The loop happens well inside the window; afterwards the plane speeds off.
          const loop = clamp((k - 0.26) / 0.32, 0, 1), turn = loop * Math.PI * 2;
          return [start.x + 110 * k + 80 * k ** 4 + 13 * Math.sin(turn), start.y - 12 * k - 13 * (1 - Math.cos(turn)) + 5 * Math.sin(k * Math.PI)];
        };
        c.path('plane', k => {
          const [x, y] = at(k), [nx, ny] = at(Math.min(1, k + 0.002)), [px, py] = at(Math.max(0, k - 0.002));
          return { x, y, r: deg(Math.atan2(ny - py, nx - px)) };
        }, 2800, 'linear');
        c.par(a => { a.to({ head: -10 }, 600); a.look(4, -2, 500); a.wait(700); a.to({ head: 0 }, 400); }, a => a.to({ ...REST, lean: 0 }, 400));
        c.mood('happy'); c.say('yay', 520); c.hop(5); c.mood('open');
        c.walk(240, { speed: 40 });
      } },
    music: { kind: 'fun', weight: 2, minElapsedMs: 20000,
      props: [{ id: 'radio', art: 'radio', layer: 'front' }],
      play(c, r) {
        c.set({ rx: -40, armF: -40, elbF: -42 });
        const hand = c.point('handF');
        c.set({ radio: { x: hand[0], y: hand[1] + 11.6 } }); c.attach('radio', 'handF');
        const spot = r.range(64, 80);
        c.walk(spot, { speed: 30 });
        c.par(a => a.crouch(3), a => a.place('radio', 'F', [spot + 17, 0], 420, 'io', { with: { lean: 20 } })); c.drop('radio');
        c.reachAt('F', 'radio', [-2, -9.6], 260, 'io', { with: { lean: 22 } }); c.set({ 'radio.notes': 1 });
        c.par(a => a.crouch(0), a => a.rest(360));
        c.look(1, -0.6); c.mood('happy');
        const beats = r.int(6, 8);
        for (let i = 0; i < beats; i++) {
          const s = i % 2 ? -1 : 1;
          c.to({ lean: 8 * s, ry: -1.6, armF: -120 + 30 * s, elbF: -40, armB: -120 - 30 * s, elbB: -40, legF: -8 * s, legB: 8 * s }, 230, 'out');
          c.to({ ry: 0, legF: 0, legB: 0 }, 200, 'in');
          if (i === 3) c.turn(-1, 220).turn(1, 220);
        }
        c.rest(300); c.mood('open'); c.look(2.4, 0);
        c.par(a => a.crouch(3), a => a.reachAt('F', 'radio', [-2, -9.6], 360, 'io', { with: { lean: 22 } })); c.set({ 'radio.notes': 0 });
        c.reachAt('F', 'radio', [0, -11.6], 200, 'io', { with: { lean: 22 } }); c.grab('radio');
        c.par(a => a.crouch(0, 400), a => a.to({ lean: 0, armF: -40, elbF: -42 }, 400));
        c.walk(240, { speed: 34 });
      } },
    skate: { kind: 'fun', opener: true, weight: 2,
      props: [{ id: 'deck', art: 'skate', x: -45 }],
      play(c, r) {
        const ride = { ry: -4.6, legF: -16, kneeF: 24, legB: 14, kneeB: 10, armF: -70, elbF: -10, armB: 60, elbB: -10, lean: 4 };
        c.set({ rx: -45, ...ride });
        c.attach('deck', 'root');
        const mid = r.range(80, 110);
        c.tw({ rx: mid }, 1500, 'linear'); c.tw({ 'deck.wl': 1400, 'deck.wr': 1400 }, 1500, 'linear');
        c.wait(900); c.look(0, 0); c.mood('happy'); c.wait(600);
        c.tw({ rx: mid + 40 }, 700, 'linear'); c.tw({ 'deck.wl': 2000, 'deck.wr': 2000 }, 700, 'linear');
        c.to({ ry: -18, kneeF: 40, kneeB: 36, armF: -120, armB: 120 }, 350, 'out'); c.to({ ry: -4.6, ...ride }, 350, 'in');
        c.mood('open'); c.look(2.4, 0);
        c.tw({ rx: 250 }, 1300, 'linear'); c.tw({ 'deck.wl': 3800, 'deck.wr': 3800 }, 1300, 'linear'); c.wait(1300);
      } },
    sweep: { kind: 'fun', opener: true, weight: 2, minElapsedMs: 5000,
      props: [{ id: 'broom', art: 'broom', layer: 'front' }, { id: 'achoo', art: 'bubble', layer: 'front', text: 'ATCHIM!', tone: 'pow', o: 0, angle: -10 }],
      play(c, r) {
        const hold = { armF: -42, elbF: -34, armB: -30, elbB: -40, lean: 8 };
        c.set({ rx: -42, ...hold });
        const hand = c.point('handF');
        c.set({ broom: { x: hand[0], y: hand[1], r: -30 }, 'broom.dust': 1 }); c.attach('broom', 'handF');
        const sweepTo = (x, strokes) => c.par(a => a.walk(x, { arms: false, speed: 22 }), a => {
          for (let i = 0; i < strokes; i++) a.to({ armF: hold.armF - 14, 'broom.tilt': 18 }, 260, 'io').to({ armF: hold.armF + 4, 'broom.tilt': -6 }, 260, 'io');
        });
        sweepTo(r.range(70, 90), 6);
        c.set({ 'broom.dust': 0 });
        c.to({ head: -12, lean: -6 }, 380, 'in'); c.mood('closed'); c.wait(160);
        c.to({ head: 12, lean: 14 }, 110, 'out'); c.say('achoo', 520, { dx: 10, dy: -17 }); c.mood('wide');
        c.to({ head: 0, lean: hold.lean }, 300); c.wait(160); c.mood('happy'); c.wait(260); c.mood('open'); c.set({ 'broom.dust': 1 });
        sweepTo(250, 12);
      } },
    garden: { kind: 'fun', weight: 2, minElapsedMs: 10000,
      props: [{ id: 'can', art: 'can', x: -26, y: -16.8, layer: 'front' }, { id: 'plant', art: 'plant', x: -18 }, { id: 'love', art: 'bubble', layer: 'front', text: 'QUE LINDEZA!', o: 0 }],
      play(c, r) {
        const grip = [-16.6, -16.8];
        c.set({ rx: -46 }); c.attach('can', 'plant');
        c.grip('plant', grip, { lean: 14 });
        c.haul(['plant'], r.range(52, 70), { speed: 28 });
        c.rest(300);
        const plant = c.pose('plant').x;
        c.walk(plant - 20, { speed: 22 });
        c.reachAt('F', 'can', [0, 0], 360, 'io', { with: { lean: 10 } }); c.grab('can');
        c.reach('F', c.point('root', [9, -33]), 420, 'io', { with: { lean: 0 } });
        c.place('can', 'F', [plant - 10, -38], 380);
        c.to({ 'can.tilt': 34 }, 320); c.set({ 'can.drops': 1 });
        c.par(a => a.wait(1300), a => { a.wait(400); a.to({ 'plant.l3': 1 }, 600, 'back'); a.to({ 'plant.fl': 1 }, 500, 'back'); });
        c.set({ 'can.drops': 0 }); c.to({ 'can.tilt': 0 }, 300);
        c.mood('happy'); c.say('love', 600);
        c.place('can', 'F', [plant - 8, -16.8], 440, 'io', { with: { lean: 10 } }); c.drop('can', { onto: 'plant' });
        c.rest(280); c.mood('open');
        c.goTo(plant + grip[0] - 10, 1);
        c.grip('plant', grip, { lean: 14, length: 300 });
        c.haul(['plant'], 236, { speed: 38 });
      } },
    delivery: { kind: 'fun', opener: true, weight: 1,
      props: [{ id: 'box', art: 'box', layer: 'front' }],
      play(c, r) {
        c.set({ rx: -42, armF: -70, elbF: -40, armB: -60, elbB: -46, lean: -4 });
        const hand = c.point('handF');
        c.set({ box: { x: hand[0] + 4, y: hand[1] + 7 } }); c.attach('box', 'handF');
        const mid = r.range(82, 104);
        const steps = Math.floor(Math.abs(mid + 42) / 26 * 1000 / 640);
        c.par(a => a.walk(mid, { arms: false, speed: 26 }), a => { for (let i = 0; i < steps; i++) a.to({ 'box.tilt': 4 }, 320).to({ 'box.tilt': -4 }, 320); a.to({ 'box.tilt': 0 }, 200); });
        c.to({ lean: -12, head: -10 }, 320); c.look(0, -1.6); c.mood('happy'); c.wait(500); c.to({ lean: -4, head: 0 }, 300); c.mood('open'); c.look(2.4, 0);
        c.walk(244, { arms: false, speed: 30 });
      } },
    // ---------------------------------------------------------------- tech set
    hacker: { kind: 'work', families: ['reading', 'verification', 'calculation'], weight: 5,
      props: r => [{ id: 'chair', art: 'chair', x: -44 }, { id: 'desk', art: 'pcdesk', x: -20 }, { id: 'screen', art: 'dash', x: 102, y: -165 },
        { id: 'got', art: 'bubble', layer: 'front', text: r.pick(['ACHEI O CABRA!', 'PEGUEI, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r) {
        const grip = [-7.2, -22.5];
        c.set({ rx: -44 + grip[0] - 10 }); c.attach('desk', 'chair');
        c.grip('chair', grip, { lean: 8 });
        c.haul(['chair'], 74 + grip[0] - 10, { speed: 30 });
        c.rest(300);
        const chair = c.pose('chair').x;
        c.walk(chair, { speed: 22 });
        // A gesture calls the big screen down from the ceiling.
        c.look(2.4, -2); c.to({ armF: -172, elbF: -8, head: -10 }, 320, 'out');
        summon(c, 'screen', -52, 1000); c.to({ armF: -105, elbF: -30 }, 1000, 'io'); c.rest(260);
        c.to(sitPose(9.5), 420, 'io');
        c.to({ head: -12 }, 260); c.look(3.2, -2);
        for (const x of [1.4, 4, 2]) c.look(x, -2, 420);
        c.to({ armF: -128, elbF: -24 }, 300); c.to({ armF: -104 }, 260); c.to({ armF: -128 }, 260);
        c.tw({ 'screen.alert': 1 }, 160); c.mood('wide'); c.wait(500); c.mood('open');
        const keys = c.point('desk', [-3.5, -17]);
        c.reach('both', keys, 360, 'io', { with: { head: 6 } }); c.look(3, 1.6, 160);
        const typing = { elbF: c.cur('elbF'), elbB: c.cur('elbB') };
        for (let i = 0; i < 9; i++) c.to({ elbF: typing.elbF - (i % 2 ? 0 : 7), elbB: typing.elbB + (i % 2 ? 6 : 0) }, 90, 'io');
        c.look(3, -2, 200); c.to({ head: -10 }, 200);
        for (let i = 0; i < 7; i++) c.to({ elbF: typing.elbF - (i % 2 ? 0 : 7), elbB: typing.elbB + (i % 2 ? 6 : 0) }, 90, 'io');
        c.tw({ 'screen.alert': 0, 'screen.lock': 1 }, 300); c.wait(300);
        c.mood('happy'); c.say('got', 700);
        c.to({ ...REST, ry: 0, legF: 0, legB: 0, kneeF: 0, kneeB: 0, head: 0, lean: 0 }, 420, 'io'); c.mood('open'); c.look(2.4, -2);
        c.to({ armF: -105, elbF: -30 }, 260); summon(c, 'screen', -165, 900); c.to({ armF: -172, elbF: -8 }, 900, 'in'); c.rest(260); c.look(2.4, 0);
        c.walk(chair + grip[0] - 10, { face: false, speed: 20 });
        c.grip('chair', grip, { lean: 8, length: 280 });
        c.haul(['chair'], 240, { speed: 40 });
      } },
    architect: { kind: 'work', families: ['composition', 'calculation', 'restoration'], weight: 5,
      props: r => [{ id: 'board', art: 'arch', x: -26 }, { id: 'hmm', art: 'bubble', layer: 'front', text: 'OXENTE?', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['ISSO, MACHO!', 'FICOU MASSA!']), tone: 'pow', o: 0 }],
      play(c, r) {
        const grip = [-24.6, -30];
        c.set({ rx: -26 + grip[0] - 10 });
        c.grip('board', grip, { lean: 6 });
        c.haul(['board'], 112 + grip[0] - 10, { speed: 30 });
        c.rest(300);
        const board = c.pose('board').x;
        const draw = (part, local, length) => {
          c.walk(board + local[0] - 11, { speed: 22 });
          c.reachAt('F', 'board', local, 260, 'io', { with: { lean: 2 } });
          c.par(a => a.to({ [`board.${part}`]: 0 }, length, 'io'), a => {
            for (let i = 0; i < 3; i++) a.reach('F', a.point('board', [local[0] + (i % 2 ? 2.4 : -1), local[1] + (i % 2 ? 1.6 : -1)]), length / 3, 'io');
          });
        };
        c.look(3.6, -1.6);
        draw('b1', [-13.5, -39], 520); draw('a1', [-6, -39], 300); draw('b2', [1.5, -39], 520); draw('a2', [1.5, -33.6], 280);
        draw('db', [1.5, -27], 560); draw('a3', [9, -39], 300); draw('cl', [16, -38.6], 560); draw('fl', [11.5, -30], 420);
        c.rest(240); c.walk(board - 34, { face: false, speed: 22 });
        c.to({ armF: -140, elbF: -112 }, 320); c.look(3.4, -1.6); c.say('hmm', 520);
        c.walk(board + 1.5 - 11, { speed: 26 }); draw('ci', [1.5, -39], 620);
        c.rest(240); c.mood('happy'); c.say('ok', 560); c.mood('open');
        c.goTo(board + grip[0] - 10, 1);
        c.grip('board', grip, { lean: 6, length: 300 });
        c.haul(['board'], 236, { speed: 40 });
      } },
    soc: { kind: 'work', families: ['reading', 'verification', 'access', 'checkpoint'], weight: 5,
      props: r => [{ id: 'wall', art: 'socwall', x: 100, y: -170 }, { id: 'crew', art: 'crew', x: -28, layer: 'front' },
        { id: 'look', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA, É ATAQUE!', 'OLHA O CABRA AÍ!']), tone: 'pow', o: 0 }],
      play(c, r) {
        const grip = [-20.3, -19];
        c.set({ rx: -28 + grip[0] - 10 });
        c.grip('crew', grip, { lean: 6 });
        c.haul(['crew'], 84 + grip[0] - 10, { speed: 28 });
        c.rest(300);
        const crew = c.pose('crew').x;
        // Walk behind the operators and call the monitor wall down.
        c.walk(126, { speed: 30 });
        c.look(2.4, -2); c.to({ armF: -172, elbF: -8, head: -10 }, 320, 'out');
        summon(c, 'wall', -46, 1100); c.to({ armF: -105, elbF: -30 }, 1100, 'io'); c.rest(260);
        c.turn(-1); c.look(3, -2.2);
        c.to({ armF: -150, elbF: -8, head: -8 }, 320, 'out'); c.set({ 'wall.alert': 1 }); c.mood('wide');
        c.par(a => a.wait(700), a => { a.wait(160); a.to({ 'crew.h1': 14 }, 260).to({ 'crew.h2': 20 }, 220); });
        c.rest(260); c.mood('open'); c.look(2.4, 0.6);
        c.walk(116, { speed: 20 });
        const shoulder = c.point('crew', [19.5, -27]);
        c.reach('F', shoulder, 320, 'io', { with: { lean: 10 } });
        for (let i = 0; i < 2; i++) c.reach('F', [shoulder[0], shoulder[1] + 1.4], 120, 'io', { with: { lean: 10 } }).reach('F', shoulder, 120, 'io', { with: { lean: 10 } });
        c.rest(260);
        c.to({ 'crew.thumbo': 1, 'crew.thumb': 0 }, 260, 'back'); c.mood('happy'); c.say('look', 700, { dx: 6, dy: -20 });
        c.to({ 'crew.thumb': 60, 'crew.thumbo': 0, 'crew.h1': 0, 'crew.h2': 0 }, 260); c.mood('open');
        c.walk(126, { face: false, speed: 22 }); c.turn(1);
        c.to({ armF: -105, elbF: -30 }, 240); c.set({ 'wall.alert': 0 });
        summon(c, 'wall', -170, 1000); c.to({ armF: -172, elbF: -8 }, 1000, 'in'); c.rest(260); c.look(2.4, 0);
        c.goTo(crew + grip[0] - 10, 1);
        c.grip('crew', grip, { lean: 6, length: 280 });
        c.haul(['crew'], 240, { speed: 36 });
      } },
    servers: { kind: 'work', families: ['checkpoint', 'restoration'], weight: 5,
      props: r => [{ id: 'rack', art: 'rack', x: -14 }, { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['TÁ NO JEITO!', 'AGORA VAI, MACHO!']), o: 0 }],
      play(c, r) {
        const grip = [-12.8, -27];
        c.set({ rx: -14 + grip[0] - 10 });
        c.grip('rack', grip, { lean: 6 });
        c.haul(['rack'], 108 + grip[0] - 10, { speed: 30 });
        c.rest(300);
        const rack = c.pose('rack').x;
        // Stand back far enough that the pulled blade and its LEDs stay in view.
        c.walk(rack - 26, { speed: 22 }); c.look(3.4, 0.4);
        c.reachAt('F', 'rack', [-9.6, -27.2], 360, 'io', { with: { lean: 14 } });
        c.par(a => a.to({ 'rack.blade': -16 }, 620, 'io'), a => a.reachAt('F', 'rack', [-25.6, -27.2], 620, 'io', { with: { lean: 4 } }));
        c.set({ 'rack.bad': 1 }); c.mood('wide'); c.to({ head: 8, lean: 8 }, 260); c.wait(620); c.mood('open');
        c.reachAt('F', 'rack', [-12, -30.8], 240, 'out', { with: { lean: 12 } }); c.reachAt('F', 'rack', [-12, -29.4], 120, 'in', { with: { lean: 12 } });
        c.set({ 'rack.bad': 0 }); c.wait(320);
        c.reachAt('F', 'rack', [-25.6, -27.2], 240, 'io', { with: { lean: 4 } });
        c.par(a => a.to({ 'rack.blade': 0 }, 560, 'io'), a => a.reachAt('F', 'rack', [-9.6, -27.2], 560, 'io', { with: { lean: 14 } }));
        c.rest(260); c.to({ head: 0 }, 160); c.mood('happy'); c.say('ok', 620); c.mood('open');
        c.goTo(rack + grip[0] - 10, 1);
        c.grip('rack', grip, { lean: 6, length: 280 });
        c.haul(['rack'], 236, { speed: 40 });
      } },
    hologram: { kind: 'fun', weight: 2, minElapsedMs: 6000,
      props: [{ id: 'puck', art: 'puck', layer: 'front' }, { id: 'wow', art: 'bubble', layer: 'front', text: 'VISH, QUE MUNDÃO!', tone: 'pow', o: 0 }],
      play(c, r) {
        c.set({ rx: -40, armF: -40, elbF: -48 });
        const hand = c.point('handF');
        c.set({ puck: { x: hand[0], y: hand[1] + 3.6 } }); c.attach('puck', 'handF');
        c.walk(r.range(78, 86), { speed: 30 });
        const spot = c.cur('rx') + 20;
        c.par(a => a.crouch(3), a => a.place('puck', 'F', [spot, 0], 420, 'io', { with: { lean: 22 } })); c.drop('puck');
        c.par(a => a.crouch(0), a => a.rest(360));
        c.walk(c.cur('rx') - 6, { face: false, speed: 18 });
        c.to({ 'puck.beam': 1 }, 300); c.to({ 'puck.globe': 1 }, 520, 'back'); c.look(3.4, -1.6); c.to({ head: -8 }, 200);
        for (let i = 0; i < 2; i++) c.to({ armF: -100, elbF: -20 }, 260, 'out').to({ armF: -62, elbF: -30 }, 300, 'io');
        c.to({ armF: -96, elbF: -40, armB: -90, elbB: -40 }, 260); c.to({ 'puck.globe': 1.18 }, 300, 'out'); c.to({ 'puck.globe': 1 }, 300, 'io');
        c.rest(260); c.mood('happy'); c.say('wow', 700); c.mood('open');
        c.to({ 'puck.globe': 0 }, 320, 'in'); c.to({ 'puck.beam': 0, head: 0 }, 240); c.look(2.4, 0);
        c.walk(spot - 20, { speed: 18 });
        c.par(a => a.crouch(3), a => a.reachAt('F', 'puck', [0, -3.6], 360, 'io', { with: { lean: 22 } })); c.grab('puck');
        c.par(a => a.crouch(0, 400), a => a.to({ lean: 0, armF: -40, elbF: -48 }, 400));
        c.walk(240, { speed: 32 });
      } }
  };
  // A frozen standing pose for reduced motion. If motion resumes, the robot simply leaves.
  const STILL = { props: [], play(c) { c.set({ rx: 100, gaze: { x: 0, y: 0 } }); c.mood('happy'); c.wait(700); c.mood('open'); c.walk(240); } };
  const SKIT_INFO = Object.freeze(Object.fromEntries(Object.entries(SKITS).map(([name, skit]) => [name,
    Object.freeze({ kind: skit.kind, families: Object.freeze([...(skit.families || [])]), weight: skit.weight, minElapsedMs: skit.minElapsedMs || 0, opener: !!skit.opener })])));

  function compile(name, { seed = 1 } = {}) {
    const skit = name === 'still' ? STILL : SKITS[name];
    if (!skit) throw Error(`Cena desconhecida: ${name}`);
    const r = tools(seeded(seed));
    const props = (typeof skit.props === 'function' ? skit.props(r) : skit.props).map(p => typeof p.text === 'function' ? { ...p, text: r.pick(['Uau!', 'Lá vai!', 'Iuhu!']) } : p);
    const clip = createClip(props);
    skit.play(clip.api, r);
    return { name, ...clip.finish() };
  }

  // ------------------------------------------------------------------ director
  // Chooses the next skit only when the room is empty. Context skits follow the
  // current phase family; fun skits appear more as the wait grows longer.
  function createDirector({ random = Math.random } = {}) {
    const r = tools(random), history = [];
    let count = 0;
    const weighted = names => {
      let ticket = r.next() * names.reduce((sum, name) => sum + SKIT_INFO[name].weight, 0);
      return names.find(name => (ticket -= SKIT_INFO[name].weight) < 0) || names.at(-1);
    };
    return Object.freeze({
      next(family = 'neutral', elapsedMs = 0) {
        const elapsed = measured(elapsedMs) ?? 0, recent = history.slice(-2);
        const fits = name => SKIT_INFO[name].minElapsedMs <= elapsed && !recent.includes(name);
        const names = Object.keys(SKIT_INFO);
        const work = names.filter(name => SKIT_INFO[name].kind === 'work' && SKIT_INFO[name].families.includes(family) && fits(name));
        const fun = names.filter(name => SKIT_INFO[name].kind === 'fun' && fits(name) && (count > 0 || SKIT_INFO[name].opener));
        let pool = work.length && (!fun.length || r.chance(count === 0 ? 0.6 : 0.45)) ? work : fun;
        if (!pool.length) pool = names.filter(name => SKIT_INFO[name].kind === 'fun' && SKIT_INFO[name].minElapsedMs <= elapsed && name !== history.at(-1));
        const name = weighted(pool);
        history.push(name); if (history.length > 8) history.shift();
        count++;
        return { name, mirror: r.chance(0.5), seed: Math.floor(r.next() * 2147483647) + 1 };
      },
      gap(first = false) { return first ? r.range(350, 1200) : r.range(1400, 4800); },
      inspect: () => ({ history: history.slice(), count })
    });
  }

  // ------------------------------------------------------------------ component
  const ROOM = `<div class="wv-ring"><span class="wv-ring-arc"></span><span class="wv-ring-head"></span></div><div class="wv-room"><svg viewBox="22 38 156 156" aria-hidden="true" focusable="false" xmlns="http://www.w3.org/2000/svg">
    <path class="wv-strip" d="M80 72h40"/><path class="wv-grid" d="M100 146 70 200M100 146 130 200M100 146 30 200M100 146 170 200M0 162h200M0 182h200"/><path class="wv-horizon" d="M0 146h200"/>
    <g transform="translate(0 146)"><g class="wv-mirror"><g class="wv-actors"></g></g></g>
  </svg><span class="wv-scan"></span><span class="wv-glass"></span></div><span class="wv-timer"></span>`;

  function mount(host, initialSnapshot = {}, options = {}) {
    if (!host?.ownerDocument?.createElement || typeof host.append !== 'function') throw new TypeError('WaitingVisuals.mount precisa de um elemento.');
    const document = host.ownerDocument, view = document.defaultView || window;
    const root = document.createElement('div'); root.className = 'waiting-visual';
    const art = document.createElement('div'); art.className = 'wv-art'; art.setAttribute('aria-hidden', 'true');
    const status = document.createElement('div'); status.className = 'wv-status';
    status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite'); status.setAttribute('aria-atomic', 'true');
    const metric = document.createElement('div'); metric.className = 'wv-metric';
    const details = document.createElement('div'); details.className = 'wv-details';
    // Only the constant room markup above enters innerHTML; receipt text uses textContent.
    art.innerHTML = ROOM;
    root.append(art, status, metric, details); host.append(root);
    // text: false — the host shows its own phase and figures; the status stays for screen readers.
    if (options.text === false) root.classList.add('wv-quiet');
    const actors = art.querySelector?.('.wv-actors') || null, mirrorGroup = art.querySelector?.('.wv-mirror') || null, timer = art.querySelector?.('.wv-timer') || null;
    const supported = !!actors && typeof actors.animate === 'function' && typeof timer?.animate === 'function';
    const now = typeof view.performance?.now === 'function' ? () => view.performance.now() : () => 0;
    const elapsedClock = createElapsedClock(now), mountedAt = now();
    const random = typeof options.random === 'function' ? options.random : options.reactionSeed != null ? seeded(options.reactionSeed) : Math.random;
    const director = createDirector({ random });
    const media = typeof view.matchMedia === 'function' ? view.matchMedia('(prefers-reduced-motion: reduce)') : null;
    let current, destroyed = false, visible = true, intersecting = false, moving = false, first = true;
    let motionEnabled = options.motionEnabled !== false;
    let mode = 'empty', skit = '', mirrored = false, anims = [], clock = null;
    const timeline = document.timeline;

    function clearScene() {
      for (const animation of anims) animation.cancel();
      anims = [];
      if (actors) actors.innerHTML = '';
      skit = ''; root.dataset.skit = '';
    }
    function startClock(length, done) {
      clock = timer.animate([{ opacity: 0 }, { opacity: 0 }], { duration: length });
      const mine = clock;
      clock.onfinish = () => { if (mine === clock && !destroyed) { clock = null; done(); } };
      if (!moving) clock.pause();
    }
    function play(name, flip, seed) {
      clearScene();
      const clip = compile(name, { seed });
      actors.innerHTML = clip.markup;
      mirrorGroup.setAttribute('transform', flip ? 'matrix(-1 0 0 1 200 0)' : 'matrix(1 0 0 1 0 0)');
      mirrored = flip; root.dataset.mirror = String(flip); root.dataset.skit = name; skit = name; mode = 'skit';
      const find = target => actors.querySelectorAll(`[data-c="${target}"]`);
      for (const { target, style } of clip.statics) for (const node of find(target)) Object.assign(node.style, style);
      for (const track of clip.tracks) for (const node of find(track.target)) anims.push(node.animate(track.keyframes, { duration: clip.duration, fill: 'both' }));
      startClock(clip.duration, () => { clearScene(); mode = 'empty'; schedule(); });
      const start = timeline?.currentTime;
      if (moving && Number.isFinite(start)) for (const animation of [...anims, clock]) animation.startTime = start;
      if (!moving) for (const animation of anims) animation.pause();
    }
    const wantsSkits = () => !destroyed && supported && current?.state === 'running' && !!current.operationId;
    function schedule() {
      if (mode !== 'empty' || !wantsSkits() || !moving) return;
      mode = 'gap';
      startClock(director.gap(first), () => {
        mode = 'empty';
        if (!wantsSkits()) return;
        const elapsed = Math.max(now() - mountedAt, elapsedClock.read() ?? 0);
        const pick = director.next(current.family, elapsed);
        first = false;
        play(pick.name, pick.mirror, pick.seed);
      });
    }
    function pauseAll() { for (const animation of clock ? [...anims, clock] : anims) animation.pause(); }
    function resumeAll() {
      if (mode === 'skit' && clock) {
        const time = clock.currentTime ?? 0, start = timeline?.currentTime;
        for (const animation of [...anims, clock]) {
          if (Number.isFinite(start)) animation.startTime = start - time;
          else { animation.currentTime = time; animation.play(); }
        }
      } else clock?.play();
    }
    function syncMotion() {
      if (destroyed || !current) return;
      const reduced = !!media?.matches;
      const next = current.live && motionEnabled && !reduced && visible && intersecting && !document.hidden && root.isConnected;
      root.dataset.motion = next ? 'running' : 'static';
      // Reduced motion (or a host-disabled animation) in an empty room shows a still robot.
      if (!next && supported && current.live && (reduced || !motionEnabled) && (mode === 'empty' || mode === 'gap')) {
        clock?.cancel(); clock = null; play('still', false, 1); mode = 'skit';
      }
      if (next !== moving) { moving = next; if (moving) resumeAll(); else pauseAll(); }
      schedule();
    }
    function update(snapshot = {}) {
      if (destroyed) return false;
      const previous = current;
      current = derive(snapshot);
      // A committed segment stays inside the pending operation and keeps its scene family.
      if (current.state === 'running' && current.partialCheckpoint && previous?.live && current.operationId === previous.operationId)
        current = { ...current, family: previous.family };
      elapsedClock.observe(current);
      root.dataset.state = current.state; root.dataset.family = current.family;
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
    function setVisible(shown) {
      if (destroyed) return;
      visible = !!shown; root.dataset.visible = String(visible); root.inert = !visible;
      root.setAttribute('aria-hidden', String(!visible)); syncMotion();
    }
    document.addEventListener('visibilitychange', syncMotion);
    if (media?.addEventListener) media.addEventListener('change', syncMotion);
    else if (media?.addListener) media.addListener(syncMotion);
    // No IntersectionObserver means static, not an unbounded offscreen loop.
    const observer = typeof view.IntersectionObserver === 'function' ? new view.IntersectionObserver(entries => {
      if (destroyed) return;
      for (const entry of entries) if (entry.target === root) intersecting = entry.isIntersecting && entry.intersectionRatio > 0;
      syncMotion();
    }, { threshold: 0 }) : null;
    observer?.observe(root);
    update(initialSnapshot);
    function destroy() {
      if (destroyed) return;
      destroyed = true; root.dataset.motion = 'static';
      for (const animation of anims) animation.cancel();
      clock?.cancel(); anims = []; clock = null;
      observer?.disconnect();
      document.removeEventListener('visibilitychange', syncMotion);
      if (media?.removeEventListener) media.removeEventListener('change', syncMotion);
      else if (media?.removeListener) media.removeListener(syncMotion);
      root.remove();
    }
    // Results settle immediately: the host closes and the room goes with it.
    function complete() { destroy(); return false; }
    return Object.freeze({ element: root, update, setVisible, setMotionEnabled, complete, destroy,
      inspect: () => ({ mode, skit, mirrored, moving, family: current?.family, ...director.inspect() }),
      // Development aid for previews: plays a named skit immediately in an empty room.
      preview(name, { mirror = false, seed = 1 } = {}) { if (supported && !destroyed) { clock?.cancel(); clock = null; play(name, mirror, seed); } } });
  }
  return Object.freeze({ derive, mount, compile, createDirector, createElapsedClock, skits: SKIT_INFO });
})();
