/* Bounded native Java structure in the existing event drawer. */
window.JavaTrace = (() => {
  "use strict";
  const NODE_STEP = 8, FRAME_STEP = 20;
  const SCALARS = [
    ["java.exception.class", "Classe"], ["java.exception.message", "Mensagem"],
    ["java.root_cause.class", "Causa final observada"], ["java.trace.fingerprint", "Padrão da stack trace"],
  ];
  const DIAGNOSTICS = {
    input_limit: "Limite de texto inspecionado", node_limit: "Limite de exceções", frame_limit: "Limite de frames",
    depth_limit: "Limite de profundidade", string_limit: "Valor individual limitado", payload_limit: "Limite da estrutura",
    unknown_line: "Linha não interpretada", unknown_frame: "Frame não interpretado", orphan_frame: "Frame sem exceção identificada",
    invalid_indent: "Indentação ambígua", invalid_elision: "Contagem de frames compartilhados inválida",
    circular_reference: "Referência circular", multiple_roots: "Mais de uma exceção inicial", missing_root: "Exceção inicial ausente",
    unrecognized: "Estrutura não reconhecida",
  };
  const element = (tag, className, text) => {
    const node = document.createElement(tag); node.className = className || "";
    if (text != null) node.textContent = text;
    return node;
  };
  const display = value => {
    // A user-derived scalar can be larger than the native parser strings. Do
    // not redact/render its entire body just to make a small menu label.
    if (typeof value === "string" && value.length > 4096) return "[valor extenso · abrir pelo menu]";
    return String(window.EvidenceUI?.redact(value) ?? value ?? "");
  };
  const short = value => { const text = display(value); return text.length > 120 ? `${text.slice(0, 119)}…` : text; };
  const integer = value => Number.isSafeInteger(value) && value >= 0;
  const boundedString = value => value == null || typeof value === "string" && value.length <= 4096;
  // Coordinates are absolute in the decoded block, which may have a prefix
  // before the parser's bounded input window. They are metadata, never slices.
  const span = value => value && integer(value.start) && integer(value.end) && value.start <= value.end;
  function valid(trace) {
    if (!(trace && trace.schemaVersion === 1 && trace.spanBasis === "utf8_bytes_in_raw_block"
      && typeof trace.complete === "boolean" && Array.isArray(trace.nodes) && trace.nodes.length <= 64
      && Array.isArray(trace.frames) && trace.frames.length <= 1024 && Array.isArray(trace.diagnostics) && trace.diagnostics.length <= 16
      && (trace.root === null || integer(trace.root) && trace.root < trace.nodes.length)
      && trace.fingerprintVersion === 1 && (trace.fingerprint == null || typeof trace.fingerprint === "string" && trace.fingerprint.length <= 128)
      && trace.diagnostics.every(item => item && typeof item.code === "string" && item.code.length <= 64 && (item.span == null || span(item.span)))
      && trace.nodes.every(node => node && ["root", "cause", "suppressed"].includes(node.relation)
        && typeof node.class === "string" && node.class.length <= 4096 && integer(node.frameStart) && integer(node.frameCount)
        && node.frameStart + node.frameCount <= trace.frames.length && integer(node.depth) && node.depth <= 32 && boundedString(node.message)
        && span(node.header) && (node.parent === null || integer(node.parent) && node.parent < trace.nodes.length)
        && (node.elidedFrames == null || integer(node.elidedFrames) && node.elidedFrames <= 4294967295))
      && trace.frames.every(frame => frame && span(frame.span) && (frame.node == null || integer(frame.node) && frame.node < trace.nodes.length)
        && (frame.parts == null || typeof frame.parts.class === "string" && typeof frame.parts.method === "string"
          && [frame.parts.class, frame.parts.method, frame.parts.loader, frame.parts.module, frame.parts.moduleVersion].every(boundedString)
          && ["native", "unknown", "file"].includes(frame.parts.location?.kind)
          && (frame.parts.location.kind !== "file" || typeof frame.parts.location.name === "string" && boundedString(frame.parts.location.name)
            && (frame.parts.location.line == null || integer(frame.parts.location.line) && frame.parts.location.line <= 4294967295)))))) return false;
    // Saved/imported rows may predate the native guard. Bound their aggregate
    // string content too, without serializing an object just to measure it.
    let characters = 0;
    for (const node of trace.nodes) characters += node.class.length + (node.message?.length || 0);
    for (const frame of trace.frames) if (frame.parts) {
      for (const value of [frame.parts.class, frame.parts.method, frame.parts.loader, frame.parts.module, frame.parts.moduleVersion, frame.parts.location.name]) characters += value?.length || 0;
    }
    return characters <= 256 * 1024;
  }
  function frameText(parts) {
    if (!parts || typeof parts.class !== "string" || typeof parts.method !== "string") return null;
    const module = parts.module ? `${parts.module}${parts.moduleVersion ? `@${parts.moduleVersion}` : ""}` : "";
    const prefix = parts.loader ? `${parts.loader}/${module}/` : module ? `${module}/` : "";
    const where = parts.location?.kind === "native" ? "Native Method" : parts.location?.kind === "unknown" ? "Unknown Source"
      : parts.location?.kind === "file" ? `${parts.location.name || ""}${parts.location.line == null ? "" : `:${parts.location.line}`}` : "?";
    return `${prefix}${parts.class}.${parts.method}(${where})`;
  }

  function render(event, options = {}) {
    if (!event?.fields) return null;
    const marked = Object.hasOwn(event.fields, "java.trace.complete") && typeof event.fields["java.trace.complete"] === "boolean";
    const legacy = typeof event.fields.exception === "string" && event.fields.exception.length > 0
      || Array.isArray(event.fields.stacktrace) && event.fields.stacktrace.length > 0;
    if (!marked && !legacy) return null;
    let trace = null;
    const current = () => options.isCurrent?.() !== false;
    const hasRaw = typeof event.raw === "string" && event.raw.length > 0;
    const raw = () => { if (current() && hasRaw) options.raw?.(event); };
    const button = (label, action, className = "") => {
      const node = element("button", `btn ghost small ${className}`.trim(), label); node.type = "button";
      node.onclick = event => { event?.stopPropagation(); if (current()) return action(); };
      return node;
    };
    const sourceButton = () => {
      const node = button(hasRaw ? "Ver texto decodificado" : "Texto decodificado indisponível", raw, "java-trace-source");
      node.disabled = !hasRaw; return node;
    };
    function originalButton(container) {
      if (!Object.hasOwn(event.fields, "stacktrace") || !options.original) return;
      const original = button("Ver stacktrace original", () => options.original("stacktrace", original), "java-trace-original");
      container.append(original);
    }
    async function copy(text) {
      if (!current()) return;
      try {
        await (options.copy ? options.copy(text) : navigator.clipboard.writeText(text));
        if (current()) options.copied?.();
      } catch { if (current()) options.copyFailed?.(); }
    }
    function virtualMenu(target, value, label) {
      target.oncontextmenu = domEvent => {
        domEvent.preventDefault(); domEvent.stopPropagation(); if (!current()) return;
        const items = [];
        if (value != null) items.push({ icon: "fa-copy", label, onClick: () => copy(value) });
        if (hasRaw) items.push({ icon: "fa-code", label: "Ver texto decodificado", onClick: raw });
        options.menu?.(domEvent, items);
      };
    }
    const block = element("details", "java-trace");
    const summary = element("summary", "java-trace-summary"), summaryStatus = element("span", "java-trace-status", "Expandir estrutura observada");
    const exceptionClass = event.fields["java.exception.class"];
    const title = element("strong", "", marked ? `Exceção Java${typeof exceptionClass === "string" ? ` · ${short(exceptionClass)}` : ""}` : "Interpretar stack trace");
    summary.append(title, summaryStatus);
    block.append(summary);
    let built = false, pending = null, serial = 0, notice = null;
    function state(message, retry = false) {
      notice?.remove(); notice = element("div", "java-trace-load-state");
      notice.append(element("p", "java-trace-note", message));
      if (retry) notice.append(button("Tentar novamente", load, "java-trace-retry"));
      notice.append(sourceButton()); originalButton(notice); block.append(notice);
    }
    function cancel(expected = serial) {
      if (!pending || expected !== serial) return;
      serial++; pending = null;
      if (current()) {
        options.cancel?.();
        state("Consulta cancelada. Expanda novamente ou tente de novo.", true);
      }
    }
    async function load() {
      if (!current() || pending || built) return;
      if (trace) { build(); return; }
      if (!options.load) { state("Estrutura indisponível para a evidência preservada. Consulte o texto decodificado."); return; }
      state("Lendo a estrutura deste registro…");
      const mine = ++serial;
      notice.append(button("Cancelar", () => cancel(mine), "java-trace-cancel"));
      const request = Promise.resolve().then(() => { if (mine !== serial || !current()) return null; return options.load(); });
      pending = request;
      try {
        const response = await request;
        if (mine !== serial || !current()) return;
        pending = null;
        if (response?.state === "unavailable") {
          const messages = { raw_unavailable: "Texto decodificado indisponível para estruturar este registro.",
            not_java: "Nenhuma estrutura Java foi reconhecida neste registro.", record_unavailable: "Este registro não está disponível no recorte atual." };
          state(messages[response.reason] || "Estrutura Java indisponível para este registro."); return;
        }
        if (response?.state !== "available" || !valid(response.trace)) { state("A estrutura recebida não é compatível. Consulte o texto decodificado."); return; }
        trace = response.trace;
        if (block.open) build();
      } catch (error) {
        if (mine !== serial || !current()) return;
        pending = null; state(`Não foi possível ler a estrutura: ${String(error)}`, true);
      }
    }
    block.ontoggle = () => {
      if (!block.open) { cancel(); return; }
      if (!current()) { block.open = false; return; }
      return load();
    };
    function build() {
      if (built || !current()) return;
      notice?.remove(); notice = null;
      if (!marked) title.textContent = "Exceção Java · estrutura observada";
      summaryStatus.textContent = `${trace.frames.length} frames na estrutura disponível${trace.complete ? "" : " · interpretação incompleta"}`;
      built = true;
      const content = element("div", "java-trace-content");
      content.append(element("p", "java-trace-note", trace.complete
        ? "Estrutura do trecho observado. O registro pode conter frames omitidos pelo produtor."
        : "Parte da estrutura não foi interpretada. Consulte o texto decodificado para ver o registro disponível."));
      if (!trace.complete) {
        const diagnostics = element("ul", "java-trace-diagnostics");
        for (const code of new Set(trace.diagnostics.map(item => item.code))) diagnostics.append(element("li", "", Object.hasOwn(DIAGNOSTICS, code) ? DIAGNOSTICS[code] : "Limitação da interpretação"));
        content.append(diagnostics);
      }
      const scalars = element("div", "java-trace-scalars");
      for (const [column, label] of SCALARS) {
        if (!Object.hasOwn(event.fields, column) || typeof event.fields[column] !== "string") continue;
        if (!trace.complete && ["java.root_cause.class", "java.trace.fingerprint"].includes(column)) continue;
        const line = element("div", "java-trace-scalar"), value = element("button", "java-trace-scalar-value", short(event.fields[column]));
        value.type = "button"; value.title = display(event.fields[column]); value.dataset.column = column;
        value.setAttribute("aria-label", `${label}: ${display(event.fields[column])}`);
        // This is the actual literal Event.fields key, never a virtual node path.
        value.oncontextmenu = domEvent => { domEvent.preventDefault(); domEvent.stopPropagation(); if (current()) options.scalarMenu?.(domEvent, column); };
        value.onclick = domEvent => { if (current()) options.scalarMenu?.(domEvent, column); };
        line.append(element("span", "java-trace-scalar-label", label), value); scalars.append(line);
      }
      content.append(scalars);
      if (trace.complete && trace.fingerprint) content.append(element("p", "java-trace-note", `Padrão v${trace.fingerprintVersion}: agrupa classes e frames observados; não identifica a evidência nem comprova uma causa comum.`));
      const nodes = element("div", "java-trace-nodes"), count = element("p", "java-trace-count");
      let shown = 0;
      const more = button("Mostrar mais exceções", appendNodes, "java-trace-more-nodes");
      function appendNodes() {
        if (!current()) return;
        const end = Math.min(shown + NODE_STEP, trace.nodes.length);
        for (; shown < end; shown++) nodes.append(renderNode(trace.nodes[shown], shown));
        count.textContent = `${shown} de ${trace.nodes.length} exceções`;
        more.hidden = shown >= trace.nodes.length;
      }
      function renderNode(node, index) {
        const item = element("details", "java-trace-node"); item.dataset.node = String(index);
        item.style.setProperty("--java-depth", Math.min(node.depth, 3));
        const head = element("summary", "java-trace-node-title"), relation = node.relation === "cause" ? "Causada por" : node.relation === "suppressed" ? "Suprimida" : "Exceção";
        head.textContent = `${relation}: ${short(node.class)} · ${node.frameCount} frames`;
        head.title = `${relation}: ${display(node.class)}${node.parent != null ? ` · vinculada à exceção ${node.parent + 1}` : ""}`;
        virtualMenu(head, node.class, "Copiar classe observada"); item.append(head);
        let framesBuilt = false;
        item.ontoggle = () => {
          if (!item.open) return;
          if (!current()) { item.open = false; return; }
          if (framesBuilt) return;
          framesBuilt = true;
          if (typeof node.message === "string" && node.message) {
            const message = element("p", "java-trace-message", display(node.message));
            virtualMenu(message, node.message, "Copiar mensagem observada"); item.append(message);
          }
          const frames = element("ol", "java-trace-frames"), frameCount = element("p", "java-trace-count");
          let shownFrames = 0;
          const moreFrames = button("Mostrar mais frames", appendFrames, "java-trace-more-frames");
          function appendFrames() {
            if (!current()) return;
            const end = Math.min(shownFrames + FRAME_STEP, node.frameCount);
            for (; shownFrames < end; shownFrames++) {
              const frame = trace.frames[node.frameStart + shownFrames], text = frameText(frame?.parts);
              const row = element("li", "java-trace-frame", text == null ? "Frame não interpretado · ver texto decodificado" : display(text));
              virtualMenu(row, text, "Copiar frame interpretado"); frames.append(row);
            }
            frameCount.textContent = `${shownFrames} de ${node.frameCount} frames`;
            moreFrames.hidden = shownFrames >= node.frameCount;
          }
          item.append(frames, frameCount, moreFrames);
          if (node.elidedFrames != null) item.append(element("p", "java-trace-note", `O texto informa ${node.elidedFrames} frames compartilhados omitidos.`));
          appendFrames();
        };
        return item;
      }
      content.append(nodes, count, more, sourceButton());
      originalButton(content);
      block.append(content); appendNodes();
    }
    return block;
  }
  return { render };
})();
