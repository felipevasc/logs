/* Pure helpers of the extraction workbench. Nothing here executes Grok or PCRE2: patterns
 * are built and displayed as text, and only the native engine ever runs them. Positions are
 * UTF-16 offsets, the same unit the native engine reports. */
window.ExtractionModel = (() => {
  "use strict";
  const PALETTE = ["#4f9dde", "#e0813f", "#58b368", "#c76bd0", "#d4b43a", "#e0607e", "#3fb8af", "#8b83e8", "#a9b94a", "#d98f6b", "#5b8fa8", "#b06f9a"];
  /** The same name always gets the same colour (an explicit choice in `ui.colors` wins). */
  function fieldColor(name, overrides = {}) {
    if (overrides && /^#[0-9a-f]{6}$/i.test(overrides[name] || "")) return overrides[name];
    let hash = 2166136261;
    for (let i = 0; i < name.length; i++) hash = Math.imul(hash ^ name.charCodeAt(i), 16777619) >>> 0;
    return PALETTE[hash % PALETTE.length];
  }
  const isDigit = (c) => c >= "0" && c <= "9";
  const isWordChar = (c) => c === "_" || isDigit(c) || c.toLowerCase() !== c.toUpperCase();
  const isSpace = (c) => c === " " || c === "\t" || c === "\n" || c === "\r" || c === "\f" || c === "\v";

  /** Tokens of a Grok expression as editable chips: `%{PATTERN:field:conv}` or literal text. */
  function chips(text) {
    const out = [];
    let literal = "", literalStart = 0, i = 0;
    const flush = (end) => {
      if (literal) out.push({ type: "literal", text: literal, start: literalStart, end });
      literal = "";
    };
    while (i < text.length) {
      if (text[i] === "\\" && i + 1 < text.length) {
        if (!literal) literalStart = i;
        literal += text.slice(i, i + 2);
        i += 2;
        continue;
      }
      if (text[i] === "%" && text[i + 1] === "{") {
        const close = text.indexOf("}", i + 2);
        if (close > 0) {
          const [pattern, field, conv] = text.slice(i + 2, close).split(":");
          flush(i);
          out.push({ type: "token", pattern: pattern || "", field: field || "", conv: conv || "", start: i, end: close + 1 });
          i = close + 1;
          continue;
        }
        if (!literal) literalStart = i;
        literal += text.slice(i);
        i = text.length;
        out.push({ type: "literal", text: literal, start: literalStart, end: i, broken: true });
        literal = "";
        continue;
      }
      if (!literal) literalStart = i;
      literal += text[i++];
    }
    flush(text.length);
    return out;
  }
  const chipText = (chip) =>
    chip.type === "literal" ? chip.text : `%{${chip.pattern}${chip.field ? `:${chip.field}` : ""}${chip.conv ? `:${chip.conv}` : ""}}`;
  const fromChips = (list) => list.map(chipText).join("");
  const fieldsOf = (text) => chips(text).filter((c) => c.type === "token" && c.field).map((c) => c.field);

  /** A catalog pattern for text the user selected, by looking at its characters only. */
  function classify(text) {
    if (!text) return "DATA";
    let digits = 0, dots = 0, word = 0, space = 0;
    for (const c of text) {
      if (isDigit(c)) digits++;
      else if (c === "." || c === ",") dots++;
      else if (isWordChar(c)) word++;
      else if (isSpace(c)) space++;
    }
    const length = [...text].length;
    if (digits === length) return "INT";
    if (digits + dots === length && dots === 1 && isDigit(text[0]) && isDigit(text[text.length - 1])) return "NUMBER";
    if (digits + word === length) return "WORD";
    if (space === 0) return "NOTSPACE";
    return "DATA";
  }
  const LIT_SPECIAL = "\\.^$|?*+()[]{}";
  /** Literal text for a Grok expression. `variation` lets spaces repeat and digits change. */
  function literal(text, variation = false) {
    let out = "", i = 0;
    while (i < text.length) {
      const c = text[i];
      if (isSpace(c)) {
        let j = i;
        while (j < text.length && isSpace(text[j])) j++;
        out += variation || j - i > 1 ? "\\s+" : c === " " ? " " : "\\s";
        i = j;
      } else if (variation && isDigit(c)) {
        let j = i;
        while (j < text.length && isDigit(text[j])) j++;
        out += "%{INT}";
        i = j;
      } else {
        out += LIT_SPECIAL.includes(c) ? `\\${c}` : c;
        i++;
      }
    }
    return out;
  }
  /** A field name taken from what stands before the value (`user=`, `usuario:`), or a default. */
  function nameBefore(text, start) {
    let i = start;
    while (i > 0 && (text[i - 1] === "=" || text[i - 1] === ":" || text[i - 1] === '"' || text[i - 1] === " ")) i--;
    let j = i;
    while (j > 0 && isWordChar(text[j - 1])) j--;
    return clean(text.slice(j, i));
  }
  function clean(name) {
    let out = "";
    for (const c of String(name || "").normalize("NFD")) if (c.charCodeAt(0) < 0x300 || c.charCodeAt(0) > 0x36f) out += isWordChar(c) ? c : "_";
    out = out.replace(/^_+|_+$/g, "").toLowerCase();
    if (/^\d/.test(out)) out = `campo_${out}`;
    return out.slice(0, 64);
  }
  function uniqueName(base, taken) {
    const root = clean(base) || "campo";
    if (!taken.has(root)) return root;
    for (let n = 2; n < 1000; n++) if (!taken.has(`${root}_${n}`)) return `${root}_${n}`;
    return `${root}_x`;
  }

  /* --- segments: the visual selections made on one sample, turned into a Grok pattern --- */
  const overlaps = (a, b) => a.start < b.end && b.start < a.end;
  function addSegment(segments, segment) {
    if (segment.start >= segment.end || segment.start < 0) return { error: "Selecione um trecho do texto." };
    const clash = segments.find((s) => overlaps(s, segment));
    if (clash) return { error: "Este trecho se sobrepõe a outro já marcado. Remova a marcação anterior primeiro.", clash };
    return { segments: [...segments, segment].sort((a, b) => a.start - b.start) };
  }
  const removeSegment = (segments, index) => segments.filter((_, i) => i !== index);
  /** The expression for the segments, over the sample they were selected in. */
  function compile(sample, segments) {
    let out = "", cursor = 0;
    const sorted = [...segments].sort((a, b) => a.start - b.start);
    sorted.forEach((s, index) => {
      const gap = sample.slice(cursor, s.start);
      // Text before the first marked piece is not part of the pattern: the engine searches for it.
      const gapText = index === 0 && cursor === 0 ? "" : literal(gap, s.variation);
      const body =
        s.kind === "literal" ? literal(sample.slice(s.start, s.end), s.variation)
        : s.kind === "ignore" ? `%{${s.token || "DATA"}}`
        : `%{${s.token || classify(sample.slice(s.start, s.end))}${s.name ? `:${s.name}` : ""}${s.conv ? `:${s.conv}` : ""}}`;
      out += s.optional ? `(?:${gapText}${body})?` : gapText + body;
      cursor = s.end;
    });
    return out;
  }
  /** Where a value that starts at `start` ends when nothing else was said: at the next separator. */
  function untilSeparator(text, start) {
    let i = start;
    while (i < text.length && !isSpace(text[i]) && !",;|)]}\"'".includes(text[i])) i++;
    return i;
  }
  /** Segments added by a visual action on the selection {start, end}. */
  function apply(sample, current, action, selection, options = {}) {
    const taken = new Set(current.filter((s) => s.name).map((s) => s.name));
    const { start, end } = selection;
    const picked = sample.slice(start, end);
    const named = (hint, at) => uniqueName(options.name || hint || nameBefore(sample, at) || "campo", taken);
    switch (action) {
      case "extract":
        return addSegment(current, { kind: "field", start, end, token: options.token || classify(picked), name: named("", start), conv: options.conv || "" });
      case "after": {
        // The selected text is a marker; the value is what follows it, up to the next separator.
        const stop = untilSeparator(sample, end);
        if (stop <= end) return { error: "Não há valor depois deste marcador." };
        const marker = addSegment(current, { kind: "literal", start, end, variation: !!options.variation });
        if (marker.error) return marker;
        return addSegment(marker.segments, { kind: "field", start: end, end: stop, token: classify(sample.slice(end, stop)), name: named("", start) });
      }
      case "before": {
        let from = start;
        while (from > 0 && !isSpace(sample[from - 1]) && !",;|([{\"'".includes(sample[from - 1])) from--;
        if (from >= start) return { error: "Não há valor antes deste marcador." };
        const value = addSegment(current, { kind: "field", start: from, end: start, token: classify(sample.slice(from, start)), name: named("", from) });
        if (value.error) return value;
        return addSegment(value.segments, { kind: "literal", start, end, variation: !!options.variation });
      }
      case "between": {
        const { open, close } = options;
        if (!open || !close) return { error: "Informe o marcador inicial e o final." };
        const a = sample.indexOf(open, start), b = a < 0 ? -1 : sample.indexOf(close, a + open.length);
        if (a < 0 || b < 0) return { error: "Os marcadores não foram encontrados nesta amostra." };
        const first = addSegment(current, { kind: "literal", start: a, end: a + open.length });
        if (first.error) return first;
        const value = addSegment(first.segments, { kind: "field", start: a + open.length, end: b, token: "DATA", name: named("", a) });
        if (value.error) return value;
        return addSegment(value.segments, { kind: "literal", start: b, end: b + close.length });
      }
      case "ignore":
        return addSegment(current, { kind: "ignore", start, end, token: classify(picked) === "DATA" ? "DATA" : classify(picked) });
      case "fixed":
        return addSegment(current, { kind: "literal", start, end });
      case "variation": {
        const at = current.findIndex((s) => s.start <= start && s.end >= end);
        if (at < 0) return addSegment(current, { kind: "literal", start, end, variation: true });
        return { segments: current.map((s, i) => (i === at ? { ...s, variation: !s.variation } : s)) };
      }
      case "optional": {
        const at = current.findIndex((s) => s.start <= start && s.end >= end);
        if (at < 0) return { error: "Marque o trecho como campo ou texto fixo antes de torná-lo opcional." };
        return { segments: current.map((s, i) => (i === at ? { ...s, optional: !s.optional } : s)) };
      }
      case "recognize":
        return addSegment(current, { kind: "field", start, end, token: options.token || classify(picked), name: named(options.hint, start), conv: "" });
      default:
        return { error: "Ação desconhecida." };
    }
  }
  /** What a delimiter selection means for a delimited extraction. */
  function delimiterOf(sample, selection) {
    const text = sample.slice(selection.start, selection.end);
    return text.length >= 1 && text.length <= 8 ? text : null;
  }

  /* --- definitions --- */
  const blank = (id, title) => ({ id, version: "1", title, schema: 2, rules: [], enabled: true });
  const blankRule = (id) => ({ id, selector: {}, priority: 0, fields: {} });
  const outputCount = (def) => def.rules.reduce((n, r) => n + Object.keys(r.fields || {}).length, 0);
  function uniqueRuleId(def, base = "regra") {
    const taken = new Set(def.rules.map((r) => r.id));
    for (let n = 1; n < 1000; n++) if (!taken.has(`${base}-${n}`)) return `${base}-${n}`;
    return `${base}-x`;
  }
  function ensureOutput(rule, name, from, type) {
    rule.fields = rule.fields || {};
    if (!rule.fields[name]) rule.fields[name] = { from: [from], ...(type ? { type } : {}) };
    return rule.fields[name];
  }
  /** Type suggested by the pattern an extracted field used: recognising is not converting, so
   * this only pre-selects what the user can change. */
  function typeForToken(token, conv) {
    if (conv === "int") return "integer";
    if (conv === "float") return "decimal";
    return ({ INT: "integer", POSINT: "integer", NONNEGINT: "integer", NUMBER: "decimal", BASE10NUM: "decimal", IP: "ip", IPV4: "ip", IPV6: "ip" })[token] || "text";
  }
  /** Keeps `ui` (colours, notes) out of every comparison that decides whether values change. */
  function semantic(def) {
    const copy = structuredClone(def);
    delete copy.ui;
    delete copy.title;
    delete copy.description;
    for (const rule of copy.rules || []) {
      delete rule.description;
      for (const mapping of Object.values(rule.fields || {})) {
        delete mapping.description;
        delete mapping.index;
      }
    }
    return JSON.stringify(copy);
  }
  class History {
    constructor(limit = 100) { this.limit = limit; this.past = []; this.future = []; this.current = null; }
    reset(value) { this.past = []; this.future = []; this.current = value; }
    push(value) {
      if (value === this.current) return false;
      if (this.current != null) this.past.push(this.current);
      if (this.past.length > this.limit) this.past.shift();
      this.current = value;
      this.future = [];
      return true;
    }
    undo() { if (!this.past.length) return null; this.future.push(this.current); this.current = this.past.pop(); return this.current; }
    redo() { if (!this.future.length) return null; this.past.push(this.current); this.current = this.future.pop(); return this.current; }
    get canUndo() { return this.past.length > 0; }
    get canRedo() { return this.future.length > 0; }
  }
  /** Parts of a text to paint: non-overlapping ranges, each tagged with the field it holds. */
  function paintPlan(text, ranges) {
    const sorted = ranges.filter((r) => r.end > r.start).sort((a, b) => a.start - b.start || b.end - a.end);
    const parts = [];
    let cursor = 0;
    for (const r of sorted) {
      if (r.start < cursor) continue;
      if (r.start > cursor) parts.push({ text: text.slice(cursor, r.start) });
      parts.push({ text: text.slice(r.start, r.end), field: r.field });
      cursor = r.end;
    }
    if (cursor < text.length) parts.push({ text: text.slice(cursor) });
    return parts;
  }
  return { PALETTE, fieldColor, chips, chipText, fromChips, fieldsOf, classify, literal, nameBefore, clean, uniqueName, addSegment, removeSegment, compile, apply, untilSeparator, delimiterOf, blank, blankRule, outputCount, uniqueRuleId, ensureOutput, typeForToken, semantic, History, paintPlan };
})();
