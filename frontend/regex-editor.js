/* Free-text expression editor with a visual layer. The textarea is the single
 * source of truth: native caret, selection, IME, clipboard and undo stack are
 * never replaced. The layer below it is rebuilt from RegexSyntax on every change,
 * so decoration can never move the caret. Assisted actions are applied as one
 * minimal replacement through the browser's own edit command, which keeps each
 * action a single undo step. */
window.RegexEditor = (() => {
  "use strict";
  const SLOTS = 8, S = () => window.RegexSyntax;
  const esc = (t) => String(t).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const digitsCircle = (n) => String(n);

  /** Keeps a capture's colour while the text around it changes. */
  class SlotMap {
    constructor() { this.items = []; this.cursor = 0; }
    update(previous, next, captures) {
      const moved = [];
      if (previous != null && this.items.length) {
        const d = S().diff(previous, next), shift = d.text.length - (d.end - d.start);
        for (const item of this.items) {
          if (item.open < d.start) moved.push({ ...item });
          else if (item.open >= d.end) moved.push({ ...item, open: item.open + shift });
        }
      }
      const used = new Set(), assigned = [];
      for (const g of captures) {
        const hit = moved.find((m) => m.open === g.start && !used.has(m.slot));
        if (hit) { used.add(hit.slot); assigned.push({ id: g.id, open: g.start, slot: hit.slot }); }
        else assigned.push({ id: g.id, open: g.start, slot: null });
      }
      for (const a of assigned) {
        if (a.slot != null) continue;
        let slot = 0; while (slot < SLOTS && used.has(slot)) slot++;
        if (slot >= SLOTS) slot = this.cursor++ % SLOTS;
        used.add(slot); a.slot = slot;
      }
      this.items = assigned.map((a) => ({ open: a.open, slot: a.slot }));
      this.byId = new Map(assigned.map((a) => [a.id, a.slot]));
    }
    slotOf(id) { return this.byId?.get(id) ?? 0; }
  }

  function create(host, options = {}) {
    const root = document.createElement("div");
    root.className = "rx-editor";
    root.innerHTML = `<div class="rx-field"><pre class="rx-backdrop" aria-hidden="true"><code></code></pre><textarea class="rx-input" spellcheck="false" autocomplete="off" autocapitalize="off" autocorrect="off" rows="2"></textarea></div><div class="rx-legend" role="list" aria-label="Grupos de captura"></div><div class="rx-info" role="status" aria-live="polite"></div><div class="rx-tip" role="tooltip" hidden></div>`;
    host.append(root);
    const field = root.querySelector(".rx-field"), code = root.querySelector("code"), input = root.querySelector("textarea");
    const legend = root.querySelector(".rx-legend"), info = root.querySelector(".rx-info"), tip = root.querySelector(".rx-tip");
    input.setAttribute("aria-label", options.label || "Expressão");
    input.placeholder = options.placeholder || "";
    const slots = new SlotMap(), listeners = { change: [], analysis: [], selection: [] };
    let analysis = S().analyze(""), previousText = null, extended = false, focusGroup = null, nativeMark = null, observed = new Map(), notices = [], composing = false, destroyed = false, hover = null;
    const emit = (name, ...args) => { for (const fn of listeners[name]) fn(...args); };
    const on = (name, fn) => { listeners[name].push(fn); return () => { listeners[name] = listeners[name].filter((f) => f !== fn); }; };

    function metrics() {
      const lines = input.value.split("\n").length, lineHeight = parseFloat(getComputedStyle(input).lineHeight) || 20;
      const rows = Math.max(options.minRows || 2, Math.min(options.maxRows || 8, lines));
      field.style.height = `${Math.ceil(rows * lineHeight + 18)}px`;
    }
    function syncScroll() { code.parentElement.scrollTop = input.scrollTop; code.parentElement.scrollLeft = input.scrollLeft; }

    function analyze() {
      const text = input.value;
      const next = S().analyze(text, { extended });
      const oldRefs = analysis.references.filter((r) => r.refKind === "numbered" && r.valid && !r.octal && r.targets.length === 1).map((r) => ({ start: r.start, slot: slots.byId?.get(r.targets[0]), text: r.syntax }));
      slots.update(previousText, text, next.captures);
      notices = [];
      if (previousText != null && previousText !== text && oldRefs.length) {
        const d = S().diff(previousText, text), shift = d.text.length - (d.end - d.start);
        for (const old of oldRefs) {
          const start = old.start < d.start ? old.start : old.start >= d.end ? old.start + shift : -1;
          const now = next.references.find((r) => r.start === start && r.refKind === "numbered" && r.valid && r.targets.length === 1);
          if (now && slots.slotOf(now.targets[0]) !== old.slot) {
            const g = next.groups[now.targets[0]];
            notices.push(`A numeração mudou: a referência ${text.slice(now.start, now.end)} agora aponta para o grupo ${g.index}${g.name ? ` (${g.name})` : ""}. A expressão não foi alterada; revise-a.`);
          }
        }
      }
      previousText = text;
      analysis = next;
      render();
      emit("analysis", analysis);
    }

    function segmentHtml() {
      const text = analysis.text;
      const segs = S().segments(analysis, { slotOf: (id) => slots.slotOf(id), focus: focusGroup });
      let html = "";
      for (const s of segs) {
        let cls = s.classes.join(" ");
        const bad = nativeMark && s.start < nativeMark.end && s.end > nativeMark.start;
        if (bad) cls += nativeMark.stale ? " rx-native-stale" : " rx-native-error";
        const group = [...s.layers].reverse().find((l) => l.data?.group != null)?.data.group;
        const ref = [...s.layers].reverse().find((l) => l.data?.ref != null)?.data.ref;
        html += `<span class="rx-s ${esc(cls)}" data-s="${s.start}" data-e="${s.end}"${group != null ? ` data-g="${group}"` : ""}${ref != null ? ` data-r="${ref}"` : ""}>${esc(text.slice(s.start, s.end))}</span>`;
      }
      if (!text.length) return "";
      return html + (text.endsWith("\n") ? " " : "");
    }
    function renderLegend() {
      const caps = analysis.captures.filter((g) => !g.autoNoncapture);
      legend.hidden = !caps.length;
      const show = caps.slice(0, 16);
      legend.innerHTML = show.map((g) => {
        const refs = analysis.refsByGroup.get(g.id)?.length || 0, value = observed.get(g.index) ?? (g.name != null ? observed.get(g.name) : undefined);
        const neutral = g.uncertain ? " rx-chip-neutral" : "";
        return `<span role="listitem" class="rx-li"><button type="button" class="rx-chip rx-c-${slots.slotOf(g.id)}${neutral}" data-g="${g.id}" title="${esc(`Ir ao grupo ${g.index}${g.name ? ` (${g.name})` : ""}`)}"><b>${g.index}</b>${g.name ? `<span>${esc(g.name)}</span>` : ""}${g.insideLookaround ? '<i class="rx-chip-look" title="Dentro de uma condição de contexto">ctx</i>' : ""}${refs ? `<i>${refs} ref.</i>` : ""}${value !== undefined ? `<em title="Valor observado na amostra atual">${esc(String(value).slice(0, 24))}${String(value).length > 24 ? "…" : ""}</em>` : ""}</button></span>`;
      }).join("") + (caps.length > show.length ? `<span class="rx-more">+${caps.length - show.length} grupos</span>` : "");
      legend.querySelectorAll("[data-g]").forEach((b) => (b.onclick = () => selectGroup(+b.dataset.g)));
    }
    function infoText() {
      const parts = [];
      const caret = input.selectionStart === input.selectionEnd ? S().at(analysis, input.selectionStart) : { group: null, ref: null };
      const h = hover || caret;
      const d = S().describe(analysis, h);
      if (d) parts.push(d);
      for (const n of notices) parts.push(n);
      if (nativeMark?.message && !nativeMark.stale) parts.push(nativeMark.message);
      if (!d && !notices.length && analysis.problems.length) parts.push(`${analysis.problems[0].message} (indicação do assistente; o PCRE2 decide a validade)`);
      if (analysis.uncertain.some((u) => u.start === 0 && u.end === analysis.text.length)) parts.push(analysis.uncertain.find((u) => u.start === 0 && u.end === analysis.text.length).reason + " Essas relações ficam sem cor.");
      return parts.join(" ");
    }
    function renderInfo() {
      info.textContent = infoText();
      info.dataset.problem = String(!!(analysis.problems.length || nativeMark || notices.length));
    }
    function render() {
      code.innerHTML = segmentHtml();
      renderLegend();
      renderInfo();
      metrics();
      syncScroll();
    }

    function setFocusFromCaret() {
      const hit = input.selectionStart === input.selectionEnd ? S().at(analysis, input.selectionStart) : { group: null, ref: null };
      const g = hit.ref ? (hit.ref.targets.length === 1 ? hit.ref.targets[0] : null) : hit.group && hit.group.kind === "capture" ? hit.group.id : null;
      if (g !== focusGroup) { focusGroup = g; code.innerHTML = segmentHtml(); syncScroll(); }
      renderInfo();
      emit("selection", selection());
    }
    function selectGroup(id) {
      const g = analysis.groups[id];
      if (!g) return;
      input.focus();
      input.setSelectionRange(g.start, g.end ?? input.value.length);
      setFocusFromCaret();
    }
    /** Move between a group's definition and its references (Alt+↓ / Alt+↑). */
    function cycle(direction) {
      const caret = input.selectionStart, hit = S().at(analysis, caret);
      let group = hit.group?.kind === "capture" ? hit.group : hit.ref?.targets.length === 1 ? analysis.groups[hit.ref.targets[0]] : null;
      if (!group) return false;
      const stops = [{ start: group.start, end: group.end }, ...(analysis.refsByGroup.get(group.id) || []).map((id) => analysis.references[id])].sort((a, b) => a.start - b.start);
      let at = stops.findIndex((s) => caret >= s.start && caret <= s.end);
      at = (at + direction + stops.length) % stops.length;
      input.setSelectionRange(stops[at].start, stops[at].end);
      setFocusFromCaret();
      return true;
    }

    /** One replacement, recorded as one undo step by the browser. */
    function replace(start, end, text, { select = null } = {}) {
      input.focus();
      input.setSelectionRange(start, end);
      let done = false;
      try { done = document.execCommand("insertText", false, text); } catch { done = false; }
      if (!done || input.value.slice(start, start + text.length) !== text) {
        input.setRangeText(text, start, end, "end");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      if (select) input.setSelectionRange(select[0], select[1]);
      setFocusFromCaret();
    }
    /** Turn the current text into `next` with the smallest single edit. */
    function apply(next, { select = null } = {}) {
      const d = S().diff(input.value, next);
      if (d.start === d.end && !d.text) return false;
      replace(d.start, d.end, d.text, { select: select ? [select[0], select[1]] : null });
      return true;
    }
    function setValue(value, { silent = true } = {}) {
      const text = String(value ?? "");
      if (input.value === text) return;
      input.value = text;
      previousText = null;
      slots.items = [];
      analyze();
      if (!silent) emit("change", text, { source: "api" });
    }
    function selection() {
      const start = input.selectionStart, end = input.selectionEnd;
      return { start, end, text: input.value.slice(start, end), collapsed: start === end, wrap: start === end ? null : S().wrappable(analysis, start, end), active: document.activeElement === input };
    }

    input.addEventListener("input", () => {
      hover = null; nativeMark = nativeMark ? { ...nativeMark, stale: true } : null;
      analyze();
      setFocusFromCaret();
      emit("change", input.value, { source: "user" });
    });
    input.addEventListener("scroll", syncScroll);
    for (const type of ["keyup", "click", "focus"]) input.addEventListener(type, () => { if (!composing) setFocusFromCaret(); });
    input.addEventListener("select", () => { setFocusFromCaret(); });
    input.addEventListener("blur", () => { emit("selection", { ...selection(), active: false }); });
    input.addEventListener("compositionstart", () => { composing = true; });
    input.addEventListener("compositionend", () => { composing = false; setFocusFromCaret(); });
    input.addEventListener("keydown", (e) => {
      if (e.altKey && (e.key === "ArrowDown" || e.key === "ArrowUp") && cycle(e.key === "ArrowDown" ? 1 : -1)) { e.preventDefault(); return; }
      if (e.key === "Enter" && !e.shiftKey && !(e.ctrlKey || e.metaKey) && options.singleLine) { e.preventDefault(); options.onSubmit?.(); return; }
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); options.onSubmit?.(); }
    });
    // Hover: the layer below the textarea answers which construct the pointer is over.
    field.addEventListener("pointermove", (e) => {
      const span = document.elementsFromPoint(e.clientX, e.clientY).find((n) => n.classList?.contains("rx-s"));
      const g = span?.dataset.g != null ? analysis.groups[+span.dataset.g] : null, r = span?.dataset.r != null ? analysis.references[+span.dataset.r] : null;
      const next = r ? { ref: r, group: null } : g ? { group: g, ref: null } : null;
      const text = next ? S().describe(analysis, next) : "";
      if (!text) { if (hover) { hover = null; renderInfo(); } tip.hidden = true; return; }
      hover = next; renderInfo();
      const box = root.getBoundingClientRect();
      tip.textContent = text; tip.hidden = false;
      tip.style.left = `${Math.max(0, Math.min(e.clientX - box.left, box.width - 240))}px`;
      tip.style.top = `${e.clientY - box.top + 18}px`;
    });
    field.addEventListener("pointerleave", () => { hover = null; tip.hidden = true; renderInfo(); });
    const resize = typeof ResizeObserver === "function" ? new ResizeObserver(() => { if (!destroyed) metrics(); }) : null;
    resize?.observe(root);
    setValue(options.value || "");

    return {
      root, input, on,
      get value() { return input.value; },
      get analysis() { return analysis; },
      slotOf: (id) => slots.slotOf(id),
      selection, replace, apply, setValue, selectGroup, cycle,
      focus: () => input.focus(),
      setExtended(value) { extended = !!value; analyze(); },
      /** Native diagnostic as a UTF-16 range; `null` clears it. */
      markNative(range) { nativeMark = range ? { start: range.start, end: Math.max(range.end, range.start + 1), message: range.message, stale: false } : null; render(); },
      /** Values seen in the current sample, by group number and name. */
      setObserved(map) { observed = map || new Map(); renderLegend(); },
      setDisabled(value) { input.disabled = !!value; root.dataset.disabled = String(!!value); },
      destroy() { destroyed = true; resize?.disconnect(); root.remove(); },
    };
  }
  return { create, SLOTS };
})();
