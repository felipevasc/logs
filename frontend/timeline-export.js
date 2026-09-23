/* Offline visual exports. PDF captures one bounded tile at a time, never a giant canvas. */
window.TimelineExport = (() => {
  "use strict";
  const MAX_PIXELS = 24_000_000, MAX_EDGE = 16_000, MAX_PAGES = 500, MAX_BYTES = 32 * 1024 * 1024;
  const PDF_WIDTH = 1060, PDF_BODY = 660, HEADER = 84, FOOTER = 26;
  let libraries, embeddedFonts, dialog = null;
  const abort = signal => { if (signal?.aborted) throw new DOMException("Exportação cancelada", "AbortError"); };
  const nextFrame = () => new Promise(resolve => requestAnimationFrame(() => setTimeout(resolve, 0)));
  const text = (tag, className, value) => { const node = document.createElement(tag); node.className = className; node.textContent = value; return node; };
  const loadScript = src => new Promise((resolve, reject) => {
    const script = document.createElement("script"); script.src = src; script.onload = resolve;
    script.onerror = () => { script.remove(); reject(new Error("Não foi possível carregar o exportador local. Tente novamente.")); }; document.head.append(script);
  });
  async function loadLibraries() {
    if (!libraries) libraries = Promise.all([
      window.htmlToImage ? null : loadScript("vendor/timeline-export/html-to-image.js"),
      window.jspdf ? null : loadScript("vendor/timeline-export/jspdf.umd.min.js"),
    ]).catch(error => { libraries = null; throw error; });
    await libraries;
    if (!embeddedFonts) embeddedFonts = Promise.all([
      ["fa-solid fa-circle", "fa-solid-900.woff2", 900],
      ["fa-regular fa-circle", "fa-regular-400.woff2", 400],
      ["fa-brands fa-aws", "fa-brands-400.woff2", 400],
    ].map(async ([className, filename, weight]) => {
      const probe = document.createElement("i"); probe.className = className; probe.hidden = true; document.body.append(probe);
      const family = getComputedStyle(probe).fontFamily; probe.remove();
      const response = await fetch(`vendor/fa/webfonts/${filename}`);
      if (!response.ok) throw new Error("Não foi possível carregar os ícones locais da timeline.");
      const blob = await response.blob();
      return new Promise((resolve, reject) => {
        const reader = new FileReader(); reader.onload = () => resolve(`@font-face{font-family:${family};font-style:normal;font-weight:${weight};font-display:block;src:url(${reader.result}) format("woff2")}`); reader.onerror = reject; reader.readAsDataURL(blob);
      });
    })).then(styles => styles.join("\n")).catch(error => { embeddedFonts = null; throw error; });
    return embeddedFonts;
  }
  function measure({ source, type }) {
    const board = type === "activity" ? source : source.querySelector(".ct-board");
    if (!board?.isConnected) throw new Error("A timeline mudou. Feche esta janela e abra a exportação novamente.");
    const width = Math.ceil(board.getBoundingClientRect().width);
    const height = Math.ceil(type === "activity" ? Math.max(board.scrollHeight, board.getBoundingClientRect().height) : board.offsetHeight);
    if (!(width > 0 && height > 0)) throw new Error("A timeline mudou. Feche esta janela e abra a exportação novamente.");
    const matrix = type === "matrix";
    const labelWidth = matrix ? parseFloat(board.style.getPropertyValue("--ct-label-width")) || 220 : 0;
    return { board, width, height, matrix, labelWidth };
  }
  function pngPlan(info) {
    const height = info.height + HEADER + FOOTER;
    const ratio = Math.min(2, MAX_EDGE / Math.max(info.width, height), Math.sqrt(MAX_PIXELS / (info.width * height)));
    return { width: info.width, height, ratio: Math.max(0, ratio), allowed: ratio >= 1 };
  }
  function planPages(info) {
    const { board, width, height, matrix, labelWidth } = info;
    const rowHeight = PDF_BODY - (matrix ? 48 : 0);
    const rect = board.getBoundingClientRect();
    const occupied = [...board.querySelectorAll(".ct-entry,.ct-note,.ct-axis-tick,.ct-day,.tl-summary,.tl-chart-layout,.tl-selection,.tl-signals,.tx-note-paragraph")].map(node => {
      const r = node.getBoundingClientRect(); return { top: r.top - rect.top - 4, bottom: r.bottom - rect.top + 4 };
    });
    // Merge occupied spans once. A page boundary must not cut through any label;
    // checking every label for every candidate made long timelines quadratic.
    const spans = [];
    for (const span of occupied.slice().sort((a, b) => a.top - b.top || a.bottom - b.bottom)) {
      const last = spans.at(-1);
      if (last && span.top < last.bottom) last.bottom = Math.max(last.bottom, span.bottom);
      else spans.push({ ...span });
    }
    const candidates = (matrix ? [...board.querySelectorAll(".ct-matrix-lane")].map(node => node.offsetTop + node.offsetHeight) : occupied.map(r => r.bottom + 4)).sort((a, b) => a - b);
    let spanIndex = 0;
    const preferred = candidates.filter(value => {
      while (spanIndex < spans.length && spans[spanIndex].bottom <= value) spanIndex++;
      const span = spans[spanIndex]; return !span || value <= span.top;
    });
    const lastAt = target => {
      let lo = 0, hi = preferred.length;
      while (lo < hi) { const mid = (lo + hi) >>> 1; if (preferred[mid] <= target) lo = mid + 1; else hi = mid; }
      return preferred[lo - 1];
    };
    const rows = [];
    for (let y = matrix ? 48 : 0; y < height;) {
      const target = Math.min(height, y + rowHeight);
      let end = target;
      if (target < height) {
        const candidate = lastAt(target);
        if (candidate > y + rowHeight * .4) end = candidate;
      }
      rows.push({ y, height: end - y + (matrix ? 48 : 0) }); y = end;
      if (rows.length > MAX_PAGES) break;
    }
    const tileWidth = Math.min(PDF_WIDTH, width), step = Math.max(1, tileWidth - labelWidth);
    const columns = matrix ? Math.max(1, Math.ceil((width - labelWidth) / step)) : Math.max(1, Math.ceil(width / tileWidth));
    const pages = [];
    for (let row = 0; row < rows.length; row++) for (let column = 0; column < columns; column++) {
      pages.push({ x: column * (matrix ? step : tileWidth), y: rows[row].y, width: tileWidth, height: rows[row].height, row: row + 1, column: column + 1, rows: rows.length, columns });
      if (pages.length > MAX_PAGES) return pages;
    }
    return pages;
  }
  function snapshot(info) {
    const template = info.board.cloneNode(true), original = info.board.getBoundingClientRect();
    // html-to-image preserves SVG subtrees as markup, so resolve their stylesheet rules here.
    const sourceSvg = [...info.board.querySelectorAll("svg *")], targetSvg = [...template.querySelectorAll("svg *")];
    const svgStyles = ["fill", "fill-opacity", "stroke", "stroke-width", "stroke-opacity", "stroke-dasharray", "stroke-linecap", "stroke-linejoin", "opacity", "vector-effect", "paint-order", "font-family", "font-size", "font-weight", "text-anchor"];
    sourceSvg.forEach((node, index) => { const style = getComputedStyle(node); for (const property of svgStyles) targetSvg[index].style?.setProperty(property, style.getPropertyValue(property)); });
    // Cache bounds once; discard off-page elements before expensive computed-style cloning.
    const selector = ".ct-entry,.ct-note,.ct-axis-tick,.ct-day,.ct-range-bar,.ct-matrix-lane,.ct-matrix-gridline,.ct-links path,.ct-links polygon";
    const sourceNodes = [...info.board.querySelectorAll(selector)], copyNodes = [...template.querySelectorAll(selector)], bounds = [];
    sourceNodes.forEach((node, index) => {
      let rect = node.getBoundingClientRect();
      const dots = node.matches(".ct-entry") ? [...node.querySelectorAll(".ct-matrix-dot")].map(dot => dot.getBoundingClientRect()) : [];
      if (dots.length) rect = { left: Math.min(rect.left, ...dots.map(r => r.left)), right: Math.max(rect.right, ...dots.map(r => r.right)), top: Math.min(rect.top, ...dots.map(r => r.top)), bottom: Math.max(rect.bottom, ...dots.map(r => r.bottom)) };
      bounds.push({ left: rect.left - original.left, right: rect.right - original.left, top: rect.top - original.top, bottom: rect.bottom - original.top, lane: node.matches(".ct-matrix-lane") });
      copyNodes[index].dataset.txIndex = String(index);
    });
    template.querySelectorAll(".ct-controls,.ct-editor-backdrop,.ct-open,.tx-trigger,.tl-mode-switch,.tl-controls,.tl-hover,.tl-drag-band,.tl-selection-actions").forEach(node => node.remove());
    // Keep the exported note text complete even when the live label is truncated.
    const notes = [...info.board.querySelectorAll(".ct-note span")].filter(node => node.scrollHeight > node.clientHeight + 1).map(node => node.textContent);
    const ticks = [...info.board.querySelectorAll(".ct-matrix-time")].map(node => { const r = node.getBoundingClientRect(); return { left: r.left - original.left, right: r.right - original.left }; });
    return { template, bounds, notes, ticks };
  }
  function pageNode(spec, info, snapshot, tile, pageIndex, pageCount) {
    const sheet = document.createElement("section"); sheet.className = `tx-sheet ct-shell ${info.matrix ? "ct-horizontal ct-matrix" : spec.type === "vertical" ? "ct-vertical" : "tx-activity"}`;
    sheet.style.width = `${tile.width}px`;
    const heading = text("header", "tx-sheet-heading", "");
    heading.append(text("strong", "", spec.title || "Linha do tempo"), text("p", "", spec.subtitle || "")); sheet.append(heading);
    const viewport = text("div", "tx-sheet-viewport", ""); viewport.style.width = `${tile.width}px`; viewport.style.height = `${tile.height}px`;
    const clone = snapshot.template.cloneNode(true), offsetY = tile.y - (info.matrix ? 48 : 0);
    clone.style.cssText += `;position:absolute!important;left:${-tile.x}px!important;top:${-offsetY}px!important;width:${info.width}px!important;height:${info.height}px!important;max-height:none!important;overflow:visible!important;margin:0!important;--tx-left:${tile.x}px;`;
    if (info.matrix) clone.querySelector(".ct-matrix-ruler").style.top = `${offsetY}px`;
    if (info.matrix && tile.columns > 1) clone.querySelectorAll(".ct-matrix-time").forEach((tick, index) => { const bounds = snapshot.ticks[index]; if (bounds.left < tile.x + info.labelWidth + 5 || bounds.right > tile.x + tile.width - 5) tick.remove(); });
    const left = tile.x, right = left + tile.width, top = tile.y, bottom = top + tile.height - (info.matrix ? 48 : 0);
    clone.querySelectorAll("[data-tx-index]").forEach(node => {
      const r = snapshot.bounds[Number(node.dataset.txIndex)];
      if (r.bottom < top - 16 || r.top > bottom + 16 || !r.lane && (r.right < left - 16 || r.left > right + 16)) node.remove();
      else node.removeAttribute("data-tx-index");
    });
    viewport.append(clone); sheet.append(viewport);
    const footer = text("footer", "tx-sheet-footer", "");
    footer.append(text("span", "", `LogInsight · ${Intl.DateTimeFormat().resolvedOptions().timeZone || "horário local"}`), text("span", "", pageCount > 1 ? `Página ${pageIndex + 1}/${pageCount}${tile.columns > 1 ? ` · faixa ${tile.column}/${tile.columns} · trecho ${tile.row}/${tile.rows}` : ""}` : "")); sheet.append(footer);
    return sheet;
  }
  async function render(spec, { signal, progress = () => {} } = {}) {
    abort(signal); const info = measure(spec), png = pngPlan(info), pages = spec.format === "pdf" ? planPages(info) : [{ x: 0, y: info.matrix ? 48 : 0, width: info.width, height: info.height }];
    const firstChild = info.board.firstElementChild;
    if (spec.format === "png" && !png.allowed) throw new Error("A timeline é grande demais para uma única imagem sem perder legibilidade. Exporte em PDF, que divide o conteúdo em páginas.");
    if (pages.length > MAX_PAGES) throw new Error(`Esta visão excede ${MAX_PAGES} páginas. Reduza a escala horizontal ou refine o recorte e exporte novamente.`);
    progress(0, pages.length, "Preparando fontes e desenho…");
    const fontEmbedCSS = await loadLibraries(); abort(signal); await document.fonts.ready; abort(signal);
    const current = measure(spec);
    if (current.board !== info.board || current.board.firstElementChild !== firstChild || current.width !== info.width || current.height !== info.height) throw new Error("A timeline mudou durante a preparação. Abra a exportação novamente.");
    const frozen = snapshot(info), stage = text("div", "tx-stage", ""); stage.setAttribute("aria-hidden", "true"); document.body.append(stage);
    if (spec.theme === "light") stage.classList.add("ct-report-light");
    const tasks = pages.map(tile => ({ spec, info, frozen, tile }));
    let appendix = null;
    if (frozen.notes.length && spec.noteAppendix !== false) {
      appendix = text("section", "tx-note-appendix", ""); appendix.style.width = `${Math.min(info.width, PDF_WIDTH)}px`;
      frozen.notes.forEach((note, index) => appendix.append(text("p", "tx-note-paragraph", `${index + 1}. ${note}`)));
      if (spec.format === "pdf") {
        stage.append(appendix);
        const appendixInfo = measure({ source: appendix, type: "activity" }), appendixSnapshot = snapshot(appendixInfo);
        tasks.push(...planPages(appendixInfo).map(tile => ({ spec: { ...spec, type: "activity", title: `${spec.title || "Linha do tempo"} · Notas completas` }, info: appendixInfo, frozen: appendixSnapshot, tile })));
      }
    }
    if (tasks.length > MAX_PAGES) { stage.remove(); throw new Error(`A exportação excede ${MAX_PAGES} páginas. Refine o recorte e tente novamente.`); }
    let pdf, blob, outputWidth, outputHeight;
    if (spec.format === "pdf") {
      pdf = new window.jspdf.jsPDF({ orientation: "landscape", unit: "pt", format: "a4", compress: true });
      pdf.setProperties({ title: spec.title || "Linha do tempo", subject: spec.subtitle || "", creator: "LogInsight", author: "" });
    }
    try {
      for (let index = 0; index < tasks.length; index++) {
        abort(signal); progress(index, tasks.length, spec.format === "pdf" ? `Desenhando página ${index + 1} de ${tasks.length}…` : "Desenhando imagem completa…");
        const task = tasks[index], tile = task.tile, sheet = pageNode(task.spec, task.info, task.frozen, tile, index, tasks.length);
        if (!pdf && appendix) { appendix.style.width = "100%"; appendix.prepend(text("h3", "", "Notas completas")); sheet.insertBefore(appendix, sheet.lastElementChild); }
        stage.replaceChildren(sheet); await nextFrame(); abort(signal);
        const ratio = pdf ? 1.5 : Math.min(2, MAX_EDGE / Math.max(sheet.offsetWidth, sheet.offsetHeight), Math.sqrt(MAX_PIXELS / (sheet.offsetWidth * sheet.offsetHeight)));
        if (ratio < 1) throw new Error("A imagem completa ultrapassa o limite de tamanho. Escolha PDF para manter todas as notas legíveis.");
        const canvas = await window.htmlToImage.toCanvas(sheet, { pixelRatio: ratio, fontEmbedCSS, skipAutoScale: true, backgroundColor: getComputedStyle(sheet).backgroundColor, cacheBust: false });
        abort(signal); outputWidth = canvas.width; outputHeight = canvas.height;
        if (pdf) {
          if (index) pdf.addPage();
          const pageW = pdf.internal.pageSize.getWidth(), pageH = pdf.internal.pageSize.getHeight(), margin = 20;
          const scale = Math.min((pageW - margin * 2) / tile.width, (pageH - margin * 2) / (tile.height + HEADER + FOOTER));
          pdf.addImage(canvas, "PNG", margin, margin, tile.width * scale, (tile.height + HEADER + FOOTER) * scale, `page${index}`, "FAST");
        } else blob = await new Promise((resolve, reject) => canvas.toBlob(value => value ? resolve(value) : reject(new Error("Não foi possível criar a imagem. Tente PDF ou reduza o recorte.")), "image/png"));
        canvas.width = 1; canvas.height = 1; await nextFrame(); abort(signal);
      }
      if (pdf) blob = pdf.output("blob");
      if (!blob || blob.size > MAX_BYTES) throw new Error("O arquivo ultrapassa 32 MB. Reduza a escala ou refine o recorte e tente novamente.");
      progress(tasks.length, tasks.length, "Arquivo pronto.");
      return { blob, width: outputWidth, height: outputHeight, pages: pdf ? pdf.getNumberOfPages() : 1 };
    } finally { stage.remove(); }
  }
  const base64 = blob => new Promise((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result).split(",")[1]); reader.onerror = reject; reader.readAsDataURL(blob); });
  function filenameFor(spec, date = new Date()) {
    const slug = String(spec.filename || "timeline").replace(/\.(?:png|pdf)$/i, "").normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 145) || "timeline";
    const stamp = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
    return `${slug}-${stamp}.${spec.format}`;
  }
  async function save(blob, spec) {
    const filename = filenameFor(spec);
    if (window.__TAURI__?.core?.invoke) return window.__TAURI__.core.invoke("export_timeline", { format: spec.format, filename, base64: await base64(blob) });
    const url = URL.createObjectURL(blob), link = document.createElement("a"); link.href = url; link.download = filename; link.hidden = true; document.body.append(link); link.click(); setTimeout(() => { link.remove(); URL.revokeObjectURL(url); }, 30000); return { saved: true };
  }
  function open(spec, origin) {
    if (dialog) return;
    let info;
    try { info = measure(spec); } catch (error) { if (window.toast) toast(error.message, "err"); return; }
    const png = pngPlan(info), pdfPages = planPages(info).length;
    const overlay = text("div", "tx-backdrop", ""); dialog = overlay;
    overlay.innerHTML = '<section class="tx-dialog" role="dialog" aria-modal="true" aria-labelledby="tx-title"><h2 id="tx-title">Exportar timeline</h2><p>A visão completa, incluindo o conteúdo fora da rolagem, com o tema atual.</p><div class="tx-formats"><label><input type="radio" name="tx-format" value="png" checked><span><strong>Imagem PNG</strong><small>Um arquivo completo</small></span></label><label><input type="radio" name="tx-format" value="pdf"><span><strong>Documento PDF</strong><small>Páginas legíveis</small></span></label></div><p data-tx-status role="status" aria-live="polite"></p><progress hidden></progress><div class="tx-actions"><button type="button" class="btn ghost" data-tx-cancel>Cancelar</button><button type="button" class="btn primary" data-tx-save>Exportar</button></div></section>';
    document.body.append(overlay);
    const status = overlay.querySelector("[data-tx-status]"), submit = overlay.querySelector("[data-tx-save]"), cancel = overlay.querySelector("[data-tx-cancel]"), meter = overlay.querySelector("progress");
    let controller = null, writing = false;
    const selectedFormat = () => overlay.querySelector("input:checked").value;
    const update = () => { const format = selectedFormat(); status.classList.remove("error"); status.textContent = format === "png" ? png.allowed ? `${Math.round(png.width * png.ratio).toLocaleString("pt-BR")} × ${Math.round(png.height * png.ratio).toLocaleString("pt-BR")} pixels` : "Esta timeline é grande demais para uma única imagem. Escolha PDF para preservar a leitura." : pdfPages > MAX_PAGES ? `Mais de ${MAX_PAGES} páginas. Reduza a escala ou refine o recorte.` : `${pdfPages.toLocaleString("pt-BR")} ${pdfPages === 1 ? "página" : "páginas"} estimadas · conteúdo dividido sem reduzir tudo a uma miniatura.`; submit.disabled = format === "png" ? !png.allowed : pdfPages > MAX_PAGES; };
    const close = () => { if (writing) return; controller?.abort(); overlay.remove(); dialog = null; origin?.focus({ preventScroll: true }); };
    cancel.onclick = close; overlay.onclick = event => { if (event.target === overlay && !controller) close(); };
    overlay.onkeydown = event => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
      if (event.key !== "Tab") return;
      const nodes = [...overlay.querySelectorAll("button,input")].filter(node => !node.disabled);
      if (event.shiftKey && document.activeElement === nodes[0]) { event.preventDefault(); nodes.at(-1).focus(); }
      else if (!event.shiftKey && document.activeElement === nodes.at(-1)) { event.preventDefault(); nodes[0].focus(); }
    };
    overlay.querySelectorAll("input").forEach(input => input.onchange = update);
    submit.onclick = async () => {
      const format = selectedFormat(); controller = new AbortController();
      const operation = controller; submit.disabled = true; overlay.querySelectorAll("input").forEach(input => input.disabled = true); meter.hidden = false; status.classList.remove("error");
      try {
        const result = await render({ ...spec, format }, { signal: operation.signal, progress: (done, total, message) => { meter.max = total; meter.value = done; status.textContent = message; } });
        abort(operation.signal); writing = true; cancel.disabled = true; status.textContent = "Escolha onde salvar o arquivo…";
        const saved = await save(result.blob, { ...spec, format }); writing = false; controller = null;
        if (saved?.saved) { close(); if (window.toast) toast(`${format.toUpperCase()} exportado.`, "ok"); }
        else { status.textContent = "Salvamento cancelado. A timeline continua disponível."; }
      } catch (error) { if (error.name !== "AbortError" && overlay.isConnected) { status.textContent = String(error.message || error); status.classList.add("error"); } }
      finally { writing = false; controller = null; meter.hidden = true; submit.disabled = false; cancel.disabled = false; overlay.querySelectorAll("input").forEach(input => input.disabled = false); }
    };
    update(); submit.focus();
  }
  function attach(target, spec) {
    const button = document.createElement("button"); button.type = "button"; button.className = "btn ghost small tx-trigger"; button.innerHTML = '<i class="fas fa-download"></i> Exportar'; button.title = "Exportar timeline completa em PNG ou PDF";
    button.onclick = () => open(typeof spec === "function" ? spec() : spec, button); target.append(button); return button;
  }
  return { attach, open, render, measure, planPages, filenameFor, limits: { maxPixels: MAX_PIXELS, maxEdge: MAX_EDGE, maxBytes: MAX_BYTES, maxPages: MAX_PAGES } };
})();
