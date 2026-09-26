/* Search box: validation while typing, field and value completion, syntax help on demand. */
window.QueryBar = (() => {
  "use strict";
  const input = $("#quick-search");
  if (!input) return { status() {} };
  const box = input.closest(".search-box");
  box.classList.add("query-box");
  input.placeholder = "Buscar ou filtrar…";
  input.setAttribute("autocomplete", "off");
  input.setAttribute("aria-autocomplete", "list");

  const help = el("button", "query-help");
  help.type = "button";
  help.setAttribute("aria-label", "Como buscar");
  help.innerHTML = '<i class="fas fa-circle-question" aria-hidden="true"></i>';
  box.append(help);
  const error = el("div", "query-error");
  error.hidden = true;
  box.after(error);
  const list = el("div", "query-suggest");
  list.setAttribute("role", "listbox");
  list.hidden = true;
  document.body.append(list);

  const EXAMPLES = [
    ["falha login", "texto em qualquer campo"],
    ["user:admin", "valor de um campo"],
    ["status>=500", "comparação numérica"],
    ["ip:10.0.0.0/8", "rede"],
    ["host:web*", "curinga"],
    ["user:(ana OR bruno)", "lista de valores"],
    ["-level:info", "excluir"],
    ["@action:logon @outcome:failure", "entidades de todas as fontes"],
    ["deteccao:auth.bruteforce.source", "registros de uma regra"],
  ];
  const tip = el("div", "query-tip");
  tip.hidden = true;
  tip.innerHTML = `<strong>Como buscar</strong>${EXAMPLES.map(([q, d]) => `<button type="button" data-example="${esc(q)}"><code>${esc(q)}</code><span>${esc(d)}</span></button>`).join("")}<small>Tab completa campos · AND, OR, NOT e parênteses combinam termos</small>`;
  document.body.append(tip);
  const placeTip = () => { const r = box.getBoundingClientRect(); tip.style.left = `${Math.min(r.left, innerWidth - 330)}px`; tip.style.top = `${r.bottom + 6}px`; };
  let tipTimer = null;
  const showTip = () => { clearTimeout(tipTimer); placeTip(); tip.hidden = false; };
  const hideTip = () => { tipTimer = setTimeout(() => { tip.hidden = true; }, 180); };
  help.onmouseenter = showTip; help.onfocus = showTip; help.onmouseleave = hideTip; help.onblur = hideTip;
  help.onclick = () => (tip.hidden ? showTip() : (tip.hidden = true));
  tip.onmouseenter = () => clearTimeout(tipTimer); tip.onmouseleave = hideTip;
  tip.onclick = event => {
    const example = event.target.closest("[data-example]");
    if (!example) return;
    input.value = example.dataset.example; tip.hidden = true; input.focus();
    input.dispatchEvent(new Event("input", { bubbles: true }));
  };

  function status(problem) {
    box.classList.toggle("invalid", !!problem);
    error.textContent = problem || "";
    error.hidden = !problem || document.activeElement !== input;
  }
  input.addEventListener("focus", () => { error.hidden = !box.classList.contains("invalid"); });
  input.addEventListener("blur", () => { setTimeout(() => { list.hidden = true; error.hidden = true; }, 150); });

  // ---------------------------------------------------------------- completion
  const ROLES = ["@user", "@src_ip", "@dst_ip", "@host", "@process", "@parent_process", "@cmdline", "@url", "@domain", "@hash", "@dst_port", "@user_agent", "@file", "@status", "@action", "@outcome", "@src_scope", "@dst_scope", "@tool"];
  const ACTIONS = ["logon", "logoff", "process_start", "network_connection", "dns_query", "http_request", "service_install", "task_create", "account_create", "group_member_add", "password_change", "log_clear", "privilege_use", "script_execution", "file_create", "registry_change", "ids_alert", "malware_detected"];
  let items = [], active = -1, token = null, valueCache = new Map(), serial = 0;
  const fold = text => text.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

  function currentToken() {
    const caret = input.selectionStart ?? input.value.length;
    const before = input.value.slice(0, caret);
    let m = before.match(/(^|[\s(])(-?)([\p{L}@_][\p{L}\p{N}_.@-]*):([^\s()":]*)$/u);
    if (m) return { kind: "value", field: m[3], prefix: m[4], start: caret - m[4].length, end: caret };
    m = before.match(/(^|[\s(])(-?)([\p{L}@_][\p{L}\p{N}_.@-]*)$/u);
    if (m && m[3].length >= 1) return { kind: "field", prefix: m[3], start: caret - m[3].length, end: caret };
    return null;
  }
  function fieldOptions(prefix) {
    const query = fold(prefix);
    const columns = [...new Set([...ROLES, ...(state.columns || [])])].filter(c => c !== "timestamp");
    return columns
      .map(column => ({ column, label: colLabel(column) }))
      .filter(({ column, label }) => fold(column).includes(query) || fold(label).includes(query))
      .sort((a, b) => (fold(a.column).startsWith(query) ? 0 : 1) - (fold(b.column).startsWith(query) ? 0 : 1) || (a.column.startsWith("@") ? 0 : 1) - (b.column.startsWith("@") ? 0 : 1))
      .slice(0, 8)
      .map(({ column, label }) => ({ text: `${column}:`, label, detail: column }));
  }
  async function valueOptions(field, prefix) {
    const resolved = window.QueryLang.resolve(field).role || window.QueryLang.resolve(field).name;
    if (resolved === "@action") return ACTIONS.filter(a => a.startsWith(prefix.toLowerCase())).slice(0, 8).map(a => ({ text: a, label: a, detail: "" }));
    if (resolved === "@outcome") return ["success", "failure"].filter(a => a.startsWith(prefix.toLowerCase())).map(a => ({ text: a, label: a === "success" ? "sucesso" : "falha", detail: a }));
    if (["deteccao", "detecção", "detection"].includes(field.toLowerCase())) {
      let rules = [];
      try { rules = await window.Security?.rules() || []; } catch { /* suggestions are optional */ }
      const q = fold(prefix);
      return rules.filter(r => r.enabled && (fold(r.id).includes(q) || fold(r.name).includes(q))).slice(0, 8).map(r => ({ text: r.id, label: r.name, detail: r.id }));
    }
    if (resolved === "level") return ["erro", "aviso", "informação", "crítico", "depuração"].filter(a => fold(a).startsWith(fold(prefix))).map(a => ({ text: a, label: a, detail: "" }));
    const key = JSON.stringify([workspaceScope(), window.Workspace?.sourceKey?.(), resolved]);
    let values = valueCache.get(key);
    if (!values) {
      try {
        const request = typeof analyticsRequest === "function" ? analyticsRequest(workspaceScope()) : { filters: [] };
        const result = await api("tree_aggs", { ...request, filters: (request.filters || []).filter(f => !f._quick && f.op !== "query"), columns: [resolved] }, { silent: true });
        const agg = result?.[0]?.[1];
        values = (agg?.rows || []).map((row, i) => ({ value: agg.group_values ? agg.group_values[i] : row[agg.columns[0]], count: row[agg.columns[1]] ?? 0 })).filter(v => v.value != null && v.value !== "").slice(0, 200);
      } catch { values = []; }
      valueCache.set(key, values);
      if (valueCache.size > 40) valueCache.delete(valueCache.keys().next().value);
    }
    const query = fold(prefix);
    return values.filter(v => fold(String(v.value)).includes(query)).slice(0, 8).map(v => {
      const text = String(v.value);
      return { text: /[\s():"]/.test(text) ? `"${text.replace(/"/g, '\\"')}"` : text, label: text, detail: fmtNum(v.count) };
    });
  }
  function draw() {
    list.hidden = !items.length;
    if (!items.length) return;
    list.innerHTML = items.map((item, i) => `<div class="query-option${i === active ? " active" : ""}" role="option" data-index="${i}"><span>${esc(item.label)}</span><small>${esc(item.detail || "")}</small></div>`).join("");
    const r = box.getBoundingClientRect();
    list.style.left = `${r.left}px`; list.style.top = `${r.bottom + 4}px`; list.style.minWidth = `${Math.max(r.width, 260)}px`;
  }
  async function suggest() {
    const mine = ++serial;
    token = currentToken();
    if (!token || document.activeElement !== input) { items = []; draw(); return; }
    const found = token.kind === "field" ? fieldOptions(token.prefix) : await valueOptions(token.field, token.prefix);
    if (mine !== serial) return;
    // A complete field name needs no suggestion of itself.
    items = found.filter(item => item.text !== (token.kind === "field" ? `${token.prefix}:` : token.prefix));
    active = items.length ? 0 : -1;
    draw();
  }
  function accept(index = active) {
    const item = items[index];
    if (!item || !token) return false;
    const value = input.value;
    const insert = token.kind === "value" ? `${item.text} ` : item.text;
    input.value = value.slice(0, token.start) + insert + value.slice(token.end);
    const caret = token.start + insert.length;
    input.setSelectionRange(caret, caret);
    items = []; draw();
    input.dispatchEvent(new Event("input", { bubbles: true }));
    if (token.kind === "field") suggest();
    return true;
  }
  let suggestTimer = null;
  input.addEventListener("input", () => {
    clearTimeout(suggestTimer);
    // Suggestions for another token are stale as soon as the text changes.
    const next = currentToken();
    if (!next || !token || next.kind !== token.kind || next.start !== token.start) { items = []; draw(); }
    suggestTimer = setTimeout(suggest, 90);
  });
  input.addEventListener("keydown", event => {
    if (list.hidden) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      active = (active + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      draw();
    } else if ((event.key === "Tab" || event.key === "Enter") && active >= 0) {
      event.preventDefault();
      accept();
    } else if (event.key === "Escape") {
      items = []; draw();
    }
  });
  list.addEventListener("mousedown", event => {
    const option = event.target.closest("[data-index]");
    if (!option) return;
    event.preventDefault();
    accept(+option.dataset.index);
  });
  document.addEventListener("workspace-context-change", () => valueCache.clear());
  // A canonical field name still being typed ("@us") is not a text search yet.
  function typingField() {
    const t = currentToken();
    return !!t && t.kind === "field" && t.prefix.startsWith("@") && ROLES.some(r => r !== t.prefix && r.startsWith(t.prefix));
  }
  return { status, typingField, clearCache: () => valueCache.clear() };
})();
