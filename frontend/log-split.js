/* Estrutura → Separar logs: cuts a large log into the items that match a rule and the rest.
   Files are chosen in native pickers opened by the backend; this page only holds their token. */
window.LogSplit = (() => {
  "use strict";
  const FRAMINGS = [
    ["auto", "Automático", "Reconhece sozinho: itens que começam com data, com stack traces e continuações juntos."],
    ["line", "Cada linha", "Um item por linha."],
    ["startDate", "Começa com data e hora", "Toda linha iniciada por data/hora abre um item; as demais continuam o anterior."],
    ["continuation", "Recuos e stack traces continuam", "Linhas recuadas, “at …”, “Caused by:” e linhas em branco ficam no item de cima."],
    ["startRegex", "Começa com um padrão", "Toda linha que corresponde à expressão regular abre um item.", "^\\d{4}-\\d{2}-\\d{2}"],
    ["endRegex", "Termina com um padrão", "A linha que corresponde à expressão regular é a última do item.", "^</event>$"],
    ["blankLine", "Separado por linha em branco", "Blocos de linhas separados por uma ou mais linhas vazias."],
    ["delimiter", "Separador personalizado", "Texto que encerra cada item, em qualquer posição. Use \\n, \\t, \\0 ou \\xNN.", "\\n\\n"],
  ];
  const MODES = [
    ["matched", "Só o arquivo filtrado", "Grava os itens que atendem aos critérios. O original fica intacto."],
    ["both", "Filtrado e restante", "Grava dois arquivos: o que atende e o que não atende. O original fica intacto; é preciso espaço livre do tamanho dele."],
    ["move", "Filtrado e restante, removendo o original", "No fim, o original é apagado. Com pouco espaço livre, a separação é feita em partes a partir do fim do arquivo, sem duplicar o volume; cancelar devolve tudo ao original."],
  ];
  const KINDS = { text: "Texto", regex: "Expressão regular", date: "Data e hora", ordinal: "Posição" };
  const PHASES = { "split-decide": "Analisando o arquivo (nada é alterado nesta etapa)", "split-carve": "Separando do fim para o início", "split-assemble": "Montando os arquivos", "split-undo": "Devolvendo os trechos ao original", "split-stream": "Separando" };
  let serial = 0;
  const ui = {
    host: null, file: null, framing: { kind: "auto", pattern: "", ignoreCase: false, text: "\\n\\n" },
    combine: "all", sorted: false, mode: "both", conditions: [], preview: null, previewError: "",
    result: null, error: "", running: null, pending: [], progress: null,
  };
  const fmtBytes = n => n == null ? "—" : n >= 1024 ** 3 ? `${(n / 1024 ** 3).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} GB` : n >= 1024 ** 2 ? `${(n / 1024 ** 2).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} MB` : `${fmtNum(Math.ceil(n / 1024))} KB`;
  const newCondition = kind => ({ id: ++serial, kind, negate: false, op: "contains", values: "", ignoreCase: true, scope: "item", pattern: "", from: "", to: "", locate: "auto", format: "", order: "auto", zone: "local", missing: "previous", unit: "item", start: "", end: "" });
  if (!ui.conditions.length) ui.conditions.push(newCondition("text"));

  const toMs = (value, zone) => {
    if (!value) return null;
    const text = value.length === 16 ? `${value}:00` : value;
    const ms = zone === "utc" ? Date.parse(`${text}Z`) : new Date(text).getTime();
    return Number.isFinite(ms) ? ms : NaN;
  };
  const toCount = value => {
    if (value === "" || value == null) return null;
    const n = Number(value);
    return Number.isInteger(n) && n >= 1 ? n : NaN;
  };

  /** The request the backend reads, or an error message. */
  function buildSpec() {
    const f = ui.framing;
    const framing = ["startRegex", "endRegex"].includes(f.kind) ? { kind: f.kind, pattern: f.pattern, ignoreCase: f.ignoreCase } : f.kind === "delimiter" ? { kind: "delimiter", text: f.text } : { kind: f.kind };
    if (framing.pattern === "" || framing.text === "") return { error: "Informe o padrão que delimita os itens." };
    const conditions = [];
    for (const [index, c] of ui.conditions.entries()) {
      const n = index + 1;
      if (c.kind === "text") {
        const values = c.values.split("\n").map(v => v.replace(/\r$/, "")).filter(v => v !== "");
        if (!values.length) return { error: `Critério ${n}: informe o texto procurado.` };
        conditions.push({ kind: "text", negate: c.negate, op: c.op, values, ignoreCase: c.ignoreCase, scope: c.scope });
      } else if (c.kind === "regex") {
        if (!c.pattern) return { error: `Critério ${n}: informe a expressão regular.` };
        conditions.push({ kind: "regex", negate: c.negate, pattern: c.pattern, ignoreCase: c.ignoreCase, scope: c.scope });
      } else if (c.kind === "date") {
        const from = toMs(c.from, c.zone), to = toMs(c.to, c.zone);
        if (Number.isNaN(from) || Number.isNaN(to)) return { error: `Critério ${n}: data inválida.` };
        if (from == null && to == null) return { error: `Critério ${n}: informe “a partir de”, “antes de” ou ambos.` };
        if (["regex", "field"].includes(c.locate) && !c.pattern) return { error: `Critério ${n}: informe ${c.locate === "regex" ? "a expressão regular da data" : "o campo JSON da data"}.` };
        conditions.push({ kind: "date", negate: c.negate, from, to, locate: c.locate, pattern: c.pattern || null, format: c.format || null, order: c.order, zone: c.zone, missing: c.missing });
      } else {
        const from = toCount(c.start), to = toCount(c.end);
        if (Number.isNaN(from) || Number.isNaN(to)) return { error: `Critério ${n}: use números inteiros a partir de 1.` };
        if (from == null && to == null) return { error: `Critério ${n}: informe o início, o fim ou ambos.` };
        conditions.push({ kind: "ordinal", negate: c.negate, unit: c.unit, from, to });
      }
    }
    if (!conditions.length) return { error: "Adicione ao menos um critério." };
    return { spec: { framing, mode: ui.mode, rule: { combine: ui.combine, sorted: ui.sorted && ui.conditions.some(c => c.kind === "date"), conditions } } };
  }

  // ------------------------------------------------------------------ rendering
  const radio = (name, value, checked, title, text, extra = "") =>
    `<label class="ls-choice${checked ? " selected" : ""}"><input type="radio" name="${name}" value="${value}"${checked ? " checked" : ""}><span><strong>${esc(title)}</strong><small>${esc(text)}</small>${extra}</span></label>`;

  function fileCard() {
    const f = ui.file;
    if (!f) return `<div class="ls-empty"><i class="fas fa-scissors" aria-hidden="true"></i><div><strong>Separe um log grande em partes menores.</strong><p>O arquivo é lido em fluxo, sem carregar tudo na memória. Escolha os critérios, teste no início do arquivo e gere o resultado.</p></div></div>`;
    const chrono = f.chronological === true ? "As datas do início do arquivo estão em ordem cronológica." : f.chronological === false ? "As datas do início do arquivo não estão em ordem." : "";
    return `<dl class="ls-facts">
        <div><dt>Arquivo</dt><dd class="ls-path" title="${esc(f.path)}">${esc(f.name)}</dd></div>
        <div><dt>Tamanho</dt><dd>${fmtBytes(f.size)}</dd></div>
        <div><dt>Codificação</dt><dd>${esc(f.encoding)} · ${esc(f.lineBreak)}</dd></div>
        <div><dt>Primeira data</dt><dd>${f.firstDate != null ? esc(fmtTsFull(f.firstDate)) : "não reconhecida"}</dd></div>
        <div><dt>Livre no destino</dt><dd>${fmtBytes(f.freeBytes)}</dd></div>
      </dl>
      ${chrono ? `<p class="ls-hint">${esc(chrono)}</p>` : ""}
      <details class="ls-sample"><summary>Primeiras linhas</summary><pre>${esc(f.lines.join("\n"))}</pre></details>`;
  }

  function framingCard() {
    const f = ui.framing;
    const list = FRAMINGS.map(([kind, title, text, placeholder]) => {
      let extra = "";
      if (kind === f.kind && (kind === "startRegex" || kind === "endRegex")) extra = `<span class="ls-inline"><input data-framing="pattern" spellcheck="false" placeholder="${esc(placeholder)}" value="${esc(f.pattern)}" aria-label="Expressão regular"><label class="ls-check"><input type="checkbox" data-framing="ignoreCase"${f.ignoreCase ? " checked" : ""}> ignorar maiúsculas</label></span>`;
      if (kind === f.kind && kind === "delimiter") extra = `<span class="ls-inline"><input data-framing="text" spellcheck="false" placeholder="${esc(placeholder)}" value="${esc(f.text)}" aria-label="Separador"></span>`;
      return radio("ls-framing", kind, kind === f.kind, title, text, extra);
    }).join("");
    const note = f.kind === "auto" && ui.file ? `<p class="ls-hint"><i class="fas fa-wand-magic-sparkles" aria-hidden="true"></i> ${esc(ui.file.framing)}</p>` : "";
    return `<div class="ls-choices ls-grid">${list}</div>${note}`;
  }

  function conditionFields(c) {
    const opt = (value, label, current) => `<option value="${value}"${value === current ? " selected" : ""}>${esc(label)}</option>`;
    const scope = `<label>Onde<select data-field="scope">${opt("item", "No item inteiro", c.scope)}${opt("firstLine", "Só na primeira linha", c.scope)}</select></label>`;
    const caseBox = `<label class="ls-check"><input type="checkbox" data-field="ignoreCase"${c.ignoreCase ? " checked" : ""}> ignorar maiúsculas e minúsculas</label>`;
    if (c.kind === "text") return `<div class="ls-fields"><label>Regra<select data-field="op">${opt("contains", "contém", c.op)}${opt("startsWith", "começa com", c.op)}${opt("endsWith", "termina com", c.op)}${opt("equals", "é igual a", c.op)}</select></label>${scope}<label class="ls-wide">Textos <small>um por linha · basta um deles</small><textarea data-field="values" rows="2" spellcheck="false" placeholder="ERROR">${esc(c.values)}</textarea></label>${caseBox}</div>`;
    if (c.kind === "regex") return `<div class="ls-fields"><label class="ls-wide">Expressão regular <small>^ e $ marcam início e fim de cada linha</small><input data-field="pattern" spellcheck="false" placeholder="(?i)timeout|refused" value="${esc(c.pattern)}"></label>${scope}${caseBox}</div>`;
    if (c.kind === "ordinal") return `<div class="ls-fields"><label>Contar<select data-field="unit">${opt("item", "itens (registros)", c.unit)}${opt("line", "linhas do arquivo", c.unit)}</select></label><label>Do nº<input type="number" min="1" step="1" data-field="start" value="${esc(c.start)}" placeholder="1"></label><label>Até o nº <small>inclusive</small><input type="number" min="1" step="1" data-field="end" value="${esc(c.end)}" placeholder="último"></label><p class="ls-hint ls-wide">${c.unit === "line" ? "Um item de várias linhas entra pela linha em que começa." : "Itens contados como definido em “O que é um item”: um stack trace inteiro conta como um."}</p></div>`;
    const patternLabel = c.locate === "regex" ? `Expressão com a data <small>grupo (?P&lt;data&gt;…) ou o 1º grupo</small>` : `Campo JSON <small>caminho com pontos, ex.: @timestamp ou event.created</small>`;
    return `<div class="ls-fields">
      <label>A partir de <small>inclusive</small><input type="datetime-local" step="1" data-field="from" value="${esc(c.from)}"></label>
      <label>Antes de <small>exclusive</small><input type="datetime-local" step="1" data-field="to" value="${esc(c.to)}"></label>
      <label>Horários sem fuso estão em<select data-field="zone">${opt("local", "fuso deste computador", c.zone)}${opt("utc", "UTC", c.zone)}</select></label>
      <label>Item sem data<select data-field="missing">${opt("previous", "usa a data do item anterior", c.missing)}${opt("noMatch", "não entra", c.missing)}${opt("match", "entra", c.missing)}</select></label>
      <details class="ls-wide ls-date-how"${c.locate !== "auto" || c.format || c.order !== "auto" ? " open" : ""}><summary>Como reconhecer a data</summary><div class="ls-fields">
        <label>Onde está<select data-field="locate">${opt("auto", "primeira data do item (qualquer formato)", c.locate)}${opt("lineStart", "no início da linha", c.locate)}${opt("regex", "capturada por expressão regular", c.locate)}${opt("field", "campo de JSON", c.locate)}</select></label>
        ${["regex", "field"].includes(c.locate) ? `<label class="ls-wide">${patternLabel}<input data-field="pattern" spellcheck="false" value="${esc(c.pattern)}" placeholder="${c.locate === "regex" ? "time=(?P&lt;data&gt;\\S+)" : "@timestamp"}"></label>` : ""}
        <label>Formato <small>opcional</small><input data-field="format" spellcheck="false" value="${esc(c.format)}" placeholder="dd/MM/yyyy HH:mm:ss · %d/%m/%Y · epoch"></label>
        <label>Datas numéricas<select data-field="order">${opt("auto", "ordem detectada no arquivo", c.order)}${opt("dayFirst", "dia/mês", c.order)}${opt("monthFirst", "mês/dia", c.order)}</select></label>
        <p class="ls-hint ls-wide">Sem formato, a data é reconhecida em qualquer layout comum (ISO, Apache, syslog, datas por extenso, epoch). Use “Testar” para conferir a data lida em cada item.</p>
      </div></details>
    </div>`;
  }

  function conditionsCard() {
    const rows = ui.conditions.map((c, index) => `<div class="ls-cond" data-id="${c.id}">
        <div class="ls-cond-head"><span class="ls-cond-kind">${index ? `<em>${ui.combine === "all" ? "E" : "OU"}</em>` : ""}${esc(KINDS[c.kind])}</span>
          <label class="ls-check"><input type="checkbox" data-field="negate"${c.negate ? " checked" : ""}> inverter (NÃO)</label>
          <button type="button" class="icon-btn" data-remove="${c.id}" title="Remover critério" aria-label="Remover critério"${ui.conditions.length === 1 ? " disabled" : ""}><i class="fas fa-xmark"></i></button></div>
        ${conditionFields(c)}</div>`).join("");
    const hasDate = ui.conditions.some(c => c.kind === "date");
    const hint = ui.file?.chronological === true ? " As datas do início deste arquivo estão em ordem." : "";
    return `${rows}<div class="ls-add"><span>Adicionar</span>${Object.entries(KINDS).map(([kind, label]) => `<button type="button" class="btn ghost small" data-add="${kind}"><i class="fas fa-plus"></i> ${esc(label)}</button>`).join("")}</div>
      ${hasDate ? `<label class="ls-check ls-sorted"><input type="checkbox" id="ls-sorted"${ui.sorted ? " checked" : ""}> <span><strong>O arquivo está em ordem cronológica</strong><small>Bem mais rápido em arquivos grandes: localiza o início do período por busca binária e para de ler quando o passa. Itens fora de ordem depois desse ponto não são revistos.${esc(hint)}</small></span></label>` : ""}`;
  }

  function outputRow(side, label) {
    const f = ui.file, path = side === "matched" ? f?.matchedPath : f?.restPath, chosen = side === "matched" ? f?.matchedChosen : f?.restChosen;
    return `<div class="ls-output"><span>${label}</span><code title="${esc(path || "")}">${esc(path || "—")}</code><button type="button" class="btn ghost small" data-output="${side}"${f ? "" : " disabled"}>${chosen ? "Alterar…" : "Escolher outro…"}</button></div>`;
  }

  function spaceNote() {
    const f = ui.file;
    if (!f || f.freeBytes == null) return "";
    if (ui.mode === "both" && f.freeBytes < f.size) return `<p class="notice">Espaço livre (${fmtBytes(f.freeBytes)}) menor que o tamanho do original (${fmtBytes(f.size)}). Escolha outra pasta ou remova o original, que separa em partes.</p>`;
    if (ui.mode === "move" && f.freeBytes < f.size) return f.sameVolume
      ? `<p class="ls-hint"><i class="fas fa-layer-group" aria-hidden="true"></i> Pouco espaço livre: a separação será feita em partes, consumindo o original do fim para o início. Precisa de algumas dezenas de MB livres.</p>`
      : `<p class="notice">Para separar em partes sem espaço extra, salve os dois arquivos no mesmo disco do original.</p>`;
    return "";
  }

  function outputCard() {
    return `<div class="ls-choices">${MODES.map(([mode, title, text]) => radio("ls-mode", mode, mode === ui.mode, title, text)).join("")}</div>
      <div class="ls-outputs">${outputRow("matched", "Filtrado")}${ui.mode === "matched" ? "" : outputRow("rest", "Restante")}</div>${spaceNote()}`;
  }

  function progressCard() {
    const r = ui.running, p = ui.progress;
    if (!r) return "";
    const pct = p && p.total > 0 ? Math.min(100, (p.completed / p.total) * 100) : null;
    const locked = p?.phaseId === "split-assemble" || p?.phaseId === "split-undo" || r.cancelling;
    const phase = p ? PHASES[p.phaseId] || p.phase : "Preparando";
    const detail = p && p.total > 0 ? `${fmtNum(p.completed)} de ${fmtNum(p.total)} MB${p.phaseId === "split-stream" && p.selected ? ` · ${fmtNum(p.selected)} itens filtrados` : ""}` : "";
    return `<div class="ls-progress"><div class="ls-progress-head"><strong>${esc(r.label)}</strong><span>${esc(phase)}${detail ? ` · ${esc(detail)}` : ""}</span></div>
      <div class="ls-bar${pct == null ? " indeterminate" : ""}"><i style="width:${pct ?? 30}%"></i></div>
      <div class="ls-progress-foot"><span class="ls-hint">${p?.phaseId === "split-carve" ? "Cancelar devolve ao original os trechos já separados." : p?.phaseId === "split-assemble" ? "Montando os arquivos finais; esta etapa não pode ser interrompida." : "Você pode trocar de página; a separação continua."}</span>
      <button type="button" class="btn ghost small" data-act="cancel"${locked ? " disabled" : ""}>${r.cancelling ? "Cancelando…" : "Cancelar"}</button></div></div>`;
  }

  function resultCard() {
    if (ui.error) {
      const cancelled = /cancelad/i.test(ui.error);
      return `<div class="ls-result ${cancelled ? "cancelled" : "failed"}"><i class="fas ${cancelled ? "fa-circle-stop" : "fa-triangle-exclamation"}" aria-hidden="true"></i><div><strong>${cancelled ? "Separação cancelada" : "A separação não foi concluída"}</strong><p>${esc(cancelled && /^Opera[cç][aã]o cancelada\.?$/i.test(ui.error) ? "Nenhum arquivo foi alterado; os arquivos parciais foram removidos." : ui.error)}</p></div></div>`;
    }
    const r = ui.result;
    if (!r) return "";
    const s = r.stats, outputs = [["Filtrado", r.matched], ["Restante", r.rest]].filter(([, o]) => o);
    const counts = s.items > 0 ? `${fmtNum(s.items)} itens avaliados um a um · ${fmtNum(s.matchedItems)} filtrados` : "";
    const notes = [
      counts,
      s.regionBytes > 0 ? `${fmtBytes(s.regionBytes)} decididos em bloco (sem precisar avaliar item a item)` : "",
      s.undated > 0 ? `${fmtNum(s.undated)} itens sem data reconhecida` : "",
      s.oversized > 0 ? `${fmtNum(s.oversized)} itens maiores que 16 MB, decididos pelo início` : "",
      r.strategy === "parts" ? "Separado em partes, sem duplicar o volume" : "",
      r.originalRemoved ? "Arquivo original removido" : "",
      `${window.PerformanceTools?.duration ? window.PerformanceTools.duration(r.elapsedMs / 1000) : `${Math.round(r.elapsedMs / 1000)} s`}`,
    ].filter(Boolean);
    return `<div class="ls-result"><i class="fas fa-circle-check" aria-hidden="true"></i><div><strong>Separação concluída</strong>
      ${outputs.map(([label, o]) => `<div class="ls-output"><span>${label}</span><code title="${esc(o.path)}">${esc(o.path)}</code><b>${fmtBytes(o.bytes)}</b></div>`).join("")}
      <p class="ls-hint">${notes.map(esc).join(" · ")}</p></div></div>`;
  }

  function previewCard() {
    if (ui.previewError) return `<p class="notice">${esc(ui.previewError)}</p>`;
    const p = ui.preview;
    if (!p) return "";
    const rows = p.items.map(item => `<tr class="${item.matched ? "in" : "out"}"><td class="num">${fmtNum(item.number)}</td><td class="num">${fmtNum(item.line)}${item.lines > 1 ? `<small>+${fmtNum(item.lines - 1)}</small>` : ""}</td><td class="ls-date">${item.date != null ? esc(fmtTsFull(item.date)) : "<em>sem data</em>"}</td><td><span class="ls-badge ${item.matched ? "in" : "out"}">${item.matched ? "Filtrado" : "Restante"}</span></td><td><pre>${esc(item.text)}</pre></td></tr>`).join("");
    const scope = p.wholeFile ? "no arquivo inteiro" : `nos primeiros ${fmtBytes(p.sampledBytes)}`;
    return `<div class="card-heading"><h2>Teste no início do arquivo</h2><span>${fmtNum(p.evaluated)} itens ${scope} · ${fmtNum(p.matched)} filtrados${p.undated ? ` · ${fmtNum(p.undated)} sem data` : ""}</span></div>
      <p class="ls-hint ls-pad">${esc(p.framing)}</p>
      <div class="ls-table-scroll"><table class="ws-table ls-preview"><thead><tr><th class="num">Item</th><th class="num">Linha</th><th>Data reconhecida</th><th>Resultado</th><th>Conteúdo</th></tr></thead><tbody>${rows || `<tr><td colspan="5">Nenhum item lido.</td></tr>`}</tbody></table></div>
      ${p.items.length < p.evaluated ? `<p class="ls-hint ls-pad">Mostrando os ${fmtNum(p.items.length)} primeiros itens.</p>` : ""}`;
  }

  function pendingCard() {
    if (!ui.pending.length) return "";
    return ui.pending.map(job => `<div class="notice ls-pending"><div><strong>Separação interrompida: ${esc(job.original.split(/[\\/]/).pop())}</strong><br>${job.phase === "carve" ? `${fmtBytes(job.carved)} de ${fmtBytes(job.originalLen)} já foram movidos para partes temporárias; o original está mais curto até a separação ser concluída ou desfeita.` : "Faltava montar os arquivos finais."}</div>
      <div class="ls-pending-actions"><button type="button" class="btn primary small" data-recover="${esc(job.id)}" data-undo="false"${ui.running ? " disabled" : ""}>Concluir</button>${job.canUndo ? `<button type="button" class="btn ghost small" data-recover="${esc(job.id)}" data-undo="true"${ui.running ? " disabled" : ""}>Desfazer</button>` : ""}</div></div>`).join("");
  }

  function render() {
    const host = ui.host;
    if (!host?.isConnected) return;
    const focus = document.activeElement && host.contains(document.activeElement) ? { key: focusKey(document.activeElement), start: document.activeElement.selectionStart, end: document.activeElement.selectionEnd } : null;
    const busy = !!ui.running;
    host.innerHTML = `<div class="ls-page">
      ${pendingCard()}
      <section class="ws-card ls-step"><div class="card-heading"><h2><b>1</b>Arquivo</h2><button type="button" class="btn ${ui.file ? "ghost" : "primary"} small" data-act="choose"${busy ? " disabled" : ""}><i class="fas fa-folder-open"></i> ${ui.file ? "Trocar arquivo…" : "Escolher arquivo…"}</button></div><div class="ls-body">${fileCard()}</div></section>
      <fieldset class="ls-fieldset"${busy ? " disabled" : ""}>
      <section class="ws-card ls-step"><div class="card-heading"><h2><b>2</b>O que é um item do log</h2><span>Logs Java têm itens de várias linhas (stack traces)</span></div><div class="ls-body">${framingCard()}</div></section>
      <section class="ws-card ls-step"><div class="card-heading"><h2><b>3</b>Critérios do filtro</h2><label class="ls-combine">Combinar<select id="ls-combine"><option value="all"${ui.combine === "all" ? " selected" : ""}>todos os critérios (E)</option><option value="any"${ui.combine === "any" ? " selected" : ""}>qualquer critério (OU)</option></select></label></div><div class="ls-body">${conditionsCard()}</div></section>
      <section class="ws-card ls-step"><div class="card-heading"><h2><b>4</b>Resultado</h2></div><div class="ls-body">${outputCard()}</div></section>
      </fieldset>
      <div class="ls-actions"><button type="button" class="btn ghost" data-act="preview"${ui.file && !busy ? "" : " disabled"}><i class="fas fa-flask"></i> Testar no início do arquivo</button><button type="button" class="btn primary" data-act="run"${ui.file && !busy ? "" : " disabled"}><i class="fas fa-scissors"></i> Separar</button></div>
      <section class="ws-card" id="ls-progress"${busy ? "" : " hidden"}>${progressCard()}</section>
      <section class="ws-card" id="ls-result"${ui.result || ui.error ? "" : " hidden"}>${resultCard()}</section>
      <section class="ws-card" id="ls-preview"${ui.preview || ui.previewError ? "" : " hidden"}>${previewCard()}</section>
    </div>`;
    if (focus?.key) {
      const node = host.querySelector(focus.key);
      if (node) { node.focus(); try { if (focus.start != null) node.setSelectionRange(focus.start, focus.end); } catch { /* not a text field */ } }
    }
  }

  function focusKey(node) {
    const cond = node.closest("[data-id]")?.dataset.id;
    if (node.dataset.field && cond) return `[data-id="${cond}"] [data-field="${node.dataset.field}"]`;
    if (node.dataset.framing) return `[data-framing="${node.dataset.framing}"]`;
    if (node.id) return `#${node.id}`;
    return null;
  }

  function paintProgress() {
    const card = ui.host?.querySelector("#ls-progress");
    if (card) { card.hidden = !ui.running; card.innerHTML = progressCard(); }
  }

  // ------------------------------------------------------------------ actions
  async function choose() {
    try {
      const file = await api("split_choose_input", {}, { silent: true });
      if (!file) return;
      ui.file = file; ui.preview = null; ui.previewError = ""; ui.result = null; ui.error = "";
      ui.sorted = false;
      render();
    } catch (error) { toast(String(error), "err"); }
  }

  async function chooseOutput(side) {
    try {
      const file = await api("split_choose_output", { token: ui.file.token, side }, { silent: true });
      if (file) { ui.file = file; render(); }
    } catch (error) { toast(String(error), "err"); }
  }

  async function preview() {
    const built = buildSpec();
    if (built.error) { toast(built.error, "err"); return; }
    ui.previewError = ""; ui.preview = null;
    const button = ui.host.querySelector('[data-act="preview"]');
    if (button) { button.disabled = true; button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Testando…'; }
    try {
      ui.preview = await api("split_preview", { token: ui.file.token, spec: built.spec }, { latest: "log-split-preview", silent: true });
    } catch (error) {
      if (!/cancelad/i.test(String(error))) ui.previewError = String(error);
    }
    render();
    ui.host?.querySelector("#ls-preview")?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  async function track(label, command, args) {
    ui.running = { label, cancelling: false }; ui.progress = null; ui.result = null; ui.error = "";
    render();
    const promise = api(command, args, { latest: "log-split", silent: true });
    ui.running.operationId = window.Tasks?.operationFor("log-split") || null;
    try {
      const result = await promise;
      ui.result = result || null;
      if (result) toast("Separação concluída.", "ok");
      else toast("O arquivo original foi restaurado.", "ok");
    } catch (error) {
      ui.error = String(error).replace(/^Error:\s*/, "");
    } finally {
      ui.running = null; ui.progress = null;
      if (ui.result?.originalRemoved) ui.file = null;
      else if (ui.file) ui.file = await api("split_inspect", { token: ui.file.token }, { silent: true }).catch(() => ui.file);
      await loadPending();
      render();
      ui.host?.querySelector("#ls-result")?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
  }

  async function run() {
    const built = buildSpec();
    if (built.error) { toast(built.error, "err"); return; }
    const f = ui.file;
    if (ui.mode === "move" && !confirm(`Separar “${f.name}” e REMOVER o arquivo original ao final?\n\nFiltrado: ${f.matchedPath}\nRestante: ${f.restPath}\n\nCom pouco espaço livre o original é consumido em partes; se você cancelar, ele é restaurado.`)) return;
    await track(`Separando ${f.name}`, "split_run", { token: f.token, spec: built.spec });
  }

  async function recover(id, undo) {
    const job = ui.pending.find(j => j.id === id);
    if (undo && !confirm("Devolver ao arquivo original todos os trechos já separados?")) return;
    await track(`${undo ? "Desfazendo" : "Concluindo"} a separação de ${job?.original.split(/[\\/]/).pop() || "arquivo"}`, "split_recover", { id, undo });
  }

  function cancel() {
    if (!ui.running) return;
    const id = ui.running.operationId || window.Tasks?.operationFor("log-split");
    if (!id) return;
    ui.running.cancelling = true; paintProgress();
    window.Tasks?.cancelOperation(id);
  }

  async function loadPending() {
    try { ui.pending = await api("split_pending", {}, { silent: true }) || []; } catch { ui.pending = []; }
  }

  // ------------------------------------------------------------------ events
  function condition(node) {
    const id = Number(node.closest("[data-id]")?.dataset.id);
    return ui.conditions.find(c => c.id === id);
  }

  function onInput(event) {
    const t = event.target;
    if (t.dataset.framing) { ui.framing[t.dataset.framing] = t.type === "checkbox" ? t.checked : t.value; return; }
    const c = t.dataset.field && condition(t);
    if (c) {
      c[t.dataset.field] = t.type === "checkbox" ? t.checked : t.value;
      // Fields that change which inputs exist redraw the card.
      if (event.type === "change" && ["locate", "unit"].includes(t.dataset.field)) render();
    }
  }

  function onChange(event) {
    const t = event.target;
    if (t.name === "ls-framing") { ui.framing.kind = t.value; ui.framing.pattern = ""; render(); return; }
    if (t.name === "ls-mode") { ui.mode = t.value; render(); return; }
    if (t.id === "ls-combine") { ui.combine = t.value; render(); return; }
    if (t.id === "ls-sorted") { ui.sorted = t.checked; return; }
    onInput(event);
  }

  function onClick(event) {
    const t = event.target.closest("button");
    if (!t || t.disabled) return;
    if (t.dataset.add) { ui.conditions.push(newCondition(t.dataset.add)); render(); return; }
    if (t.dataset.remove) { ui.conditions = ui.conditions.filter(c => c.id !== Number(t.dataset.remove)); render(); return; }
    if (t.dataset.output) { chooseOutput(t.dataset.output); return; }
    if (t.dataset.recover) { recover(t.dataset.recover, t.dataset.undo === "true"); return; }
    ({ choose, preview, run, cancel })[t.dataset.act]?.();
  }

  function onProgress(event) {
    const id = ui.running && (ui.running.operationId || window.Tasks?.operationFor("log-split"));
    if (!id || event.detail?.operationId !== id) return;
    ui.running.operationId = id;
    const entry = window.Tasks?.entryFor("log-split");
    if (entry?.progress) { ui.progress = entry.progress; paintProgress(); }
  }
  document.addEventListener("task-progress", onProgress);

  async function mount(host) {
    ui.host = host;
    host.addEventListener("input", onInput);
    host.addEventListener("change", onChange);
    host.addEventListener("click", onClick);
    render();
    await loadPending();
    if (ui.host === host) render();
  }

  function unmount() {
    const host = ui.host;
    if (!host) return;
    host.removeEventListener("input", onInput);
    host.removeEventListener("change", onChange);
    host.removeEventListener("click", onClick);
    ui.host = null;
  }

  return { mount, unmount, buildSpec, state: ui };
})();
