/* Mock de window.__TAURI__ para pré-visualizar o frontend no navegador.
   Gera um dataset sintético em memória e implementa os comandos usados
   pela tela de exploração do artefato. Uso: servido por serve.mjs. */
(() => {
  "use strict";
  // marcador ANTES do app rodar: só pula o auto-load se a store já existia ao abrir a página
  const hadStore = !!localStorage.getItem("__mockStore");

  // ---------------------------------------------------------------- dataset
  const LEVELS = ["Informação", "Informação", "Informação", "Aviso", "Erro", "Crítico", "Depuração"];
  const SOURCES = ["API", "Worker", "Auth", "Scheduler"];
  const CODES = {
    API: ["200", "200", "200", "301", "404", "500"],
    Worker: ["1000", "1001", "1002", "1005"],
    Auth: ["4624", "4625", "4634", "4768"],
    Scheduler: ["100", "107", "140"],
  };
  const NAMES = {
    200: "Requisição atendida", 301: "Redirecionamento", 404: "Recurso não encontrado",
    500: "Erro interno", 1000: "Job iniciado", 1001: "Job concluído", 1002: "Job falhou",
    1005: "Fila cheia", 4624: "Logon bem-sucedido", 4625: "Falha de logon",
    4634: "Logoff", 4768: "TGT solicitado", 100: "Tarefa agendada", 107: "Tarefa disparada",
    140: "Tarefa excedeu tempo",
  };
  const USUARIOS = ["ana.souza", "bruno.lima", "carla.mendes", "diego.reis", "elisa.paz", "fabio.nunes", "gabi.rocha"];
  const IPS = ["10.0.0.12", "10.0.0.34", "172.16.8.4", "192.168.1.20", "10.0.3.99", "172.16.9.9"];
  const MESSAGES = [
    "Processando requisição do cliente", "Falha ao abrir conexão com o banco",
    "Cache invalidado com sucesso", "Timeout aguardando resposta do upstream",
    "Usuário autenticado", "Tentativa de acesso negada", "Sincronização concluída",
    "Fila de mensagens processada", "Certificado próximo do vencimento",
    "Retry agendado para a operação",
  ];

  const rnd = (() => { let s = 42; return () => (s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff; })();
  const pick = (arr) => arr[Math.floor(rnd() * arr.length)];

  const now = Date.now();
  const events = [];
  for (let i = 0; i < 4000; i++) {
    const source = pick(SOURCES);
    const code = pick(CODES[source]);
    const ts = now - Math.floor(rnd() * 24 * 3600 * 1000);
    const sizeMB = Math.floor(Math.exp(rnd() * 6)); // ~1..400 MB
    const lat = Math.floor(rnd() * 2500);
    const msg = pick(MESSAGES);
    events.push({
      id: i,
      timestamp: ts,
      source,
      level: pick(LEVELS),
      code,
      name: NAMES[code] || "",
      description: "",
      message: `${msg} (usuário ${pick(USUARIOS)}, ${sizeMB} MB em ${lat} ms)`,
      raw: "",
      fields: {
        usuario: pick(USUARIOS),
        ip_cliente: pick(IPS),
        status: String(code.length === 3 ? code : pick(["200", "301", "404", "500", "503"])),
        tamanho: `${sizeMB} MB`,
        latencia: `${lat} ms`,
        ativo: rnd() > 0.3 ? "true" : "false",
        ambiente: "prod",
        anotacao: rnd() > 0.75 ? "revisar" : "",
        request_id: "req-" + (100000 + i),
      },
    });
  }
  events.sort((a, b) => b.timestamp - a.timestamp);

  let COLUMNS = ["timestamp", "source", "level", "code", "name", "description", "message",
    "usuario", "ip_cliente", "status", "tamanho", "latencia", "ativo", "ambiente", "anotacao", "request_id"];
  const loadedParts = ["mock.jsonl (preview)"];
  const derivedFields = [];
  const mockCalls = {};
  window.__mockCalls = mockCalls;
  let merged = false;

  // lote de uma segunda fonte para a opção "Unir"
  function appendFirewallBatch() {
    const base = events.length;
    for (let i = 0; i < 2000; i++) {
      const allow = rnd() > 0.25;
      const ts = now - Math.floor(rnd() * 24 * 3600 * 1000);
      events.push({
        id: base + i,
        timestamp: ts,
        source: "Firewall",
        level: allow ? "Informação" : "Erro",
        code: allow ? "ALLOW" : "DENY",
        name: allow ? "Conexão permitida" : "Conexão bloqueada",
        description: "",
        message: `${allow ? "Permitido" : "Bloqueado"} tráfego para ${pick(IPS)} (${Math.floor(rnd() * 900)} KB)`,
        raw: "",
        fields: {
          ip_cliente: pick(IPS),
          regra: pick(["web-out", "dns", "vpn"]),
          tamanho: `${Math.floor(rnd() * 900)} KB`,
        },
      });
    }
    events.sort((a, b) => b.timestamp - a.timestamp);
    COLUMNS = [...COLUMNS, "regra"];
    loadedParts.push("firewall.log (preview)");
    merged = true;
  }

  // ---------------------------------------------------------------- helpers
  const colStr = (ev, col) => {
    if (col === "id") return String(ev.id);
    if (col === "timestamp") return ev.timestamp == null ? "" : new Date(ev.timestamp).toISOString();
    if (col in ev && typeof ev[col] === "string") return ev[col];
    const v = ev.fields?.[col];
    return v === undefined || v === null ? "" : String(v);
  };
  const parseNumUnit = (s) => {
    const m = String(s).trim().toLowerCase().match(/^(-?[\d.,]+)\s*(tb|gb|mb|kb|b|gbps|mbps|kbps|bps|ms|s|min|h|%)?$/);
    if (!m) return null;
    let n = parseFloat(m[1].replace(",", "."));
    if (Number.isNaN(n)) return null;
    switch (m[2]) {
      case "kb": n *= 1024; break;
      case "mb": n *= 1024 ** 2; break;
      case "gb": n *= 1024 ** 3; break;
      case "tb": n *= 1024 ** 4; break;
      case "kbps": n *= 1e3; break;
      case "mbps": n *= 1e6; break;
      case "gbps": n *= 1e9; break;
      case "s": n *= 1000; break;
      case "min": n *= 60000; break;
      case "h": n *= 3600000; break;
    }
    return n;
  };
  const colNum = (ev, col) => {
    if (col === "timestamp") return ev.timestamp;
    if (col === "id") return ev.id;
    return parseNumUnit(colStr(ev, col));
  };
  const valNum = (col, s) => {
    const t = String(s).trim();
    // número puro só se a string inteira for numérica (parseFloat é leniente demais)
    if (/^-?\d+(\.\d+)?$/.test(t)) return parseFloat(t);
    if (col === "timestamp") { const d = Date.parse(t); return Number.isNaN(d) ? null : d; }
    return parseNumUnit(t);
  };

  function matchFilter(ev, f) {
    const hay = f.column === "_all" ? ev.message : colStr(ev, f.column);
    const v = f.value ?? "";
    switch (f.op) {
      case "contains": return hay.toLowerCase().includes(v.toLowerCase());
      case "not_contains": return !hay.toLowerCase().includes(v.toLowerCase());
      case "equals": return hay.toLowerCase() === String(v).trim().toLowerCase();
      case "not_equals": return hay.toLowerCase() !== String(v).trim().toLowerCase();
      case "starts_with": return hay.toLowerCase().startsWith(v.toLowerCase());
      case "empty": return !hay.trim();
      case "not_empty": return !!hay.trim();
      case "gt": case "gte": case "lt": case "lte": {
        const a = colNum(ev, f.column), b = valNum(f.column, v);
        if (a == null || b == null) return false;
        return f.op === "gt" ? a > b : f.op === "gte" ? a >= b : f.op === "lt" ? a < b : a <= b;
      }
      case "between": {
        const a = colNum(ev, f.column), lo = valNum(f.column, v), hi = valNum(f.column, f.value2 ?? "");
        return a != null && lo != null && hi != null && a >= lo && a <= hi;
      }
      default: return true;
    }
  }
  const applyFilters = (filters, pool = events) => pool.filter((ev) => (filters || []).every((f) => matchFilter(ev, f)));
  const poolOf = (caseEvents) => (Array.isArray(caseEvents) ? caseEvents : events);

  const countBy = (rows, col) => {
    const m = new Map();
    for (const ev of rows) { const k = colStr(ev, col) || "(vazio)"; m.set(k, (m.get(k) || 0) + 1); }
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  };

  function makeStats(rows) {
    const tss = rows.map((e) => e.timestamp).filter((t) => t != null);
    if (!tss.length) return { buckets: [], bucket_ms: 0, levels: [] };
    const tmin = Math.min(...tss), tmax = Math.max(...tss);
    const bucketMs = Math.max(1, Math.ceil((tmax - tmin + 1) / 60));
    const buckets = new Map();
    for (const t of tss) {
      const b = tmin + Math.floor((t - tmin) / bucketMs) * bucketMs;
      buckets.set(b, (buckets.get(b) || 0) + 1);
    }
    return {
      buckets: [...buckets.entries()].sort((a, b) => a[0] - b[0]),
      bucket_ms: bucketMs,
      levels: countBy(rows, "level"),
    };
  }

  const aggRows = (rows, col) => ({
    columns: [col, "n"],
    rows: countBy(rows, col).map(([k, n]) => ({ [col]: k, n })),
  });

  const isIp = (s) => /^\d{1,3}(\.\d{1,3}){3}$/.test(s) || (/^[\da-f:]+$/i.test(s) && s.includes(":"));
  const isBool = (s) => ["true", "false", "0", "1", "sim", "não", "nao", "yes", "no"].includes(s.toLowerCase());

  function profileFields(rows) {
    const profiles = [];
    for (const col of COLUMNS) {
      const counts = new Map();
      let total = 0, numeric = 0, bools = 0, ips = 0, min = null, max = null, empty = 0;
      for (const ev of rows) {
        const v = colStr(ev, col).trim();
        if (!v) { empty++; continue; }
        total++;
        counts.set(v, (counts.get(v) || 0) + 1);
        if (isBool(v)) bools++;
        if (isIp(v)) ips++;
        const n = parseNumUnit(v);
        if (n != null) {
          numeric++;
          min = min == null ? n : Math.min(min, n);
          max = max == null ? n : Math.max(max, n);
        }
      }
      const cardinality = counts.size;
      const ratio = total ? numeric / total : 0;
      let kind;
      if (col === "timestamp") kind = "time";
      else if (total && bools / total >= 0.9) kind = "bool";
      else if (total && ips / total >= 0.85) kind = "ip";
      else if (ratio >= 0.85) kind = "number";
      else if (cardinality <= 25) kind = "category";
      else if (total && cardinality / total >= 0.95) kind = "id";
      else kind = "text";
      // refinamento de unidade
      if (kind === "number") {
        const sample = [...counts.keys()][0] || "";
        if (/[kmgt]b$/i.test(sample)) kind = "bytes";
        else if (/bps$/i.test(sample)) kind = "bits";
        else if (/\b(ms|s|min|h)$/i.test(sample)) kind = "duration";
        else if (/%$/.test(sample)) kind = "percent";
      }
      const top = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 10);
      profiles.push({
        name: col, kind, cardinality, empty,
        numeric_ratio: Math.round(ratio * 100) / 100,
        min, max, top,
      });
    }
    return profiles;
  }

  // ---------------------------------------------------------------- comandos
  const handlers = {
    list_channels: () => ["Application", "System", "Security"],
    list_formats: () => [{ id: "auto", name: "Automático (inferir)" }, { id: "wildfly", name: "WildFly" }],
    list_derived_fields: () => derivedFields.map((f) => ({ ...f })),
    delete_derived_field: ({ name }) => {
      const i = derivedFields.findIndex((f) => f.name === name);
      if (i >= 0) derivedFields.splice(i, 1);
      for (const ev of events) delete ev.fields[name];
      COLUMNS = COLUMNS.filter((c) => c !== name);
      return null;
    },
    save_derived_field: ({ name, source, rules }) => {
      derivedFields.push({ name, source, rules });
      if (!COLUMNS.includes(name)) COLUMNS = [...COLUMNS, name];
      const expand = (t, m) => t.replace(/\$(\d+)/g, (_, gi) => m[Number(gi)] ?? "");
      for (const ev of events) {
        const val = colStr(ev, source);
        if (!val) continue;
        for (const rule of rules || []) {
          if (rule.filter && !matchFilter(ev, rule.filter)) continue;
          const m = new RegExp(rule.pattern).exec(val);
          if (!m) continue;
          const out = rule.template ? expand(rule.template, m) : (m[1] ?? m[0]);
          if (out) { ev.fields[name] = out; break; } // primeira regra que pega (OU)
        }
      }
      return null;
    },
    cases_save: ({ data }) => { localStorage.setItem("__mockStore", JSON.stringify(data)); return null; },
    cases_load: () => {
      const saved = localStorage.getItem("__mockStore");
      if (saved) return JSON.parse(saved);
      return {
        active: "case-demo",
        cases: [{
          id: "case-demo", name: "Caso Demo", createdAt: now,
          items: [
          {
            id: "it-1", name: "Eventos de autenticação", label: "Eventos de autenticação", kind: "events", createdAt: now,
            rows: (() => {
              const auth = events.filter((e) => e.source === "Auth").slice(0, 400);
              // rajada: 4 "Falha de logon" seguidas, 1 min de intervalo
              const t0 = now - 3 * 3600e3;
              for (let k = 0; k < 4; k++) {
                auth.push({
                  ...auth[0],
                  id: 90000 + k,
                  timestamp: t0 + k * 60e3,
                  code: "4625", name: "Falha de logon",
                  message: "Tentativa de acesso negada (rajada)",
                });
              }
              return auth;
            })(),
            sourceFilters: [], tags: ["auth"], note: "", relevance: "normal",
          },
          {
            id: "it-2", name: "Falhas da API", label: "Falhas da API", kind: "events", createdAt: now,
            rows: events.filter((e) => e.source === "API" && ["Erro", "Crítico"].includes(e.level)).slice(0, 250),
            sourceFilters: [], tags: [], note: "", relevance: "importante",
          },
        ],
        manual: [{
          id: "m1", name: "Janela de ataque", createdAt: now,
          start: now - 5 * 3600e3, end: now - 2.5 * 3600e3,
          description: "Período de tentativas suspeitas",
        }],
        stations: [], artifacts: [],
        workspace: { view: "source", analysisView: "overview" },
      }],
      };
    },
    get_codes: () => "{}",
    get_codes_path: () => "C:\\mock\\codes.json",
    save_codes: () => null,
    system_codes_count: () => 0,
    harvest_codes: () => ({ added: 0 }),
    test_ts_config: ({ config }) => {
      const srcVal = (ev, s) => s === "linha" ? (ev.raw || ev.message)
        : s === "timestamp" ? (ev.timestamp != null ? new Date(ev.timestamp).toISOString() : "")
        : colStr(ev, s);
      return events.slice(0, 5).map((ev) => {
        const joined = (config.sources || []).map((s) => srcVal(ev, s)).join(" ");
        const rules = config.rules?.length ? config.rules : [{ regex: config.regex, template: config.template }];
        const expand = (t, m) => t.replace(/\$(\d+)/g, (_, gi) => m[Number(gi)] ?? "");
        for (const rule of rules) {
          if (!rule.regex) { if (joined.trim()) return [joined, joined.trim()]; continue; }
          const m = new RegExp(rule.regex).exec(joined);
          if (!m) continue; // OU: próxima regra
          const candidate = rule.template ? expand(rule.template, m) : (m.length >= 3 ? `${m[1]} ${m[2]}` : (m[1] ?? m[0]));
          if (candidate) return [joined, candidate];
        }
        return [joined, "(não reconhecido)"];
      });
    },
    get_ts_config: () => null,
    set_ts_config: async () => {
      for (let done = 0; done <= 6000; done += 1500) {
        emitMock("operation-progress", {
          operation: "data/hora", phase: "Recalculando timestamps",
          completed: done, total: 6000, unit: "linhas", cancellable: false,
        });
        await delay(300);
      }
      return null;
    },
    clear_events: () => { events.length = 0; COLUMNS = []; return null; },
    source_summary: () => ({
      count: events.length,
      columns: events.length ? [...COLUMNS] : [],
      source_desc: events.length ? loadedParts.join(" + ") : "",
      source_names: events.length ? [...loadedParts] : [],
    }),
    mcp_status: () => ({
      enabled: true,
      running: true,
      port: 39117,
      url: "http://127.0.0.1:39117/mcp",
      config_path: "C:\\mock\\LogInsight\\mcp.json",
      tools: [
        { name: "load_file", description: "Carrega um arquivo de log como fonte atual (muta estado)" },
        { name: "load_files", description: "Carrega vários arquivos unidos em uma única fonte (muta estado)" },
        { name: "load_event_log", description: "Carrega eventos de um canal do Event Log do Windows (muta estado)" },
        { name: "list_channels", description: "Lista os canais disponíveis do Event Log do Windows" },
        { name: "clear_events", description: "Descarta a fonte de eventos carregada (muta estado)" },
        { name: "source_summary", description: "Resumo da fonte carregada: contagem, colunas e descrição" },
        { name: "query_events", description: "Consulta eventos com filtros, ordenação e paginação" },
        { name: "event_detail", description: "Retorna um evento completo pelo id (inclui a linha bruta)" },
        { name: "explore_snapshot", description: "Recorte do explorador: linhas, histograma e facetas em uma chamada" },
        { name: "aggregate_events", description: "Agrega eventos por coluna (count, sum, avg, min, max, ...)" },
        { name: "trail_events", description: "Trilha temporal em torno de um evento (N antes, N depois)" },
        { name: "count_filtered", description: "Conta quantos eventos passam nos filtros" },
        { name: "tree_aggs", description: "Contagens por valor de várias colunas (árvore de exploração)" },
        { name: "stats_events", description: "Histograma temporal e distribuição por nível" },
        { name: "profile_fields", description: "Perfil estatístico dos campos do recorte filtrado" },
        { name: "compute_series", description: "Séries para gráficos: temporal ou ranking de termos" },
        { name: "pivot", description: "Tabela dinâmica (pivô OLAP) sobre o recorte filtrado" },
        { name: "list_formats", description: "Lista os formatos de log disponíveis (ids para load_file)" },
        { name: "save_custom_format", description: "Cria/atualiza um formato customizado (muta estado)" },
        { name: "test_parse", description: "Testa um formato customizado contra linhas de exemplo" },
        { name: "get_ts_config", description: "Retorna a config de data/hora salva para um arquivo" },
        { name: "set_ts_config", description: "Grava/remove config de data/hora e reaplica na fonte (muta estado)" },
        { name: "test_ts_config", description: "Testa uma config de data/hora nos primeiros eventos" },
        { name: "list_derived_fields", description: "Lista os campos derivados configurados" },
        { name: "save_derived_field", description: "Cria/atualiza um campo derivado por regex (muta estado)" },
        { name: "delete_derived_field", description: "Remove um campo derivado (muta estado)" },
        { name: "get_codes", description: "Retorna o catálogo de códigos do usuário" },
        { name: "get_codes_path", description: "Caminho do arquivo codes.json em disco" },
        { name: "save_codes", description: "Substitui o catálogo de códigos e re-enriquece eventos (muta estado)" },
        { name: "harvest_codes", description: "Reextrai o catálogo de eventos do sistema operacional (muta estado)" },
        { name: "system_codes_count", description: "Quantidade de códigos no catálogo extraído do sistema" },
        { name: "cases_load", description: "Carrega os casos de análise persistidos" },
        { name: "cases_save", description: "Persiste os casos de análise (muta estado)" },
      ],
    }),
    load_file: ({ merge } = {}) => {
      if (merge && !merged) appendFirewallBatch();
      return {
        count: events.length,
        columns: COLUMNS,
        source_desc: loadedParts.join(" + "),
        warnings: [],
      };
    },
    load_files: ({ paths, merge } = {}) => {
      if ((paths?.length > 1 || merge) && !merged) appendFirewallBatch();
      return handlers.load_file();
    },
    load_event_log: () => handlers.load_file(),
    event_detail: ({ id }) => events.find((e) => e.id === id) || null,
    query_events: ({ filters, offset, limit }) => {
      const rows = applyFilters(filters);
      return { total: rows.length, rows: rows.slice(offset, offset + limit) };
    },
    stats_events: ({ filters }) => makeStats(applyFilters(filters)),
    explore_snapshot: ({ filters, offset, limit }) => {
      const rows = applyFilters(filters);
      return {
        query: { total: rows.length, rows: rows.slice(offset, offset + limit) },
        stats: makeStats(rows),
        sources: aggRows(rows, "source"),
        codes: aggRows(rows, "code"),
      };
    },
    trail_events: ({ centerId, before, after, filters, caseEvents }) => {
      const pool = applyFilters(filters, poolOf(caseEvents))
        .slice().sort((a, b) => (a.timestamp - b.timestamp) || (a.id - b.id));
      let pos = pool.findIndex((e) => e.id === centerId);
      if (pos < 0) pos = pool.length - 1;
      const start = Math.max(0, pos - before);
      const end = Math.min(pool.length, pos + after + 1);
      return { events: pool.slice(start, end), before_available: start, after_available: pool.length - end };
    },
    count_filtered: ({ filters, caseEvents }) => applyFilters(filters, poolOf(caseEvents)).length,
    tree_aggs: ({ columns, filters, caseEvents }) => {
      mockCalls.tree_aggs = (mockCalls.tree_aggs || 0) + 1;
      const pool = poolOf(caseEvents);
      return (columns || []).map((col) => {
        const rows = applyFilters((filters || []).filter((f) => f.column !== col), pool);
        return [col, aggRows(rows, col)];
      });
    },
    aggregate_events: ({ groupColumn, aggs, filters, caseEvents }) => {
      mockCalls.aggregate_events = (mockCalls.aggregate_events || 0) + 1;
      const rows = applyFilters(filters, poolOf(caseEvents));
      const base = aggRows(rows, groupColumn);
      const metrics = (aggs || []).filter((a) => a.metric && a.metric !== "count");
      base.rows.forEach((r) => {
        r.n = r.n;
        for (const a of metrics) {
          const vals = rows.filter((ev) => (colStr(ev, groupColumn) || "(vazio)") === r[groupColumn])
            .map((ev) => colNum(ev, a.field)).filter((n) => n != null);
          const key = `${a.metric}_${a.field}`;
          r[key] = !vals.length ? null
            : a.metric === "sum" ? vals.reduce((x, y) => x + y, 0)
            : a.metric === "avg" ? vals.reduce((x, y) => x + y, 0) / vals.length
            : a.metric === "min" ? Math.min(...vals)
            : a.metric === "max" ? Math.max(...vals)
            : null;
        }
      });
      return base;
    },
    profile_fields: ({ filters, caseEvents }) => profileFields(applyFilters(filters, poolOf(caseEvents))),
    compute_series: ({ filters, caseEvents, spec }) => {
      const rows = applyFilters(filters, poolOf(caseEvents));
      if (spec.chart === "terms") {
        const counts = countBy(rows, spec.field).slice(0, spec.limit || 12);
        return { kind: "terms", unit: null, x: counts.map(([k]) => k), series: [{ name: spec.field, points: counts.map(([, n]) => n) }] };
      }
      const tss = rows.map((e) => e.timestamp).filter((t) => t != null);
      if (!tss.length) return { kind: "time", unit: null, x: [], series: [] };
      const tmin = Math.min(...tss), tmax = Math.max(...tss);
      const span = Math.max(1, tmax - tmin);
      const n = 40, bucket = Math.ceil(span / n);
      const vals = new Array(n).fill(0);
      for (const t of tss) vals[Math.min(n - 1, Math.floor((t - tmin) / bucket))]++;
      return { kind: "time", unit: null, x: vals.map((_, i) => tmin + i * bucket), series: [{ name: "eventos", points: vals }] };
    },
    pivot: () => ({ columns: [], rows: [], cells: [], totals: [], truncated: false }),
  };

  const delay = (ms) => new Promise((r) => setTimeout(r, ms));
  const listeners = {};
  const emitMock = (name, payload) => (listeners[name] || []).forEach((cb) => cb({ payload }));
  // dispara o evento de mudança de estado via MCP (teste do live-refresh):
  // window.__mockMcpEmit("source" | "cases" | "codes" | "derived" | "ts_config" | "formats")
  window.__mockMcpEmit = (kind) => {
    if (kind === "source" && !merged) appendFirewallBatch(); // simula outra fonte carregada via MCP
    emitMock("mcp-state-changed", { kind });
  };
  // simula a indexação de um arquivo com progresso granular
  async function simulateLoad(label, total) {
    for (let done = 0; done < total; done += 900) {
      emitMock("operation-progress", {
        operation: "carregamento", phase: `Indexando ${label}`,
        completed: done, total, unit: "linhas", cancellable: false,
      });
      await delay(240);
    }
    emitMock("operation-progress", {
      operation: "carregamento", phase: "Concluído",
      completed: total, total, unit: "linhas", cancellable: false,
    });
  }
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args = {}) => {
        const h = handlers[cmd];
        if (!h) return Promise.reject(`mock: comando não implementado: ${cmd}`);
        try {
          // latência artificial para visualizar os estados de carregamento
          if (["load_file", "load_files", "load_event_log"].includes(cmd)) await simulateLoad("mock.jsonl", 6300);
          if (["explore_snapshot", "aggregate_events", "profile_fields"].includes(cmd)) await delay(350);
          return h(args);
        } catch (e) {
          return Promise.reject(String(e));
        }
      },
    },
    event: {
      listen: (name, cb) => {
        (listeners[name] = listeners[name] || []).push(cb);
        return Promise.resolve(() => {});
      },
    },
    dialog: {
      open: (opts = {}) => Promise.resolve(opts.multiple
        ? ["C:\\mock\\mock.jsonl", "C:\\mock\\firewall.log"]
        : "C:\\mock\\mock.jsonl"),
    },
  };

  // auto-carrega um caso + artefato para o preview abrir direto na tela de exploração
  window.addEventListener("load", () => {
    // sem clique extra: o app deve navegar sozinho para a listagem após carregar
    const load = () => document.querySelector("#btn-load")?.click();
    setTimeout(() => {
      // reabertura com estado salvo: não auto-carrega (simula reabrir o app)
      if (hadStore) return;
      const sel = document.querySelector("#case-select");
      const hasCase = sel && [...sel.options].some((o) => o.value);
      if (hasCase) { setTimeout(load, 250); return; }
      document.querySelector("#btn-new-case")?.click();
      setTimeout(() => {
        const input = document.querySelector("#case-name-input");
        if (input) {
          input.value = "Caso Preview";
          input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
        }
        setTimeout(load, 250);
      }, 250);
    }, 300);
  });
})();
