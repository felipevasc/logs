/* Local exploration. Native offsets, scopes and result snapshots are authoritative. */
window.LocalExplorer = (() => {
  "use strict";
  const pages = new Set(["sql", "processes"]);
  const defaults = () => ({
    sql: "SELECT id, event_ref, timestamp, level, message\nFROM events\nORDER BY timestamp DESC\nLIMIT 200",
    history: [],
    saved: [],
    expression: null,
    // Drafts of the extraction workbench: unsaved definitions kept apart from the ones saved in the Case.
    extraction: { drafts: {}, current: "", field: "message", count: 40 },
    rules: [
      {
        id: "severity-error",
        label: "Erro / crítico",
        color: "#ff9c8f",
        enabled: true,
        pattern: {
          engine: "rust",
          text: "\\b(error|critical|fatal|erro|crítico)\\b",
          caseSensitive: false,
        },
      },
      {
        id: "severity-warning",
        label: "Aviso",
        color: "#e9c76f",
        enabled: true,
        pattern: {
          engine: "rust",
          text: "\\b(warn(?:ing)?|aviso)\\b",
          caseSensitive: false,
        },
      },
    ],
  });
  let prefs = defaults(),
    root = null,
    page = null,
    generation = 0,
    ownerKey = "";
  let expressionTimer = 0;
  let sqlResult = null,
    schema = [],
    graph = null;
  const q = (selector) => root?.querySelector(selector);
  const h = (value) => esc(String(value ?? ""));
  const number = (value) => Number(value || 0).toLocaleString("pt-BR");
  const scope = () => workspaceScope();
  const stamp = () => window.LocalNative.stamp();
  const call = (request, options) => window.LocalNative.call(request, options);
  const button = (id, label, icon = "", cls = "ghost") =>
    `<button type="button" id="${id}" class="btn ${cls} small">${icon ? `<i class="fas ${icon}" aria-hidden="true"></i> ` : ""}${label}</button>`;
  const pageButton = (label, action, disabled = false) =>
    `<button type="button" class="btn ghost small" data-lx-page="${action}" ${disabled ? "disabled" : ""}>${label}</button>`;
  function notice(text = "", error = false) {
    const node = q("#lx-notice");
    if (node) {
      node.hidden = !text;
      node.dataset.error = String(error);
      node.textContent = text;
    }
  }
  function keep() {
    window.WorkspaceContext?.capture();
    void saveCases();
  }
  function capture() {
    return structuredClone(prefs);
  }
  function sanitize(value) {
    const base = defaults();
    if (!value || typeof value !== "object") return base;
    if (typeof value.sql === "string" && value.sql.length <= 131072)
      base.sql = value.sql;
    base.history = (Array.isArray(value.history) ? value.history : [])
      .filter((s) => typeof s === "string" && s.length <= 131072)
      .slice(0, 30);
    base.saved = (Array.isArray(value.saved) ? value.saved : [])
      .filter(
        (s) =>
          s &&
          typeof s.name === "string" &&
          s.name.length <= 120 &&
          typeof s.sql === "string" &&
          s.sql.length <= 131072,
      )
      .slice(0, 50);
    if (Array.isArray(value.rules))
      base.rules = value.rules
        .filter(
          (r) =>
            r &&
            typeof r.id === "string" &&
            typeof r.label === "string" &&
            /^#[a-f\d]{6}$/i.test(r.color) &&
            typeof r.pattern?.text === "string" &&
            r.pattern.text.length <= 16384 &&
            ["literal", "rust", "pcre2"].includes(r.pattern.engine),
        )
        .slice(0, 16);
    const e = value.extraction;
    if (e && typeof e === "object") {
      const drafts = {};
      for (const [name, draft] of Object.entries(e.drafts && typeof e.drafts === "object" ? e.drafts : {}).slice(0, 20))
        if (name.length <= 512 && draft && typeof draft.map === "string" && draft.map.length <= 262144 && Number.isFinite(draft.savedAt)) drafts[name] = { map: draft.map, savedAt: draft.savedAt, base: typeof draft.base === "string" && draft.base.length <= 262144 ? draft.base : "" };
      base.extraction = { drafts, current: typeof e.current === "string" && e.current.length <= 512 ? e.current : "", field: typeof e.field === "string" && e.field.length <= 512 ? e.field : "message", count: [20, 40, 80, 120].includes(e.count) ? e.count : 40 };
    }
    const x = value.expression;
    if (x && typeof x === "object" && typeof x.pattern === "string" && x.pattern.length <= 16384 && ["literal", "rust", "pcre2"].includes(x.engine) && typeof x.field === "string" && x.field.length <= 512)
      base.expression = { pattern: x.pattern, engine: x.engine, field: x.field, caseSensitive: !!x.caseSensitive, multiline: !!x.multiline, dotAll: !!x.dotAll, scope: x.scope === "line" ? "line" : "event" };
    return base;
  }
  function resetRuntime() {
    window.LocalNative.cancelAll();
    sqlResult = null;
    schema = [];
    graph = null;
  }
  function restore(value) {
    prefs = sanitize(value);
    resetRuntime();
    ownerKey = "";
  }
  function bind(id, action) {
    const node = q(`#${id}`),
      view = generation;
    if (node)
      node.onclick = () => {
        Promise.resolve()
          .then(action)
          .catch((error) => {
            if (view === generation)
              notice(String(error.message || error), true);
          });
      };
  }
  function pending(id, action, label = "Processando…") {
    bind(id, async () => {
      const node = q(`#${id}`),
        restore = node ? btnBusy(node, label) : () => {};
      try {
        await action();
      } finally {
        restore();
      }
    });
  }
  function paint(text, spans = [], marks = [], active = false) {
    // Interval boundaries preserve Unicode offsets supplied by Rust (UTF-16).
    const boundaries = new Set([0, text.length]);
    for (const s of [...spans, ...marks]) {
      boundaries.add(Math.max(0, Math.min(text.length, s.start)));
      boundaries.add(Math.max(0, Math.min(text.length, s.end)));
    }
    const points = [...boundaries].sort((a, b) => a - b);
    let output = "";
    for (let i = 0; i < points.length - 1; i++) {
      const a = points[i],
        b = points[i + 1];
      if (b <= a) continue;
      const match = spans.find((s) => s.start < b && s.end > a),
        mark = marks.find((s) => s.start < b && s.end > a);
      const chunk = h(text.slice(a, b));
      output += match
        ? `<mark class="lx-mark${active ? " lx-active" : ""}">${chunk}</mark>`
        : mark
          ? `<mark class="lx-mark" style="background:${mark.color}" title="${h(mark.label)}">${chunk}</mark>`
          : chunk;
    }
    return output;
  }
  const examples = [
    [
      "Eventos recentes",
      "SELECT id, event_ref, timestamp, level, message\nFROM events ORDER BY timestamp DESC LIMIT 200",
    ],
    [
      "Volume por nível",
      "SELECT level, count(*) AS total\nFROM events GROUP BY level ORDER BY total DESC",
    ],
    [
      "CTE e ranking",
      "WITH volume AS (\n  SELECT source, level, count(*) AS total\n  FROM events GROUP BY source, level\n)\nSELECT *, dense_rank() OVER (PARTITION BY source ORDER BY total DESC) AS ranking\nFROM volume ORDER BY source, ranking",
    ],
    [
      "Processos normalizados",
      'SELECT id, event_ref, "norm.process.image",\n       try_cast("norm.process.pid" AS BIGINT) AS pid,\n       "norm.process.user"\nFROM events WHERE "norm.event.kind" = \'process_start\'',
    ],
    [
      "Lista em linhas",
      '-- Um valor por linha de um campo que guarda uma lista separada por vírgula\n-- (por exemplo, uma coluna criada com "Todas as ocorrências")\nSELECT id, unnest(string_split("nmap.ports_open", \',\')) AS item\nFROM events WHERE "nmap.ports_open" IS NOT NULL',
    ],
    [
      "Uma linha por porta (nmap)",
      'WITH portas AS (\n  SELECT "destination.ip" AS ip, unnest(json_extract("nmap.ports", \'$[*]\')) AS p\n  FROM events WHERE "nmap.ports" IS NOT NULL\n)\nSELECT ip,\n       try_cast(json_extract_string(p, \'$.port\') AS INTEGER) AS porta,\n       json_extract_string(p, \'$.state\') AS estado,\n       json_extract_string(p, \'$.service\') AS servico,\n       json_extract_string(p, \'$.version\') AS versao\nFROM portas\nWHERE json_extract_string(p, \'$.state\') = \'open\'\nORDER BY ip, porta',
    ],
    [
      "Janelas de tempo",
      "SELECT date_trunc('minute', epoch_ms(timestamp)) AS minute, count(*) AS total\nFROM events WHERE timestamp IS NOT NULL\nGROUP BY minute ORDER BY minute",
    ],
  ];
  function historyControls() {
    if (!q("#lx-history")) return;
    q("#lx-history").innerHTML =
      `<label>Histórico<select id="lx-history-select"><option value="">Selecionar consulta…</option>${prefs.history.map((sql, i) => `<option value="${i}">${h(sql.slice(0, 100))}</option>`).join("")}</select></label><label>Consultas salvas<select id="lx-saved-select"><option value="">Selecionar…</option>${prefs.saved.map((item, i) => `<option value="${i}">${h(item.name)}</option>`).join("")}</select></label>${button("lx-query-delete", "Excluir salva")}`;
    q("#lx-history-select").onchange = () => {
      const sql = prefs.history[+q("#lx-history-select").value];
      if (sql) setSql(sql);
    };
    q("#lx-saved-select").onchange = () => {
      const saved = prefs.saved[+q("#lx-saved-select").value];
      if (saved) {
        setSql(saved.sql);
        q("#lx-query-name").value = saved.name;
      }
    };
    bind("lx-query-delete", () => {
      const index = q("#lx-saved-select").value;
      if (index !== "") {
        prefs.saved.splice(+index, 1);
        historyControls();
        keep();
      }
    });
  }
  function setSql(sql) {
    prefs.sql = sql;
    q("#lx-sql").value = sql;
    keep();
  }
  async function fetchSchema() {
    schema = await call(
      { action: "schema" },
      { latest: "local-schema", filters: q("#lx-sql-filters").checked },
    );
    renderSchema();
  }
  function renderSchema() {
    const node = q("#lx-schema");
    if (!node) return;
    node.innerHTML =
      schema
        .map(
          (column, i) =>
            `<button type="button" data-column="${i}"><span>${h(column.name)}</span><small>${h(column.dataType)}</small></button>`,
        )
        .join("") ||
      '<div class="lx-empty">Carregue os campos do contexto atual.</div>';
    node.querySelectorAll("[data-column]").forEach(
      (b) =>
        (b.onclick = () => {
          const editor = q("#lx-sql"),
            name = schema[+b.dataset.column].name;
          editor.setRangeText(
            `"${name.replaceAll('"', '""')}"`,
            editor.selectionStart,
            editor.selectionEnd,
            "end",
          );
          editor.focus();
          prefs.sql = editor.value;
          keep();
        }),
    );
  }
  async function executeSql() {
    notice();
    prefs.sql = q("#lx-sql").value;
    keep();
    const result = await call(
      { action: "query", sql: prefs.sql },
      { latest: "local-sql", filters: q("#lx-sql-filters").checked },
    );
    sqlResult = result;
    prefs.history = [
      prefs.sql,
      ...prefs.history.filter((s) => s !== prefs.sql),
    ].slice(0, 30);
    keep();
    renderSqlResult();
    historyControls();
  }
  function renderSqlResult() {
    const node = q("#lx-sql-result");
    if (!node) return;
    if (!sqlResult) {
      node.innerHTML =
        '<div class="lx-empty">Execute uma consulta para explorar o resultado.</div>';
      return;
    }
    const result = sqlResult,
      idIndex = result.columns.findIndex((c) => c.name === "id"),
      refIndex = result.columns.findIndex((c) => c.name === "event_ref"),
      canOpen = idIndex >= 0 && refIndex >= 0;
    node.innerHTML = `<div class="lx-actions"><strong>${number(result.total)} linhas no resultado completo</strong><span class="lx-status-pill">Valores exatos · ${result.columns.length} colunas</span><span class="lx-status-pill">${result.filterCount ? `${result.filterCount} filtros capturados` : "Todos os eventos visíveis"}</span></div><div class="lx-results"><table class="lx-table"><thead><tr>${canOpen ? "<th>Origem</th>" : ""}${result.columns.map((c) => `<th>${h(c.name)}<small>${h(c.dataType)}</small></th>`).join("")}</tr></thead><tbody>${result.rows.map((row, i) => `<tr>${canOpen ? `<td><button type="button" class="btn ghost small" data-sql-event="${i}">Abrir</button></td>` : ""}${row.map((cell) => `<td data-null="${cell === null}">${cell === null ? "NULL" : h(cell)}</td>`).join("")}</tr>`).join("")}</tbody></table></div><div class="lx-pagination">${pageButton("← Anterior", "prev", !result.offset)}<span>${number(result.rows.length ? result.offset + 1 : 0)}–${number(result.offset + result.rows.length)} de ${number(result.total)}</span>${pageButton("Próxima →", "next", BigInt(result.offset + result.rows.length) >= BigInt(result.total))}</div><p class="lx-help">O resultado é um snapshot completo. Colunas dinâmicas de logs são texto; use TRY_CAST para números e JSON. timestamp usa milissegundos UTC. Resultados agregados só abrem uma origem quando preservam id e event_ref válidos.</p>`;
    node.querySelectorAll("[data-lx-page]").forEach(
      (b) =>
        (b.onclick = () =>
          void (async () => {
            sqlResult = await call(
              {
                action: "query_page",
                result: result.result,
                offset: Math.max(
                  0,
                  result.offset + (b.dataset.lxPage === "prev" ? -80 : 80),
                ),
                limit: 80,
              },
              { latest: "local-sql-page" },
            );
            renderSqlResult();
          })().catch((e) => notice(String(e), true))),
    );
    node.querySelectorAll("[data-sql-event]").forEach(
      (b) =>
        (b.onclick = () => {
          const row = result.rows[+b.dataset.sqlEvent],
            id = Number(row[idIndex]);
          if (Number.isSafeInteger(id) && id >= 0 && row[refIndex])
            void openDetail(id, { eventRef: row[refIndex] });
          else
            notice(
              "A linha não preservou uma identidade de evento válida.",
              true,
            );
        }),
    );
  }
  function sqlPage() {
    root.innerHTML = `<section class="lx"><div class="lx-notice" id="lx-notice" role="status" hidden></div><div class="lx-examples">${examples.map(([label], i) => `<button type="button" data-example="${i}">${h(label)}</button>`).join("")}</div><div class="lx-editor"><label class="lx-field">Consulta em events · Ctrl+Enter executa · Ctrl+Espaço completa o campo<textarea id="lx-sql" class="lx-code" spellcheck="false" aria-label="Consulta SQL">${h(prefs.sql)}</textarea><select id="lx-completion" aria-label="Sugestões de campos" hidden></select></label><aside><div class="lx-actions">${button("lx-schema-load", "Atualizar campos", "fa-list")}</div><div class="lx-schema" id="lx-schema"></div></aside></div><div class="lx-toolbar">${button("lx-sql-run", "Executar", "fa-play", "primary")}${button("lx-sql-stop", "Cancelar", "fa-stop")}<label class="lx-inline"><input type="checkbox" id="lx-sql-filters">Aplicar filtros atuais do Explorar</label><span class="lx-grow"></span>${button("lx-sql-csv", "Exportar tudo · CSV")}${button("lx-sql-json", "Exportar tudo · JSONL")}</div><div class="lx-save-row"><label class="lx-field">Nome da consulta<input id="lx-query-name" maxlength="120" placeholder="Ex.: processos por usuário"></label>${button("lx-query-save", "Salvar no Caso", "fa-bookmark")}</div><div id="lx-history" class="lx-history"></div><div id="lx-sql-result"></div><p class="lx-help">SELECT, CTEs, agregações e janelas. Somente a visão events e as CTEs da consulta são acessíveis; funções de arquivo, extensões e execução externa são recusadas. CSV preserva a representação textual; JSONL inclui valores exatos e tipos por coluna.</p></section>`;
    q("#lx-sql").oninput = () => {
      prefs.sql = q("#lx-sql").value;
    };
    q("#lx-sql").onblur = keep;
    q("#lx-sql").onkeydown = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        q("#lx-sql-run").click();
      }
      if (e.ctrlKey && e.code === "Space") {
        e.preventDefault();
        const select = q("#lx-completion"),
          editor = q("#lx-sql"),
          prefix =
            editor.value
              .slice(0, editor.selectionStart)
              .match(/[\w.]+$/)?.[0] || "";
        select.innerHTML = schema
          .filter((c) => c.name.toLowerCase().startsWith(prefix.toLowerCase()))
          .map(
            (c) =>
              `<option value="${h(c.name)}">${h(c.name)} · ${h(c.dataType)}</option>`,
          )
          .join("");
        select.hidden = false;
        select.focus();
        select.onchange = () => {
          editor.setRangeText(
            `"${select.value.replaceAll('"', '""')}"`,
            editor.selectionStart - prefix.length,
            editor.selectionEnd,
            "end",
          );
          select.hidden = true;
          editor.focus();
          prefs.sql = editor.value;
        };
      }
    };
    root
      .querySelectorAll("[data-example]")
      .forEach(
        (b) => (b.onclick = () => setSql(examples[+b.dataset.example][1])),
      );
    pending("lx-schema-load", fetchSchema);
    pending("lx-sql-run", executeSql, "Executando…");
    bind("lx-sql-stop", () => {
      Tasks.cancelLatest("local-sql");
      notice(
        "Cancelamento solicitado. O resultado anterior permanece disponível.",
      );
    });
    bind("lx-query-save", () => {
      const name = q("#lx-query-name").value.trim();
      if (!name) {
        notice("Dê um nome à consulta.");
        return;
      }
      const item = { name, sql: q("#lx-sql").value },
        at = prefs.saved.findIndex((s) => s.name === name);
      if (at >= 0) prefs.saved[at] = item;
      else if (prefs.saved.length < 50) prefs.saved.push(item);
      else {
        notice("O limite é de 50 consultas salvas.");
        return;
      }
      keep();
      historyControls();
      notice("Consulta salva neste Caso.");
    });
    async function exportSql(format) {
      if (!sqlResult) {
        notice("Execute uma consulta antes de exportar.");
        return;
      }
      const path = await dialogApi.save({
        defaultPath: `consulta.${format}`,
        filters: [{ name: format.toUpperCase(), extensions: [format] }],
      });
      if (path) {
        const rows = await call(
          { action: "query_export", result: sqlResult.result, path, format },
          { latest: "local-export" },
        );
        notice(`${number(rows)} linhas exportadas.`);
      }
    }
    pending("lx-sql-csv", () => exportSql("csv"));
    pending("lx-sql-json", () => exportSql("jsonl"));
    renderSchema();
    renderSqlResult();
    historyControls();
  }
  const relationLabels = {
    explicit_guid: "Pai por GUID",
    inferred_pid_lifetime: "Pai inferido · PID + vida + boot",
    missing_guid_parent: "Pai GUID ausente",
    ambiguous_guid: "Pai GUID ambíguo",
    missing_pid_parent: "Pai PID ausente",
    ambiguous_pid: "Pai PID ambíguo",
    unknown_boot: "Boot desconhecido · vínculo não inferido",
    root: "Raiz",
    cycle_or_depth_limit: "Ciclo / limite de profundidade",
  };
  function nodeHtml(node) {
    return `<div class="lx-tree-node" data-node="${h(node.key)}"><div class="lx-tree-row"><button type="button" data-expand="${h(node.key)}" aria-label="Expandir filhos" aria-expanded="false" ${!node.children ? "disabled" : ""}>${node.children ? "▸" : "·"}</button><button type="button" class="lx-node-label" data-process="${h(node.key)}"><strong>${h(node.image || "Processo sem imagem")} <span class="lx-badge">PID ${h(node.pid || "?")}</span></strong><small>${h(node.host)} · ${h(node.user)} · ${node.started != null ? h(new Date(node.started).toLocaleString("pt-BR")) : "horário ausente"}</small><span class="lx-badge" data-inferred="${node.relation !== "explicit_guid" && node.relation !== "root"}">${h(relationLabels[node.relation] || node.relation)}</span><span class="lx-badge">${number(node.events)} eventos · ${number(node.children)} filhos</span></button></div><div class="lx-tree-children" hidden></div></div>`;
  }
  const graphNodes = new Map();
  function bindTree(container) {
    container
      .querySelectorAll("[data-process]")
      .forEach(
        (b) =>
          (b.onclick = () =>
            void processDetail(b.dataset.process).catch((e) =>
              notice(String(e), true),
            )),
      );
    container.querySelectorAll("[data-expand]").forEach(
      (b) =>
        (b.onclick = () =>
          void (async () => {
            const branch = b
              .closest(".lx-tree-node")
              .querySelector(":scope > .lx-tree-children");
            if (!branch.hidden) {
              branch.hidden = true;
              b.setAttribute("aria-expanded", "false");
              b.textContent = "▸";
              return;
            }
            branch.hidden = false;
            b.setAttribute("aria-expanded", "true");
            b.textContent = "▾";
            if (!branch.childElementCount)
              await children(b.dataset.expand, branch, 0);
          })().catch((e) => notice(String(e), true))),
    );
  }
  async function children(parent, container, offset) {
    const response = await call(
      {
        action: "process_page",
        graph: graph.graph,
        parent,
        search: q("#lx-process-search").value,
        offset,
        selected: null,
      },
      { latest: `local-branch-${parent}` },
    );
    response.nodes.forEach((n) => graphNodes.set(n.key, n));
    container.querySelector("[data-more-children]")?.remove();
    container.insertAdjacentHTML(
      "beforeend",
      response.nodes.map(nodeHtml).join(""),
    );
    if (offset + response.nodes.length < response.total) {
      const b = document.createElement("button");
      b.className = "btn ghost small";
      b.dataset.moreChildren = "true";
      b.textContent = "Mais filhos…";
      b.onclick = () =>
        void children(parent, container, offset + 80).catch((e) =>
          notice(String(e), true),
        );
      container.append(b);
    }
    bindTree(container);
  }
  async function processDetail(key, offset = 0) {
    const response = await call(
      {
        action: "process_page",
        graph: graph.graph,
        parent: null,
        search: "",
        offset,
        selected: key,
      },
      { latest: "local-process-detail" },
    );
    const n = response.ancestors[0] || graphNodes.get(key);
    if (!n) return;
    q("#lx-process-detail").innerHTML =
      `<h2>${h(n.image || "Processo")}</h2><p class="lx-help">${h(relationLabels[n.relation] || n.relation)}${n.candidates > 1 ? ` · ${n.candidates} candidatos` : ""}</p><dl>${[
        ["Máquina", n.host],
        ["Boot", n.boot || "desconhecido"],
        ["GUID", n.guid || "ausente"],
        ["PID", n.pid],
        ["Usuário", n.user],
        [
          "Início",
          n.started == null ? "ausente" : new Date(n.started).toISOString(),
        ],
        [
          "Fim / próximo uso",
          n.ended == null ? "não observado" : new Date(n.ended).toISOString(),
        ],
        ["Comando", n.command || "não informado"],
      ]
        .map(([k, v]) => `<dt>${h(k)}</dt><dd>${h(v)}</dd>`)
        .join("")}</dl><h2>Ancestrais observados</h2><p class="lx-help">${
        response.ancestors
          .slice(1)
          .map((a) => h(a.image || a.pid))
          .join(" ← ") || "Nenhum vínculo confirmado neste conjunto."
      }</p><h2>Eventos associados</h2>${response.evidence.map((e, i) => `<button type="button" class="lx-hit" data-process-event="${i}"><small>${e.timestamp == null ? "Sem horário" : h(new Date(e.timestamp).toLocaleString("pt-BR"))} · ${e.kind === 1 ? "criação" : e.kind === 2 ? "término" : "atividade"}</small>#${number(e.id)} · ${h(e.event_ref)}</button>`).join("")}<div class="lx-pagination">${pageButton("← Eventos", "prev", offset === 0)}<span>${number(offset + response.evidence.length)} de ${number(n.events)}</span>${pageButton("Eventos →", "next", offset + response.evidence.length >= n.events)}</div>`;
    q("#lx-process-detail")
      .querySelectorAll("[data-process-event]")
      .forEach(
        (b) =>
          (b.onclick = () => {
            const event = response.evidence[+b.dataset.processEvent];
            void openDetail(event.id, { eventRef: event.event_ref });
          }),
      );
    q("#lx-process-detail")
      .querySelectorAll("[data-lx-page]")
      .forEach(
        (b) =>
          (b.onclick = () =>
            void processDetail(
              key,
              Math.max(0, offset + (b.dataset.lxPage === "prev" ? -80 : 80)),
            ).catch((e) => notice(String(e), true))),
      );
  }
  async function graphPage(offset = 0) {
    graph = await call(
      {
        action: "process_page",
        graph: graph.graph,
        parent: null,
        search: q("#lx-process-search").value,
        offset,
        selected: null,
      },
      { latest: "local-process-page" },
    );
    renderGraph();
  }
  function renderGraph() {
    const node = q("#lx-process-tree");
    if (!node) return;
    if (!graph) {
      node.innerHTML =
        '<div class="lx-empty">Construa a árvore para consultar processos, filhos e evidências.</div>';
      return;
    }
    graph.nodes.forEach((n) => graphNodes.set(n.key, n));
    node.innerHTML =
      graph.nodes.map(nodeHtml).join("") ||
      '<div class="lx-empty">Nenhum processo com identidade e máquina suficientes foi encontrado.</div>';
    bindTree(node);
    q("#lx-process-pages").innerHTML =
      `${pageButton("← Raízes", "prev", !graph.offset)}<span>${number(graph.total)} raízes · ${number(graph.skipped)} registros sem identidade suficiente</span>${pageButton("Raízes →", "next", graph.offset + graph.nodes.length >= graph.total)}`;
    q("#lx-process-pages")
      .querySelectorAll("button")
      .forEach(
        (b) =>
          (b.onclick = () =>
            void graphPage(
              Math.max(
                0,
                graph.offset + (b.dataset.lxPage === "prev" ? -80 : 80),
              ),
            ).catch((e) => notice(String(e), true))),
      );
  }
  function processesPage() {
    root.innerHTML = `<section class="lx"><div class="lx-notice" id="lx-notice" role="status" hidden></div><div class="lx-toolbar">${button("lx-process-build", "Construir árvore", "fa-code-branch", "primary")}<label class="lx-inline"><input type="checkbox" id="lx-process-filters">Destacar recorte atual, preservando ancestrais</label><label class="lx-grow">Buscar imagem, comando, usuário, PID ou GUID<input id="lx-process-search" placeholder="Ex.: powershell.exe"></label>${button("lx-process-find", "Buscar")}${button("lx-process-stop", "Cancelar")}</div><p class="lx-help">GUID tem preferência. PID exige máquina, horário de criação, intervalo de vida e boot conhecido. Security 4688 usa NewProcessId; o PID do provedor que escreveu o evento não identifica o processo descrito. Relações inferidas aparecem com borda tracejada.</p><div class="lx-tree-layout"><section class="lx-pane"><header><h2>Instâncias de processos</h2></header><div class="lx-tree" id="lx-process-tree"></div><div id="lx-process-pages" class="lx-pagination"></div></section><section class="lx-pane"><header><h2>Processo e evidências</h2></header><div class="lx-process-detail" id="lx-process-detail"><p class="lx-help">Selecione um processo para ver comando, usuário, duração, ancestrais e eventos originais.</p></div></section></div><p class="lx-help">Sysmon e Windows Security são reconhecidos por provedor e canal. Campos normalizados norm.process.* também são aceitos. Sem boot informado ou marcador de inicialização observado, pais por PID permanecem sem inferência. Pais fora do conjunto visível são indicados como ausentes.</p></section>`;
    pending(
      "lx-process-build",
      async () => {
        graphNodes.clear();
        graph = await call(
          { action: "processes" },
          {
            latest: "local-process-build",
            filters: q("#lx-process-filters").checked,
          },
        );
        renderGraph();
        notice("Árvore construída sobre os eventos visíveis.");
      },
      "Construindo…",
    );
    pending("lx-process-find", () => (graph ? graphPage() : Promise.resolve()));
    bind("lx-process-stop", () => {
      Tasks.cancelLatest("local-process-build");
      notice("Cancelamento solicitado.");
    });
    q("#lx-process-search").onkeydown = (e) => {
      if (e.key === "Enter") q("#lx-process-find").click();
    };
    renderGraph();
  }
  async function mount(container, next) {
    root = container;
    page = next;
    generation++;
    const current = stamp();
    if (ownerKey && ownerKey !== current) resetRuntime();
    ownerKey = current;
    if (next === "sql") sqlPage();
    else if (next === "processes") processesPage();
  }
  function unmount() {
    root = null;
    page = null;
    generation++;
  }
  document.addEventListener("analysis-context-change", () => {
    if (ownerKey && ownerKey !== stamp()) {
      resetRuntime();
      ownerKey = stamp();
      if (root && page) void mount(root, page);
    }
  });
  return {
    pages,
    mount,
    unmount,
    rules: () => prefs.rules,
    extraction: () => prefs.extraction,
    keep,
    addRule() {
      if (prefs.rules.length >= 16) return false;
      prefs.rules.push({ id: `rule-${Date.now()}`, label: "Novo destaque", color: "#b7a2f7", enabled: false, pattern: { engine: "literal", text: "", caseSensitive: false } });
      return true;
    },
    expression: () => prefs.expression,
    setExpression(draft) {
      prefs.expression = draft && draft.pattern.length <= 16384 ? { pattern: draft.pattern, engine: draft.engine, field: draft.field, caseSensitive: !!draft.caseSensitive, multiline: !!draft.multiline, dotAll: !!draft.dotAll, scope: draft.scope === "line" ? "line" : "event" } : null;
      clearTimeout(expressionTimer);
      expressionTimer = setTimeout(keep, 800);
    },
    capture,
    restore,
    sanitize,
    paint,
  };
})();
