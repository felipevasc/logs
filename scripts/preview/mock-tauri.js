// Legacy browser fixtures deliberately retain their legacy mock contract.
// The startup acceptance fixture selects native transport before production bootstrap.
if (window.CaseEvidence && !window.__mockNativeCaseBootstrapEnabled) window.CaseEvidence.active = false;
/* Mock de window.__TAURI__ para pré-visualizar o frontend no navegador.
   Gera um dataset sintético em memória e implementa os comandos usados
   pela tela de exploração do artefato. Uso: servido por serve.mjs. */
(() => {
  "use strict";
  // marcador ANTES do app rodar: só pula o auto-load se a store já existia ao abrir a página
  const hadStore = !!window.__mockNativeCaseBootstrapEnabled || !!localStorage.getItem("__mockStore");

  // Synthetic restart-only resource preferences. These values do not measure
  // browser RAM or native engines; reloading the preview simulates app restart.
  const resourceDefault = { schemaVersion: 1, mode: "automatic", memoryLimitMib: null, parallelismLimit: null };
  const resourceRead = () => {
    try { return { ...resourceDefault, ...JSON.parse(localStorage.getItem("__mockResourceSettings")) }; }
    catch { return resourceDefault; }
  };
  const resourceStartup = structuredClone(resourceRead());
  const resourceBudget = resourceStartup.mode === "custom" ? resourceStartup.memoryLimitMib : 2730;
  const resourceStatus = () => {
    const saved = resourceRead();
    return {
      active: { memoryAvailableMib: 8192, memoryBudgetMib: resourceBudget, duckdbPerInstanceMib: Math.floor(resourceBudget / 4),
        textIndexMib: Math.floor(resourceBudget / 8), selectionCacheMib: Math.floor(resourceBudget / 8),
        globalParallelism: Math.min(resourceStartup.parallelismLimit ?? 7, 8), maximumParallelism: 8,
        parserThreads: 4, queryThreadsPerSession: 3, textThreads: 1, environmentOverrideMib: null,
        invalidEnvironmentOverride: false, conservativeBuilder: resourceBudget < 2730 },
      activePreferences: resourceStartup, saved, minimumMemoryMib: 128, maximumMemoryMib: 4096, maximumParallelism: 8,
      restartRequired: saved.mode !== resourceStartup.mode || saved.memoryLimitMib !== resourceStartup.memoryLimitMib || saved.parallelismLimit !== resourceStartup.parallelismLimit,
      startupWarning: null, savedWarning: null,
    };
  };

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
      event_ref: `preview:${i}`,
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
  // Deterministic evidence for contextual analysis; preserve all source counts.
  // Eight contexts normally have distinct outcomes. Only Checkout + Sul + Web
  // changes in a middle window, so broad marginal comparisons hide the signal.
  for (let i=0;i<576;i++) {
    const event=events[i], bin=Math.floor(i/48), context=Math.floor(i/6)%8, repetition=i%6;
    const operation=context&1?'Checkout':'Consulta', region=context&2?'Sul':'Norte', channel=context&4?'Web':'App';
    const incident=bin===5&&context===7;
    event.timestamp=now-24*3600*1000+bin*2*3600*1000+repetition*120000;
    Object.assign(event.fields,{operacao:operation,regiao:region,canal:channel,latencia:incident?'8900 ms':'120 ms'});
    Object.assign(event,{event_ref:`preview:${event.id}`,code:incident?'UPSTREAM_TIMEOUT':`NORMAL_${context}`,level:incident?'Erro':'Informação',name:incident?'Timeout no checkout':'Fluxo concluído',message:incident?`Operação ${operation} ${region} ${channel} interrompida por timeout`:`Operação ${operation} ${region} ${channel} concluída`});
  }
  // A separate, global burst makes the Changes view independently useful.
  for(let i=576;i<776;i++) Object.assign(events[i],{timestamp:now-8*3600*1000+(i-576)*20000,event_ref:`preview:${events[i].id}`,level:'Aviso',code:'QUEUE_RETRY',name:'Reprocessamento da fila',message:'Reprocessamento extraordinário de fila após reconexão'});
  window.__mockJourneySeed?.(events);
  events.sort((a, b) => b.timestamp - a.timestamp);

  let COLUMNS = ["timestamp", "source", "level", "code", "name", "description", "message",
    "usuario", "ip_cliente", "status", "tamanho", "latencia", "ativo", "ambiente", "anotacao", "request_id", "correlation_id", "operacao", "regiao", "canal"];
  COLUMNS.push("mock_payload_b64");
  for (const [index, event] of events.slice(0, 8).entries()) event.fields.mock_payload_b64 = btoa(JSON.stringify({ user: "preview-user", attempt: index, allowed: index % 2 === 0 }));
  if (window.__mockRareDerivedFieldsEnabled) {
    const event = events.at(-1);
    event.fields.mock_payload_b64 = btoa(JSON.stringify({ rare: { flag: true, latency: 42 } }));
    window.__mockRareDerivedFixture = { id: event.id, eventRef: event.event_ref };
  }
  // Opt-in browser evidence only. These structures are generated by the real
  // Rust parser, supplied by the scenario; ordinary preview records stay as-is.
  const javaFixtureDetails = new Map();
  if (window.__mockJavaFixturesEnabled && Array.isArray(window.__mockJavaFixtures)) {
    window.__mockJavaFixtureRows = [];
    for (const [index, fixture] of window.__mockJavaFixtures.slice(0, 4).entries()) {
      const event = events[index];
      event.raw = fixture.raw; event.message = fixture.raw.split(/\r?\n/, 1)[0];
      event.name = "Exceção Java de teste"; event.level = "Erro";
      Object.assign(event.fields, structuredClone(fixture.fields));
      javaFixtureDetails.set(event.event_ref, { raw: fixture.raw, trace: fixture.detail.trace });
      COLUMNS.push(...Object.keys(fixture.fields).filter(field => !COLUMNS.includes(field)));
      window.__mockJavaFixtureRows.push({ name: fixture.name, id: event.id, eventRef: event.event_ref });
    }
  }
  // Opt-in transport fixture only: JSON numbers and integer-like object keys
  // deliberately lose their native spelling/order when decoded by JavaScript.
  // Keep the authoritative strings separately, just as the native command does.
  const canonicalFixture = new Map();
  if (window.__mockCanonicalFieldsEnabled) {
    const event = events[0];
    event.event_ref ||= `preview:${event.id}`;
    const values = {
      native_float: "1.0",
      native_object: '{"10":"ten","2":"two"}',
      native_u64: "18446744073709551615",
      native_null: "null",
    };
    for (const [column, text] of Object.entries(values)) {
      event.fields[column] = JSON.parse(text);
      canonicalFixture.set(`${event.event_ref}\n${column}`, text);
    }
    // A native string keeps all line endings. Textarea display normalization
    // must not silently replace its CRLF and lone CR while editing a filter.
    values.native_multiline = "alpha\r\nbravo\ncharlie\rdelta";
    event.fields.native_multiline = values.native_multiline;
    canonicalFixture.set(`${event.event_ref}\nnative_multiline`, values.native_multiline);
    COLUMNS.push(...Object.keys(values), "native_missing");
    window.__mockCanonicalFixture = { id: event.id, eventRef: event.event_ref, values };
  }
  const baseSourceEvents = structuredClone(events), baseSourceColumns = [...COLUMNS];
  const loadedParts = ["mock.jsonl (preview)"];
  const derivedFields = [];
  const mockCalls = {};
  window.__mockCalls = mockCalls;
  let merged = false;
  let sourceGeneration = 0, sourceOperationId = null, sourceAnalysisContext = null;
  let sourceInputs = [{ kind: "file", paths: ["C:\\mock\\mock.jsonl"], format: "auto" }];

  // lote de uma segunda fonte para a opção "Unir"
  function appendFirewallBatch() {
    const base = events.length;
    for (let i = 0; i < 2000; i++) {
      const allow = rnd() > 0.25;
      const ts = now - Math.floor(rnd() * 24 * 3600 * 1000);
      events.push({
        id: base + i,
        event_ref: `preview:${base + i}`,
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
    if (col.startsWith("@") && window.QueryLang) return window.QueryLang.fieldValue(ev, window.QueryLang.resolve(col)) ?? "";
    if (col === "event_ref") return ev.event_ref || `preview:${ev.id}`;
    if (col === "id") return String(ev.id);
    if (col === "timestamp") return ev.timestamp == null ? "" : new Date(ev.timestamp).toISOString().replace(/\.000Z$/, "+00:00").replace(/Z$/, "+00:00");
    if (col in ev && typeof ev[col] === "string") return ev[col];
    const v = ev.fields?.[col];
    return v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v);
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
    if (window.QueryLang && !window.QueryLang.detectionMatch) { window.QueryLang.detectionMatch = mockDetectionMatch; window.QueryLang.derive = mockDerive; }
    if (f.op === "detection") return mockDetectionMatch(ev, f.value);
    if (f.op === "in_exact") return new Set(String(f.value || "").split("\n")).has(colStr(ev,f.column));
    const language = window.QueryLang?.matchFilter(ev, f);
    if (language !== undefined) return language;
    const hay = f.column === "_all" ? `${ev.message||''}\n${ev.raw||''}` : colStr(ev, f.column);
    const v = f.value ?? "";
    switch (f.op) {
      case "pattern": return patternOf(ev.message) === v;
      case "regex": return new RegExp(v).test(hay);
      case "contains": return hay.toLowerCase().includes(v.toLowerCase());
      case "not_contains": return !hay.toLowerCase().includes(v.toLowerCase());
      case "equals": return hay.toLowerCase() === String(v).trim().toLowerCase();
      case "not_equals": return hay.toLowerCase() !== String(v).trim().toLowerCase();
      case "equals_exact": return f.column !== '_all' && (Object.hasOwn(ev,f.column) && ev[f.column] != null || Object.hasOwn(ev.fields || {}, f.column)) && hay === String(v);
      case "not_equals_exact": return f.column !== '_all' && (!(Object.hasOwn(ev,f.column) && ev[f.column] != null || Object.hasOwn(ev.fields || {}, f.column)) || hay !== String(v));
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
  const sortedRows=(rows,column,direction)=>!column?rows:rows.slice().sort((a,b)=>{
    const x=colNum(a,column),y=colNum(b,column);
    const result=x!=null&&y!=null?x-y:colStr(a,column).toLowerCase().localeCompare(colStr(b,column).toLowerCase());
    return direction==='desc'?-result:result;
  });

  const countBy = (rows, col) => {
    const m = new Map();
    for (const ev of rows) { const k = colStr(ev, col) || "(vazio)"; m.set(k, (m.get(k) || 0) + 1); }
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  };

  function temporalFor(rows) {
    const dated=rows.filter(e=>Number.isFinite(e.timestamp));
    const result={behavior_shifts:[],changes:[],timed_sample_count:dated.length,time_bins:0,temporal_limited:false};
    if(dated.length<24)return result;
    const min=Math.min(...dated.map(e=>e.timestamp)),max=Math.max(...dated.map(e=>e.timestamp)),width=Math.max(1,Math.ceil((max-min+1)/12)),bins=Math.floor((max-min)/width)+1;
    result.time_bins=bins;if(bins<2)return result;
    const contextual=['operacao','regiao','canal'],groups=new Map([['[]',{context:[],rows:dated}]]);
    for(let mask=1;mask<8;mask++)for(const event of dated) {
      const context=contextual.filter((_,i)=>mask&(1<<i)).map(field=>({field,value:colStr(event,field)}));
      if(context.some(item=>!item.value))continue;
      const key=JSON.stringify(context);if(!groups.has(key))groups.set(key,{context,rows:[]});groups.get(key).rows.push(event);
    }
    const candidates=[];
    for(const group of groups.values()) {
      if(group.rows.length<24)continue;
      for(const field of ['message','code','level']) {
        const value=e=>field==='message'?patternOf(e.message):colStr(e,field),all=new Map(),windows=Array.from({length:bins},()=>new Map()),windowRows=Array.from({length:bins},()=>[]);
        for(const event of group.rows) {const v=value(event);if(!v)continue;const bin=Math.min(bins-1,Math.floor((event.timestamp-min)/width));all.set(v,(all.get(v)||0)+1);windows[bin].set(v,(windows[bin].get(v)||0)+1);windowRows[bin].push(event);}
        const total=[...all.values()].reduce((a,b)=>a+b,0);
        windows.forEach((window,bin)=>{
          const window_count=windowRows[bin].length,baseline_count=total-window_count;if(window_count<3||baseline_count<20)return;
          const [expected,baseline_expected]=[...all].map(([v,n])=>[v,n-(window.get(v)||0)]).sort((a,b)=>b[1]-a[1]||a[0].localeCompare(b[0]))[0];
          const expected_share=baseline_expected/baseline_count;if(group.context.length&&expected_share<.8)return;
          for(const [observed,window_observed]of window){
            if(observed===expected||window_observed<3)continue;
            const baseline_observed=all.get(observed)-window_observed,observed_share=window_observed/window_count,baseline_observed_share=baseline_observed/baseline_count,delta=observed_share-baseline_observed_share;
            const fresh=!group.context.length&&field==='message'&&baseline_observed===0&&observed_share>=.15;if(delta<.35&&!fresh)continue;
            const example=windowRows[bin].find(e=>value(e)===observed);
            candidates.push({kind:group.context.length?'behavior_shift':fresh?'new_pattern':'distribution_shift',context:group.context,outcome_field:field,outcome_op:field==='message'?'pattern':'equals_exact',expected,observed,baseline_count,baseline_expected,baseline_observed,window_count,window_expected:window.get(expected)||0,window_observed,expected_share,observed_share,baseline_observed_share,delta,start:min+bin*width,end:Math.min(max,min+(bin+1)*width-1),score:delta*Math.sqrt(window_observed)*(group.context.length?expected_share:1),event_id:example.id,event_ref:example.event_ref||`preview:${example.id}`});
          }
        });
      }
    }
    candidates.sort((a,b)=>b.score-a.score||a.context.length-b.context.length||['message','code','level'].indexOf(a.outcome_field)-['message','code','level'].indexOf(b.outcome_field));
    const subset=(a,b)=>a.every(x=>b.some(y=>x.field===y.field&&x.value===y.value));
    for(const finding of candidates){
      const target=finding.context.length?result.behavior_shifts:result.changes;
      if(target.length===8||target.some(old=>old.start<=finding.end+1&&finding.start<=old.end+1&&(subset(old.context,finding.context)||subset(finding.context,old.context))&&((old.outcome_field===finding.outcome_field&&old.observed===finding.observed)||(old.event_ref===finding.event_ref&&old.window_observed===finding.window_observed&&old.baseline_observed===finding.baseline_observed))))continue;
      target.push(finding);
    }
    return result;
  }

  function makeStats(rows) {
    const tss = rows.map((e) => e.timestamp).filter((t) => t != null);
    if (!tss.length) return { buckets: [], bucket_ms: 0, levels: countBy(rows,"level") };
    const tmin = tss.reduce((a,b)=>Math.min(a,b),Infinity), tmax = tss.reduce((a,b)=>Math.max(a,b),-Infinity);
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

  const aggRows = (rows, col) => window.__mockAggregate(rows, col, [{ func: "count", column: "*", alias: "n" }]);

  const isIp = (s) => /^\d{1,3}(\.\d{1,3}){3}$/.test(s) || (/^[\da-f:]+$/i.test(s) && s.includes(":"));
  const isBool = (s) => ["true", "false", "0", "1", "sim", "não", "nao", "yes", "no"].includes(s.toLowerCase());

  function profileFields(rows) {
    if (window.__mockRareDerivedFieldsEnabled) rows = rows.slice(0, 3000);
    const profiles = [];
    const columns = new Set(COLUMNS); for (const row of rows) for (const key of Object.keys(row.fields || {})) columns.add(key);
    for (const col of columns) {
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
  const patternOf = text => String(text).split("\n")[0].replace(/\b[0-9a-f]{8}-[0-9a-f-]{27,}\b|\b(?:\d{1,3}\.){3}\d{1,3}\b|\b0x[0-9a-f]+\b|\b\d+(?:[.,]\d+)?\b/gi, "‹…›").slice(0,400);
  function overviewFor(rows) {
    const times=rows.map(r=>r.timestamp).filter(t=>t!=null), start=times.length?times.reduce((a,b)=>Math.min(a,b),Infinity):0,end=times.length?times.reduce((a,b)=>Math.max(a,b),-Infinity):0,width=Math.max(1000,Math.floor((end-start)/90)+1);
    const buckets=Array.from({length:times.length?Math.floor((end-start)/width)+1:0},(_,i)=>({timestamp:start+i*width,count:0,errors:0}));
    const patterns=new Map();let errors=0,warnings=0;
    for(const e of rows){const err=["Erro","Crítico"].includes(e.level);errors+=err;warnings+=e.level==="Aviso";if(e.timestamp!=null){const b=buckets[Math.floor((e.timestamp-start)/width)];b.count++;b.errors+=err;}
      const key=patternOf(e.message),p=patterns.get(key)||{pattern:key,count:0,errors:0,first:null,last:null,example:e};p.count++;p.errors+=err;if(e.timestamp!=null){p.first=p.first==null?e.timestamp:Math.min(p.first,e.timestamp);p.last=p.last==null?e.timestamp:Math.max(p.last,e.timestamp);}patterns.set(key,p);}
    const list=[...patterns.values()].sort((a,b)=>b.count-a.count);
    const failure=list.find(p=>p.errors);
    return {total:rows.length,errors,warnings,undated:rows.length-times.length,start:times.length?start:null,end:times.length?end:null,buckets,levels:Object.fromEntries(countBy(rows,"level")),sources:countBy(rows,"source"),patterns:list.slice(0,80),patterns_limited:false,complete:true,latency:null,
      findings:failure?[{kind:"pattern",title:"Falha recorrente",detail:`${failure.errors} ocorrências · ${failure.pattern}`,start:failure.first,end:failure.last,event_id:failure.example.id}]:[]};
  }
  let mcpEnabled = true;
  const updateState = { phase: "idle", checkOnStart: true, skippedVersion: null, installOnClose: false, downloaded: 0, total: null, lastCheck: null };
  let updateSnapshotRevision = 0n;
  const updateStatus = () => {
    const next = window.__mockUpdate, announced = next && updateState.phase !== "idle";
    return {
      snapshotRevision: String(++updateSnapshotRevision),
      currentVersion: "0.5.1", installKind: "nsis", unavailable: null, blocker: null, needsAdmin: false,
      checkOnStart: updateState.checkOnStart, skippedVersion: updateState.skippedVersion, lastCheck: updateState.lastCheck,
      phase: updateState.phase, available: announced ? { version: next.version, notes: next.notes || null, date: "2026-09-28T12:00:00Z" } : null,
      downloaded: updateState.downloaded, total: updateState.total, error: null, installOnClose: updateState.installOnClose,
      notice: null, releasesUrl: "https://github.com/felipevasc/logs/releases",
    };
  };
  const updatePublish = () => { const status = updateStatus(); emitMock("update-state", status); return status; };
  const updateCheck = () => {
    const next = window.__mockUpdate;
    updateState.phase = next ? "available" : "idle";
    updateState.lastCheck = { at: new Date().toISOString(), outcome: next ? "available" : "current", message: next ? `A versão ${next.version} está disponível.` : "Você está usando a versão mais recente." };
    return updatePublish();
  };
  const handlers = {
    mcp_configure: ({ enabled }) => { mcpEnabled = enabled; return handlers.mcp_status(); },
    validate_filters: ({filters}) => { for(const f of filters||[]) { if(f.op==="regex") new RegExp(f.value); if(f.op==="query") { const problem = window.QueryLang?.validate(f.value); if (problem) throw new Error(problem); } } return null; },
    cancel_operation: () => { window.__mockGeneration = (window.__mockGeneration || 0) + 1; window.__mockRemoteCancel?.(); return null; },
    remote_list: args => window.__mockRemote('remote_list',args),
    remote_save: args => window.__mockRemote('remote_save',args),
    remote_delete: args => window.__mockRemote('remote_delete',args),
    remote_test: args => window.__mockRemote('remote_test',args),
    remote_import: args => window.__mockRemote('remote_import',args),
    journey_fields: args => window.__mockJourneys('journey_fields', args, events, applyFilters),
    journey_index: args => window.__mockJourneys('journey_index', args, events, applyFilters),
    journey_events: args => window.__mockJourneys('journey_events', args, events, applyFilters),
    threat_catalog: async () => (await import('/__mock-threats__.js')).threatCatalog(),
    threat_catalog_update: async () => (await import('/__mock-threats__.js')).threatCatalogUpdate(),
    threat_scan: async args => (await import('/__mock-threats__.js')).threatScan(applyFilters((args.filters || []).filter(f => f.op !== 'threat_rule'), poolOf(args.caseEvents)), args.filters),
    threat_events: async args => (await import('/__mock-threats__.js')).threatEvents(applyFilters((args.filters || []).filter(f => f.op !== 'threat_rule'), poolOf(args.caseEvents)), args),
    expand_paths: ({paths}) => paths,
    export_events: ({filters,caseEvents}) => applyFilters(filters,poolOf(caseEvents)).length,
    export_document: () => null,
    case_image_add: async args => (await import('/__mock-case-images__.js')).addImage(args),
    case_image_read: async args => (await import('/__mock-case-images__.js')).readImage(args),
    export_investigation: async args => (await import('/__mock-case-images__.js')).exportInvestigation(args),
    import_investigation: async args => (await import('/__mock-case-images__.js')).importInvestigation(args),
    export_timeline: async ({format, filename, base64}) => {
      const state = window.__timelineExportMock ||= { files: [], cancel: false, download: true };
      if (state.cancel) { state.cancel = false; return { saved: false }; }
      const bytes = Uint8Array.from(atob(base64), character => character.charCodeAt(0));
      state.files.push({ format, filename, base64, bytes: bytes.length });
      if (state.download) {
        const response = await fetch("/__timeline-downloads__/", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ format, filename, base64 }) });
        const download = await response.json();
        if (!response.ok) throw new Error(download.error || "Não foi possível preparar o download.");
        const link = document.createElement("a"); link.href = download.url; link.download = download.filename; link.hidden = true;
        document.body.append(link); link.click(); setTimeout(() => link.remove(), 1000);
      }
      return { saved: true, path: filename, bytes: bytes.length };
    },
    dataset_overview: ({filters,caseEvents}) => overviewFor(applyFilters(filters,poolOf(caseEvents))),
    timeline_range: ({filters,start,end,bucketCount,caseEvents}) => {
      if (start > end) throw new Error("O início deve ser anterior ao fim.");
      let count=Math.max(1,Math.min(240,bucketCount||120));
      const bucketMs=Math.max(1,Math.ceil((end-start+1)/count));
      count=Math.min(count,Math.floor((end-start)/bucketMs)+1);
      const buckets=Array.from({length:count},(_,i)=>({timestamp:start+i*bucketMs,count:0,errors:0,warnings:0}));
      let total=0,errors=0,warnings=0;
      for(const e of applyFilters(filters,poolOf(caseEvents))){
        if(e.timestamp==null||e.timestamp<start||e.timestamp>end)continue;
        const bucket=buckets[Math.min(count-1,Math.floor((e.timestamp-start)/bucketMs))];
        const error=["Erro","Crítico"].includes(e.level),warning=e.level==="Aviso";
        bucket.count++;bucket.errors+=error;bucket.warnings+=warning;total++;errors+=error;warnings+=warning;
      }
      return {start,end,bucketMs,total,errors,warnings,buckets};
    },
    list_sources: ({caseEvents} = {}) => [{id:"mock-app",name:"application.jsonl",path:"C:\\mock\\mock.jsonl",format:"jsonl",bytes:2400000,count:poolOf(caseEvents).length,undated:0,start:now-86400000,end:now,sampled:200,unparsed:0}],
    compare_periods: ({filters,before,after,caseEvents}) => {
      if(before.start>before.end||after.start>after.end)throw new Error('Revise os intervalos de comparação.');
      if(before.start<=after.end&&after.start<=before.end)throw new Error('Os períodos não podem se sobrepor.');
      const rows=applyFilters(filters,poolOf(caseEvents)),a=rows.filter(e=>e.timestamp!=null&&e.timestamp>=before.start&&e.timestamp<=before.end),b=rows.filter(e=>e.timestamp!=null&&e.timestamp>=after.start&&e.timestamp<=after.end);
      const groups=new Map();for(const [which,list] of [["before",a],["after",b]])for(const e of list){const k=patternOf(e.message),g=groups.get(k)||{pattern:k,before:0,after:0,example:e};g[which]++;groups.set(k,g);}
      const changes=[...groups.values()].map(g=>({...g,before_rate:g.before/Math.max(1,a.length),after_rate:g.after/Math.max(1,b.length),delta:g.after/Math.max(1,b.length)-g.before/Math.max(1,a.length)})).sort((a,b)=>Math.abs(b.delta)-Math.abs(a.delta));
      return {before_total:a.length,after_total:b.length,before_errors:a.filter(e=>["Erro","Crítico"].includes(e.level)).length,after_errors:b.filter(e=>["Erro","Crítico"].includes(e.level)).length,changes:changes.slice(0,100),limited:false};
    },

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
                  event_ref: `preview:${90000 + k}`,
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
    set_ts_config: async ({ operationId } = {}) => {
      for (let done = 0; done <= 6000; done += 1500) {
        emitMock("operation-progress", {
          operationId, phaseId: "timestamp", operation: "data/hora", phase: "Recalculando timestamps",
          completed: done, total: 6000, unit: "linhas", cancellable: false,
        });
        await delay(300);
        if (window.__mockCancelledIds?.has(operationId)) throw Error("Operação cancelada.");
      }
      return null;
    },
    clear_events: () => { events.length = 0; COLUMNS = []; return null; },
    source_snapshot: () => ({
      generation: sourceGeneration, operationId: sourceOperationId, analysisContext: sourceAnalysisContext,
      count: events.length, columns: events.length ? [...COLUMNS] : [],
      sourceDesc: events.length ? loadedParts.join(" + ") : "",
      sourceNames: events.length ? [...loadedParts] : [], sources: events.length ? structuredClone(sourceInputs) : [],
    }),
    source_summary: ({caseEvents} = {}) => ({
      count: poolOf(caseEvents).length,
      columns: events.length ? [...COLUMNS] : [],
      source_desc: events.length ? loadedParts.join(" + ") : "",
      source_names: events.length ? [...loadedParts] : [],
    }),
    // The preview cannot zoom the browser; screenshots emulate a scale with the viewport.
    ui_zoom: () => false,
    resource_settings_status: () => resourceStatus(),
    resource_settings_save: ({ preferences }) => {
      const valid = window.ResourceSettings.preferences(preferences.mode, String(preferences.memoryLimitMib), 4096, preferences.parallelismLimit, 8);
      if (preferences.schemaVersion !== 1) throw Error("Versão de configuração não suportada.");
      localStorage.setItem("__mockResourceSettings", JSON.stringify(valid));
      return resourceStatus();
    },
    // Updates (updates.js): nothing is announced unless a test sets window.__mockUpdate = { version, notes },
    // so the dialog never covers other previews.
    update_status: () => updateStatus(),
    update_startup: () => {
      const notice = window.__mockUpdateNotice || null; window.__mockUpdateNotice = null;
      return { ...(updateState.checkOnStart ? updateCheck() : updateStatus()), notice };
    },
    update_check: () => updateCheck(),
    update_download: () => {
      Object.assign(updateState, { phase: "downloading", downloaded: 0, total: 42 * 1048576 });
      const timer = setInterval(() => {
        if (updateState.phase !== "downloading") return clearInterval(timer);
        updateState.downloaded = Math.min(updateState.total, updateState.downloaded + 7 * 1048576);
        if (updateState.downloaded >= updateState.total) { clearInterval(timer); updateState.phase = "ready"; }
        updatePublish();
      }, 150);
      return updatePublish();
    },
    update_cancel: () => { if (updateState.phase === "downloading") Object.assign(updateState, { phase: "available", downloaded: 0 }); return updatePublish(); },
    update_install: () => { updateState.phase = "installing"; updatePublish(); return null; },
    update_install_on_close: ({ enabled }) => { updateState.installOnClose = enabled; return updatePublish(); },
    update_set_check_on_start: ({ enabled }) => { updateState.checkOnStart = enabled; return updatePublish(); },
    update_skip: ({ version }) => { updateState.skippedVersion = version || null; return updatePublish(); },
    update_open_page: ({ version }) => { window.__mockOpenedRelease = version || "latest"; return null; },
    mcp_status: () => ({
      enabled: mcpEnabled,
      running: mcpEnabled,
      token: "preview-key",
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
        { name: "discover_patterns", description: "Descoberta local de padrões: templates, anomalias numéricas e desvios" },
        { name: "compare_periods", description: "Compara dois recortes temporais (before/after) e mudanças de padrões" },
        { name: "timeline_range", description: "Histograma, erros e limites temporais para a linha do tempo" },
        { name: "export_events", description: "Exporta recorte filtrado para JSONL ou CSV com máscara opcional" },
        { name: "expand_paths", description: "Expande pastas, curingas e arquivos compactados gzip" },
        { name: "threat_scan", description: "Varre eventos contra o catálogo de 378 regras locais de ameaças" },
        { name: "threat_events", description: "Lista eventos e evidências de ameaças com paginação" },
        { name: "threat_catalog", description: "Catálogo de regras de ameaças, severidades e categorias" },
        { name: "threat_catalog_update", description: "Recarrega catálogo de regras de ameaças do disco (muta estado)" },
        { name: "journey_fields", description: "Campos sugeridos para rastrear jornadas entre origens" },
        { name: "journey_index", description: "Indexa jornadas por identificador com duração e agregações" },
        { name: "journey_events", description: "Eventos cronológicos de uma jornada específica" },
        { name: "remote_list", description: "Lista conexões Elasticsearch e Kibana salvas" },
        { name: "remote_test", description: "Testa acesso e autenticação em conexão Elasticsearch ou Kibana" },
        { name: "remote_import", description: "Consulta e importa registros remotos para arquivo JSONL local" },
        { name: "triage", description: "Triagem de segurança: detecções, episódios, táticas ATT&CK e entidades de risco" },
        { name: "event_insights", description: "Entidades, ação/resultado, conteúdo decodificado e regras de um evento" },
        { name: "detection_rules", description: "Regras de detecção embutidas e Sigma importadas" },
      ],
    }),
    load_file: ({ merge } = {}) => {
      if (!events.length) { events.push(...structuredClone(baseSourceEvents)); COLUMNS = [...baseSourceColumns]; merged = false; loadedParts.splice(0, loadedParts.length, "mock.jsonl (preview)"); }
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
    load_bundle: ({ members }) => handlers.load_files({ paths: members.flatMap(s => s.paths || [s.path || s.channel]), merge: false }),
    event_detail: args => {
      validateFieldContentToken(args);
      const event = poolOf(args.caseEvents).find(row => row.id === args.id) || null;
      if (event && args.eventRef != null && event.event_ref !== args.eventRef) throw Error("O registro retornado não corresponde à referência solicitada.");
      return event;
    },
    java_trace_detail: args => {
      validateFieldContentToken(args);
      if (!args.analysisContext || !analysisFor(args)) throw Error("ANALYSIS_CONTEXT_CHANGED: Contexto de análise obrigatório.");
      if (!args.caseKey && args.sourceGeneration !== sourceGeneration) throw Error("SOURCE_GENERATION_CHANGED: Atualize a origem.");
      if (typeof args.eventRef !== "string" || !args.eventRef) throw Error("JAVA_TRACE_ADMISSION: Referência estável obrigatória.");
      const event = poolOf(args.caseEvents).find(row => row.id === args.id) || null;
      if (!event) return { state: "unavailable", trace: null, reason: "record_unavailable", row: null };
      if (event.event_ref !== args.eventRef) throw Error("O registro retornado não corresponde à referência solicitada.");
      const row = { id: event.id, eventRef: event.event_ref };
      if (!event.raw) return { state: "unavailable", trace: null, reason: "raw_unavailable", row };
      const fixture = javaFixtureDetails.get(event.event_ref);
      if (!fixture?.trace || fixture.raw !== event.raw) return { state: "unavailable", trace: null, reason: "not_java", row };
      return { state: "available", trace: structuredClone(fixture.trace), reason: null, row };
    },
    query_page: ({ filters, offset=0, limit=100, cursor, sortColumn='', sortDir='', caseEvents }) => {
      const rows = sortedRows(applyFilters(filters, poolOf(caseEvents)), sortColumn, sortDir);
      const start = cursor ? Number(cursor) : offset, size = Math.max(1, Math.min(2000, limit));
      return { rows: rows.slice(start, start + size).map(row => ({ ...row, raw: "" })), total: null, hasMore: start + size < rows.length, nextCursor: start + size < rows.length ? String(start + size) : null, engine: 'columnar', warning: null };
    },
    engine_status: () => window.__mockEngineStatus || { state: 'ready', baseReady: true, derivedReady: true, phase: 'Pronto', completedRows: poolOf().length, totalRows: poolOf().length, completedSegments: 1, totalSegments: 1, resumedRows: 0, canResume: false, error: null },
    engine_retry: () => { window.__mockEngineStatus = null; return null; },
    cancel_task: ({ operationId }) => { (window.__mockCancelledIds ||= new Set()).add(operationId); return true; },
    query_events: ({ filters, offset=0, limit=100, sortColumn='', sortDir='',caseEvents }) => {
      const rows = sortedRows(applyFilters(filters,poolOf(caseEvents)),sortColumn,sortDir);
      return { total: rows.length, rows: rows.slice(offset, offset + Math.max(1,Math.min(2000,limit))) };
    },
    stats_events: ({ filters,caseEvents }) => makeStats(applyFilters(filters,poolOf(caseEvents))),
    explore_snapshot: ({ filters, offset=0, limit=100,sortColumn='',sortDir='',caseEvents }) => {
      const rows = sortedRows(applyFilters(filters,poolOf(caseEvents)),sortColumn,sortDir);
      return {
        query: { total: rows.length, rows: rows.slice(offset, offset + Math.max(1,Math.min(2000,limit))) },
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
      if (!window.__mockAggregate) throw new Error("A prévia está desatualizada. Recarregue a página para carregar os cálculos.");
      return window.__mockAggregate(applyFilters(filters, poolOf(caseEvents)), groupColumn, aggs);
    },
    profile_fields: ({ filters, caseEvents }) => profileFields(applyFilters(filters, poolOf(caseEvents))),
    discover_patterns: ({filters,caseEvents}) => {
      const rows=applyFilters(filters,poolOf(caseEvents)), profiles=profileFields(rows), overview=overviewFor(rows);
      const categories=profiles.filter(p=>p.cardinality>1&&p.cardinality<=32&&!['message','timestamp','description','name'].includes(p.name)).slice(0,8).map(p=>{
        const counts=countBy(rows,p.name).filter(([v])=>v!=='(vazio)'),present=counts.reduce((a,[,n])=>a+n,0);
        return {field:p.name,present,distinct:counts.length,dominant:{value:counts[0][0],count:counts[0][1],share:counts[0][1]/present},rare:counts.filter(([,n])=>n/present<=.02).slice(0,5).map(([value,count])=>({value,count,share:count/present}))};
      });
      const associations=[];
      for(let i=0;i<categories.length;i++) for(let j=i+1;j<categories.length;j++) {
        const left=categories[i].field,right=categories[j].field,lc=new Map(countBy(rows,left)),rc=new Map(countBy(rows,right)),pairs=new Map();
        for(const e of rows){const a=colStr(e,left),b=colStr(e,right);if(a&&b){const key=JSON.stringify([a,b]);pairs.set(key,(pairs.get(key)||0)+1);}}
        for(const [key,count] of pairs) {const [a,b]=JSON.parse(key),lift=count*rows.length/(lc.get(a)*rc.get(b));if(count>=5&&lift>=1.5)associations.push({left:{field:left,value:a},right:{field:right,value:b},count,support:count/rows.length,confidence:count/lc.get(a),lift});}
      }
      associations.sort((a,b)=>b.lift-a.lift||b.count-a.count);
      const outliers=[];
      for(const p of profiles.filter(p=>['number','duration','bytes'].includes(p.kind)&&!['code','status','timestamp'].includes(p.name))) {
        const vals=rows.map(e=>({ev:e,v:colNum(e,p.name)})).filter(x=>x.v!=null).sort((a,b)=>a.v-b.v);if(vals.length<20)continue;
        const median=vals[Math.floor(vals.length/2)].v,ds=vals.map(x=>Math.abs(x.v-median)).sort((a,b)=>a-b),mad=ds[Math.floor(ds.length/2)],margin=Math.max(1,6*1.4826*mad),lower=median-margin,upper=median+margin,outs=vals.filter(x=>x.v<lower||x.v>upper);
        if(outs.length)outliers.push({field:p.name,unit:p.kind,count:vals.length,median,mad,lower,upper,outlier_count:outs.length,min:vals[0].v,max:vals.at(-1).v,examples:outs.slice(0,3).map(x=>({event_id:x.ev.id,value:x.v}))});
      }
      return {total:rows.length,sample_count:rows.length,limited:false,complete:true,fields_considered:categories.map(c=>c.field),errors:overview.errors,warnings:overview.warnings,missing_time:overview.undated,start:overview.start,end:overview.end,categories,associations:associations.slice(0,12),outliers,templates:overview.patterns.map(p=>({pattern:p.pattern,count:p.count,share:p.count/Math.max(1,rows.length),errors:p.errors,event_id:p.example.id})),...temporalFor(rows)};
    },
    compute_series: ({ filters, caseEvents, spec }) => {
      const rows = applyFilters(filters, poolOf(caseEvents));
      if (spec.chart === "terms") {
        const grouped=window.__mockAggregate(rows,spec.field,[{func:'count',column:'*',alias:'n'}]);
        const counts=grouped.rows.map((row,index)=>({label:row[spec.field],raw:grouped.group_values[index],count:row.n})).sort((a,b)=>b.count-a.count).slice(0,spec.limit||12);
        return { kind: "terms", unit: null, x: counts.map(v=>v.label), x_values:counts.map(v=>v.raw), series: [{ name: spec.field, points: counts.map(v=>v.count) }] };
      }
      const tss = rows.map((e) => e.timestamp).filter((t) => t != null);
      if (!tss.length) return { kind: "time", unit: null, x: [], series: [] };
      const tmin = Math.min(...tss), tmax = Math.max(...tss);
      const bucket=spec.interval_ms || Math.max(1,Math.ceil((tmax-tmin+1)/40)), n=Math.floor((tmax-tmin)/bucket)+1;
      const names=spec.split?countBy(rows,spec.split).slice(0,spec.limit||10).map(([v])=>v):['registros'];
      const groups=new Map(names.map(name=>[name,Array.from({length:n},()=>[])]));
      for(const e of rows){if(e.timestamp==null)continue;const name=spec.split?(colStr(e,spec.split)||'(vazio)'):'registros',cells=groups.get(name);if(!cells)continue;const value=spec.metric==='count'?1:colNum(e,spec.field);if(value!=null)cells[Math.min(n-1,Math.floor((e.timestamp-tmin)/bucket))].push(value);}
      const calc=values=>!values.length?0:spec.metric==='avg'?values.reduce((a,b)=>a+b,0)/values.length:spec.metric==='max'?Math.max(...values):spec.metric==='min'?Math.min(...values):values.reduce((a,b)=>a+b,0);
      const profile=profileFields(rows).find(p=>p.name===spec.field);
      return {kind:'time',unit:profile?.kind||'number',interval_ms:bucket,x:Array.from({length:n},(_,i)=>tmin+i*bucket),series:names.map(name=>({name,points:groups.get(name).map(calc),samples:groups.get(name).map(v=>v.length)}))};
    },
    pivot: ({filters,caseEvents,spec}) => window.__mockPivot(applyFilters(filters,poolOf(caseEvents)),spec),
  };

  // ---------------------------------------------------------------- security (preview)
  const caseStore = new Map(); let casePublication = 0;
  const defaultDetectionSettings = () => ({ disabled: [], suppress: [], threats: true, mappings: [], coverage: [] });
  const MOCK_RULES = [
    { id: "auth.bruteforce.source", name: "Força bruta de senha", severity: "medium", kind: "threshold", attack: [{ id: "T1110.001", name: "Adivinhação de senha", tactics: ["credential-access"] }], description: "Muitas falhas de autenticação da mesma origem." },
    { id: "auth.bruteforce.success", name: "Acesso após força bruta", severity: "high", kind: "sequence", attack: [{ id: "T1110", name: "Força bruta", tactics: ["credential-access"] }, { id: "T1078", name: "Contas válidas", tactics: ["initial-access", "persistence"] }], description: "Falhas seguidas de acesso bem-sucedido da mesma origem." },
    { id: "evasion.log-clear", name: "Registro de auditoria apagado ou desativado", severity: "high", kind: "single", attack: [{ id: "T1070.001", name: "Limpeza de logs de eventos do Windows", tactics: ["stealth"] }], description: "O log de auditoria foi apagado." },
  ];
  const TACTICS = [["TA0043","reconnaissance","Reconhecimento"],["TA0042","resource-development","Preparação"],["TA0001","initial-access","Acesso inicial"],["TA0002","execution","Execução"],["TA0003","persistence","Persistência"],["TA0004","privilege-escalation","Escalada de privilégio"],["TA0005","stealth","Ocultação"],["TA0112","defense-impairment","Enfraquecimento de defesas"],["TA0006","credential-access","Acesso a credenciais"],["TA0007","discovery","Descoberta"],["TA0008","lateral-movement","Movimento lateral"],["TA0009","collection","Coleta"],["TA0011","command-and-control","Comando e controle"],["TA0010","exfiltration","Exfiltração"],["TA0040","impact","Impacto"]];
  function mockDerive(ev, role) {
    const logon = ev.code === "4624" || ev.code === "4625";
    if (role === "@action") return logon ? "logon" : null;
    if (role === "@outcome") return logon ? (ev.code === "4625" ? "failure" : "success") : null;
    return null;
  }
  function mockDetectionMatch(ev, id) {
    if (id === "auth.bruteforce.source") return ev.code === "4625";
    if (id === "auth.bruteforce.success") return ev.code === "4625" || ev.code === "4624";
    return false;
  }
  const ipOf = ev => ev.fields?.ip_cliente || "";
  const scopeOf = ip => /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(ip) ? "privado" : "público";
  function mockTriage(rows, detectionSettings = defaultDetectionSettings()) {
    rows.forEach(e => e.event_ref ||= `preview:${e.id}`);
    const enabled = MOCK_RULES.filter(r => !detectionSettings.disabled.includes(r.id));
    const byIp = new Map();
    for (const e of rows) if (e.code === "4625" || e.code === "4624") { const ip = ipOf(e); if (!ip) continue; (byIp.get(ip) || byIp.set(ip, []).get(ip)).push(e); }
    const detections = [];
    // The preview highlights one source, as a real brute force would.
    const top = [...byIp.entries()].sort((a, b) => b[1].filter(e => e.code === "4625").length - a[1].filter(e => e.code === "4625").length).slice(0, 1);
    for (const [ip, list] of top) {
      list.sort((a, b) => a.timestamp - b.timestamp);
      const failures = list.filter(e => e.code === "4625");
      if (failures.length >= 20 && enabled.some(r => r.id === "auth.bruteforce.source")) {
        detections.push({ rule: "auth.bruteforce.source", ids: failures.map(e => e.id), list: failures, ip, distinct: 0 });
        const success = list.find(e => e.code === "4624" && e.timestamp > failures[4].timestamp);
        if (success && enabled.some(r => r.id === "auth.bruteforce.success")) detections.push({ rule: "auth.bruteforce.success", ids: [...failures.slice(0, 5).map(e => e.id), success.id], list: [...failures.slice(0, 5), success], ip, distinct: 0 });
      }
    }
    const shaped = detections.map((d, n) => {
      const rule = MOCK_RULES.find(r => r.id === d.rule);
      return { id: `d${n}-${d.rule}`, rule: d.rule, name: rule.name, description: rule.description, severity: rule.severity, origin: "builtin", kind: rule.kind, attack: rule.attack, tactics: rule.attack.map(a => a.tactics[0]),
        start: d.list[0].timestamp, end: d.list.at(-1).timestamp, count: d.list.length, entities: [{ column: "@src_ip", label: "IP de origem", value: d.ip }],
        summary: d.rule === "auth.bruteforce.source" ? `${d.list.length} falhas de autenticação de ${d.ip}` : `Falhas seguidas de acesso bem-sucedido a partir de ${d.ip}`,
        evidence_members: d.list.map(e=>({event_ref:e.event_ref,event_id:e.id,step:0,fields:[]})), evidence_level: d.rule === "auth.bruteforce.source" ? 1 : 2, evidence_reasons: ["Cenario sintetico de demonstracao"], missing_evidence: ["Uso abusivo posterior"], benign_alternatives: ["Senha expirada"], outcome: "mixed", event_refs: d.list.map(e => e.event_ref), policy_version: "evidence-1", rule_version: "2", normalization_version: "normalization-1", distinct: 0, period_ms: null, event_ids: d.ids, filters: [{ column: "_all", op: "detection", value: d.rule, value2: null }, { column: "@src_ip", op: "equals_exact", value: d.ip, value2: null }] };
    }).filter(d => !detectionSettings.suppress.some(s => s.rule === d.rule && (!s.value || d.entities.some(e => e.value === s.value))));
    const suppressed = detections.length - shaped.length;
    const groups = new Map();
    shaped.forEach((d, i) => { const k = d.entities[0].value; (groups.get(k) || groups.set(k, []).get(k)).push(i); });
    const rank = { critical: 4, high: 3, medium: 2, low: 1, info: 0 };
    const episodes = [...groups.values()].map((idx, n) => {
      const list = idx.map(i => shaped[i]).sort((a, b) => rank[b.severity] - rank[a.severity]);
      const tactics = [...new Set(list.flatMap(d => d.tactics))].sort((a, b) => TACTICS.findIndex(t => t[1] === a) - TACTICS.findIndex(t => t[1] === b));
      return { id: `e${n}`, evidence_level: Math.max(...list.map(d => d.evidence_level)), event_refs: [...new Set(list.flatMap(d => d.event_refs))], title: list[0].name, summary: list.length > 1 ? `${list[0].summary} · também ${list[1].name}` : list[0].summary, severity: list[0].severity, score: list.length * 30, start: Math.min(...list.map(d => d.start)), end: Math.max(...list.map(d => d.end)), detections: idx, tactics, entities: list[0].entities };
    }).sort((a, b) => rank[b.severity] - rank[a.severity]);
    const entities = [...groups.keys()].map(ip => { const evs = byIp.get(ip) || []; const failures = evs.filter(e => e.code === "4625").length; const n = shaped.filter(d => d.entities[0].value === ip).length; const score = Math.min(100, n * 35); return { column: "@src_ip", label: "IP de origem", value: ip, score, evidence_level: 2, level: score >= 70 ? "alto" : score >= 40 ? "médio" : "baixo", detections: n, events: evs.length, failures, first: evs[0]?.timestamp ?? null, last: evs.at(-1)?.timestamp ?? null, scope: scopeOf(ip), tactics: [...new Set(shaped.filter(d => d.entities[0].value === ip).flatMap(d => d.tactics))] }; }).sort((a, b) => b.score - a.score);
    const codes = countBy(rows, "code").filter(([v, n]) => v !== "(vazio)" && n <= 6);
    const rare = codes.slice(0, 8).map(([value, count]) => { const first = rows.filter(e => e.code === value).sort((a, b) => a.timestamp - b.timestamp)[0]; return { column: "code", label: "Código", value, count, first: first?.timestamp ?? null, event_id: first?.id ?? 0, role_events: rows.length, role_distinct: codes.length, filter: { column: "code", op: "equals", value, value2: null } }; });
    const tactics = TACTICS.map(([id, key, label]) => { const hits = shaped.filter(d => d.attack.some(a => a.tactics[0] === key)); const techniques = []; for (const d of hits) for (const a of d.attack) if (a.tactics[0] === key && !techniques.some(t => t.id === a.id)) techniques.push({ id: a.id, name: a.name, count: hits.filter(h => h.attack.some(x => x.id === a.id)).length }); return { id, key, label, count: hits.length, techniques }; });
    const times = rows.map(e => e.timestamp).filter(t => t != null);
    return { analysis_id: "preview-analysis", policy_version: "evidence-1", normalization_version: "normalization-1", attack_version: "19.2", counts_by_level: [1,2,3,4,5].map(n => shaped.filter(d => d.evidence_level === n).length), rule_coverage: [{ rule: "execution", status: "missing_fields", missing: ["process.entity_id"] }], limitations: ["Cenario sintetico de demonstracao"], total: rows.length, undated: rows.length - times.length, start: times.length ? Math.min(...times) : null, end: times.length ? Math.max(...times) : null, complete: true, limited: false, detections: shaped, episodes, entities, rare, tactics, coverage: [{ column: "@user", label: "Usuário", count: rows.filter(e => e.fields?.usuario).length }, { column: "@src_ip", label: "IP de origem", count: rows.filter(e => e.fields?.ip_cliente).length }], suppressed, rules: MOCK_RULES.length - detectionSettings.disabled.length, sigma_rules: 0, sigma_errors: [], threat_rules: detectionSettings.threats ? 378 : 0, elapsed_ms: 120 };
  }
  Object.assign(handlers, {
    case_sync: ({ key, events }) => {
      const caseContentToken = `preview-case-publication:${++casePublication}`;
      caseStore.set(key, { rows: events, contentToken: caseContentToken });
      if (caseStore.size > 3) caseStore.delete(caseStore.keys().next().value);
      return { caseContentToken };
    },
    triage: ({ filters, caseEvents }) => mockTriage(poolOf(caseEvents)),
    triage_evidence_event: ({eventId,eventRef,caseEvents}) => { const e=poolOf(caseEvents).find(e=>e.id===eventId && (e.event_ref || `preview:${e.id}`)===eventRef); if(!e)throw Error("Evento indisponível"); return structuredClone(e); },
    event_insights: ({ event }) => {
      const entities = [];
      if (event.fields?.usuario) entities.push({ role: "User", column: "@user", label: "Usuário", value: event.fields.usuario, scope: null });
      if (event.fields?.ip_cliente) entities.push({ role: "SrcIp", column: "@src_ip", label: "IP de origem", value: event.fields.ip_cliente, scope: scopeOf(event.fields.ip_cliente) });
      const logon = event.code === "4624" || event.code === "4625";
      return { entities, action: logon ? "logon" : null, action_label: logon ? "Autenticação" : null, outcome: event.code === "4625" ? "failure" : event.code === "4624" ? "success" : null, decoded: [], threats: [], rules: event.code === "4625" ? [{ id: "auth.bruteforce.source", name: "Força bruta de senha", severity: "medium", kind: "builtin", attack: MOCK_RULES[0].attack }] : [] };
    },
    timeline_lanes: ({ filters, start, end, bucketCount, column, limit = 8, caseEvents }) => {
      let count = Math.max(1, Math.min(240, bucketCount || 120));
      const width = Math.max(1, Math.ceil((end - start + 1) / count)); count = Math.min(count, Math.floor((end - start) / width) + 1);
      const lanes = new Map(); let missing = 0;
      for (const e of applyFilters(filters, poolOf(caseEvents))) {
        if (e.timestamp == null || e.timestamp < start || e.timestamp > end) continue;
        const value = colStr(e, column); if (!value) { missing++; continue; }
        const lane = lanes.get(value) || lanes.set(value, { value, total: 0, errors: 0, counts: Array(count).fill(0), error_counts: Array(count).fill(0) }).get(value);
        const i = Math.min(count - 1, Math.floor((e.timestamp - start) / width)), error = ["Erro", "Crítico"].includes(e.level);
        lane.total++; lane.counts[i]++; if (error) { lane.errors++; lane.error_counts[i]++; }
      }
      const all = [...lanes.values()].sort((a, b) => b.total - a.total), top = all.slice(0, limit), rest = all.slice(limit);
      const others = rest.length ? rest.reduce((o, l) => { o.total += l.total; o.errors += l.errors; l.counts.forEach((c, i) => { o.counts[i] += c; o.error_counts[i] += l.error_counts[i]; }); return o; }, { value: "", total: 0, errors: 0, counts: Array(count).fill(0), error_counts: Array(count).fill(0) }) : null;
      return { column, start, end, bucketMs: width, buckets: count, lanes: top, others, missing, distinctLimited: false };
    },
    entity_summary: ({ filters, caseEvents, limit = 50 }) => {
      const rows = applyFilters(filters, poolOf(caseEvents));
      return [["@user", "Usuário", "usuario"], ["@src_ip", "IP de origem", "ip_cliente"]].map(([column, label, field]) => {
        const map = new Map();
        for (const e of rows) { const v = e.fields?.[field]; if (!v) continue; const x = map.get(v) || map.set(v, { value: v, count: 0, failures: 0, first: null, last: null, scope: column === "@src_ip" ? scopeOf(v) : null }).get(v); x.count++; x.failures += e.code === "4625"; x.first = x.first == null ? e.timestamp : Math.min(x.first, e.timestamp); x.last = x.last == null ? e.timestamp : Math.max(x.last, e.timestamp); }
        return { column, label, distinct: map.size, distinct_limited: false, values: [...map.values()].sort((a, b) => b.count - a.count).slice(0, limit) };
      }).filter(g => g.values.length);
    },
    ioc_sightings: ({ values, filters }) => values.map(value => {
      const needle = String(value).toLowerCase();
      const hits = applyFilters(filters, events).filter(e => `${e.message} ${JSON.stringify(e.fields || {})}`.toLowerCase().includes(needle));
      return { value, count: hits.length, first: hits.length ? Math.min(...hits.map(e => e.timestamp)) : null, last: hits.length ? Math.max(...hits.map(e => e.timestamp)) : null, event_ids: hits.slice(0, 5).map(e => e.id), sources: [...new Set(hits.map(e => e.source))].slice(0, 8) };
    }),
    source_hashes: () => [{ id: "mock-app", path: "C:\\mock\\mock.jsonl", name: "application.jsonl", bytes: 2400000, sha256: "9f2b5c1e7d3a4b6c8e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f7", origin: "original" }],
  });

  handlers.grouped_timeline = ({ field, grid, filters, caseEvents, caseKey, analysisContext, sourceGeneration: generation, limit = 12 }) => {
    limit = Math.max(1, Math.min(24, limit));
    const counts = () => ({ count: 0, buckets: Array(grid.bucketCount).fill(0) });
    const total = counts(), missing = counts(), other = counts(), groups = new Map();
    let untimed = 0, outsideGrid = 0;
    const add = (group, bucket) => { group.count++; group.buckets[bucket]++; };
    for (const event of applyFilters(filters, poolOf(caseEvents))) {
      const time = event.timestamp;
      if (time == null || time === 0) { untimed++; continue; }
      const bucket = grid.bucketMs ? Math.floor((time - grid.start) / grid.bucketMs) : -1;
      if (bucket < 0 || bucket >= grid.bucketCount) { outsideGrid++; continue; }
      add(total, bucket);
      const raw = field.startsWith("@") && window.QueryLang ? window.QueryLang.fieldValue(event, window.QueryLang.resolve(field))
        : field === "timestamp" || field === "event_ref" ? colStr(event, field) : Object.hasOwn(event, field) ? event[field] : event.fields?.[field];
      if (raw == null) { add(missing, bucket); continue; }
      const key = typeof raw === "string" ? raw : typeof raw === "object" ? JSON.stringify(raw) : String(raw);
      if (!groups.has(key)) groups.set(key, { key, ...counts() });
      add(groups.get(key), bucket);
    }
    const compareKey = (a, b) => {
      const left = [...a], right = [...b];
      for (let i = 0; i < Math.min(left.length, right.length); i++) { const order = left[i].codePointAt(0) - right[i].codePointAt(0); if (order) return order; }
      return left.length - right.length;
    };
    const ranked = [...groups.values()].sort((a, b) => b.count - a.count || compareKey(a.key, b.key));
    for (const group of ranked.slice(limit)) { other.count += group.count; group.buckets.forEach((count, i) => { other.buckets[i] += count; }); }
    return { field, grid, total, series: ranked.slice(0, limit), other, missing, untimed, outsideGrid, limit, selection: "top",
      context: { analysis: analysisContext || null, sourceGeneration: caseKey ? null : generation ?? null, caseKey: caseKey || null } };
  };

  // Case-scoped configuration mirrors native receipts; source events remain immutable.
  const analysisContexts = new Map(); let analysisSerial = 0, storeRevision = 0;
  const analysisIdentity = value => value ? Object.fromEntries(["caseId", "analysisId", "configRevision", "visibilityRevision"].map(key => [key, value[key]])) : null;
  const newAnalysis = caseId => ({ schemaVersion: 1, caseId, analysisId: `preview-analysis-${++analysisSerial}-${Date.now()}`, configRevision: 0, visibilityRevision: 0, config: { derivedFields: [], references: [] }, migrationDiagnostics: [], legacyRaw: null });
  const analysisFor = args => {
    if (!args.analysisContext) return null;
    const value = analysisContexts.get(args.analysisContext.caseId);
    if (!value || JSON.stringify(analysisIdentity(value)) !== JSON.stringify(analysisIdentity(args.analysisContext))) throw Error("ANALYSIS_CONTEXT_CHANGED: Atualize a configuração do Caso.");
    return value;
  };
  // These editor fixtures mirror Case-effective settings and CAS receipts.
  const defaultInterpretation = () => ({ schemaVersion: 1, codes: {}, systemCodes: {}, timestamps: {}, formats: [] });
  const interpretationFor = args => {
    const context = analysisFor(args); if (!context) throw Error("ANALYSIS_CONTEXT_CHANGED: Escolha um Caso antes de editar.");
    return { context, settings: structuredClone(context.interpretation || defaultInterpretation()) };
  };
  const saveInterpretation = (context, settings) => {
    context.interpretation = settings; context.configRevision++;
    const saved = localStorage.getItem("__mockStore");
    if (saved) { const data = JSON.parse(saved), item = data.cases?.find(value => value.id === context.caseId); if (item) { item.analysisContext = structuredClone(context); localStorage.setItem("__mockStore", JSON.stringify(data)); } }
    return { analysisContext: structuredClone(context) };
  };
  // Synthetic Case resource receipts. Native tests verify actual reservations.
  const caseResourceStatus = context => {
    const preferences = structuredClone(context.interpretation?.resources || { schemaVersion: 1, mode: "inherit", workLimitMib: null });
    const limit = Math.min(preferences.workLimitMib ?? 1152, 1152);
    return { analysisContext: structuredClone(context), preferences,
      effective: { accountedLimitMib: limit, accountedUsedBytes: 0, workLiveMib: Math.min(limit, 128), materializedMib: Math.min(limit, 64),
        selectionMib: Math.min(limit, 1024), selectionCacheMib: Math.min(limit, 128), collectedIdsMib: Math.min(limit, 32), analyticsMib: Math.min(limit, 32),
        applicationWorkMib: 128, applicationSelectionMib: 1024 }, minimumWorkMib: 8, maximumWorkMib: 1152,
      clamped: preferences.workLimitMib != null && preferences.workLimitMib > 1152 };
  };
  handlers.case_resource_settings_status = ({ identity }) => {
    const context = analysisFor({ analysisContext: identity });
    if (!context) throw Error("Escolha um Caso antes de configurar recursos.");
    return caseResourceStatus(context);
  };
  handlers.case_resource_settings_save = ({ expected, preferences }) => {
    const { context, settings } = interpretationFor({ analysisContext: expected });
    if (preferences.schemaVersion !== 1) throw Error("Versão de recursos do Caso não suportada.");
    settings.resources = window.ResourceSettings.casePreferences(preferences.mode, String(preferences.workLimitMib), 8192);
    saveInterpretation(context, settings);
    return caseResourceStatus(context);
  };
  const securityFor = args => {
    const value = interpretationFor(args);
    value.settings.security ||= { detectionSettingsJson: JSON.stringify(defaultDetectionSettings()), customRulesJson: null, sigmaSources: [], threatCatalogJson: null };
    return { ...value, security: value.settings.security };
  };
  handlers.detection_rules = args => { const { security } = securityFor(args), preferences = JSON.parse(security.detectionSettingsJson); return { rules: MOCK_RULES.map(r => ({ ...r, origin: "builtin", enabled: !preferences.disabled.includes(r.id) })), sigma_errors: [], sigma_dir: "Fontes Sigma deste Caso (prévia)", settings: preferences, custom_rules_json: security.customRulesJson }; };
  handlers.detection_settings_save = args => { const { context, settings, security } = securityFor(args); security.detectionSettingsJson = JSON.stringify(args.settings); if (args.customRulesJson != null) { if (args.customRulesJson.trim()) JSON.parse(args.customRulesJson); security.customRulesJson = args.customRulesJson.trim() ? args.customRulesJson : null; } return saveInterpretation(context, settings); };
  handlers.sigma_import = args => { securityFor(args); throw Error("A prévia não pode importar arquivos Sigma; use o aplicativo nativo."); };
  handlers.sigma_clear = args => { const { context, settings, security } = securityFor(args); security.sigmaSources = []; return saveInterpretation(context, settings); };
  handlers.triage = args => mockTriage(poolOf(args.caseEvents), JSON.parse(securityFor(args).security.detectionSettingsJson));
  const threatOverride = args => { const text = securityFor(args).security.threatCatalogJson; return text ? JSON.parse(text) : null; };
  handlers.threat_catalog = async args => { const override = threatOverride(args); return (await import('/__mock-threats__.js')).threatCatalog(override); };
  handlers.threat_catalog_update = async args => {
    const { context, settings, security } = securityFor(args), module = await import('/__mock-threats__.js');
    let result;
    if (args.catalogJson != null) { const file = JSON.parse(args.catalogJson); if (!Array.isArray(file.rules) || file.version !== 1 || !file.name) throw Error("Catálogo inválido."); security.threatCatalogJson = args.catalogJson; result = { added: 0, backup_path: null, catalog: await module.threatCatalog(file) }; }
    else { result = await module.threatCatalogUpdate(security.threatCatalogJson ? JSON.parse(security.threatCatalogJson) : null); const { version, name, rules } = result.catalog; security.threatCatalogJson = JSON.stringify({ version, name, rules }); }
    analysisFor(args); return { ...result, ...saveInterpretation(context, settings) };
  };
  handlers.threat_scan = async args => { const override = threatOverride(args); return (await import('/__mock-threats__.js')).threatScan(applyFilters((args.filters || []).filter(f => f.op !== 'threat_rule'), poolOf(args.caseEvents)), args.filters, override); };
  handlers.threat_events = async args => { const override = threatOverride(args); return (await import('/__mock-threats__.js')).threatEvents(applyFilters((args.filters || []).filter(f => f.op !== 'threat_rule'), poolOf(args.caseEvents)), args, override); };
  const previewValidateFilters = handlers.validate_filters;
  handlers.validate_filters = args => { interpretationFor(args); return previewValidateFilters(args); };
  const builtinFormats = handlers.list_formats;
  handlers.list_formats = args => { const { settings } = interpretationFor(args); return [...builtinFormats(), ...(settings.formats || []).map(format => ({ id: `custom:${format.name}`, name: format.name }))]; };
  handlers.get_codes = args => JSON.stringify(interpretationFor(args).settings.codes || {}, null, 2);
  handlers.save_codes = args => { const { context, settings } = interpretationFor(args); settings.codes = JSON.parse(args.text); return saveInterpretation(context, settings); };
  handlers.system_codes_count = args => { interpretationFor(args); return 0; };
  handlers.harvest_codes = args => { const { context, settings } = interpretationFor(args); settings.systemCodes = {}; return { count: 0, sources: 0, ...saveInterpretation(context, settings) }; };
  handlers.save_custom_format = args => {
    const { context, settings } = interpretationFor(args); if (!args.name?.trim()) throw Error("Nome de formato obrigatório.");
    settings.formats = (settings.formats || []).filter(value => value.name !== args.name);
    settings.formats.push(Object.fromEntries(["name", "kind", "pattern", "separator", "fields"].map(key => [key, args[key]])));
    return saveInterpretation(context, settings);
  };
  handlers.get_ts_config = args => structuredClone(interpretationFor(args).settings.timestamps?.[args.path] || null);
  const previewSetTimestamp = handlers.set_ts_config;
  handlers.set_ts_config = async args => {
    interpretationFor(args); await previewSetTimestamp(args);
    const { context, settings } = interpretationFor(args); settings.timestamps ||= {};
    if (args.config == null) delete settings.timestamps[args.path]; else settings.timestamps[args.path] = structuredClone(args.config);
    const receipt = saveInterpretation(context, settings);
    sourceGeneration++; sourceOperationId = args.operationId || null; sourceAnalysisContext = analysisIdentity(context);
    return { ...receipt, publication: { generation: sourceGeneration, operationId: sourceOperationId, analysisContext: structuredClone(sourceAnalysisContext) } };
  };
  const initialCasesLoad = handlers.cases_load;
  handlers.cases_load = () => {
    const store = initialCasesLoad(); storeRevision = store.revision || 0;
    for (const item of store.cases || []) {
      if (!analysisContexts.has(item.id)) analysisContexts.set(item.id, item.analysisContext?.schemaVersion === 1 ? structuredClone(item.analysisContext) : newAnalysis(item.id));
      item.analysisContext = structuredClone(analysisContexts.get(item.id));
    }
    return store;
  };
  handlers.cases_save = ({ data }) => {
    const receipts = [];
    for (const item of data.cases || []) {
      if (!analysisContexts.has(item.id)) { const value = newAnalysis(item.id); analysisContexts.set(item.id, value); receipts.push(structuredClone(value)); }
      item.analysisContext = structuredClone(analysisContexts.get(item.id));
    }
    for (const id of analysisContexts.keys()) if (!data.cases.some(item => item.id === id)) analysisContexts.delete(id);
    data.revision = ++storeRevision; localStorage.setItem("__mockStore", JSON.stringify(data));
    return { revision: storeRevision, analysisContexts: receipts };
  };
  handlers.exclusion_capabilities = () => ({ available: false, reason: "O arquivo de exclusões ainda não está disponível nesta versão." });
  const exclusions = window.createMockExclusions?.({ contextFor: analysisFor, identity: analysisIdentity, generation: () => sourceGeneration,
    sourceAvailable: () => sourceInputs.length > 0,
    rows: () => events, filter: applyFilters, persist: context => {
      const stored = localStorage.getItem("__mockStore"); if (!stored) return;
      const data = JSON.parse(stored), item = data.cases?.find(item => item.id === context.caseId);
      if (item) { item.analysisContext = structuredClone(context); localStorage.setItem("__mockStore", JSON.stringify(data)); }
    } });
  if (exclusions) { Object.assign(handlers, exclusions.handlers); window.__mockExclusions = exclusions; }
  const references = window.createMockReferences?.({ contextFor: analysisFor, identity: analysisIdentity, columnText: colStr,
    persist: context => { const saved = localStorage.getItem("__mockStore"); if (!saved) return; const data = JSON.parse(saved), item = data.cases?.find(item => item.id === context.caseId); if (item) { item.analysisContext = structuredClone(context); localStorage.setItem("__mockStore", JSON.stringify(data)); } } });
  if (references) { Object.assign(handlers, references.handlers); window.__mockReferences = references; }
  handlers.preview_field_transform = ({ value, steps }) => window.__mockFieldTransforms.transform(value, steps);
  handlers.analysis_context_snapshot = ({ caseId }) => {
    const value = analysisContexts.get(caseId); if (!value) throw Error("Caso não encontrado."); return structuredClone(value);
  };
  for (const command of ["list_derived_fields", "save_derived_field", "delete_derived_field"]) {
    const legacy = handlers[command];
    handlers[command] = args => {
      const context = analysisFor(args); if (!context) return legacy(args);
      if (command === "list_derived_fields") return structuredClone(context.config.derivedFields);
      const list = context.config.derivedFields, index = list.findIndex(field => field.name === args.name);
      const previous = list[index], steps = args.steps === undefined ? previous?.steps || [] : args.steps;
      if (command === "save_derived_field" && !args.rules?.length && !steps.length) throw Error("O campo derivado precisa de regras ou transformações.");
      if (index >= 0) list.splice(index, 1);
      if (command === "save_derived_field") list.push({ ...previous, name: args.name, source: args.source, rules: structuredClone(args.rules || []), steps: structuredClone(steps) });
      context.configRevision++;
      const stored = localStorage.getItem("__mockStore");
      if (stored) { const data = JSON.parse(stored), item = data.cases?.find(item => item.id === context.caseId); if (item) { item.analysisContext = context; localStorage.setItem("__mockStore", JSON.stringify(data)); } }
      return { analysisContext: structuredClone(context) };
    };
  }
  if (window.__mockNativeCaseBootstrapEnabled) {
    if (window.CaseEvidence?.active !== true || !window.createMockNativeCase) throw Error("The native startup fixture requires the production native script order and active flag.");
    const native = window.createMockNativeCase({ analysisContexts }); Object.assign(handlers, native.handlers); window.__mockNativeCaseBootstrap = native;
  }
  function analysisRows(rows, context) {
    if (!context) return rows;
    const lookups = references?.prepare(context);
    return rows.map(row => {
      const event = structuredClone(row);
      event.fields = Object.assign(Object.create(null), event.fields);
      // Captured rows may already contain an overlay from another configuration.
      // Restore its exact originals before applying this request's definitions.
      for (const [name, original] of Object.entries(event.derived_originals || {})) {
        if (original.state === "present") event.fields[name] = original.value;
        else if (original.state === "missing") delete event.fields[name];
      }
      delete event.derived_originals;
      delete event.derived_diagnostics;
      const writeFields = (fields, typed) => {
        if (typed && Object.keys(fields).some(name => Object.hasOwn(event.fields, name))) {
          event.derived_diagnostics = [...event.derived_diagnostics || [], { field: typed, code: "target_conflict", message: "O nome do campo ou de um subcampo já existe no registro original.", warning: false }];
          return false;
        }
        event.derived_originals ||= Object.create(null);
        for (const name of Object.keys(fields)) {
          if (!Object.hasOwn(event.derived_originals, name)) event.derived_originals[name] = Object.hasOwn(event.fields, name)
            ? { state: "present", value: event.fields[name] } : { state: "missing" };
        }
        Object.assign(event.fields, fields);
        return true;
      };
      for (const field of context.config.derivedFields) {
        try {
          if (field.lookup) {
            const lookup = lookups?.get(field.name); if (!lookup) throw Error("Referência indisponível para esta consulta.");
            const result = lookup(event); if (result.matched) writeFields(window.__mockFieldTransforms?.expand(field.name, result.value) || { [field.name]: result.value }, field.name);
            continue;
          }
          const hasField = Object.hasOwn(event.fields || {}, field.source);
          const canonical = ["id", "event_ref", "timestamp", "source", "level", "code", "name", "description", "message", "raw"].includes(field.source);
          if (!hasField && !canonical && !field.source.startsWith("@")) continue;
          let value = !canonical && hasField ? event.fields[field.source] : colStr(event, field.source);
          if (field.rules?.length) {
            let matched = false;
            for (const rule of field.rules) {
              if (rule.filter && !matchFilter(event, rule.filter)) continue;
              const match = new RegExp(rule.pattern).exec(typeof value === "string" ? value : JSON.stringify(value)); if (!match) continue;
              const extracted = rule.template ? rule.template.replace(/\$(\d+)/g, (_, i) => match[Number(i)] ?? "") : match[1] ?? match[0];
              if (!extracted) continue;
              value = extracted; matched = true; break;
            }
            if (!matched) continue;
          }
          if (field.steps?.length) {
            const transformed = window.__mockFieldTransforms.transform(value, field.steps);
            if (!writeFields(window.__mockFieldTransforms.expand(field.name, transformed.value), field.name)) continue;
            if (transformed.notices.length) event.derived_diagnostics = [...event.derived_diagnostics || [], ...transformed.notices.map(code => ({ field: field.name, code, warning: true }))];
          } else writeFields({ [field.name]: value });
        } catch (error) { event.derived_diagnostics = [...event.derived_diagnostics || [], { field: field.name, code: field.lookup ? "reference_lookup_error" : "transform_error", message: String(error), warning: false }]; }
      }
      return event;
    });
  }

  function validateFieldContentToken(args) {
    const token = args.caseContentToken;
    if (!args.caseKey) {
      if (token != null) throw Error("ANALYSIS_FIELD_ADMISSION: A origem não aceita um token de evidências do Caso.");
      return;
    }
    if (typeof token !== "string" || !token || new TextEncoder().encode(token).length > 128)
      throw Error("ANALYSIS_FIELD_ADMISSION: Informe um token válido das evidências sincronizadas do Caso.");
    const admitted = caseStore.get(args.caseKey);
    if (!admitted) throw Error("CASE_CACHE_MISS");
    if (token !== admitted.contentToken) throw Error("CASE_CACHE_CHANGED: As evidências do Caso mudaram; recarregue a consulta.");
  }
  handlers.analysis_field_text = args => {
    // Recheck after artificial latency as well as before row admission.
    validateFieldContentToken(args);
    const context = analysisFor(args);
    if (!context) throw Error("ANALYSIS_CONTEXT_CHANGED: Contexto de análise obrigatório.");
    if (!args.caseKey && args.sourceGeneration !== sourceGeneration) throw Error("SOURCE_GENERATION_CHANGED: Atualize a origem.");
    const event = poolOf(args.caseEvents).find(row => row.id === args.id && (row.event_ref || `preview:${row.id}`) === args.eventRef);
    if (!event) throw Error("Evento indisponível para este contexto de análise.");
    const fixed = ["id", "event_ref", "timestamp", "source", "level", "code", "name", "description", "message", "raw"].includes(args.column);
    let present = fixed || Object.hasOwn(event.fields || {}, args.column), value;
    if (args.column === "event_ref") value = event.event_ref || `preview:${event.id}`;
    else if (args.column === "timestamp") value = event.timestamp == null ? null : colStr(event, "timestamp");
    else if (fixed) value = event[args.column];
    else if (args.column.startsWith("@") && window.QueryLang) {
      value = window.QueryLang.fieldValue(event, window.QueryLang.resolve(args.column));
      present = value != null;
    } else value = event.fields?.[args.column];
    const presence = !present ? "missing" : value == null ? "null" : "present";
    const valueType = !present ? null : value == null ? "null" : Array.isArray(value) ? "array" : typeof value;
    let canonicalText = !present || (args.column === "timestamp" && value == null) ? null
      : typeof value === "string" ? value : JSON.stringify(value ?? null);
    // Only original fixture fields have authoritative overrides. A derived
    // field with the same name must use the current analysis overlay instead.
    const fixtureKey = `${args.eventRef}\n${args.column}`;
    if (!args.caseKey && present && canonicalFixture.has(fixtureKey) && !Object.hasOwn(event.derived_originals || {}, args.column)) canonicalText = canonicalFixture.get(fixtureKey);
    return {
      kind: "exact_field", version: 1,
      receipt: { analysisContext: analysisIdentity(context), sourceGeneration: args.caseKey ? null : sourceGeneration,
        caseKey: args.caseKey || null, caseContentToken: args.caseKey ? args.caseContentToken : null,
        catalogSignature: "ca".repeat(32), catalogEpoch: 0 },
      row: { id: event.id, eventRef: args.eventRef }, column: args.column, presence, valueType, canonicalText,
    };
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
  async function simulateLoad(label, total, operationId) {
    for (let done = 0; done < total; done += 900) {
      emitMock("operation-progress", {
        operationId, phaseId: "parse", operation: "carregamento", phase: `Indexando ${label}`,
        completed: done, total, unit: "linhas", cancellable: false,
      });
      await delay(240);
    }
    emitMock("operation-progress", {
      operationId, phaseId: "ready", operation: "carregamento", phase: "Concluído",
      completed: total, total, unit: "linhas", cancellable: false,
    });
  }
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args = {}) => {
        if (window.__mockExclusionsEnabled) for (const event of events) event.event_ref ||= `preview:${event.id}`;
        window.__mockRequests ||= []; window.__mockRequests.push({ cmd, cursor: args.cursor, offset: args.offset, operationId: args.operationId, field: args.field, grid: args.grid, caseKey: args.caseKey, analysisContext: args.analysisContext, sourceGeneration: args.sourceGeneration,
          ...(["analysis_field_text", "event_detail", "java_trace_detail"].includes(cmd) ? { id: args.id, eventRef: args.eventRef, column: args.column, caseContentToken: args.caseContentToken, hasCaseEvents: Object.hasOwn(args, "caseEvents") } : {}) }); if (window.__mockRequests.length > 400) window.__mockRequests.shift();
        window.__mockCommandCalls ||= {};
        window.__mockCommandCalls[cmd] = (window.__mockCommandCalls[cmd] || 0) + 1;
        const h = handlers[cmd];
        if (!h) return Promise.reject(`mock: comando não implementado: ${cmd}`);
        if (cmd === "analysis_field_text" || cmd === "event_detail" || cmd === "java_trace_detail") {
          if ((cmd === "event_detail" || cmd === "java_trace_detail") && args.caseEvents != null) return Promise.reject("ANALYSIS_DETAIL_ADMISSION: Sincronize as evidências do Caso antes de abrir o detalhe.");
          try { validateFieldContentToken(args); } catch (error) { return Promise.reject(String(error)); }
        }
        if (args.caseKey && !args.caseEvents) {
          if (!caseStore.has(args.caseKey)) return Promise.reject("CASE_CACHE_MISS");
          const admitted = caseStore.get(args.caseKey);
          args = { ...args, caseEvents: admitted.rows };
        }
        try {
          const scopedCommands=['list_sources','source_summary','event_detail','analysis_field_text','java_trace_detail','triage','triage_evidence_event','grouped_timeline','query_page','query_events','explore_snapshot','stats_events','dataset_overview','timeline_range','compare_periods','export_events','aggregate_events','profile_fields','discover_patterns','compute_series','pivot','count_filtered','tree_aggs','trail_events','journey_fields','journey_index','journey_events'];
          if (scopedCommands.includes(cmd) && args.analysisContext) args = { ...args, caseEvents: analysisRows(exclusions?.visible(args.analysisContext, poolOf(args.caseEvents)) || poolOf(args.caseEvents), analysisFor(args)) };
          if(scopedCommands.includes(cmd)&&args.filters?.some(filter=>filter.op==='threat_rule')){
            const module=await import('/__mock-threats__.js');
            args={...args,caseEvents:await module.threatFilterRows(poolOf(args.caseEvents),args.filters.filter(filter=>filter.op==='threat_rule'),threatOverride(args)),filters:args.filters.filter(filter=>filter.op!=='threat_rule')};
          }
          // latência artificial para visualizar os estados de carregamento
          if (["load_file", "load_files", "load_event_log"].includes(cmd)) await simulateLoad("mock.jsonl", 6300, args.operationId);
          if (["explore_snapshot", "aggregate_events", "profile_fields"].includes(cmd)) await delay(350);
          // Tests can slow commands down (window.__mockLatency = { cmd: ms }); a cancel in between aborts them like the engine does.
          const extra = window.__mockLatency?.[cmd];
          if (extra) { const generation = window.__mockGeneration || 0; for (let elapsed = 0; elapsed < extra; elapsed += 25) { await delay(Math.min(25, extra - elapsed)); if ((window.__mockGeneration || 0) !== generation || window.__mockCancelledIds?.has(args.operationId)) throw new Error("Operação cancelada."); } }
          if (window.__mockFailures?.[cmd]) throw new Error(window.__mockFailures[cmd]);
          let result = await h(args);
          if (window.__mockExclusionsEnabled && cmd === "cases_load") for (const item of result.cases || []) for (const group of item.items || []) for (const event of group.rows || []) event.event_ref = `preview:${event.id}`;
          if (["load_file", "load_files", "load_bundle", "load_event_log", "clear_events"].includes(cmd)) {
            sourceGeneration++; sourceOperationId = args.operationId || null; sourceAnalysisContext = args.analysisContext ? structuredClone(args.analysisContext) : null;
            const inputs = cmd === "clear_events" ? [] : cmd === "load_bundle" ? args.members
              : cmd === "load_event_log" ? [{ kind: "eventlog", channel: args.channel, maxEvents: args.maxEvents }]
              : [{ kind: "file", paths: args.paths || [args.path], format: args.format || "auto" }];
            sourceInputs = args.merge ? [...sourceInputs, ...inputs] : inputs;
            result ||= {}; result.publication = { generation: sourceGeneration, operationId: sourceOperationId, analysisContext: args.analysisContext || null };
            if (window.__mockExclusionsEnabled) for (const event of events) event.event_ref ||= `preview:${event.id}`;
          }
          return result;
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
      save: () => Promise.resolve("C:\\mock\\export.jsonl"),
      open: (opts = {}) => Promise.resolve(opts.filters?.some(filter => filter.name === "Referência JSONL")
        ? window.__mockReferencePath || references?.defaultPath || null
        : opts.multiple ? ["C:\\mock\\mock.jsonl", "C:\\mock\\firewall.log"] : "C:\\mock\\mock.jsonl"),
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
