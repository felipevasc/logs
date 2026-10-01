/* Search box: validation while typing, field and value completion, syntax help on demand. */
window.QueryBar = (() => {
  "use strict";
  const input = $("#quick-search");
  if (!input) return { status() {} };
  const box = input.closest(".search-box");
  box.classList.add("query-box");
  input.placeholder = "Digite e pressione Enter…";
  input.setAttribute("autocomplete", "off");
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-label", "Rascunho de busca; Enter adiciona como filtro");
  input.setAttribute("aria-invalid", "false");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-expanded", "false");
  input.setAttribute("aria-controls", "quick-search-suggestions");

  const help = el("button", "query-help");
  help.type = "button";
  help.setAttribute("aria-label", "Como buscar");
  help.innerHTML = '<i class="fas fa-circle-question" aria-hidden="true"></i>';
  box.append(help);
  const error = el("div", "query-error");
  error.id = "quick-search-error";
  error.hidden = true;
  box.after(error);
  const list = el("div", "query-suggest");
  list.id = "quick-search-suggestions";
  list.setAttribute("role", "listbox");
  list.hidden = true;
  document.body.append(list);

  const EXAMPLES = [
    ["falha login", "texto em qualquer campo"],
    ["user:admin", "valor de um campo"],
    ["code>=500", "comparação numérica"],
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
    input.setAttribute("aria-invalid", String(!!problem));
    if (problem) input.setAttribute("aria-errormessage", error.id);
    else input.removeAttribute("aria-errormessage");
    error.textContent = problem || "";
    error.hidden = !problem || document.activeElement !== input;
  }
  input.addEventListener("focus", () => { error.hidden = !box.classList.contains("invalid"); suggest(); });
  input.addEventListener("blur", () => { setTimeout(() => { if (document.activeElement !== input) { dismissSuggestions(); error.hidden = true; } }, 150); });

  // ---------------------------------------------------------------- completion
  const ROLES = ["@user", "@src_ip", "@dst_ip", "@host", "@process", "@parent_process", "@cmdline", "@url", "@domain", "@hash", "@dst_port", "@user_agent", "@file", "@status", "@action", "@outcome", "@src_scope", "@dst_scope", "@tool"];
  const ACTIONS = ["logon", "logoff", "process_start", "network_connection", "dns_query", "http_request", "service_install", "task_create", "account_create", "group_member_add", "password_change", "log_clear", "privilege_use", "script_execution", "file_create", "registry_change", "ids_alert", "malware_detected"];
  let items = [], active = -1, token = null, valueCache = new Map(), serial = 0, suggestionNotice = "";
  const fold = text => text.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
  const sourceIdentities = new WeakMap(); let nextSourceIdentity = 0;
  function sourceIdentity(value) {
    if (value == null || typeof value !== "object") return null;
    if (!sourceIdentities.has(value)) sourceIdentities.set(value, ++nextSourceIdentity);
    return sourceIdentities.get(value);
  }

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
    const scope = workspaceScope(), sourceKey = window.Workspace?.sourceKey?.();
    const key = JSON.stringify([scope, sourceKey, state.datasetRevision, resolved]);
    // The tree retains useful previous results while refreshing. Only take
    // counts whose selection/revision prefix still matches the current source.
    const facetPrefix = [scope, state.datasetRevision, JSON.stringify(state.derivedFields), JSON.stringify(backendFilters())].join("|") + "|";
    const cached = state.treeAggSig?.[scope]?.startsWith(facetPrefix) ? state.treeAgg?.[scope]?.[resolved] : null;
    const prior = valueCache.get(key), rowToken = sourceIdentity(state.rows), facetToken = sourceIdentity(cached);
    let values = prior?.facetToken === facetToken && prior?.rowToken === rowToken ? prior.values : null;
    const sampled = !Array.isArray(cached);
    if (!values) {
      let oversized = false;
      // Completion is local while the search is a draft. Reuse already
      // displayed facets/rows instead of launching a full aggregation per token.
      if (Array.isArray(cached)) values = cached.slice(0, 200).map(([value, count]) => ({ value, count }));
      else {
        const seen = new Map();
        for (const event of state.rows || []) {
          const value = resolved.startsWith("@") ? window.QueryLang.fieldValue(event, window.QueryLang.resolve(resolved)) : cellValue(event, resolved);
          const text = value == null ? "" : String(value);
          if (text.length > 4096) { oversized = true; continue; }
          if (text !== "") seen.set(text, (seen.get(text) || 0) + 1);
          if (seen.size >= 200) break;
        }
        values = [...seen].map(([value, count]) => ({ value, count }));
      }
      // Completion is optional. Do not normalize/render or insert megabyte
      // values into the search box; the full-value composer remains available.
      oversized ||= values.some(item => String(item.value ?? "").length > 4096);
      values = values.filter(item => item.value != null && String(item.value).length <= 4096);
      // Tokens invalidate by identity without retaining whole old pages/facet arrays.
      valueCache.set(key, { values, facetToken, rowToken, oversized });
      if (valueCache.size > 40) valueCache.delete(valueCache.keys().next().value);
    }
    const query = fold(prefix);
    const found = values.filter(v => fold(String(v.value)).includes(query)).slice(0, 8).map(v => {
      const text = String(v.value);
      return { text: `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`, label: text, detail: `${fmtNum(v.count)} ${sampled ? "nesta página" : "no painel de campos"}` };
    });
    found.notice = valueCache.get(key)?.oversized ? "Valores longos foram omitidos das sugestões. Abra o evento e use Criar filtro para o valor completo." : "";
    return found;
  }
  function suggestionLabel(value) {
    const text = String(value ?? "");
    if (text.length <= 200) return text;
    let end = 199;
    if (/[\uD800-\uDBFF]/.test(text[end - 1]) && /[\uDC00-\uDFFF]/.test(text[end])) end--;
    return text.slice(0, end) + "…";
  }
  function dismissSuggestions() {
    clearTimeout(suggestTimer); serial++; items = []; suggestionNotice = ""; active = -1; draw();
  }
  function draw() {
    list.hidden = !items.length && !suggestionNotice;
    input.setAttribute("aria-expanded", String(!list.hidden));
    if (!list.hidden && active >= 0 && items[active]) input.setAttribute("aria-activedescendant", `${list.id}-${active}`);
    else input.removeAttribute("aria-activedescendant");
    if (list.hidden) return;
    list.innerHTML = items.map((item, i) => `<div id="${list.id}-${i}" class="query-option${i === active ? " active" : ""}" role="option" aria-selected="${i === active}" data-index="${i}"><span>${esc(suggestionLabel(item.label))}</span><small>${esc(suggestionLabel(item.detail || ""))}</small></div>`).join("")
      + (suggestionNotice ? `<div class="query-option muted small" role="note">${esc(suggestionNotice)}</div>` : "");
    const r = box.getBoundingClientRect();
    list.style.left = `${r.left}px`; list.style.top = `${r.bottom + 4}px`; list.style.minWidth = `${Math.max(r.width, 260)}px`;
  }
  async function suggest() {
    const mine = ++serial;
    const requestToken = currentToken();
    token = requestToken;
    if (!token || composing || document.activeElement !== input) { items = []; suggestionNotice = ""; draw(); return; }
    const found = token.kind === "field" ? fieldOptions(token.prefix) : await valueOptions(token.field, token.prefix);
    if (mine !== serial || document.activeElement !== input) return;
    // A complete field name needs no suggestion of itself.
    suggestionNotice = found.notice || "";
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
    items = []; suggestionNotice = ""; draw();
    input.dispatchEvent(new Event("input", { bubbles: true }));
    if (token.kind === "field") suggest();
    return true;
  }
  let suggestTimer = null;
  input.addEventListener("input", () => {
    clearTimeout(suggestTimer);
    // Invalidate before the debounce, including async suggestions for the
    // same field/prefix. Typing never starts a native data query or a save.
    serial++; items = []; suggestionNotice = ""; draw();
    if (!composing) suggestTimer = setTimeout(suggest, 90);
  });
  let composing = false;
  input.addEventListener("compositionstart", () => { composing = true; clearTimeout(suggestTimer); serial++; items = []; suggestionNotice = ""; draw(); });
  input.addEventListener("compositionend", () => { composing = false; clearTimeout(suggestTimer); suggestTimer = setTimeout(suggest, 90); });
  function clearDraft() { clearTimeout(suggestTimer); serial++; items = []; suggestionNotice = ""; draw(); status(null); }
  function clearCache() { valueCache.clear(); clearTimeout(suggestTimer); serial++; items = []; suggestionNotice = ""; draw(); }
  function captureDraft() {
    return { value: input.value, start: input.selectionStart, end: input.selectionEnd, direction: input.selectionDirection };
  }
  function restoreDraft(draft) {
    clearDraft(); composing = false;
    input.value = typeof draft?.value === "string" ? draft.value : "";
    const bounded = n => Number.isInteger(n) ? Math.max(0, Math.min(input.value.length, n)) : input.value.length;
    const start = bounded(draft?.start), end = Math.max(start, bounded(draft?.end));
    input.setSelectionRange(start, end, ["forward", "backward"].includes(draft?.direction) ? draft.direction : "none");
    $("#btn-add-search").disabled = !input.value.trim();
    status(window.QueryLang?.validate(input.value) || null);
  }
  function submit() { if (composing) return false; const applied = commitQuickSearch(); if (applied) clearDraft(); return applied; }
  input.addEventListener("keydown", event => {
    if (event.isComposing || composing || event.keyCode === 229) return;
    if (event.key === "Enter") { event.preventDefault(); if (!event.repeat) submit(); return; }
    if (event.key === "Escape") {
      if (!list.hidden) { event.preventDefault(); event.stopPropagation(); }
      dismissSuggestions(); return;
    }
    if (list.hidden || !items.length) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      active = (active + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      draw();
    } else if (event.key === "Tab" && active >= 0) {
      event.preventDefault();
      accept();
    }
  });
  list.addEventListener("mousedown", event => {
    const option = event.target.closest("[data-index]");
    if (!option) return;
    event.preventDefault();
    accept(+option.dataset.index);
  });
  document.addEventListener("workspace-context-change", clearCache);
  // A canonical field name still being typed ("@us") is not a text search yet.
  function typingField() {
    const t = currentToken();
    return !!t && t.kind === "field" && t.prefix.startsWith("@") && ROLES.some(r => r !== t.prefix && r.startsWith(t.prefix));
  }
  // Workspace restoration may run before this script. Synchronize the existing
  // draft without applying it, querying, saving or moving keyboard focus.
  restoreDraft(captureDraft());
  return { status, typingField, submit, clearDraft, captureDraft, restoreDraft, clearCache };
})();
