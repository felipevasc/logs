/* Search language, mirrored from src-tauri/src/querylang.rs for records evaluated in the interface
   (case chronology) and for suggestions/validation while typing. */
window.QueryLang = (() => {
  "use strict";
  const OR = ["OR", "OU", "||"], AND = ["AND", "E", "&&"], NOT = ["NOT", "NÃO", "NAO"];
  const STANDARD = { nivel: "level", "nível": "level", level: "level", severidade: "level", origem: "source", source: "source", fonte: "source", codigo: "code", "código": "code", code: "code", evento: "code", eventid: "code", mensagem: "message", msg: "message", message: "message", nome: "name", name: "name", descricao: "description", "descrição": "description", description: "description", data: "timestamp", hora: "timestamp", horario: "timestamp", "horário": "timestamp", timestamp: "timestamp", time: "timestamp", bruto: "raw", raw: "raw", tudo: "_all", _all: "_all", "*": "_all" };
  const ROLE_ALIASES = { user: "@user", usuario: "@user", "usuário": "@user", conta: "@user", account: "@user", ip: "@src_ip", src_ip: "@src_ip", srcip: "@src_ip", source_ip: "@src_ip", ip_origem: "@src_ip", client_ip: "@src_ip", dst_ip: "@dst_ip", dstip: "@dst_ip", dest_ip: "@dst_ip", destino: "@dst_ip", ip_destino: "@dst_ip", host: "@host", hostname: "@host", maquina: "@host", "máquina": "@host", computer: "@host", process: "@process", processo: "@process", image: "@process", exe: "@process", parent: "@parent_process", processo_pai: "@parent_process", cmd: "@cmdline", cmdline: "@cmdline", command: "@cmdline", comando: "@cmdline", url: "@url", uri: "@url", domain: "@domain", dominio: "@domain", "domínio": "@domain", hash: "@hash", sha256: "@hash", md5: "@hash", port: "@dst_port", porta: "@dst_port", dst_port: "@dst_port", ua: "@user_agent", user_agent: "@user_agent", useragent: "@user_agent", file: "@file", status: "@status", action: "@action", acao: "@action", "ação": "@action", outcome: "@outcome", resultado: "@outcome", tool: "@tool", ferramenta: "@tool" };
  // Field names that commonly carry each role (records saved before canonical columns existed).
  const FALLBACK = { "@user": ["user.name", "TargetUserName", "username", "user_name", "user", "usuario", "account", "login", "suser"], "@src_ip": ["source.ip", "src_ip", "srcip", "src", "client_ip", "c-ip", "ip_cliente", "IpAddress", "sourceIPAddress", "id.orig_h", "SourceIp"], "@dst_ip": ["destination.ip", "dst_ip", "dst", "dest_ip", "DestinationIp", "id.resp_h"], "@host": ["host.name", "hostname", "host", "computer", "Computer"], "@process": ["process.executable", "Image", "NewProcessName", "process", "exe", "comm"], "@cmdline": ["CommandLine", "command_line", "cmdline"], "@url": ["url", "uri", "path", "cs-uri-stem"], "@status": ["status", "sc-status", "status_code"] };
  const TEXT = new Set(["message", "_all", "raw", "description", "name", "@cmdline", "@url", "@user_agent", "@file"]);
  const LEVELS = { error: "Erro", err: "Erro", erro: "Erro", e: "Erro", warn: "Aviso", warning: "Aviso", aviso: "Aviso", w: "Aviso", info: "Informação", information: "Informação", "informação": "Informação", notice: "Informação", i: "Informação", critical: "Crítico", crit: "Crítico", fatal: "Crítico", "crítico": "Crítico", critico: "Crítico", debug: "Depuração", "depuração": "Depuração", trace: "Rastreio", verbose: "Rastreio", rastreio: "Rastreio" };

  function isPlain(text) {
    if (/[:=<>"()]/.test(text)) return false;
    return !text.split(/\s+/).some(w => [...OR, ...AND, ...NOT].includes(w) || (w.length > 1 && (w[0] === "-" || w[0] === "!")));
  }
  function resolve(name) {
    const lower = name.toLowerCase();
    const standard = STANDARD[lower];
    const column = standard || name;
    const role = column.startsWith("@") ? column : standard ? null : ROLE_ALIASES[lower] || null;
    return { name: column, role, text: TEXT.has(column) };
  }
  function ipToBig(ip) {
    if (ip.includes(".") && !ip.includes(":")) {
      const parts = ip.split(".").map(Number);
      if (parts.length !== 4 || parts.some(p => !Number.isInteger(p) || p < 0 || p > 255)) return null;
      return { v: 4, n: parts.reduce((a, p) => (a << 8n) + BigInt(p), 0n) };
    }
    if (!/^[0-9a-f:]+$/i.test(ip)) return null;
    const [head, tail] = ip.split("::");
    const h = head ? head.split(":") : [], t = tail !== undefined && tail ? tail.split(":") : [];
    if (tail === undefined && h.length !== 8) return null;
    const groups = [...h, ...Array(8 - h.length - t.length).fill("0"), ...t];
    return { v: 6, n: groups.reduce((a, g) => (a << 16n) + BigInt(parseInt(g || "0", 16)), 0n) };
  }
  function cidr(text) {
    const [addr, prefix] = text.trim().split("/");
    const ip = ipToBig(addr); if (!ip) return null;
    const max = ip.v === 4 ? 32 : 128, p = prefix === undefined ? max : Number(prefix);
    if (!Number.isInteger(p) || p < 0 || p > max) return null;
    const mask = p === 0 ? 0n : ((1n << BigInt(max)) - 1n) ^ ((1n << BigInt(max - p)) - 1n);
    return { v: ip.v, net: ip.n & mask, mask };
  }
  const inNet = (value, nets) => {
    const clean = String(value).trim().replace(/^\[|\]$/g, "");
    const ip = ipToBig(clean) || (clean.includes(".") ? ipToBig(clean.replace(/:\d+$/, "")) : null);
    return !!ip && nets.some(n => n.v === ip.v && (ip.n & n.mask) === n.net);
  };
  const wildcard = (pattern, anchored) => new RegExp((anchored ? "^" : "") + pattern.split("").map(c => c === "*" ? ".*" : c === "?" ? "." : c.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("") + (anchored ? "$" : ""), "is");
  const numberFor = (field, value) => {
    const t = String(value).trim();
    if (/^-?\d+(\.\d+)?$/.test(t)) return Number(t);
    if (field === "timestamp") { const d = Date.parse(t); return Number.isNaN(d) ? null : d; }
    return typeof jsParseNumUnit === "function" ? jsParseNumUnit(t) : null;
  };

  function parse(text) {
    const chars = [...text]; let pos = 0, depth = 0;
    const fail = message => { throw new Error(`${message} (posição ${pos + 1}).`); };
    const ws = () => { while (pos < chars.length && /\s/.test(chars[pos])) pos++; };
    const peekWord = () => { let end = pos; while (end < chars.length && !/\s/.test(chars[end]) && chars[end] !== "(" && chars[end] !== ")") end++; return chars.slice(pos, end).join(""); };
    const takeKeyword = set => { const w = peekWord(); if (set.includes(w)) { const next = pos + [...w].length; if (next >= chars.length || /\s/.test(chars[next]) || chars[next] === "(") { pos = next; return true; } } return false; };
    const regexAhead = at => { let i = at + 1; if (chars[i] === "/") return false; while (i < chars.length) { if (chars[i] === "\\") { i += 2; continue; } if (chars[i] === "/") { const n = chars[i + 1]; if (i > at + 1 && (n === undefined || /\s/.test(n) || n === ")")) return true; } i++; } return false; };
    const readRegex = () => {
      const open = pos; let i = open + 1, close = -1;
      while (i < chars.length) { if (chars[i] === "\\") { i += 2; continue; } if (chars[i] === "/") { const n = chars[i + 1]; if (i > open + 1 && (n === undefined || /\s/.test(n) || n === ")")) { close = i; break; } } i++; }
      if (close < 0) fail("Expressão regular sem a barra final");
      let pattern = "";
      for (let j = open + 1; j < close; j++) { if (chars[j] === "\\" && j + 1 < close) { if (chars[j + 1] !== "/") pattern += "\\"; pattern += chars[j + 1]; j++; continue; } pattern += chars[j]; }
      pos = close + 1;
      try { return new RegExp(pattern, "i"); } catch (e) { throw new Error(`Expressão regular inválida: ${e.message}`); }
    };
    const readQuoted = () => { pos++; let out = ""; while (pos < chars.length) { const c = chars[pos++]; if (c === "\\" && pos < chars.length) { out += chars[pos++]; continue; } if (c === '"') return out; out += c; } fail("Aspas sem fechamento"); };
    const fieldAhead = () => {
      const first = chars[pos]; if (!first || !/[\p{L}_@]/u.test(first)) return null;
      let end = pos; while (end < chars.length && /[\p{L}\p{N}_.@\-$]/u.test(chars[end])) end++;
      const op = chars[end]; if (!op || !":=!<>".includes(op)) return null;
      if (op === "!" && chars[end + 1] !== "=") return null;
      if (op === ":") { const a = chars[end + 1], b = chars[end + 2]; if (a === "\\" || a === ":" || (a === "/" && b === "/")) return null; }
      return { name: chars.slice(pos, end).join(""), end };
    };
    const readOp = () => { const two = chars.slice(pos, pos + 2).join(""); const op = [">=", "<=", "!=", "=="].includes(two) ? (two === "==" ? "=" : two) : chars[pos]; pos += two === "==" || op.length === 2 ? 2 : 1; return op; };
    const valueToken = () => {
      if (chars[pos] === '"') return [readQuoted(), true];
      if (chars[pos] === "[") { const start = pos; while (pos < chars.length && chars[pos++] !== "]"); const inner = chars.slice(start + 1, pos - 1).join(""); const m = inner.split(/ TO | ATÉ /); return [m.length === 2 ? `${m[0].trim()}..${m[1].trim()}` : inner, false]; }
      const start = pos; while (pos < chars.length && !/\s/.test(chars[pos]) && chars[pos] !== ")" && chars[pos] !== ",") pos++;
      const v = chars.slice(start, pos).join(""); if (!v) fail("Valor vazio"); return [v, false];
    };
    const smart = (field, value, quoted) => {
      if (field.name === "level") return { kind: "level", value: LEVELS[value.trim().toLowerCase()] || value };
      if (["regra", "rule", "ameaca", "ameaça", "threat"].includes(field.name)) return { kind: "threat", value };
      if (["deteccao", "detecção", "detection"].includes(field.name)) return { kind: "detection", value };
      if (!quoted) {
        if (value === "*") return { kind: "exists" };
        if (value.includes("/") && !value.startsWith("/")) { const n = cidr(value); if (n) return { kind: "cidr", nets: [n] }; }
        if (value.includes("..")) { const [a, b] = value.split(".."); const lo = numberFor(field.name, a), hi = numberFor(field.name, b); if (lo != null && hi != null) { if (lo > hi) throw new Error(`O início do intervalo deve ser menor que o fim em ${field.name}.`); return { kind: "range", lo, hi }; } }
        if (/[*?]/.test(value)) return { kind: "re", re: wildcard(value, true) };
      }
      return field.text || field.name === "_all" ? { kind: "contains", value: value.toLowerCase() } : { kind: "equals", value: value.toLowerCase() };
    };
    const term = () => {
      const f = fieldAhead();
      if (f) {
        pos = f.end; const op = readOp(), field = resolve(f.name);
        if (pos >= chars.length || /\s|\)/.test(chars[pos])) fail(`Informe um valor para ${field.name}`);
        if (chars[pos] === "(" && op === ":") {
          pos++; const items = [];
          for (;;) { ws(); if (pos >= chars.length) fail("Falta fechar a lista de valores"); if (chars[pos] === ")") { pos++; break; } if (takeKeyword(OR) || chars[pos] === ",") { if (chars[pos] === ",") pos++; continue; } items.push(valueToken()); }
          if (!items.length) fail("Lista de valores vazia");
          return { or: items.map(([v, q]) => ({ field, m: smart(field, v, q) })) };
        }
        if (op === ":" && chars[pos] === "/" && regexAhead(pos)) return { field, m: { kind: "re", re: readRegex() } };
        const [value, quoted] = valueToken();
        if (op === "=") return { field, m: { kind: "exact", value } };
        if (op === "!=") return { not: { field, m: smart(field, value, true) } };
        if ([">", ">=", "<", "<="].includes(op)) { const n = numberFor(field.name, value); if (n == null) fail(`Valor numérico inválido: ${value}`); return { field, m: { kind: "cmp", op, n } }; }
        return { field, m: smart(field, value, quoted) };
      }
      if (chars[pos] === '"') return { field: null, m: { kind: "contains", value: readQuoted().toLowerCase() } };
      if (chars[pos] === "/" && regexAhead(pos)) return { field: null, m: { kind: "re", re: readRegex() } };
      const start = pos; while (pos < chars.length && !/\s/.test(chars[pos]) && chars[pos] !== ")" && !(chars[pos] === "(" && pos > start)) pos++;
      const word = chars.slice(start, pos).join("");
      if (!word) fail("Termo vazio");
      if (word === "*") return { all: true };
      if (/[*?]/.test(word)) return { field: null, m: { kind: "re", re: wildcard(word, false) } };
      return { field: null, m: { kind: "contains", value: word.toLowerCase() } };
    };
    const primary = () => {
      ws(); if (pos >= chars.length) fail("Consulta incompleta");
      if (chars[pos] === "(") { pos++; const inner = or(); ws(); if (chars[pos] !== ")") fail("Falta fechar o parêntese"); pos++; return inner; }
      if (chars[pos] === ")") fail("Parêntese fechado sem abertura correspondente");
      return term();
    };
    const unary = () => { ws(); if (takeKeyword(NOT)) { ws(); return { not: unary() }; } const c = chars[pos], n = chars[pos + 1]; if ((c === "-" || c === "!") && n && !/\s/.test(n) && n !== "=") { pos++; return { not: unary() }; } return primary(); };
    const and = () => { const items = [unary()]; for (;;) { ws(); if (pos >= chars.length || chars[pos] === ")") break; if (OR.includes(peekWord())) break; takeKeyword(AND); ws(); items.push(unary()); } return items.length === 1 ? items[0] : { and: items }; };
    function or() { if (++depth > 64) fail("Consulta com aninhamento excessivo"); const items = [and()]; for (;;) { ws(); if (takeKeyword(OR)) { ws(); items.push(and()); } else break; } depth--; return items.length === 1 ? items[0] : { or: items }; }
    const tree = or(); ws();
    if (pos < chars.length) fail(chars[pos] === ")" ? "Parêntese fechado sem abertura correspondente" : "Trecho inesperado");
    return tree;
  }

  function compile(text) {
    const trimmed = String(text || "").trim();
    if (!trimmed) return { all: true };
    if (isPlain(trimmed)) return { field: null, m: { kind: "contains", value: trimmed.toLowerCase() } };
    return parse(trimmed);
  }
  function validate(text) {
    try { compile(text); return null; } catch (e) { return e.message; }
  }

  const scalar = v => v == null ? null : typeof v === "object" ? JSON.stringify(v) : String(v);
  function fieldValue(ev, field) {
    const standard = ["source", "level", "code", "name", "description", "message", "raw"];
    if (field.name === "timestamp") return ev.timestamp == null ? null : new Date(ev.timestamp).toISOString();
    if (standard.includes(field.name)) return ev[field.name] ?? "";
    const direct = scalar(ev.fields?.[field.name]);
    if (direct != null && direct !== "") return direct;
    const role = field.role;
    if (!role) return null;
    const annotated = scalar(ev.fields?.[role]);
    if (annotated != null && annotated !== "") return annotated;
    for (const key of FALLBACK[role] || []) { const v = scalar(ev.fields?.[key]); if (v && v !== "-") return v; }
    if (role === "@src_ip" && /^(?:\d{1,3}\.){3}\d{1,3}$/.test(ev.source || "")) return ev.source;
    return api.derive ? api.derive(ev, role) ?? null : null;
  }
  const anyValue = (ev, test) => [ev.message, ev.source, ev.code, ev.name, ev.description].some(v => v && test(String(v)))
    || Object.entries(ev.fields || {}).some(([k, v]) => !["arquivo", "caminho"].includes(k) && v != null && test(scalar(v)));
  function evalTerm(ev, t) {
    const m = t.m;
    if (!t.field) {
      if (m.kind === "contains") return anyValue(ev, v => v.toLowerCase().includes(m.value));
      if (m.kind === "re") return anyValue(ev, v => m.re.test(v));
      return false;
    }
    if (t.field.name === "_all") {
      if (m.kind === "contains" || m.kind === "equals") return anyValue(ev, v => v.toLowerCase().includes(m.value)) || String(ev.raw || "").toLowerCase().includes(m.value);
      if (m.kind === "re") return anyValue(ev, v => m.re.test(v));
      if (m.kind === "exact") return anyValue(ev, v => v === m.value);
      if (m.kind === "detection") return api.detectionMatch ? api.detectionMatch(ev, m.value) : true;
      return m.kind === "threat";
    }
    if (m.kind === "detection") return api.detectionMatch ? api.detectionMatch(ev, m.value) : true;
    if (m.kind === "threat") return true; // evaluated by the engine; kept visible locally
    if (m.kind === "cmp" || m.kind === "range") {
      const raw = t.field.name === "timestamp" ? ev.timestamp : numberFor("", fieldValue(ev, t.field) ?? "");
      if (raw == null) return false;
      return m.kind === "range" ? raw >= m.lo && raw <= m.hi : m.op === ">" ? raw > m.n : m.op === ">=" ? raw >= m.n : m.op === "<" ? raw < m.n : raw <= m.n;
    }
    const value = fieldValue(ev, t.field);
    if (value == null) return false;
    switch (m.kind) {
      case "contains": return value.toLowerCase().includes(m.value);
      case "equals": return value.trim().toLowerCase() === m.value;
      case "exact": return value === m.value;
      case "re": return m.re.test(value);
      case "cidr": return inNet(value, m.nets);
      case "exists": return !!value.trim();
      case "level": return value === m.value;
      default: return false;
    }
  }
  function test(node, ev) {
    if (node.all) return true;
    if (node.and) return node.and.every(n => test(n, ev));
    if (node.or) return node.or.some(n => test(n, ev));
    if (node.not) return !test(node.not, ev);
    return evalTerm(ev, node);
  }
  const cache = new Map();
  function matches(ev, text) {
    let node = cache.get(text);
    if (!node) { try { node = compile(text); } catch { node = { never: true }; } if (cache.size > 64) cache.clear(); cache.set(text, node); }
    return node.never ? false : test(node, ev);
  }
  const listValues = value => String(value || "").split(/[\n\r,]+/).map(v => v.trim()).filter(Boolean);
  /** Local evaluation of the operators added with the search language. */
  function matchFilter(ev, f) {
    if (f.op === "query") return matches(ev, f.value);
    if (f.op === "in" || f.op === "not_in") { const set = new Set(listValues(f.value).map(v => v.toLowerCase())); const v = fieldValue(ev, resolve(f.column)); const hit = v != null && set.has(v.trim().toLowerCase()); return f.op === "in" ? hit : !hit; }
    if (f.op === "cidr" || f.op === "not_cidr") { const nets = listValues(f.value).flatMap(v => v.split(/\s+/)).map(cidr).filter(Boolean); const v = fieldValue(ev, resolve(f.column)); const hit = v != null && inNet(v, nets); return f.op === "cidr" ? hit : !hit; }
    if (f.op === "detection") return true;
    return undefined;
  }
  // Hosts able to evaluate rules or derive canonical values locally may provide
  // `detectionMatch(ev, ruleId)` and `derive(ev, role)`; records from the engine arrive annotated.
  const api = { compile, validate, matches, matchFilter, isPlain, resolve, fieldValue, cidr, listValues, detectionMatch: null, derive: null };
  return api;
})();
