/* Expression workbench: the editor, its options, a sample test and the explicit
 * apply step. Drafting, testing on a sample and applying to the event set are
 * separate: typing and testing never scan the logs, and Apply is a button. */
window.RegexWorkbench = (() => {
  "use strict";
  const S = () => window.RegexSyntax;
  const e = (t) => String(t ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const ENGINES = [["rust", "Regex rápida (Rust)"], ["pcre2", "PCRE2 avançado"], ["literal", "Texto literal"]];
  const SAMPLE_BYTES = 1 << 20;
  let counter = 0;

  /** Editable example text with a highlight layer built from native ranges. */
  function createSample(host) {
    const root = document.createElement("div");
    root.className = "rx-editor";
    root.innerHTML = '<div class="rx-field"><pre class="rx-backdrop" aria-hidden="true"><code></code></pre><textarea class="rx-input" spellcheck="false" autocomplete="off" aria-label="Amostra editável"></textarea></div>';
    host.append(root);
    const code = root.querySelector("code"), input = root.querySelector("textarea"), listeners = [];
    // The textarea cannot hold CR; the true text (with CRLF) is kept until the user edits it.
    let truth = "", edited = false;
    const crMap = () => { const map = []; for (let i = 0; i < truth.length; i++) if (truth[i] !== "\r" || truth[i + 1] !== "\n") map.push(i); map.push(truth.length); return map; };
    const toTruth = (index) => (edited || !truth.includes("\r") ? index : crMap()[Math.min(index, crMap().length - 1)]);
    const text = () => (edited ? input.value : truth);
    function paint(html) { code.innerHTML = html; code.parentElement.scrollTop = input.scrollTop; code.parentElement.scrollLeft = input.scrollLeft; }
    function renderPlain() { paint(e(text()) + (text().endsWith("\n") ? " " : "")); }
    input.addEventListener("scroll", () => { code.parentElement.scrollTop = input.scrollTop; code.parentElement.scrollLeft = input.scrollLeft; });
    input.addEventListener("input", () => { edited = true; truth = input.value; renderPlain(); for (const fn of listeners.filter((l) => l.t === "change")) fn.f(text()); });
    const select = () => {
      const a = toTruth(input.selectionStart), b = toTruth(input.selectionEnd);
      const info = { start: a, end: b, text: text().slice(a, b), collapsed: a === b, active: document.activeElement === input };
      for (const fn of listeners.filter((l) => l.t === "selection")) fn.f(info);
    };
    for (const type of ["select", "keyup", "mouseup", "blur"]) input.addEventListener(type, select);
    return {
      root, input, get text() { return text(); },
      set(value, { fromUser = false } = {}) { truth = String(value ?? ""); edited = fromUser; input.value = truth; renderPlain(); },
      paint, plainHtml: () => e(text()), hasCR: () => !edited && truth.includes("\r"),
      on(t, f) { listeners.push({ t, f }); },
      focus: () => input.focus(),
    };
  }

  function create(host, cfg = {}) {
    const id = ++counter, mode = cfg.mode || "search";
    const draft = {
      field: cfg.field || "message", engine: cfg.engine || "rust", pattern: cfg.pattern || "", caseSensitive: !!cfg.caseSensitive,
      multiline: !!cfg.multiline, dotAll: !!cfg.dotAll, scope: cfg.scope || "event",
    };
    const fields = cfg.fields?.length ? cfg.fields : [["message", "Mensagem"], ["raw", "Conteúdo bruto"]];
    const root = document.createElement("section");
    root.className = "rxw";
    root.innerHTML = `<div class="rxw-bar">${mode === "search" ? `<label>Campo pesquisado<select data-f="field" aria-label="Campo pesquisado">${fields.map(([v, l]) => `<option value="${e(v)}">${e(l)}</option>`).join("")}</select></label>` : ""}<label>Motor<select data-f="engine" aria-label="Motor">${ENGINES.map(([v, l]) => `<option value="${v}">${l}</option>`).join("")}</select></label><label class="rxw-check"><input type="checkbox" data-f="caseSensitive"> Maiúsculas importam</label><details class="rxw-advanced"><summary>Avançado</summary><div class="rxw-advanced-body"><label><input type="checkbox" data-f="multiline"> <span><code>^</code> e <code>$</code> valem em cada linha</span></label><label><input type="checkbox" data-f="dotAll"> <span>O ponto inclui a quebra de linha</span></label><label>Escopo <select data-f="scope" aria-label="Escopo"><option value="event">Registro completo</option><option value="line">Linha física</option></select></label></div></details></div><div class="rxw-edit-row"><div data-slot="editor" style="flex:1;min-width:0"></div><button type="button" class="btn ghost small" data-rxw-assist data-act="assist" title="Montar a expressão a partir de uma seleção (menu também no botão direito)" aria-haspopup="menu"><i class="fas fa-wand-magic-sparkles" aria-hidden="true"></i> Ajudar a montar</button></div><div class="rxw-status" data-slot="status" role="status" aria-live="polite"></div><details class="rxw-sample" ${cfg.sampleOpen ? "open" : ""}><summary>Testar em uma amostra <small style="color:var(--text-2)">— não pesquisa o conjunto de eventos</small></summary><div class="rxw-sample-body"><div class="rxw-sample-bar"><button type="button" class="btn ghost small" data-act="use-event" ${cfg.sampleProvider ? "" : "hidden"}>Usar o texto do evento aberto</button><span data-slot="sample-note"></span></div><div data-slot="sample"></div><div class="rxw-occ" data-slot="occ"></div><div data-slot="captures"></div></div></details><div class="rxw-actions"><button type="button" class="btn primary" data-act="apply" ${cfg.hideApply ? "hidden" : ""}>${e(cfg.applyLabel || "Aplicar")}</button>${mode === "search" ? '<button type="button" class="btn ghost" data-act="clear">Limpar expressão</button>' : ""}<span class="rxw-grow"></span><span class="rxw-note" data-slot="apply-note">${e(cfg.applyNote || "Editar e testar usam só a amostra. A pesquisa no conjunto começa quando você aplica.")}</span></div>`;
    host.append(root);
    const q = (s) => root.querySelector(s), ctl = (name) => root.querySelector(`[data-f="${name}"]`);
    const editor = window.RegexEditor.create(q('[data-slot="editor"]'), { value: draft.pattern, label: "Expressão", placeholder: cfg.placeholder || "Digite ou cole uma expressão. Exemplo: (?<=user=)\\w+", onSubmit: () => apply() });
    const sample = createSample(q('[data-slot="sample"]'));
    let version = 0, previewSeq = 0, result = null, resultDraft = -1, resultText = null, draftVersion = 0, occ = 0, busy = false, timer = 0, destroyed = false, applying = false;
    const key = `rx-preview-${id}`;
    ctl("engine").value = draft.engine;
    if (ctl("field")) ctl("field").value = draft.field;
    ctl("caseSensitive").checked = draft.caseSensitive; ctl("multiline").checked = draft.multiline; ctl("dotAll").checked = draft.dotAll; ctl("scope").value = draft.scope;
    sample.set(cfg.sample?.text || "");
    const noteSample = (extra = "") => { q('[data-slot="sample-note"]').textContent = [cfg.sample?.label, extra].filter(Boolean).join(" · "); };
    noteSample(cfg.sample?.truncated ? "Amostra recortada: o valor completo do campo é maior." : "");
    const say = (message) => { q('[data-slot="status"]').innerHTML = `<span class="rxw-pill" data-state="stale">${e(message)}</span>`; };

    const snapshot = () => ({ ...draft });
    const dirty = () => { version++; draftVersion++; };
    const options = () => ({ engine: draft.engine, text: draft.pattern, caseSensitive: draft.caseSensitive, multiline: draft.multiline, dotAll: draft.dotAll });

    // ---- native preview (debounced, versioned, replaced by newer requests)
    function schedule(delay = 320) {
      clearTimeout(timer);
      window.LocalNative?.cancel(key);
      previewSeq++;
      renderStatus();
      if (!draft.pattern) { result = null; resultDraft = -1; renderResult(); return; }
      timer = setTimeout(() => void runPreview(), delay);
    }
    async function runPreview() {
      if (destroyed || !draft.pattern) return null;
      const seq = ++previewSeq, forDraft = draftVersion, text = sample.text;
      busy = true; renderStatus();
      try {
        if (new TextEncoder().encode(text).length > SAMPLE_BYTES) throw Error("A amostra excede 1 MiB. Use um trecho menor para testar.");
        const response = await window.LocalNative.call({ action: "regex_preview", pattern: options(), text, scope: draft.scope }, { latest: key });
        if (destroyed || seq !== previewSeq) return null;
        result = response; resultDraft = forDraft; resultText = text;
      } catch (error) {
        if (destroyed || seq !== previewSeq || window.LocalNative.superseded(error)) return null;
        result = { ok: false, stage: "transport", error: { message: String(error.message || error) } }; resultDraft = forDraft; resultText = text;
      } finally { if (seq === previewSeq) busy = false; }
      occ = 0;
      renderStatus(); renderResult();
      void loadSeverity(text);
      return result;
    }
    const fresh = () => result && resultDraft === draftVersion;
    let severity = [], severityText = null;
    async function loadSeverity(text) {
      const rules = (window.LocalExplorer?.rules() || []).filter((r) => r.enabled && r.pattern?.text);
      if (!rules.length || !text) { if (severity.length) { severity = []; severityText = text; renderResult(); } return; }
      try {
        const marks = await window.LocalNative.call({ action: "highlights", rules, texts: [text] }, { latest: `${key}-sev` });
        if (destroyed || sample.text !== text) return;
        severity = marks[0] || []; severityText = text; renderResult();
      } catch { /* colours are a convenience; the test result is already shown */ }
    }

    function renderStatus() {
      const node = q('[data-slot="status"]');
      let html = "";
      if (!draft.pattern) html = '<span class="rxw-pill" data-state="stale">Expressão vazia</span>';
      else if (busy) html = '<span class="rxw-pill" data-state="busy">Validando no motor nativo…</span>';
      else if (!result) html = '<span class="rxw-pill" data-state="busy">Aguardando você terminar de digitar…</span>';
      else {
        const stale = !fresh();
        if (result.ok) html = `<span class="rxw-pill" data-state="${stale ? "stale" : "ok"}">${stale ? "Prévia da versão anterior" : `Válida · ${e(result.engine === "pcre2" ? `PCRE2 ${result.pcre2Version}` : result.engine === "rust" ? "Rust regex" : "texto literal")}`}</span>${stale ? "" : `<span>${result.spans.length ? `${result.spans.length}${result.complete ? "" : "+"} ocorrência${result.spans.length > 1 ? "s" : ""} na amostra` : "Nenhuma ocorrência nesta amostra"}</span>`}${!result.complete && !stale ? `<span class="rxw-pill" data-state="error">Resultado incompleto: limite de ${result.limit === "groups" ? "grupos" : result.limit === "values" ? "valores" : "ocorrências"}</span>` : ""}`;
        else {
          const pos = result.error?.offset != null ? ` (posição ${result.error.offset + 1})` : "";
          html = `<span class="rxw-pill" data-state="${stale ? "stale" : "error"}">${stale ? "Erro da versão anterior" : result.stage === "run" ? "Limite do motor — não significa zero ocorrências" : result.stage === "compile" ? `Expressão inválida${pos}` : "Teste indisponível"}</span><span>${e(result.error?.message || "")}</span>`;
        }
      }
      node.innerHTML = html;
      const marked = result && !result.ok && result.stage === "compile" && result.error?.offset != null;
      editor.markNative(marked ? { start: result.error.offset, end: result.error.offset + 1, message: result.error.message } : null);
      if (marked && !fresh()) editor.markNative({ start: Math.min(result.error.offset, editor.value.length), end: result.error.offset + 1, message: "" });
      q('[data-act="apply"]').disabled = applying || !draft.pattern;
    }

    // ---- sample view
    const slotFor = (index) => {
      const a = editor.analysis, list = a.captures.filter((g) => g.index === index);
      if (list.length !== 1 || list[0].uncertain || !fresh() || a.total !== (result?.groups?.length ? result.groups.length - 1 : -1)) return null;
      return editor.slotOf(list[0].id);
    };
    function renderResult() {
      const text = sample.text, shows = result?.ok && resultText === text;
      const sev = severityText === text ? severity : [], cuts = new Set([0, text.length]), marks = [];
      // Existing highlight rules (severity, colours) are a separate layer under matches and captures.
      for (const m of sev) { marks.push({ start: m.start, end: m.end, cls: "rxs-sev", color: m.color, title: m.label }); cuts.add(m.start); cuts.add(m.end); }
      if (!shows && !sev.length) { sample.paint(sample.plainHtml() + (text.endsWith("\n") ? " " : "")); renderOcc(); renderCaptures(); updateObserved(); return; }
      const spans = shows ? result.spans : [];
      spans.forEach((s, i) => {
        marks.push({ start: s.start, end: s.end, cls: i === occ ? "rxs-match rxs-current" : "rxs-match" });
        cuts.add(s.start); cuts.add(s.end);
        for (const g of s.groups || []) if (g.participated && g.end > g.start) { const slot = slotFor(g.index); marks.push({ start: g.start, end: g.end, size: g.end - g.start, cls: `rxs-group ${slot == null ? "rxs-neutral" : `rx-c-${slot}`}${i === occ ? "" : " rxs-dim"}`, title: `Grupo ${g.index}${g.names?.length ? ` ${g.names[0]}` : ""}` }); cuts.add(g.start); cuts.add(g.end); }
      });
      const points = [...cuts].filter((p) => p >= 0 && p <= text.length).sort((a, b) => a - b), empties = new Set(spans.filter((s) => s.start === s.end).map((s) => s.start));
      let html = "";
      for (let i = 0; i < points.length; i++) {
        const a = points[i], b = points[i + 1];
        if (empties.has(a)) html += '<span class="rxs-empty" title="Ocorrência vazia"></span>';
        if (b == null || b <= a) continue;
        const covering = marks.filter((m) => m.start <= a && m.end >= b);
        const group = covering.filter((m) => m.size).sort((x, y) => x.size - y.size)[0], sevMark = covering.find((m) => m.color);
        const cls = covering.filter((m) => !m.size).map((m) => m.cls).join(" ") + (group ? ` ${group.cls}` : "");
        const style = sevMark && /^#[0-9a-f]{6}$/i.test(sevMark.color) ? ` style="--sev:${sevMark.color}"` : "";
        html += cls ? `<span class="${e(cls)}"${style}${group?.title || sevMark?.title ? ` title="${e(group?.title || sevMark.title)}"` : ""}>${e(text.slice(a, b))}</span>` : e(text.slice(a, b));
      }
      sample.paint(html + (text.endsWith("\n") ? " " : ""));
      renderOcc(); renderCaptures(); updateObserved();
    }
    function renderOcc() {
      const node = q('[data-slot="occ"]'), spans = result?.ok && resultText === sample.text ? result.spans : [];
      if (!spans.length) { node.innerHTML = result?.ok && resultText === sample.text ? "<span>Nenhuma ocorrência nesta amostra. Isso não diz nada sobre os eventos do conjunto.</span>" : ""; return; }
      node.innerHTML = `<button type="button" class="btn ghost small" data-occ="-1" aria-label="Ocorrência anterior">◀</button><span>Ocorrência ${occ + 1} de ${spans.length}${result.complete ? "" : "+"}${fresh() ? "" : " · prévia desatualizada"}</span><button type="button" class="btn ghost small" data-occ="1" aria-label="Próxima ocorrência">▶</button>`;
      node.querySelectorAll("[data-occ]").forEach((b) => (b.onclick = () => { occ = (occ + +b.dataset.occ + spans.length) % spans.length; renderResult(); }));
    }
    const shown = (v) => (v === "" ? '<span class="rxw-none">(vazio)</span>' : `<code>${e(v)}</code>`);
    function renderCaptures() {
      const node = q('[data-slot="captures"]'), ok = result?.ok && resultText === sample.text, span = ok ? result.spans[occ] : null;
      if (!span) { node.innerHTML = ""; return; }
      const infos = result.groups || [];
      const rows = infos.slice(1).map((info) => {
        const g = (span.groups || []).find((x) => x.index === info.index), slot = slotFor(info.index);
        const label = `<span class="rx-chip ${slot == null ? "rx-chip-neutral" : `rx-c-${slot}`}" style="cursor:default"><b>${info.index}</b>${info.names?.length ? `<span>${e(info.names.join(", "))}</span>` : ""}</span>`;
        const state = !g || !g.participated ? '<span class="rxw-none">não participou</span>' : `${shown(g.value ?? "")}${g.valueTruncated ? " …" : ""} <small style="color:var(--text-2)">[${g.start}–${g.end}]</small>`;
        return `<tr><td>${label}</td><td>${state}</td></tr>`;
      }).join("");
      node.innerHTML = `<table class="rxw-table"><thead><tr><th>Grupo</th><th>Valor nesta ocorrência</th></tr></thead><tbody><tr><td>Correspondência completa</td><td>${shown(sample.text.slice(span.start, span.end))} <small style="color:var(--text-2)">[${span.start}–${span.end}]${span.start === span.end ? " · ocorrência vazia" : ""}</small></td></tr>${rows}</tbody></table><p class="rxw-note">Valores e posições vêm do motor. Em repetições, o PCRE2 informa só a última iteração de cada grupo.${editor.analysis.references.some((r) => r.type === "backref") ? " A posição consumida por uma backreference não é informada pelo motor." : ""}</p>`;
    }
    function updateObserved() {
      const map = new Map(), span = result?.ok && resultText === sample.text && fresh() ? result.spans[occ] : null;
      for (const g of span?.groups || []) if (g.participated) { map.set(g.index, g.value ?? ""); for (const n of g.names || []) map.set(n, g.value ?? ""); }
      observed = map; editor.setObserved(map);
    }
    let observed = new Map();

    // ---- wiring
    function changed() {
      dirty();
      draft.pattern = editor.value;
      cfg.onChange?.(snapshot());
      schedule();
    }
    editor.on("change", () => changed());
    sample.on("change", () => { version++; schedule(200); });
    const syncDraft = () => {
      for (const name of ["field", "engine", "scope"]) if (ctl(name)) draft[name] = ctl(name).value;
      for (const name of ["caseSensitive", "multiline", "dotAll"]) draft[name] = ctl(name).checked;
      dirty(); cfg.onChange?.(snapshot()); schedule();
    };
    root.querySelectorAll("[data-f]").forEach((n) => n.addEventListener("change", syncDraft));
    q('[data-act="clear"]')?.addEventListener("click", () => { editor.replace(0, editor.value.length, ""); editor.focus(); });
    q('[data-act="use-event"]')?.addEventListener("click", () => void loadEventSample());
    const wb = {
      id, editor, sample, version: () => version, getDraft: snapshot, say,
      getSampleText: () => sample.text, observedValues: () => observed,
      setEngine(engine) { ctl("engine").value = engine; draft.engine = engine; dirty(); schedule(); cfg.onChange?.(snapshot()); },
      canSaveCapture: !!cfg.onSaveCapture,
      saveCapture(group) { return cfg.onSaveCapture?.(snapshot(), group.name || String(group.index)); },
    };
    const assist = window.RegexAssist.attach(wb);
    const assistButton = q('[data-act="assist"]');
    assistButton.addEventListener("click", (ev) => assist.openMenu(assistButton, ev));
    for (const el of [editor.input, sample.input]) el.addEventListener("contextmenu", (ev) => { ev.preventDefault(); assist.openMenu(assistButton, ev); });
    for (const el of [editor.input, sample.input]) el.addEventListener("keydown", (ev) => { if (ev.key === "ContextMenu" || (ev.shiftKey && ev.key === "F10")) { ev.preventDefault(); assist.openMenu(assistButton, ev); } });
    editor.input.addEventListener("keydown", (ev) => { if ((ev.ctrlKey || ev.metaKey) && ev.key === " ") { ev.preventDefault(); assist.openMenu(assistButton, ev); } });

    async function loadEventSample() {
      const target = cfg.sampleProvider?.();
      if (!target) { window.toast?.("Abra um evento em Detalhes para usar o seu texto.", "info"); return; }
      const field = draft.field === "_all" ? "raw" : draft.field;
      try {
        const got = await window.LocalNative.call({ action: "sample_text", id: target.id, event_ref: target.event_ref, field }, { latest: `rx-sample-${id}` });
        sample.set(got.text);
        cfg.sample = { ...(cfg.sample || {}), label: `Evento #${target.id} · ${field}`, truncated: got.truncated };
        noteSample(got.truncated ? `Amostra recortada: ${got.bytes.toLocaleString("pt-BR")} bytes no campo, ${new TextEncoder().encode(got.text).length.toLocaleString("pt-BR")} usados.` : "");
        root.querySelector(".rxw-sample").open = true;
        version++; schedule(0);
      } catch (error) { if (!window.LocalNative.superseded(error)) window.toast?.(String(error.message || error), "err"); }
    }
    async function apply() {
      if (applying || !draft.pattern) return;
      applying = true; renderStatus();
      try {
        // A draft that was never validated, or changed since, is checked first.
        if (!fresh() || !result?.ok) { clearTimeout(timer); await runPreview(); }
        if (!fresh()) return;
        if (!result?.ok) { editor.focus(); say(result?.stage === "run" ? "O motor atingiu um limite com esta expressão. Ela não foi aplicada." : "A expressão não é válida. Corrija o erro indicado antes de aplicar."); return; }
        await cfg.onApply?.(snapshot(), result);
      } catch (error) { window.toast?.(String(error.message || error), "err"); }
      finally { applying = false; renderStatus(); }
    }
    q('[data-act="apply"]').addEventListener("click", () => void apply());
    renderStatus(); renderResult();
    if (draft.pattern) schedule(0);

    return {
      root, id, editor, sample, assist, getDraft: snapshot, apply, runPreview,
      setDraft(next = {}) {
        Object.assign(draft, next);
        for (const name of ["field", "engine", "scope"]) if (ctl(name) && next[name] != null) ctl(name).value = draft[name];
        for (const name of ["caseSensitive", "multiline", "dotAll"]) if (next[name] != null) ctl(name).checked = !!draft[name];
        if (next.pattern != null) editor.setValue(next.pattern);
        dirty(); schedule(0);
      },
      setFields(list) {
        const select = ctl("field"); if (!select) return;
        const keep = select.value;
        select.innerHTML = list.map(([v, l]) => `<option value="${e(v)}">${e(l)}</option>`).join("");
        select.value = list.some(([v]) => v === keep) ? keep : list[0]?.[0];
        draft.field = select.value;
      },
      setSample(text, meta = {}) { sample.set(text); cfg.sample = { ...meta }; noteSample(meta.truncated ? "Amostra recortada: o valor completo do campo é maior." : ""); version++; schedule(0); root.querySelector(".rxw-sample").open = true; },
      /** Selection taken from text outside the workbench (a message, a cell): treated as literal. */
      useSelection(text, anchor, kind = "find") { const sel = { source: "sample", text, start: 0, end: text.length, external: true }; if (kind === "capture") { sel.external = true; assist.capture(sel, anchor || assistButton); } else assist.findText(sel, anchor || assistButton); },
      focus: () => editor.focus(),
      destroy() { destroyed = true; clearTimeout(timer); window.LocalNative?.cancel(key); assist.close(); editor.destroy(); root.remove(); },
    };
  }
  return { create, ENGINES };
})();
