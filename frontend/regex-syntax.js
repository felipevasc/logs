/* Structural reader for the PCRE2 syntax subset the visual assistant understands.
 * It tokenizes (escapes, classes, \Q..\E, comments, groups, references) and never
 * guesses: whatever it cannot map with certainty is reported as `uncertain` and
 * the matching visual transformation is refused. The native compiler stays the
 * authority on validity; nothing here evaluates a pattern. */
window.RegexSyntax = (() => {
  "use strict";
  // Beyond this the visual layer is off: marking stays predictable, editing and the native check do not.
  const MAX_LENGTH = 8192;
  const NAME = /^[A-Za-z_][A-Za-z0-9_]{0,31}$/;
  const digit = (c) => c >= "0" && c <= "9";
  const wordChar = (c) => /[A-Za-z0-9_]/.test(c || "");
  const GROUP_LABEL = {
    capture: "Grupo de captura",
    noncapture: "Grupo sem captura",
    lookahead: "Condição de contexto (lookahead)",
    lookbehind: "Condição de contexto (lookbehind)",
    atomic: "Grupo atômico",
    branchreset: "Grupo com numeração reiniciada por alternativa",
    conditional: "Condicional",
    scriptrun: "Script run",
    option: "Grupo com opções",
  };
  const ALPHA_ASSERTIONS = {
    pla: "lookahead", positive_lookahead: "lookahead", nla: "lookahead", negative_lookahead: "lookahead",
    plb: "lookbehind", positive_lookbehind: "lookbehind", nlb: "lookbehind", negative_lookbehind: "lookbehind",
    napla: "lookahead", non_atomic_positive_lookahead: "lookahead",
    naplb: "lookbehind", non_atomic_positive_lookbehind: "lookbehind",
    atomic: "atomic", sr: "scriptrun", script_run: "scriptrun", asr: "scriptrun", atomic_script_run: "scriptrun",
  };
  const NEGATIVE_ALPHA = new Set(["nla", "negative_lookahead", "nlb", "negative_lookbehind"]);

  function analyze(text, { extended = false } = {}) {
    text = String(text ?? "");
    const out = {
      text, tooLong: text.length > MAX_LENGTH, tokens: [], groups: [], captures: [], references: [], problems: [],
      uncertain: [], atoms: [], complete: true, features: { extended: false, branchReset: false, duplicateNames: false, conditionals: false, subroutines: false, numberingOff: false },
    };
    if (out.tooLong) { out.complete = false; out.uncertain.push({ start: 0, end: text.length, reason: "Expressão longa demais para a análise visual." }); return out; }
    const n = text.length, stack = [], tokens = out.tokens, groups = out.groups;
    let flags = { x: !!extended, n: false }, captureCount = 0, i = 0;
    const root = { id: -1, kind: "root", start: 0, openEnd: 0, contentStart: 0, contentEnd: n, end: n, parent: null, depth: -1 };
    const parentId = () => (stack.length ? stack.at(-1).group.id : -1);
    const token = (type, start, end, extra = {}) => { const t = { type, start, end, parent: parentId(), ...extra }; tokens.push(t); return t; };
    const uncertain = (start, end, reason) => { out.uncertain.push({ start, end, reason }); };
    // Atoms tile each group's content; a quantifier belongs to the atom before it.
    const atom = (start, end, extra = {}) => { const a = { start, end, parent: parentId(), ...extra }; out.atoms.push(a); return a; };
    const lastAtom = () => { const a = out.atoms.at(-1); return a && a.parent === parentId() && a.kind !== "alt" && a.kind !== "trivia" ? a : null; };
    const problem = (start, end, message, code) => out.problems.push({ start, end, message, code });
    function openGroup(kind, start, openEnd, extra = {}) {
      const group = { id: groups.length, kind, start, openEnd, contentStart: openEnd, contentEnd: n, end: null, closeStart: null, parent: parentId(), depth: stack.length, index: null, name: null, negative: false, insideLookaround: false, uncertain: false, ...extra };
      const enclosing = stack.at(-1)?.group;
      group.insideLookaround = !!enclosing && (["lookahead", "lookbehind"].includes(enclosing.kind) || enclosing.insideLookaround);
      groups.push(group);
      stack.push({ group, saved: { ...flags }, resetBase: captureCount, resetMax: captureCount });
      token("open", start, openEnd, { group: group.id });
      return group;
    }
    function closeGroup(at) {
      const frame = stack.pop();
      const g = frame.group;
      g.closeStart = at; g.end = at + 1; g.contentEnd = at;
      flags = frame.saved;
      if (g.kind === "branchreset") captureCount = Math.max(frame.resetMax, captureCount);
      token("close", at, at + 1, { group: g.id });
      // The closed group is one atom of its parent (quantifier may extend it).
      atom(g.start, g.end, { kind: "group", group: g.id });
      return g;
    }
    const parseName = (from, close) => {
      const end = text.indexOf(close, from);
      if (end < 0) return null;
      return { name: text.slice(from, end), start: from, end, after: end + 1 };
    };
    const registerCapture = (g, name, nameRange) => {
      captureCount++; g.index = captureCount; g.kind = "capture";
      const top = stack.at(-1);
      if (top) top.resetMax = Math.max(top.resetMax, captureCount);
      for (let k = stack.length - 2; k >= 0; k--) if (stack[k].group.kind === "branchreset") stack[k].resetMax = Math.max(stack[k].resetMax, captureCount);
      if (name != null) { g.name = name; g.nameRange = nameRange; if (!NAME.test(name)) problem(nameRange.start, nameRange.end, "Nome de grupo inválido: use letras, dígitos e _, sem começar por dígito (até 32).", "name"); }
    };
    const refs = out.references;
    const addRef = (type, start, end, data) => { const r = { id: refs.length, start, end, type, ...data }; refs.push(r); token("ref", start, end, { ref: r.id, refType: type }); atom(start, end, { kind: "ref", ref: r.id }); return r; };

    while (i < n) {
      const c = text[i];
      // Extended mode: whitespace and # comments are not part of the pattern.
      if (flags.x && /\s/.test(c)) { let j = i; while (j < n && /\s/.test(text[j])) j++; token("space", i, j); atom(i, j, { kind: "trivia" }); i = j; continue; }
      if (flags.x && c === "#") { let j = text.indexOf("\n", i); if (j < 0) j = n; token("comment", i, j, { extended: true }); atom(i, j, { kind: "trivia" }); i = j; continue; }
      if (c === "\\") { i = escape(i); continue; }
      if (c === "[") { i = charClass(i); continue; }
      if (c === "(") { i = openParen(i); continue; }
      if (c === ")") {
        if (!stack.length) { problem(i, i + 1, "Parêntese de fechamento sem abertura.", "unbalanced"); token("stray", i, i + 1); atom(i, i + 1, { kind: "stray" }); i++; continue; }
        closeGroup(i); i++; continue;
      }
      if (c === "|") {
        const frame = stack.at(-1);
        if (frame && frame.group.kind === "branchreset") { captureCount = frame.resetBase; }
        token("alt", i, i + 1); atom(i, i + 1, { kind: "alt" }); i++; continue;
      }
      if (c === "*" || c === "+" || c === "?") { i = quantifier(i, i + 1); continue; }
      if (c === "{") {
        const m = /^\{(\d+)(,(\d*))?\}|^\{,(\d+)\}/.exec(text.slice(i, i + 24));
        if (m) { i = quantifier(i, i + m[0].length); continue; }
      }
      if (c === "^" || c === "$") { token("anchor", i, i + 1); atom(i, i + 1, { kind: "anchor" }); i++; continue; }
      if (c === ".") { token("dot", i, i + 1); atom(i, i + 1, { kind: "dot" }); i++; continue; }
      // Literal run (single code point; keeps surrogate pairs whole).
      const cp = text.codePointAt(i), len = cp > 0xffff ? 2 : 1;
      token("literal", i, i + len); atom(i, i + len, { kind: "literal" }); i += len;
    }
    for (const frame of stack) { frame.group.end = n; frame.group.unclosed = true; out.complete = false; problem(frame.group.start, frame.group.openEnd, "Parêntese aberto sem fechamento.", "unclosed"); }

    function quantifier(start, end) {
      let e = end;
      if (text[e] === "?" || text[e] === "+") e++; // lazy / possessive suffix
      token("quantifier", start, e);
      const a = lastAtom();
      if (a) a.end = e; else atom(start, e, { kind: "stray" });
      return e;
    }
    function charClass(start) {
      let j = start + 1;
      if (text[j] === "^") j++;
      if (text[j] === "]") j++; // first ] is literal
      let closed = false;
      while (j < n) {
        const d = text[j];
        if (d === "\\") {
          if (text[j + 1] === "Q") { const e = text.indexOf("\\E", j + 2); j = e < 0 ? n : e + 2; continue; }
          j += 2; continue;
        }
        if (d === "[" && text[j + 1] === ":") { const e = text.indexOf(":]", j + 2); if (e > 0 && /^[\^a-z]*$/.test(text.slice(j + 2, e))) { j = e + 2; continue; } }
        if (d === "]") { closed = true; j++; break; }
        j++;
      }
      j = Math.min(j, n);
      token("class", start, j);
      atom(start, j, { kind: "class" });
      if (!closed) { out.complete = false; problem(start, Math.min(start + 1, n), "Classe de caracteres sem o ] de fechamento.", "unclosed-class"); uncertain(start, n, "Classe de caracteres incompleta."); }
      return j;
    }
    function escape(start) {
      const next = text[start + 1];
      if (next === undefined) { token("escape", start, n); atom(start, n, { kind: "stray" }); problem(start, n, "A barra invertida no fim exige um caractere seguinte.", "escape"); return n; }
      if (next === "Q") {
        const e = text.indexOf("\\E", start + 2);
        const end = e < 0 ? n : e + 2;
        token("quote", start, end, { bodyStart: start + 2, bodyEnd: e < 0 ? n : e });
        atom(start, end, { kind: "quote" });
        return end;
      }
      if (next === "E") { token("escape", start, start + 2, { ignored: true }); atom(start, start + 2, { kind: "trivia" }); return start + 2; }
      if (next === "k" && "<'{".includes(text[start + 2] || "x")) {
        const open = text[start + 2], close = open === "<" ? ">" : open === "{" ? "}" : "'";
        const p = parseName(start + 3, close);
        if (p) { addRef("backref", start, p.after, { refKind: "named", name: p.name, nameStart: p.start, nameEnd: p.end, syntax: `\\k${open}` }); return p.after; }
      }
      if (next === "g") {
        let m = /^\\g\{(-|\+)?(\d+)\}/.exec(text.slice(start, start + 24));
        if (m) { addRef("backref", start, start + m[0].length, { refKind: "numbered", number: Number(m[2]), relative: m[1] || null, syntax: "\\g{}" }); return start + m[0].length; }
        m = /^\\g(-|\+)?(\d+)/.exec(text.slice(start, start + 24));
        if (m) { addRef("backref", start, start + m[0].length, { refKind: "numbered", number: Number(m[2]), relative: m[1] || null, syntax: "\\g" }); return start + m[0].length; }
        m = /^\\g\{([A-Za-z_]\w*)\}/.exec(text.slice(start, start + 64));
        if (m) { addRef("backref", start, start + m[0].length, { refKind: "named", name: m[1], nameStart: start + 3, nameEnd: start + 3 + m[1].length, syntax: "\\g{}" }); return start + m[0].length; }
        if (text[start + 2] === "<" || text[start + 2] === "'") {
          const close = text[start + 2] === "<" ? ">" : "'", p = parseName(start + 3, close);
          if (p) {
            const numeric = /^(-|\+)?\d+$/.exec(p.name);
            addRef("subroutine", start, p.after, numeric ? { refKind: "numbered", number: Number(p.name.replace(/^[-+]/, "")), relative: numeric[1] || null, syntax: "\\g<>" } : { refKind: "named", name: p.name, nameStart: p.start, nameEnd: p.end, syntax: "\\g<>" });
            out.features.subroutines = true; return p.after;
          }
        }
      }
      if (digit(next) && next !== "0") {
        let j = start + 1; while (j < n && digit(text[j])) j++;
        const number = Number(text.slice(start + 1, j));
        // \10 and above are references only when that many groups exist; resolved later.
        const r = addRef("backref", start, j, { refKind: "numbered", number, relative: null, syntax: "\\N", pendingOctal: number >= 10 });
        void r; return j;
      }
      // \x{..}, \o{..}, \N{..}, \p{..}, \P{..}, \c?, \0dd, single letters
      let end = start + 2;
      if ("xopPN".includes(next) && text[start + 2] === "{") { const e = text.indexOf("}", start + 3); end = e < 0 ? n : e + 1; }
      else if (next === "x") { while (end < n && end < start + 4 && /[0-9A-Fa-f]/.test(text[end])) end++; }
      else if (next === "0") { while (end < n && end < start + 4 && /[0-7]/.test(text[end])) end++; }
      else if (next === "c") end = Math.min(n, start + 3);
      else if (/\p{L}/u.test(next) === false && next.codePointAt(0) > 0xffff) end = start + 3;
      const cls = "dDwWsShHvVRXNpP".includes(next) ? "class-escape" : "bBAzZGK".includes(next) ? "anchor-escape" : "escape";
      token("escape", start, end, { escClass: cls, char: text.slice(start, end) });
      atom(start, end, { kind: cls === "anchor-escape" ? "anchor" : "escape" });
      return end;
    }
    function openParen(start) {
      // (*VERB) and (*alpha_assertion:
      if (text[start + 1] === "*") {
        const m = /^\(\*([A-Za-z_]+)(:|\))/.exec(text.slice(start, start + 64));
        if (m && m[2] === ":" && ALPHA_ASSERTIONS[m[1].toLowerCase()]) {
          const kind = ALPHA_ASSERTIONS[m[1].toLowerCase()];
          openGroup(kind, start, start + m[0].length, { negative: NEGATIVE_ALPHA.has(m[1].toLowerCase()), alpha: true });
          return start + m[0].length;
        }
        const e = text.indexOf(")", start);
        const end = e < 0 ? n : e + 1;
        token("verb", start, end); atom(start, end, { kind: "verb" });
        if (e < 0) problem(start, n, "Verbo (*…) sem o ) de fechamento.", "unclosed");
        return end;
      }
      if (text[start + 1] !== "?") {
        const g = openGroup("capture", start, start + 1);
        if (flags.n) { g.kind = "noncapture"; g.autoNoncapture = true; out.features.numberingOff = true; }
        else registerCapture(g, null, null);
        return start + 1;
      }
      const rest = text.slice(start + 2, start + 80), at = start + 2;
      let m;
      if (rest[0] === "#") { // (?#comment)
        const e = text.indexOf(")", start); const end = e < 0 ? n : e + 1;
        token("comment", start, end); atom(start, end, { kind: "trivia" });
        if (e < 0) { out.complete = false; problem(start, n, "Comentário (?#…) sem o ) de fechamento.", "unclosed"); uncertain(start, n, "Comentário incompleto."); }
        return end;
      }
      if (rest[0] === ":") { openGroup("noncapture", start, at + 1); return at + 1; }
      if (rest[0] === "|") { out.features.branchReset = true; openGroup("branchreset", start, at + 1); return at + 1; }
      if (rest[0] === ">") { openGroup("atomic", start, at + 1); return at + 1; }
      if (rest[0] === "=" || rest[0] === "!") { openGroup("lookahead", start, at + 1, { negative: rest[0] === "!" }); return at + 1; }
      if (rest.startsWith("*") ) { openGroup("lookahead", start, at + 1, { nonAtomic: true }); return at + 1; }
      if (rest.startsWith("<=") || rest.startsWith("<!")) { openGroup("lookbehind", start, at + 2, { negative: rest[1] === "!" }); return at + 2; }
      if (rest.startsWith("<*")) { openGroup("lookbehind", start, at + 2, { nonAtomic: true }); return at + 2; }
      if ((m = /^<([^>]*)>/.exec(rest)) || (m = /^'([^']*)'/.exec(rest)) || (m = /^P<([^>]*)>/.exec(rest))) {
        const prefix = m[0].startsWith("P") ? 2 : 1, nameStart = at + prefix, g = openGroup("capture", start, at + m[0].length);
        registerCapture(g, m[1], { start: nameStart, end: nameStart + m[1].length });
        if (flags.n) { /* (?n) does not stop named groups from capturing */ }
        return at + m[0].length;
      }
      if ((m = /^P=([^)]*)\)/.exec(rest))) { addRef("backref", start, at + m[0].length, { refKind: "named", name: m[1], nameStart: at + 2, nameEnd: at + 2 + m[1].length, syntax: "(?P=)" }); return at + m[0].length; }
      if ((m = /^P>([^)]*)\)/.exec(rest))) { addRef("subroutine", start, at + m[0].length, { refKind: "named", name: m[1], nameStart: at + 2, nameEnd: at + 2 + m[1].length, syntax: "(?P>)" }); out.features.subroutines = true; return at + m[0].length; }
      if ((m = /^&([^)]*)\)/.exec(rest))) { addRef("subroutine", start, at + m[0].length, { refKind: "named", name: m[1], nameStart: at + 1, nameEnd: at + 1 + m[1].length, syntax: "(?&)" }); out.features.subroutines = true; return at + m[0].length; }
      if ((m = /^(R|[+-]?\d+)\)/.exec(rest))) {
        out.features.subroutines = true;
        if (m[1] === "R") { token("subroutine", start, at + m[0].length, { whole: true }); atom(start, at + m[0].length, { kind: "ref" }); return at + m[0].length; }
        const rel = /^[+-]/.exec(m[1])?.[0] || null;
        addRef("subroutine", start, at + m[0].length, { refKind: "numbered", number: Number(m[1].replace(/^[+-]/, "")), relative: rel, syntax: "(?N)" });
        return at + m[0].length;
      }
      if (rest[0] === "(") { // (?(condition)yes|no)
        out.features.conditionals = true;
        const g = openGroup("conditional", start, at);
        const cond = /^\((?:([+-]?\d+)|<([^>]*)>|'([^']*)'|(R&?\w*|R\d+|DEFINE|VERSION[^)]*|[A-Za-z_]\w*))\)/.exec(rest);
        if (cond) {
          const condEnd = at + cond[0].length;
          token("condition", at, condEnd, { group: g.id });
          if (cond[1] != null) addRef("condition", at, condEnd, { refKind: "numbered", number: Number(cond[1].replace(/^[+-]/, "")), relative: /^[+-]/.exec(cond[1])?.[0] || null, syntax: "(?(N))" });
          else if (cond[2] != null || cond[3] != null || (cond[4] && !/^(R|DEFINE|VERSION)/.test(cond[4]))) {
            const nm = cond[2] ?? cond[3] ?? cond[4], from = at + 1 + (cond[4] != null ? 0 : 1);
            addRef("condition", at, condEnd, { refKind: "named", name: nm, nameStart: from, nameEnd: from + nm.length, syntax: "(?(<>))" });
          }
          g.contentStart = condEnd;
          return condEnd;
        }
        // Assertion condition: the inner (?=…) opens as an ordinary child group.
        return at;
      }
      if ((m = /^([imnsxJU^]*(?:-[imnsxJU]*)?)([:)])/.exec(rest)) && (m[1] || m[2] === ":" || rest[0] === "^")) {
        const letters = m[1];
        const before = { ...flags };
        applyFlags(letters);
        if (m[2] === ")") { token("option", start, at + m[0].length, { letters }); atom(start, at + m[0].length, { kind: "trivia" }); if (/x/i.test(letters)) out.features.extended = true; return at + m[0].length; }
        const g = openGroup("option", start, at + m[0].length, { letters });
        stack.at(-1).saved = before; // flags revert at the group's end
        return at + m[0].length;
      }
      // Anything else after (? is outside the modelled subset: keep editing, stay neutral.
      const g = openGroup("noncapture", start, at);
      g.uncertain = true; g.unknown = true;
      uncertain(start, at, "Construção (?…) fora do subconjunto visual.");
      out.complete = out.complete && true;
      return at;
    }
    function applyFlags(letters) {
      const [on, off = ""] = letters.split("-");
      if (on.includes("^")) { flags.x = false; flags.n = false; }
      for (const ch of on) { if (ch === "x") flags.x = true; if (ch === "n") flags.n = true; }
      for (const ch of off) { if (ch === "x") flags.x = false; if (ch === "n") flags.n = false; }
    }

    // ---- captures, numbering and references
    const total = captureCount;
    for (const g of groups) {
      if (g.kind === "capture") out.captures.push(g);
    }
    for (const g of out.captures) {
      g.sharedIndex = out.captures.some((o) => o !== g && o.index === g.index);
      if (g.sharedIndex) g.uncertain = true;
    }
    const byName = new Map();
    for (const g of out.captures) if (g.name != null) { if (!byName.has(g.name)) byName.set(g.name, []); byName.get(g.name).push(g); }
    for (const [, list] of byName) if (list.length > 1) { out.features.duplicateNames = true; for (const g of list) g.duplicateName = true; }
    const opened = (position) => out.captures.filter((g) => g.start < position).length;
    for (const r of refs) {
      r.targets = []; r.valid = true; r.ambiguous = false;
      const tok = tokens.find((t) => t.ref === r.id);
      if (r.refKind === "named") {
        const list = byName.get(r.name) || [];
        r.targets = list.map((g) => g.id);
        if (!list.length) { r.valid = false; r.message = `Não existe um grupo chamado “${r.name}”.`; }
        else if (list.length > 1) { r.ambiguous = true; r.message = `O nome “${r.name}” é usado por mais de um grupo.`; }
      } else if (r.refKind === "numbered") {
        let number = r.number;
        if (r.relative === "-") number = opened(r.start) - r.number + 1;
        else if (r.relative === "+") number = opened(r.start) + r.number;
        r.resolvedNumber = number;
        const matches = out.captures.filter((g) => g.index === number);
        if (r.pendingOctal && total < number) {
          // \10 and above without that many groups is an octal escape, not a reference.
          r.octal = true; r.valid = true; r.targets = [];
          if (tok) { tok.type = "escape"; tok.escClass = "escape"; tok.char = text.slice(r.start, r.end); delete tok.ref; }
          const a = out.atoms.find((x) => x.ref === r.id); if (a) { a.kind = "escape"; delete a.ref; }
          continue;
        }
        r.targets = matches.map((g) => g.id);
        if (number === 0 && (r.refKind === "numbered") && r.syntax === "(?N)") { r.valid = true; r.whole = true; }
        else if (!matches.length) { r.valid = false; r.message = `Não existe o grupo ${number}${total ? ` (a expressão tem ${total})` : ""}.`; }
        else if (matches.length > 1) r.ambiguous = true;
      }
      if (!r.valid) problem(r.start, r.end, r.message, "reference");
    }
    out.refsByGroup = new Map();
    for (const r of refs) for (const id of r.targets) { if (!out.refsByGroup.has(id)) out.refsByGroup.set(id, []); out.refsByGroup.get(id).push(r.id); }
    // Reliability: shared numbers (branch reset) and duplicate names stay neutral.
    if (out.features.branchReset) uncertain(0, n, "Numeração com (?|…): capturas de alternativas diferentes compartilham o mesmo número.");
    if (out.features.duplicateNames) uncertain(0, n, "Nomes de grupo repetidos: a referência por nome não aponta para um único grupo.");
    out.total = total;
    out.root = root;
    return out;
  }

  // ------------------------------------------------------------ segments (decorations)
  /* Flat, non-overlapping segments with the classes of every layer covering them. */
  function segments(analysis, { slotOf = () => 0, focus = null } = {}) {
    const text = analysis.text, n = text.length, layers = [];
    if (analysis.tooLong) return [{ start: 0, end: n, classes: ["rx-plain"] }];
    const add = (start, end, cls, data, kind) => { if (end > start) layers.push({ start, end, cls, data, kind }); };
    const capByGroup = (g) => g.kind === "capture" && !g.uncertain;
    for (const g of analysis.groups) {
      const end = g.end ?? n;
      if (g.kind === "lookahead" || g.kind === "lookbehind") add(g.start, end, `rx-look rx-${g.kind}${g.negative ? " rx-neg" : ""}`, { group: g.id });
      else if (capByGroup(g)) add(g.start, end, `rx-cap rx-cap-${slotOf(g.id)}`, { group: g.id, capture: g.index }, "cap");
      else if (g.kind === "capture") add(g.start, end, "rx-cap rx-cap-neutral", { group: g.id }, "cap");
      else if (g.kind === "noncapture" || g.kind === "atomic" || g.kind === "option" || g.kind === "conditional" || g.kind === "branchreset") add(g.start, g.openEnd, "rx-open-plain", { group: g.id });
      if (g.kind !== "lookahead" && g.kind !== "lookbehind") { /* delimiters of non-capture groups stay plain */ }
      add(g.start, g.openEnd, "rx-edge", { group: g.id });
      if (g.closeStart != null) add(g.closeStart, g.closeStart + 1, "rx-edge", { group: g.id });
      if (g.nameRange) add(g.nameRange.start, g.nameRange.end, "rx-name", { group: g.id });
    }
    for (const t of analysis.tokens) {
      if (t.type === "escape") add(t.start, t.end, `rx-esc${t.escClass === "class-escape" ? " rx-esc-class" : ""}${t.escClass === "anchor-escape" ? " rx-esc-anchor" : ""}`);
      else if (t.type === "class") add(t.start, t.end, "rx-class");
      else if (t.type === "quote") add(t.start, t.end, "rx-quote");
      else if (t.type === "quantifier") add(t.start, t.end, "rx-quant");
      else if (t.type === "comment") add(t.start, t.end, "rx-comment");
      else if (t.type === "alt") add(t.start, t.end, "rx-alt");
      else if (t.type === "anchor") add(t.start, t.end, "rx-anchor");
      else if (t.type === "verb" || t.type === "option") add(t.start, t.end, "rx-opt");
      else if (t.type === "condition") add(t.start, t.end, "rx-opt");
      else if (t.type === "stray") add(t.start, t.end, "rx-problem");
      else if (t.type === "ref") {
        const r = analysis.references[t.ref];
        if (!r.valid) add(t.start, t.end, "rx-ref rx-ref-bad", { ref: r.id });
        else if (t.refType === "subroutine") add(t.start, t.end, "rx-ref rx-sub", { ref: r.id });
        else if (r.targets.length === 1 && !r.ambiguous && !analysis.groups[r.targets[0]].uncertain) add(t.start, t.end, `rx-ref rx-cap-${slotOf(r.targets[0])}`, { ref: r.id, group: r.targets[0] });
        else add(t.start, t.end, "rx-ref rx-ref-neutral", { ref: r.id });
      }
    }
    for (const p of analysis.problems) add(p.start, p.end, "rx-problem", { problem: true });
    for (const u of analysis.uncertain) if (u.end - u.start < n && u.end > u.start) add(u.start, u.end, "rx-uncertain");
    if (focus != null) {
      const g = analysis.groups[focus];
      if (g) { add(g.start, g.end ?? n, "rx-focus"); for (const rid of analysis.refsByGroup.get(g.id) || []) { const r = analysis.references[rid]; add(r.start, r.end, "rx-focus"); } }
    }
    const cuts = new Set([0, n]);
    for (const l of layers) { cuts.add(l.start); cuts.add(l.end); }
    const points = [...cuts].sort((x, y) => x - y), result = [];
    // Sweep: only the layers open at a position are examined, so cost follows nesting depth.
    layers.sort((x, y) => x.start - y.start);
    let next = 0, active = [];
    for (let k = 0; k < points.length - 1; k++) {
      const start = points[k], end = points[k + 1];
      while (next < layers.length && layers[next].start <= start) active.push(layers[next++]);
      active = active.filter((l) => l.end >= end);
      const all = active;
      // Nested captures: the innermost one owns the colour of the characters it covers.
      let deepest = null;
      for (const l of all) if (l.kind === "cap" && (!deepest || analysis.groups[l.data.group].depth >= analysis.groups[deepest.data.group].depth)) deepest = l;
      const covering = all.filter((l) => l.kind !== "cap" || l === deepest);
      result.push({ start, end, classes: covering.map((l) => l.cls).join(" ").split(" ").filter(Boolean), layers: all.slice() });
    }
    return result;
  }

  // ------------------------------------------------------------ queries
  function innermostGroup(analysis, position) {
    let best = null;
    for (const g of analysis.groups) if (g.contentStart <= position && position <= (g.contentEnd ?? analysis.text.length) && g.start < position && (!best || g.depth > best.depth)) best = g;
    return best;
  }
  /** Group, reference or token under `position` (caret between characters counts for both sides). */
  function at(analysis, position) {
    const hit = { group: null, ref: null };
    for (const r of analysis.references) if (position >= r.start && position <= r.end) { hit.ref = r; break; }
    if (!hit.ref) {
      let best = null;
      for (const g of analysis.groups) {
        const onEdge = (position >= g.start && position <= g.openEnd) || (g.closeStart != null && position >= g.closeStart && position <= g.closeStart + 1);
        if (onEdge && (!best || g.depth >= best.depth)) best = g;
      }
      hit.group = best;
    }
    return hit;
  }
  const overlapsUncertain = (analysis, start, end) => analysis.uncertain.some((u) => u.start < end && u.end > start && !(u.start === 0 && u.end === analysis.text.length));
  /** Selection that is a sequence of whole atoms of one group (or the top level). */
  function wrappable(analysis, start, end) {
    if (analysis.tooLong) return { ok: false, reason: "Expressão longa demais para a ação assistida." };
    if (!analysis.complete) return { ok: false, reason: "Corrija os parênteses ou a classe incompleta antes de usar esta ação." };
    if (start >= end) return { ok: false, reason: "Selecione um trecho da expressão." };
    let parent = null;
    for (const g of analysis.groups) if (g.contentStart <= start && end <= g.contentEnd && (!parent || g.depth > parent.depth)) parent = g;
    const parentId = parent ? parent.id : -1;
    const contentStart = parent ? parent.contentStart : 0;
    const siblings = analysis.atoms.filter((a) => a.parent === parentId);
    const starts = new Set([contentStart, ...siblings.map((a) => a.start)]), ends = new Set([contentStart, ...siblings.map((a) => a.end)]);
    if (!starts.has(start) || !ends.has(end)) return { ok: false, reason: "A seleção corta um escape, classe, grupo ou quantificador. Selecione trechos inteiros." };
    if (overlapsUncertain(analysis, start, end)) return { ok: false, reason: "Este trecho usa uma construção que o assistente não consegue mapear com segurança." };
    return { ok: true, parent: parent ? parent.id : null };
  }
  function describe(analysis, hit) {
    if (hit.ref) {
      const r = hit.ref;
      if (!r.valid) return r.message;
      if (hit.ref.syntax === "(?(N))" || hit.ref.syntax === "(?(<>))") return "Condição: depende de o grupo ter participado da correspondência.";
      const target = r.targets.length === 1 ? analysis.groups[r.targets[0]] : null;
      const label = target ? `grupo ${target.index}${target.name ? ` (${target.name})` : ""}` : "um grupo";
      if (r.type === "subroutine") return `Chamada de subrotina: aplica de novo a regra do ${label}. Não exige o mesmo valor.`;
      return `Backreference: exige exatamente o mesmo texto capturado pelo ${label}.`;
    }
    const g = hit.group;
    if (!g) return "";
    if (g.kind === "lookahead" || g.kind === "lookbehind") return `Condição de contexto (${g.kind === "lookbehind" ? "Lookbehind" : "Lookahead"}${g.negative ? " negativo" : ""}): participa do teste, mas não é consumida como parte do trecho encontrado.`;
    if (g.kind === "capture") return `Grupo ${g.index}${g.name ? ` “${g.name}”` : ""}: captura o trecho encontrado aqui.`;
    return GROUP_LABEL[g.kind] || "Grupo";
  }

  // ------------------------------------------------------------ builders (pure text → text)
  const META = new Set("\\^$.|?*+()[]{}");
  /** Literal text as a pattern fragment. Extended mode needs spaces and # escaped as well. */
  function escapeLiteral(value, { extended = false } = {}) {
    let out = "";
    for (const ch of String(value)) {
      if (META.has(ch)) out += `\\${ch}`;
      else if (ch === "\n") out += "\\n";
      else if (ch === "\r") out += "\\r";
      else if (ch === "\t") out += "\\t";
      else if (ch.codePointAt(0) < 0x20 || ch.codePointAt(0) === 0x7f) out += `\\x${ch.codePointAt(0).toString(16).padStart(2, "0")}`;
      else if (extended && (ch === "#" || ch === " ")) out += `\\${ch}`;
      else out += ch;
    }
    return out;
  }
  const classEscape = (ch) => (ch === "\\" || ch === "]" || ch === "^" || ch === "-" || ch === "[" ? `\\${ch}` : ch === "\n" ? "\\n" : ch === "\r" ? "\\r" : ch === "\t" ? "\\t" : ch);
  const VALUE_KINDS = [
    { id: "word", label: "Palavra", hint: "letras, dígitos e _" },
    { id: "number", label: "Número", hint: "inteiro ou decimal" },
    { id: "space", label: "Até espaço ou quebra de linha", hint: "tudo que não é espaço" },
    { id: "delimiter", label: "Até um delimitador", hint: "informe o delimitador" },
    { id: "custom", label: "Expressão personalizada", hint: "sua própria expressão" },
  ];
  /** Pattern for a value of the chosen kind. `custom` is wrapped when it has a top-level `|`. */
  function valueExpression(kind, { delimiter = "", custom = "", extended = false } = {}) {
    switch (kind) {
      case "word": return { text: "\\w+", ok: true };
      case "number": return { text: "-?\\d+(?:[.,]\\d+)?", ok: true };
      case "space": return { text: "\\S+", ok: true };
      case "delimiter": {
        const chars = [...String(delimiter)];
        if (!chars.length) return { text: "", ok: false, reason: "Informe o delimitador." };
        if (chars.length === 1) return { text: `[^${classEscape(chars[0])}\\r\\n]+`, ok: true };
        // A class would stop at any one of the characters; a multi-character delimiter needs a lookahead.
        return { text: `(?:(?!${escapeLiteral(delimiter, { extended })})[^\\r\\n])+`, ok: true };
      }
      case "custom": {
        const body = String(custom);
        if (!body.trim()) return { text: "", ok: false, reason: "Informe a expressão do valor." };
        const a = analyze(body);
        const topAlt = a.tokens.some((t) => t.type === "alt" && t.parent === -1);
        return { text: topAlt ? `(?:${body})` : body, ok: true, custom: true };
      }
      default: return { text: "", ok: false, reason: "Escolha o tipo de valor." };
    }
  }
  function captureName(suggestion, taken = new Set()) {
    let base = String(suggestion || "").normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/[^A-Za-z0-9_]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 28);
    if (!base || digit(base[0])) base = base ? `v_${base}` : "valor";
    base = base.slice(0, 28);
    let name = base, k = 2;
    while (taken.has(name)) name = `${base}_${k++}`;
    return name;
  }
  const wrapCapture = (body, name) => (name ? `(?<${name}>${body})` : `(${body})`);
  const EXPLAIN = {
    word: "uma palavra (letras, dígitos e _)",
    number: "um número, inteiro ou decimal",
    space: "tudo até o próximo espaço ou quebra de linha",
    custom: "o que a sua expressão descrever",
  };
  /** After/before/between: returns the generated expression and its plain-language reading. */
  function relative(direction, parts) {
    const { before = "", after = "", value, name = null, extended = false } = parts;
    const v = value.text, body = name ? wrapCapture(v, name) : v;
    const prefix = direction === "after" || direction === "between" ? escapeLiteral(before, { extended }) : "";
    const suffix = direction === "before" || direction === "between" ? escapeLiteral(after, { extended }) : "";
    const text = `${prefix ? `(?<=${prefix})` : ""}${body}${suffix ? `(?=${suffix})` : ""}`;
    const what = EXPLAIN[value.kind] || (value.kind === "delimiter" ? `tudo até “${parts.delimiter}” (ou o fim da linha)` : "o valor");
    const needs = [];
    if (prefix) needs.push(`exige ${before} antes do valor`);
    if (suffix) needs.push(`exige ${after} depois do valor`);
    const explanation = `${needs.length ? `${needs[0].replace(/^./, (c) => c.toUpperCase())}${needs[1] ? ` e ${needs[1]}` : ""}, sem incluir ${needs.length > 1 ? "esses trechos" : "esse trecho"} no resultado.` : ""} O resultado é ${what}.`.trim();
    return { text, explanation, usesLookbehind: !!prefix, usesLookahead: !!suffix };
  }
  /** Reference to an existing capture at the caret. Named when possible. */
  function referenceText(group, nextChar = "") {
    if (group.name) return `\\k<${group.name}>`;
    return digit(nextChar) ? `\\g{${group.index}}` : `\\${group.index}`;
  }
  /** Rename a named group and exactly its syntactic references. */
  function rename(analysis, oldName, newName) {
    if (!NAME.test(newName)) return { ok: false, reason: "Use letras, dígitos e _, sem começar por dígito (até 32 caracteres)." };
    if (oldName === newName) return { ok: false, reason: "O nome não mudou." };
    if (analysis.captures.some((g) => g.name === newName)) return { ok: false, reason: `Já existe um grupo chamado “${newName}”.` };
    const edits = [];
    for (const g of analysis.captures) if (g.name === oldName) edits.push({ start: g.nameRange.start, end: g.nameRange.end });
    for (const r of analysis.references) if (r.refKind === "named" && r.name === oldName && r.nameStart != null) edits.push({ start: r.nameStart, end: r.nameEnd });
    if (!edits.length) return { ok: false, reason: "O grupo não foi encontrado." };
    return { ok: true, edits: edits.sort((a, b) => a.start - b.start).map((e) => ({ ...e, text: newName })) };
  }
  function applyEdits(text, edits) {
    let out = "", cursor = 0;
    for (const e of [...edits].sort((a, b) => a.start - b.start)) { out += text.slice(cursor, e.start) + e.text; cursor = e.end; }
    return out + text.slice(cursor);
  }
  /** Smallest single replacement that turns `from` into `to` (one undoable transaction). */
  function diff(from, to) {
    let a = 0;
    while (a < from.length && a < to.length && from[a] === to[a]) a++;
    let b = 0;
    while (b < from.length - a && b < to.length - a && from[from.length - 1 - b] === to[to.length - 1 - b]) b++;
    // Never split a surrogate pair.
    while (a > 0 && /[\uDC00-\uDFFF]/.test(from[a]) ) a--;
    return { start: a, end: from.length - b, text: to.slice(a, to.length - b) };
  }

  return { analyze, segments, at, innermostGroup, wrappable, describe, escapeLiteral, valueExpression, VALUE_KINDS, captureName, wrapCapture, relative, referenceText, rename, applyEdits, diff, overlapsUncertain, NAME, GROUP_LABEL, MAX_LENGTH };
})();
