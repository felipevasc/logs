/* Local, read-only interpretation. Nothing here authenticates a token or executes its contents. */
window.ValueInspector = (() => {
  "use strict";
  const LIMITS = Object.freeze({ bytes: 256 * 1024, nodes: 1000, depth: 12, transforms: 3, preview: 2048 });
  let active = null;
  const utf8 = new TextEncoder();
  const pathKey = (path, key) => `${path}[${JSON.stringify(String(key))}]`;
  const fits = text => text.length <= LIMITS.bytes && utf8.encode(text).length <= LIMITS.bytes;

  function decodeBase64(input, url = false) {
    if (!input || !fits(input) || !/^[A-Za-z0-9+/_-]*={0,2}$/.test(input)) return null;
    if (!url && /[-_]/.test(input)) return null;
    const bare = input.replace(/=+$/, "");
    if (bare.length % 4 === 1 || (input.includes("=") && input.length % 4 !== 0)) return null;
    try {
      const normalized = bare.replace(/-/g, "+").replace(/_/g, "/");
      const binary = atob(normalized + "=".repeat((4 - normalized.length % 4) % 4));
      // Reject noncanonical pad bits, which atob would silently discard.
      if (btoa(binary).replace(/=+$/, "") !== normalized) return null;
      const bytes = Uint8Array.from(binary, character => character.charCodeAt(0));
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(text)) return null;
      return text;
    } catch { return null; }
  }

  // Compare decimal values, not binary floating-point approximations. The scale
  // stays an integer; never expand an exponent into a potentially enormous string.
  function decimalParts(text) {
    const match = /^(-?)(\d+)(?:\.(\d+))?(?:[eE]([+-]?\d+))?$/.exec(text);
    if (!match) return null;
    const fraction = match[3] || "";
    const digits = (match[2] + fraction).replace(/^0+/, "");
    if (!digits) return { sign: match[1], coefficient: "0", scale: 0 };
    const coefficient = digits.replace(/0+$/, "");
    const exponent = match[4] || "0";
    const magnitude = exponent.replace(/^[+-]/, "").replace(/^0+/, "") || "0";
    // A larger scale cannot match any finite Number here. Refuse conservatively.
    if (magnitude.length > 9) return null;
    const scale = (exponent.startsWith("-") ? -1 : 1) * Number(magnitude) - fraction.length + digits.length - coefficient.length;
    return { sign: match[1], coefficient, scale };
  }

  function exactNumber(number, source) {
    if (!Number.isFinite(number) || (Number.isInteger(number) && !Number.isSafeInteger(number))) return false;
    const original = decimalParts(source), represented = decimalParts(JSON.stringify(number));
    return !!original && !!represented && original.sign === represented.sign && original.coefficient === represented.coefficient && original.scale === represented.scale;
  }

  function hasUnsafeNumericLexeme(text) {
    // Consume quoted strings (including escaped quotes) as whole tokens so their
    // contents cannot be mistaken for numeric values on older WebView runtimes.
    const tokens = /"(?:\\[\s\S]|[^"\\])*"|(-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;
    for (const token of text.matchAll(tokens)) if (token[1] && !exactNumber(Number(token[1]), token[1])) return true;
    return false;
  }

  function parseContainer(text) {
    if (!/^[\s]*[\[{]/.test(text)) return null;
    let notice = "", fallbackRisk;
    const precision = "Este ambiente não preserva a representação desse número; estrutura não interpretada. Consulte o texto original.";
    try {
      const value = JSON.parse(text, (_key, number, context) => {
        if (typeof number === "number") {
          if (typeof context?.source === "string") {
            if (!exactNumber(number, context.source)) {
              notice = "Números fora da precisão segura foram preservados como texto, sem arredondamento.";
              return context.source;
            }
          } else {
            fallbackRisk ??= hasUnsafeNumericLexeme(text);
            if (fallbackRisk) throw new RangeError(precision);
          }
        }
        return number;
      });
      return value && typeof value === "object" ? { value, notice } : null;
    } catch (error) { return error instanceof RangeError ? { declined: true, notice: precision } : null; }
  }

  function decodeJwt(token) {
    const parts = token.split(".");
    if (parts.length === 3 && parts[0] && parts[1] && parts.every(part => /^[A-Za-z0-9_-]*$/.test(part))) {
      const header = parseContainer(decodeBase64(parts[0], true) || "");
      const payload = parseContainer(decodeBase64(parts[1], true) || "");
      if (header?.declined || payload?.declined) return { declined: true, notice: header?.notice || payload?.notice };
      if (header && !Array.isArray(header.value) && payload && !Array.isArray(payload.value)) {
        const value = { header: header.value, payload: payload.value, signature_base64url: parts[2] };
        const dates = {};
        for (const key of ["iat", "nbf", "exp"]) {
          const seconds = payload.value[key];
          if (typeof seconds === "number" && Number.isFinite(seconds) && Math.abs(seconds * 1000) <= 8.64e15) {
            dates[key] = new Date(seconds * 1000).toISOString();
          }
        }
        if (Object.keys(dates).length) value.datas_declaradas_UTC = dates;
        return { value, notice: header.notice || payload.notice };
      }
    }
    return null;
  }

  function interpret(text) {
    if (!fits(text)) return null;
    const jwt = decodeJwt(text.trim().replace(/^Bearer\s+/i, ""));
    if (jwt) return { kind: "jwt", label: "JWT · estrutura não verificada", ...jwt };
    const json = parseContainer(text);
    if (json) return { kind: "json", label: "JSON interpretado", ...json };
    const tokens = Object.create(null);
    const candidates = /(?:^|[^A-Za-z0-9_-])([A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*)(?=$|[^A-Za-z0-9_-])/g;
    let attempts = 0, found = 0, notice = "";
    for (const match of text.matchAll(candidates)) {
      if (++attempts > 32 || found >= 8) break;
      const decoded = decodeJwt(match[1]);
      if (!decoded) continue;
      notice ||= decoded.notice;
      if (decoded.declined) continue;
      const offset = match.index + match[0].length - match[1].length;
      tokens[`ocorrência_${++found}`] = { inicio_UTF16: offset, fim_UTF16: offset + match[1].length, ...decoded.value };
    }
    if (found) return { kind: "jwt_tokens", label: "JWT no texto · até 8 ocorrências, offsets UTF-16, estrutura não verificada", value: tokens, notice };
    if (notice) return { declined: true, notice };
    const compact = text.trim();
    // Short ordinary words are ambiguous; only show plausible UTF-8 Base64 text.
    if (compact.length >= 8) {
      const decoded = decodeBase64(compact, /[-_]/.test(compact));
      if (decoded !== null && decoded !== compact && decoded.trim()) {
        return { kind: "base64", label: "Base64 · texto UTF-8 interpretado", value: decoded };
      }
    }
    return null;
  }

  function analyze(value, path = "$", label = "Valor original") {
    let count = 0, bytes = 0;
    const seen = new WeakSet(), notices = new Set();
    const limit = (name, path, reason) => {
      notices.add(reason);
      return { label: name, path, preview: `[${reason}]`, children: [], limited: true };
    };
    function visit(current, name, currentPath, depth, transforms) {
      if (count >= LIMITS.nodes) return limit(name, currentPath, "Limite de 1.000 nós atingido");
      count++;
      if (depth > LIMITS.depth) return limit(name, currentPath, "Limite de profundidade atingido");
      const node = { label: name, path: currentPath, value: current, children: [] };
      if (current && typeof current === "object") {
        if (seen.has(current)) return limit(name, currentPath, "Referência repetida ou circular");
        seen.add(current);
        node.preview = Array.isArray(current) ? `Array (${current.length})` : "Objeto";
        for (const key in current) {
          if (!Object.prototype.hasOwnProperty.call(current, key)) continue;
          if (count >= LIMITS.nodes) { notices.add("Limite de 1.000 nós atingido"); break; }
          if (!fits(key) || bytes + utf8.encode(key).length > LIMITS.bytes) {
            count++; node.children.push(limit("…", currentPath, "Limite de 256 KiB atingido")); break;
          }
          bytes += utf8.encode(key).length;
          const descriptor = Object.getOwnPropertyDescriptor(current, key);
          if (descriptor && "value" in descriptor) node.children.push(visit(descriptor.value, key, pathKey(currentPath, key), depth + 1, transforms));
          else { count++; node.children.push(limit(key, pathKey(currentPath, key), "Propriedade dinâmica não avaliada")); }
        }
      } else if (typeof current === "string") {
        if (!fits(current) || bytes + utf8.encode(current).length > LIMITS.bytes) return limit(name, currentPath, "Limite de 256 KiB atingido");
        bytes += utf8.encode(current).length;
        node.preview = current.slice(0, LIMITS.preview) + (current.length > LIMITS.preview ? "… [prévia abreviada]" : "");
        if (["[oculto]", "[hash protegido]", "[chave privada oculta]"].includes(current)) notices.add("Valor já protegido na fonte; original indisponível neste registro");
        if (transforms < LIMITS.transforms && count < LIMITS.nodes) {
          const decoded = interpret(current);
          if (decoded?.notice) notices.add(decoded.notice);
          if (decoded && !decoded.declined) node.children.push(visit(decoded.value, decoded.label, `${currentPath}::${decoded.kind}`, depth + 1, transforms + 1));
        }
      } else node.preview = typeof current === "function" ? "[função não avaliada]" : String(current);
      return node;
    }
    return { root: visit(value, label, path, 0, 0), notices: [...notices] };
  }

  // Copy is requested only by a button. Bound serialization separately from the visible tree.
  function copyText(value) {
    if (typeof value === "string") return fits(value) ? value : null;
    let nodes = 0, bytes = 0;
    const seen = new WeakSet();
    const refused = Symbol("limited");
    function clone(current, depth) {
      if (++nodes > LIMITS.nodes || depth > LIMITS.depth) throw refused;
      if (typeof current === "number" && !Number.isFinite(current)) throw refused;
      if (typeof current === "string") { if (!fits(current)) throw refused; bytes += utf8.encode(current).length; }
      else if (current && typeof current === "object") {
        if (seen.has(current)) throw refused;
        seen.add(current);
        // Serialize only our own data-only clone: inherited/nonenumerable toJSON
        // and getters on the source must never run, even during explicit copy.
        const array = Array.isArray(current);
        if (array && current.length > LIMITS.nodes) throw refused;
        const result = array ? Object.setPrototypeOf(new Array(current.length), null) : Object.create(null);
        for (const key in current) {
          if (!Object.prototype.hasOwnProperty.call(current, key)) continue;
          if (!fits(key)) throw refused;
          bytes += utf8.encode(key).length;
          if (bytes > LIMITS.bytes) throw refused;
          const property = Object.getOwnPropertyDescriptor(current, key);
          if (!property || !("value" in property)) throw refused;
          result[key] = clone(property.value, depth + 1);
        }
        return result;
      } else if (!["number", "boolean", "undefined"].includes(typeof current) && current !== null) throw refused;
      if (bytes > LIMITS.bytes) throw refused;
      return current;
    }
    try { const text = JSON.stringify(clone(value, 0), null, 2); return typeof text === "string" && fits(text) ? text : null; }
    catch { return null; }
  }

  const element = (tag, className, text) => { const node = document.createElement(tag); node.className = className; if (text !== undefined) node.textContent = text; return node; };
  function close() { active?.(); }
  function open(value, { label = "Valor", path = "$", revealed = false } = {}) {
    close();
    const previous = document.activeElement, dialog = element("dialog", "evidence-inspector value-inspector");
    dialog.setAttribute("aria-label", "Inspecionar estrutura do valor");
    const header = element("header", ""), heading = element("div", "");
    const short = text => String(text).slice(0, LIMITS.preview) + (String(text).length > LIMITS.preview ? "…" : "");
    heading.append(element("small", "", "Inspeção local"), element("h3", "", short(label)));
    const dismiss = element("button", "btn ghost small", "Fechar"); dismiss.type = "button";
    header.append(heading, dismiss);
    const body = element("div", "evidence-inspector-body"), toggle = element("button", "btn ghost small"); toggle.type = "button";
    const note = element("p", "muted small", "Estruturas interpretadas, sem verificação de assinatura ou autenticidade. Datas de JWT são apenas declarações do token. Caminhos com :: são virtuais, disponíveis somente neste inspetor.");
    const tree = element("div", "value-tree"), status = element("p", "muted small"); status.setAttribute("role", "status");
    body.append(toggle, note, tree, status); dialog.append(header, body);
    let visible = !!revealed;
    const cleanup = () => {
      if (active !== cleanup) return;
      active = null; tree.replaceChildren(); value = null;
      dialog.close(); dialog.remove(); if (previous?.isConnected) previous.focus();
    };
    active = cleanup;
    const copy = async getter => {
      const text = getter();
      if (text === null) { status.textContent = "Valor não copiado: excede os limites da inspeção ou contém uma referência não serializável."; return; }
      try { await navigator.clipboard.writeText(text); if (dialog.isConnected) status.textContent = "Copiado."; }
      catch { if (dialog.isConnected) status.textContent = "Não foi possível copiar."; }
    };
    function renderNode(node, depth = 0) {
      const branch = element(node.children.length ? "details" : "div", "value-node");
      if (node.children.length) branch.open = depth === 0;
      const line = element(node.children.length ? "summary" : "div", "value-node-line");
      line.append(element("strong", "value-node-key", short(node.label)), element("span", "value-node-preview mono", node.preview));
      branch.append(line);
      const actions = element("div", "value-node-actions");
      const pathButton = element("button", "text-button", "Copiar caminho"); pathButton.type = "button";
      pathButton.onclick = event => { event.preventDefault(); event.stopPropagation(); copy(() => node.path); };
      actions.append(pathButton);
      if (!node.limited) {
        const valueButton = element("button", "text-button", "Copiar valor"); valueButton.type = "button";
        valueButton.onclick = event => { event.preventDefault(); event.stopPropagation(); copy(() => copyText(node.value)); };
        actions.append(valueButton);
      }
      branch.append(actions);
      for (const child of node.children) branch.append(renderNode(child, depth + 1));
      return branch;
    }
    function render() {
      toggle.textContent = visible ? "Ocultar valor e subcampos" : "Mostrar valor e subcampos";
      toggle.setAttribute("aria-pressed", String(visible));
      tree.replaceChildren(); status.textContent = "";
      if (!visible) { tree.append(element("p", "muted", "[oculto] · Revele para examinar o valor original e seus subcampos.")); return; }
      const model = analyze(value, path);
      tree.append(renderNode(model.root));
      if (model.notices.length) status.textContent = model.notices.join(". ") + ". A evidência original permanece intacta.";
    }
    toggle.onclick = () => { visible = !visible; render(); };
    dismiss.onclick = cleanup;
    dialog.addEventListener("cancel", event => { event.preventDefault(); cleanup(); });
    dialog.addEventListener("click", event => { if (event.target === dialog) { const r = dialog.getBoundingClientRect(); if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) cleanup(); } });
    document.body.append(dialog); render(); dialog.showModal(); toggle.focus();
    return cleanup;
  }
  if (typeof document !== "undefined") document.addEventListener("workspace-context-change", close);
  return { analyze, copyText, open, close, limits: LIMITS };
})();
