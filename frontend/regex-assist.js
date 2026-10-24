/* Assisted construction on top of the expression editor: a small menu (also on the
 * "Ajudar a montar" button, no right click needed) and compact panels that show the
 * generated expression, its plain-language reading and a native test on the
 * current sample before anything is inserted. Selections taken from example text are
 * literal; selections taken from the expression are syntax. */
window.RegexAssist = (() => {
  "use strict";
  const S = () => window.RegexSyntax;
  const e = (t) => String(t ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const ENGINE_NAME = { literal: "Texto literal", rust: "Regex rápida (Rust)", pcre2: "PCRE2 avançado" };
  const PCRE_ONLY = new Set(["lookahead", "lookbehind", "atomic", "conditional", "branchreset", "scriptrun"]);
  /** Engine a generated expression needs, or null when the current one suffices. */
  function requiredEngine(text, current) {
    const a = S().analyze(text);
    const pcre = a.groups.some((g) => PCRE_ONLY.has(g.kind)) || a.references.some((r) => r.type !== undefined && (r.type === "backref" || r.type === "subroutine" || r.type === "condition")) || /[*+?}][+]/.test(text.replace(/\\./g, ""));
    if (pcre) return current === "pcre2" ? null : "pcre2";
    return current === "literal" ? "rust" : null;
  }
  const featureNames = (text) => {
    const a = S().analyze(text), out = [];
    if (a.groups.some((g) => g.kind === "lookbehind")) out.push("lookbehind");
    if (a.groups.some((g) => g.kind === "lookahead")) out.push("lookahead");
    if (a.references.some((r) => r.type === "backref")) out.push("backreference");
    return out;
  };

  function attach(wb) {
    let pop = null, popAnchor = null, tracked = null, popSerial = 0, previewTimer = 0;
    const menu = window.ContextMenu?.create({ fallbackFocus: () => [wb.editor.input] });

    // ---- remembered selection (menus steal focus; the selection must survive)
    function remember(source, info) {
      if (info && !info.collapsed && info.text) tracked = { source, text: info.text, start: info.start, end: info.end, wrap: info.wrap, version: wb.version(), stamp: window.LocalNative?.stamp?.() };
    }
    wb.editor.on("selection", (info) => remember("editor", info));
    wb.sample.on("selection", (info) => remember("sample", info));
    const current = () => tracked && tracked.version === wb.version() && tracked.stamp === window.LocalNative?.stamp?.() ? tracked : null;

    function close() {
      popSerial++; clearTimeout(previewTimer);
      window.LocalNative?.cancel(`rx-assist-${wb.id}`);
      if (pop) { pop.remove(); pop = null; }
    }
    // The panel grows as previews arrive. It is placed once, with room for its tallest
    // state, so it never moves under the pointer while the user is aiming at a button.
    const POP_ROOM = 560;
    function reposition() {
      if (!pop) return;
      const box = (popAnchor || wb.editor.root).getBoundingClientRect(), w = pop.offsetWidth;
      pop.style.maxHeight = `${Math.min(POP_ROOM, innerHeight - 16)}px`;
      pop.style.left = `${Math.max(8, Math.min(box.left, innerWidth - w - 8))}px`;
      pop.style.top = `${Math.max(8, Math.min(box.bottom + 6, innerHeight - Math.min(POP_ROOM, innerHeight - 16) - 8))}px`;
    }
    function place(node, anchor) {
      popAnchor = anchor;
      document.body.append(node);
      reposition();
    }
    function open(html, anchor, label) {
      close();
      pop = document.createElement("div");
      pop.className = "rxw-pop"; pop.setAttribute("role", "dialog"); pop.setAttribute("aria-label", label); pop.innerHTML = html;
      pop.addEventListener("keydown", (ev) => {
        if (ev.key === "Escape") { ev.stopPropagation(); close(); wb.editor.focus(); return; }
        if (ev.key === "Tab") {
          const f = [...pop.querySelectorAll("button,input,select,textarea")].filter((n) => !n.disabled && n.offsetParent !== null);
          if (!f.length) return;
          if (ev.shiftKey && document.activeElement === f[0]) { ev.preventDefault(); f.at(-1).focus(); }
          else if (!ev.shiftKey && document.activeElement === f.at(-1)) { ev.preventDefault(); f[0].focus(); }
        }
      });
      place(pop, anchor);
      // Keyboard users land inside the panel; Escape then closes it from anywhere in it.
      (pop.querySelector(".btn.primary") || pop.querySelector("input,select,button"))?.focus({ preventScroll: true });
      return pop;
    }
    document.addEventListener("pointerdown", (ev) => { if (pop && !pop.contains(ev.target) && !ev.target.closest?.("[data-rxw-assist]")) close(); }, true);

    // ---- placement of generated text
    function placementButtons(need) {
      const empty = !wb.editor.value.trim();
      const engine = wb.getDraft().engine;
      const switching = need && need !== engine;
      const label = (t) => (switching ? `${t} e usar ${ENGINE_NAME[need]}` : t);
      const buttons = empty
        ? [{ id: "replace", text: label("Usar na expressão"), primary: true }]
        : [{ id: "cursor", text: label("Inserir no cursor"), primary: true }, { id: "append", text: "Acrescentar ao final" }, { id: "replace", text: "Substituir a expressão" }];
      const keep = switching ? [{ id: "keep-" + buttons[0].id, text: `Inserir mantendo ${ENGINE_NAME[engine]}`, quiet: true }] : [];
      return { buttons, keep, switching };
    }
    function insertGenerated(mode, text, need, switchEngine) {
      const editor = wb.editor, value = editor.value;
      const engineNow = wb.getDraft().engine;
      if (switchEngine && need && need !== engineNow) wb.setEngine(need);
      const sel = editor.input.selectionStart, end = editor.input.selectionEnd;
      if (mode === "replace") editor.replace(0, value.length, text, { select: [text.length, text.length] });
      else if (mode === "append") editor.replace(value.length, value.length, text, { select: [value.length, value.length + text.length] });
      else editor.replace(sel, end, text, { select: [sel, sel + text.length] });
      editor.focus();
    }
    function engineBanner(text) {
      const need = requiredEngine(text, wb.getDraft().engine), draft = wb.getDraft();
      if (!need || need === draft.engine) return { need: null, html: "" };
      const names = featureNames(text);
      const what = names.length ? names.join(" e ") : "recursos de expressão regular";
      return { need, html: `<p class="rxw-warn">Esta expressão usa ${e(what)}${need === "pcre2" ? ", que só o PCRE2 oferece" : ""}. Motor atual: ${e(ENGINE_NAME[draft.engine])}. Escolha abaixo se deseja trocar para ${e(ENGINE_NAME[need])}; nada é trocado sem a sua escolha.</p>` };
    }
    /** Native test of a candidate expression on the current sample. */
    function candidateTest(node, text, need) {
      const out = node.querySelector("[data-rx-test]");
      if (!out) return;
      clearTimeout(previewTimer);
      const id = ++popSerial;
      if (!text) { out.textContent = ""; return; }
      out.textContent = "Testando na amostra…";
      previewTimer = setTimeout(async () => {
        const draft = wb.getDraft(), sample = wb.getSampleText();
        try {
          const result = await window.LocalNative.call({ action: "regex_preview", pattern: { engine: need || (draft.engine === "literal" ? "rust" : draft.engine), text, caseSensitive: draft.caseSensitive, multiline: draft.multiline, dotAll: draft.dotAll }, text: sample, scope: draft.scope }, { latest: `rx-assist-${wb.id}` });
          if (id !== popSerial || !pop?.contains(out)) return;
          if (!result.ok) out.innerHTML = `<span class="rxw-err">${e(result.error?.message || "A expressão não foi aceita pelo motor.")}</span>`;
          else if (!result.spans.length) out.textContent = "Nenhuma ocorrência nesta amostra (isso não diz nada sobre o conjunto de eventos).";
          else {
            const shown = result.spans.slice(0, 3).map((s) => `“${e(sample.slice(s.start, s.end) || "(vazio)")}”`).join(", ");
            out.innerHTML = `Na amostra: ${shown}${result.spans.length > 3 ? ` e mais ${result.spans.length - 3}` : ""} · ${result.spans.length} ocorrência${result.spans.length > 1 ? "s" : ""}${result.complete ? "" : " (incompleto: limite do motor)"}.`;
          }
        } catch (error) { if (id === popSerial && !window.LocalNative.superseded(error)) out.textContent = `Não foi possível testar agora: ${String(error.message || error)}`; }
      }, 140);
    }
    function wirePlacement(node, getText) {
      node.querySelectorAll("[data-place]").forEach((b) => (b.onclick = () => {
        const text = getText();
        if (!text) return;
        if (!current() && node.dataset.requiresSelection !== "false") { toastStale(); return; }
        const id = b.dataset.place, keep = id.startsWith("keep-"), mode = keep ? id.slice(5) : id, need = requiredEngine(text, wb.getDraft().engine);
        close(); insertGenerated(mode, text, need, !keep);
      }));
    }
    const toastStale = () => { close(); window.toast?.("O conteúdo mudou desde a seleção. Selecione o trecho novamente.", "info"); };
    const placementHtml = (text) => {
      const need = requiredEngine(text, wb.getDraft().engine), p = placementButtons(need);
      return `${engineBanner(text).html}<div class="rxw-buttons">${p.keep.map((b) => `<button type="button" class="btn ghost small" data-place="${b.id}">${e(b.text)}</button>`).join("")}${p.buttons.slice(1).map((b) => `<button type="button" class="btn ghost small" data-place="${b.id}">${e(b.text)}</button>`).join("")}${p.buttons.slice(0, 1).map((b) => `<button type="button" class="btn primary small" data-place="${b.id}">${e(b.text)}</button>`).join("")}</div>`;
    };

    // ---- value rule chooser shared by "after", "before", "between" and "capture"
    const valueFields = (prefix, { exact = false } = {}) => `<fieldset><legend>Tipo de valor</legend>${exact ? `<label><input type="radio" name="${prefix}-k" value="exact" checked> Exatamente este texto</label>` : ""}${S().VALUE_KINDS.map((k, i) => `<label><input type="radio" name="${prefix}-k" value="${k.id}" ${!exact && i === 0 ? "checked" : ""}> ${e(k.label)} <small style="color:var(--text-2)">${e(k.hint)}</small></label>`).join("")}<label>Delimitador <input type="text" data-f="delimiter" maxlength="16" aria-label="Delimitador" placeholder="ex.: ; ou &&"></label><label>Expressão <input type="text" data-f="custom" maxlength="200" aria-label="Expressão personalizada" placeholder="ex.: [A-Z]{3}\\d+"></label></fieldset>`;
    const readValue = (node, prefix, exactText = null) => {
      const kind = node.querySelector(`input[name="${prefix}-k"]:checked`)?.value || "word";
      if (kind === "exact") {
        const engine = wb.getDraft().engine;
        return { ...{ text: engine === "literal" ? exactText : S().escapeLiteral(exactText, { extended: wb.editor.analysis.features.extended }), ok: true }, kind: "exact" };
      }
      const v = S().valueExpression(kind, { delimiter: node.querySelector('[data-f="delimiter"]')?.value || "", custom: node.querySelector('[data-f="custom"]')?.value || "", extended: wb.editor.analysis.features.extended });
      return { ...v, kind };
    };

    // ---- "Encontrar este texto"
    function findText(sel, anchor) {
      const engine = wb.getDraft().engine, text = engine === "literal" ? sel.text : S().escapeLiteral(sel.text, { extended: wb.editor.analysis.features.extended });
      const node = open(`<h3>Encontrar este texto</h3><p>Procura exatamente “${e(sel.text.length > 60 ? sel.text.slice(0, 60) + "…" : sel.text)}”${engine === "literal" ? "." : ", com todos os caracteres especiais tratados como texto."}</p><div class="rxw-preview" data-rx-out></div><p class="rxw-note" data-rx-test></p>${placementHtml(text)}`, anchor, "Encontrar este texto");
      node.querySelector("[data-rx-out]").textContent = text;
      if (sel.external) node.dataset.requiresSelection = "false";
      candidateTest(node, text, requiredEngine(text, engine));
      wirePlacement(node, () => text);
    }

    // ---- after / before / between
    function relative(direction, sel, anchor) {
      const fromEditor = sel.source === "editor", engine = wb.getDraft().engine;
      const title = direction === "after" ? "Encontrar o que vem depois deste trecho" : direction === "before" ? "Encontrar o que vem antes deste trecho" : "Encontrar entre dois trechos";
      const taken = new Set(wb.editor.analysis.captures.map((g) => g.name).filter(Boolean));
      const suggested = S().captureName(sel.text.replace(/[^\p{L}\p{N}_]+/gu, " ").trim().split(/\s+/).pop() || "valor", taken);
      const node = open(`<h3>${e(title)}</h3><p>${fromEditor ? "O trecho selecionado é usado como está, como parte da expressão." : `Trecho de referência: <code>${e(sel.text.length > 60 ? sel.text.slice(0, 60) + "…" : sel.text)}</code>, tratado como texto literal.`}</p>${direction === "between" ? `<label>Trecho depois do valor <input type="text" data-f="after" maxlength="200" aria-label="Trecho depois do valor"></label>` : ""}${valueFields("rxv")}<label><input type="checkbox" data-f="named" checked> Capturar o valor com nome <input type="text" data-f="name" value="${e(suggested)}" maxlength="32" aria-label="Nome da captura"></label><div class="rxw-preview" data-rx-out></div><p class="rxw-note" data-rx-explain></p><p class="rxw-note" data-rx-test></p><p class="rxw-err" data-rx-err></p><div data-rx-place></div>`, anchor, title);
      const out = node.querySelector("[data-rx-out]"), why = node.querySelector("[data-rx-explain]"), err = node.querySelector("[data-rx-err]"), slot = node.querySelector("[data-rx-place]");
      let generated = "";
      const refresh = () => {
        err.textContent = "";
        const value = readValue(node, "rxv");
        const named = node.querySelector('[data-f="named"]').checked, name = node.querySelector('[data-f="name"]').value.trim();
        node.querySelector('[data-f="name"]').disabled = !named;
        node.querySelector('[data-f="delimiter"]').disabled = node.querySelector('input[name="rxv-k"]:checked')?.value !== "delimiter";
        node.querySelector('[data-f="custom"]').disabled = node.querySelector('input[name="rxv-k"]:checked')?.value !== "custom";
        const trailing = direction === "between" ? node.querySelector('[data-f="after"]').value : "";
        if (!value.ok) { err.textContent = value.reason; generated = ""; }
        else if (named && !S().NAME.test(name)) { err.textContent = "Use letras, dígitos e _ no nome, sem começar por dígito."; generated = ""; }
        else if (direction === "between" && !trailing) { err.textContent = "Informe o trecho que vem depois do valor."; generated = ""; }
        else {
          // From a sample the selection is literal text (escaped by the builder); from the
          // expression it is syntax and stays inside the condition exactly as written.
          const after = direction === "between" ? trailing : sel.text;
          const built = S().relative(direction, { before: direction === "before" ? "" : sel.text, after: direction === "after" ? "" : after, value: { ...value, kind: value.kind }, name: named ? name : null, delimiter: node.querySelector('[data-f="delimiter"]').value, extended: wb.editor.analysis.features.extended });
          generated = fromEditor ? rawRelative(direction, sel.text, value.text, named ? name : null, trailing) : built.text;
          why.textContent = (fromEditor ? "" : built.explanation) || `Resultado: ${generated}`;
        }
        out.textContent = generated || "—";
        slot.innerHTML = generated ? (fromEditor ? `<div class="rxw-buttons"><button type="button" class="btn primary small" data-apply-editor>Aplicar na expressão</button></div>${engineBanner(generated).html}` : placementHtml(generated)) : "";
        if (generated) candidateTest(node, fromEditor ? wholeWith(sel, generated) : generated, requiredEngine(generated, engine));
        else node.querySelector("[data-rx-test]").textContent = "";
        if (!fromEditor) wirePlacement(node, () => generated);
        slot.querySelector("[data-apply-editor]")?.addEventListener("click", () => applyInPlace(sel, generated));
      };
      node.addEventListener("input", refresh); node.addEventListener("change", (ev) => { if (ev.target.type !== "text") refresh(); });
      refresh();
      node.querySelector('input[name="rxv-k"]:checked')?.focus();
    }
    // The selection itself stays inside the lookaround: syntax, not escaped text.
    function rawRelative(direction, selected, valueText, name, trailing) {
      const body = name ? S().wrapCapture(valueText, name) : valueText;
      if (direction === "after") return `(?<=${selected})${body}`;
      if (direction === "before") return `${body}(?=${selected})`;
      return `(?<=${selected})${body}(?=${trailing})`;
    }
    const wholeWith = (sel, replacement) => wb.editor.value.slice(0, sel.start) + replacement + wb.editor.value.slice(sel.end);
    function applyInPlace(sel, replacement) {
      if (!current() || wb.editor.value.slice(sel.start, sel.end) !== sel.text) { toastStale(); return; }
      const need = requiredEngine(replacement, wb.getDraft().engine);
      if (need && need !== wb.getDraft().engine) wb.setEngine(need);
      close();
      wb.editor.replace(sel.start, sel.end, replacement, { select: [sel.start, sel.start + replacement.length] });
    }

    // ---- "Capturar trecho"
    function capture(sel, anchor) {
      const fromEditor = sel.source === "editor";
      const a = wb.editor.analysis;
      let literalOnly = !fromEditor;
      if (fromEditor) {
        const parts = a.tokens.filter((t) => t.start >= sel.start && t.end <= sel.end);
        literalOnly = parts.length > 0 && parts.every((t) => t.type === "literal" || (t.type === "escape" && /^\\[^A-Za-z0-9]$/.test(t.char || "")));
      }
      if (fromEditor && !sel.wrap?.ok) { wb.say(sel.wrap?.reason || "Esta seleção não pode virar grupo com segurança."); return; }
      const taken = new Set(a.captures.map((g) => g.name).filter(Boolean));
      const suggested = S().captureName(literalOnly ? sel.text.replace(/[^\p{L}\p{N}_]+/gu, "_").slice(0, 20) : "valor", taken);
      const node = open(`<h3>Capturar trecho</h3><p>${fromEditor ? "Envolve o trecho selecionado em um grupo, sem alterar o que ele significa." : `Cria um grupo para o trecho <code>${e(sel.text.length > 60 ? sel.text.slice(0, 60) + "…" : sel.text)}</code>.`}</p>${literalOnly ? valueFields("rxc", { exact: true }) : ""}<fieldset><legend>Tipo de grupo</legend><label><input type="radio" name="rxc-t" value="named" checked> Com nome <input type="text" data-f="name" value="${e(suggested)}" maxlength="32" aria-label="Nome da captura"></label><label><input type="radio" name="rxc-t" value="numbered"> Numerada</label></fieldset>${literalOnly ? `<p class="rxw-note">Por padrão o exemplo é mantido exatamente como está. Escolher “Palavra” ou outra regra generaliza o exemplo: passa a aceitar qualquer valor desse tipo.</p>` : ""}<div class="rxw-preview" data-rx-out></div><p class="rxw-note" data-rx-test></p><p class="rxw-err" data-rx-err></p><div data-rx-place></div>`, anchor, "Capturar trecho");
      const out = node.querySelector("[data-rx-out]"), err = node.querySelector("[data-rx-err]"), slot = node.querySelector("[data-rx-place]");
      if (sel.external) node.dataset.requiresSelection = "false";
      let generated = "";
      const refresh = () => {
        err.textContent = "";
        const named = node.querySelector('input[name="rxc-t"]:checked').value === "named", name = node.querySelector('[data-f="name"]').value.trim();
        node.querySelector('[data-f="name"]').disabled = !named;
        let inner = fromEditor ? sel.text : (wb.getDraft().engine === "literal" ? sel.text : S().escapeLiteral(sel.text, { extended: a.features.extended }));
        if (literalOnly) {
          const kind = node.querySelector('input[name="rxc-k"]:checked')?.value || "exact";
          node.querySelector('[data-f="delimiter"]').disabled = kind !== "delimiter"; node.querySelector('[data-f="custom"]').disabled = kind !== "custom";
          if (kind !== "exact") {
            const v = readValue(node, "rxc", sel.text);
            if (!v.ok) { err.textContent = v.reason; generated = ""; out.textContent = "—"; slot.innerHTML = ""; return; }
            inner = v.text;
          } else inner = fromEditor ? sel.text : inner;
        }
        if (named && !S().NAME.test(name)) { err.textContent = "Use letras, dígitos e _ no nome, sem começar por dígito."; generated = ""; out.textContent = "—"; slot.innerHTML = ""; return; }
        if (named && taken.has(name)) { err.textContent = `Já existe um grupo chamado “${name}”.`; generated = ""; out.textContent = "—"; slot.innerHTML = ""; return; }
        generated = S().wrapCapture(inner, named ? name : null);
        out.textContent = fromEditor ? wholeWith(sel, generated) : generated;
        const needsEngine = requiredEngine(generated, wb.getDraft().engine);
        slot.innerHTML = fromEditor ? `<div class="rxw-buttons"><button type="button" class="btn primary small" data-apply-editor>Aplicar na expressão</button></div>` : placementHtml(generated);
        candidateTest(node, fromEditor ? wholeWith(sel, generated) : generated, needsEngine);
        if (!fromEditor) wirePlacement(node, () => generated);
        slot.querySelector("[data-apply-editor]")?.addEventListener("click", () => applyInPlace(sel, generated));
      };
      node.addEventListener("input", refresh); node.addEventListener("change", (ev) => { if (ev.target.type !== "text") refresh(); });
      refresh();
      (node.querySelector('[data-f="name"]'))?.focus();
    }

    // ---- "Repetir valor capturado"
    function backref(anchor) {
      const a = wb.editor.analysis, caret = wb.editor.input.selectionStart, end = wb.editor.input.selectionEnd;
      const list = a.captures.filter((g) => !g.autoNoncapture);
      if (!list.length) { wb.say("Crie um grupo de captura antes de repetir o seu valor."); return; }
      const observed = wb.observedValues();
      const node = open(`<h3>Repetir valor capturado</h3><p>Insere uma referência que exige o <b>mesmo texto</b> já capturado pelo grupo, e não apenas a mesma regra. Posição: ${caret}.</p><div class="rxw-list">${list.map((g) => `<button type="button" data-g="${g.id}" class="rx-c-${wb.editor.slotOf(g.id)}"><span class="rx-chip rx-c-${wb.editor.slotOf(g.id)}" style="cursor:inherit"><b>${g.index}</b>${g.name ? `<span>${e(g.name)}</span>` : ""}</span><code>${e(a.text.slice(g.start, g.end ?? a.text.length).slice(0, 48))}</code>${observed.has(g.index) ? `<em style="font:11px var(--font-mono);font-style:normal">= ${e(String(observed.get(g.index)).slice(0, 24))}</em>` : ""}${g.start >= caret ? '<small style="color:var(--text-2)">definido depois</small>' : g.end > caret && g.start < caret ? '<small style="color:var(--text-2)">dentro do próprio grupo</small>' : ""}</button>`).join("")}</div>`, anchor, "Repetir valor capturado");
      const stamp = wb.version();
      node.querySelectorAll("[data-g]").forEach((b) => (b.onclick = () => {
        if (stamp !== wb.version()) { toastStale(); return; }
        const g = a.groups[+b.dataset.g], text = S().referenceText(g, a.text[end] || "");
        const need = requiredEngine(text, wb.getDraft().engine);
        if (need && need !== wb.getDraft().engine) wb.setEngine(need);
        close();
        wb.editor.replace(caret, end, text, { select: [caret + text.length, caret + text.length] });
      }));
      node.querySelector("[data-g]")?.focus();
    }

    // ---- rename
    function rename(group, anchor) {
      const node = open(`<h3>Renomear captura</h3><p>Troca o nome do grupo e das referências a ele (<code>\\k&lt;nome&gt;</code>, <code>(?P=nome)</code> …). Textos literais e comentários não são tocados.</p><label>Novo nome <input type="text" data-f="name" value="${e(group.name)}" maxlength="32" aria-label="Novo nome"></label><p class="rxw-err" data-rx-err></p><div class="rxw-buttons"><button type="button" class="btn primary small" data-ok>Renomear</button></div>`, anchor, "Renomear captura");
      const input = node.querySelector("input"), err = node.querySelector("[data-rx-err]"), stamp = wb.version();
      input.select(); input.focus();
      const go = () => {
        if (stamp !== wb.version()) { toastStale(); return; }
        const result = S().rename(wb.editor.analysis, group.name, input.value.trim());
        if (!result.ok) { err.textContent = result.reason; return; }
        close();
        wb.editor.apply(S().applyEdits(wb.editor.value, result.edits));
      };
      node.querySelector("[data-ok]").onclick = go;
      input.onkeydown = (ev) => { if (ev.key === "Enter") { ev.preventDefault(); go(); } };
    }

    // ---- menu
    function openMenu(anchor, ev) {
      const sel = current(), a = wb.editor.analysis, caret = wb.editor.input.selectionStart;
      const hit = S().at(a, caret);
      const group = hit.group?.kind === "capture" ? hit.group : hit.ref?.targets.length === 1 ? a.groups[hit.ref.targets[0]] : null;
      const literalSel = sel && sel.source === "sample", syntaxSel = sel && sel.source === "editor" && sel.wrap?.ok;
      const usable = literalSel || syntaxSel;
      const why = !sel ? "Selecione um trecho no exemplo ou na expressão." : sel.source === "editor" && !sel.wrap?.ok ? sel.wrap.reason : "";
      const rect = (anchor || wb.editor.root).getBoundingClientRect();
      const run = (fn) => () => { const s = current(); if (!s) { toastStale(); return; } fn(s, anchor || wb.editor.root); };
      const items = [
        { icon: "fa-quote-left", label: "Encontrar este texto", disabled: !literalSel, title: literalSel ? "" : "Disponível ao selecionar texto no exemplo", onClick: run((s, n) => findText(s, n)) },
        { icon: "fa-arrow-right-to-bracket", label: "Encontrar o que vem depois", disabled: !usable, title: usable ? "" : why, onClick: run((s, n) => relative("after", s, n)) },
        { icon: "fa-arrow-right-from-bracket", label: "Encontrar o que vem antes", disabled: !usable, title: usable ? "" : why, onClick: run((s, n) => relative("before", s, n)) },
        { icon: "fa-left-right", label: "Encontrar entre dois trechos", disabled: !usable, title: usable ? "" : why, onClick: run((s, n) => relative("between", s, n)) },
        { icon: "fa-brackets-round", label: "Capturar trecho", disabled: !usable, title: usable ? "" : why, onClick: run((s, n) => capture(s, n)) },
        { sep: true },
        { icon: "fa-rotate-left", label: "Repetir valor capturado", disabled: !a.captures.length, title: a.captures.length ? "Insere uma referência ao valor de um grupo no cursor" : "Nenhum grupo de captura na expressão", onClick: () => backref(anchor || wb.editor.root) },
      ];
      if (group) {
        items.push({ sep: true },
          { icon: "fa-location-crosshairs", label: "Ir à definição do grupo", onClick: () => wb.editor.selectGroup(group.id) },
          { icon: "fa-link", label: `Ver onde o grupo ${group.index} é referenciado`, disabled: !(a.refsByGroup.get(group.id)?.length), title: a.refsByGroup.get(group.id)?.length ? "" : "Nenhuma referência", onClick: () => { wb.editor.selectGroup(group.id); wb.editor.cycle(1); } });
        if (group.name) items.push({ icon: "fa-pen", label: "Renomear captura", onClick: () => rename(group, anchor || wb.editor.root) });
        if (wb.canSaveCapture) items.push({ icon: "fa-floppy-disk", label: "Salvar captura como campo", onClick: () => wb.saveCapture(group) });
      }
      menu.open(ev?.type === "contextmenu" ? ev.clientX : rect.left, ev?.type === "contextmenu" ? ev.clientY : rect.bottom, items, { trigger: anchor, label: "Ajudar a montar a expressão" });
    }
    return { openMenu, close, backref, rename, capture, relative, findText, requiredEngine };
  }
  return { attach, requiredEngine, ENGINE_NAME };
})();
