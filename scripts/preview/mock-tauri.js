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
    if (col === "timestamp") return ev.timestamp == null ? "" : new Date(ev.timestamp).toISOString().replace(/\.000Z$/, "+00:00").replace(/Z$/, "+00:00");
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
  const handlers = {
    mcp_configure: ({ enabled }) => { mcpEnabled = enabled; return handlers.mcp_status(); },
    validate_filters: ({filters}) => { for(const f of filters||[]) if(f.op==="regex") new RegExp(f.value); return null; },
    cancel_operation: () => { window.__mockRemoteCancel?.(); return null; },
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
    list_sources: () => [{id:"mock-app",name:"application.jsonl",path:"C:\\mock\\mock.jsonl",format:"jsonl",bytes:2400000,count:events.length,undated:0,start:now-86400000,end:now,sampled:200,unparsed:0}],
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
    load_bundle: ({ members }) => handlers.load_files({ paths: members.flatMap(s => s.paths || [s.path || s.channel]), merge: false }),
    event_detail: ({ id }) => events.find((e) => e.id === id) || null,
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
        window.__mockCommandCalls ||= {};
        window.__mockCommandCalls[cmd] = (window.__mockCommandCalls[cmd] || 0) + 1;
        const h = handlers[cmd];
        if (!h) return Promise.reject(`mock: comando não implementado: ${cmd}`);
        try {
          const scopedCommands=['query_events','explore_snapshot','stats_events','dataset_overview','timeline_range','compare_periods','export_events','aggregate_events','profile_fields','discover_patterns','compute_series','pivot','count_filtered','tree_aggs','trail_events','journey_fields','journey_index','journey_events'];
          if(scopedCommands.includes(cmd)&&args.filters?.some(filter=>filter.op==='threat_rule')){
            const module=await import('/__mock-threats__.js');
            args={...args,caseEvents:await module.threatFilterRows(poolOf(args.caseEvents),args.filters.filter(filter=>filter.op==='threat_rule')),filters:args.filters.filter(filter=>filter.op!=='threat_rule')};
          }
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
      save: () => Promise.resolve("C:\\mock\\export.jsonl"),
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
