/* Loading "room" for real operations. Load as a classic script with waiting-visuals.css.

   mount(host, receipt, options); update replaces the receipt.
   setVisible(false) pauses hidden panels; destroy before removing the host.

   The loader is a ring around a small round room. Inside it, a robot occasionally
   performs a short skit. Every skit starts and ends with an empty room: the robot
   walks in, brings its props, acts, takes the props away and walks out. A new skit
   is only chosen while the room is empty, so phase changes never cut a scene.
   Most skits are composed from interchangeable pieces (see "composition"): one of
   many arrivals (walking, a car, a bike, a jetpack, a secret door at the back...),
   the activity, an optional break (coffee, a rest, an update...) and a departure,
   while ambient extras cross the room on their own clock.
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
  // dz/deep move the whole robot in depth (a doorway at the back, or close to the glass),
  // ghost fades it (teleports, darkness) and fly lifts the body above its floor shadow.
  const ROBOT_CHANNELS = [
    ['rx', 'tx', -40], ['dz', 'ty', 0], ['deep', 'scale', 1], ['ghost', 'op', 1], ['fly', 'ty', 0],
    ['ry', 'ty', 0], ['face', 'sx', 1], ['lean', 'rot', 0], ['head', 'rot', 0],
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
  const ROBOT = `<g class="wv-robot" data-c="rx"><g data-c="dz"><g data-c="deep"><g data-c="ghost"><ellipse class="wv-floor-glow" cy=".6" rx="14" ry="2.6"/><ellipse class="wv-shadow" cy=".5" rx="9.5" ry="1.7"/><g data-c="fly"><g data-c="ry"><g data-c="face">
    ${leg('B')}
    ${joint(0, HIP_Y, 'lean', `${arm('B')}
      <rect class="wv-waist" x="-3.6" y="-6" width="7.2" height="6" rx="1.6"/>
      <path class="wv-shell" d="M-7.4 -11.2q0-2.8 2.8-2.8h9.2q2.8 0 2.8 2.8l-.9 5.4q-6.4 2.9-12.8 0Z"/>
      <path class="wv-shine" d="M-5.2 -12.6h4.6"/>
      <path class="wv-shell wv-belt" d="M-5.8 -1.8h11.6l-1.3 3.5h-9Z"/>
      <circle class="wv-core-glow" cx="-3" cy="-7.4" r="2.4"/><circle class="wv-core" cx="-3" cy="-7.4" r="1.3"/>
      ${joint(0, NECK_Y, 'head', head)}`)}
    ${leg('F')}
  </g></g></g></g></g></g></g>`;
  const ROBOT_ARM = `<g class="wv-robot" data-c="rx"><g data-c="dz"><g data-c="deep"><g data-c="ghost"><g data-c="fly"><g data-c="ry"><g data-c="face">${joint(0, HIP_Y, 'lean', arm('F'))}</g></g></g></g></g></g></g>`;

  // ------------------------------------------------------------------ props
  // Prop-local units: origin at the floor contact (or the grip for hand tools).
  // Sub-channels are [kind, initial]. half is the horizontal reach used to keep
  // entrances and exits outside the round window.
  // One hardware vocabulary for everything: gunmetal plates with a lit top bevel, dark
  // glass with scan lines and a sheen, glowing status LEDs and neon accents with a halo.
  const plate = (x, y, w, h, r = 1.4, tone = '') => `<rect class="wv-metal${tone}" x="${n3(x)}" y="${n3(y)}" width="${n3(w)}" height="${n3(h)}" rx="${r}"/><path class="wv-bevel" d="M${n3(x + r)} ${n3(y + 0.5)}h${n3(Math.max(0, w - 2 * r))}"/>`;
  const glass = (x, y, w, h, r = 0.8) => `<rect class="wv-glass" x="${n3(x)}" y="${n3(y)}" width="${n3(w)}" height="${n3(h)}" rx="${r}"/>`;
  function scanlines(x, y, w, h, step = 0.9) {
    let d = '';
    for (let k = y + step / 2; k < y + h; k += step) d += `M${n3(x)} ${n3(k)}h${n3(w)}`;
    return `<path class="wv-scanlines" d="${d}"/>`;
  }
  const sheen = (x, y, w, h) => `<path class="wv-sheen" d="M${n3(x + w * 0.1)} ${n3(y + h)}L${n3(x + w * 0.46)} ${n3(y)}h${n3(w * 0.15)}L${n3(x + w * 0.27)} ${n3(y + h)}Z"/>`;
  // A powered screen: bezel, glowing glass with its content, scan lines and a sheen.
  const monitor = (x, y, w, h, content = '', tone = '') => `<rect class="wv-bezel" x="${n3(x - 1)}" y="${n3(y - 1)}" width="${n3(w + 2)}" height="${n3(h + 2)}" rx="1.3"/><g class="wv-screen${tone}">${glass(x, y, w, h)}${content}</g>${scanlines(x, y, w, h)}${sheen(x, y, w, h)}`;
  const led = (x, y, tone = '', r = 0.7) => `<circle class="wv-led${tone}" cx="${n3(x)}" cy="${n3(y)}" r="${r}"/>`;
  const vents = (x, y, count, width = 4, gap = 1.1) => `<path class="wv-vent" d="${Array.from({ length: count }, (_, i) => `M${n3(x)} ${n3(y + i * gap)}h${n3(width)}`).join('')}"/>`;
  const neon = (d, tone = '') => `<path class="wv-neon${tone}" d="${d}"/>`;
  const hazard = (x, y, w, h) => `<rect class="wv-hazard-bg" x="${n3(x)}" y="${n3(y)}" width="${n3(w)}" height="${n3(h)}"/><path class="wv-hazard" d="${Array.from({ length: Math.ceil(w / 2.4) + 1 }, (_, i) => `M${n3(x + i * 2.4 - h)} ${n3(y + h)}l${n3(h)} ${n3(-h)}`).join('')}"/>`;

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
  // Digital rain: columns of real glyphs whose white-hot head falls row by row and
  // leaves a green trail, each column at its own speed. Deterministic per seed.
  const GLYPHS = 'ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾗﾘﾙﾚﾛﾜ012345789Z';
  function rain(x, y, width, height, columns, seed) {
    let s = (seed * 7919 + 104729) % 233280;
    const next = () => (s = (s * 9301 + 49297) % 233280) / 233280;
    const size = Math.min(2.7, width / columns), rowsCount = Math.max(3, Math.floor(height / size));
    let cells = '';
    for (let column = 0; column < columns; column++) {
      const cx = x + (column + 0.5) * width / columns, speed = 1.2 + next() * 1.6, start = next() * speed;
      for (let row = 0; row < rowsCount; row++) {
        const phase = (((start - row * speed / (rowsCount + 5)) % speed) + speed) % speed;
        cells += `<text x="${n3(cx)}" y="${n3(y + (row + 0.9) * size)}" style="animation-duration:${n3(speed)}s;animation-delay:-${n3(phase)}s">${GLYPHS[Math.floor(next() * GLYPHS.length)]}</text>`;
      }
    }
    return `<g class="wv-matrix" style="font-size:${n3(size * 1.08)}px">${cells}</g>`;
  }
  const wheel = (id, part, x, y, r) => `<g transform="translate(${x} ${y})"><g data-c="${id}.${part}"><circle class="wv-wheel" r="${r}"/><circle class="wv-hub" r="${n3(r * 0.46)}"/><path class="wv-spoke" d="M${n3(-r * 0.8)} 0h${n3(r * 1.6)}"/></g></g>`;
  const wheels = (r, ...parts) => ({ r, parts });
  // Shadow plus two wheels: the base of every rolling piece of furniture.
  const rolling = (id, half, r, spread, shadow = half - 4) => `<ellipse class="wv-shadow" cy=".5" rx="${shadow}" ry="1.5"/>${wheel(id, 'wl', -spread, -r, r)}${wheel(id, 'wr', spread, -r, r)}`;
  const ART = {
    cart: { half: 23, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.4, 'wl', 'wr'),
      svg: id => `<path class="wv-cone" d="M-13 -12.6h26l4 12.6h-34Z"/><path class="wv-strut" d="M-11 -13.4V-3M11 -13.4V-3M-17 -15l-2.6-12"/><path class="wv-grip" d="M-19.3 -27h-3"/>
        ${plate(-17, -16, 34, 3, 1)}${neon('M-14 -12.6h28')}${led(14.6, -14.5, ' wv-led-ok', 0.5)}${rolling(id, 21, 2.4, 11)}` },
    machine: { half: 9, ch: { light: ['op', 0.2], drip: ['op', 0] },
      svg: id => `${plate(-4.5, -17, 11, 17, 1.8)}<rect class="wv-metal-dark" x="-3.4" y="-15.6" width="8.8" height="2.2" rx=".6"/>
        <path class="wv-metal" d="M-4.5 -14.5h-4.4v2.6h4.4"/><path class="wv-edge" d="M-7.4 -11.9v1.1"/>
        ${glass(-2.2, -10.5, 6.6, 4.6, 1)}${neon('M-.8 -8.2h3.8')}${vents(-2.6, -3.4, 2, 7.2)}
        <path class="wv-coffee" data-c="${id}.drip" d="M-7.4 -10.4V-6"/><circle class="wv-led" data-c="${id}.light" cx="-6.6" cy="-15.6" r="1"/>` },
    mug: { half: 5, ch: { steam: ['op', 0] },
      svg: id => `<g class="wv-steam" data-c="${id}.steam"><path d="M-.9 -7.4c-.8-1 .8-1.9 0-2.9"/><path d="M1.2 -7.8c-.8-1 .8-1.9 0-2.9"/></g>
        <path class="wv-edge" d="M-2.6 -4.6h-1.2a1.2 1.2 0 0 0 0 2.4h1.2"/>
        <path class="wv-mug" d="M-2.6 -5.8h5.2v4.3a1.5 1.5 0 0 1-1.5 1.5h-2.2a1.5 1.5 0 0 1-1.5-1.5Z"/>${neon('M-2.2 -3.9h4.4')}` },
    sheet: { half: 5,
      svg: () => `<rect class="wv-paper" x="-4.5" y="-11.5" width="9" height="11.5" rx=".8"/><path class="wv-paper-lines" d="M-2.6 -9h5.2M-2.6 -6.8h5.2M-2.6 -4.6h3.4M-2.6 -2.4h4.4"/>` },
    tray: { half: 6,
      svg: () => `${plate(-5.5, -7, 11, 7, 1.2)}${neon('M-4.2 -6.6h8.4')}${vents(-3, -3.6, 2, 6)}` },
    // Comic onomatopoeia: no balloon, just outlined lettering with action strokes.
    // The origin is the point it bursts out of (above the head or a prop).
    bubble: { half: 34, svg: (id, p) => comic(p.text, p.angle, p.tone) },
    zzz: { half: 12,
      svg: () => `<g class="wv-unmirror"><g class="wv-zzz"><path d="M0 -3h3l-3 3h3"/><path d="M4 -9h3.6l-3.6 3.6h3.6"/><path d="M9 -16h4.4l-4.4 4.4h4.4"/></g></g>` },
    cushion: { half: 15,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="14" ry="1.4"/><path class="wv-cushion" d="M-14 0c-1.6-6 3-10 14-10s15.6 4 14 10Z"/>${neon('M-8 -6.2q8 2.6 16 0', ' wv-dim')}<path class="wv-bevel" d="M-7 -9.2q7-1.4 14 0"/>` },
    board: { half: 20, ch: { wl: ['rot', 0], wr: ['rot', 0], ax: ['dash', 1], b1: ['dash', 1], b2: ['dash', 1], b3: ['dash', 1], tr: ['dash', 1] },
      wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-12 -17V-3M12 -17V-3M-17 -26h-2.6"/>
        <rect class="wv-board" x="-17" y="-42" width="34" height="25" rx="1.8"/><rect class="wv-board-edge" x="-15.4" y="-40.4" width="30.8" height="21.8" rx=".8"/>
        ${sheen(-15.4, -40.4, 30.8, 21.8)}${plate(-15, -17.8, 30, 1.8, 0.6)}
        <path class="wv-chalk" pathLength="1" data-c="${id}.ax" d="M-11 -38v17h22"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b1" d="M-7.5 -21v-5.5h3.6v5.5"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b2" d="M-1.8 -21v-9.5h3.6v9.5"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b3" d="M3.9 -21v-13h3.6v13"/>
        <path class="wv-chalk wv-chalk-hot" pathLength="1" data-c="${id}.tr" d="M-6 -30.5 0 -34.5l5.8-4.6m-2.6-.2 2.6.2-.4 2.6"/>
        ${rolling(id, 19, 2.3, 12)}` },
    stand: { half: 15, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-8 -4.4h16M0 -4.4V-22M-8 -4.4l-3.5-16h-2.6M-9 -27h-3v-3"/>${plate(-4, -23.4, 8, 2, 0.8)}
        <rect class="wv-paper" x="-8.5" y="-45" width="17" height="22" rx="1"/>
        <path class="wv-paper-lines" d="M-5.4 -41h10.8M-5.4 -37.8h10.8M-5.4 -34.6h7M-5.4 -31.4h10.8M-5.4 -28.2h6"/>
        ${rolling(id, 15, 2.2, 6.5, 11)}` },
    lens: { half: 12,
      svg: () => `<path class="wv-grip" d="M0 0l4.4-4.4"/><circle class="wv-lens" cx="7.4" cy="-7.4" r="4.2"/><circle class="wv-lens-rim" cx="7.4" cy="-7.4" r="4.2"/><path class="wv-glint" d="M5.3 -8.4a2.6 2.6 0 0 1 2-2.1"/>` },
    cabinet: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0], led: ['op', 0.2] }, wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-10 -24h-2.8v-6h2.8"/>${plate(-10, -33, 20, 28.4, 1.6)}
        ${[-31.4, -22.6, -13.8].map(y => `<rect class="wv-drawer" x="-8.4" y="${y}" width="16.8" height="7.8" rx=".7"/><path class="wv-bevel" d="M-7.6 ${n3(y + 0.5)}h15.2"/>${neon(`M-3.5 ${n3(y + 4.6)}h7`)}<rect class="wv-label" x="-2.4" y="${n3(y + 1.5)}" width="4.8" height="1.5" rx=".3"/>`).join('')}
        <path class="wv-slot" d="M-5.5 -33h11"/><circle class="wv-led" data-c="${id}.led" cx="6.4" cy="-29.4" r=".9"/>
        ${rolling(id, 14, 2.3, 6.5, 11)}` },
    folder: { half: 7,
      svg: () => `<path class="wv-folder" d="M-5.6 0V-9.6q0-.8.8-.8h3.4l1.3 1.4h4.9q.8 0 .8.8V0Z"/><path class="wv-folder-front" d="M-5.6 0l.9-6.8h11.2L5.6 0Z"/>${neon('M-3.4 -3.6h6.8')}` },
    printer: { half: 10, ch: { led: ['op', 0.2] },
      svg: id => `${plate(-6, -15.5, 12, 3.5, 1, ' wv-metal-dark')}${plate(-9, -12.5, 18, 12.5, 2)}<path class="wv-slot" d="M-5 -12.3h10"/>
        ${glass(-5.5, -8.4, 7, 3.6, 0.8)}${neon('M-4.6 -6.6h5')}<circle class="wv-led" data-c="${id}.led" cx="5.4" cy="-6.6" r="1"/>${vents(-6.6, -2.8, 2, 13.2)}` },
    frame: { half: 12,
      svg: () => `${plate(-11.5, -12.5, 23, 12.5, 1.4)}${[-9.4, -2.8, 3.8].map(x => `<rect class="wv-socket" x="${x}" y="-9.6" width="5.6" height="5.6" rx=".9"/>`).join('')}${neon('M-9 -1.8h18', ' wv-dim')}` },
    block: { half: 3,
      svg: (id, p) => `<rect class="wv-block wv-block-${p.tone || 1}" x="-2.6" y="-2.6" width="5.2" height="5.2" rx=".9"/><path class="wv-block-core wv-block-${p.tone || 1}" d="M-1 -.9h2M-1 .7h1.2"/>` },
    terminal: { half: 13, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-7 -4.4h14M0 -4.4V-24M-8 -31h-3.4v8"/>${plate(-8.5, -40, 17, 16, 2)}
        ${monitor(-6.4, -38, 12.8, 9, `<path class="wv-lock" d="M-1.8 -33v-1.7a1.8 1.8 0 0 1 3.6 0v1.7"/><rect class="wv-lock-body" x="-2.6" y="-33.2" width="5.2" height="3.4" rx=".7"/>`, ' wv-screen-hot')}
        <circle class="wv-button" cx="-4" cy="-26.4" r="1.2"/><circle class="wv-led-wait" cx="4" cy="-26.4" r="1.1"/>
        ${rolling(id, 13, 2.2, 5, 10)}` },
    radio: { half: 9, ch: { notes: ['op', 0] },
      svg: id => `<path class="wv-strut" d="M-4.5 -9v-2.6h9V-9M5.4 -9l3-6"/>${led(8.4, -15, ' wv-led-hot', 0.6)}${plate(-7.5, -9, 15, 9, 2)}
        <circle class="wv-speaker" cx="-3.4" cy="-4.5" r="2.7"/><circle class="wv-speaker" cx="3.4" cy="-4.5" r="2.7"/><circle class="wv-led" cx="-3.4" cy="-4.5" r=".7"/><circle class="wv-led" cx="3.4" cy="-4.5" r=".7"/>
        <g transform="translate(-2 -14)"><g data-c="${id}.notes"><g class="wv-unmirror">${[[0, 0, ''], [7, -4, ' wv-note-b'], [-6, -7, ' wv-note-c']].map(([x, y, late]) =>
          `<g transform="translate(${x} ${y})"><g class="wv-note${late}"><path class="wv-note-stem" d="M1.6 0V-5.2l2.4.8"/><ellipse class="wv-note-head" cx=".5" cy=".1" rx="1.35" ry="1"/></g></g>`).join('')}</g></g></g>` },
    ball: { half: 3,
      svg: (id, p) => `<circle class="wv-ball wv-ball-${p.tone || 1}" r="2.3"/><circle class="wv-ball-core" r="1"/><path class="wv-ball-shine" d="M-1 -1.2a1.4 1.4 0 0 1 1.1-.7"/>` },
    plane: { half: 8, ch: { flat: ['op', 1], fold: ['op', 0] },
      svg: id => `<g data-c="${id}.flat"><rect class="wv-paper" x="-4" y="-5.5" width="8" height="11" rx=".7"/><path class="wv-paper-lines" d="M-2.2 -3h4.4M-2.2 -.8h4.4"/></g>
        <g data-c="${id}.fold"><path class="wv-paper" d="M-6 0 6.5-1.4-6-4.6-3.2-1.4Z"/><path class="wv-paper-lines" d="M-3.2 -1.4 6.5-1.4M-6 0l2.8-1.4"/></g>` },
    skate: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(1.6, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="11" ry="1.3"/><path class="wv-cone" d="M-10 -3h20l2 3h-24Z"/><path class="wv-deck" d="M-12.5 -5.6q.6 1.8 2.6 1.8h19.8q2 0 2.6-1.8"/>${neon('M-9 -3.2h18')}
        ${wheel(id, 'wl', -6.5, -1.6, 1.6)}${wheel(id, 'wr', 6.5, -1.6, 1.6)}` },
    broom: { half: 8, ch: { dust: ['op', 0] },
      svg: id => `<path class="wv-strut" d="M0 -3V17"/>${neon('M0 6v4')}<path class="wv-bristle" d="M-3.6 16.6h7.2l1.6 5.4h-10.4Z"/><path class="wv-edge" d="M-1.8 18v3.6M0 18v3.6M1.8 18v3.6"/>
        <g data-c="${id}.dust" class="wv-dust"><circle cx="-6" cy="21" r="1.6"/><circle cx="7" cy="20.4" r="1.3"/><circle cx="-9" cy="19" r="1"/></g>` },
    can: { half: 13, ch: { drops: ['op', 0] },
      svg: id => `<path class="wv-strut" d="M-3.4 3.4q3.4-6.4 6.8 0"/><path class="wv-metal" d="M-4.4 3h9.4l-.8 7.4h-7.8Z"/><path class="wv-metal" d="M4.8 5.2 11.4 1l.8 1.2-6.8 5"/>${neon('M-3.6 6.2h7.8')}
        <g data-c="${id}.drops" class="wv-drops"><circle cx="13" cy="4" r=".75"/><circle cx="12.2" cy="6" r=".65"/><circle cx="13.6" cy="7.2" r=".7"/></g>` },
    plant: { half: 18, ch: { wl: ['rot', 0], wr: ['rot', 0], l3: ['scale', 0], fl: ['scale', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-12 -5.2l-2.4-11.6h-2.6"/>${plate(-12, -6.4, 24, 2.2, 0.8)}
        <g transform="translate(4 0)"><path class="wv-stem" d="M0 -16V-27"/>
          <g transform="translate(0 -19)"><path class="wv-leaf" d="M0 0q-6.4-.6-7.6-5.4 5.4-.2 7.6 5.4Z"/></g>
          <g transform="translate(0 -22.5)"><path class="wv-leaf" d="M0 0q6.4-.6 7.6-5.4-5.4-.2-7.6 5.4Z"/></g>
          <g transform="translate(0 -25.6)"><g data-c="${id}.l3"><path class="wv-leaf" d="M0 0q-5.4-.6-6.4-4.6 4.6-.2 6.4 4.6Z"/></g></g>
          <g transform="translate(0 -28)"><g data-c="${id}.fl"><circle class="wv-petal" cx="-2" r="1.9"/><circle class="wv-petal" cx="2" r="1.9"/><circle class="wv-petal" cy="-2" r="1.9"/><circle class="wv-petal" cy="2" r="1.9"/><circle class="wv-flower" r="1.4"/></g></g>
          <path class="wv-pot" d="M-5.4 -16h10.8l-1.6 9.6h-7.6Z"/>${neon('M-4.6 -13.6h9.2')}</g>
        ${rolling(id, 17, 2.2, 8, 13)}` },
    box: { half: 10,
      svg: () => `${plate(-9, -15, 18, 15, 1.4)}${hazard(-9, -4.4, 18, 2.2)}<path class="wv-edge" d="M-9 -11.4h18"/>${neon('M-5.6 -8.2h11.2')}${led(6.4, -13.2, ' wv-led-ok', 0.5)}` },
    // ---- tech set: desk, chair, ceiling screens, SOC crew, rack, hologram
    pcdesk: { half: 18, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2.2, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-14 -13.6V-3M14 -13.6V-3"/><path class="wv-edge" d="M-14 -7h19.5"/>
        ${plate(5.5, -12.6, 7, 10, 1)}${neon('M9 -11v7', ' wv-rgb')}${vents(6.6, -4.2, 2, 1.4, 0.9)}
        ${plate(-17, -16, 34, 2.4, 1)}${neon('M-15 -13.4h30', ' wv-dim')}
        <rect class="wv-keys" x="-9.5" y="-17.5" width="12" height="1.5" rx=".5"/>${neon('M-8.5 -16.75h10', ' wv-rgb')}<ellipse class="wv-metal" cx="6" cy="-16.6" rx="1.5" ry=".8"/>
        ${rolling(id, 18, 2.2, 14, 16)}` },
    chair: { half: 10,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="7" ry="1.2"/><path class="wv-strut" d="M0 -8.4V-2.6M-4.6 -2.4h9.2"/>
        <circle class="wv-wheel" cx="-4.2" cy="-1.3" r="1.2"/><circle class="wv-wheel" cx="4.2" cy="-1.3" r="1.2"/>
        ${plate(-6.5, -10, 13, 1.9, 0.9)}<rect class="wv-chair-back" x="-8.6" y="-23" width="2.8" height="14" rx="1.2"/>${neon('M-7.2 -21.4v9.6')}` },
    // A wall screen hung from the ceiling with a live attack map (origin: bottom edge).
    dash: { half: 28, ch: { alert: ['op', 0], lock: ['op', 0] },
      svg: id => `<path class="wv-cable" d="M-20 -37V-150M20 -37V-150"/>
        ${monitor(-27, -36, 54, 36, `<path class="wv-screen-grid" d="M-25 -27h50M-25 -18h50M-25 -9h50M-13 -34v32M0 -34v32M13 -34v32"/>
        <path class="wv-land" d="M-23 -27q3-5 9-3t6 4-3 5-8 1-4-7ZM-4 -30q6-3 11 0t3 6-6 3-7-3-1-6ZM9 -20q5-2 9 1t1 6-6 1-4-8ZM-17 -14q3 2 2 6t-4 2-1-8Z"/>
        <path class="wv-arc" pathLength="1" d="M-16 -24Q-6 -38 3 -26"/><path class="wv-arc wv-arc-b" pathLength="1" d="M3 -26Q13 -36 16 -15"/><path class="wv-arc wv-arc-c" pathLength="1" d="M-12 -9Q1 -21 16 -15"/>
        <circle class="wv-pulse" cx="16" cy="-15" r="1.7"/><circle class="wv-node" cx="-16" cy="-24" r=".9"/><circle class="wv-node" cx="3" cy="-26" r=".9"/><circle class="wv-node wv-node-hot" cx="16" cy="-15" r="1"/><circle class="wv-node" cx="-12" cy="-9" r=".9"/>
        <path class="wv-screen-text" d="M-24 -33.6h9M-24 -32h5M19 -33.6h5M-24 -4h6M-24 -2.6h4"/>
        <rect class="wv-bar" x="20" y="-8" width="1.6" height="5"/><rect class="wv-bar wv-bar-b" x="22.4" y="-8" width="1.6" height="5"/>`)}
        <g data-c="${id}.alert"><rect class="wv-alert-frame" x="-26.3" y="-35.3" width="52.6" height="34.6" rx="1.2"/><path class="wv-alert-icon" d="M-1 -23l3.6 6.2h-7.2Z"/><path class="wv-alert-mark" d="M-1 -21.4v2.2"/></g>
        <g data-c="${id}.lock"><rect class="wv-lock-ok-body" x="-3.4" y="-21" width="6.8" height="5" rx="1"/><path class="wv-lock-ok" d="M-2.2 -21v-1.8a2.2 2.2 0 0 1 4.4 0v1.8"/></g>
        <rect class="wv-handle" x="-3" y="-.4" width="6" height="1.2" rx=".6"/>${led(24.6, -37, ' wv-led-ok', 0.5)}` },
    // A SOC wall: six monitors of digital rain hanging from the ceiling (origin: bottom edge).
    socwall: { half: 34, ch: { alert: ['op', 0] },
      svg: id => {
        const screens = [[-33, -40], [-11, -40], [11, -40], [-33, -20.5], [-11, -20.5], [11, -20.5]]
          .map(([x, y], i) => monitor(x + 0.5, y + 0.5, 20, 17.5, rain(x + 1, y + 0.8, 18, 16.6, 7, i + 1), ' wv-screen-matrix')).join('');
        return `<path class="wv-cable" d="M-26 -42V-150M26 -42V-150"/>${plate(-34.5, -41.5, 69, 40, 1.6, ' wv-metal-dark')}${screens}
          <g data-c="${id}.alert"><rect class="wv-alert-frame" x="-10.6" y="-39.6" width="20.2" height="17.7" rx=".8"/><path class="wv-alert-icon" d="M-.5 -36l4.2 7.2h-8.4Z"/><path class="wv-alert-mark" d="M-.5 -34.2v2.6"/></g>
          ${led(-31.8, -0.6, ' wv-led-ok', 0.45)}${led(-29.8, -0.6, ' wv-led-ok', 0.45)}`;
      } },
    // Two operators seen from behind on rolling chairs; heads and a thumb can move.
    // Drawn 30% larger than the robot: they sit closer to the viewer.
    crew: { half: 26, ch: { h1: ['rot', 0], h2: ['rot', 0], thumb: ['rot', 60], thumbo: ['op', 0] },
      svg: id => [[-13, 1], [13, 2]].map(([x, i]) => `<g transform="translate(${x} 0) scale(1.3)">
          <ellipse class="wv-shadow" cy=".4" rx="7" ry="1.2"/><path class="wv-strut" d="M0 -8.4V-2.6M-4.6 -2.4h9.2"/>
          <circle class="wv-wheel" cx="-4.2" cy="-1.3" r="1.2"/><circle class="wv-wheel" cx="4.2" cy="-1.3" r="1.2"/>
          <path class="wv-op" d="M-6.8 -10c0-7.6 2-12.6 6.8-12.6s6.8 5 6.8 12.6Z"/><path class="wv-op-rim" d="M-5.4 -19.4q5.4-4.6 10.8 0M-6.4 -12.6q-.6-3.4.4-6"/>
          <g transform="translate(0 -26)"><g data-c="${id}.h${i}"><circle class="wv-op" r="4.2"/><path class="wv-op-rim" d="M-3.4 -2.4a4.2 4.2 0 0 1 6.8 0"/><path class="wv-headset" d="M-4.6 0a4.6 4.6 0 0 1 9.2 0"/><circle class="wv-op-ear" cx="4.5" cy=".4" r="1.2"/></g></g>
          ${i === 2 ? `<g transform="translate(6 -19)"><g data-c="${id}.thumbo"><g data-c="${id}.thumb"><path class="wv-op-arm" d="M0 0 2.6-6.4"/><circle class="wv-op" cx="2.9" cy="-7.4" r="1.4"/><path class="wv-op-rim" d="M2.6 -9.4v-1.4"/></g></g></g>` : ''}
          <rect class="wv-chair-back" x="-5.6" y="-16" width="11.2" height="6.6" rx="2"/>${neon('M-4.2 -12.7h8.4', ' wv-dim')}${plate(-6.5, -10, 13, 1.9, 0.9)}
        </g>`).join('') + `<path class="wv-edge" d="M-19 -3.1h38"/>` },
    // Architecture whiteboard: boxes, arrows, a database, a cloud and a flow (origin: floor).
    arch: { half: 26, ch: { wl: ['rot', 0], wr: ['rot', 0], b1: ['dash', 1], a1: ['dash', 1], b2: ['dash', 1], a2: ['dash', 1], db: ['dash', 1], a3: ['dash', 1], cl: ['dash', 1], fl: ['dash', 1], ci: ['dash', 1] },
      wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => `<path class="wv-strut" d="M-15 -18V-3M15 -18V-3M-22 -30h-2.6"/>
        <rect class="wv-board" x="-22" y="-46" width="44" height="28" rx="1.8"/><rect class="wv-board-edge" x="-20.4" y="-44.4" width="40.8" height="24.8" rx=".8"/>
        ${sheen(-20.4, -44.4, 40.8, 24.8)}${plate(-20, -18.8, 40, 1.8, 0.6)}
        <path class="wv-chalk" pathLength="1" data-c="${id}.b1" d="M-18 -42h9v6h-9Z"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.a1" d="M-8.4 -39h5m-1.6-1.4 1.6 1.4-1.6 1.4"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.b2" d="M-3 -42h9v6h-9Z"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.a2" d="M1.5 -35.4v3.6m-1.3-1.4 1.3 1.4 1.3-1.4"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.db" d="M-2.5 -29.4a4 1.3 0 0 0 8 0 4 1.3 0 0 0-8 0v5a4 1.3 0 0 0 8 0v-5"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.a3" d="M6.6 -39h4.6m-1.6-1.4 1.6 1.4-1.6 1.4"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.cl" d="M12.4 -36h7.2a2.2 2.2 0 0 0-.6-4.3 3 3 0 0 0-5.6-.8 2.4 2.4 0 0 0-1 5.1Z"/>
        <path class="wv-chalk" pathLength="1" data-c="${id}.fl" d="M16 -35.4c0 6-4 8.6-9.6 8.6"/>
        <path class="wv-chalk wv-chalk-hot" pathLength="1" data-c="${id}.ci" d="M-5.8 -39a7.3 4.6 0 1 0 14.6 0 7.3 4.6 0 1 0-14.6 0"/>
        ${rolling(id, 26, 2.3, 15, 18)}` },
    // Server rack: running LEDs; one blade slides out and shows a fault.
    rack: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0], blade: ['tx', 0], bad: ['op', 0] }, wheels: wheels(2.3, 'wl', 'wr'),
      svg: id => {
        const unit = (y, leds, delay) => `<rect class="wv-unit" x="-8.4" y="${y}" width="16.8" height="5.6" rx=".6"/><path class="wv-bevel" d="M-7.8 ${n3(y + 0.45)}h15.6"/>${vents(-7, y + 1.6, 3, 6.4, 1.05)}${[0, 1, 2, 3].map(i =>
          `<rect class="${leds}" x="${2 + i * 1.6}" y="${y + 2.2}" width=".9" height="1.2" style="animation-delay:-${n3(((delay + i) * 0.17) % 1.2)}s"/>`).join('')}`;
        return `<path class="wv-strut" d="M-10 -24h-2.8v-6h2.8"/>${plate(-10, -48, 20, 43.4, 1.6, ' wv-metal-dark')}${neon('M-7.6 -46.8h15.2')}
          ${[-44, -37, -23, -16, -9].map((y, i) => unit(y, 'wv-led-run', i * 2)).join('')}
          <g data-c="${id}.blade">${unit(-30, 'wv-led-run', 5)}<rect class="wv-handle" x="-10.4" y="-28.4" width="1.6" height="2.4" rx=".5"/>
            <g data-c="${id}.bad">${[0, 1, 2, 3].map(i => `<rect class="wv-led-bad" x="${2 + i * 1.6}" y="-27.8" width=".9" height="1.2"/>`).join('')}</g></g>
          ${rolling(id, 14, 2.3, 6.5, 11)}`;
      } },
    // A hologram projector puck; the beam and the globe open above it.
    puck: { half: 12, ch: { beam: ['op', 0], globe: ['scale', 0] },
      svg: id => `<g data-c="${id}.beam"><path class="wv-beam" d="M-4.6 -2.6-11.5-26h23L4.6-2.6Z"/></g>
        <g transform="translate(0 -27)"><g data-c="${id}.globe"><g class="wv-holo-g"><circle class="wv-holo" r="10.5"/>
          ${[0, 1, 2, 3].map(i => `<ellipse class="wv-holo-line wv-meridian" rx="10.5" ry="10.5" style="animation-delay:-${i * 0.75}s"/>`).join('')}
          <path class="wv-holo-line" d="M-10 -3.4h20M-9 3.4h18M-6.6 -7.8h13.2M-6.6 7.8h13.2"/>
          <g class="wv-orbit"><circle class="wv-holo-dot" cx="13.5" r=".9"/><circle class="wv-holo-dot" cx="-12" cy="4" r=".7"/></g></g></g></g>
        <ellipse class="wv-puck" cy="-1.6" rx="5.6" ry="1.9"/>${neon('M-3.8 -1.7h7.6')}` }
  };

  // ---- arrivals and departures: vehicles, gear, a teleport pad and a secret door
  Object.assign(ART, {
    // The scan line that materializes and dissolves sets (origin: the line's centre).
    scanbeam: { half: 24, svg: () => `<rect class="wv-scanband" x="-20" y="-8" width="40" height="8"/><path class="wv-scanline" d="M-21 0h42"/>` },
    puff: { half: 10, svg: () => `<g class="wv-puff"><circle cx="-5" cy="-1.6" r="2.2"/><circle cx="0" cy="-2.6" r="2.9"/><circle cx="5.2" cy="-1.5" r="2"/></g>` },
    // A low pod car seen from the side; the robot sits behind the windshield.
    car: { half: 24, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(3.4, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="18" ry="1.6"/><path class="wv-cone wv-headbeam" d="M17.6 -6.8 42 -11.6v10Z"/>
        <path class="wv-car" d="M-18.4 -3.4v-5.4q0-2.4 2.4-2.6l7.4-.6h17.4l6.6 2.8q3.2 1.4 3.2 4.2v1.6q0 1.4-1.4 1.4H-17q-1.4 0-1.4-1.4Z"/>
        <path class="wv-metal" d="M-18.2 -10.4l-1.8-3h4.6l1.2 2.6Z"/><path class="wv-bevel" d="M-15.4 -11.2h22.6"/><path class="wv-windshield" d="M8.6 -11.4 4.4 -19.6"/>
        <path class="wv-arch" d="M-15.4 -2a4.4 4.4 0 0 1 8.8 0M6.6 -2a4.4 4.4 0 0 1 8.8 0"/>
        ${neon('M-13.4 -5.6h25')}${led(17.8, -7, ' wv-led-head', 0.95)}${led(-18.2, -7.4, ' wv-led-hot', 0.75)}${vents(-12.6, -9.4, 2, 4, 1)}
        ${wheel(id, 'wl', -11, -3.4, 3.4)}${wheel(id, 'wr', 11, -3.4, 3.4)}` },
    // A light bike: diamond frame, saddle at (-3.6, -16.4), bars at (10.2, -19.4), crank at (0, -5.4).
    bike: { half: 17, ch: { wl: ['rot', 0], wr: ['rot', 0], crank: ['rot', 0] }, wheels: wheels(5.4, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="15" ry="1.4"/>${wheel(id, 'wl', -9, -5.4, 5.4)}${wheel(id, 'wr', 9, -5.4, 5.4)}
        <path class="wv-frame" d="M-9 -5.4 -3.4 -14.8H6.8L9 -5.4M-3.4 -14.8 0 -5.4 6.8 -14.8M0 -5.4H-9M6.8 -14.8l.9-4.6h2.8"/>${neon('M-2.6 -14.4h8.6')}
        <path class="wv-grip" d="M-6.2 -16.4h4.6"/><g transform="translate(0 -5.4)"><g data-c="${id}.crank"><path class="wv-frame" d="M0 -2.6V2.6"/><path class="wv-grip" d="M-1.1 -2.6h2.2M-1.1 2.6h2.2"/></g></g>
        ${led(10.4, -18, ' wv-led-head', 0.55)}` },
    scooter: { half: 14, ch: { wl: ['rot', 0], wr: ['rot', 0] }, wheels: wheels(2, 'wl', 'wr'),
      svg: id => `<ellipse class="wv-shadow" cy=".5" rx="11" ry="1.3"/><path class="wv-cone" d="M-8 -2h16l2 2h-20Z"/><path class="wv-frame" d="M8.6 -2.6 6.6 -24M4.4 -24h4.4"/>
        <rect class="wv-deck" x="-9" y="-3.8" width="17" height="1.9" rx=".9"/>${neon('M-7 -1.6h13')}${wheel(id, 'wl', -7.6, -2, 2)}${wheel(id, 'wr', 8.4, -2, 2)}${led(7, -21, ' wv-led-head', 0.6)}` },
    hover: { half: 13,
      svg: () => `<ellipse class="wv-shadow" cy=".5" rx="9" ry="1.2"/><path class="wv-thrust" d="M-7 -2.6h14l-2.6 3.6h-8.8Z"/>
        <ellipse class="wv-metal" cy="-3.6" rx="10.6" ry="1.8"/><path class="wv-bevel" d="M-7.6 -4.9h15.2"/>${neon('M-8.6 -3h17.2')}${led(-6.4, -3.6, ' wv-led-hot', 0.45)}${led(6.4, -3.6, '', 0.45)}` },
    // Worn on the back: origin at the robot's spine, the pack extends behind it.
    jetpack: { half: 8, ch: { flame: ['op', 0] },
      svg: id => `<rect class="wv-metal" x="-5.8" y="-5.4" width="3.1" height="9.4" rx="1.5"/><rect class="wv-metal wv-metal-lit" x="-3" y="-5.9" width="3.1" height="10.2" rx="1.5"/>
        <path class="wv-bevel" d="M-2.4 -5.1h1.9"/>${neon('M-1.45 -3.4v5', ' wv-amb')}<path class="wv-metal-dark" d="M-5.4 4h2.4l.4 1.6h-3.2ZM-2.7 4.2h2.4l.4 1.6h-3.2Z"/>
        <g data-c="${id}.flame"><path class="wv-flame" d="M-4.8 6.2q.8 5 1.6 0M-2.1 6.4q.9 6 1.8 0"/></g>` },
    // A quadcopter with a grab bar at (0, 5.2).
    drone: { half: 15,
      svg: () => `<path class="wv-strut" d="M-9 -1.4h18"/><ellipse class="wv-rotor" cx="-9" cy="-3.2" rx="5.2" ry=".7"/><ellipse class="wv-rotor wv-rotor-b" cx="9" cy="-3.2" rx="5.2" ry=".7"/>
        <path class="wv-edge" d="M-9 -1.4v-1.8M9 -1.4v-1.8"/><rect class="wv-metal" x="-4.8" y="-2.8" width="9.6" height="3.8" rx="1.7"/><path class="wv-bevel" d="M-3.2 -2.3h6.4"/>
        ${led(3.3, -0.9, ' wv-led-hot', 0.55)}${neon('M-2.8 .3h5.6')}<path class="wv-cable" d="M0 1v4"/><path class="wv-grip" d="M-2.8 5.2h5.6"/>` },
    tele: { half: 12, ch: { col: ['op', 0] },
      svg: id => `<g data-c="${id}.col"><path class="wv-telebeam" d="M-8 0V-66h16V0Z"/><path class="wv-teleline" d="M-8 0V-66M8 0V-66"/>
        ${[0, 1, 2].map(i => `<ellipse class="wv-telering" cy="-6" rx="9" ry="1.5" style="animation-delay:-${n3(i * 0.42)}s"/>`).join('')}</g>
        <ellipse class="wv-metal" cy="-.7" rx="10.4" ry="1.7"/>${neon('M-7.4 -.7h14.8')}` },
    // A secret door in the back wall: a corridor recedes behind it (back layer)...
    door: { half: 15, ch: { trim: ['dash', 1] },
      svg: id => `<rect class="wv-door-in" x="-9" y="-34" width="18" height="34"/><path class="wv-door-floor" d="M-9 0-3.2-11h6.4L9 0Z"/><path class="wv-door-wall" d="M-9 0-3.2-11V-24.6L-9 -34ZM9 0 3.2-11V-24.6L9 -34Z"/>
        <rect class="wv-door-light" x="-3.2" y="-24.6" width="6.4" height="13.6"/><path class="wv-jamb" d="M-10.2 0V-35.2h20.4V0"/>
        <path class="wv-door-trim" pathLength="1" data-c="${id}.trim" d="M-11.4 0V-36.4h22.8V0"/>${led(0, -38.6, ' wv-led-hot', 0.6)}` },
    // ...and a blast shutter rolls up in front of it (front layer).
    shutter: { half: 15, ch: { roll: ['sy', 1] },
      svg: id => `<g transform="translate(0 -34)"><g data-c="${id}.roll"><rect class="wv-panel" x="-9" y="0" width="18" height="34"/>
        <path class="wv-panel-seam" d="M-9 8.5h18M-9 17h18M-9 25.5h18"/>${hazard(-9, 30.6, 18, 3.4)}</g></g>` }
  });

  // ---- breaks: things a robot does when it pauses mid-task
  Object.assign(ART, {
    upbar: { half: 12, ch: { fill: ['sx', 0] },
      svg: id => `<g class="wv-unmirror"><rect class="wv-bezel" x="-10" y="-3.2" width="20" height="5.4" rx="1.3"/><g transform="translate(-8.6 -1.8)"><g data-c="${id}.fill"><rect class="wv-upfill" width="17.2" height="2.6" rx=".6"/></g></g>
        ${led(-8.8, -5.6, ' wv-led-amber', 0.55)}<path class="wv-screen-text" d="M-7 -5.6h7.4"/></g>` },
    batt: { half: 8, ch: { low: ['op', 1], full: ['op', 0] },
      svg: id => `<g class="wv-unmirror"><rect class="wv-bezel" x="-5.4" y="-3.2" width="9.8" height="5.6" rx="1.1"/><path class="wv-edge" d="M5 -1.4v2"/>
        <g data-c="${id}.low"><rect class="wv-batt-low" x="-4.4" y="-2.2" width="2" height="3.6" rx=".4"/></g>
        <g data-c="${id}.full"><rect class="wv-batt-full" x="-4.4" y="-2.2" width="7.8" height="3.6" rx=".4"/><path class="wv-bolt" d="M.4 -2.2-1.3.2h1.9l-1.2 2"/></g></g>` },
    cell: { half: 3, svg: () => `<rect class="wv-metal" x="-1.6" y="-4.6" width="3.2" height="6.2" rx="1"/><path class="wv-edge" d="M-.7 -5.2h1.4"/>${neon('M-1 -2.6h2', ' wv-mx')}` },
    phone: { half: 3, ch: { flash: ['op', 0] },
      svg: id => `<rect class="wv-metal-dark" x="-1.5" y="-2.8" width="3" height="5.4" rx=".7"/><rect class="wv-phone-screen" x="-1.05" y="-2.3" width="2.1" height="4.1" rx=".35"/>
        <g data-c="${id}.flash"><circle class="wv-flash" r="5.5"/></g>` },
    orbit: { half: 14,
      svg: () => `<g class="wv-orbitbits">${['0', '1', '1', '0', '1', '0'].map((g, i) => {
        const a = i / 6 * Math.PI * 2;
        return `<text x="${n3(Math.cos(a) * 11)}" y="${n3(Math.sin(a) * 3.4)}">${g}</text>`;
      }).join('')}</g>` },
    cookie: { half: 3, ch: { bite: ['op', 0] },
      svg: id => `<circle class="wv-cookie" r="2.6"/><circle class="wv-chipdot" cx="-.9" cy="-.6" r=".45"/><circle class="wv-chipdot" cx=".9" cy=".5" r=".45"/><circle class="wv-chipdot" cx="-.2" cy="1.2" r=".35"/>
        <g data-c="${id}.bite"><circle class="wv-bite" cx="2.4" cy="-1.4" r="1.4"/></g>` },
    spray: { half: 10,
      svg: () => `<g class="wv-spraybits">${['1', '0', '1', '0', '1'].map((g, i) => `<text x="${n3(2 + i * 2.6)}" y="${n3(-1 + (i % 2 ? -1.6 : 1))}">${g}</text>`).join('')}</g>` }
  });

  // ---- extras: ambient things that cross the room on their own
  Object.assign(ART, {
    roomba: { half: 9,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="7" ry="1.1"/><rect class="wv-metal" x="-6.6" y="-3.6" width="13.2" height="3.4" rx="1.7"/><path class="wv-bevel" d="M-4.8 -3.1h9.6"/>
        ${neon('M-5.2 -1.5h10.4')}${led(4.6, -2.6, ' wv-led-hot', 0.45)}<path class="wv-brush" d="M6.6 -.5l2.2.5M6.4 -.3l1.6 1"/>` },
    bug: { half: 6,
      svg: () => `<g class="wv-bug"><path class="wv-bug-legs" d="M-2 -1.2-3.4.4M0 -1.2v1.8M2 -1.2 3.4.4"/><ellipse class="wv-bug-body" cy="-2.1" rx="3" ry="1.9"/>
        <path class="wv-bug-shell" d="M-2.6 -2.5h5M0 -3.9v3.6"/><circle class="wv-bug-head" cx="3.3" cy="-2.3" r="1.1"/><path class="wv-bug-ant" d="M3.9 -3.1 5.2-4.6M3.4 -3.3l.4-1.7"/>
        <circle class="wv-bug-eye" cx="3.8" cy="-2.5" r=".35"/></g>` },
    bits: { half: 8,
      svg: () => `<g class="wv-risebits">${['0', '1', '1', '0', '1'].map((g, i) => `<text x="${n3((i - 2) * 2.8)}" y="${n3(-(i % 3) * 3)}" style="animation-delay:-${n3(i * 0.36)}s">${g}</text>`).join('')}</g>` },
    mouse: { half: 10,
      svg: () => `<path class="wv-mouse-cable" d="M-3.8 -1.2q-3.2-1.8-5.4.2t-4.4.3"/><path class="wv-mouse" d="M-3.9 -.2q0-3.8 3.9-3.8t3.9 3.8Z"/><path class="wv-edge" d="M0 -4v1.7"/>${led(0, -3.1, '', 0.45)}<ellipse class="wv-shadow" cy=".3" rx="4" ry=".7"/>` },
    sat: { half: 14,
      svg: () => `<rect class="wv-solar" x="-13" y="-2" width="8" height="4" rx=".4"/><rect class="wv-solar" x="5" y="-2" width="8" height="4" rx=".4"/><path class="wv-edge" d="M-5 0h10"/>
        ${plate(-2.6, -3, 5.2, 6, 1)}<path class="wv-strut" d="M0 -3v-2.4"/>${led(0, -5.8, ' wv-led-hot', 0.5)}<path class="wv-solar-lines" d="M-11 -2v4M-9 -2v4M-7 -2v4M7 -2v4M9 -2v4M11 -2v4"/>` },
    floppy: { half: 6,
      svg: () => `<rect class="wv-floppy" x="-4" y="-4" width="8" height="8" rx=".6"/><rect class="wv-floppy-shutter" x="-2" y="-4" width="4" height="2.8"/><rect class="wv-floppy-label" x="-2.8" y=".4" width="5.6" height="3"/>` },
    cursor: { half: 8, ch: { ring: ['op', 0] },
      svg: id => `<g data-c="${id}.ring"><circle class="wv-click" r="4"/></g><path class="wv-cursor" d="M0 0v9.6l2.5-2.4 1.9 4.2 1.6-.7-1.9-4.1h3.5Z"/>` },
    catbot: { half: 10,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="7" ry="1"/><g class="wv-cat"><path class="wv-cat-legs" d="M-4 -3.4V0M-2 -3.4V0M3 -3.4V0M5 -3.4V0"/>
        <path class="wv-cat-tail" d="M-5.6 -5q-3.4-1-3-5.6"/><rect class="wv-metal" x="-6" y="-7" width="12" height="4.2" rx="2"/><path class="wv-bevel" d="M-4 -6.5h8"/>
        <path class="wv-metal" d="M4.6 -6.6l.4-3 1.4 1.4h2.4l1.4-1.4.4 3q.4 3.4-3 3.4t-3-3.4Z"/>${led(6.9, -6.2, ' wv-led-ok', 0.4)}${led(8.6, -6.2, ' wv-led-ok', 0.4)}</g>` },
    cableball: { half: 6,
      svg: () => `<g class="wv-cableball"><circle class="wv-tangle" r="4.2"/><path class="wv-tangle" d="M-3.6 -1.6q3.4 4.6 7-1M-2.4 3q.6-5.6 4.6-6.2M-4 1.2q4-.4 7.2 1.8M-1 -3.8q3.8 2 2.2 7.6"/>
        <path class="wv-neon wv-cy" d="M3.4 2.6l2 1.6"/></g>` },
    packets: { half: 20,
      svg: () => `<path class="wv-edge" d="M-17 -2.2h34"/>${[-12, 0, 12].map((x, i) => `${plate(x - 4, -7.4, 8, 5.2, 1)}<text class="wv-packet-text" x="${x}" y="-3.6">${['01', '10', '11'][i]}</text>`).join('')}` },
    comet: { half: 16, svg: () => `<path class="wv-comet-tail" d="M-16 -1.2 0 0-16 1.2"/><circle class="wv-comet" r="1.1"/>` },
    cloud: { half: 16,
      svg: () => `<path class="wv-cloud" d="M-11 0h22a4.6 4.6 0 0 0-1.2-9 6.4 6.4 0 0 0-12-2.4A5 5 0 0 0-11 0Z"/><g class="wv-cloudbits">${[-6, -1, 4, 8].map((x, i) => `<text x="${x}" y="4" style="animation-delay:-${n3(i * 0.3)}s">${i % 2}</text>`).join('')}</g>` },
    forkbot: { half: 14,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="10" ry="1.2"/>${plate(-8, -9, 11, 7, 1.4)}${neon('M-6.6 -5h8')}<path class="wv-strut" d="M4 -16V-1.6M4 -2.4h7"/>
        ${plate(5, -13, 10, 9, 1)}${hazard(5, -6.4, 10, 1.8)}<circle class="wv-wheel" cx="-5" cy="-1.6" r="1.6"/><circle class="wv-wheel" cx="1" cy="-1.6" r="1.6"/>${led(-6.6, -10.4, ' wv-led-amber', 0.5)}` },
    spinner: { half: 6, svg: () => `<circle class="wv-spinner-track" cy="-4.4" r="4.2"/><circle class="wv-spinner" cy="-4.4" r="4.2" pathLength="1"/>` }
  });

  // ---- activity sets
  Object.assign(ART, {
    stool: { half: 8, svg: () => `<ellipse class="wv-shadow" cy=".4" rx="6" ry="1"/><path class="wv-strut" d="M-4 0l1.6-8M4 0l-1.6-8"/>${plate(-5.2, -10, 10.4, 2.2, 1)}${neon('M-3.4 -6.6h6.8', ' wv-dim')}` },
    duck: { half: 7, svg: () => `<path class="wv-duck" d="M-4.6 -2.6q0-3 3.4-3.2h3.4q1.2-.1 1.6-1.4.2-3.4 3-3.4 2.8.2 2.8 3-.2 1.6-1.4 2.4 1.4 2.2-.4 4.2-1.8 1.8-5.2 1.8H-1.4q-3.2 0-3.2-3.4Z"/>
      <path class="wv-beak" d="M8.2 -8.4l2.8.6-2.6 1.1Z"/><circle class="wv-duck-eye" cx="6.4" cy="-8.4" r=".6"/><path class="wv-duck-wing" d="M-1.6 -3.6q2.4 1.6 4.4-.2"/>` }
  });

  Object.assign(ART, {
    tower: { half: 9, ch: { ok: ['op', 0], bad: ['op', 1] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="8" ry="1.1"/>${plate(-7, -30, 14, 30, 1.6)}<rect class="wv-drawer" x="-5.4" y="-27.6" width="10.8" height="6" rx=".6"/>
        ${vents(-4.6, -18.6, 6, 9.2, 1.4)}${neon('M5.2 -27v22', ' wv-cy')}<rect class="wv-port" x="-8" y="-17" width="1.6" height="2.4" rx=".3"/>
        <g data-c="${id}.bad">${led(-3.2, -24.6, ' wv-led-hot', 0.7)}</g><g data-c="${id}.ok">${led(-3.2, -24.6, ' wv-led-ok', 0.8)}</g><circle class="wv-button" cx="2.4" cy="-24.6" r=".9"/>` },
    usbstick: { half: 5, ch: { flip: ['sy', 1] },
      svg: id => `<g data-c="${id}.flip"><rect class="wv-metal" x="0" y="-1.3" width="5.4" height="2.6" rx=".7"/><rect class="wv-usb-top" x=".6" y="-1.15" width="4.2" height=".9" rx=".3"/>
        <rect class="wv-usb-plug" x="5.4" y="-.9" width="2.6" height="1.8"/><path class="wv-usb-pins" d="M6 -.45h1.6"/></g>` },
    mobo: { half: 16, ch: { live: ['op', 0] },
      svg: id => {
        const traces = 'M-12 -38h6v6h4M-12 -22h8l3-3h5M2 -18v-4h10M-7 -26v-6M13 -38v5h-3';
        return `<ellipse class="wv-shadow" cy=".4" rx="13" ry="1.2"/><path class="wv-strut" d="M-9 0l2-14M9 0l-2-14"/>
          <rect class="wv-pcb" x="-15" y="-42" width="30" height="28" rx="1.2"/><path class="wv-trace" d="${traces}"/>
          ${plate(-11, -36, 10, 10, 0.8)}${vents(-9.6, -33.6, 6, 7.2, 1.2)}
          <rect class="wv-slot2" x="2" y="-34" width="11" height="1.8" rx=".3"/><rect class="wv-slot2" x="2" y="-29.6" width="11" height="1.8" rx=".3"/>
          <g data-c="${id}.live"><path class="wv-trace-live" d="${traces}"/>${led(12.4, -40, ' wv-led-ok', 0.6)}${led(-12.6, -16.4, ' wv-led-ok', 0.5)}</g>`;
      } },
    ramstick: { half: 7,
      svg: () => `<rect class="wv-ram" x="-5.6" y="-1.8" width="11.2" height="3.2" rx=".4"/>${[-4.4, -2, 0.4, 2.8].map(x => `<rect class="wv-ramchip" x="${x}" y="-1.3" width="1.8" height="1.8" rx=".2"/>`).join('')}<path class="wv-gold" d="M-5 1.1h10"/>` },
    patch: { half: 15, ch: { l0: ['op', 0], l1: ['op', 0], l2: ['op', 0], spark: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="9" ry="1.1"/><path class="wv-strut" d="M0 0V-20M-6 0h12"/>${plate(-13, -38, 26, 18, 1.4)}
        ${[-8, 0, 8].map((x, i) => `<rect class="wv-port" x="${x - 1.6}" y="-33.2" width="3.2" height="3.4" rx=".4"/><rect class="wv-port" x="${x - 1.6}" y="-26.6" width="3.2" height="3.4" rx=".4"/>
          ${led(x, -35.8, ' wv-led-off', 0.5)}<g data-c="${id}.l${i}">${led(x, -35.8, ' wv-led-ok', 0.55)}</g>`).join('')}
        ${neon('M-11 -21.6h22', ' wv-dim')}<g data-c="${id}.spark"><path class="wv-spark" d="M10 -31l3-2-1 2.6 3-1.4M6 -28l-2 3M9 -27l2.4 1.6"/></g>` },
    plug: { half: 4,
      svg: (id, p) => `<path class="wv-cord wv-tone-${p.tone || 1}" d="M0 1.6q.8 5-1.4 8.6t.6 6"/><rect class="wv-plug wv-tone-${p.tone || 1}" x="-1.3" y="-1.8" width="2.6" height="3.4" rx=".5"/>` },
    hotchip: { half: 12, ch: { heat: ['op', 1], cool: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="9" ry="1.1"/>${plate(-6, -12, 12, 12, 1)}${vents(-4.6, -9, 4, 9.2, 1.6)}
        <g data-c="${id}.heat"><rect class="wv-heatglow" x="-10" y="-26" width="20" height="16" rx="3"/><g class="wv-heatwave"><path d="M-4 -28q1-2 0-4t0-4"/><path d="M0 -28q1-2 0-4t0-4"/><path d="M4 -28q1-2 0-4t0-4"/></g></g>
        <g data-c="${id}.cool"><rect class="wv-coolglow" x="-10" y="-26" width="20" height="16" rx="3"/></g>
        <rect class="wv-chipbody" x="-8" y="-24" width="16" height="12" rx="1.2"/>${[-6, -3, 0, 3, 6].map(x => `<path class="wv-pin" d="M${x} -24v-1.6M${x} -12v1.6"/>`).join('')}
        <rect class="wv-die" x="-3.6" y="-20.6" width="7.2" height="5.2" rx=".6"/>` },
    deskfan: { half: 10, ch: { spin: ['rot', 0], air: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="6" ry="1"/>${plate(-5, -2.4, 10, 2.4, 1)}<path class="wv-strut" d="M0 -2.4V-14"/>
        <g data-c="${id}.air"><g class="wv-wind"><path d="M9 -22h9"/><path d="M9 -18h7"/><path d="M10 -26h6"/></g></g>
        <circle class="wv-fan-cage" cy="-20" r="7"/><g transform="translate(0 -20)"><g data-c="${id}.spin">${[0, 120, 240].map(a => `<path class="wv-blade" transform="rotate(${a})" d="M0 0c1.6-1.6 4.6-1.4 5.8.6-1.4 1.8-4.4 1.6-5.8-.6Z"/>`).join('')}</g></g>
        <circle class="wv-hub2" cy="-20" r="1.3"/><path class="wv-fan-grill" d="M-7 -20h14M0 -27v14"/>` },
    crt: { half: 15, ch: { noise: ['op', 1], pic: ['op', 0] },
      svg: id => {
        let dots = '';
        for (let y = 0; y < 6; y++) for (let x = 0; x < 6; x++) dots += `<rect x="${n3(-9 + x * 2.6)}" y="${n3(-33 + y * 2.6)}" width="1.6" height="1.2" style="animation-delay:-${n3(((x * 7 + y * 13) % 10) * 0.031)}s"/>`;
        return `<ellipse class="wv-shadow" cy=".4" rx="12" ry="1.2"/><path class="wv-strut" d="M-9 0v-10M9 0v-10"/>${plate(-12, -12, 24, 2.2, 0.8)}
          <path class="wv-metal-dark" d="M-8 -14l2-12h12l2 12Z"/>${plate(-12.4, -36.4, 24.8, 22.6, 3)}<rect class="wv-crt-glass" x="-10" y="-34" width="17" height="17.4" rx="3"/>
          <g data-c="${id}.noise"><g class="wv-static">${dots}</g><rect class="wv-rollbar" x="-10" y="-30" width="17" height="2"/></g>
          <g data-c="${id}.pic"><rect class="wv-crt-pic" x="-10" y="-34" width="17" height="17.4" rx="3"/><path class="wv-crt-logo" d="M-5.4 -21v-4M-2.8 -21v-7M-.2 -21v-5.4M2.4 -21v-8.4"/></g>
          ${scanlines(-10, -34, 17, 17.4, 0.8)}${sheen(-10, -34, 17, 17.4)}<circle class="wv-knob" cx="9.6" cy="-30" r="1.1"/><circle class="wv-knob" cx="9.6" cy="-26" r="1.1"/>${led(9.6, -20.6, ' wv-led-ok', 0.5)}`;
      } },
    screen2: { half: 15, ch: { err: ['op', 1], boot: ['op', 0], ok: ['op', 0], bar: ['sx', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="9" ry="1.1"/><path class="wv-strut" d="M0 0V-14M-6 0h12"/>
        ${monitor(-13, -34, 26, 18)}
        <g data-c="${id}.err"><rect class="wv-bsod" x="-13" y="-34" width="26" height="18" rx=".8"/><text class="wv-bsod-face" x="-9.6" y="-25.4">:(</text><path class="wv-bsod-lines" d="M-9.6 -22h14M-9.6 -20.2h10M-9.6 -18.4h12"/></g>
        <g data-c="${id}.boot"><circle class="wv-boot-ring" cy="-27" r="3.4"/><g transform="translate(-6 -20.6)"><g data-c="${id}.bar"><rect class="wv-upfill" width="12" height="1.3" rx=".5"/></g></g></g>
        <g data-c="${id}.ok"><path class="wv-ok-check" d="M-4 -25.6l2.6 2.6 5.4-5.4"/><path class="wv-screen-text" d="M-10 -31.4h6M6 -31.4h4M-10 -18.6h20"/></g>` },
    powerbtn: { half: 7, ch: { press: ['ty', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="5" ry="1"/>${plate(-3.4, -14, 6.8, 14, 1.2)}${neon('M-1.8 -4h3.6', ' wv-dim')}
        <g data-c="${id}.press"><rect class="wv-metal-dark" x="-4" y="-16.6" width="8" height="2.8" rx="1.2"/><path class="wv-power" d="M-1.4 -15.9a1.9 1.9 0 1 0 2.8 0M0 -16.6v1.6"/></g>` },
    circuit: { half: 14, ch: { crack: ['op', 1], fixed: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="11" ry="1.1"/><path class="wv-strut" d="M-7 0l2-12M7 0l-2-12"/>
        <rect class="wv-pcb" x="-13" y="-34" width="26" height="22" rx="1.2"/><path class="wv-trace" d="M-11 -30h5l3 3h6M-11 -16h7l2-2h8M8 -31v8l3 3"/>
        <rect class="wv-chipbody" x="-6" y="-29" width="12" height="12" rx="1"/>${[-4, -1.3, 1.3, 4].map(x => `<path class="wv-pin" d="M${x} -29v-1.6M${x} -17v1.6"/>`).join('')}
        <g data-c="${id}.crack"><path class="wv-crack" d="M-4 -28-1-25-3-22.4 1-20.4-.6-18"/>${led(10, -31.4, ' wv-led-hot', 0.6)}</g>
        <g data-c="${id}.fixed"><path class="wv-weld" d="M-4 -28-1-25-3-22.4 1-20.4-.6-18"/>${led(10, -31.4, ' wv-led-ok', 0.6)}</g>` },
    laser: { half: 22, svg: () => `<path class="wv-laser" d="M0 0h20"/><path class="wv-laser-core" d="M0 0h20"/>` },
    spark: { half: 4, svg: () => `<g class="wv-sparks"><path d="M0 0-2.6-2.2M0 0l2.8-1.6M0 0-.6 3M0 0l2.2 2.2M0 0l-3 .8"/></g><circle class="wv-spark-core" r=".9"/>` },
    fdrive: { half: 12, ch: { busy: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="11" ry="1.1"/><path class="wv-strut" d="M-9 0v-8M9 0v-8"/>${plate(-11, -10, 22, 2, 0.8)}
        ${plate(-9, -21, 18, 11, 1.4)}<rect class="wv-slot3" x="-6" y="-17.4" width="12" height="1.4" rx=".4"/>${vents(-6, -13.6, 2, 7, 1.1)}
        ${led(6.4, -12.8, ' wv-led-off', 0.5)}<g data-c="${id}.busy">${led(6.4, -12.8, ' wv-led-busy', 0.6)}</g>` },
    p3d: { half: 15, ch: { head: ['tx', 0], grow: ['sy', 0], hot: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="13" ry="1.2"/>${plate(-13, -6, 26, 6, 1.2)}<path class="wv-strut" d="M-11 -6V-34M11 -6V-34"/>${plate(-12.4, -36, 24.8, 2.6, 1)}
        ${plate(-8, -8, 16, 2, 0.6, ' wv-metal-dark')}
        <g transform="translate(0 -8)"><g data-c="${id}.grow"><path class="wv-print" d="M-2.6 0v-5.4h5.2V0ZM-2 -7.4a2 2 0 1 0 4 0 2 2 0 1 0-4 0"/></g></g>
        <g data-c="${id}.head"><path class="wv-edge" d="M0 -33.4v3.4"/><rect class="wv-metal" x="-2.2" y="-30" width="4.4" height="3" rx=".8"/><path class="wv-nozzle" d="M-.6 -27h1.2l-.6 1.6Z"/>
          <g data-c="${id}.hot"><path class="wv-printbeam" d="M0 -25.4V-16.6"/>${led(1.4, -28.6, ' wv-led-hot', 0.45)}</g></g>
        ${neon('M-11 -2.6h22', ' wv-vi')}` },
    mini: { half: 4, svg: () => `<path class="wv-print" d="M-2.6 0v-5.4h5.2V0ZM-2 -7.4a2 2 0 1 0 4 0 2 2 0 1 0-4 0"/><path class="wv-mini-eyes" d="M-.9 -7.4h.4M.5 -7.4h.4"/>` }
  });

  const gearPath = (r, teeth) => {
    const points = [];
    for (let i = 0; i < teeth; i++) {
      const a = i / teeth * Math.PI * 2, s = Math.PI / teeth;
      for (const [da, rr] of [[-s * 0.45, r], [s * 0.45, r], [s * 0.6, r * 0.72], [s * 1.4, r * 0.72]]) points.push(`${n3(Math.cos(a + da) * rr)} ${n3(Math.sin(a + da) * rr)}`);
    }
    return `M${points.join('L')}Z`;
  };
  Object.assign(ART, {
    paperstack: { half: 7,
      svg: () => `${[0, 1, 2, 3, 4, 5].map(i => `<rect class="wv-paper" x="${n3(-5 + (i % 2 ? 0.5 : -0.3))}" y="${n3(-1.4 - i * 1.35)}" width="10" height="1.35" rx=".3"/>`).join('')}<path class="wv-paper-lines" d="M-3 -4.2h6M-3 -6.9h4"/>` },
    filecab: { half: 14, ch: { dr: ['tx', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="11" ry="1.2"/>
        <g data-c="${id}.dr"><rect class="wv-drawer-in" x="-9.6" y="-34.6" width="17" height="8.6" rx=".5"/><path class="wv-tabs" d="M-7 -34.6v-2M-4.4 -34.6v-2.6M-1.8 -34.6v-1.8M.8 -34.6v-2.4M3.4 -34.6v-2"/></g>
        ${plate(-10, -36, 20, 36, 1.6)}<path class="wv-edge" d="M-10 -24h20M-10 -12h20"/>${neon('M6 -33v-2M6 -21v-2M6 -9v-2', ' wv-amb')}<rect class="wv-label" x="-4" y="-33" width="8" height="2" rx=".4"/>
        <g data-c="${id}.dr"><rect class="wv-drawer" x="-11" y="-35" width="2.2" height="9.4" rx=".6"/><rect class="wv-handle" x="-12.2" y="-31.4" width="1.4" height="2.2" rx=".4"/></g>
        ${[-23.4, -11.4].map(y => `<rect class="wv-drawer" x="-11" y="${y}" width="2.2" height="9.4" rx=".6"/><rect class="wv-handle" x="-12.2" y="${n3(y + 3.6)}" width="1.4" height="2.2" rx=".4"/>`).join('')}` },
    nfolder: { half: 9, ch: { cover: ['sy', 1] },
      svg: id => `<path class="wv-folder" d="M-7 0V-11.4q0-1 1-1h4l1.6 1.8h6.4q1 0 1 1V0Z"/><rect class="wv-folder-in" x="-6" y="-9.8" width="12" height="9" rx=".5"/>
        <g data-c="${id}.cover"><path class="wv-folder-front" d="M-7 0l1-8.4h12L7 0Z"/>${neon('M-3.6 -3.8h7.2', ' wv-amb')}</g>` },
    doc: { half: 4,
      svg: () => `<path class="wv-doc" d="M-2.6 0V-7h3.6l1.6 1.6V0Z"/><path class="wv-doc-lines" d="M-1.6 -4.6h2.8M-1.6 -3.2h2.8M-1.6 -1.8h2"/><path class="wv-edge" d="M1 -7v1.6h1.6"/>` },
    table: { half: 16,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="14" ry="1.2"/><path class="wv-strut" d="M-12 0V-11M12 0V-11"/>${plate(-15, -13, 30, 2.4, 1)}${neon('M-13 -10.2h26', ' wv-dim')}` },
    shredder: { half: 10, ch: { cut: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="9" ry="1.1"/><path class="wv-bin" d="M-8 0l-1-16h18l-1 16Z"/>
        <path class="wv-confetti" d="M-6 -1.4h3M-1 -2.6h2M3 -1.2h3M-4.4 -4h2M1 -4.4h3M-6.4 -5.6h2"/>
        <g data-c="${id}.cut"><g class="wv-strips">${[-5, -2.5, 0, 2.5, 5].map((x, i) => `<path d="M${x} -16v4" style="animation-delay:-${n3(i * 0.13)}s"/>`).join('')}</g></g>
        ${plate(-10, -22, 20, 6, 1.4)}<rect class="wv-slot3" x="-6.6" y="-22.4" width="13.2" height="1.2" rx=".4"/>${led(7, -19, ' wv-led-ok', 0.5)}` },
    compiler: { half: 20, ch: { gear: ['rot', 0], err: ['op', 0], run: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="16" ry="1.3"/>${plate(-13, -26, 22, 26, 1.8)}<path class="wv-metal" d="M-11 -26l-2.4-6.4h13l-2.4 6.4Z"/><path class="wv-bevel" d="M-12.6 -31.8h11.4"/>
        <circle class="wv-gearwin" cx="-2" cy="-14" r="6.4"/><g transform="translate(-2 -14)"><g data-c="${id}.gear"><path class="wv-gear" d="${gearPath(5.2, 8)}"/><circle class="wv-glass" r="1.4"/></g></g>
        <path class="wv-metal" d="M9 -9.6h9l1.4 2.4H9Z"/>${led(6, -23, ' wv-led-off', 0.6)}<g data-c="${id}.run">${led(6, -23, ' wv-led-ok', 0.65)}</g>
        <g data-c="${id}.err">${led(6, -23, ' wv-led-hot', 0.75)}<g class="wv-smoke"><circle cx="-8" cy="-34" r="2"/><circle cx="-5" cy="-37" r="2.6"/><circle cx="-9" cy="-40" r="2.2"/></g></g>
        ${vents(-11, -5, 2, 6)}${neon('M2 -5h5', ' wv-dim')}` },
    parcel: { half: 5,
      svg: () => `${plate(-4.4, -8, 8.8, 8, 1)}<path class="wv-neon wv-mx" d="M0 -8V0M-4.4 -4h8.8"/><path class="wv-ok-check" d="M-2.6 -4.6l1.2 1.2 2.4-2.4" transform="translate(1.6 0)"/>` },
    safe: { half: 11, ch: { door: ['sx', 1], dial: ['rot', 0], lock: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="10" ry="1.2"/>${plate(-10, -20, 20, 20, 1.6)}<rect class="wv-safe-in" x="-8" y="-18" width="16" height="16" rx=".6"/><path class="wv-edge" d="M-8 -10h16"/>
        <g transform="translate(-8 0)"><g data-c="${id}.door"><rect class="wv-safe-door" x="0" y="-18" width="16" height="16" rx=".8"/><path class="wv-bevel" d="M1 -17.4h14"/>
          <g transform="translate(8 -10)"><g data-c="${id}.dial"><circle class="wv-dial" r="3.6"/><path class="wv-dial-mark" d="M0 -3.6v1.6M3.6 0h-1M-3.6 0h1M0 3.6v-1"/></g></g>
          <rect class="wv-handle" x="13" y="-12" width="1.4" height="4" rx=".5"/></g></g>
        ${led(7.6, -21.8, ' wv-led-off', 0.5)}<g data-c="${id}.lock">${led(7.6, -21.8, ' wv-led-ok', 0.6)}</g>` }
  });

  Object.assign(ART, {
    // Sorting bars: height p.h, tone 1..4 from cool to warm; ok glows when sorted.
    sbar: { half: 4, ch: { ok: ['op', 0] },
      svg: (id, p) => `<rect class="wv-sbar wv-sbar-${p.tone}" x="-2.6" y="${-p.h}" width="5.2" height="${p.h}" rx=".8"/><path class="wv-bevel" d="M-1.8 ${n3(-p.h + 0.6)}h3.6"/>
        <g data-c="${id}.ok"><rect class="wv-sbar-ok" x="-2.6" y="${-p.h}" width="5.2" height="${p.h}" rx=".8"/></g>` },
    shelf: { half: 26, svg: () => `<ellipse class="wv-shadow" cy=".4" rx="24" ry="1.2"/>${plate(-24, -2.4, 48, 2.4, 1)}${neon('M-22 -.6h44', ' wv-dim')}` },
    trashbin: { half: 8, ch: { lid: ['rot', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="7" ry="1"/><path class="wv-bin" d="M-6 0l-.8-12h13.6L6 0Z"/><path class="wv-recycle" d="M-2 -5.4l2-3.2 2 3.2ZM0 -3.6a2.6 2.6 0 1 0 .1 0"/>
        <g transform="translate(-7 -12)"><g data-c="${id}.lid">${plate(0, -1.6, 14, 1.8, 0.7)}</g></g>` },
    token: { half: 3,
      svg: (id, p) => `<rect class="wv-token" x="-2.4" y="-5" width="4.8" height="5" rx="1"/><text class="wv-token-text" y="-1.3">${p.g}</text>` },
    brick: { half: 5,
      svg: () => `<rect class="wv-brick" x="-4" y="-4" width="8" height="4" rx=".5"/><path class="wv-brick-line" d="M-4 -2h8M0 -4v2M-2 -2v2M2 -2v2"/>` },
    flame: { half: 10,
      svg: () => `<g class="wv-flames"><path d="M-6 0q-1-4 1-6 0 2 1.4 2.4Q-3 -8 0 -10q-.4 3 1.6 4.2.6-1.6 1.6-2.4.4 3 2.6 4.6.6 2.4-.2 3.6Z"/></g>` },
    virus: { half: 4,
      svg: () => `<g class="wv-virus"><path class="wv-virus-spikes" d="M0 -4.6v-1.4M0 4.6V6M4.6 0H6M-4.6 0H-6M3.3 -3.3l1 -1M-3.3 3.3l-1 1M3.3 3.3l1 1M-3.3 -3.3l-1 -1"/><circle class="wv-virus-body" r="3.6"/>
        <path class="wv-virus-eyes" d="M-2 -1.2l1.4.8M2 -1.2l-1.4.8"/><path class="wv-virus-mouth" d="M-1.4 1.6q1.4-1 2.8 0"/></g>` },
    box2: { half: 5,
      svg: () => `${plate(-4.4, -7, 8.8, 7, 1)}<text class="wv-box2-text" y="-2.2">{ }</text>` },
    paddle: { half: 5, svg: () => `<path class="wv-grip" d="M0 0l-2.4 2.4"/><circle class="wv-paddle" cx="2.2" cy="-2.2" r="3"/>` },
    pball: { half: 2, svg: () => `<circle class="wv-pball" r="1.2"/>` },
    pwall: { half: 8,
      svg: () => `<ellipse class="wv-shadow" cy=".4" rx="6" ry="1"/><path class="wv-strut" d="M-3 0v-4M3 0v-4"/>${plate(-3.4, -36, 6.8, 32, 1.4)}<circle class="wv-target" cx="-3.4" cy="-22" r="3"/><circle class="wv-target" cx="-3.4" cy="-22" r="1.2"/>` },
    coin: { half: 3, ch: { f0: ['op', 1], f1: ['op', 0] },
      svg: id => `<circle class="wv-coin" r="2.6"/><g data-c="${id}.f0"><text class="wv-coin-text" y="1.1">0</text></g><g data-c="${id}.f1"><text class="wv-coin-text" y="1.1">1</text></g>` },
    kbd: { half: 30, ch: { k0: ['op', 0], k1: ['op', 0], k2: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="28" ry="1.3"/>${plate(-28, -5, 56, 5, 1.4)}
        ${[-22, -14, -6, 2, 10, 18].map((x, i) => `<rect class="wv-key" x="${x}" y="-6.4" width="6.4" height="2.4" rx=".6"/>${[1, 3, 5].includes(i) ? `<g data-c="${id}.k${(i - 1) / 2}"><rect class="wv-key-lit" x="${x}" y="-6.4" width="6.4" height="2.4" rx=".6"/></g>` : ''}`).join('')}
        ${neon('M-26 -1.6h52', ' wv-rgb')}` },
    term: { half: 24, ch: { t0: ['op', 0], t1: ['op', 0], t2: ['op', 0] },
      svg: id => `<path class="wv-cable" d="M-14 -24V-150M14 -24V-150"/>${monitor(-20, -23, 40, 22, `<text class="wv-term-prompt" x="-17" y="-8">&gt;</text>
        ${['O', 'L', 'Á'].map((g, i) => `<g data-c="${id}.t${i}"><text class="wv-term-text" x="${-12 + i * 6}" y="-8">${g}</text></g>`).join('')}<path class="wv-term-cursor" d="M7 -8h4"/>`, ' wv-screen-matrix')}` }
  });

  Object.assign(ART, {
    // A barbell whose plates are hard-disk platters (origin: the bar's centre).
    barbell: { half: 17, svg: () => `<path class="wv-bar" d="M-15.4 0h30.8"/>${[-11, 11].map(x => `<circle class="wv-platter" cx="${x}" r="4.8"/><circle class="wv-platter-ring" cx="${x}" r="3.1"/><circle class="wv-spindle" cx="${x}" r=".9"/>`).join('')}` },
    net: { half: 16, svg: () => `<path class="wv-strut" d="M0 0l9.4-9.4"/><g transform="translate(12.4 -12.4) rotate(-45)"><path class="wv-mesh" d="M-3.8 0q3.8 7 7.6 0"/><ellipse class="wv-hoop" rx="3.8" ry="1.6"/></g>` },
    umb: { half: 13, ch: { open: ['sx', 0.15] },
      svg: id => `<path class="wv-strut" d="M0 0v-16"/><path class="wv-grip" d="M0 .4q0 1.8-1.8 1.8"/>
        <g transform="translate(0 -16)"><g data-c="${id}.open"><path class="wv-canopy" d="M-11 0q0-7.4 11-7.4T11 0q-1.8-1.6-3.7 0-1.8-1.6-3.7 0-1.8-1.6-3.6 0-1.8-1.6-3.6 0-1.8-1.6-3.7 0-1.8-1.6-3.7 0Z"/>${neon('M-9 -2.8q9-4 18 0', ' wv-cy')}</g></g>` },
    bitrain: { half: 14,
      svg: () => `<g class="wv-bitrain">${Array.from({ length: 12 }, (_, i) => `<text x="${n3(-11 + i * 2)}" y="0" style="animation-delay:-${n3((i * 7 % 12) * 0.075)}s">${(i * 5) % 3 ? '1' : '0'}</text>`).join('')}</g>` },
    dish: { half: 12, ch: { aim: ['rot', -30] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="8" ry="1"/><path class="wv-strut" d="M-6 0l6-14 6 14M0 -14v-3"/>
        <g transform="translate(0 -17)"><g data-c="${id}.aim"><path class="wv-dish" d="M-2.4 -9.6q9 1 10 10-9-1-10-10Z"/><path class="wv-edge" d="M2.6 -4.6l4.2-4.2"/>${led(7, -8.8, ' wv-led-hot', 0.55)}</g></g>` },
    sigpanel: { half: 7, ch: { s1: ['op', 0], s2: ['op', 0], s3: ['op', 0], s4: ['op', 0] },
      svg: id => `<ellipse class="wv-shadow" cy=".4" rx="5" ry="1"/><path class="wv-strut" d="M0 0v-10"/>${plate(-5.4, -21, 10.8, 11, 1.2)}
        ${[0, 1, 2, 3].map(i => `<rect class="wv-sig-off" x="${n3(-3.8 + i * 2.1)}" y="${n3(-12.6 - i * 1.7)}" width="1.4" height="${n3(1.4 + i * 1.7)}"/><g data-c="${id}.s${i + 1}"><rect class="wv-sig" x="${n3(-3.8 + i * 2.1)}" y="${n3(-12.6 - i * 1.7)}" width="1.4" height="${n3(1.4 + i * 1.7)}"/></g>`).join('')}` },
    fogspot: { half: 12, svg: () => `<ellipse class="wv-fogspot" rx="10" ry="7"/>` },
    heartline: { half: 8, ch: { draw: ['dash', 1] },
      svg: id => `<path class="wv-heartline" pathLength="1" data-c="${id}.draw" d="M0 3.6C-4.4.6-5-1.8-5-3a2.6 2.6 0 0 1 5-1 2.6 2.6 0 0 1 5 1c0 1.2-.6 3.6-5 6.6Z"/>` },
    ripple: { half: 6, svg: () => `<circle class="wv-ripple" r="3"/>` }
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
    sy: v => ({ transform: `scale(1, ${n3(v)})` }),
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
    // rx/dz place the feet, deep scales around them; fly + ry lift the body, face mirrors it.
    const rootMatrix = v => mul(mul(translate(v('rx'), v('dz')), scale(v('deep'))), mul(translate(0, v('fly') + v('ry')), scale(v('face'), 1)));
    function robotMatrix(part, time) {
      const v = name => value(name, time);
      if (part === 'base') return mul(translate(v('rx'), v('dz')), scale(v('deep')));
      let m = rootMatrix(v);
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
      const body = mul(rootMatrix(v), mul(translate(0, HIP_Y), rotate(v('lean'))));
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
    function spin(id, distance, length, ease, at) {
      const turns = meta.get(id)?.art.wheels;
      if (turns) for (const part of turns.parts) api.tw({ [`${id}.${part}`]: channel(`${id}.${part}`).cur + deg(distance / turns.r) }, length, ease, at);
    }
    // Two-bone leg IK in the root frame: hip -> knee (7.5) -> sole (8.4), knee forward.
    function legAngles(side, lx, ly) {
      const A = 7.5, B = 8.4, dx = lx - (side === 'F' ? 1.6 : -1.6), dy = ly - HIP_Y;
      const d = clamp(Math.hypot(dx, dy), Math.abs(A - B) + 0.3, A + B - 0.05), base = Math.atan2(-dx, dy);
      const alpha = Math.acos(clamp((A * A + d * d - B * B) / (2 * A * d), -1, 1));
      const phi = Math.acos(clamp((A * A + B * B - d * d) / (2 * A * B), -1, 1));
      return { [`leg${side}`]: deg(base - alpha), [`knee${side}`]: deg(Math.PI - phi) };
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
      // Vehicles: props ride along with the robot (no gait) and their wheels turn with the distance.
      ride(ids, x, { speed = 60, ease = 'walk' } = {}) {
        const distance = x - channel('rx').cur;
        if (Math.abs(distance) < 0.05) return api;
        for (const id of ids) api.attach(id, 'base');
        const length = Math.max(420, Math.abs(distance) / speed * 1000), start = t;
        api.tw({ rx: x }, length, ease, start);
        for (const id of ids) spin(id, distance, length, ease, start);
        t = start + length;
        for (const id of ids) api.detach(id);
        return api;
      },
      // Rolls a free prop to x on its own wheels.
      roll(id, x, { speed = 60, ease = 'walk' } = {}) {
        const distance = x - poseOf(id, t).x;
        if (Math.abs(distance) < 0.05) return api;
        const length = Math.max(320, Math.abs(distance) / speed * 1000), start = t;
        api.tw({ [id]: { x } }, length, ease, start); spin(id, distance, length, ease, start);
        t = start + length;
        return api;
      },
      // Pedals a bike (crank at its (0, -5.4)) to x: the soles follow the pedals by leg IK.
      pedal(bike, x, { speed = 55, ease = 'io', snap = false } = {}) {
        const distance = x - channel('rx').cur;
        const inv = invert(rootMatrix(name => value(name, t)));
        const crank = apply(inv, poseOf(bike, t).x, -5.4), c0 = channel(`${bike}.crank`).cur;
        const legsAt = angle => Object.assign(legAngles('F', crank[0] + 2.6 * Math.sin(angle), crank[1] - 2.6 * Math.cos(angle)),
          legAngles('B', crank[0] - 2.6 * Math.sin(angle), crank[1] + 2.6 * Math.cos(angle)));
        if (snap) api.set(legsAt(rad(c0))); else api.to(legsAt(rad(c0)), 200);
        const length = Math.max(700, Math.abs(distance) / speed * 1000), start = t, easing = easingFns[ease];
        const revs = Math.max(1, Math.round(Math.abs(distance) / 20)), steps = revs * 8;
        api.attach(bike, 'base');
        api.tw({ rx: x }, length, ease, start); spin(bike, distance, length, ease, start);
        api.tw({ [`${bike}.crank`]: c0 + 360 * revs }, length, ease, start);
        let previous = start;
        for (let i = 1; i <= steps; i++) {
          const time = start + length * i / steps;
          api.tw(legsAt(rad(c0 + 360 * revs * easing(i / steps))), time - previous, 'linear', previous);
          previous = time;
        }
        t = start + length;
        api.detach(bike);
        return api;
      },
      // Places both soles on world points with two-bone leg IK.
      stepTo(targets, length = 0, ease = 'io') {
        const inv = invert(rootMatrix(name => value(name, t))), map = {};
        for (const [side, [x, y]] of Object.entries(targets)) { const [lx, ly] = apply(inv, x, y); Object.assign(map, legAngles(side, lx, ly)); }
        return length ? api.to(map, length, ease) : api.set(map);
      },
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
        // Faded out, or lifted above the window (jetpacks, drones), also counts as gone.
        robotHidden: value('ghost', time) < 0.01 || value('dz', time) + value('fly', time) * value('deep', time) < -100,
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

  // ------------------------------------------------------------------ composition
  // Composed skits are built from interchangeable pieces: an arrival, the activity, an
  // optional break and a departure, while ambient extras cross the room on their own
  // clock. Activities build their set with k.bring (a scan beam, a drop from the ceiling
  // or a cube that unfolds) and clear it before leaving, so every arrival and departure
  // owns an empty stage and any piece fits with any other.
  const STAND = { ry: 0, legF: 0, legB: 0, kneeF: 0, kneeB: 0, lean: 0, head: 0 };
  const SEAT_CAR = { ry: 9, legF: -84, legB: -80, kneeF: 20, kneeB: 24, lean: 3 };
  const FLYING = { lean: 8, legF: 8, kneeF: 14, legB: 18, kneeB: 20, armF: 24, elbF: -8, armB: 18, elbB: -10 };
  const HANGING = { armF: -174, elbF: -4, armB: -168, elbB: -8, legF: 6, kneeF: 10, legB: 12, kneeB: 14 };
  const SNEAK = { armF: -76, elbF: -96, armB: -44, elbB: -104 };
  const faded = (ids, v) => Object.fromEntries(ids.map(id => [`${id}.o`, v]));
  // A teleport-style flicker for the robot and whatever it carries.
  function flicker(c, show, carry = [], length = 760) {
    for (const v of show ? [0.6, 0.15, 0.85, 0.35, 1] : [0.4, 0.9, 0.2, 0.6, 0]) c.to({ ghost: v, ...faded(carry, v) }, length / 5, 'linear');
  }
  function puff(c, x, id = 'mpuff') {
    c.set({ [id]: { x, y: 0 }, [`${id}.o`]: 0.85, [`${id}.pop`]: 0.5 });
    c.tw({ [`${id}.o`]: 0, [`${id}.pop`]: 1.6 }, 560, 'out');
  }
  const hop = (c, map, length = 520, height = 8) => c.par(a => a.to(map, length, 'io'), a => a.to({ fly: -height }, length / 2, 'out').to({ fly: 0 }, length / 2, 'in'));
  // Gear folds into a palm-sized cube that the robot pockets, and unfolds from it.
  function fold(c, id) {
    c.to({ armF: -40, elbF: -50 }, 220);
    const pocket = c.point('root', [3, -12]), from = c.pose(id);
    c.par(a => a.to({ [`${id}.pop`]: 0.12, [`${id}.tilt`]: 180 }, 440, 'in'),
      a => a.path(id, k => ({ x: from.x + (pocket[0] - from.x) * k, y: from.y + (pocket[1] - from.y) * k }), 440, 'in'));
    c.to({ [`${id}.o`]: 0 }, 120); c.set({ [`${id}.tilt`]: 0 }); c.rest(220);
  }
  function unfold(c, id, x) {
    c.to({ armF: -40, elbF: -50 }, 220);
    const palm = c.point('handF');
    c.set({ [id]: { x: palm[0], y: palm[1], r: 0, sx: 1, sy: 1 }, [`${id}.pop`]: 0.12, [`${id}.tilt`]: -180, [`${id}.o`]: 1 });
    c.par(a => a.to({ [`${id}.pop`]: 1, [`${id}.tilt`]: 0 }, 500, 'back'),
      a => a.path(id, k => ({ x: palm[0] + (x - palm[0]) * k, y: palm[1] * (1 - k) - 8 * Math.sin(Math.PI * k) }), 500, 'io'));
    c.rest(200);
  }
  function strap(c, id) {
    const face = Math.sign(c.cur('face')) || 1, back = c.point('body', [-7.4, -9]);
    c.set({ [id]: { x: back[0], y: back[1], sx: face } }); c.attach(id, 'body');
  }
  const faceRight = c => { if (c.cur('face') < 0) c.turn(1); };

  const MOVES = {
    walk: { weight: 7, carry: true,
      arrive(c, r, x, carry) { c.set({ rx: -38 }); c.walk(x, { speed: r.range(carry.length ? 26 : 34, carry.length ? 30 : 42), arms: !carry.length }); },
      leave(c, r, carry) { c.walk(242, { speed: r.range(carry.length ? 26 : 36, carry.length ? 30 : 44), arms: !carry.length }); } },
    run: { weight: 3, carry: true, props: () => [{ id: 'mpuff', art: 'puff', o: 0, layer: 'front' }],
      arrive(c, r, x, carry) {
        c.set({ rx: -46 });
        c.par(a => a.walk(x - 10, { speed: 84, period: 210, ease: 'linear', arms: !carry.length }), a => a.to({ lean: 11 }, 240));
        // A skid stop with a puff of dust.
        c.par(a => a.to({ rx: x, lean: -9, legF: -24, kneeF: 4, legB: 16, kneeB: 18, ry: 1.6 }, 340, 'out'), a => puff(a, x + 5));
        c.to(STAND, 300);
      },
      leave(c, r, carry) {
        faceRight(c);
        const x = c.cur('rx');
        c.to({ lean: 11, ry: 1, kneeF: 16, kneeB: 16 }, 160, 'out');
        c.par(a => a.walk(252, { speed: 86, period: 210, ease: 'in', face: false, arms: !carry.length }), a => puff(a, x - 4));
      } },
    sneak: { weight: 2, carry: true, props: r => [{ id: 'mpsiu', art: 'bubble', layer: 'front', text: r.pick(['PSIU…', 'SHHH…']), o: 0 }],
      arrive(c, r, x, carry) {
        c.set({ rx: -36, lean: 13, head: 6, ...(carry.length ? {} : SNEAK) });
        c.walk(x, { speed: 17, arms: false, period: 460 });
        c.look(-1.2, 0, 220); c.wait(240); c.look(3.8, 0, 260); c.wait(200); c.look(0, -0.4, 180);
        c.say('mpsiu', 560, { dx: 8, dy: -18 }); c.look(2.4, 0, 160);
        c.to({ lean: 0, head: 0, ...(carry.length ? {} : REST) }, 320);
      },
      leave(c, r, carry) {
        faceRight(c);
        c.to({ lean: 13, head: 6, ...(carry.length ? {} : SNEAK) }, 300);
        c.look(-1.2, 0, 200); c.wait(220); c.look(2.4, 0, 160);
        c.walk(240, { speed: 18, arms: false, period: 460 });
      } },
    car: { weight: 2, props: r => [{ id: 'mcar', art: 'car', x: -70, layer: 'front' },
      { id: 'mhonk', art: 'bubble', layer: 'front', text: r.pick(['BI-BI!', 'FOM-FOM!']), tone: 'pow', o: 0 },
      { id: 'mfiu', art: 'bubble', layer: 'front', text: 'FIU-FIU!', o: 0 }],
      arrive(c, r, x) {
        const car = x + 24;
        c.set({ rx: -75, ...SEAT_CAR, mcar: { x: -70 } });
        c.reach('both', [-66, -15.5], 0, 'io', { with: { lean: 3 } });
        c.ride(['mcar'], car - 5, { speed: 74, ease: 'out' });
        c.to({ 'mcar.tilt': 2.5 }, 110, 'out').to({ 'mcar.tilt': 0 }, 280, 'back');
        c.say('mhonk', 480, { on: 'mcar', dx: 12, dy: -14 });
        // Hop out; the self-driving car leaves on its own and the robot waves it off.
        hop(c, { rx: x, ...STAND, ...REST }, 560, 9);
        c.look(3.6, 0);
        c.par(a => a.roll('mcar', 264, { speed: 95, ease: 'in' }), a => { a.mood('happy'); a.wave(2); a.rest(260); a.mood('open'); a.look(2.4, 0); });
      },
      leave(c) {
        faceRight(c);
        const x = c.cur('rx');
        c.to({ armF: -150, elbF: -118, head: -3 }, 260, 'out'); c.say('mfiu', 520, { dx: 9, dy: -18 }); c.rest(220);
        // Whistled for, the car backs in from wherever it waited.
        c.set({ mcar: { x: 264 } }); c.look(3.6, 0);
        c.roll('mcar', x + 24, { speed: 80, ease: 'out' });
        hop(c, { rx: x + 19, ...SEAT_CAR }, 560, 9);
        c.reach('both', [x + 28, -15.5], 220, 'io', { with: { lean: 3 } });
        c.say('mhonk', 400, { on: 'mcar', dx: 12, dy: -14 });
        c.ride(['mcar'], 270, { speed: 90, ease: 'in' });
      } },
    bike: { weight: 2, props: () => [{ id: 'mbike', art: 'bike', x: -60 }, { id: 'mtrim', art: 'bubble', layer: 'front', text: 'TRIM-TRIM!', tone: 'pow', o: 0 }],
      arrive(c, r, x) {
        c.set({ rx: -64, ry: -0.6, mbike: { x: -60.4 } });
        c.reach('both', [-50.2, -19.4], 0, 'io', { with: { lean: 18 } });
        c.pedal('mbike', x + 7, { speed: 58, ease: 'out', snap: true });
        c.say('mtrim', 480, { on: 'mbike', dx: 8, dy: -26 });
        hop(c, { rx: x, ...STAND, ...REST }, 480, 6);
        fold(c, 'mbike');
      },
      leave(c) {
        faceRight(c);
        const x = c.cur('rx');
        unfold(c, 'mbike', x + 10);
        hop(c, { rx: x + 6.4, ry: -0.6, lean: 18 }, 480, 6);
        c.reach('both', [x + 20.2, -19.4], 220, 'io', { with: { lean: 18 } });
        c.say('mtrim', 420, { on: 'mbike', dx: 8, dy: -26 });
        c.pedal('mbike', 268, { speed: 64, ease: 'in' });
      } },
    scooter: { weight: 2, props: () => [{ id: 'mscoot', art: 'scooter', x: -60 }],
      arrive(c, r, x) {
        const ride = { ry: -2.6, legF: -6, kneeF: 4, legB: 12, kneeB: 8, lean: 6 };
        c.set({ rx: -61, ...ride, mscoot: { x: -60 } });
        c.reach('both', [-53.4, -24], 0);
        c.par(a => a.ride(['mscoot'], x + 7, { speed: 72, ease: 'out' }), a => a.to({ lean: 9 }, 600).to({ lean: 6 }, 600));
        hop(c, { rx: x, ...STAND, ...REST }, 460, 6);
        fold(c, 'mscoot');
      },
      leave(c) {
        faceRight(c);
        const x = c.cur('rx');
        unfold(c, 'mscoot', x + 8);
        hop(c, { rx: x + 7, ry: -2.6, legF: -6, kneeF: 4, legB: 12, kneeB: 8, lean: 6 }, 460, 6);
        c.reach('both', [x + 14.6, -24], 200);
        c.ride(['mscoot'], 266, { speed: 80, ease: 'in' });
      } },
    hover: { weight: 2, props: () => [{ id: 'mhover', art: 'hover', x: -60 }],
      arrive(c, r, x) {
        const surf = { ry: -4.4, legF: -14, kneeF: 12, legB: 16, kneeB: 10, armF: -70, elbF: -12, armB: 64, elbB: -14, lean: 4 };
        c.set({ rx: -60, ...surf, mhover: { x: -60 } });
        const start = c.t;
        c.ride(['mhover'], x + 4, { speed: 52, ease: 'soft' });
        const end = c.t; c.at(start);
        for (let time = start; time < end - 500; time += 500) c.to({ dz: -1.4 }, 250).to({ dz: 0 }, 250);
        c.at(Math.max(c.t, end));
        hop(c, { rx: x, ...STAND, ...REST }, 460, 5);
        fold(c, 'mhover');
      },
      leave(c) {
        faceRight(c);
        const x = c.cur('rx');
        unfold(c, 'mhover', x + 4);
        hop(c, { rx: x + 4, ry: -4.4, legF: -14, kneeF: 12, legB: 16, kneeB: 10, armF: -70, elbF: -12, armB: 64, elbB: -14, lean: 4 }, 460, 5);
        const start = c.t;
        c.ride(['mhover'], 266, { speed: 60, ease: 'in' });
        const end = c.t; c.at(start);
        for (let time = start; time < end - 500; time += 500) c.to({ dz: -1.4 }, 250).to({ dz: 0 }, 250);
        c.at(Math.max(c.t, end));
      } },
    jetpack: { weight: 2, props: r => [{ id: 'mjet', art: 'jetpack', o: 0, y: -150 }, { id: 'mpuff', art: 'puff', o: 0, layer: 'front' },
      { id: 'mvush', art: 'bubble', layer: 'front', text: r.pick(['VUUUSH!', 'FUUUU!']), tone: 'pow', o: 0 }],
      arrive(c, r, x) {
        c.set({ rx: x - 56, fly: -132, ...FLYING });
        strap(c, 'mjet'); c.set({ 'mjet.o': 1, 'mjet.flame': 1 });
        c.par(a => a.tw({ rx: x }, 1700, 'out'), a => a.to({ fly: 0 }, 1700, 'out'),
          a => { a.wait(1000); a.to({ lean: -4, legF: -10, kneeF: 18, legB: 6, kneeB: 18, armF: 4, armB: -6 }, 700); });
        c.set({ 'mjet.flame': 0 }); puff(c, x);
        c.crouch(3, 160, 'out'); c.crouch(0, 260);
        c.to({ ...REST, lean: 0 }, 240);
        c.to({ 'mjet.pop': 0.1, 'mjet.o': 0 }, 260, 'in');
      },
      leave(c) {
        faceRight(c);
        const x = c.cur('rx');
        strap(c, 'mjet'); c.set({ 'mjet.pop': 0.1 }); c.to({ 'mjet.o': 1, 'mjet.pop': 1 }, 320, 'back');
        c.crouch(3, 240); c.set({ 'mjet.flame': 1 }); puff(c, x); c.say('mvush', 420, { dx: 8, dy: -20 });
        c.par(a => a.to({ fly: -142 }, 1300, 'in'), a => a.tw({ rx: x + 46 }, 1300, 'in'), a => a.to({ ry: 0, ...FLYING }, 500));
      } },
    drone: { weight: 2, props: () => [{ id: 'mdrone', art: 'drone', x: -60, y: -150, layer: 'front' }],
      arrive(c, r, x) {
        c.set({ rx: x - 50, fly: -130, ...HANGING });
        const hand = c.point('handF'); c.set({ mdrone: { x: hand[0], y: hand[1] - 5.2 } }); c.attach('mdrone', 'handF');
        c.par(a => a.tw({ rx: x }, 1800, 'soft'), a => a.to({ fly: 0 }, 1800, 'out'));
        c.crouch(2.4, 160, 'out'); c.detach('mdrone');
        const d = c.pose('mdrone');
        c.par(a => a.path('mdrone', k => ({ x: d.x + 90 * k * k, y: d.y - 120 * k, r: 12 * k }), 1200, 'in'),
          a => { a.crouch(0, 260); a.rest(300); a.look(3, -2, 200); a.wave(2); a.rest(260); a.look(2.4, 0); });
      },
      leave(c) {
        const x = c.cur('rx');
        c.look(3, -2, 200); c.to({ armF: -174, elbF: -4, armB: -168, elbB: -8 }, 420);
        const grip = c.point('handF');
        c.set({ mdrone: { x: x + 90, y: -150, r: 0 } });
        c.path('mdrone', k => ({ x: x + 90 + (grip[0] - x - 90) * k, y: -150 + (grip[1] - 5.2 + 150) * k }), 1100, 'out'); c.wait(1100);
        c.attach('mdrone', 'handF');
        c.par(a => a.to({ fly: -142 }, 1500, 'in'), a => a.tw({ rx: x - 30 }, 1500, 'in'), a => a.to({ legF: 6, kneeF: 10, legB: 12, kneeB: 14 }, 400));
      } },
    teleport: { weight: 2, carry: true, props: r => [{ id: 'mtele', art: 'tele', o: 0 }, { id: 'mzap', art: 'bubble', layer: 'front', text: r.pick(['ZAP!', 'VUPT!']), tone: 'pow', o: 0 }],
      arrive(c, r, x, carry) {
        c.set({ rx: x, ghost: 0, mtele: { x }, ...faded(carry, 0) });
        c.to({ 'mtele.o': 1 }, 260); c.to({ 'mtele.col': 1 }, 300);
        flicker(c, true, carry); c.say('mzap', 380, { dx: 9, dy: -20 });
        c.to({ 'mtele.col': 0 }, 300); c.to({ 'mtele.o': 0 }, 240);
      },
      leave(c, r, carry) {
        const x = c.cur('rx'); c.set({ mtele: { x } });
        c.to({ 'mtele.o': 1 }, 240); c.to({ 'mtele.col': 1 }, 300);
        if (!carry.length) { c.look(0, -0.4, 160); c.mood('happy'); c.wave(2); c.rest(200); c.mood('open'); }
        flicker(c, false, carry); c.to({ 'mtele.col': 0 }, 260); c.to({ 'mtele.o': 0 }, 220);
      } },
    // Out of (and back into) a secret door at the back: the corridor gives real depth.
    door: { weight: 2, carry: true, props: () => [{ id: 'mdoor', art: 'door', o: 0 }, { id: 'mshut', art: 'shutter', o: 0, layer: 'front' }],
      arrive(c, r, x, carry, k) {
        const d = k.doorX();
        c.set({ rx: d, dz: -7, deep: 0.52, ghost: 0, mdoor: { x: d }, mshut: { x: d }, ...faded(carry, 0) });
        c.to({ 'mdoor.o': 1, 'mshut.o': 1 }, 360); c.to({ 'mdoor.trim': 0 }, 520);
        c.to({ 'mshut.roll': 0.02 }, 700, 'io'); c.to({ 'mshut.o': 0 }, 80);
        c.to({ ghost: 1, ...faded(carry, 1) }, 420);
        c.par(a => a.gait(a.t, 1400, { arms: !carry.length }), a => a.to({ dz: 0, deep: 1 }, 1400, 'io'));
        c.par(a => a.walk(x, { speed: 34 }), a => { a.wait(500); a.to({ 'mshut.o': 1 }, 60); a.to({ 'mshut.roll': 1 }, 600, 'io'); a.to({ 'mdoor.trim': 1 }, 400); a.to({ 'mdoor.o': 0, 'mshut.o': 0 }, 360); });
      },
      leave(c, r, carry, k) {
        const d = k.doorX();
        c.set({ mdoor: { x: d }, mshut: { x: d }, 'mshut.roll': 1 });
        c.par(a => a.walk(d, { speed: 34 }), a => { a.to({ 'mdoor.o': 1, 'mshut.o': 1 }, 360); a.to({ 'mdoor.trim': 0 }, 520); a.to({ 'mshut.roll': 0.02 }, 700, 'io'); a.to({ 'mshut.o': 0 }, 80); });
        c.par(a => a.gait(a.t, 1400, { arms: !carry.length }), a => a.to({ dz: -7, deep: 0.52 }, 1400, 'io'));
        c.to({ ghost: 0, ...faded(carry, 0) }, 420);
        c.to({ 'mshut.o': 1 }, 60); c.to({ 'mshut.roll': 1 }, 600, 'io'); c.to({ 'mdoor.trim': 1 }, 400); c.to({ 'mdoor.o': 0, 'mshut.o': 0 }, 360);
      } },
    chair: { weight: 1, props: r => [{ id: 'mchair', art: 'chair', x: -60 }, { id: 'muii', art: 'bubble', layer: 'front', text: r.pick(['UIII!', 'ÉGUAAA!']), tone: 'pow', o: 0 }],
      arrive(c, r, x) {
        // Rolls in backwards on an office chair, kicking the floor.
        c.set({ rx: -54, face: -1, ...sitPose(9.5), mchair: { x: -54, sx: -1 } });
        c.attach('mchair', 'base');
        for (let pos = -54; pos < x - 4;) {
          pos = Math.min(x, pos + 26);
          c.to({ legF: -32, kneeF: 24, legB: -28, kneeB: 28 }, 190, 'io');
          c.to({ rx: pos, legF: -86, kneeF: 84, legB: -80, kneeB: 80 }, 460, 'out');
        }
        c.detach('mchair');
        c.par(a => a.turn(1, 200).turn(-1, 200).turn(1, 200), a => a.say('muii', 520, { dx: 8, dy: -16 }));
        c.to({ ...STAND, ...REST }, 380);
        c.par(a => a.roll('mchair', -70, { speed: 80, ease: 'out' }), a => { a.look(-1, 0, 200); a.wait(500); a.look(2.4, 0, 200); });
      } },
    slide: { weight: 1, props: r => [{ id: 'mpuff', art: 'puff', o: 0, layer: 'front' }, { id: 'mwow', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA!', 'CHEGUEI, MACHO!']), tone: 'pow', o: 0 }],
      arrive(c, r, x) {
        c.set({ rx: -46 });
        c.par(a => a.walk(x - 36, { speed: 88, period: 200, ease: 'in' }), a => a.to({ lean: 12 }, 260));
        // A rock-star knee slide.
        c.par(a => a.to({ ry: 9.4, legF: -6, kneeF: 92, legB: 4, kneeB: 92, lean: -14, armF: -160, elbF: -20, armB: -150, elbB: -24, head: -8 }, 220, 'out'),
          a => a.to({ rx: x }, 900, 'out'), a => puff(a, x - 32));
        c.mood('happy'); c.say('mwow', 560, { dx: 8, dy: -18 });
        c.to({ ...STAND, ...REST }, 420); c.mood('open');
      } },
    moonwalk: { weight: 1, props: r => [{ id: 'mhihi', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA!', 'RI-RI!']), tone: 'pow', o: 0 }],
      leave(c) {
        if (c.cur('face') > 0) c.turn(-1);
        let x = c.cur('rx');
        const step = () => {
          c.to({ rx: x += 10, legF: 0, kneeF: 24, legB: 7, kneeB: 0 }, 300, 'linear');
          c.to({ rx: x += 10, legF: 7, kneeF: 0, legB: 0, kneeB: 24 }, 300, 'linear');
        };
        for (let i = 0; i < 2; i++) step();
        c.to({ legF: 0, legB: 0, kneeF: 0, kneeB: 0 }, 120);
        c.turn(1, 180).turn(-1, 180);
        c.to({ fly: -2.4, legF: -6, legB: 4, armF: -150, elbF: -10 }, 200); c.say('mhihi', 420, { dx: -8, dy: -18 }); c.to({ fly: 0, ...REST, legF: 0, legB: 0 }, 220);
        while (x < 242) step();
      } }
  };
  const ARRIVALS = Object.keys(MOVES).filter(name => MOVES[name].arrive), DEPARTURES = Object.keys(MOVES).filter(name => MOVES[name].leave);

  // Sets arrive and go in one of three ways; hide() is the hidden starting state.
  function wrist(c) {
    c.to({ armF: -58, elbF: -100 }, 240, 'out'); c.look(2, 1.6, 120);
    c.to({ armB: -30, elbB: -112 }, 200); c.to({ elbB: -100 }, 90).to({ elbB: -112 }, 90);
    c.look(2.4, 0, 120); c.rest(260);
  }
  function sweep(c, ids, show, length = 900) {
    const xs = ids.map(id => c.pose(id).x), lo = Math.min(...xs) - 16, hi = Math.max(...xs) + 16, start = c.t;
    c.set({ scan: { x: (lo + hi) / 2, y: -62, sx: (hi - lo) / 40 } });
    c.tw({ 'scan.o': 1 }, 120);
    c.path('scan', k => ({ y: -62 + 62 * k }), length, 'io');
    ids.forEach((id, i) => {
      let at = start + length * (0.3 + 0.4 * i / Math.max(1, ids.length));
      for (const v of show ? [0.7, 0.2, 0.9, 0.5, 1] : [0.4, 0.9, 0.2, 0.6, 0]) { c.tw({ [`${id}.o`]: v }, 70, 'linear', at); at += 70; }
    });
    c.at(start + length); c.to({ 'scan.o': 0 }, 160);
  }
  const DELIVER = {
    beam: { hide: () => ({ o: 0 }),
      bring(c, ids) { wrist(c); sweep(c, ids, true); },
      clear(c, ids) { wrist(c); sweep(c, ids, false); } },
    drop: { hide: () => ({ y: -150 }),
      bring(c, ids, homes) {
        c.to({ armF: -168, elbF: -12, head: -8 }, 300, 'out'); c.look(2, -2, 160);
        let landed = c.t;
        ids.forEach((id, i) => {
          const home = homes[id], s = c.t + i * 170;
          c.at(s); c.path(id, k => ({ y: -150 + (home.y + 150) * k }), 560, 'in');
          c.at(s + 560); c.to({ [id]: { sy: 0.84 } }, 90, 'out').to({ [id]: { sy: 1 } }, 260, 'back');
          landed = Math.max(landed, c.t);
        });
        c.at(landed - 300); c.say('dthud', 380, { on: ids[0], dx: 0, dy: -18 });
        c.to({ head: 0 }, 200); c.rest(240); c.look(2.4, 0, 160);
      },
      clear(c, ids) {
        c.to({ armF: -168, elbF: -12, head: -8 }, 300, 'out'); c.look(2, -2, 160);
        let gone = c.t;
        ids.forEach((id, i) => {
          const from = c.pose(id), s = c.t + i * 120;
          c.at(s); c.path(id, k => ({ y: from.y + (-150 - from.y) * k }), 760, 'in'); gone = Math.max(gone, s + 760);
        });
        c.at(gone); c.to({ head: 0 }, 200); c.rest(240); c.look(2.4, 0, 160);
      } },
    unfold: { hide: () => ({ o: 0 }),
      bring(c, ids, homes) {
        c.to({ armF: -40, elbF: -58 }, 260);
        for (const id of ids) {
          const home = homes[id], palm = c.point('handF', [0, -2]);
          c.set({ [id]: { x: palm[0], y: palm[1] }, [`${id}.pop`]: 0.08, [`${id}.o`]: 1 });
          c.path(id, k => ({ x: palm[0] + (home.x - palm[0]) * k, y: palm[1] + (home.y - palm[1]) * k - 10 * Math.sin(Math.PI * k) }), 580, 'io');
          c.tw({ [`${id}.pop`]: 1 }, 580, 'back'); c.wait(430);
        }
        c.wait(150); c.rest(260);
      },
      clear(c, ids) {
        c.to({ armF: -40, elbF: -58 }, 260);
        for (const id of ids) {
          const from = c.pose(id), palm = c.point('handF', [0, -2]);
          c.path(id, k => ({ x: from.x + (palm[0] - from.x) * k, y: from.y + (palm[1] - from.y) * k - 10 * Math.sin(Math.PI * k) }), 520, 'io');
          c.tw({ [`${id}.pop`]: 0.08 }, 520, 'in'); c.wait(520); c.to({ [`${id}.o`]: 0 }, 80);
        }
        c.rest(260);
      } }
  };

  // Breaks happen in place, hands free, and end in the standing pose they started from.
  const BREAKS = {
    rest: { weight: 3, props: r => [{ id: 'bufa', art: 'bubble', layer: 'front', text: r.pick(['UFA!', 'AFE, MARIA!']), tone: 'pow', o: 0 }],
      play(c) {
        c.crouch(5, 300);
        c.to({ ry: 15.4, legF: -88, legB: -84, kneeF: 4, kneeB: 8, lean: -12, armF: 34, elbF: -18, armB: 40, elbB: -14 }, 440, 'io');
        c.mood('closed'); c.to({ armF: -150, elbF: -112, head: -6 }, 360);
        c.to({ elbF: -92 }, 160).to({ elbF: -114 }, 160).to({ elbF: -92 }, 160);
        c.par(a => a.to({ armF: 34, elbF: -18, head: 0 }, 320), a => a.say('bufa', 640, { dx: 8, dy: -16 }));
        for (let i = 0; i < 2; i++) c.to({ lean: -15 }, 560).to({ lean: -11 }, 560);
        c.mood('open'); c.to({ ry: 5.6, legF: -38, kneeF: 74, legB: -34, kneeB: 76, lean: 18, ...REST }, 420);
        c.to({ ...STAND, ...REST }, 380);
      } },
    coffee: { weight: 3, props: r => [{ id: 'bmug', art: 'mug', layer: 'front', o: 0 }, { id: 'bslurp', art: 'bubble', layer: 'front', text: r.pick(['SÓ O FILÉ, OH!', 'GOSTOSO, OH!']), o: 0 }],
      play(c) {
        const face = Math.sign(c.cur('face')) || 1;
        c.to({ armF: -62, elbF: -40 }, 300);
        const hand = c.point('handF');
        c.set({ bmug: { x: hand[0] + 4.2 * face, y: hand[1] + 3.4, sx: face }, 'bmug.pop': 0.3, 'bmug.steam': 1 }); c.attach('bmug', 'handF');
        c.to({ 'bmug.o': 1, 'bmug.pop': 1 }, 300, 'back');
        for (let sip = 0; sip < 2; sip++) {
          c.par(a => a.reach('F', a.point('root', [7.5, -36]), 480, 'io', { with: { lean: -2 } }), a => a.tw({ head: -9, 'bmug.tilt': -24 * face }, 480));
          c.mood('closed'); c.wait(sip ? 360 : 520); c.mood('happy');
          c.par(a => a.reach('F', a.point('root', [10, -25]), 400, 'io', { with: { lean: 0 } }), a => a.tw({ head: 0, 'bmug.tilt': 0 }, 400));
          if (!sip) c.say('bslurp', 700);
        }
        c.to({ 'bmug.pop': 0.2, 'bmug.o': 0 }, 260); c.detach('bmug'); c.rest(300); c.mood('open');
      } },
    update: { weight: 2, props: r => [{ id: 'bbar', art: 'upbar', layer: 'front', o: 0 }, { id: 'bok', art: 'bubble', layer: 'front', text: r.pick(['PRONTO, MACHO!', 'ATUALIZADO!']), tone: 'pow', o: 0 }],
      play(c) {
        c.look(0, 0, 160); c.mood('closed'); c.tw({ bulb: 0.3 }, 300);
        c.to({ head: 8, lean: 3, armF: 8, elbF: -6, armB: -8, elbB: -6 }, 320);
        const [x, y] = c.point('root', [0, -62]); c.set({ bbar: { x, y } });
        c.to({ 'bbar.o': 1 }, 200); c.to({ 'bbar.fill': 0.36 }, 700, 'out'); c.wait(300); c.to({ 'bbar.fill': 0.42 }, 600); c.to({ 'bbar.fill': 1 }, 800, 'in');
        c.wait(160); c.to({ 'bbar.o': 0 }, 200);
        c.set({ bulb: 1 }); c.mood('wide'); c.to({ head: 0, lean: 0, ...REST }, 200); c.hop(3, 320); c.mood('happy'); c.say('bok', 600); c.mood('open'); c.look(2.4, 0);
      } },
    battery: { weight: 2, props: r => [{ id: 'bbat', art: 'batt', layer: 'front', o: 0 }, { id: 'bcell', art: 'cell', layer: 'front', o: 0 },
      { id: 'bgas', art: 'bubble', layer: 'front', text: r.pick(['TÔ NO GÁS!', 'CARREGADO, MACHO!']), tone: 'pow', o: 0 }],
      play(c) {
        const [x, y] = c.point('root', [0, -61]); c.set({ bbat: { x, y } }); c.to({ 'bbat.o': 1 }, 220);
        c.to({ lean: 14, head: 16, armF: 14, elbF: -4, armB: 6, elbB: -4, ry: 1 }, 700, 'in'); c.tw({ bulb: 0.25 }, 400); c.mood('closed');
        c.wait(500); c.mood('open'); c.tw({ bulb: 0.6 }, 200);
        c.to({ lean: 4, head: 4, ry: 0 }, 300); c.to({ armF: 12, elbF: -24 }, 260);
        const hand = c.point('handF'); c.set({ bcell: { x: hand[0], y: hand[1] + 1 }, 'bcell.pop': 0.4 }); c.attach('bcell', 'handF');
        c.to({ 'bcell.o': 1, 'bcell.pop': 1 }, 220);
        c.reach('F', c.point('body', [-1.4, -7.4]), 380, 'io');
        c.to({ 'bcell.o': 0, 'bcell.pop': 0.3 }, 160); c.detach('bcell');
        c.set({ 'bbat.low': 0 }); c.to({ 'bbat.full': 1 }, 200); c.tw({ bulb: 1 }, 200); c.mood('wide');
        c.to({ ...REST, lean: 0, head: 0 }, 260); c.hop(5, 360); c.mood('happy'); c.say('bgas', 600); c.to({ 'bbat.o': 0 }, 220); c.mood('open');
      } },
    stretch: { weight: 2, props: [],
      play(c) {
        c.mood('closed'); c.to({ armF: -172, elbF: -6, armB: -166, elbB: -10, head: -8 }, 520);
        c.to({ lean: -10 }, 420); c.to({ lean: 12 }, 520); c.to({ lean: 0 }, 360); c.wait(260);
        c.rest(380); c.mood('happy'); c.wait(260); c.mood('open');
      } },
    wave: { weight: 2, props: r => [{ id: 'bhi', art: 'bubble', layer: 'front', text: r.pick(['EI, MÁ!', 'E AÍ, MACHO?', 'EI, BICHO!']), o: 0 }],
      play(c) {
        c.look(0, -0.4, 200); c.mood('happy');
        c.par(a => a.wave(3), a => { a.wait(140); a.say('bhi', 800); });
        c.rest(280); c.mood('open'); c.look(2.4, 0, 180);
      } },
    yawn: { weight: 2, props: () => [{ id: 'byawn', art: 'bubble', layer: 'front', text: 'UAAAHH…', o: 0 }],
      play(c) {
        c.mood('closed');
        c.par(a => a.to({ armF: -150, elbF: -60, armB: -140, elbB: -64, head: -14, lean: -6 }, 600), a => { a.wait(200); a.say('byawn', 700, { dx: 8, dy: -20 }); });
        c.to({ head: 0, lean: 0, ...REST }, 420); c.to({ head: 6 }, 160).to({ head: -6 }, 160).to({ head: 0 }, 160);
        c.mood('open');
      } },
    glitch: { weight: 2, props: r => [{ id: 'bbug', art: 'bubble', layer: 'front', text: r.pick(['OXE, TRAVOU!', 'BUGOU, MACHO!']), tone: 'hot', o: 0 }],
      play(c, r, k) {
        c.mood('wide');
        for (let i = 0; i < 9; i++) c.to({ ry: r.range(-1.2, 1.2), lean: r.range(-6, 6), head: r.range(-12, 12), ghost: i % 3 === 1 ? 0.35 : 1, bulb: i % 2 ? 0.2 : 1 }, 60, 'linear');
        c.to({ ry: 0, lean: 0, head: 14, ghost: 1, bulb: 1 }, 120);
        c.wait(380);
        c.to({ armF: -150, elbF: -120 }, 220, 'out'); c.to({ elbF: -100 }, 80).to({ elbF: -124 }, 80);
        c.to({ head: 0 }, 160, 'back'); c.rest(260);
        c.say('bbug', 600); c.mood('happy'); c.wait(160); c.mood('open');
      } },
    dance: { weight: 2, props: [],
      play(c, r, k) {
        c.look(1, -0.6); c.mood('happy');
        const beats = r.int(4, 6);
        for (let i = 0; i < beats; i++) {
          const s = i % 2 ? -1 : 1;
          c.to({ lean: 8 * s, ry: -1.6, armF: -120 + 30 * s, elbF: -40, armB: -120 - 30 * s, elbB: -40, legF: -8 * s, legB: 8 * s }, 220, 'out');
          c.to({ ry: 0, legF: 0, legB: 0 }, 190, 'in');
        }
        c.rest(300); c.to({ lean: 0 }, 120); c.mood('open'); c.look(2.4, 0);
      } },
    lookout: { weight: 2, props: r => [{ id: 'bpsiu', art: 'bubble', layer: 'front', text: r.pick(['PSIU…', 'TEM ALGUÉM AÍ?']), o: 0 }],
      play(c) {
        c.crouch(4, 300); c.to({ lean: 10, armF: -76, elbF: -96 }, 260);
        c.look(-1.4, 0, 200); c.to({ head: -4 }, 200); c.wait(380); c.look(3.8, 0, 260); c.wait(380); c.look(0, -0.4, 200);
        c.to({ armF: -128, elbF: -128 }, 260);
        c.say('bpsiu', 600, { dx: 8, dy: -16 });
        c.par(a => a.crouch(0, 320), a => a.to({ lean: 0, head: 0, ...REST }, 320)); c.look(2.4, 0);
      } },
    phone: { weight: 2, props: r => [{ id: 'bphone', art: 'phone', layer: 'front', o: 0 }, { id: 'balo', art: 'bubble', layer: 'front', text: r.pick(['ALÔ, MACHO?', 'DIGA, BICHO!']), o: 0 },
      { id: 'bta', art: 'bubble', layer: 'front', text: r.pick(['TÁ, TÁ…', 'SEI, SEI…']), o: 0 }],
      play(c) {
        c.to({ armF: -150, elbF: -118, head: 6 }, 320);
        const ear = c.point('handF'); c.set({ bphone: { x: ear[0], y: ear[1] - 1 }, 'bphone.pop': 0.3 }); c.attach('bphone', 'handF');
        c.to({ 'bphone.o': 1, 'bphone.pop': 1 }, 200, 'back');
        c.say('balo', 620, { dx: 9, dy: -18 }); c.nod(2); c.look(3.4, 0.8, 200); c.say('bta', 560, { dx: 9, dy: -18 }); c.nod(1);
        c.to({ 'bphone.o': 0, 'bphone.pop': 0.3 }, 200); c.detach('bphone'); c.rest(300); c.to({ head: 0 }, 160); c.look(2.4, 0);
      } },
    watch: { weight: 2, props: r => [{ id: 'blate', art: 'bubble', layer: 'front', text: r.pick(['VISH, TÁ TARDE!', 'EITA, A HORA!']), tone: 'pow', o: 0 }],
      play(c) {
        c.to({ armF: -76, elbF: -78 }, 300); c.look(2.2, 1.6, 160); c.to({ head: 8 }, 200); c.wait(500);
        c.mood('wide'); c.say('blate', 560); c.mood('open'); c.to({ head: 0 }, 160); c.rest(260); c.look(2.4, 0);
      } },
    meditate: { weight: 1, props: () => [{ id: 'borbit', art: 'orbit', layer: 'front', o: 0 }, { id: 'bom', art: 'bubble', layer: 'front', text: 'OMMM…', o: 0 }],
      play(c) {
        c.crouch(5, 300);
        c.to({ ry: 13, legF: -84, kneeF: 140, legB: -80, kneeB: 136, armF: -40, elbF: -60, armB: -36, elbB: -64 }, 420); c.mood('closed');
        c.to({ fly: -7 }, 700, 'io');
        const [x, y] = c.point('root', [0, -24]); c.set({ borbit: { x, y } }); c.attach('borbit', 'root'); c.to({ 'borbit.o': 1 }, 400);
        c.say('bom', 900, { dx: 8, dy: -18 });
        for (let i = 0; i < 2; i++) c.to({ fly: -9 }, 600).to({ fly: -7 }, 600);
        c.to({ 'borbit.o': 0 }, 300); c.detach('borbit'); c.to({ fly: 0 }, 500, 'in'); c.mood('open');
        c.to({ ry: 5.6, legF: -38, kneeF: 74, legB: -34, kneeB: 76, lean: 14, ...REST }, 380); c.to({ ...STAND, ...REST }, 360);
      } },
    selfie: { weight: 1, props: () => [{ id: 'bphone', art: 'phone', layer: 'front', o: 0 }, { id: 'bclic', art: 'bubble', layer: 'front', text: 'CLIC!', tone: 'pow', o: 0 }],
      play(c) {
        c.to({ armF: -128, elbF: -20 }, 320);
        const hand = c.point('handF'); c.set({ bphone: { x: hand[0], y: hand[1] - 2 }, 'bphone.pop': 0.3 }); c.attach('bphone', 'handF');
        c.to({ 'bphone.o': 1, 'bphone.pop': 1 }, 200, 'back');
        c.look(3, -1.6, 200); c.mood('happy'); c.to({ armB: -150, elbB: -40, head: -6 }, 260); c.wait(300);
        c.set({ 'bphone.flash': 1 }); c.par(a => a.to({ 'bphone.flash': 0 }, 300), a => a.say('bclic', 380, { on: 'bphone', dx: 0, dy: -6 }));
        c.reach('F', c.point('root', [9, -32]), 360); c.look(3, 1, 160); c.to({ head: 6 }, 200); c.wait(400); c.nod(1);
        c.to({ 'bphone.o': 0, 'bphone.pop': 0.3 }, 200); c.detach('bphone'); c.rest(300); c.to({ head: 0 }, 160); c.mood('open'); c.look(2.4, 0);
      } },
    sneeze: { weight: 1, props: () => [{ id: 'bachoo', art: 'bubble', layer: 'front', text: 'ATCHIM!', tone: 'pow', o: 0, angle: -10 }, { id: 'bspray', art: 'spray', layer: 'front', o: 0 }],
      play(c) {
        const face = Math.sign(c.cur('face')) || 1;
        c.to({ head: -12, lean: -6 }, 380, 'in'); c.mood('closed'); c.wait(160);
        c.to({ head: 12, lean: 14 }, 110, 'out');
        const [x, y] = c.point('head', [8, -8]);
        c.set({ bspray: { x, y, sx: face } }); c.tw({ 'bspray.o': 1 }, 80);
        c.path('bspray', k => ({ x: x + face * 16 * k, y: y + 6 * k }), 700, 'out'); c.tw({ 'bspray.o': 0 }, 400, 'in', c.t + 300);
        c.say('bachoo', 520, { dx: 10, dy: -17 }); c.mood('wide'); c.to({ head: 0, lean: 0 }, 300); c.mood('happy'); c.wait(240); c.mood('open');
      } },
    cookie: { weight: 1, props: r => [{ id: 'bcookie', art: 'cookie', layer: 'front', o: 0 }, { id: 'bnhac', art: 'bubble', layer: 'front', text: 'NHAC!', tone: 'pow', o: 0 },
      { id: 'byum', art: 'bubble', layer: 'front', text: r.pick(['COOKIE ACEITO!', 'AI QUE DELÍCIA!']), o: 0 }],
      play(c) {
        c.to({ armF: -62, elbF: -40 }, 300);
        const hand = c.point('handF'); c.set({ bcookie: { x: hand[0], y: hand[1] - 1.6 }, 'bcookie.pop': 0.3 }); c.attach('bcookie', 'handF');
        c.to({ 'bcookie.o': 1, 'bcookie.pop': 1 }, 240, 'back'); c.look(3, 1, 160); c.wait(200);
        c.reach('F', c.point('root', [6, -34]), 420); c.set({ 'bcookie.bite': 1 }); c.say('bnhac', 380, { dx: 9, dy: -18 });
        c.reach('F', c.point('root', [10, -25]), 360); c.mood('happy'); c.say('byum', 640);
        c.to({ 'bcookie.o': 0, 'bcookie.pop': 0.3 }, 200); c.detach('bcookie'); c.rest(280); c.mood('open'); c.look(2.4, 0);
      } },
    doze: { weight: 1, props: () => [{ id: 'bzzz', art: 'zzz', layer: 'front', o: 0 }, { id: 'boxe', art: 'bubble', layer: 'front', text: 'OXE!', tone: 'pow', o: 0 }],
      play(c) {
        c.mood('closed'); c.tw({ bulb: 0.3 }, 500); c.to({ head: 14, lean: 6, armF: 6, armB: -2 }, 900, 'in');
        const [x, y] = c.point('head', [8, -17]); c.set({ bzzz: { x, y } }); c.attach('bzzz', 'head'); c.tw({ 'bzzz.o': 1 }, 300);
        for (let i = 0; i < 2; i++) c.to({ lean: 9, head: 17 }, 700).to({ lean: 6, head: 14 }, 700);
        c.to({ 'bzzz.o': 0 }, 160); c.detach('bzzz'); c.mood('wide'); c.set({ bulb: 1 });
        c.to({ head: -4, lean: -3 }, 140, 'out'); c.say('boxe', 420, { dx: -8, dy: -17 }); c.to({ head: 0, lean: 0, ...REST }, 300); c.mood('open');
      } },
    scanself: { weight: 1, props: r => [{ id: 'bscan', art: 'scanbeam', layer: 'front', o: 0 }, { id: 'bclean', art: 'bubble', layer: 'front', text: r.pick(['LIMPO, MACHO!', 'SEM VÍRUS!']), o: 0 }],
      play(c) {
        const x = c.cur('rx');
        c.look(0, -1.6, 200); c.to({ armF: 12, armB: -12, elbF: -6, elbB: -6 }, 300);
        c.set({ bscan: { x, y: -62, sx: 0.55 } }); c.to({ 'bscan.o': 1 }, 160);
        c.path('bscan', k => ({ y: -62 + 62 * k }), 1100, 'io'); c.look(0, 1.6, 900); c.wait(240);
        c.to({ 'bscan.o': 0 }, 160); c.mood('happy'); c.say('bclean', 560); c.mood('open'); c.rest(220); c.look(2.4, 0);
      } }
  };

  // Ambient extras: each plays in exactly `length` ms from the current time and only
  // touches its own props; they always start and end outside the window or invisible.
  const sideOf = r => r.chance(0.5) ? 1 : -1;
  const EXTRAS = {
    roomba: { weight: 3, length: 6200, props: () => [{ id: 'xroomba', art: 'roomba', x: -30, y: 22, layer: 'front' }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -30 : 230, mid = r.range(80, 120), start = c.t;
        c.set({ xroomba: { x: from, y: 22, sx: 1.25 * dir, sy: 1.25 } });
        c.tw({ xroomba: { x: mid } }, 2600, 'out'); c.at(start + 3400);
        c.tw({ xroomba: { x: 200 - from } }, 2800, 'in');
      } },
    drone: { weight: 2, length: 4200, props: () => [{ id: 'xdrone', art: 'drone', x: -30, y: -84 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -30 : 230, y = r.range(-92, -76);
        c.path('xdrone', k => ({ x: from + (200 - 2 * from) * k, y: y + 2.4 * Math.sin(k * Math.PI * 6), r: 6 * dir, sx: 0.8, sy: 0.8 }), 4200, 'linear');
      } },
    bug: { weight: 2, length: 6000, props: () => [{ id: 'xbug', art: 'bug', x: -20, y: 16, layer: 'front' }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -20 : 220, a = from + dir * r.range(80, 100), b = a + dir * r.range(30, 50), start = c.t;
        c.set({ xbug: { x: from, y: 16, sx: 1.3 * dir, sy: 1.3 } });
        c.tw({ xbug: { x: a } }, 1800, 'io'); c.at(start + 2400); c.tw({ xbug: { x: b } }, 900, 'io'); c.at(start + 3700);
        c.tw({ xbug: { x: 200 - from } }, 2300, 'io');
      } },
    bits: { weight: 2, length: 2800, props: () => [{ id: 'xbits', art: 'bits', x: 100, o: 0 }],
      play(c, r, k) {
        const x = r.range(46, 154);
        c.set({ xbits: { x, y: -2 } }); c.tw({ 'xbits.o': 1 }, 400);
        c.path('xbits', k => ({ x, y: -2 - 30 * k }), 2800, 'out'); c.tw({ 'xbits.o': 0 }, 900, 'in', c.t + 1900);
      } },
    mouse: { weight: 2, length: 4200, props: () => [{ id: 'xmouse', art: 'mouse', x: -24, y: 20, layer: 'front' }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -24 : 224, a = from + dir * r.range(90, 120), start = c.t;
        c.set({ xmouse: { x: from, y: 20, sx: 1.3 * dir, sy: 1.3 } });
        c.tw({ xmouse: { x: a } }, 1300, 'out'); c.at(start + 1500);
        c.tw({ xmouse: { r: -8 * dir } }, 200).tw({ xmouse: { r: 0 } }, 200, 'io', start + 1900); c.at(start + 2700);
        c.tw({ xmouse: { x: 200 - from } }, 1500, 'in');
      } },
    sat: { weight: 2, length: 8000, props: () => [{ id: 'xsat', art: 'sat', x: -30, y: -96 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -30 : 230;
        c.path('xsat', k => ({ x: from + (200 - 2 * from) * k, y: -96 + 4 * k, r: 8 * k, sx: 0.8, sy: 0.8 }), 8000, 'linear');
      } },
    floppy: { weight: 2, length: 2400, props: () => [{ id: 'xfloppy', art: 'floppy', x: -20, y: -40 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -20 : 220, y = r.range(-58, -40);
        c.path('xfloppy', k => ({ x: from + (200 - 2 * from) * k, y: y - 14 * Math.sin(Math.PI * k), r: 720 * k * dir }), 2400, 'linear');
      } },
    cursor: { weight: 2, length: 3400, props: () => [{ id: 'xcursor', art: 'cursor', x: 220, y: -120, layer: 'front' }],
      play(c, r, k) {
        const tx = r.range(70, 130), ty = r.range(-60, -36), start = c.t;
        c.set({ xcursor: { x: 230, y: -130 } });
        c.path('xcursor', k => ({ x: 230 + (tx - 230) * k, y: -130 + (ty + 130) * k }), 1100, 'io'); c.at(start + 1300);
        c.set({ 'xcursor.ring': 1 }); c.tw({ 'xcursor.ring': 0 }, 500, 'out');
        c.to({ xcursor: { sx: 0.85, sy: 0.85 } }, 90).to({ xcursor: { sx: 1, sy: 1 } }, 160);
        c.at(start + 2300); c.path('xcursor', k => ({ x: tx + (-30 - tx) * k, y: ty + (-130 - ty) * k }), 1100, 'in');
      } },
    catbot: { weight: 2, length: 7200, props: () => [{ id: 'xcat', art: 'catbot', x: -24, y: -1 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -24 : 224, mid = r.range(70, 130), start = c.t;
        c.set({ xcat: { x: from, y: -1, sx: 0.95 * dir, sy: 0.95 } });
        c.tw({ xcat: { x: mid } }, 2800, 'io'); c.at(start + 4200);
        c.tw({ xcat: { x: 200 - from } }, 3000, 'io');
      } },
    cableball: { weight: 2, length: 3600, props: () => [{ id: 'xcable', art: 'cableball', x: -20, y: -4 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -20 : 220;
        c.path('xcable', k => ({ x: from + (200 - 2 * from) * k, y: -4.2 - 9 * Math.abs(Math.sin(k * Math.PI * 4)) * (1 - k * 0.5), r: 900 * k * dir }), 3600, 'linear');
      } },
    packets: { weight: 2, length: 4400, props: () => [{ id: 'xpack', art: 'packets', x: -30, y: -1 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -30 : 230;
        c.set({ xpack: { x: from, y: -1, sx: 0.8, sy: 0.8 } }); c.tw({ xpack: { x: 200 - from } }, 4400, 'linear');
      } },
    comet: { weight: 2, length: 1000, props: () => [{ id: 'xcomet', art: 'comet', x: -30, y: -96 }],
      play(c, r, k) {
        const y = r.range(-100, -84);
        c.path('xcomet', k => ({ x: -30 + 260 * k, y: y + 14 * k, r: 3 }), 1000, 'in');
      } },
    cloud: { weight: 2, length: 9000, props: () => [{ id: 'xcloud', art: 'cloud', x: -30, y: -88 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -30 : 230;
        c.set({ xcloud: { x: from, y: r.range(-94, -84) } }); c.tw({ xcloud: { x: 200 - from } }, 9000, 'linear');
      } },
    forkbot: { weight: 2, length: 7000, props: () => [{ id: 'xfork', art: 'forkbot', x: -30, y: -1 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -30 : 230, mid = r.range(80, 120), start = c.t;
        c.set({ xfork: { x: from, y: -1, sx: 0.85 * dir, sy: 0.85 } });
        c.tw({ xfork: { x: mid } }, 2800, 'out'); c.at(start + 3600); c.tw({ xfork: { x: 200 - from } }, 3400, 'in');
      } },
    spinner: { weight: 1, length: 3000, props: () => [{ id: 'xspin', art: 'spinner', x: -20, y: 0 }],
      play(c, r, k) {
        const dir = sideOf(r), from = dir > 0 ? -20 : 220;
        c.path('xspin', k => ({ x: from + (200 - 2 * from) * k, y: 0, r: 0 }), 3000, 'linear');
      } }
  };

  const pickWeighted = (r, table, ok = () => true) => {
    const names = Object.keys(table).filter(name => ok(table[name], name));
    let ticket = r.next() * names.reduce((sum, name) => sum + (table[name].weight || 1), 0);
    return names.find(name => (ticket -= table[name].weight || 1) < 0) || names.at(-1);
  };
  const propsOf = (piece, r) => typeof piece.props === 'function' ? piece.props(r) : piece.props || [];
  function choosePlan(skit, r, parts) {
    const composed = !!skit.compose;
    const arrive = composed ? parts.arrive ?? pickWeighted(r, MOVES, m => m.arrive && (!skit.carryIn || m.carry)) : null;
    const leave = composed ? parts.leave ?? pickWeighted(r, MOVES, m => m.leave && (!skit.carryOut || m.carry)) : null;
    const deliver = composed ? parts.deliver ?? r.pick(skit.deliver || Object.keys(DELIVER)) : null;
    const brk = parts.brk !== undefined ? parts.brk : skit.pauses === false ? null : r.chance(composed ? 0.55 : 0.4) ? pickWeighted(r, BREAKS) : null;
    let extras = parts.extras;
    if (!extras) {
      extras = [];
      const count = r.chance(0.6) ? (r.chance(0.3) ? 2 : 1) : 0;
      while (extras.length < count) extras.push(pickWeighted(r, EXTRAS, (x, name) => !extras.includes(name)));
    }
    return { arrive, leave, deliver, brk, extras };
  }
  // The skit's own props, then the hidden starting state of its set, then every piece's props.
  function planProps(own, plan, r) {
    const homes = {};
    const list = own.map(p => {
      if (!p.set) return p;
      homes[p.id] = { x: p.x ?? 0, y: p.y ?? 0 };
      return plan.deliver ? { ...p, ...DELIVER[plan.deliver].hide(p) } : p;
    });
    const add = items => { for (const p of items) if (!list.some(q => q.id === p.id)) list.push(p); };
    if (plan.deliver) add([{ id: 'scan', art: 'scanbeam', layer: 'front', o: 0 }, { id: 'dthud', art: 'bubble', layer: 'front', text: 'TUM!', tone: 'pow', o: 0 }]);
    for (const name of [plan.arrive, plan.leave]) if (name) add(propsOf(MOVES[name], r));
    if (plan.brk) add(propsOf(BREAKS[plan.brk], r));
    for (const name of plan.extras) add(propsOf(EXTRAS[name], r));
    return { props: list, homes };
  }
  function kit(c, r, plan, homes) {
    let pending = !!plan.brk;
    const k = {
      plan,
      home: id => homes[id],
      // A secret door opens on the side of the room the set leaves free.
      doorX() {
        const xs = Object.values(homes).map(h => h.x);
        const mean = xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 100;
        return mean > 100 ? 40 : 160;
      },
      arrive(x, { carry = [] } = {}) { MOVES[plan.arrive].arrive(c, r, x, carry, k); faceRight(c); return k; },
      leave({ carry = [] } = {}) { if (pending) k.pause(true); MOVES[plan.leave].leave(c, r, carry, k); return k; },
      // Natural pause points; the planned break (if any) happens at one of them.
      pause(force = false) {
        if (!pending || (!force && !r.chance(0.5))) return k;
        pending = false; BREAKS[plan.brk].play(c, r); return k;
      },
      bring(ids) { DELIVER[plan.deliver].bring(c, ids, homes); return k; },
      clear(ids) { DELIVER[plan.deliver].clear(c, ids, homes); return k; },
      extras() {
        const total = c.t;
        for (const name of plan.extras) {
          const extra = EXTRAS[name];
          if (total < extra.length + 1000) continue;
          c.at(r.range(400, total - extra.length - 400)); extra.play(c, r);
        }
        c.at(total);
      }
    };
    return k;
  }

  const SKITS = {
    greet: { kind: 'fun', opener: true, weight: 3,
      props: r => [{ id: 'hi', art: 'bubble', layer: 'front', text: r.pick(['E AÍ, MACHO VÉI?', 'EI, MÁ, BLZ?', 'FALA, MACHO!', 'EI, BICHO, BLZ?']), o: 0 }],
      play(c, r, k) {
        c.walk(r.range(84, 104));
        c.wait(160); c.look(0, -0.4, 220); c.mood('happy');
        c.par(a => a.wave(3), a => { a.wait(140); a.say('hi', 900); });
        c.rest(300); c.mood('open'); c.look(2.4, 0, 200); c.wait(200);
        k.pause(true);
        c.walk(236);
      } },
    peek: { kind: 'fun', opener: true, weight: 2,
      props: r => [{ id: 'bang', art: 'bubble', layer: 'front', text: r.pick(['VISH!', 'OXE!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
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
      play(c, r, k) {
        c.walk(r.range(86, 112));
        c.mood('closed');
        c.to({ armF: -172, elbF: -6, armB: -166, elbB: -10, head: -8 }, 560, 'io');
        c.to({ lean: -10 }, 460); c.to({ lean: 12 }, 560); c.to({ lean: 0 }, 380);
        c.wait(520);
        c.rest(400); c.mood('happy'); c.wait(320); c.mood('open');
        k.pause(true);
        c.walk(236);
      } },
    coffee: { kind: 'fun', weight: 3, minElapsedMs: 15000,
      props: r => [{ id: 'cart', art: 'cart', x: CART_X }, { id: 'mug', art: 'mug', ...onCart(5.7), layer: 'front' }, { id: 'mach', art: 'machine', ...onCart(13) },
        { id: 'love', art: 'bubble', layer: 'front', text: r.pick(['SÓ O FILÉ, OH!', 'GOSTOSO, OH!']), o: 0 },
        { id: 'slurp', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA!', 'ARRIÉGUA!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
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
      play(c, r, k) {
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
      play(c, r, k) {
        c.set({ rx: CART_X - BEHIND });
        for (const id of ['s0', 's1', 's2', 'inbox', 'outbox']) c.attach(id, 'cart');
        cartIn(c, 'cart', r.range(46, 56));
        k.pause();
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
      play(c, r, k) {
        const grip = [-12.8, -27];
        c.set({ rx: -36 }); c.attach('fold', 'cab');
        c.grip('cab', grip, { lean: 6 });
        c.haul(['cab'], r.range(62, 80), { speed: 30 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        const grip = [-19.6, -26];
        c.set({ rx: CART_X - BEHIND });
        c.grip('board', grip, { lean: 6 });
        c.haul(['board'], r.range(36, 50), { speed: 30 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        const grip = [-14, -20.4];
        c.set({ rx: -42 }); c.attach('lens', 'stand');
        c.grip('stand', grip, { lean: 6 });
        c.haul(['stand'], r.range(56, 74), { speed: 30 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        c.set({ rx: CART_X - BEHIND }); c.attach('page', 'cart'); c.attach('prn', 'cart');
        cartIn(c, 'cart', r.range(50, 60));
        k.pause();
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
      play(c, r, k) {
        c.set({ rx: CART_X - BEHIND });
        for (const id of ['frame', 'k1', 'k2', 'k3', 'basket']) c.attach(id, 'cart');
        cartIn(c, 'cart', r.range(46, 56));
        k.pause();
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
      play(c, r, k) {
        const grip = [-11.4, -27];
        c.set({ rx: -36 });
        c.grip('term', grip, { lean: 6 });
        c.haul(['term'], r.range(66, 82), { speed: 32 });
        c.rest(280); k.pause();
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
      play(c, r, k) {
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
      play(c, r, k) {
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
        k.pause(true);
        c.walk(240, { speed: 40 });
      } },
    music: { kind: 'fun', weight: 2, minElapsedMs: 20000,
      props: [{ id: 'radio', art: 'radio', layer: 'front' }],
      play(c, r, k) {
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
        k.pause();
        c.par(a => a.crouch(3), a => a.reachAt('F', 'radio', [-2, -9.6], 360, 'io', { with: { lean: 22 } })); c.set({ 'radio.notes': 0 });
        c.reachAt('F', 'radio', [0, -11.6], 200, 'io', { with: { lean: 22 } }); c.grab('radio');
        c.par(a => a.crouch(0, 400), a => a.to({ lean: 0, armF: -40, elbF: -42 }, 400));
        c.walk(240, { speed: 34 });
      } },
    skate: { kind: 'fun', opener: true, weight: 2,
      props: [{ id: 'deck', art: 'skate', x: -45 }],
      play(c, r, k) {
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
      play(c, r, k) {
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
      play(c, r, k) {
        const grip = [-16.6, -16.8];
        c.set({ rx: -46 }); c.attach('can', 'plant');
        c.grip('plant', grip, { lean: 14 });
        c.haul(['plant'], r.range(52, 70), { speed: 28 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        c.set({ rx: -42, armF: -70, elbF: -40, armB: -60, elbB: -46, lean: -4 });
        const hand = c.point('handF');
        c.set({ box: { x: hand[0] + 4, y: hand[1] + 7 } }); c.attach('box', 'handF');
        const mid = r.range(82, 104);
        const steps = Math.floor(Math.abs(mid + 42) / 26 * 1000 / 640);
        c.par(a => a.walk(mid, { arms: false, speed: 26 }), a => { for (let i = 0; i < steps; i++) a.to({ 'box.tilt': 4 }, 320).to({ 'box.tilt': -4 }, 320); a.to({ 'box.tilt': 0 }, 200); });
        c.to({ lean: -12, head: -10 }, 320); c.look(0, -1.6); c.mood('happy'); c.wait(500); c.to({ lean: -4, head: 0 }, 300); c.mood('open'); c.look(2.4, 0);
        c.walk(244, { arms: false, speed: 30 });
      } },
    // ---------------------------------------------------------------- composed activities
    duck: { kind: 'work', families: ['verification', 'reading', 'calculation'], weight: 4, compose: true,
      props: r => [{ id: 'stool', art: 'stool', x: 114, set: true }, { id: 'duck', art: 'duck', x: 113, y: -10, set: true },
        { id: 'bla', art: 'bubble', layer: 'front', text: r.pick(['ENTÃO, MACHO…', 'É O SEGUINTE…']), o: 0 },
        { id: 'quac', art: 'bubble', layer: 'front', text: 'QUAC!', tone: 'pow', o: 0 },
        { id: 'got', art: 'bubble', layer: 'front', text: r.pick(['AH, ENTENDI!', 'ÉGUA, ERA ISSO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(82); k.bring(['stool', 'duck']);
        c.walk(94, { speed: 20 }); c.look(3.4, 0.6);
        c.say('bla', 700);
        for (let i = 0; i < 4; i++) c.to({ armF: i % 2 ? -70 : -104, elbF: i % 2 ? -60 : -30, head: i % 2 ? 4 : -2 }, 280, 'out');
        c.rest(260); c.nod(1);
        k.pause();
        c.par(a => a.to({ 'duck.pop': 1.18 }, 120, 'out').to({ 'duck.pop': 1 }, 220, 'back'), a => a.say('quac', 560, { on: 'duck', dx: 2, dy: -12 }));
        c.mood('wide'); c.wait(300); c.mood('happy'); c.say('got', 640); c.hop(4); c.mood('open'); c.rest(200);
        c.walk(80, { face: false, speed: 22 });
        k.clear(['stool', 'duck']); k.leave();
      } },
    // The USB plug that never goes in the first time (nor the second).
    usb: { kind: 'work', families: ['access', 'restoration', 'checkpoint'], weight: 4, compose: true,
      props: r => [{ id: 'pc', art: 'tower', x: 118, set: true }, { id: 'stick', art: 'usbstick', layer: 'front', o: 0 },
        { id: 'no1', art: 'bubble', layer: 'front', text: r.pick(['OXE!', 'VISH!']), tone: 'pow', o: 0 },
        { id: 'no2', art: 'bubble', layer: 'front', text: r.pick(['DE NOVO?!', 'AFE, MARIA!']), tone: 'hot', o: 0 },
        { id: 'tec', art: 'bubble', layer: 'front', text: 'TÉC!', tone: 'pow', o: 0 },
        { id: 'yes', art: 'bubble', layer: 'front', text: r.pick(['ARRIÉGUA, ENTROU!', 'ENTROU, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['pc']);
        c.to({ armF: -66, elbF: -50 }, 240);
        const hand = c.point('handF'); c.set({ stick: { x: hand[0] - 0.6, y: hand[1] }, 'stick.pop': 0.3 }); c.attach('stick', 'handF');
        c.to({ 'stick.o': 1, 'stick.pop': 1 }, 240, 'back'); c.look(3, 0.8);
        const port = [110.8, -15.8], near = dx => [port[0] - 8 - dx, port[1]];
        c.walk(port[0] - 13, { speed: 20 });
        const attempt = (fail, say) => {
          c.place('stick', 'F', near(4), 320, 'io', { with: { lean: 6 } });
          c.place('stick', 'F', near(0.6), 180, 'in', { with: { lean: 7 } });
          if (fail) { c.place('stick', 'F', near(3), 160, 'out', { with: { lean: 4 } }); c.mood('wide'); c.say(say, 460); c.mood('open'); }
        };
        const inspect = flip => { c.reach('F', c.point('root', [9, -33]), 300); c.look(3, 1.2, 160); c.to({ head: 8 }, 200); c.to({ 'stick.flip': flip }, 280); c.to({ head: 0 }, 160); };
        attempt(true, 'no1'); inspect(-1);
        attempt(true, 'no2'); inspect(1);
        attempt(false);
        c.place('stick', 'F', near(-1.6), 200, 'in', { with: { lean: 7 } }); c.drop('stick', { onto: 'pc' });
        c.set({ 'pc.bad': 0 }); c.to({ 'pc.ok': 1 }, 120);
        c.par(a => a.to({ 'pc.pop': 1.05 }, 100, 'out').to({ 'pc.pop': 1 }, 200, 'back'), a => a.say('tec', 360, { on: 'pc', dx: -4, dy: -32 }));
        c.rest(260); c.walk(port[0] - 22, { face: false, speed: 22 }); c.mood('happy'); c.hop(5); c.say('yes', 640); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['pc', 'stick']); k.leave();
      } },
    ram: { kind: 'work', families: ['restoration', 'checkpoint', 'calculation'], weight: 4, compose: true,
      props: r => [{ id: 'board', art: 'mobo', x: 120, set: true }, { id: 'stick', art: 'ramstick', layer: 'front', o: 0 },
        { id: 'tec', art: 'bubble', layer: 'front', text: 'TÉC!', tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['RODOU, MACHO!', 'MAIS MEMÓRIA!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(78); k.bring(['board']);
        c.to({ armF: -66, elbF: -50 }, 240);
        const hand = c.point('handF'); c.set({ stick: { x: hand[0] + 3, y: hand[1] + 1.6 }, 'stick.pop': 0.3 }); c.attach('stick', 'handF');
        c.to({ 'stick.o': 1, 'stick.pop': 1 }, 240, 'back');
        c.look(3, 0.6); c.reach('F', c.point('root', [9, -33]), 300); c.to({ head: 6 }, 200); c.wait(300); c.to({ head: 0 }, 160);
        const slot = [127.5, -33.1];
        c.walk(slot[0] - 12, { speed: 20 });
        c.place('stick', 'F', [slot[0], slot[1] - 4], 380, 'io', { with: { lean: 4 } });
        for (const dx of [-1, 1, 0]) c.place('stick', 'F', [slot[0] + dx, slot[1] - 3], 140, 'io', { with: { lean: 4 } });
        c.place('stick', 'F', slot, 260, 'in', { with: { lean: 5 } }); c.drop('stick', { onto: 'board' });
        c.par(a => a.to({ 'board.pop': 1.04 }, 90, 'out').to({ 'board.pop': 1 }, 200, 'back'), a => a.say('tec', 360, { on: 'board', dx: 8, dy: -46 }));
        c.rest(260); c.walk(slot[0] - 28, { face: false, speed: 22 });
        for (const v of [1, 0.2, 1, 0.5, 1]) c.to({ 'board.live': v }, 80, 'linear');
        c.look(3.4, -0.6); c.mood('happy'); c.say('ok', 620); c.nod(1); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['board', 'stick']); k.leave();
      } },
    cables: { kind: 'work', families: ['access', 'checkpoint', 'restoration'], weight: 4, compose: true,
      props: r => [{ id: 'panel', art: 'patch', x: 134, set: true },
        ...[1, 2, 3].map(i => ({ id: `p${i}`, art: 'plug', tone: i, x: 104.6 + i * 3.2, y: -14.4, set: true })),
        { id: 'crate', art: 'box', x: 111, set: true },
        { id: 'zap', art: 'bubble', layer: 'front', text: r.pick(['VISH!', 'ÉGUA!']), tone: 'hot', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['TÁ LIGADO!', 'CONECTADO, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['panel', 'p1', 'p2', 'p3', 'crate']);
        const ports = [-8, 0, 8].map(dx => [134 + dx, -31.5]);
        [1, 2, 3].forEach((i, n) => {
          const id = `p${i}`;
          c.walk(c.pose(id).x - 10, { speed: 28 }); c.turn(1);
          c.par(a => a.crouch(2, 220), a => a.reachAt('F', id, [0, -1], 220, 'io', { with: { lean: 10 } })); c.grab(id);
          c.par(a => a.crouch(0, 260), a => a.reach('F', a.point('root', [10, -30]), 260));
          c.walk(ports[n][0] - 11, { speed: 28 });
          c.place(id, 'F', [ports[n][0], ports[n][1] - 3], 260, 'io', { with: { lean: 4 } });
          c.place(id, 'F', ports[n], 160, 'in', { with: { lean: 5 } }); c.drop(id, { onto: 'panel' });
          c.set({ [`panel.l${n}`]: 1 });
          if (n === 2) {
            c.set({ 'panel.spark': 1 }); c.mood('wide'); c.to({ head: -10, lean: -8, ry: -2 }, 90, 'out');
            c.say('zap', 420, { dx: 6, dy: -20 }); c.to({ head: 0, lean: 0, ry: 0, 'panel.spark': 0 }, 260); c.mood('open');
          }
          c.rest(180);
        });
        c.walk(106, { face: false, speed: 26 }); c.turn(1); c.look(3.4, -0.6); c.mood('happy'); c.say('ok', 620); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['panel', 'p1', 'p2', 'p3', 'crate']); k.leave();
      } },
    cooling: { kind: 'work', families: ['calculation', 'verification', 'checkpoint'], weight: 4, compose: true,
      props: r => [{ id: 'cpu', art: 'hotchip', x: 126, set: true }, { id: 'fan', art: 'deskfan', x: 102, set: true },
        { id: 'hot', art: 'bubble', layer: 'front', text: r.pick(['TÁ QUENTE, MACHO!', 'VISH, FERVENDO!']), tone: 'hot', o: 0 },
        { id: 'ai', art: 'bubble', layer: 'front', text: 'AI!', tone: 'hot', o: 0 },
        { id: 'ah', art: 'bubble', layer: 'front', text: r.pick(['AH, FRESQUIM!', 'QUE VENTINHO BOM!']), o: 0 }],
      play(c, r, k) {
        k.arrive(84); k.bring(['cpu']);
        c.walk(110, { speed: 20 }); c.look(3.4, 0.6); c.mood('wide'); c.say('hot', 600); c.mood('open');
        c.reachAt('F', 'cpu', [-8.4, -18], 380, 'io', { with: { lean: 8 } });
        c.to({ armF: -150, elbF: -30, lean: -8, ry: -2 }, 110, 'out'); c.say('ai', 360, { dx: 8, dy: -18 });
        c.to({ ry: 0, lean: 0 }, 160); c.to({ elbF: -60 }, 120).to({ elbF: -20 }, 120).to({ elbF: -60 }, 120); c.rest(220);
        c.to({ lean: 14, head: 8 }, 260); c.to({ head: 12 }, 200).to({ head: 6 }, 240); c.to({ lean: 0, head: 0 }, 220);
        c.walk(86, { face: false, speed: 26 }); c.turn(1);
        k.bring(['fan']);
        c.walk(92, { speed: 20 }); c.reachAt('F', 'fan', [-3, -21], 300, 'io', { with: { lean: 6 } }); c.to({ elbF: c.cur('elbF') - 6 }, 100); c.rest(240);
        const start = c.t;
        c.tw({ 'fan.spin': 720 }, 1600, 'in', start); c.tw({ 'fan.spin': 720 + 360 * 12 }, 3600, 'linear', start + 1600); c.tw({ 'fan.air': 1 }, 400, 'io', start + 500);
        c.tw({ 'cpu.heat': 0 }, 1800, 'io', start + 900); c.tw({ 'cpu.cool': 1 }, 1600, 'io', start + 1400);
        c.walk(114, { speed: 20 }); c.turn(-1);
        c.mood('closed'); c.to({ armF: -120, elbF: -10, armB: -110, elbB: -14, head: -6 }, 400); c.say('ah', 700); c.wait(300);
        c.rest(300); c.mood('open'); c.turn(1);
        c.at(Math.max(c.t, start + 5200));
        c.tw({ 'fan.spin': 720 + 360 * 13 }, 900, 'out'); c.tw({ 'fan.air': 0 }, 400);
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['cpu', 'fan']); k.leave();
      } },
    crt: { kind: 'work', families: ['reading', 'verification', 'restoration'], weight: 4, compose: true,
      props: r => [{ id: 'tv', art: 'crt', x: 120, set: true },
        { id: 'chuv', art: 'bubble', layer: 'front', text: r.pick(['CHUVISCO, MACHO!', 'OXE, CADÊ A IMAGEM?']), o: 0 },
        { id: 'pa', art: 'bubble', layer: 'front', text: 'PÁ!', tone: 'pow', o: 0 },
        { id: 'pa2', art: 'bubble', layer: 'front', text: 'PÁ-PÁ!', tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['AGORA FOI!', 'PEGOU, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(84); k.bring(['tv']);
        c.walk(98, { speed: 20 }); c.look(3.4, -0.6); c.to({ head: 8 }, 200); c.say('chuv', 700); c.to({ head: 0 }, 160);
        const side = [107.4, -26];
        const whack = (big, say) => {
          c.reach('F', [side[0] - 8, side[1] - 8], 260, 'io', { with: { lean: -4 } });
          c.reach('F', side, 110, 'in', { with: { lean: 10 } });
          c.par(a => a.to({ 'tv.tilt': big ? 4 : 2 }, 80, 'out').to({ 'tv.tilt': 0 }, 260, 'back'), a => a.say(say, 340, { on: 'tv', dx: -10, dy: -40 }));
        };
        whack(false, 'pa');
        for (const [n, p] of [[0.2, 0.8], [1, 0], [0.4, 0.6], [1, 0]]) c.to({ 'tv.noise': n, 'tv.pic': p }, 90, 'linear');
        c.rest(240); c.look(3.4, -0.6); c.to({ head: 10 }, 200).to({ head: 0 }, 200);
        k.pause();
        c.hop(3, 300); whack(true, 'pa2');
        c.to({ 'tv.noise': 0, 'tv.pic': 1 }, 220);
        c.rest(240); c.walk(90, { face: false, speed: 22 }); c.turn(1); c.mood('happy');
        for (let i = 0; i < 2; i++) c.to({ armF: -60, elbF: -60, armB: -50, elbB: -70 }, 120).to({ armF: -50, elbF: -70, armB: -60, elbB: -60 }, 120);
        c.say('ok', 620); c.rest(240); c.mood('open');
        c.walk(80, { face: false, speed: 22 });
        k.clear(['tv']); k.leave();
      } },
    restart: { kind: 'work', families: ['restoration', 'access', 'verification'], weight: 4, compose: true,
      props: r => [{ id: 'mon', art: 'screen2', x: 126, set: true }, { id: 'btn', art: 'powerbtn', x: 104, set: true },
        { id: 'afe', art: 'bubble', layer: 'front', text: r.pick(['DE NOVO, MACHO?', 'AFE, TRAVOU!']), tone: 'hot', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['DESLIGA E LIGA!', 'RESOLVIDO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['mon', 'btn']);
        c.look(3.4, -0.8); c.mood('wide'); c.wait(300);
        c.to({ armF: -150, elbF: -116, head: 10, lean: 4 }, 320); c.mood('closed'); c.say('afe', 640); c.rest(300); c.to({ head: 0 }, 160); c.mood('open');
        const press = () => {
          c.reachAt('F', 'btn', [0, -16.6], 300, 'io', { with: { lean: 14 } });
          c.to({ 'btn.press': 1.2 }, 100, 'in'); c.to({ 'btn.press': 0 }, 160, 'out'); c.rest(240);
        };
        c.walk(96, { speed: 20 }); press(); c.to({ 'mon.err': 0 }, 120);
        c.look(3.4, -0.8);
        for (let i = 0; i < 3; i++) c.to({ legF: -14, kneeF: 18 }, 150, 'out').to({ legF: 0, kneeF: 0 }, 150, 'in');
        k.pause();
        press(); c.to({ 'mon.boot': 1 }, 200); c.to({ 'mon.bar': 1 }, 1400, 'io'); c.to({ 'mon.boot': 0, 'mon.ok': 1 }, 200);
        c.walk(88, { face: false, speed: 22 }); c.turn(1); c.look(3.4, -0.8); c.mood('happy');
        c.to({ armF: -96, elbF: -86 }, 260); c.say('ok', 640); c.rest(260); c.mood('open');
        c.walk(80, { face: false, speed: 22 });
        k.clear(['mon', 'btn']); k.leave();
      } },
    laserfix: { kind: 'work', families: ['restoration', 'verification'], weight: 4, compose: true,
      props: r => [{ id: 'chip', art: 'circuit', x: 122, set: true }, { id: 'beam', art: 'laser', layer: 'front', o: 0 }, { id: 'spark', art: 'spark', layer: 'front', o: 0 },
        { id: 'hmm', art: 'bubble', layer: 'front', text: r.pick(['RACHOU, VISH…', 'TÁ QUEBRADO…']), o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['TÁ NOS TRINQUE!', 'SOLDADINHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['chip']);
        c.walk(94, { speed: 20 }); c.look(3.4, -0.4); c.to({ head: 6 }, 200); c.say('hmm', 640); c.to({ head: 0 }, 160);
        k.pause();
        // Either the visor or the palm fires the welding laser along the crack.
        const eyes = r.chance(0.5);
        const crack = [[-4, -28], [-1, -25], [-3, -22.4], [1, -20.4], [-0.6, -18]].map(([x, y]) => [122 + x, y]);
        let from;
        if (eyes) { c.mood('wide'); c.to({ head: 10 }, 260); from = c.point('head', [4.6, -8.8]); }
        else { c.reach('F', c.point('root', [11, -30]), 320, 'io'); from = c.point('handF'); }
        const at = q => {
          const s = Math.min(crack.length - 1.001, q * (crack.length - 1)), i = Math.floor(s), f = s - i;
          return [crack[i][0] + (crack[i + 1][0] - crack[i][0]) * f, crack[i][1] + (crack[i + 1][1] - crack[i][1]) * f];
        };
        const aim = q => { const [x, y] = at(q); return { x: from[0], y: from[1], r: deg(Math.atan2(y - from[1], x - from[0])), sx: Math.hypot(x - from[0], y - from[1]) / 20 }; };
        c.set({ beam: aim(0), spark: { x: at(0)[0], y: at(0)[1] } }); c.to({ 'beam.o': 1, 'spark.o': 1 }, 120);
        c.path('beam', aim, 2000, 'io'); c.path('spark', q => { const [x, y] = at(q); return { x, y }; }, 2000, 'io');
        c.tw({ 'chip.crack': 0 }, 1800, 'io'); c.tw({ 'chip.fixed': 1 }, 1600, 'io', c.t + 400); c.wait(2000);
        c.to({ 'beam.o': 0, 'spark.o': 0 }, 160); c.to({ head: 0 }, 200); c.rest(260); c.mood('happy'); c.say('ok', 620); c.mood('open');
        c.walk(80, { face: false, speed: 22 });
        k.clear(['chip']); k.leave();
      } },
    floppy: { kind: 'work', families: ['checkpoint', 'composition'], weight: 4, compose: true,
      props: r => [{ id: 'drv', art: 'fdrive', x: 120, set: true }, { id: 'disk', art: 'floppy', layer: 'front', o: 0 },
        { id: 'rrk', art: 'bubble', layer: 'front', text: 'RRRK-RRRK!', o: 0 }, { id: 'ploc', art: 'bubble', layer: 'front', text: 'PLOC!', tone: 'pow', o: 0 },
        { id: 'love', art: 'bubble', layer: 'front', text: '♥', tone: 'love', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['SALVO, MACHO!', 'BACKUP FEITO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['drv']);
        c.to({ armF: -66, elbF: -50 }, 240);
        const hand = c.point('handF'); c.set({ disk: { x: hand[0] + 2.4, y: hand[1] - 2.4 }, 'disk.pop': 0.3 }); c.attach('disk', 'handF');
        c.to({ 'disk.o': 1, 'disk.pop': 1 }, 240, 'back'); c.look(3, 0.6);
        c.walk(104, { speed: 20 });
        const slot = [120, -16.7];
        c.place('disk', 'F', [slot[0], slot[1] - 4.6], 380, 'io', { with: { lean: 8 } });
        c.detach('disk'); c.to({ disk: { y: slot[1], sy: 0.12 }, 'disk.o': 0 }, 260, 'in');
        c.rest(240);
        c.set({ 'drv.busy': 1 });
        c.par(a => { for (let i = 0; i < 3; i++) a.to({ 'drv.pop': 1.03 }, 120).to({ 'drv.pop': 1 }, 120); }, a => a.say('rrk', 900, { on: 'drv', dx: 0, dy: -26 }));
        k.pause();
        c.set({ 'drv.busy': 0 });
        c.to({ disk: { y: slot[1] - 7, sy: 1 }, 'disk.o': 1 }, 160, 'out'); c.say('ploc', 300, { on: 'drv', dx: 0, dy: -26 });
        c.reachAt('F', 'disk', [-2.4, 2.4], 260); c.grab('disk'); c.reach('F', c.point('root', [8, -34]), 320);
        c.mood('happy'); c.say('love', 420, { dx: 6, dy: -16 }); c.say('ok', 560);
        c.to({ 'disk.o': 0, 'disk.pop': 0.3 }, 220); c.detach('disk'); c.rest(260); c.mood('open');
        c.walk(80, { face: false, speed: 22 });
        k.clear(['drv']); k.leave();
      } },
    print3d: { kind: 'work', families: ['composition', 'restoration'], weight: 4, compose: true,
      props: r => [{ id: 'p3d', art: 'p3d', x: 122, set: true }, { id: 'mini', art: 'mini', x: 122, y: -8, o: 0, layer: 'front' },
        { id: 'cute', art: 'bubble', layer: 'front', text: r.pick(['QUE BICHIM LINDO!', 'ÉGUA, SOU EU!']), tone: 'love', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['p3d']);
        c.walk(98, { speed: 20 }); c.reachAt('F', 'p3d', [-11, -20], 300, 'io', { with: { lean: 6 } }); c.to({ elbF: c.cur('elbF') - 6 }, 100); c.rest(240);
        const start = c.t;
        c.set({ 'p3d.hot': 1 });
        for (let i = 0; i < 8; i++) c.tw({ 'p3d.head': i % 2 ? -4.6 : 4.6 }, 360, 'io', start + i * 360);
        c.tw({ 'p3d.grow': 1 }, 2880, 'linear', start);
        c.look(3.4, 0.4); c.to({ lean: 10, head: 6 }, 400); c.wait(900); c.look(2.4, 0.8, 300); c.wait(600); c.to({ lean: 0, head: 0 }, 300);
        c.at(Math.max(c.t, start + 2880)); c.tw({ 'p3d.head': 0 }, 300); c.set({ 'p3d.hot': 0 });
        c.wait(300); c.set({ 'p3d.grow': 0, 'mini.o': 1 });
        c.walk(106, { speed: 18 });
        c.reachAt('F', 'mini', [0, -3], 300, 'io', { with: { lean: 10 } }); c.grab('mini');
        c.reach('F', c.point('root', [9, -36]), 400); c.look(3, 0.4); c.mood('happy');
        c.to({ 'mini.tilt': 12 }, 160).to({ 'mini.tilt': -12 }, 160).to({ 'mini.tilt': 0 }, 160);
        c.say('cute', 700);
        c.place('mini', 'F', [122, -8], 400, 'io', { with: { lean: 10 } }); c.drop('mini', { onto: 'p3d' }); c.rest(260); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['p3d', 'mini']); k.leave();
      } },
    // Carries a wobbly stack of papers in (and out); the top sheet always tries to escape.
    papers: { kind: 'work', families: ['reading', 'composition'], weight: 4, compose: true, carryIn: true, carryOut: true,
      props: r => [{ id: 'stack', art: 'paperstack', layer: 'front' }, { id: 'loose', art: 'sheet', layer: 'front', o: 0 },
        { id: 'vixe', art: 'bubble', layer: 'front', text: r.pick(['VIXE!', 'VOLTA, BICHO!']), tone: 'pow', o: 0 },
        { id: 'got', art: 'bubble', layer: 'front', text: r.pick(['PEGUEI!', 'NA MÃO, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        c.set({ armF: -70, elbF: -40, armB: -60, elbB: -46, lean: -4 });
        const hand = c.point('handF'); c.set({ stack: { x: hand[0] + 4, y: hand[1] + 2.6 } }); c.attach('stack', 'handF');
        k.arrive(84, { carry: ['stack'] });
        c.to({ 'stack.tilt': 6 }, 260).to({ 'stack.tilt': -5 }, 260).to({ 'stack.tilt': 8 }, 220);
        const s = c.pose('stack'), land = s.x + r.range(26, 34);
        c.set({ loose: { x: s.x, y: s.y - 8.2 }, 'loose.o': 1 });
        c.path('loose', q => ({ x: s.x + (land - s.x) * q + 3 * Math.sin(q * 9), y: (s.y - 8.2) * (1 - q) - 9 * Math.sin(q * Math.PI), r: 50 * Math.sin(q * 7) * (1 - q) }), 1700, 'linear');
        c.mood('wide'); c.to({ 'stack.tilt': 0 }, 200); c.say('vixe', 420);
        c.par(a => a.crouch(5, 360), a => a.reach('F', a.point('root', [10, -12]), 360, 'io', { with: { lean: 24 } }));
        c.drop('stack'); c.to({ stack: { y: 0 } }, 140, 'in');
        c.mood('open'); c.par(a => a.crouch(0, 300), a => a.to({ ...REST, lean: 0 }, 300));
        k.pause();
        c.walk(land - 9, { speed: 30 });
        c.par(a => a.crouch(5, 300), a => a.reachAt('F', 'loose', [0, -2], 300, 'io', { with: { lean: 22 } })); c.grab('loose');
        c.par(a => a.crouch(0, 300), a => a.reach('F', a.point('root', [9, -32]), 300)); c.set({ 'loose.tilt': 0 });
        c.mood('happy'); c.say('got', 520); c.mood('open');
        const stack = c.pose('stack');
        c.goTo(stack.x - 11, 1);
        c.par(a => a.crouch(5, 300), a => a.place('loose', 'F', [stack.x, -8.6], 300, 'io', { with: { lean: 22 } }));
        c.drop('loose', { onto: 'stack' }); c.to({ loose: { r: 0 } }, 100);
        c.reachAt('F', 'stack', [-4, -4], 260, 'io', { with: { lean: 22 } }); c.grab('stack');
        c.par(a => a.crouch(0, 380), a => a.to({ armF: -70, elbF: -40, armB: -60, elbB: -46, lean: -4 }, 380));
        k.leave({ carry: ['stack', 'loose'] });
      } },
    // Physical files: a drawer that never wants to stay shut.
    filing: { kind: 'work', families: ['checkpoint', 'composition', 'reading'], weight: 4, compose: true,
      props: r => [{ id: 'cab', art: 'filecab', x: 132, set: true },
        { id: 'f1', art: 'folder', x: 90, y: -5, set: true }, { id: 'f2', art: 'folder', x: 94, y: -6, set: true }, { id: 'f3', art: 'folder', x: 98, y: -5, set: true },
        { id: 'crate', art: 'box', x: 94, set: true },
        { id: 'plof', art: 'bubble', layer: 'front', text: 'PLOF!', tone: 'pow', o: 0 }, { id: 'tum', art: 'bubble', layer: 'front', text: 'TUM!', tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['ARRUMADINHO!', 'TUDO NO LUGAR!']), o: 0 }],
      play(c, r, k) {
        k.arrive(74); k.bring(['cab', 'f1', 'f2', 'f3', 'crate']);
        c.walk(106, { speed: 22 });
        c.reachAt('F', 'cab', [-11.5, -30.3], 320, 'io', { with: { lean: 6 } });
        c.par(a => a.to({ 'cab.dr': -9 }, 420, 'io'), a => a.reachAt('F', 'cab', [-20.5, -30.3], 420, 'io', { with: { lean: 0 } }));
        c.rest(240);
        for (const id of ['f3', 'f2', 'f1']) {
          c.turn(-1); c.reachAt('F', id, [0, -8], 300, 'io', { with: { lean: 12 } }); c.grab(id);
          c.reach('F', c.point('root', [9, -32]), 300); c.turn(1);
          c.look(3, 0.6, 160); c.nod(1);
          c.place(id, 'F', [117, -36], 320, 'io', { with: { lean: 6 } }); c.detach(id);
          c.to({ [id]: { y: -29 }, [`${id}.o`]: 0 }, 260, 'in');
        }
        c.reachAt('F', 'cab', [-20.5, -30.3], 260);
        c.par(a => a.to({ 'cab.dr': 0 }, 300, 'in'), a => a.reachAt('F', 'cab', [-11.5, -30.3], 300));
        c.rest(200); c.wait(200);
        c.par(a => a.to({ 'cab.dr': -5 }, 180, 'back'), a => a.say('plof', 360, { on: 'cab', dx: -18, dy: -40 })); c.mood('wide'); c.wait(220);
        c.reachAt('F', 'cab', [-17, -30.3], 220, 'in', { with: { lean: 10 } });
        c.par(a => a.to({ 'cab.dr': 0 }, 110, 'in'), a => a.say('tum', 340, { on: 'cab', dx: -18, dy: -40 }));
        c.rest(220); c.mood('happy'); c.say('ok', 600); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['cab', 'crate', 'f1', 'f2', 'f3']); k.leave();
      } },
    // A folder inside a folder inside a folder... and finally the file.
    nested: { kind: 'work', families: ['reading', 'verification'], weight: 4, compose: true,
      props: r => [{ id: 'desk', art: 'table', x: 120, set: true },
        { id: 'f1', art: 'nfolder', x: 112, y: -13, set: true }, { id: 'f2', art: 'nfolder', x: 112, y: -15, o: 0, layer: 'front' },
        { id: 'f3', art: 'nfolder', x: 128, y: -15, o: 0, layer: 'front' }, { id: 'doc', art: 'doc', x: 128, y: -15, o: 0, layer: 'front' },
        { id: 'oxe', art: 'bubble', layer: 'front', text: r.pick(['OUTRA PASTA?', 'OXE, PASTA?']), o: 0 },
        { id: 'oxe2', art: 'bubble', layer: 'front', text: r.pick(['DE NOVO?!', 'ÉGUA, OUTRA!']), tone: 'hot', o: 0 },
        { id: 'got', art: 'bubble', layer: 'front', text: r.pick(['ACHEI, MACHO!', 'ÊITA, ACHEI!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        c.set({ f1: { sx: 1.5, sy: 1.5 }, f2: { sx: 1.1, sy: 1.1 }, f3: { sx: 0.75, sy: 0.75 } });
        k.arrive(80); k.bring(['desk', 'f1']);
        const open = (id, near) => {
          c.walk(near, { speed: 20 });
          c.reachAt('F', id, [0, -8], 280, 'io', { with: { lean: 10 } });
          c.par(a => a.to({ [`${id}.cover`]: -0.3 }, 360, 'io'), a => a.reachAt('F', id, [2, 4], 360, 'io', { with: { lean: 10 } }));
          c.rest(220);
        };
        const pop = (id, from, to) => {
          c.set({ [id]: { x: from[0], y: from[1] }, [`${id}.pop`]: 0.4 });
          c.tw({ [`${id}.o`]: 1 }, 120);
          c.path(id, q => ({ x: from[0] + (to[0] - from[0]) * q, y: from[1] + (to[1] - from[1]) * q - 14 * Math.sin(Math.PI * q) }), 620, 'io');
          c.to({ [`${id}.pop`]: 1 }, 620, 'back');
        };
        open('f1', 96);
        pop('f2', [112, -18], [120, -15]); c.look(3.4, 0.6); c.say('oxe', 520);
        k.pause();
        open('f2', 104);
        pop('f3', [120, -18], [128, -15]); c.mood('wide'); c.say('oxe2', 480); c.mood('open');
        open('f3', 113);
        pop('doc', [128, -16], [128, -22]);
        c.reachAt('F', 'doc', [0, -3.5], 280, 'io', { with: { lean: 8 } }); c.grab('doc');
        c.to({ armF: -160, elbF: -14 }, 360, 'out'); c.mood('happy'); c.say('got', 640); c.mood('open');
        c.to({ 'doc.o': 0, 'doc.pop': 0.3 }, 220); c.detach('doc'); c.rest(240);
        c.walk(84, { face: false, speed: 22 });
        k.clear(['desk', 'f1', 'f2', 'f3']); k.leave();
      } },
    shredder: { kind: 'work', families: ['checkpoint', 'access'], weight: 3, compose: true,
      props: r => [{ id: 'shred', art: 'shredder', x: 124, set: true }, { id: 'tray', art: 'tray', x: 98, set: true },
        ...[0, 1, 2].map(i => ({ id: `s${i}`, art: 'sheet', x: 97 + i * 1.2, y: -7 - i * 0.6, set: true, layer: 'front' })),
        { id: 'rrr', art: 'bubble', layer: 'front', text: 'RRRRRR!', o: 0 },
        { id: 'bye', art: 'bubble', layer: 'front', text: r.pick(['TCHAU, DADOS!', 'SEGREDO É SEGREDO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['shred', 'tray', 's0', 's1', 's2']);
        for (const [n, id] of ['s2', 's1', 's0'].entries()) {
          c.goTo(98 - 10, 1);
          c.reachAt('F', id, [0, -3], 280, 'io', { with: { lean: 16 } }); c.grab(id);
          c.reach('F', c.point('root', [10, -32]), 300);
          if (n === 2) { c.look(3, 1, 160); c.to({ head: 8 }, 200); c.wait(400); c.to({ head: 0 }, 160); }
          c.walk(112, { speed: 24 });
          c.place(id, 'F', [124, -26], 300, 'io', { with: { lean: 6 } }); c.detach(id);
          c.set({ 'shred.cut': 1 });
          c.par(a => a.to({ [id]: { y: -14 }, [`${id}.o`]: 0 }, 700, 'linear'), a => { for (let i = 0; i < 3; i++) a.to({ 'shred.pop': 1.03 }, 110).to({ 'shred.pop': 1 }, 110); },
            a => { if (!n) a.say('rrr', 520, { on: 'shred', dx: 0, dy: -30 }); });
          c.set({ 'shred.cut': 0 }); c.rest(160);
        }
        c.walk(104, { face: false, speed: 22 }); c.turn(1); c.mood('happy'); c.say('bye', 600); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['shred', 'tray']); k.leave();
      } },
    // Code in, package out; sometimes it needs a kick.
    compile: { kind: 'work', families: ['calculation', 'composition'], weight: 4, compose: true,
      props: r => [{ id: 'mach', art: 'compiler', x: 126, set: true }, { id: 'tray', art: 'tray', x: 96, set: true },
        ...[0, 1].map(i => ({ id: `s${i}`, art: 'sheet', x: 95.4 + i * 1.2, y: -7 - i * 0.6, set: true, layer: 'front' })),
        { id: 'pkg', art: 'parcel', x: 140, y: -9.6, o: 0 },
        { id: 'err', art: 'bubble', layer: 'front', text: r.pick(['DEU ERRO, VISH!', 'OXE, TRAVOU!']), tone: 'hot', o: 0 },
        { id: 'tum', art: 'bubble', layer: 'front', text: 'TUM!', tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['COMPILOU!', 'BUILD PASSOU!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(78); k.bring(['mach', 'tray', 's0', 's1']);
        for (const id of ['s1', 's0']) {
          c.goTo(86, 1);
          c.reachAt('F', id, [0, -3], 280, 'io', { with: { lean: 16 } }); c.grab(id);
          c.reach('F', c.point('root', [10, -34]), 300);
          c.walk(108, { speed: 24 });
          c.place(id, 'F', [119.6, -34], 320, 'io', { with: { lean: 4 } }); c.detach(id);
          c.to({ [id]: { y: -27 }, [`${id}.o`]: 0 }, 300, 'in'); c.rest(160);
        }
        const run = length => {
          const start = c.t;
          c.set({ 'mach.run': 1 }); c.tw({ 'mach.gear': c.cur('mach.gear') + length * 0.45 }, length, 'io', start);
          for (let time = 0; time < length; time += 220) c.tw({ 'mach.tilt': (time / 220) % 2 ? -1.2 : 1.2 }, 110, 'io', start + time).tw({ 'mach.tilt': 0 }, 110, 'io', start + time + 110);
          return start + length;
        };
        c.walk(100, { face: false, speed: 22 }); c.turn(1);
        let end = run(1600); c.look(3.4, -0.2); c.wait(400); c.to({ lean: 6 }, 300).to({ lean: 0 }, 300);
        c.at(Math.max(c.t, end));
        if (r.chance(0.5)) {
          c.set({ 'mach.run': 0, 'mach.err': 1 }); c.mood('wide'); c.say('err', 560); c.mood('open');
          k.pause();
          c.walk(111, { speed: 24 });
          c.to({ legF: -30, kneeF: 30 }, 160, 'out'); c.to({ legF: -62, kneeF: 4 }, 90, 'in');
          c.par(a => a.to({ 'mach.tilt': 3 }, 80).to({ 'mach.tilt': 0 }, 260, 'back'), a => a.say('tum', 320, { on: 'mach', dx: -12, dy: -30 }));
          c.to({ legF: 0, kneeF: 0 }, 200); c.set({ 'mach.err': 0 });
          c.walk(100, { face: false, speed: 22 }); c.turn(1);
          end = run(900); c.at(Math.max(c.t, end));
        }
        c.set({ 'mach.run': 0 });
        c.to({ 'pkg.o': 1 }, 80); c.to({ pkg: { x: 146 } }, 300, 'out');
        c.path('pkg', q => ({ x: 146 + 6 * q, y: -9.6 * (1 - q) * (1 - q) - 4 * Math.sin(Math.PI * q) }), 420, 'in'); c.wait(420);
        c.to({ pkg: { sy: 0.85 } }, 80).to({ pkg: { sy: 1 } }, 200, 'back');
        c.mood('happy'); c.say('ok', 620); c.hop(4); c.mood('open');
        c.walk(80, { face: false, speed: 22 });
        k.clear(['mach', 'tray', 'pkg']); k.leave();
      } },
    encrypt: { kind: 'work', families: ['access', 'checkpoint'], weight: 4, compose: true,
      props: r => [{ id: 'safe', art: 'safe', x: 122, set: true }, { id: 'doc', art: 'doc', layer: 'front', o: 0 },
        { id: 'tic', art: 'bubble', layer: 'front', text: 'TIC-TIC-TIC', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['TRANCADO, MACHO!', 'CRIPTOGRAFADO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['safe']);
        c.walk(100, { speed: 20 });
        c.reachAt('F', 'safe', [5.6, -10], 300, 'io', { with: { lean: 14 } });
        c.par(a => a.to({ 'safe.door': -0.28 }, 480, 'io'), a => a.reachAt('F', 'safe', [-12.4, -10], 480, 'io', { with: { lean: 8 } }));
        c.rest(240);
        c.to({ armF: -66, elbF: -50 }, 240);
        const hand = c.point('handF'); c.set({ doc: { x: hand[0] + 1.6, y: hand[1] + 2.6 }, 'doc.pop': 0.3 }); c.attach('doc', 'handF');
        c.to({ 'doc.o': 1, 'doc.pop': 1 }, 240, 'back'); c.look(3, 0.6); c.wait(200);
        c.place('doc', 'F', [122, -12], 360, 'io', { with: { lean: 16 } }); c.detach('doc'); c.to({ 'doc.o': 0 }, 200);
        c.reachAt('F', 'safe', [-12.4, -10], 260, 'io', { with: { lean: 10 } });
        c.par(a => a.to({ 'safe.door': 1 }, 420, 'io'), a => a.reachAt('F', 'safe', [5.6, -10], 420, 'io', { with: { lean: 14 } }));
        c.reachAt('F', 'safe', [-0.4, -13], 240, 'io', { with: { lean: 14 } });
        c.par(a => a.to({ 'safe.dial': 300 }, 500, 'io').to({ 'safe.dial': 120 }, 400, 'io').to({ 'safe.dial': 250 }, 360, 'io'), a => a.say('tic', 1000, { on: 'safe', dx: 0, dy: -28 }));
        c.set({ 'safe.lock': 1 }); c.rest(240);
        c.reachAt('F', 'safe', [5.6, -10], 240, 'io', { with: { lean: 14 } });
        c.to({ 'safe.door': 0.95 }, 90).to({ 'safe.door': 1 }, 90).to({ 'safe.door': 0.95 }, 90).to({ 'safe.door': 1 }, 90);
        c.rest(240); c.mood('happy'); c.nod(1); c.say('ok', 600); c.mood('open');
        k.pause();
        c.walk(80, { face: false, speed: 22 });
        k.clear(['safe']); k.leave();
      } },
    // Bubble sort by hand: lift one bar, the neighbour slides over.
    sort: { kind: 'work', families: ['calculation', 'reading'], weight: 4, compose: true,
      props: r => [{ id: 'shelf', art: 'shelf', x: 120, set: true },
        ...[[14, 3], [6, 1], [18, 4], [10, 2]].map(([h, tone], i) => ({ id: `q${i}`, art: 'sbar', h, tone, x: 106 + i * 9, y: -2.4, set: true })),
        { id: 'hmm', art: 'bubble', layer: 'front', text: r.pick(['TÁ BAGUNÇADO…', 'OXE, FORA DE ORDEM?']), o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['ORDENADINHO!', 'O(N²), MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(84); k.bring(['shelf', 'q0', 'q1', 'q2', 'q3']);
        const order = ['q0', 'q1', 'q2', 'q3'], height = { q0: 14, q1: 6, q2: 18, q3: 10 }, slot = i => 106 + i * 9;
        c.look(3.4, 0.4); c.to({ head: 6 }, 200); c.say('hmm', 600); c.to({ head: 0 }, 160);
        for (const i of [0, 2, 1]) {
          const a = order[i], b = order[i + 1];
          c.goTo(slot(i) - 10, 1);
          c.reachAt('F', a, [0, -height[a]], 280, 'io', { with: { lean: 6 } }); c.grab(a);
          c.place(a, 'F', [slot(i), -10.4], 300, 'io');
          c.par(x => x.path(b, q => ({ x: slot(i + 1) - 9 * q }), 420, 'io'), x => x.walk(slot(i + 1) - 10, { arms: false, speed: 22 }));
          c.place(a, 'F', [slot(i + 1), -10.4], 200, 'io'); c.place(a, 'F', [slot(i + 1), -2.4], 240, 'in'); c.drop(a);
          [order[i], order[i + 1]] = [b, a];
          c.rest(180);
        }
        k.pause();
        c.walk(92, { face: false, speed: 22 }); c.turn(1);
        c.to(Object.fromEntries(order.map(id => [`${id}.ok`, 1])), 200);
        c.mood('happy'); c.say('ok', 640); c.mood('open'); c.to(Object.fromEntries(order.map(id => [`${id}.ok`, 0])), 400);
        c.walk(84, { face: false, speed: 22 });
        k.clear(['shelf', ...order]); k.leave();
      } },
    // Garbage collection: bits into the bin; one of them misses.
    gc: { kind: 'work', families: ['checkpoint', 'calculation'], weight: 4, compose: true,
      props: r => [{ id: 'bin', art: 'trashbin', x: 88, set: true },
        ...['0', '1', '1'].map((g, i) => ({ id: `t${i}`, art: 'token', g, x: 110 + i * 11, set: true, layer: 'front' })),
        { id: 'miss', art: 'bubble', layer: 'front', text: r.pick(['OXE!', 'ERREI, VISH!']), tone: 'hot', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['NA MOSCA!', 'CESTA, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(98); k.bring(['bin', 't0', 't1', 't2']);
        const toss = (id, miss) => {
          c.to({ armF: -150, elbF: -40 }, 200, 'out'); c.to({ armF: -60, elbF: -20 }, 140, 'in'); c.detach(id);
          const from = c.pose(id), to = miss ? [95.6, -12] : [88, -15];
          c.to({ 'bin.lid': -60 }, 160, 'out');
          c.path(id, q => ({ x: from.x + (to[0] - from.x) * q, y: from.y + (to[1] - from.y) * q - 18 * Math.sin(Math.PI * q), r: 360 * q }), 620, 'linear'); c.wait(620);
          if (miss) {
            c.path(id, q => ({ x: to[0] + 8 * q, y: -12 * (1 - q) - 5 * Math.sin(Math.PI * q), r: 360 + 200 * q }), 420, 'in'); c.wait(420); c.to({ 'bin.lid': 0 }, 200);
          } else { c.to({ [`${id}.o`]: 0 }, 120); c.to({ 'bin.lid': 0 }, 200, 'back'); }
          c.rest(180);
        };
        for (const [n, id] of ['t0', 't1', 't2'].entries()) {
          c.goTo(c.pose(id).x - 8, 1);
          c.par(a => a.crouch(5, 260), a => a.reachAt('F', id, [0, -4], 260, 'io', { with: { lean: 22 } })); c.grab(id);
          c.par(a => a.crouch(0, 260), a => a.reach('F', a.point('root', [9, -30]), 260));
          c.turn(-1); toss(id, n === 1);
          if (n === 1) {
            c.mood('wide'); c.say('miss', 420); c.mood('open');
            c.walk(c.pose(id).x + 8, { speed: 22 });
            c.par(a => a.crouch(5, 260), a => a.reachAt('F', id, [0, -4], 260, 'io', { with: { lean: 22 } })); c.grab(id);
            c.par(a => a.crouch(0, 260), a => a.reach('F', a.point('root', [9, -30]), 260));
            c.walk(97, { speed: 20, arms: false });
            c.place(id, 'F', [88, -20], 300); c.detach(id); c.to({ 'bin.lid': -60 }, 120);
            c.to({ [id]: { y: -10 }, [`${id}.o`]: 0 }, 260, 'in'); c.to({ 'bin.lid': 0 }, 200, 'back');
            c.mood('happy'); c.say('ok', 560); c.mood('open');
          }
        }
        k.pause();
        c.walk(98, { speed: 22 }); c.turn(1);
        k.clear(['bin']); k.leave();
      } },
    upload: { kind: 'work', families: ['checkpoint', 'composition'], weight: 4, compose: true,
      props: r => [...[1, 2].map(i => ({ id: `b${i}`, art: 'block', tone: i, x: 102 + i * 6, y: -2.6, set: true, layer: 'front' })),
        { id: 'cld', art: 'cloud', x: 128, y: -150 },
        { id: 'bonk', art: 'bubble', layer: 'front', text: r.pick(['OXE!', 'AI, MINHA CABEÇA!']), tone: 'hot', o: 0 },
        { id: 'up', art: 'bubble', layer: 'front', text: r.pick(['SOBE, BICHIM!', 'VAI PRA NUVEM!']), tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['NA NUVEM, MACHO!', 'UPLOAD FEITO!']), o: 0 }],
      play(c, r, k) {
        k.arrive(86); k.bring(['b1', 'b2']);
        c.look(2.4, -2); c.to({ armF: -172, elbF: -8, head: -10 }, 320, 'out');
        summon(c, 'cld', -82, 1000); c.to({ armF: -105, elbF: -30 }, 1000, 'io'); c.rest(260); c.look(2.4, 0);
        const toss = (id, big = false) => {
          if (big) c.to({ armF: 40, elbF: -30, lean: -8 }, 260);
          c.to({ armF: -170, elbF: -10, lean: 4 }, 160, 'in'); c.detach(id);
          const from = c.pose(id);
          c.path(id, q => ({ x: from.x + (128 - from.x) * q, y: from.y + (-88 - from.y) * q - 10 * Math.sin(Math.PI * q), r: 180 * q }), 700, 'out');
          c.tw({ [`${id}.o`]: 0 }, 160, 'linear', c.t + 560); c.wait(700);
          c.to({ 'cld.pop': 1.12 }, 120, 'out').to({ 'cld.pop': 1 }, 220, 'back'); c.rest(200);
        };
        for (const id of ['b2', 'b1']) {
          c.goTo(c.pose(id).x - 8, 1);
          c.par(a => a.crouch(5, 260), a => a.reachAt('F', id, [0, -1], 260, 'io', { with: { lean: 22 } })); c.grab(id);
          c.par(a => a.crouch(0, 260), a => a.reach('F', a.point('root', [9, -30]), 260));
          c.walk(108, { speed: 24 }); toss(id);
        }
        // The cloud sends one back, straight onto the robot's head.
        const head = c.point('head', [0, -18]);
        c.set({ b1: { x: 128, y: -86, r: 0 }, 'b1.o': 1 });
        c.path('b1', q => ({ x: 128 + (head[0] - 128) * q, y: -86 + (head[1] + 86) * q * q }), 520, 'linear'); c.wait(520);
        c.to({ head: 14, ry: 1.4 }, 90, 'out'); c.mood('wide');
        c.path('b1', q => ({ x: head[0] + 9 * q, y: head[1] * (1 - q) - 6 * Math.sin(Math.PI * q), r: 260 * q }), 520, 'in');
        c.say('bonk', 420, { dx: -8, dy: -18 }); c.to({ head: 0, ry: 0 }, 200); c.mood('open');
        c.par(a => a.crouch(5, 260), a => a.reachAt('F', 'b1', [0, -1], 260, 'io', { with: { lean: 22 } })); c.grab('b1');
        c.par(a => a.crouch(0, 260), a => a.reach('F', a.point('root', [9, -30]), 260));
        c.say('up', 420); toss('b1', true);
        c.mood('happy'); c.say('ok', 600); c.mood('open');
        k.pause();
        c.to({ armF: -105, elbF: -30 }, 240); summon(c, 'cld', -150, 900); c.to({ armF: -172, elbF: -8 }, 900, 'in'); c.rest(260);
        k.leave();
      } },
    firewall: { kind: 'work', families: ['access', 'verification'], weight: 4, compose: true,
      props: r => [...[[94, 0], [102, 0], [98, -4.2]].map(([x, y], i) => ({ id: `w${i}`, art: 'brick', x, y, set: true })),
        { id: 'fire', art: 'flame', x: 128, y: -8.6, o: 0 },
        { id: 'v1', art: 'virus', x: 240, y: -4, layer: 'front' }, { id: 'v2', art: 'virus', x: 250, y: -4, layer: 'front' },
        { id: 'pa', art: 'bubble', layer: 'front', text: 'PÁ!', tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['AQUI NÃO, BICHIM!', 'BARRADO, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(82); k.bring(['w0', 'w1', 'w2']);
        const wall = [[124, 0], [132, 0], [128, -4.2]];
        ['w1', 'w0', 'w2'].forEach((id, n) => {
          c.goTo(c.pose(id).x - 9, 1);
          const low = c.pose(id).y > -3;
          c.par(a => a.crouch(low ? 5 : 2, 240), a => a.reachAt('F', id, [0, -3], 240, 'io', { with: { lean: low ? 22 : 12 } })); c.grab(id);
          c.par(a => a.crouch(0, 240), a => a.reach('F', a.point('root', [9, -26]), 240));
          c.walk(wall[n][0] - 11, { speed: 32, arms: false });
          const down = wall[n][1] > -1;
          c.par(a => a.crouch(down ? 4 : 1, 260), a => a.place(id, 'F', [wall[n][0], wall[n][1] - 1.2], 260, 'io', { with: { lean: down ? 20 : 10 } }));
          c.place(id, 'F', wall[n], 120, 'in'); c.drop(id); c.crouch(0, 220);
        });
        c.walk(110, { face: false, speed: 22 }); c.turn(1);
        c.set({ fire: { x: 128, y: -8.4 }, 'fire.pop': 0.3 }); c.to({ 'fire.o': 1, 'fire.pop': 1 }, 320, 'back');
        k.pause();
        for (const [id, wait] of r.chance(0.5) ? [['v1', 0], ['v2', 400]] : [['v1', 0]]) {
          c.wait(wait);
          c.path(id, q => ({ x: 240 + (139.6 - 240) * q, y: -4 - 3 * Math.abs(Math.sin(q * Math.PI * 3)), r: -540 * q }), 900, 'linear'); c.wait(900);
          c.par(a => a.path(id, q => ({ x: 139.6 + 110 * q, y: -4 - 8 * Math.sin(Math.PI * Math.min(1, q * 2)), r: -540 + 400 * q }), 900, 'out'), a => a.say('pa', 320, { on: id, dx: 0, dy: -8 }));
          c.to({ 'fire.pop': 1.2 }, 100).to({ 'fire.pop': 1 }, 200, 'back');
        }
        for (let i = 0; i < 2; i++) c.to({ armF: -60, elbF: -60, armB: -50, elbB: -70 }, 120).to({ armF: -50, elbF: -70, armB: -60, elbB: -60 }, 120);
        c.mood('happy'); c.say('ok', 640); c.rest(240); c.mood('open');
        c.walk(86, { face: false, speed: 22 });
        k.clear(['w0', 'w1', 'w2', 'fire']); k.leave();
      } },
    // Debugging, literally: the bug always escapes.
    debug: { kind: 'work', families: ['verification', 'reading'], weight: 4, compose: true,
      props: r => [{ id: 'desk', art: 'table', x: 122, set: true }, { id: 'bugg', art: 'bug', x: 116, y: -13, set: true, layer: 'front' },
        { id: 'lens', art: 'lens', layer: 'front', o: 0 },
        { id: 'ali', art: 'bubble', layer: 'front', text: r.pick(['ALI, Ó!', 'TE PEGUEI…']), o: 0 },
        { id: 'pa', art: 'bubble', layer: 'front', text: 'PÁ!', tone: 'pow', o: 0 },
        { id: 'miss', art: 'bubble', layer: 'front', text: r.pick(['BUG ESPERTO, VISH!', 'FUGIU, MACHO!']), tone: 'hot', o: 0 }],
      play(c, r, k) {
        k.arrive(82); k.bring(['desk', 'bugg']);
        c.to({ armF: -66, elbF: -50 }, 240);
        const hand = c.point('handF'); c.set({ lens: { x: hand[0], y: hand[1] }, 'lens.pop': 0.3 }); c.attach('lens', 'handF');
        c.to({ 'lens.o': 1, 'lens.pop': 1 }, 240, 'back');
        c.walk(103, { speed: 20 }); c.look(3.4, 0.6);
        let bx = 116;
        for (const [to, len] of [[126, 800], [112, 900], [124, 700]]) {
          const from = bx;
          c.set({ bugg: { sx: to > from ? 1 : -1 } });
          c.par(a => a.path('bugg', q => ({ x: from + (to - from) * q, y: -13 }), len, 'io'), a => a.place('lens', 'F', [to - 7.4, -13 + 7.4 - 6], len, 'io', { with: { lean: 8 } }));
          bx = to; c.mood('wide'); c.wait(260); c.mood('open');
        }
        c.say('ali', 520);
        c.to({ armB: -168, elbB: -10 }, 360, 'io');
        c.reach('B', [bx, -15], 130, 'in', { with: { lean: 12 } });
        c.set({ bugg: { sx: 1 } });
        c.par(a => a.path('bugg', q => ({ x: bx + (250 - bx) * q, y: -13 * (1 - Math.min(1, q * 4)) - 6 * Math.sin(Math.PI * Math.min(1, q * 4)) }), 1200, 'linear'), a => a.say('pa', 360, { dx: 12, dy: -10 }));
        c.look(4, 0.4, 200); c.wait(300);
        c.to({ armF: -150, elbF: -116, head: 10 }, 300); c.say('miss', 640); c.to({ 'lens.o': 0 }, 200); c.detach('lens'); c.rest(260); c.to({ head: 0 }, 160);
        k.pause();
        c.walk(84, { face: false, speed: 22 });
        k.clear(['desk']); k.leave();
      } },
    // Stack overflow: one box too many.
    stack: { kind: 'work', families: ['calculation', 'checkpoint'], weight: 4, compose: true,
      props: r => [...[[100, 0], [109, 0], [104.5, -7], [104.5, -14]].map(([x, y], i) => ({ id: `k${i}`, art: 'box2', x, y, set: true })),
        { id: 'vixe', art: 'bubble', layer: 'front', text: r.pick(['VIXE!', 'NÃO, NÃO, NÃO…']), tone: 'hot', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['ESTOUROU A PILHA!', 'STACK OVERFLOW!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(80); k.bring(['k0', 'k1', 'k2', 'k3']);
        const order = ['k3', 'k2', 'k1', 'k0'];
        order.forEach((id, n) => {
          c.goTo(c.pose(id).x - 10, 1);
          const low = c.pose(id).y > -3;
          c.par(a => a.crouch(low ? 5 : 0, 240), a => a.reachAt('F', id, [0, -6], 240, 'io', { with: { lean: low ? 20 : 4 } })); c.grab(id);
          c.par(a => a.crouch(0, 240), a => a.reach('F', a.point('root', [9, -30]), 240));
          c.walk(118, { speed: 34, arms: false });
          const y = -7 * n;
          c.par(a => a.crouch(n ? 0 : 4, 260), a => a.place(id, 'F', [128, y - 1.4], 260, 'io', { with: { lean: n ? 2 : 18 } }));
          c.place(id, 'F', [128, y], 120, 'in'); c.drop(id); c.crouch(0, 200);
        });
        c.walk(110, { face: false, speed: 22 }); c.turn(1); c.look(3.4, -1);
        const start = c.t;
        order.slice().reverse().forEach((id, n) => c.path(id, q => ({ x: 128 + (4 - n) * 1.1 * Math.sin(q * Math.PI * 4) * q, y: -7 * (3 - n) }), 1100, 'linear'));
        c.mood('wide'); c.wait(300); c.say('vixe', 460); c.at(Math.max(c.t, start + 1100));
        ['k3', 'k2', 'k1', 'k0'].forEach((id, n) => {
          const from = c.pose(id), land = 140 + n * 8;
          c.path(id, q => ({ x: from.x + (land - from.x) * q, y: from.y * (1 - q * q) - 4 * Math.sin(Math.PI * q), r: (90 + n * 40) * q }), 560 + n * 60, 'in');
        });
        c.wait(800);
        c.to({ armF: -150, elbF: -116, head: 10 }, 320); c.mood('closed'); c.say('ok', 700); c.rest(260); c.to({ head: 0 }, 160); c.mood('open');
        k.pause();
        c.walk(86, { face: false, speed: 22 });
        k.clear(['k0', 'k1', 'k2', 'k3']); k.leave();
      } },
    pingpong: { kind: 'fun', weight: 2, minElapsedMs: 6000, compose: true,
      props: r => [{ id: 'pwall', art: 'pwall', x: 154, set: true }, { id: 'pad', art: 'paddle', layer: 'front', o: 0 }, { id: 'ball', art: 'pball', layer: 'front', o: 0 },
        { id: 'ping', art: 'bubble', layer: 'front', text: 'PING!', o: 0 }, { id: 'pong', art: 'bubble', layer: 'front', text: 'PONG!', tone: 'pow', o: 0 },
        { id: 'oxe', art: 'bubble', layer: 'front', text: r.pick(['OXE!', 'ÉGUA, PASSOU!']), tone: 'hot', o: 0 }],
      play(c, r, k) {
        k.arrive(96); k.bring(['pwall']);
        c.to({ armF: -40, elbF: -40 }, 240);
        const hand = c.point('handF'); c.set({ pad: { x: hand[0], y: hand[1] }, 'pad.pop': 0.3 }); c.attach('pad', 'handF');
        c.to({ 'pad.o': 1, 'pad.pop': 1 }, 240, 'back');
        const face = () => c.point('pad', [4.4, -2.2]), wall = [150.4, -22];
        let p = face();
        c.set({ ball: { x: p[0], y: p[1] } }); c.to({ 'ball.o': 1 }, 120);
        for (const [n, d] of [[0, 900], [1, 700], [2, 520]].map(([n, d]) => [n, d])) {
          const start = c.t;
          c.tw({ armF: -90, elbF: -24 }, 110, 'out', start);
          p = c.pose('ball');
          const from = [p.x, p.y];
          c.path('ball', q => ({ x: from[0] + (wall[0] - from[0]) * q, y: from[1] + (wall[1] - from[1]) * q - 5 * Math.sin(Math.PI * q) }), d / 2, 'linear');
          if (!n) c.say('ping', 220, { on: 'pad', dx: 0, dy: -8 });
          c.at(start + d / 2);
          c.path('ball', q => ({ x: wall[0] + (from[0] - wall[0]) * q, y: wall[1] + (from[1] - wall[1]) * q - 5 * Math.sin(Math.PI * q) }), d / 2, 'linear');
          if (!n) c.say('pong', 220, { on: 'pwall', dx: -4, dy: -40 });
          c.at(start + d / 2); c.tw({ armF: -40, elbF: -40 }, Math.min(260, d / 2 - 20), 'io', start + d / 2);
          c.at(start + d);
        }
        // The last one comes back too fast.
        p = c.pose('ball');
        c.path('ball', q => ({ x: p.x + (-40 - p.x) * q, y: p.y - 2 * q }), 700, 'linear');
        c.wait(120); c.to({ armF: -100, elbF: -24 }, 120, 'out'); c.turn(-1, 220); c.mood('wide'); c.say('oxe', 480); c.mood('open'); c.turn(1, 220);
        c.to({ 'pad.o': 0, 'pad.pop': 0.3 }, 200); c.detach('pad'); c.rest(240);
        k.pause();
        k.clear(['pwall']); k.leave();
      } },
    bitflip: { kind: 'fun', weight: 2, minElapsedMs: 4000, compose: true,
      props: r => [{ id: 'coin', art: 'coin', layer: 'front', o: 0 },
        { id: 'r1', art: 'bubble', layer: 'front', text: r.pick(['DEU 1, MACHO!', 'É UM, É UM!']), tone: 'pow', o: 0 },
        { id: 'r0', art: 'bubble', layer: 'front', text: r.pick(['DEU 0, VISH!', 'ZERO, ÉGUA…']), o: 0 }],
      play(c, r, k) {
        k.arrive(r.range(92, 104));
        c.to({ armF: -50, elbF: -60 }, 260);
        const hand = c.point('handF'); c.set({ coin: { x: hand[0] + 1, y: hand[1] - 2.6 }, 'coin.pop': 0.3 }); c.attach('coin', 'handF');
        c.to({ 'coin.o': 1, 'coin.pop': 1 }, 240, 'back'); c.look(3, 0.6); c.wait(200);
        const one = r.chance(0.5);
        c.to({ elbF: -80 }, 90, 'out'); c.detach('coin');
        const from = c.pose('coin');
        c.path('coin', q => ({ x: from.x, y: from.y - 34 * Math.sin(Math.PI * q), sy: Math.cos(q * Math.PI * 10) }), 1100, 'linear');
        for (let i = 0; i < 10; i++) c.tw({ 'coin.f0': i % 2, 'coin.f1': 1 - (i % 2) }, 1, 'linear', c.t + i * 110 + 55);
        c.look(3, -2, 300); c.wait(500); c.look(3, 0.6, 300); c.wait(300);
        c.attach('coin', 'handF'); c.set({ coin: { sy: 1 } });
        c.reach('B', c.point('handF', [0, -1]), 160, 'out');
        c.set({ 'coin.f0': one ? 0 : 1, 'coin.f1': one ? 1 : 0 }); c.wait(300);
        c.to({ armB: REST.armB, elbB: REST.elbB }, 200); c.look(2.6, 1, 160); c.wait(260);
        if (one) { c.mood('happy'); c.hop(4); c.say('r1', 600); } else { c.to({ armF: -70, armB: -60, elbB: -80, head: 6 }, 260); c.say('r0', 600); }
        c.to({ 'coin.o': 0, 'coin.pop': 0.3 }, 200); c.detach('coin'); c.rest(260); c.to({ head: 0 }, 160); c.mood('open');
        k.leave();
      } },
    keyboard: { kind: 'fun', weight: 2, minElapsedMs: 6000, compose: true,
      props: r => [{ id: 'kbd', art: 'kbd', x: 124, set: true }, { id: 'term', art: 'term', x: 126, y: -150 },
        { id: 'hello', art: 'bubble', layer: 'front', text: r.pick(['OLÁ, MUNDO!', 'DIGITEI, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(86); k.bring(['kbd']);
        c.look(2.4, -2); c.to({ armF: -172, elbF: -8, head: -10 }, 320, 'out');
        summon(c, 'term', -50, 1000); c.to({ armF: -105, elbF: -30 }, 1000, 'io'); c.rest(260); c.look(2.4, 0);
        const jump = (x, level) => c.par(a => a.to({ rx: x }, 440, 'io'), a => a.to({ fly: level - 10, kneeF: 20, kneeB: 20, legF: -10, legB: 6 }, 220, 'out').to({ fly: level, kneeF: 0, kneeB: 0, legF: 0, legB: 0 }, 220, 'in'));
        [113.2, 129.2, 145.2].forEach((x, i) => {
          jump(x, -5);
          c.par(a => a.crouch(1.6, 90, 'out').crouch(0, 160), a => a.set({ [`kbd.k${i}`]: 1, [`term.t${i}`]: 1 }));
          c.look(2.4, -2, 160); c.wait(200);
        });
        jump(160, 0); c.turn(-1); c.look(2.4, -2.2, 200); c.mood('happy'); c.say('hello', 700); c.mood('open'); c.turn(1);
        k.pause();
        c.to({ armF: -105, elbF: -30 }, 240); summon(c, 'term', -150, 900); c.to({ armF: -172, elbF: -8 }, 900, 'in'); c.rest(260); c.look(2.4, 0);
        k.clear(['kbd']); k.leave();
      } },
    weights: { kind: 'fun', weight: 2, minElapsedMs: 8000, compose: true,
      props: r => [{ id: 'bell', art: 'barbell', x: 112, y: -4.8, set: true, layer: 'front' },
        { id: 'ugh', art: 'bubble', layer: 'front', text: r.pick(['ARRIÉGUA…', 'PESADO, VISH…']), tone: 'hot', o: 0 },
        { id: 'tum', art: 'bubble', layer: 'front', text: 'TUM!', tone: 'pow', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['ÉGUA, MACHO!', 'TÔ FORTE!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(94); k.bring(['bell']);
        c.walk(110, { speed: 20 }); c.look(3, 1.4);
        const grab = () => { c.par(a => a.crouch(7, 360), a => a.reach('both', [112, -4.8], 360, 'io', { with: { lean: 16 } })); c.grab('bell'); };
        grab();
        c.par(a => a.crouch(3, 700, 'in'), a => a.reach('both', [112, -15], 700, 'in', { with: { lean: 4 } }));
        c.mood('closed');
        c.par(a => { for (let i = 0; i < 4; i++) a.to({ lean: i % 2 ? 2 : 6, head: i % 2 ? -2 : 3 }, 110); }, a => a.say('ugh', 420, { dx: 8, dy: -16 }));
        c.drop('bell'); c.to({ bell: { y: -4.8 } }, 160, 'in');
        c.par(a => a.say('tum', 320, { on: 'bell', dx: 0, dy: -8 }), a => a.to({ ...STAND, ...REST }, 300)); c.mood('open');
        for (let i = 0; i < 2; i++) c.to({ armF: 20, armB: -30 }, 120).to({ armF: -20, armB: 10 }, 120);
        c.rest(200);
        k.pause();
        grab();
        c.par(a => a.crouch(0, 600, 'io'), a => a.reach('both', [112, -24], 600, 'io', { with: { lean: 0 } }));
        c.reach('both', [112, -47], 420, 'out'); c.mood('happy');
        c.par(a => a.say('ok', 700, { dx: 10, dy: -10 }), a => { for (let i = 0; i < 3; i++) a.to({ lean: i % 2 ? -2 : 2 }, 200); });
        c.drop('bell');
        const y0 = c.pose('bell').y;
        c.path('bell', q => ({ y: y0 + (-4.8 - y0) * q * q }), 380, 'linear');
        c.par(a => a.rest(300), a => a.to({ ry: 0, fly: -5 }, 190, 'out').to({ fly: 0 }, 190, 'in'));
        c.say('tum', 320, { on: 'bell', dx: 0, dy: -8 });
        c.mood('open');
        c.walk(92, { face: false, speed: 22 });
        k.clear(['bell']); k.leave();
      } },
    // Into the dark corridor at the back of the room, and back with a crate.
    fetch: { kind: 'work', families: ['restoration', 'checkpoint', 'reading'], weight: 4, compose: true,
      props: r => [{ id: 'adoor', art: 'door', x: 148, o: 0 }, { id: 'ashut', art: 'shutter', x: 148, o: 0, layer: 'front' },
        { id: 'crate', art: 'box', o: 0, layer: 'front' }, { id: 'rubber', art: 'duck', o: 0 },
        { id: 'qq', art: 'bubble', layer: 'front', text: r.pick(['CADÊ?…', 'TÁ POR AQUI…']), o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['ACHEI NO DEPÓSITO!', 'TAVA LÁ NO FUNDO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(84);
        c.to({ 'adoor.o': 1, 'ashut.o': 1 }, 360); c.to({ 'adoor.trim': 0 }, 520);
        c.par(a => a.to({ 'ashut.roll': 0.02 }, 700, 'io').to({ 'ashut.o': 0 }, 80), a => a.walk(148, { speed: 30 }));
        c.par(a => a.gait(a.t, 1500), a => a.to({ dz: -7, deep: 0.52 }, 1500, 'io'));
        c.to({ ghost: 0 }, 420);
        c.say('qq', 700, { on: 'adoor', dx: 0, dy: -40 }); c.wait(300);
        // Comes back out of the dark carrying a crate that grows with it in perspective.
        c.set({ armF: -70, elbF: -40, armB: -60, elbB: -46, face: -1 });
        const carry = time => c.point('root', [10, -10], time);
        c.set({ crate: { x: carry(c.t)[0], y: carry(c.t)[1] }, 'crate.pop': 0.52 });
        c.to({ ghost: 1, 'crate.o': 1 }, 420);
        const start = c.t;
        c.par(a => a.gait(a.t, 1500, { arms: false }), a => a.to({ dz: 0, deep: 1 }, 1500, 'io'));
        c.at(start); c.path('crate', q => { const [x, y] = carry(start + q * 1500); return { x, y }; }, 1500, 'linear'); c.tw({ 'crate.pop': 1 }, 1500, 'io');
        c.at(start + 1500); c.attach('crate', 'root');
        c.par(a => a.walk(122, { speed: 26, arms: false }), a => { a.wait(400); a.to({ 'ashut.o': 1 }, 60); a.to({ 'ashut.roll': 1 }, 600, 'io'); a.to({ 'adoor.trim': 1 }, 400); a.to({ 'adoor.o': 0, 'ashut.o': 0 }, 360); });
        c.par(a => a.crouch(5, 360), a => a.to({ lean: 14 }, 360));
        c.detach('crate'); c.to({ crate: { y: 0 } }, 140, 'in');
        c.par(a => a.crouch(0, 300), a => a.to({ ...REST, lean: 0 }, 300)); c.turn(1);
        const box = c.pose('crate');
        c.set({ rubber: { x: box.x, y: -8 }, 'rubber.pop': 0.4 }); c.tw({ 'rubber.o': 1 }, 100);
        c.path('rubber', q => ({ x: box.x, y: -8 - 10 * q }), 420, 'back'); c.to({ 'rubber.pop': 1 }, 420, 'back');
        c.mood('happy'); c.say('ok', 700); c.mood('open');
        k.pause();
        k.clear(['crate', 'rubber']); k.leave();
      } },
    // Walks right up to the glass of the loader and knocks on it.
    glassknock: { kind: 'fun', weight: 2, minElapsedMs: 10000, compose: true,
      props: r => [{ id: 'fog', art: 'fogspot', layer: 'front', o: 0 }, { id: 'heart', art: 'heartline', layer: 'front', o: 0 }, { id: 'rip', art: 'ripple', layer: 'front', o: 0 },
        { id: 'toc', art: 'bubble', layer: 'front', text: 'TOC TOC!', tone: 'pow', o: 0 },
        { id: 'hi', art: 'bubble', layer: 'front', text: r.pick(['Ô DE CASA!', 'TÁ ME VENDO?']), o: 0 }],
      play(c, r, k) {
        k.arrive(100);
        c.look(0, 0, 200);
        c.par(a => a.gait(a.t, 1700), a => a.to({ dz: 30, deep: 1.85 }, 1700, 'io'));
        c.mood('happy'); c.wait(200);
        c.reach('F', c.point('root', [8, -34]), 260, 'out');
        const knuckle = c.point('handF');
        for (let i = 0; i < 2; i++) {
          c.reach('F', [knuckle[0] + 1.2, knuckle[1] - 1], 90, 'in'); c.reach('F', knuckle, 90, 'out');
          c.set({ rip: { x: knuckle[0], y: knuckle[1] }, 'rip.o': 0.9, 'rip.pop': 0.4 }); c.tw({ 'rip.o': 0, 'rip.pop': 2.4 }, 380, 'out'); c.wait(220);
        }
        c.say('toc', 480, { dx: 6, dy: -6 }); c.mood('open');
        c.rest(240); c.to({ lean: 6, head: 4 }, 300);
        const mouth = c.point('head', [6, -4]);
        c.set({ fog: { x: mouth[0] + 6, y: mouth[1] }, 'fog.pop': 0.3, heart: { x: mouth[0] + 6, y: mouth[1] } }); c.to({ 'fog.o': 0.6, 'fog.pop': 1 }, 500, 'out');
        c.to({ lean: 0, head: 0 }, 200);
        c.set({ 'heart.o': 1 });
        c.par(a => a.to({ 'heart.draw': 0 }, 900, 'io'), a => { for (const [dx, dy] of [[-4, -4], [0, -2], [4, -4], [0, 3]]) a.reach('F', [mouth[0] + 6 + dx, mouth[1] + dy], 225, 'io'); });
        c.rest(240); c.mood('happy'); c.say('hi', 640, { dx: 6, dy: -6 });
        c.reach('F', [mouth[0] + 2, mouth[1]], 200);
        for (let i = 0; i < 2; i++) c.reach('F', [mouth[0] + 11, mouth[1] + 1], 170).reach('F', [mouth[0] + 2, mouth[1] - 1], 170);
        c.tw({ 'fog.o': 0, 'heart.o': 0 }, 300); c.rest(260); c.mood('open');
        c.par(a => a.gait(a.t, 1600), a => a.to({ dz: 0, deep: 1 }, 1600, 'io'));
        c.look(2.4, 0, 200);
        k.pause();
        k.leave();
      } },
    // A bug runs in; the robot chases it with a net and catches itself instead.
    bugchase: { kind: 'fun', opener: true, weight: 2, pauses: false,
      props: r => [{ id: 'bugx', art: 'bug', x: 240, y: 0, layer: 'front' }, { id: 'net', art: 'net', layer: 'front' },
        { id: 'pega', art: 'bubble', layer: 'front', text: r.pick(['PEGA, PEGA!', 'VOLTA AQUI, BUG!']), tone: 'pow', o: 0 },
        { id: 'self', art: 'bubble', layer: 'front', text: r.pick(['ME PEGUEI!', 'OXE, ERREI!']), tone: 'hot', o: 0 }],
      play(c, r) {
        c.set({ rx: -44, armF: -150, elbF: -20, bugx: { x: 240, y: 0, sx: -1.4, sy: 1.4 } });
        const hand = c.point('handF'); c.set({ net: { x: hand[0], y: hand[1] } }); c.attach('net', 'handF');
        c.path('bugx', q => ({ x: 240 + (132 - 240) * q }), 1600, 'out');
        c.wait(500);
        c.par(a => a.walk(94, { speed: 76, period: 210, arms: false }), a => a.to({ lean: 10 }, 240));
        c.to({ lean: 0 }, 160); c.say('pega', 520);
        c.walk(112, { speed: 16, arms: false, period: 440 });
        c.to({ armF: -175, elbF: -6 }, 260, 'out');
        c.to({ armF: -30, elbF: -10, lean: 14 }, 180, 'in');
        c.set({ bugx: { sx: 1.4 } });
        c.path('bugx', q => ({ x: 132 + (-30 - 132) * q, y: -5 * Math.sin(Math.min(1, q * 3) * Math.PI) }), 1100, 'in');
        c.wait(250); c.mood('wide'); c.to({ lean: 0 }, 160); c.turn(-1, 200);
        c.to({ armF: -175, elbF: -6 }, 220, 'out'); c.to({ armF: -40, elbF: -10, lean: 10 }, 160, 'in');
        // loses balance, sits down hard and the net lands on its own head
        c.to({ ry: 15.4, legF: -88, legB: -84, kneeF: 4, kneeB: 8, lean: -14, armB: 40, elbB: -14 }, 260, 'in');
        c.detach('net'); const head = c.point('head', [0, -14]), from = c.pose('net');
        c.path('net', q => ({ x: from.x + (head[0] - 12.4 - from.x) * q, y: from.y + (head[1] + 12.4 - from.y) * q - 6 * Math.sin(Math.PI * q), r: 20 * q }), 360, 'in');
        c.wait(360); c.attach('net', 'head');
        c.say('self', 620, { dx: -8, dy: -20 }); c.mood('open');
        c.to({ ry: 5.6, legF: -38, kneeF: 74, legB: -34, kneeB: 76, lean: 16, ...REST }, 380); c.to({ ...STAND, ...REST }, 360);
        c.to({ armF: -170, elbF: -40 }, 260); c.detach('net'); c.grab('net'); c.to({ armF: -150, elbF: -20 }, 200);
        c.par(a => a.walk(-44, { speed: 70, period: 210, arms: false }), a => a.to({ lean: 10 }, 240));
      } },
    catpet: { kind: 'fun', weight: 2, minElapsedMs: 5000, compose: true,
      props: r => [{ id: 'kitty', art: 'catbot', x: 240, y: 0 },
        { id: 'hi', art: 'bubble', layer: 'front', text: r.pick(['OI, BICHANO!', 'VEM CÁ, GATIM!']), o: 0 },
        { id: 'love', art: 'bubble', layer: 'front', text: '♥', tone: 'love', o: 0 },
        { id: 'rrr', art: 'bubble', layer: 'front', text: 'RRRRR…', o: 0 }],
      play(c, r, k) {
        k.arrive(92);
        c.set({ kitty: { x: 240, y: 0, sx: -1.5, sy: 1.5 } });
        c.path('kitty', q => ({ x: 240 + (112 - 240) * q }), 2400, 'out');
        c.look(3.6, 0.6, 300); c.wait(1200); c.mood('happy'); c.say('hi', 600); c.wait(200);
        c.crouch(6, 360);
        c.reach('F', [106, -11], 280, 'io', { with: { lean: 14 } });
        c.par(a => { for (let i = 0; i < 3; i++) a.reach('F', [114, -10.6], 260, 'io', { with: { lean: 16 } }).reach('F', [106, -11], 260, 'io', { with: { lean: 14 } }); },
          a => { a.wait(300); a.say('love', 420, { on: 'kitty', dx: -6, dy: -12 }); a.say('rrr', 600, { on: 'kitty', dx: -6, dy: -14 }); },
          a => a.to({ 'kitty.tilt': -6 }, 400).to({ 'kitty.tilt': 4 }, 500).to({ 'kitty.tilt': 0 }, 400));
        c.par(a => a.crouch(0, 320), a => a.rest(320));
        c.set({ kitty: { sx: 1.5 } });
        c.path('kitty', q => ({ x: 112 + (240 - 112) * q }), 2200, 'in');
        c.wave(2); c.rest(260); c.mood('open');
        k.pause();
        k.leave();
      } },
    roombabump: { kind: 'fun', weight: 2, compose: true,
      props: r => [{ id: 'rb', art: 'roomba', x: 240, y: 0, layer: 'front' },
        { id: 'oxe', art: 'bubble', layer: 'front', text: r.pick(['OXE?', 'TÁ PERDIDO, É?']), o: 0 },
        { id: 'vai', art: 'bubble', layer: 'front', text: r.pick(['VAI, BICHIM!', 'PASSA, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(100);
        c.set({ rb: { x: 240, y: 0, sx: -1.2, sy: 1.2 } });
        c.path('rb', q => ({ x: 240 + (110 - 240) * q }), 1800, 'out'); c.wait(1300); c.look(3, 2, 300);
        c.at(c.t + 500);
        for (let i = 0; i < 3; i++) c.to({ rb: { x: 112 } }, 170, 'out').to({ rb: { x: 108.6 } }, 170, 'in');
        c.to({ head: 12 }, 200); c.say('oxe', 520); c.to({ head: 0 }, 160);
        c.walk(86, { face: false, speed: 22 });
        c.to({ rb: { x: 96 } }, 700, 'io');
        for (let i = 0; i < 2; i++) c.to({ rb: { x: 98 } }, 170, 'out').to({ rb: { x: 94.6 } }, 170, 'in');
        c.crouch(3, 160, 'out');
        c.par(a => a.to({ rx: 112 }, 560, 'io'), a => a.to({ fly: -12, kneeF: 30, kneeB: 30, legF: -16, legB: 10, ry: 0 }, 280, 'out').to({ fly: 0, kneeF: 0, kneeB: 0, legF: 0, legB: 0 }, 280, 'in'));
        c.path('rb', q => ({ x: 94.6 + (-30 - 94.6) * q }), 1800, 'in');
        c.turn(-1, 220); c.mood('happy'); c.say('vai', 560); c.wave(2); c.rest(240); c.mood('open'); c.turn(1, 220);
        k.pause();
        k.leave();
      } },
    umbrella: { kind: 'fun', weight: 2, minElapsedMs: 6000, compose: true,
      props: r => [{ id: 'cld', art: 'cloud', x: 106, y: -150 }, { id: 'rain', art: 'bitrain', x: 106, y: -80, o: 0 }, { id: 'umb', art: 'umb', layer: 'front', o: 0 },
        { id: 'vish', art: 'bubble', layer: 'front', text: r.pick(['VISH!', 'OXE, CHUVA?']), tone: 'hot', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['TEMPESTADE DE DADOS!', 'CHOVENDO BIT, MACHO!']), o: 0 }],
      play(c, r, k) {
        k.arrive(104);
        summon(c, 'cld', -84, 1200); c.look(2, -2, 300); c.to({ head: -8 }, 300); c.wait(600);
        c.set({ rain: { x: c.cur('rx') + 2, y: -82 } }); c.to({ 'rain.o': 1 }, 300);
        c.mood('wide'); c.to({ armF: -160, elbF: -100, armB: -150, elbB: -110, head: 6 }, 260, 'out'); c.say('vish', 420); c.mood('open');
        c.to({ armF: -150, elbF: -30, armB: REST.armB, elbB: REST.elbB, head: 0 }, 300);
        const hand = c.point('handF'); c.set({ umb: { x: hand[0], y: hand[1] + 10 }, 'umb.pop': 0.3 }); c.attach('umb', 'handF');
        c.to({ 'umb.o': 1, 'umb.pop': 1 }, 240, 'back'); c.to({ 'umb.open': 1 }, 360, 'back');
        for (let i = 0; i < 3; i++) c.to({ legF: -14, kneeF: 18 }, 160, 'out').to({ legF: 0, kneeF: 0 }, 160, 'in');
        c.say('ok', 760, { dx: 10, dy: -30 });
        k.pause();
        c.to({ 'rain.o': 0 }, 400); summon(c, 'cld', -150, 1000);
        c.to({ 'umb.open': 0.15 }, 300); c.to({ 'umb.o': 0, 'umb.pop': 0.3 }, 220); c.detach('umb'); c.rest(260);
        for (let i = 0; i < 4; i++) c.to({ lean: i % 2 ? -4 : 4, head: i % 2 ? 6 : -6 }, 70);
        c.to({ lean: 0, head: 0 }, 120);
        k.leave();
      } },
    satellite: { kind: 'work', families: ['access', 'reading'], weight: 4, compose: true,
      props: r => [{ id: 'dish', art: 'dish', x: 126, set: true }, { id: 'sig', art: 'sigpanel', x: 102, set: true },
        { id: 'no', art: 'bubble', layer: 'front', text: r.pick(['SEM SINAL, VISH…', 'CADÊ O SINAL?']), o: 0 },
        { id: 'oxe', art: 'bubble', layer: 'front', text: 'OXE!', tone: 'hot', o: 0 },
        { id: 'ok', art: 'bubble', layer: 'front', text: r.pick(['PEGOU O SINAL!', '4 PAUZINHOS!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        k.arrive(84); k.bring(['dish', 'sig']);
        c.look(3.4, -0.6); c.to({ head: 6 }, 200); c.say('no', 600); c.to({ head: 0 }, 160);
        c.walk(114, { speed: 22 });
        const bars = n => Object.fromEntries([1, 2, 3, 4].map(i => [`sig.s${i}`, i <= n ? 1 : 0]));
        const turnTo = (aim, n, length = 600) => c.par(a => a.to({ 'dish.aim': aim }, length, 'io'), a => a.reach('F', [126 + 4 * Math.cos(rad(aim - 40)), -17 + 6 * Math.sin(rad(aim - 40))], length, 'io', { with: { lean: 6 } }), a => { a.wait(length * 0.6); a.set(bars(n)); });
        c.reach('F', [128, -22], 300, 'io', { with: { lean: 6 } });
        turnTo(0, 2); c.look(-2, -0.6, 200); c.wait(300); c.look(3, -0.6, 200);
        turnTo(40, 1, 500); c.mood('wide'); c.say('oxe', 380); c.mood('open');
        k.pause();
        c.reach('F', [128, -22], 300, 'io', { with: { lean: 6 } });
        turnTo(16, 4, 700); c.rest(240);
        c.walk(96, { face: false, speed: 22 }); c.turn(1); c.look(3, -0.8); c.mood('happy'); c.say('ok', 620); c.mood('open');
        c.walk(84, { face: false, speed: 22 });
        k.clear(['dish', 'sig']); k.leave();
      } },
    // ---------------------------------------------------------------- tech set
    hacker: { kind: 'work', families: ['reading', 'verification', 'calculation'], weight: 5,
      props: r => [{ id: 'chair', art: 'chair', x: -44 }, { id: 'desk', art: 'pcdesk', x: -20 }, { id: 'screen', art: 'dash', x: 102, y: -165 },
        { id: 'got', art: 'bubble', layer: 'front', text: r.pick(['ACHEI O CABRA!', 'PEGUEI, MACHO!']), tone: 'pow', o: 0 }],
      play(c, r, k) {
        const grip = [-7.2, -22.5];
        c.set({ rx: -44 + grip[0] - 10 }); c.attach('desk', 'chair');
        c.grip('chair', grip, { lean: 8 });
        c.haul(['chair'], 74 + grip[0] - 10, { speed: 30 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        const grip = [-24.6, -30];
        c.set({ rx: -26 + grip[0] - 10 });
        c.grip('board', grip, { lean: 6 });
        c.haul(['board'], 112 + grip[0] - 10, { speed: 30 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        const grip = [-20.3, -19];
        c.set({ rx: -28 + grip[0] - 10 });
        c.grip('crew', grip, { lean: 6 });
        c.haul(['crew'], 84 + grip[0] - 10, { speed: 28 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
        const grip = [-12.8, -27];
        c.set({ rx: -14 + grip[0] - 10 });
        c.grip('rack', grip, { lean: 6 });
        c.haul(['rack'], 108 + grip[0] - 10, { speed: 30 });
        c.rest(300); k.pause();
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
      play(c, r, k) {
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
        k.pause();
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
    Object.freeze({ kind: skit.kind, families: Object.freeze([...(skit.families || [])]), weight: skit.weight, minElapsedMs: skit.minElapsedMs || 0, opener: !!skit.opener, composed: !!skit.compose })])));
  // The interchangeable pieces, for previews and tests.
  const PIECES = Object.freeze({ arrivals: Object.freeze(ARRIVALS.slice()), departures: Object.freeze(DEPARTURES.slice()), breaks: Object.freeze(Object.keys(BREAKS)),
    extras: Object.freeze(Object.keys(EXTRAS)), deliveries: Object.freeze(Object.keys(DELIVER)) });

  const SCENE_LIMIT = 32000;
  function compile(name, { seed = 1, parts } = {}) {
    const skit = name === 'still' ? STILL : SKITS[name];
    if (!skit) throw Error(`Cena desconhecida: ${name}`);
    const r = tools(seeded(seed));
    const own = typeof skit.props === 'function' ? skit.props(r) : skit.props;
    if (name === 'still') { const clip = createClip(own); skit.play(clip.api, r); return { name, plan: null, ...clip.finish() }; }
    const asked = parts || {};
    const build = plan => {
      const rr = tools(seeded(`${seed}:${name}`));
      const own2 = typeof skit.props === 'function' ? skit.props(rr) : skit.props;
      const { props, homes } = planProps(own2, plan, rr);
      const clip = createClip(props), k = kit(clip.api, rr, plan, homes);
      skit.play(clip.api, rr, k);
      k.extras();
      return { name, plan: { ...plan, extras: plan.extras.slice() }, ...clip.finish() };
    };
    // A scene stays short enough to watch whole: a long combination first loses its
    // break, then trades its arrival, departure and delivery for quick ones.
    let plan = choosePlan(skit, r, asked), clip = build(plan);
    const keep = (key, value) => key in asked ? asked[key] : value;
    if (clip.duration > SCENE_LIMIT) clip = build(plan = { ...plan, brk: keep('brk', null) });
    if (clip.duration > SCENE_LIMIT && skit.compose) clip = build({ ...plan, arrive: keep('arrive', 'walk'), leave: keep('leave', 'walk'), deliver: keep('deliver', 'beam') });
    return clip;
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
    <path class="wv-seam" d="M47 40V146M153 40V146M71 50V146M129 50V146M22 113h49M129 113h49M71 76h58"/>
    <circle class="wv-vault" cx="100" cy="106" r="26"/><circle class="wv-vault wv-vault-in" cx="100" cy="106" r="20"/><circle class="wv-vault-eye" cx="100" cy="106" r="1.1"/>
    <circle class="wv-wall-led" cx="59" cy="108" r=".6"/><circle class="wv-wall-led wv-wall-led-b" cx="141" cy="108" r=".6"/>
    <path class="wv-lamp" d="M82 47h36"/>
    <path class="wv-grid" d="M100 146 70 200M100 146 130 200M100 146 30 200M100 146 170 200M0 162h200M0 182h200"/><path class="wv-horizon" d="M0 146h200"/><path class="wv-horizon-glow" d="M66 146h68"/>
    <ellipse class="wv-floor-sheen" cx="100" cy="153" rx="46" ry="5"/>
    <g transform="translate(0 146)"><g class="wv-mirror"><g class="wv-actors"></g></g></g>
    <g class="wv-motes"><circle cx="90" cy="72" r=".45"/><circle cx="109" cy="88" r=".35"/><circle cx="97" cy="104" r=".4"/><circle cx="113" cy="64" r=".3"/></g>
  </svg><span class="wv-fog"></span><span class="wv-scan"></span><span class="wv-glass"></span></div><span class="wv-timer"></span>`;

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
    function play(name, flip, seed, parts) {
      clearScene();
      const clip = compile(name, { seed, parts });
      actors.innerHTML = clip.markup;
      mirrorGroup.setAttribute('transform', flip ? 'matrix(-1 0 0 1 200 0)' : 'matrix(1 0 0 1 0 0)');
      mirrored = flip; root.dataset.mirror = String(flip); root.dataset.skit = name; skit = name; mode = 'skit';
      root.dataset.parts = clip.plan ? [clip.plan.arrive, clip.plan.brk, clip.plan.leave, ...clip.plan.extras].filter(Boolean).join(' ') : '';
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
      // The line stays in the layout while empty, so hosts never jump when counts arrive.
      metric.dataset.empty = String(!current.metric);
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
      preview(name, { mirror = false, seed = 1, parts } = {}) { if (supported && !destroyed) { clock?.cancel(); clock = null; play(name, mirror, seed, parts); } } });
  }
  return Object.freeze({ derive, mount, compile, createDirector, createElapsedClock, skits: SKIT_INFO, pieces: PIECES });
})();
