/* Motor de sugestões de colunas: dado um trecho selecionado num registro, gera todas as formas de
 * extraí-lo (cada uma verificada contra o registro) e mede quanto cada uma encaixa nas outras amostras.
 * Puro (sem DOM): a verificação final é do motor nativo (PCRE2). Veja docs/prototipos/colunas.html. */
window.ColumnsEngine = (() => {
"use strict";
// ---------- catálogo de tipos (referências: ECS, padrões Grok, Splunk CIM) ----------
// b = tipo base da conversão; re = como o valor se parece; tok = padrão Grok pronto (quando existe)
const TYPES = [
  { id:"texto", g:"Básicos", l:"Texto", b:"texto", ex:"abc" }, { id:"inteiro", g:"Básicos", l:"Inteiro", b:"inteiro", re:"-?\\d+", tok:"INT", ex:"42" },
  { id:"decimal", g:"Básicos", l:"Decimal", b:"decimal", re:"-?\\d+\\.\\d+", tok:"NUMBER", ex:"12.5" }, { id:"booleano", g:"Básicos", l:"Verdadeiro/falso", b:"booleano", re:"(?:true|false|sim|não|yes|no)", ex:"true" },
  { id:"iso", g:"Data e hora", l:"Data e hora ISO 8601", b:"data-hora", re:"\\d{4}-\\d\\d-\\d\\d[T ]\\d\\d:\\d\\d:\\d\\d(?:[.,]\\d+)?(?:Z|[+-]\\d\\d:?\\d\\d)?", tok:"TIMESTAMP_ISO8601", ex:"2026-10-23 17:05:09" },
  { id:"httpdate", g:"Data e hora", l:"Data e hora de log HTTP", b:"data-hora", re:"\\d\\d/[A-Z][a-z]{2}/\\d{4}:\\d\\d:\\d\\d:\\d\\d [+-]\\d{4}", tok:"HTTPDATE", ex:"23/Oct/2026:17:05:09 +0000" },
  { id:"syslogdate", g:"Data e hora", l:"Data e hora de syslog", b:"data-hora", re:"[A-Z][a-z]{2} +\\d{1,2} \\d\\d:\\d\\d:\\d\\d", tok:"SYSLOGTIMESTAMP", ex:"Oct 23 17:05:09" },
  { id:"epoch_ms", g:"Data e hora", l:"Epoch em milissegundos", b:"data-hora", re:"\\d{13}", ex:"1729702509123" }, { id:"epoch_s", g:"Data e hora", l:"Epoch em segundos", b:"data-hora", re:"\\d{10}", ex:"1729702509" },
  { id:"data_br", g:"Data e hora", l:"Data dd/mm/aaaa", b:"data-hora", re:"\\d\\d/\\d\\d/\\d{4}", ex:"23/10/2026" }, { id:"hora", g:"Data e hora", l:"Hora hh:mm:ss", b:"texto", re:"\\d\\d:\\d\\d:\\d\\d(?:[.,]\\d+)?", tok:"TIME", ex:"17:05:09" },
  { id:"duracao", g:"Data e hora", l:"Duração (ms, s, min, h)", b:"decimal", re:"\\d+(?:\\.\\d+)?\\s?(?:ms|s|min|h)", ex:"12.5ms" },
  { id:"ipv4", g:"Rede", l:"Endereço IPv4", b:"ip", re:"(?:\\d{1,3}\\.){3}\\d{1,3}", tok:"IP", ex:"10.0.0.5" }, { id:"ipv6", g:"Rede", l:"Endereço IPv6", b:"ip", re:"(?:(?:[0-9a-fA-F]{1,4}:){2,7}[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,7}:(?:[0-9a-fA-F]{1,4})?|::(?:[0-9a-fA-F]{1,4}:){0,6}[0-9a-fA-F]{1,4})", tok:"IP", ex:"2001:db8::1" },
  { id:"cidr", g:"Rede", l:"Rede (CIDR)", b:"texto", re:"(?:\\d{1,3}\\.){3}\\d{1,3}/\\d{1,2}", ex:"10.0.0.0/8" }, { id:"porta", g:"Rede", l:"Porta", b:"inteiro", re:"\\d{1,5}", ex:"443" },
  { id:"mac", g:"Rede", l:"Endereço MAC", b:"texto", re:"(?:[0-9a-fA-F]{2}[:-]){5}[0-9a-fA-F]{2}", tok:"MAC", ex:"00:1a:2b:3c:4d:5e" }, { id:"url", g:"Rede", l:"URL", b:"texto", re:"[a-zA-Z][a-zA-Z0-9+.-]*://\\S+", tok:"URI", ex:"https://exemplo.com/a" },
  { id:"dominio", g:"Rede", l:"Domínio", b:"texto", re:"(?:[a-z0-9-]+\\.)+[a-z]{2,}", tok:"HOSTNAME", ex:"exemplo.com" }, { id:"email", g:"Rede", l:"E-mail", b:"texto", re:"[\\w.+-]+@[\\w-]+(?:\\.[\\w-]+)+", tok:"EMAILADDRESS", ex:"ana@exemplo.com" },
  { id:"metodo", g:"Web", l:"Método HTTP", b:"texto", re:"(?:GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)", ex:"GET" }, { id:"status", g:"Web", l:"Código de resposta HTTP", b:"inteiro", re:"[1-5]\\d\\d", ex:"404" }, { id:"useragent", g:"Web", l:"User-agent", b:"texto", re:"Mozilla/[^\"]+", ex:"Mozilla/5.0 …" },
  { id:"uuid", g:"Identificadores", l:"UUID / GUID", b:"texto", re:"[0-9a-fA-F]{8}-(?:[0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}", tok:"UUID", ex:"123e4567-e89b-12d3-a456-426614174000" },
  { id:"md5", g:"Identificadores", l:"Hash MD5", b:"texto", re:"[0-9a-fA-F]{32}", ex:"d41d8cd9…" }, { id:"sha1", g:"Identificadores", l:"Hash SHA-1", b:"texto", re:"[0-9a-fA-F]{40}", ex:"da39a3ee…" }, { id:"sha256", g:"Identificadores", l:"Hash SHA-256", b:"texto", re:"[0-9a-fA-F]{64}", ex:"e3b0c442…" },
  { id:"hex", g:"Identificadores", l:"Número hexadecimal", b:"texto", re:"0[xX][0-9a-fA-F]+", tok:"HEX", ex:"0x2a" }, { id:"sid", g:"Identificadores", l:"SID do Windows", b:"texto", re:"S-1-5-(?:\\d+-)+\\d+", ex:"S-1-5-21-…" }, { id:"pid", g:"Identificadores", l:"ID de processo (PID)", b:"inteiro", re:"\\d{1,7}", ex:"4242" },
  { id:"winpath", g:"Arquivos e sistema", l:"Caminho do Windows", b:"texto", re:"[A-Za-z]:\\\\(?:[^\\\\/:*?\"<>|\\r\\n]+\\\\)*[^\\\\/:*?\"<>|\\r\\n]*", tok:"WINPATH", ex:"C:\\Windows\\cmd.exe" }, { id:"unixpath", g:"Arquivos e sistema", l:"Caminho Unix", b:"texto", re:"(?:/[\\w.-]+)+", tok:"UNIXPATH", ex:"/var/log/app.log" },
  { id:"arquivo", g:"Arquivos e sistema", l:"Nome de arquivo", b:"texto", re:"[\\w.-]+\\.[A-Za-z0-9]{1,5}", ex:"app.log" }, { id:"usuario", g:"Arquivos e sistema", l:"Usuário", b:"texto", re:"[\\w.-]+", tok:"USER", ex:"ana" }, { id:"dominiouser", g:"Arquivos e sistema", l:"Domínio\\Usuário", b:"texto", re:"[\\w.-]+\\\\[\\w.-]+", ex:"CORP\\ana" },
  { id:"nivel", g:"Aplicação", l:"Nível de log", b:"texto", re:"(?:TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|FATAL|CRITICAL)", tok:"LOGLEVEL", ex:"ERROR" }, { id:"classe", g:"Aplicação", l:"Classe Java", b:"texto", re:"(?:[a-z_][\\w$]*\\.)+[A-Z][\\w$]*", tok:"JAVACLASS", ex:"br.app.Pedido" },
  { id:"excecao", g:"Aplicação", l:"Exceção Java", b:"texto", re:"(?:[a-z_][\\w$]*\\.)+[A-Z][\\w$]*(?:Exception|Error)", ex:"java.lang.IllegalStateException" }, { id:"semver", g:"Aplicação", l:"Versão (x.y.z)", b:"texto", re:"\\d+\\.\\d+\\.\\d+(?:-[\\w.]+)?", ex:"2.4.1" },
  { id:"json", g:"Aplicação", l:"JSON", b:"texto", re:"\\{[^\\n]*\\}", ex:"{\"a\":1}" },
  { id:"bytes", g:"Medidas", l:"Tamanho (B, KB, MB, GB)", b:"decimal", re:"\\d+(?:\\.\\d+)?\\s?(?:B|KB|MB|GB|TB)", ex:"12 MB" }, { id:"percent", g:"Medidas", l:"Porcentagem", b:"decimal", re:"\\d+(?:\\.\\d+)?%", ex:"92%" },
  { id:"moeda", g:"Medidas", l:"Valor monetário", b:"decimal", re:"(?:R\\$|US\\$|\\$)\\s?\\d[\\d.,]*", ex:"R$ 12,50" }, { id:"latlon", g:"Medidas", l:"Latitude/longitude", b:"decimal", re:"-?\\d{1,3}\\.\\d{4,}", ex:"-23.5505" },
];
const T = id => TYPES.find(t => t.id === id) || TYPES[0];
const SPECIFIC = ["ipv4","ipv6","cidr","mac","uuid","sha256","sha1","md5","iso","httpdate","syslogdate","email","url","winpath","unixpath","excecao","classe","dominiouser","semver","duracao","bytes","percent","moeda","hora","data_br","nivel","metodo","json","hex","sid","epoch_ms","epoch_s","status","decimal","inteiro","porta","booleano","dominio","arquivo"];
const lit = x => x.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/\s+/g, "\\s+");
// Nome estável de um padrão Grok próprio: o mesmo trecho de regex sempre dá o mesmo nome.
const grokName = (base, src) => { let h = 5381; for (const ch of src) h = ((h * 33) ^ ch.charCodeAt(0)) >>> 0; return base.replace(/[^A-Z0-9_]/gi, "_").toUpperCase() + "_" + h.toString(16).slice(0, 4).toUpperCase(); };

const lg = x => x.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/\s+/g, " ");
// ---------- motor de sugestões ----------
// Para o trecho selecionado, combinamos "O QUE é o valor" (cores) com "ONDE ele está" (localizadores) e
// VERIFICAMOS cada combinação: ela tem de devolver exatamente o trecho escolhido, na posição escolhida.
// O que sobra é medido nas demais amostras (quantas encaixam, se acha uma só vez, se o formato é parecido)
// e ordenado por confiança. Nenhuma sugestão chega à lista sem ter reproduzido a seleção.
const OPEN = { "(":")", "[":"]", "{":"}", "<":">", '"':'"', "'":"'" };
const PAIR_NAME = { "(":"parênteses", "[":"colchetes", "{":"chaves", "<":"sinais < >", '"':"aspas", "'":"aspas simples" };
const ORD = n => n + "º", ORF = n => n + "ª";
const KEYHINT = [[/ip|addr|host/i, "ipv4"], [/port/i, "porta"], [/pid/i, "pid"], [/status|code|resp/i, "status"], [/user|usuario|login|account/i, "usuario"], [/time|date|ts|when/i, "iso"], [/mail/i, "email"], [/url|uri|href/i, "url"], [/path|file|dir/i, "unixpath"], [/size|bytes|len/i, "bytes"], [/dur|elapsed|took|latency/i, "duracao"], [/lvl|level|sev/i, "nivel"]];
const SEPS = { " ":["espaço","[ \\t]+","[^ \\t\\n]"], ",":["vírgula",",","[^,\\n]"], ";":["ponto e vírgula",";","[^;\\n]"], "|":["barra vertical","\\|","[^|\\n]"], "\t":["tabulação","\\t","[^\\t\\n]"] };
function detectSep(line) { let best = null, n = 0; for (const s of Object.keys(SEPS)) { const c = line.split(s).length - 1; if (c > n && (s !== " " || c >= 2)) { best = s; n = c; } } return best; }
const EXTRAS = { // partes de um valor reconhecido (cada uma traz a sua captura)
  iso: [["Só a data", "(\\d{4}-\\d\\d-\\d\\d)(?=[T ]\\d)", "data"], ["Só a hora", "(?<=[T ])(\\d\\d:\\d\\d:\\d\\d)", "hora"], ["Data e hora sem milissegundos", "(\\d{4}-\\d\\d-\\d\\d[T ]\\d\\d:\\d\\d:\\d\\d)(?=[.,]\\d)", "quando"], ["Só o ano", "(\\d{4})(?=-\\d\\d-\\d\\d[T ])", "ano"], ["Só os milissegundos", "(?<=\\d\\d:\\d\\d:\\d\\d[.,])(\\d+)", "ms"]],
  httpdate: [["Só a data", "(\\d\\d/[A-Z][a-z]{2}/\\d{4})(?=:)", "data"], ["Só a hora", "(?<=:)(\\d\\d:\\d\\d:\\d\\d)(?= [+-])", "hora"], ["Só o fuso", "(?<=\\d )([+-]\\d{4})", "fuso"]],
  ipv4: [["Rede /24 do IP (3 primeiros números)", "((?:\\d{1,3}\\.){3})(?=\\d{1,3}\\b)", "rede"], ["Último número do IP", "(?<=(?:\\d{1,3}\\.){3})(\\d{1,3})", "host_id"], ["Primeiro número do IP", "(\\d{1,3})(?=(?:\\.\\d{1,3}){3})", "octeto1"]],
  email: [["Usuário do e-mail (antes do @)", "([\\w.+-]+)(?=@)", "usuario"], ["Domínio do e-mail (depois do @)", "(?<=@)([\\w-]+(?:\\.[\\w-]+)+)", "dominio"]],
  url: [["Domínio da URL", "(?<=://)([^/\\s:?]+)", "dominio"], ["Caminho da URL", "(?<=://[^/\\s]*)(/[^\\s?#]*)", "caminho"], ["Parâmetros da URL (depois do ?)", "(?<=\\?)(\\S+)", "parametros"], ["Protocolo da URL", "([a-zA-Z][a-zA-Z0-9+.-]*)(?=://)", "protocolo"]],
  winpath: [["Nome do arquivo", "(?<=\\\\)([^\\\\\\s]+)(?=\\s|$)", "arquivo"], ["Pasta do arquivo", "([A-Za-z]:\\\\(?:[^\\\\\\s]+\\\\)*)(?=[^\\\\\\s]+(?:\\s|$))", "pasta"], ["Extensão", "\\.(\\w+)(?=\\s|$)", "extensao"], ["Unidade (C:, D:…)", "([A-Za-z]:)(?=\\\\)", "unidade"]],
  unixpath: [["Nome do arquivo", "(?<=/)([^/\\s]+)(?=\\s|$)", "arquivo"], ["Pasta do arquivo", "((?:/[^/\\s]+)+)(?=/[^/\\s]+(?:\\s|$))", "pasta"], ["Extensão", "\\.(\\w+)(?=\\s|$)", "extensao"]],
  duracao: [["Só o número (sem a unidade)", "(\\d+(?:\\.\\d+)?)(?=\\s?(?:ms|s|min|h)\\b)", "duracao"], ["Só a unidade", "(?<=\\d\\s?)(ms|s|min|h)\\b", "unidade"]],
  bytes: [["Só o número (sem a unidade)", "(\\d+(?:\\.\\d+)?)(?=\\s?(?:B|KB|MB|GB|TB)\\b)", "tamanho"], ["Só a unidade", "(?<=\\d\\s?)(B|KB|MB|GB|TB)\\b", "unidade"]],
  percent: [["Só o número (sem %)", "(\\d+(?:\\.\\d+)?)(?=%)", "percentual"]], moeda: [["Só o valor (sem o símbolo)", "(?<=(?:R\\$|US\\$|\\$)\\s?)(\\d[\\d.,]*)", "valor"]],
  classe: [["Só o nome da classe (sem o pacote)", "(?<=\\.)([A-Z][\\w$]*)(?=\\b)", "classe"], ["Só o pacote", "((?:[a-z_][\\w$]*\\.)+)(?=[A-Z])", "pacote"]],
  excecao: [["Só o nome da exceção (sem o pacote)", "(?<=\\.)([A-Z][\\w$]*(?:Exception|Error))", "excecao"]],
  semver: [["Versão maior", "(\\d+)(?=\\.\\d+\\.\\d+)", "versao_maior"], ["Versão menor", "(?<=\\d\\.)(\\d+)(?=\\.\\d+)", "versao_menor"]],
  uuid: [["Primeiro bloco do UUID", "([0-9a-fA-F]{8})(?=-)", "uuid8"]],
  mac: [["Fabricante (3 primeiros pares)", "((?:[0-9a-fA-F]{2}[:-]){2}[0-9a-fA-F]{2})(?=[:-])", "oui"]],
};
const shapeOf = sel => { let out = "", i = 0; while (i < sel.length) { const c = sel[i]; if (/\d/.test(c)) { let j = i; while (/\d/.test(sel[j] || "")) j++; out += "\\d{" + (j - i) + "}"; i = j; } else if (/[A-Za-z]/.test(c)) { let j = i; while (/[A-Za-z]/.test(sel[j] || "")) j++; out += "[A-Za-z]{" + (j - i) + "}"; i = j; } else { out += lit(c); i++; } } return out; };
const loose = sh => sh.replace(/\\d\{\d+\}/g, "\\d+").replace(/\[A-Za-z\]\{\d+\}/g, "[A-Za-z]+");
const valueShape = v => v.replace(/\d+/g, "9").replace(/[A-Za-z]+/g, "a");
const unitRe = x => x; // (o núcleo nunca tem grupos de captura)

function analyze(rec, s, e) {
  const sel = rec.slice(s, e), before = rec.slice(0, s), after = rec.slice(e), lines = rec.split("\n");
  const lineIdx = before.split("\n").length - 1, ls = before.lastIndexOf("\n") + 1, nl = after.indexOf("\n"), le = nl < 0 ? rec.length : e + nl;
  const line = rec.slice(ls, le), inLine = before.slice(ls), restLine = rec.slice(e, le);
  const toks = (t, n, back) => { const m = t.trim().split(/\s+/).filter(Boolean); return back ? m.slice(-n) : m.slice(0, n); };
  const o0 = before.slice(-1), isWrap = OPEN[o0] && after.startsWith(OPEN[o0]);
  // vizinhos: sem contar o delimitador que já abraça a seleção
  const prev = toks(isWrap ? inLine.slice(0, -1) : inLine, 3, true), next = toks(isWrap ? restLine.slice(1) : restLine, 3, false);
  const key = (before.match(/([\w.-]{1,24})\s*[=:]\s*["']?$/) || [])[1];
  const o = before.slice(-1), wrapped = OPEN[o] && after.startsWith(OPEN[o]) ? [o, OPEN[o]] : null;
  const incl = OPEN[sel[0]] && sel.endsWith(OPEN[sel[0]]) && sel.length > 1 ? [sel[0], OPEN[sel[0]]] : null;
  const tokS = ls + (inLine.search(/\S*$/)), tokE = e + (restLine.match(/^\S*/) || [""])[0].length, token = rec.slice(tokS, tokE);
  return { sel, before, after, lines, lineIdx, nLines:lines.length, line, inLine, restLine, prev, next, key, wrapped, incl, token, partial:!/\s/.test(sel) && token !== sel && /\S/.test(token), multi:sel.includes("\n"),
    atRecStart:!before.trim(), atRecEnd:!after.trim(), atLineStart:!inLine.trim(), atLineEnd:!restLine.trim(), sep:detectSep(line), words:sel.trim().split(/\s+/).length, firstTok:(line.trim().match(/^\S+/) || [""])[0] };
}
// ---- O QUE: núcleos (sem grupos de captura). base = estrutura que define uma "ocorrência" (ex.: os parênteses)
function buildCores(c, rec, s, e, hint) {
  const cores = [], add = x => cores.push(x), sel = c.sel;
  const GENERIC = ["arquivo", "usuario", "porta", "pid", "dominio", "inteiro", "decimal", "booleano", "hex", "latlon"];
  let hits = c.multi || c.partial ? [] : SPECIFIC.map(T).filter(t => t.re && new RegExp("^(?:" + t.re + ")$").test(sel));
  if (hits.some(t => !GENERIC.includes(t.id))) hits = hits.filter(t => !GENERIC.includes(t.id) || t.id === hint);
  hits.sort((a, b) => (b.id === hint) - (a.id === hint));
  for (const t of hits.slice(0, 4)) { add({ id:t.id, label:t.l, src:t.re, tok:t.tok, custom:"TIPO_" + t.id.toUpperCase(), typeId:t.id, specific:true, rank:SPECIFIC.indexOf(t.id), names:ECS[t.id] || [], kind:"tipo" }); }
  if (c.multi) {
    add({ id:"restrec", label:"Até o fim do registro", src:"[\\s\\S]*", tok:"GREEDYDATA", typeId:"texto", rank:30, names:["detalhe"], kind:"bloco" });
    add({ id:"block", label:"As mesmas linhas (bloco)", src:sel.split("\n").map(l => l.replace(/\S.*/, m => (/^(at|\bat)\b/.test(m) ? "at [^\\n]+" : "[^\\n]*"))).join("\\n"), tok:null, custom:"BLOCO", typeId:"texto", rank:25, names:["bloco"], kind:"bloco" });
    if (/\n\s+at /.test(sel)) add({ id:"pilha", label:"As linhas “at …” (a pilha)", src:"(?:\\n[ \\t]+at [^\\n]+)+", custom:"PILHA", typeId:"texto", rank:2, names:["pilha"], kind:"bloco" });
    if (/(Exception|Error)/.test(sel)) add({ id:"exc", label:"A linha da exceção", src:"[\\w.$]+(?:Exception|Error)[^\\n]*", custom:"EXCECAO_LINHA", typeId:"texto", rank:3, names:["excecao_linha"], kind:"bloco" });
    return cores;
  }
  const pair = c.wrapped || null, inner = pair ? "[^" + lit(pair[1]) + "\\n]*" : "";
  if (pair) {
    add({ id:"inner", label:"Tudo entre " + pair[0] + " " + pair[1], src:inner, tok:"DATA", typeId:"texto", rank:3, names:[PAIR_NAME[pair[0]] === "aspas" ? "texto" : clean(PAIR_NAME[pair[0]])], kind:"delim", basePre:{ re:lit(pair[0]), gk:lg(pair[0]) }, basePost:{ re:lit(pair[1]), gk:lg(pair[1]) } });
    if (/^\d+$/.test(sel)) add({ id:"innerint", label:"Só o número entre " + pair[0] + " " + pair[1], src:"\\d+", tok:"INT", typeId:"inteiro", rank:2, names:["numero"], kind:"delim", basePre:{ re:lit(pair[0]), gk:lg(pair[0]) }, basePost:{ re:lit(pair[1]), gk:lg(pair[1]) } });
    if (/\s/.test(sel) && /^\S+\s/.test(sel)) { const w = sel.match(/^(\S+)\s+(.*)$/); add({ id:"innerwords", label:"Depois de “" + w[1] + "” entre " + pair[0] + " " + pair[1], src:"[^" + lit(pair[1]) + "\\n]*", tok:"DATA", typeId:"texto", rank:6, names:[clean(w[1])], kind:"delim", basePre:{ re:lit(pair[0]) + lit(w[1]) + "\\s+", gk:lg(pair[0] + w[1]) + " " }, basePost:{ re:lit(pair[1]), gk:lg(pair[1]) } }); }
  }
  if (c.incl) { const [o, cl] = c.incl; add({ id:"incl", label:"Com os " + o + " " + cl + " incluídos", src:lit(o) + "[^" + lit(cl) + "\\n]*" + lit(cl), custom:"COM_DELIMITADORES", typeId:"texto", rank:12, names:[clean(PAIR_NAME[o])], kind:"delim" }); const w = sel.slice(1, -1); if (/^\d+$/.test(w)) add({ id:"incl", label:"Só o número, sem os " + o + " " + cl, src:"\\d+", tok:"INT", typeId:"inteiro", rank:3, names:["numero"], kind:"delim", basePre:{ re:lit(o), gk:lg(o) }, basePost:{ re:lit(cl), gk:lg(cl) } }); }
  if (c.key && !c.partial) { // o valor de uma chave
    const q = /["']$/.test(c.before) && /^["']/.test(c.after), num = /^-?\d+(?:[.,]\d+)?$/.test(sel);
    if (q) add({ id:"kvq", label:"Texto entre aspas", src:"[^\"\\n]*", tok:"DATA", typeId:"texto", rank:4, names:[clean(c.key)], kind:"valor", basePre:{ re:"\"", gk:"\"" }, basePost:{ re:"\"", gk:"\"" } });
    add({ id:"kvs", label:"Valor até o espaço", src:"[^\\s,;)]+", tok:"NOTSPACE", typeId:"texto", rank:6, names:[clean(c.key)], kind:"valor" });
    if (num) add({ id:"kvn", label:"Só o número", src:"-?\\d+(?:\\.\\d+)?", tok:"NUMBER", typeId:"decimal", rank:4, names:[clean(c.key)], kind:"valor" });
    if (/\s/.test(sel)) add({ id:"kvl", label:"Valor até o fim da linha", src:"[^\\n]*", tok:"DATA", typeId:"texto", rank:20, names:[clean(c.key)], kind:"valor" });
  }
  if (/\s/.test(sel)) {
    add({ id:"words", label:c.words + " palavras", src:"\\S+(?:[ \\t]+\\S+){" + (c.words - 1) + "}", custom:"PALAVRAS_" + c.words, typeId:"texto", rank:22, names:["texto"], kind:"texto" });
    add({ id:"lazy", label:"Texto até a âncora seguinte", src:"[^\\n]+?", tok:"DATA", typeId:"texto", rank:12, names:["texto"], kind:"texto", lazy:true });
    if (c.atLineEnd) add({ id:"eol", label:"Até o fim da linha", src:"[^\\n]*", tok:"DATA", typeId:"texto", rank:6, names:["mensagem"], kind:"texto" });
    add({ id:"sent", label:"Uma frase (até o ponto)", src:"[^.\\n]+", custom:"FRASE", typeId:"texto", rank:16, names:["frase"], kind:"texto" });
  }
  if (!hits.length) {
    const sh = shapeOf(sel); add({ id:"shape", label:"Mesmo formato", src:sh, custom:"FORMATO", typeId:"texto", rank:14, names:[clean(c.prev[c.prev.length - 1] || "") || "valor"], kind:"formato", note:sh });
    if (loose(sh) !== sh) add({ id:"shapel", label:"Formato parecido (tamanhos livres)", src:loose(sh), custom:"FORMATO_LIVRE", typeId:"texto", rank:13, names:["valor"], kind:"formato" });
    if (/^\d+$/.test(sel)) { add({ id:"digits", label:"Só dígitos", src:"\\d+", tok:"INT", typeId:"inteiro", rank:8, names:["numero"], kind:"classe" }); add({ id:"digitsn", label:sel.length + " dígitos", src:"\\d{" + sel.length + "}", custom:"DIGITOS_" + sel.length, typeId:"inteiro", rank:9, names:["numero"], kind:"classe" }); }
    else if (/^[A-Za-z]+$/.test(sel)) { add({ id:"letters", label:"Só letras", src:"[A-Za-z]+", custom:"LETRAS", typeId:"texto", rank:9, names:["texto"], kind:"classe" }); if (/^[A-Z]+$/.test(sel)) add({ id:"upper", label:"Só maiúsculas", src:"[A-Z]+", custom:"MAIUSCULAS", typeId:"texto", rank:9, names:["texto"], kind:"classe" }); }
    else if (/^\w+$/.test(sel)) add({ id:"word", label:"Letras, números e _", src:"\\w+", tok:"WORD", typeId:"texto", rank:10, names:["valor"], kind:"classe" });
    if (/^[\w.-]+$/.test(sel)) add({ id:"ident", label:"Letras, números, ponto e traço", src:"[\\w.-]+", custom:"IDENTIFICADOR", typeId:"texto", rank:13, names:["valor"], kind:"classe" });
    add({ id:"nosp", label:"Tudo até o próximo espaço", src:"\\S+", tok:"NOTSPACE", typeId:"texto", rank:12, names:["valor"], kind:"classe" });
    if (c.sep && c.sep !== " ") add({ id:"nosep", label:"Tudo até o próximo " + SEPS[c.sep][0], src:SEPS[c.sep][2] + "+", custom:"CAMPO", typeId:"texto", rank:10, names:["campo"], kind:"classe" });
    add({ id:"len", label:sel.length + " caracteres", src:"[^\\n]{" + sel.length + "}", custom:"CARACTERES_" + sel.length, typeId:"texto", rank:24, names:["valor"], kind:"classe" });
  }
  if (c.partial && !c.key) { // pedaço de uma palavra maior
    const i = c.token.indexOf(sel), pfx = c.token.slice(0, i), sfx = c.token.slice(i + sel.length);
    const d = /^\d+$/.test(sel), core = d ? "\\d+" : /^[A-Za-z]+$/.test(sel) ? "[A-Za-z]+" : "\\w+";
    if (pfx) add({ id:"afterpfx", label:(d ? "Número" : "Parte") + " depois de “" + pfx + "”", src:core, tok:d ? "INT" : "WORD", typeId:d ? "inteiro" : "texto", rank:3, names:[clean(pfx) || "parte"], kind:"parte", basePre:{ re:lit(pfx), gk:lg(pfx) } });
    if (sfx) add({ id:"beforesfx", label:(d ? "Número" : "Parte") + " antes de “" + sfx + "”", src:core, tok:d ? "INT" : "WORD", typeId:d ? "inteiro" : "texto", rank:4, names:[clean(sfx) || "parte"], kind:"parte", basePost:{ re:"(?=" + lit(sfx) + ")", gk:"(?=" + lg(sfx) + ")" } });
    const parts = c.token.split(/[-_./:]/); if (parts.length > 1) { const sepc = c.token.match(/[-_./:]/)[0], k = c.token.slice(0, i).split(/[-_./:]/).length - 1; add({ id:"nthpart", label:ORD(k + 1) + " parte da palavra (separada por “" + sepc + "”)", src:"[^\\s" + lit(sepc) + "]+", custom:"PARTE", typeId:d ? "inteiro" : "texto", rank:5, names:["parte" + (k + 1)], kind:"parte", basePre:{ re:"(?<![^\\s])(?:[^\\s" + lit(sepc) + "]*" + lit(sepc) + "){" + k + "}", gk:"" } }); }
    add({ id:"whole", label:"A palavra inteira (“" + c.token + "”)", src:"\\S+", tok:"NOTSPACE", typeId:"texto", rank:9, names:[clean(c.token.replace(/\d+/g, "")) || "palavra"], kind:"parte" });
  }
  // o mesmo valor, mas contando como "ocorrência" o que está entre os delimitadores (3º parênteses, 2º colchetes…)
  if (pair) for (const k of cores.filter(k => !k.basePre && !k.basePost && ["formato", "classe", "texto", "valor"].includes(k.kind) && !k.lazy)) cores.push({ ...k, id:k.id + "_w", label:k.label + " entre " + pair[0] + " " + pair[1], basePre:{ re:lit(pair[0]), gk:lg(pair[0]) }, basePost:{ re:lit(pair[1]), gk:lg(pair[1]) }, rank:(k.rank ?? 50) - 1, kind:"delim" });
  if (!cores.length) add({ id:"nosp", label:"Tudo até o próximo espaço", src:"\\S+", tok:"NOTSPACE", typeId:"texto", rank:12, names:["valor"], kind:"classe" });
  return cores;
}
// ---- ONDE: localizadores. Recebem a "unidade" (como uma ocorrência se parece) e devolvem [pre, post, texto]
const ND = x => "(?:" + x + ")";
function buildLocators(c, unit, existing, bounded) {
  const L2 = [], add = (pre, post, label, why, kind, w) => L2.push({ pre:pre || { re:"", gk:"" }, post:post || { re:"", gk:"" }, label, why, kind, w:w || 0 });
  const raw = x => ({ re:x, gk:x }), litA = x => ({ re:lit(x) + "\\s+", gk:lg(x) + " " }), litB = x => ({ re:"(?=\\s*" + lit(x) + ")", gk:"(?= ?" + lg(x) + ")" });
  const occRec = (n) => raw(ND("[\\s\\S]*?" + unit) + "{" + (n - 1) + "}[\\s\\S]*?");
  const occLine = n => raw(ND("[^\\n]*?" + unit) + "{" + (n - 1) + "}[^\\n]*?");
  const lineK = k => raw("(?:^|\\n)" + ND("[^\\n]*\\n") + "{" + (k - 1) + "}");
  const lastRec = raw("(?![\\s\\S]*" + unit + ")"), lastLine = raw("(?![^\\n]*" + unit + ")");
  const fromEndRec = k => raw("(?=" + ND("[\\s\\S]*?" + unit) + "{" + (k - 1) + "}(?![\\s\\S]*" + unit + "))");
  const fromEndLine = k => raw("(?=" + ND("[^\\n]*?" + unit) + "{" + (k - 1) + "}(?![^\\n]*" + unit + "))");
  const linesFromEnd = k => raw("(?=[^\\n]*" + ND("\\n[^\\n]*") + "{" + (k - 1) + "}$)");
  // posição no registro
  // quantas vezes a "unidade" aparece: se for comum demais, contar ocorrências não faz sentido (e é caro)
  const whole = c.before + c.sel + c.after, occTotal = (() => { try { return [...whole.matchAll(new RegExp(unit, "g"))].length; } catch { return 99; } })(), many = occTotal > 6 || !bounded; // contar ocorrências de algo ilimitado ("tudo até o fim") é caro e sem sentido
  // Qual ocorrência da unidade é a selecionada (e quantas vêm depois): só se propõe contar até ela, nunca outra
  // (uma contagem errada nunca devolveria a seleção e, num registro longo, custaria segundos de retrocesso).
  const occ = (() => { if (many) return null; try { const ms = [...whole.matchAll(new RegExp(unit, "g"))], at = c.before.length, i = ms.findIndex(m => m.index <= at && at < m.index + Math.max(1, m[0].length)); return i < 0 ? null : { i, fromEnd:ms.length - 1 - i }; } catch { return null; } })();
  const lineRest = c.multi ? null : c.line.slice(c.inLine.length + c.sel.length);
  const lastInLine = !many && lineRest != null ? (() => { try { return ![...lineRest.matchAll(new RegExp(unit, "g"))].length; } catch { return true; } })() : true;
  add(null, null, "primeira ocorrência", "A primeira vez que aparece no registro", "pos", 4);
  for (let n = 2; n <= 4 && occ; n++) if (occ.i + 1 === n) add(occRec(n), null, ORD(n) + " do registro", "Sempre a " + n + "ª vez que aparece no registro", "pos", 5);
  if (!occ || occ.fromEnd === 0) add(null, lastRec, "último do registro", "Sempre a última vez que aparece no registro", "pos", 6);
  for (let k = 2; k <= 3 && occ; k++) if (occ.fromEnd + 1 === k) add(null, fromEndRec(k), ORD(k) + " a partir do fim do registro", "A " + k + "ª vez contando do fim", "pos", 4);
  // linha do registro
  const li = c.lineIdx;
  if (li >= 0) {
    const inLineN = many ? 1 : [...c.inLine.matchAll(new RegExp(unit, "g"))].length + 1;
    add(raw(lineK(li + 1).re + (inLineN > 1 ? occLine(inLineN).re : "[^\\n]*?")), null, (li === 0 ? "na 1ª linha" : "na " + (li + 1) + "ª linha") + (inLineN > 1 ? ", " + ORD(inLineN) + " da linha" : ""), "Na linha " + (li + 1) + " do registro" + (inLineN > 1 ? ", a " + inLineN + "ª ocorrência dela" : ""), "linha", 9);
    if (c.nLines > 1) {
      const fromEnd = c.nLines - li; add(raw("(?:^|\\n)[^\\n]*?"), raw(linesFromEnd(fromEnd).re), (fromEnd === 1 ? "na última linha" : "na " + ORF(fromEnd) + " linha contando do fim"), fromEnd === 1 ? "Sempre na última linha do registro" : "Linha " + fromEnd + " contando do fim", "linha", 8);
      if (inLineN > 1) add(raw("(?:^|\\n)[^\\n]*?"), raw(linesFromEnd(fromEnd).re.slice(0, -2) + "$)"), "na " + ORD(fromEnd) + " linha do fim", "", "linha", 0);
      if (lastInLine) add(raw("(?:^|\\n)[^\\n]*?"), raw(fromEndLine(1).re), "último da linha " + (li + 1), "O último desta linha", "linha", 5);
    }
    if (li === 0) { add(raw("^[^\\n]*?"), null, "na 1ª linha", "Só olha a primeira linha do registro", "linha", 8); if (inLineN > 1) add(raw("^" + occLine(inLineN).re), null, ORD(inLineN) + " da 1ª linha", "", "linha", 7); }
    if (lastInLine) add(raw("(?:^|\\n)[^\\n]*?"), lastLine, "último da linha", "O último desta linha", "linha", 5);
    for (let n = 2; n <= 3 && !many; n++) if (n === inLineN) add(raw("(?:^|\\n)" + occLine(n).re), null, ORD(n) + " da linha", "A " + n + "ª vez dentro da linha", "linha", 5);
    if (c.firstTok && c.nLines > 1 && c.lineIdx > 0) add(raw("(?:^|\\n)" + lit(c.firstTok) + "[^\\n]*?"), null, "na linha que começa com “" + c.firstTok.slice(0, 18) + "”", "A linha que começa com esta palavra", "linha", 11);
    if (c.nLines > 1) { add(raw("(?:^|\\n)[^\\n]*?"), null, "em qualquer linha (a 1ª que encaixar)", "Procura em todas as linhas do registro", "linha", 1); }
  }
  // bordas
  if (c.atRecStart) add(raw("^\\s*"), null, "no começo do registro", "Sempre o primeiro texto do registro", "borda", 10);
  else if (c.atLineStart) add(raw("(?:^|\\n)[ \\t]*"), null, "no começo da linha", "Primeiro texto da linha", "borda", 8);
  if (c.atRecEnd) add(null, raw("\\s*$"), "no fim do registro", "Sempre o último texto do registro", "borda", 10);
  else if (c.atLineEnd) add(null, raw("[ \\t]*(?:$|\\n)"), "no fim da linha", "Último texto da linha", "borda", 8);
  // vizinhança por texto fixo
  const [p1, p2, p3] = [c.prev[c.prev.length - 1], c.prev[c.prev.length - 2], c.prev[c.prev.length - 3]], [n1, n2] = c.next;
  if (c.key) { add(raw(lit(c.key) + "\\s*[=:]\\s*"), null, "valor de “" + c.key + "”", "O que vem depois de " + c.key + "=", "chave", 24); add(raw("\"" + lit(c.key) + "\"\\s*:\\s*"), null, "valor da chave JSON “" + c.key + "”", "Em JSON: \"" + c.key + "\": valor", "chave", 6); }
  if (p1) add(litA(p1), null, "depois de “" + p1 + "”", "Logo depois da palavra " + p1, "vizinho", /\d/.test(p1) ? 5 : 14);
  if (p1 && p2) add(raw(lit(p2) + "\\s+" + lit(p1) + "\\s+"), null, "depois de “" + p2 + " " + p1 + "”", "Depois das duas palavras " + p2 + " " + p1, "vizinho", 10);
  if (p1 && p2 && p3) add(raw(lit(p3) + "\\s+" + lit(p2) + "\\s+" + lit(p1) + "\\s+"), null, "depois de “" + p3 + " " + p2 + " " + p1 + "”", "", "vizinho", 6);
  if (n1 && !c.atLineEnd) add(null, litB(n1), "antes de “" + n1 + "”", "Logo antes da palavra " + n1, "vizinho", /\d/.test(n1) ? 4 : 11);
  if (n1 && n2) add(null, raw("(?=\\s*" + lit(n1) + "\\s+" + lit(n2) + ")"), "antes de “" + n1 + " " + n2 + "”", "", "vizinho", 8);
  if (p1 && n1) add(litA(p1), litB(n1), "entre “" + p1 + "” e “" + n1 + "”", "Entre duas palavras fixas do texto", "vizinho", 13);
  const pc = c.before.slice(-1), nc = c.after.charAt(0);
  if (pc && /[^\w\s]/.test(pc)) add(raw(lit(pc)), null, "depois de “" + pc + "”", "Logo depois deste símbolo", "vizinho", 9);
  if (nc && /[^\w\s]/.test(nc)) add(null, raw("(?=" + lit(nc) + ")"), "antes de “" + nc + "”", "Logo antes deste símbolo", "vizinho", 9);
  if (pc && nc && /[^\w\s]/.test(pc) && /[^\w\s]/.test(nc)) add(raw(lit(pc)), raw("(?=" + lit(nc) + ")"), "entre “" + pc + "” e “" + nc + "”", "Entre dois símbolos", "vizinho", 10);
  // por posição de campo (separador)
  if (c.sep) {
    const [sn, sre, nots] = SEPS[c.sep], k = c.inLine.split(c.sep).length - 1, tot = c.line.split(c.sep).length;
    if (c.sep !== " " || !c.partial) {
      add(raw("(?:^|\\n)" + ND(nots + "*" + sre) + "{" + k + "}"), null, ORD(k + 1) + " campo (separado por " + sn + ")", "Sempre a posição " + (k + 1) + " da linha", "campo", 13);
      add(null, raw(ND(sre + nots + "*") + "{" + (tot - k - 1) + "}[ \\t]*(?:$|\\n)"), ORD(tot - k) + " campo a partir do fim", "Contando do fim da linha", "campo", 9);
    }
  }
  // por palavra
  const wi = c.inLine.trim() ? c.inLine.trim().split(/\s+/).length : 0;
  if (!c.multi) { add(raw("(?:^|\\n)" + ND("\\S+[ \\t]+") + "{" + wi + "}"), null, ORF(wi + 1) + " palavra da linha", "Sempre a palavra de posição " + (wi + 1), "palavra", 6); const tot = c.line.trim().split(/\s+/).length, back = tot - wi - c.words; if (back >= 0) add(null, raw(ND("[ \\t]+\\S+") + "{" + back + "}[ \\t]*(?:$|\\n)"), back === 0 ? "última palavra da linha" : ORF(back + 1) + " palavra a partir do fim", "Contando as palavras do fim da linha", "palavra", 4); }
  // por deslocamento fixo
  add(raw("(?:^|\\n)[^\\n]{" + c.inLine.length + "}"), null, "a partir do caractere " + (c.inLine.length + 1) + " da linha", "Posição fixa na linha", "posfixa", 2);
  // relativo a outros valores já conhecidos
  for (const x of existing) add(raw(x.re + "[^\\n]*?"), null, "depois de “" + x.name + "”", "Depois do campo " + x.name + " que já existe", "campo-existente", 3);
  return L2;
}
function candidates(rec, s, e, ctx) {
  const c = analyze(rec, s, e), hint = c.key && (KEYHINT.find(([re]) => re.test(c.key)) || [])[1];
  const recs = ctx.records;
  const existing = []; for (const { name: n, value: v } of ctx.existing) { const m = Object.entries(TYPES).length && SPECIFIC.map(T).find(t => t.re && new RegExp("^(?:" + t.re + ")$").test(v) && !["inteiro", "arquivo", "usuario", "porta", "pid", "dominio"].includes(t.id)); existing.push({ name:n, re: m ? m.re : lit(v.slice(0, 1)) + "\\S*" }); }
  const raws = [], cores = buildCores(c, rec, s, e, hint);
  const test = (re, mode) => { try { return [...rec.matchAll(new RegExp(re, "dg"))]; } catch { return null; } };
  const out = [], seen = new Set();
  const consider = (k) => {
    if (seen.has(k.re)) return; seen.add(k.re);
    const ms = test(k.re); if (!ms) return;
    const first = ms[0], inSel = m => m && m.indices[1] && (m.indices[1][0] === s && m.indices[1][1] === e);
    const hit = inSel(first) || (k.part && first && first.indices[1] && first.indices[1][0] >= s && first.indices[1][1] <= e ? first : null); // o motor extrai a 1ª ocorrência
    if (!hit) return;                         // tem de devolver a seleção, na posição escolhida
    const vals = recs.map(r => { try { const all = [...r.matchAll(new RegExp(k.re, "g"))]; return { first:all[0] ? all[0][1] : null, n:all.length }; } catch { return { first:null, n:0 }; } });
    const ok = vals.filter(v => v.first != null), cnt = ms.length;
    if (!ok.length) return;
    const once = ok.every(v => v.n === 1), shapes = new Set(ok.map(v => valueShape(v.first)));
    const stableAnchor = !/\d/.test((k.anchorText || ""));
    out.push({ ...k, cover:ok.length, total:recs.length, values:vals.map(v => v.first), once, count:cnt, homog:shapes.size === 1, key:ok.map(v => v.first).join("\u0001") });
  };
  const started = performance.now();
  for (const core of cores) {
    // Orçamento de tempo: com registros enormes, as melhores formas já foram testadas (os núcleos vêm por ordem de qualidade).
    if (performance.now() - started > 2500 && out.length >= 8) break;
    const unit = (core.basePre ? core.basePre.re : "") + core.src + (core.basePost ? core.basePost.re : "");
    const bounded = (core.basePre && core.basePost) || !/\[\\s\\S\]\*|\[\^[^\]]*\]\*|\.\*|\*\?|\+\?|\\S\+\(\?:/.test(core.src);
    const locs = buildLocators(c, unit, existing, bounded);
    for (const lo of locs) {
      if (core.lazy && !lo.post.re) continue;                // texto "até a âncora" precisa de âncora depois
      // um valor que começa/termina em letra ou número não pode ser só um pedaço de outra palavra
      const gPre = !core.basePre && /^\w/.test(c.sel) && core.kind !== "bloco" ? "(?<!\\w)" : "", gPost = !core.basePost && /\w$/.test(c.sel) && core.kind !== "bloco" ? "(?!\\w)" : "";
      const pre = { re:lo.pre.re + (core.basePre ? core.basePre.re : "") + gPre, gk:lo.pre.gk + (core.basePre ? core.basePre.gk : "") }, post = { re:gPost + (core.basePost ? core.basePost.re : "") + lo.post.re, gk:(core.basePost ? core.basePost.gk : "") + lo.post.gk };
      consider({ re:pre.re + "(" + core.src + ")" + post.re, core, lo, label:core.label + " · " + lo.label, why:lo.why ? lo.why + (core.kind === "tipo" ? " (" + core.label.toLowerCase() + ")" : "") : core.label, typeId:core.typeId, names:core.names, specific:!!core.specific, rank:core.rank ?? 50, group:lo.kind, kind:core.kind,
        forms:name => ({ grok:pre.gk + "%{" + (core.tok || grokName(core.custom || "VALOR", core.src)) + ":" + name + "}" + post.gk, regex:pre.re + "(?<" + name + ">" + core.src + ")" + post.re, custom:core.tok ? null : { name:grokName(core.custom || "VALOR", core.src), src:core.src } }), anchorText:(lo.label || "") });
    }
  }
  // partes de um valor reconhecido (data → só a hora, caminho → nome do arquivo…)
  const types = c.multi || c.partial ? [] : SPECIFIC.map(T).filter(t => t.re && new RegExp("^(?:" + t.re + ")$").test(c.sel));
  for (const t of types.slice(0, 3)) for (const [lab, re, nm] of EXTRAS[t.id] || []) consider({ re, part:true, core:{ label:lab, kind:"parte", names:[nm] }, lo:{ kind:"parte", label:"" }, label:lab, why:"Uma parte do " + t.l.toLowerCase(), typeId:/Só o número|Só o valor|Último número|Primeiro número|Só o ano|milissegundos|Versão/i.test(lab) ? "inteiro" : "texto", names:[nm], specific:true, rank:SPECIFIC.indexOf(t.id) + 1, group:"parte", kind:"parte", forms:name => ({ grok:null, regex:re.replace("(", "(?<" + name + ">"), custom:null }) });
  // reserva: SEMPRE há ao menos uma forma de pegar exatamente este trecho (texto exato, texto ao redor, posição)
  {
    const q = x => x.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), sel = c.sel, len = sel.length, bef = c.before, aft = c.after;
    const nm = clean(c.prev[c.prev.length - 1] || "") || "valor", short = x => x.replace(/\n/g, "⏎").replace(/\t/g, "→");
    const fb = (label, why, pre, core, post, custom) => consider({ re:pre + "(" + core + ")" + post, core:{ label, kind:"reserva", names:[nm] }, lo:{ kind:"reserva", label:"", w:-15 }, label:"Alternativa: " + label, why, typeId:"texto", names:[nm], specific:false, rank:60, group:"reserva", kind:"reserva", forms:name => ({ grok:pre + "%{" + grokName(custom, core) + ":" + name + "}" + post, regex:pre + "(?<" + name + ">" + core + ")" + post, custom:{ name:grokName(custom, core), src:core } }) });
    // o texto exato (na enésima vez que aparece)
    const whole2 = bef + sel + aft, occs = [...whole2.matchAll(new RegExp(q(sel), "g"))], at = occs.findIndex(m => m.index === s);
    if (at >= 0 && at <= 6) fb("sempre o texto “" + short(sel.slice(0, 30)) + (sel.length > 30 ? "…" : "") + "”" + (at ? " · " + (at + 1) + "ª vez" : ""), at ? "O texto exato, na " + (at + 1) + "ª vez que aparece no registro" : "O texto exato, quando ele aparecer", at ? "(?:[\\s\\S]*?" + q(sel) + "){" + at + "}[\\s\\S]*?" : "", q(sel), "", "TEXTO_EXATO");
    // pelo texto exato que vem antes e/ou depois
    for (const k of [3, 8, 16, 32, 64]) {
      const b = bef.slice(-k), a = aft.slice(0, k); if (!b && !a) continue;
      if (b) fb("mesmo tamanho (" + len + " caracteres) depois de “…" + short(b) + "”", "Os " + len + " caracteres que vêm logo depois do texto “" + short(b) + "”", q(b), "[\\s\\S]{" + len + "}", "", "MESMO_TAMANHO");
      if (a) fb("tudo até “" + short(a) + "…”" + (b ? ", depois de “…" + short(b) + "”" : ""), "O que está entre " + (b ? "“" + short(b) + "”" : "o início") + " e “" + short(a) + "”", b ? q(b) : "^", "[\\s\\S]*?", "(?=" + q(a) + ")", "ENTRE_TEXTOS");
    }
    // pela posição no registro
    fb("caracteres " + (s + 1) + " a " + e + " do registro", "Sempre nesta posição, contando do começo do registro", "^[\\s\\S]{" + s + "}", "[\\s\\S]{" + len + "}", "", "POSICAO_FIXA");
    fb("os " + len + " caracteres, contando do fim do registro", "Sempre nesta posição, contando do fim do registro", "", "[\\s\\S]{" + len + "}", "[\\s\\S]{" + (whole2.length - e) + "}$", "POSICAO_DO_FIM");
  }
  // pontuação: confiança que o padrão vai pegar o mesmo tipo de coisa nas outras amostras
  for (const k of out) {
    const cov = k.cover / k.total, len = k.re.length;
    k.score = cov * 40 + (k.once ? 14 : -8) + (k.homog ? 8 : 0) + (k.count === 1 ? 6 : 0) + (k.specific ? 34 - Math.min(k.rank, 30) : 18 - Math.min(k.rank, 18) / 2) + (k.lo.w || 0) - Math.min(14, len / 45) + (k.kind === "delim" ? 8 : 0) + (k.group === "chave" ? 10 : 0) + (k.group === "parte" ? 0 : 0);
    k.names = (k.names || []).concat(c.key ? [clean(c.key)] : []);
  }
  out.sort((a, b) => b.score - a.score);
  // equivalentes (mesmos valores nas amostras): fica o mais robusto e conta os demais
  const bySig = new Map(), uniq = [];
  for (const k of out) { const sig = k.key + "|" + k.kind + "|" + k.group + "|" + (k.lo && k.lo.label && /^(primeira|último|[0-9]º)/.test(k.lo.label) ? k.lo.label : ""); if (bySig.has(sig)) { bySig.get(sig).alts = (bySig.get(sig).alts || 0) + 1; continue; } bySig.set(sig, k); uniq.push(k); }
  uniq.forEach((k, i) => k.rec = i === 0);
  const res = uniq.slice(0, 40); res.total = out.length;
  return res;
}
const ECS = { ipv4:["ip_origem","ip_destino","client.ip"], ipv6:["ip_origem","ip_destino"], iso:["quando","@timestamp"], httpdate:["quando"], email:["email","user.email"], url:["url","url.full"], status:["status","http.response.status_code"], metodo:["metodo","http.request.method"], usuario:["usuario","user.name"], winpath:["caminho","file.path"], unixpath:["caminho","file.path"], nivel:["nivel","log.level"], classe:["classe","log.logger"], duracao:["duracao","event.duration"], bytes:["bytes","http.response.bytes"], uuid:["id","event.id"], porta:["porta","destination.port"], pid:["pid","process.pid"], mac:["mac","source.mac"] };
const clean = s => (s || "").normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/[^\w.@]+/g, "_").replace(/^_+|_+$/g, "").toLowerCase();
function nameSuggestions(c) { const out = (c.names || []).map(clean); for (const n of ECS[c.typeId] || []) out.push(n); out.push(clean(T(c.typeId).l.split(" ")[0])); return [...new Set(out.filter(n => n && n.length > 1))].slice(0, 5); }
return { TYPES, T, SPECIFIC, candidates, nameSuggestions, clean, lit };
})();
