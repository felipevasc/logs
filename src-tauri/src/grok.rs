//! Grok-style extraction on the embedded PCRE2 engine.
//!
//! Compatibility decision (documented in docs/extracao-normalizacao.md): the Elastic and
//! Logstash Grok implementations run on Joni/Oniguruma; LogInsight already ships a bounded,
//! cancellable PCRE2. We implement the Grok *syntax* (`%{PATTERN}`, `%{PATTERN:field}`,
//! `%{PATTERN:field:int|float}`, user-defined and nested patterns) by expanding it into a
//! PCRE2 expression, and we refuse constructs whose meaning differs between the two
//! engines instead of accepting them with different semantics. The pattern library below
//! is an independent definition written for this project (names follow the public Grok
//! vocabulary); it does not copy the Apache-2.0 pattern files, and every entry is
//! compiled and exercised by the tests in this module.
use crate::search_regex::{Matcher, Pattern};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

pub(crate) const EXPANDED_LIMIT: usize = 64 << 10;
const MAX_DEPTH: usize = 24;
const MAX_ALTERNATIVES: usize = 16;
const MAX_FIELDS: usize = 64;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub name: &'static str,
    pub definition: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub example: &'static str,
}
const H16: &str = "[0-9A-Fa-f]{1,4}";

/// The built-in library, in catalog order.
pub(crate) const LIBRARY: &[Entry] = &[
    Entry { name: "INT", category: "Números", description: "Inteiro, com sinal opcional.", example: "-42", definition: "[+-]?[0-9]+" },
    Entry { name: "POSINT", category: "Números", description: "Inteiro positivo sem zeros à esquerda.", example: "8080", definition: "\\b[1-9][0-9]*\\b" },
    Entry { name: "NONNEGINT", category: "Números", description: "Inteiro maior ou igual a zero (aceita zeros à esquerda).", example: "00123", definition: "\\b[0-9]+\\b" },
    Entry { name: "BASE10NUM", category: "Números", description: "Número decimal com sinal e fração opcionais.", example: "-3.14", definition: "(?<![0-9.+-])[+-]?(?:[0-9]+(?:\\.[0-9]+)?|\\.[0-9]+)" },
    Entry { name: "NUMBER", category: "Números", description: "Número inteiro ou decimal.", example: "12.5", definition: "%{BASE10NUM}" },
    Entry { name: "BASE16NUM", category: "Números", description: "Número hexadecimal, com 0x opcional.", example: "0x2a", definition: "(?<![0-9A-Fa-f])[+-]?(?:0[xX])?[0-9A-Fa-f]+" },
    Entry { name: "HEX", category: "Números", description: "Hexadecimal com prefixo 0x.", example: "0x7ffe", definition: "0[xX][0-9A-Fa-f]+" },
    Entry { name: "EPOCH", category: "Datas e horas", description: "Epoch em segundos ou milissegundos (10 a 13 dígitos).", example: "1700000000", definition: "\\b[0-9]{10,13}\\b" },
    Entry { name: "WORD", category: "Texto", description: "Palavra (letras, dígitos e _).", example: "login", definition: "\\b\\w+\\b" },
    Entry { name: "NOTSPACE", category: "Texto", description: "Tudo até o próximo espaço.", example: "a/b?c=1", definition: "\\S+" },
    Entry { name: "SPACE", category: "Texto", description: "Zero ou mais espaços.", example: "   ", definition: "\\s*" },
    Entry { name: "DATA", category: "Texto", description: "Qualquer texto, o menor possível (não atravessa quebra de linha).", example: "qualquer coisa", definition: ".*?" },
    Entry { name: "GREEDYDATA", category: "Texto", description: "Qualquer texto, o maior possível (até o fim da linha).", example: "resto da linha", definition: ".*" },
    Entry { name: "QUOTEDSTRING", category: "Texto", description: "Texto entre aspas duplas ou simples, com a aspa incluída.", example: "\"ana maria\"", definition: "\"(?:\\\\.|[^\\\\\"])*\"|'(?:\\\\.|[^\\\\'])*'" },
    Entry { name: "QS", category: "Texto", description: "Mesmo que QUOTEDSTRING.", example: "'x y'", definition: "%{QUOTEDSTRING}" },
    Entry { name: "USERNAME", category: "Identidade", description: "Nome de usuário simples (letras, dígitos, ponto, _ e -).", example: "ana.silva", definition: "[a-zA-Z0-9._-]+" },
    Entry { name: "USER", category: "Identidade", description: "Mesmo que USERNAME.", example: "felipe", definition: "%{USERNAME}" },
    Entry { name: "EMAILADDRESS", category: "Identidade", description: "Endereço de e-mail.", example: "ana@example.org", definition: "[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\\.[a-zA-Z]{2,}" },
    Entry { name: "UUID", category: "Identidade", description: "UUID com hifens.", example: "3f2a1b4c-1111-2222-3333-444455556666", definition: "[A-Fa-f0-9]{8}-(?:[A-Fa-f0-9]{4}-){3}[A-Fa-f0-9]{12}" },
    Entry { name: "MAC", category: "Rede", description: "Endereço MAC com : ou -.", example: "00:1a:2b:3c:4d:5e", definition: "(?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}" },
    Entry { name: "IPV4", category: "Rede", description: "Endereço IPv4.", example: "192.168.1.10", definition: "(?<![0-9])%{IPV4_OCTET}(?:\\.%{IPV4_OCTET}){3}(?![0-9])" },
    Entry { name: "IPV4_OCTET", category: "Rede", description: "Um octeto de IPv4 (0 a 255).", example: "255", definition: "(?:25[0-5]|2[0-4][0-9]|[01]?[0-9]{1,2})" },
    Entry { name: "IPV6", category: "Rede", description: "Endereço IPv6, inclusive abreviado com ::.", example: "2001:db8::1", definition: "(?<![0-9A-Fa-f:])(?:(?:H16:){7}H16|(?:H16:){1,7}:|(?:H16:){1,6}:H16|(?:H16:){1,5}(?::H16){1,2}|(?:H16:){1,4}(?::H16){1,3}|(?:H16:){1,3}(?::H16){1,4}|(?:H16:){1,2}(?::H16){1,5}|H16:(?::H16){1,6}|:(?:(?::H16){1,7}|:))(?![0-9A-Fa-f:])" },
    Entry { name: "IP", category: "Rede", description: "Endereço IPv4 ou IPv6.", example: "10.0.0.5", definition: "(?:%{IPV6}|%{IPV4})" },
    Entry { name: "HOSTNAME", category: "Rede", description: "Nome de host ou domínio.", example: "srv-01.example.org", definition: "\\b[0-9A-Za-z][0-9A-Za-z-]{0,62}(?:\\.[0-9A-Za-z][0-9A-Za-z-]{0,62})*\\b" },
    Entry { name: "IPORHOST", category: "Rede", description: "Endereço IP ou nome de host.", example: "10.0.0.5", definition: "(?:%{IP}|%{HOSTNAME})" },
    Entry { name: "HOSTPORT", category: "Rede", description: "Host ou IP com porta.", example: "10.0.0.5:443", definition: "%{IPORHOST}:%{POSINT}" },
    Entry { name: "UNIXPATH", category: "Caminhos", description: "Caminho Unix (começa com /).", example: "/var/log/app.log", definition: "(?:/[A-Za-z0-9_%!$@:.,+~-]*)+" },
    Entry { name: "WINPATH", category: "Caminhos", description: "Caminho do Windows (C:\\… ou \\\\servidor\\…).", example: "C:\\Windows\\System32\\cmd.exe", definition: "(?:[A-Za-z]:|\\\\)(?:\\\\[^\\\\?*\\r\\n\"<>|]*)+" },
    Entry { name: "PATH", category: "Caminhos", description: "Caminho Unix ou Windows.", example: "/etc/hosts", definition: "(?:%{UNIXPATH}|%{WINPATH})" },
    Entry { name: "URIPROTO", category: "Web", description: "Esquema de URI (http, https, ftp…).", example: "https", definition: "[A-Za-z][A-Za-z0-9+.-]*" },
    Entry { name: "URIPATH", category: "Web", description: "Caminho de URI.", example: "/api/v1/items", definition: "(?:/[A-Za-z0-9$.+!*'(){},~:;=@#%&_-]*)+" },
    Entry { name: "URIPARAM", category: "Web", description: "Parâmetros de consulta, com ?.", example: "?id=7&x=y", definition: "\\?[A-Za-z0-9$.+!*'|(){},~@#%&/=:;_?\\[\\]<>-]*" },
    Entry { name: "URI", category: "Web", description: "URI completa com esquema.", example: "https://example.org/a?b=1", definition: "%{URIPROTO}://(?:%{USER}(?::[^@]*)?@)?(?:%{IPORHOST}(?::%{POSINT})?)?(?:%{URIPATH}(?:%{URIPARAM})?)?" },
    Entry { name: "MONTH", category: "Datas e horas", description: "Mês por nome em inglês (Jan, January…).", example: "Oct", definition: "\\b(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\\b" },
    Entry { name: "MONTHNUM", category: "Datas e horas", description: "Mês numérico (1 a 12).", example: "09", definition: "(?:0?[1-9]|1[0-2])" },
    Entry { name: "MONTHDAY", category: "Datas e horas", description: "Dia do mês (1 a 31).", example: "23", definition: "(?:0[1-9]|[12][0-9]|3[01]|[1-9])" },
    Entry { name: "YEAR", category: "Datas e horas", description: "Ano com 2 ou 4 dígitos.", example: "2026", definition: "(?:[0-9]{2}){1,2}" },
    Entry { name: "HOUR", category: "Datas e horas", description: "Hora (0 a 23).", example: "17", definition: "(?:2[0-3]|[01]?[0-9])" },
    Entry { name: "MINUTE", category: "Datas e horas", description: "Minuto (00 a 59).", example: "05", definition: "[0-5][0-9]" },
    Entry { name: "SECOND", category: "Datas e horas", description: "Segundo, com fração opcional.", example: "09.123", definition: "(?:[0-5]?[0-9]|60)(?:[:.,][0-9]+)?" },
    Entry { name: "TIME", category: "Datas e horas", description: "Horário HH:MM:SS(.fração).", example: "17:05:09", definition: "(?<![0-9])%{HOUR}:%{MINUTE}(?::%{SECOND})(?![0-9])" },
    Entry { name: "ISO8601_TIMEZONE", category: "Datas e horas", description: "Fuso ISO 8601 (Z, +03:00, -0300).", example: "+03:00", definition: "(?:Z|[+-]%{HOUR}(?::?%{MINUTE}))" },
    Entry { name: "TIMESTAMP_ISO8601", category: "Datas e horas", description: "Data e hora ISO 8601 (2026-10-23T17:05:09Z).", example: "2026-10-23T17:05:09.123Z", definition: "%{YEAR}-%{MONTHNUM}-%{MONTHDAY}[T ]%{HOUR}:?%{MINUTE}(?::?%{SECOND})?%{ISO8601_TIMEZONE}?" },
    Entry { name: "HTTPDATE", category: "Datas e horas", description: "Data de log de acesso HTTP (23/Oct/2026:17:05:09 +0000).", example: "23/Oct/2026:17:05:09 +0000", definition: "%{MONTHDAY}/%{MONTH}/%{YEAR}:%{TIME} [+-][0-9]{4}" },
    Entry { name: "SYSLOGTIMESTAMP", category: "Datas e horas", description: "Data de syslog (Oct 23 17:05:09).", example: "Oct 23 17:05:09", definition: "%{MONTH} +%{MONTHDAY} %{TIME}" },
    Entry { name: "LOGLEVEL", category: "Logs", description: "Nível de severidade comum (info, WARN, error…).", example: "ERROR", definition: "(?i:trace|debug|info|notice|warn(?:ing)?|err(?:or)?|crit(?:ical)?|alert|emerg(?:ency)?|fatal|severe)" },
    Entry { name: "COMMONAPACHELOG", category: "Logs", description: "Linha de acesso no formato common (Apache/Nginx).", example: "10.0.0.5 - ana [23/Oct/2026:17:05:09 +0000] \"GET /a HTTP/1.1\" 200 512", definition: "%{IPORHOST:clientip} %{USER:ident} %{USER:auth} \\[%{HTTPDATE:timestamp}\\] \"(?:%{WORD:verb} %{NOTSPACE:request}(?: HTTP/%{NUMBER:httpversion})?|%{DATA:rawrequest})\" %{NUMBER:response} (?:%{NUMBER:bytes}|-)" },
    Entry { name: "COMBINEDAPACHELOG", category: "Logs", description: "Formato combined: common mais referrer e agente.", example: "10.0.0.5 - - [23/Oct/2026:17:05:09 +0000] \"GET / HTTP/1.1\" 200 5 \"-\" \"curl/8\"", definition: "%{COMMONAPACHELOG} %{QS:referrer} %{QS:agent}" },
];

fn library_definition(name: &str) -> Option<String> {
    LIBRARY.iter().find(|e| e.name == name).map(|e| e.definition.replace("H16", H16))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Conv {
    Int,
    Float,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    /// UTF-16 range inside the pattern text the person wrote (when known).
    pub start: usize,
    pub end: usize,
    pub alternative: usize,
}

/// One piece of a Grok expression as the editor sees it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum Token {
    Pattern { name: String, field: Option<String>, conv: Option<String>, start: usize, end: usize },
    Literal { start: usize, end: usize },
}

/// Tokenize a Grok expression (UTF-16 offsets, tolerant of unfinished input).
pub(crate) fn tokenize(text: &str) -> Vec<Token> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::new();
    let (mut i, mut literal) = (0usize, 0usize);
    while i < units.len() {
        if units[i] == b'%' as u16 && units.get(i + 1) == Some(&(b'{' as u16)) {
            if let Some(close) = units[i + 2..].iter().position(|u| *u == b'}' as u16) {
                let end = i + 2 + close + 1;
                let body = String::from_utf16_lossy(&units[i + 2..end - 1]);
                let mut parts = body.splitn(3, ':');
                let name = parts.next().unwrap_or_default().to_owned();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    if literal < i {
                        out.push(Token::Literal { start: literal, end: i });
                    }
                    out.push(Token::Pattern { name, field: parts.next().map(str::to_owned), conv: parts.next().map(str::to_owned), start: i, end });
                    i = end;
                    literal = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    if literal < units.len() {
        out.push(Token::Literal { start: literal, end: units.len() });
    }
    out
}

fn valid_field(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.starts_with('_')
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '@' | '-'))
}

/// Constructs that Oniguruma (Elastic/Logstash) and PCRE2 read differently, or that would
/// bypass the field-name table. They are refused, never translated silently.
fn unsupported(literal: &str) -> Option<(String, usize, usize)> {
    let chars: Vec<(usize, char)> = literal.char_indices().collect();
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        let (at, c) = chars[i];
        let next = chars.get(i + 1).map(|p| p.1);
        let width = |k: usize| chars.get(i + k).map_or(literal.len(), |p| p.0);
        if c == '\\' {
            match next {
                Some('h') | Some('H') => return Some(("\\h e \\H significam “dígito hexadecimal” no Oniguruma (Grok do Elastic) e “espaço horizontal” no PCRE2. Escreva [0-9A-Fa-f].".into(), at, width(2))),
                Some('y') | Some('Y') => return Some(("\\y e \\Y (limites de grafema do Oniguruma) não existem no PCRE2.".into(), at, width(2))),
                Some('k') | Some('g') if !in_class && matches!(chars.get(i + 2).map(|p| p.1), Some('<') | Some('\'') | Some('{')) => {
                    return Some(("Referências por nome (\\k<…>, \\g<…>) não são suportadas em Grok: os campos recebem nomes internos. Use a expressão regular PCRE2 diretamente.".into(), at, width(2)))
                }
                _ => {}
            }
            i += 2;
            continue;
        }
        if in_class {
            if c == ']' {
                in_class = false;
            }
        } else if c == '[' {
            in_class = true;
            if next == Some('^') {
                i += 1;
            }
            if chars.get(i + 1).map(|p| p.1) == Some(']') {
                i += 1;
            }
        } else if c == '(' && next == Some('?') {
            let rest: String = chars[i + 2..].iter().take(3).map(|p| p.1).collect();
            if rest.starts_with('~') {
                return Some(("O operador de repetição ausente (?~…) do Oniguruma não existe no PCRE2.".into(), at, width(3)));
            }
            if rest.starts_with("P<") || rest.starts_with('\'') || (rest.starts_with('<') && !rest.starts_with("<=") && !rest.starts_with("<!")) {
                return Some(("Grupos nomeados escritos à mão não são aceitos em Grok: use %{PADRAO:campo}.".into(), at, width(3)));
            }
        }
        i += 1;
    }
    None
}

#[derive(Clone, Debug)]
pub(crate) struct Capture {
    pub field: String,
    pub group: usize,
    pub conv: Option<Conv>,
    pub pattern: String,
}
pub(crate) struct Alternative {
    pub source: String,
    pub matcher: Matcher,
    pub captures: Vec<Capture>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Anchor {
    #[default]
    Search,
    Full,
}
pub(crate) struct Spec<'a> {
    pub patterns: &'a [String],
    pub custom: &'a BTreeMap<String, String>,
    pub anchor: Anchor,
    pub case_insensitive: bool,
    pub dot_all: bool,
}
pub(crate) struct Compiled {
    pub alternatives: Vec<Alternative>,
    pub anchor: Anchor,
}
pub(crate) struct GrokMatch {
    pub alternative: usize,
    /// `(field, byte start, byte end)` for each capture that took part.
    pub fields: Vec<(String, usize, usize)>,
}

struct Expander<'a> {
    custom: &'a BTreeMap<String, String>,
    out_len: usize,
    captures: Vec<Capture>,
    groups: usize,
    names: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
    alternative: usize,
}
impl Expander<'_> {
    fn diag(&mut self, code: &'static str, message: String, start: usize, end: usize) {
        if self.diagnostics.len() < 32 {
            self.diagnostics.push(Diagnostic { code, message, start, end, alternative: self.alternative });
        }
    }
    /// `base` is the UTF-16 offset of `text` in the person's own pattern (nested expansions
    /// report the offset of the reference that led to them).
    fn expand(&mut self, text: &str, stack: &mut Vec<String>, report: Option<(usize, usize)>, out: &mut String) {
        if stack.len() > MAX_DEPTH {
            self.diag("expansion_depth", format!("Padrões aninhados demais (mais de {MAX_DEPTH} níveis)."), report.map_or(0, |r| r.0), report.map_or(0, |r| r.1));
            return;
        }
        let tokens = tokenize(text);
        let units: Vec<u16> = text.encode_utf16().collect();
        let slice = |a: usize, b: usize| String::from_utf16_lossy(&units[a..b]);
        for token in tokens {
            if self.out_len > EXPANDED_LIMIT {
                self.diag("expansion_size", "A expansão do padrão excede 64 KiB.".into(), 0, 0);
                return;
            }
            match token {
                Token::Literal { start, end } => {
                    let piece = slice(start, end);
                    if let Some((message, a, b)) = unsupported(&piece) {
                        let prefix = piece[..a].encode_utf16().count();
                        let width = piece[a..b.min(piece.len())].encode_utf16().count();
                        let (s, e) = report.unwrap_or((start + prefix, start + prefix + width));
                        self.diag("unsupported_construct", message, s, e);
                    }
                    self.out_len += piece.len();
                    out.push_str(&piece);
                }
                Token::Pattern { name, field, conv, start, end } => {
                    let (rs, re) = report.unwrap_or((start, end));
                    let body = match self.custom.get(&name) {
                        Some(custom) => {
                            if LIBRARY.iter().any(|e| e.name == name) {
                                self.diag("shadows_builtin", format!("O padrão “{name}” já existe na biblioteca; escolha outro nome."), rs, re);
                            }
                            custom.clone()
                        }
                        None => match library_definition(&name) {
                            Some(def) => def,
                            None => {
                                self.diag("unknown_pattern", format!("O padrão “{name}” não existe. Use a busca do catálogo ou defina-o como padrão próprio."), rs, re);
                                continue;
                            }
                        },
                    };
                    if stack.contains(&name) {
                        let mut path = stack.clone();
                        path.push(name.clone());
                        self.diag("cycle", format!("Referência circular entre padrões: {}.", path.join(" → ")), rs, re);
                        continue;
                    }
                    let conv = match conv.as_deref() {
                        None => None,
                        Some("int") => Some(Conv::Int),
                        Some("float") => Some(Conv::Float),
                        Some(other) => {
                            self.diag("unknown_conversion", format!("Conversão desconhecida “{other}”. Use int ou float; outros tipos são definidos na saída do mapa."), rs, re);
                            None
                        }
                    };
                    stack.push(name.clone());
                    let mut inner = String::new();
                    if let Some(field) = field.filter(|f| !f.is_empty()) {
                        if !valid_field(&field) {
                            self.diag("invalid_field", format!("Nome de campo inválido: “{field}”."), rs, re);
                        } else if !self.names.insert(field.clone()) {
                            self.diag("duplicate_field", format!("O campo “{field}” aparece mais de uma vez neste padrão."), rs, re);
                        } else if self.captures.len() >= MAX_FIELDS {
                            self.diag("too_many_fields", format!("Use até {MAX_FIELDS} campos por padrão."), rs, re);
                        }
                        self.groups += 1;
                        let group = self.groups;
                        self.captures.push(Capture { field, group, conv, pattern: name.clone() });
                        out.push_str(&format!("(?<g{group}>"));
                        self.expand(&body, stack, Some((rs, re)), &mut inner);
                        out.push_str(&inner);
                        out.push(')');
                    } else {
                        out.push_str("(?:");
                        self.expand(&body, stack, Some((rs, re)), &mut inner);
                        out.push_str(&inner);
                        out.push(')');
                    }
                    stack.pop();
                }
            }
        }
    }
}

/// Expand and compile every alternative. All diagnostics are returned together so the
/// editor can underline each problem; the result is `Err` when any alternative is unusable.
pub(crate) fn compile(spec: &Spec) -> Result<Compiled, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    if spec.patterns.is_empty() || spec.patterns.len() > MAX_ALTERNATIVES {
        return Err(vec![Diagnostic { code: "alternatives", message: format!("Informe de 1 a {MAX_ALTERNATIVES} padrões alternativos."), start: 0, end: 0, alternative: 0 }]);
    }
    for (name, def) in spec.custom {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() || name.len() > 64 {
            diagnostics.push(Diagnostic { code: "invalid_pattern_name", message: format!("Nome de padrão inválido: “{name}” (use letras, dígitos e _)."), start: 0, end: 0, alternative: 0 });
        }
        if def.len() > 8 << 10 {
            diagnostics.push(Diagnostic { code: "pattern_too_long", message: format!("O padrão “{name}” excede 8 KiB."), start: 0, end: 0, alternative: 0 });
        }
    }
    if spec.custom.len() > 128 {
        diagnostics.push(Diagnostic { code: "too_many_patterns", message: "Use até 128 padrões próprios.".into(), start: 0, end: 0, alternative: 0 });
    }
    let mut alternatives = Vec::new();
    for (index, source) in spec.patterns.iter().enumerate() {
        if source.trim().is_empty() {
            diagnostics.push(Diagnostic { code: "empty", message: "O padrão está vazio.".into(), start: 0, end: 0, alternative: index });
            continue;
        }
        let mut expander = Expander { custom: spec.custom, out_len: 0, captures: Vec::new(), groups: 0, names: HashSet::new(), diagnostics: Vec::new(), alternative: index };
        let mut body = String::new();
        expander.expand(source, &mut Vec::new(), None, &mut body);
        let failed = !expander.diagnostics.is_empty();
        diagnostics.append(&mut expander.diagnostics);
        if failed {
            continue;
        }
        let text = match spec.anchor {
            Anchor::Search => format!("(?n){body}"),
            Anchor::Full => format!("(?n)\\A(?:{body})\\z"),
        };
        match Matcher::expanded(&Pattern { engine: "pcre2".into(), text, case_sensitive: !spec.case_insensitive, multiline: false, dot_all: spec.dot_all }, EXPANDED_LIMIT + 256) {
            Ok(matcher) => {
                // Resolve the internal group names to PCRE2 group numbers once.
                let infos = matcher.groups();
                let mut captures = expander.captures.clone();
                for capture in &mut captures {
                    let wanted = format!("g{}", capture.group);
                    match infos.iter().find(|g| g.names.iter().any(|n| *n == wanted)) {
                        Some(info) => capture.group = info.index,
                        None => diagnostics.push(Diagnostic { code: "internal", message: "Não foi possível localizar um grupo compilado.".into(), start: 0, end: 0, alternative: index }),
                    }
                }
                alternatives.push(Alternative { source: source.clone(), matcher, captures });
            }
            Err(error) => diagnostics.push(Diagnostic { code: "compile", message: error, start: 0, end: source.encode_utf16().count(), alternative: index }),
        }
    }
    if diagnostics.is_empty() {
        Ok(Compiled { alternatives, anchor: spec.anchor })
    } else {
        Err(diagnostics)
    }
}

impl Compiled {
    /// First alternative that recognises `text`; its captures as byte ranges.
    pub(crate) fn find(&self, text: &str) -> Result<Option<GrokMatch>, String> {
        for (alternative, alt) in self.alternatives.iter().enumerate() {
            if let Some(ranges) = alt.matcher.first_ranges(text)? {
                let fields = alt
                    .captures
                    .iter()
                    .filter_map(|c| ranges.get(c.group).copied().flatten().map(|(a, b)| (c.field.clone(), a, b)))
                    .collect();
                return Ok(Some(GrokMatch { alternative, fields }));
            }
        }
        Ok(None)
    }
    pub(crate) fn conv_of(&self, alternative: usize, field: &str) -> Option<Conv> {
        self.alternatives.get(alternative)?.captures.iter().find(|c| c.field == field).and_then(|c| c.conv)
    }
    pub(crate) fn fields(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for alt in &self.alternatives {
            for c in &alt.captures {
                if !seen.contains(&c.field) {
                    seen.push(c.field.clone());
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec<'a>(patterns: &'a [String], custom: &'a BTreeMap<String, String>) -> Spec<'a> {
        Spec { patterns, custom, anchor: Anchor::Search, case_insensitive: false, dot_all: false }
    }
    fn run(pattern: &str, text: &str) -> Option<Vec<(String, String)>> {
        let patterns = vec![pattern.to_owned()];
        let custom = BTreeMap::new();
        let compiled = compile(&spec(&patterns, &custom)).unwrap_or_else(|d| panic!("{pattern}: {d:?}"));
        compiled.find(text).unwrap().map(|m| m.fields.into_iter().map(|(f, a, b)| (f, text[a..b].to_owned())).collect())
    }
    #[test]
    fn every_library_entry_compiles_and_matches_its_own_example() {
        for entry in LIBRARY {
            let pattern = format!("^%{{{}:v}}$", entry.name);
            let found = run(&pattern, entry.example).unwrap_or_else(|| panic!("{} does not match {:?}", entry.name, entry.example));
            if !matches!(entry.name, "COMMONAPACHELOG" | "COMBINEDAPACHELOG") {
                assert_eq!(found[0].1, entry.example, "{}", entry.name);
            }
        }
    }
    #[test]
    fn spec_example_extracts_ip_user_and_duration() {
        let found = run("%{IP:ip_origem} usuario=%{WORD:usuario} duracao=%{NUMBER:duracao}", "falha 10.0.0.5 usuario=ana duracao=12.5ms").unwrap();
        assert_eq!(found, vec![("ip_origem".into(), "10.0.0.5".into()), ("usuario".into(), "ana".into()), ("duracao".into(), "12.5".into())]);
    }
    #[test]
    fn ipv4_octets_ipv6_and_ports_are_bounded() {
        assert!(run("%{IPV4:ip}", "999.1.1.1").is_none());
        assert_eq!(run("%{IP:ip}", "de 2001:db8::1 para").unwrap()[0].1, "2001:db8::1");
        assert_eq!(run("%{HOSTPORT:h}", "x 10.1.1.1:8080 y").unwrap()[0].1, "10.1.1.1:8080");
        assert_eq!(run("%{IP:ip}", "a 300.1.1.1 b 10.0.0.1").unwrap()[0].1, "10.0.0.1");
    }
    #[test]
    fn nested_custom_patterns_compose_and_report_the_alternative() {
        let mut custom = BTreeMap::new();
        custom.insert("ACCT".to_owned(), "%{USER:conta}@%{HOSTNAME:dominio}".to_owned());
        let patterns = vec!["login %{ACCT} ok".to_owned(), "acesso de %{ACCT}".to_owned()];
        let compiled = compile(&spec(&patterns, &custom)).unwrap();
        let first = compiled.find("login ana@ex.org ok").unwrap().unwrap();
        assert_eq!(first.alternative, 0);
        let second = compiled.find("acesso de bia@ex.org").unwrap().unwrap();
        assert_eq!(second.alternative, 1);
        assert_eq!(second.fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), vec!["conta", "dominio"]);
        assert!(compiled.find("nada").unwrap().is_none());
    }
    #[test]
    fn diagnostics_cover_unknown_cycles_duplicates_and_unsupported_syntax() {
        let custom: BTreeMap<String, String> = [("A".to_owned(), "x%{B}".to_owned()), ("B".to_owned(), "y%{A}".to_owned())].into();
        let codes = |p: &str| {
            let patterns = vec![p.to_owned()];
            match compile(&spec(&patterns, &custom)) {
                Ok(_) => vec![],
                Err(d) => d.iter().map(|d| d.code).collect::<Vec<_>>(),
            }
        };
        assert_eq!(codes("%{NOPE:x}"), vec!["unknown_pattern"]);
        assert_eq!(codes("%{A}"), vec!["cycle"]);
        assert_eq!(codes("%{INT:n} %{INT:n}"), vec!["duplicate_field"]);
        assert_eq!(codes("\\h+ %{INT:n}"), vec!["unsupported_construct"]);
        assert_eq!(codes("(?<nome>a) %{INT:n}"), vec!["unsupported_construct"]);
        assert_eq!(codes("(?~abc) %{INT:n}"), vec!["unsupported_construct"]);
        assert_eq!(codes("%{INT:n:cor}"), vec!["unknown_conversion"]);
        assert_eq!(codes("%{INT:_x}"), vec!["invalid_field"]);
        assert_eq!(codes("[\\h] %{INT:n}"), vec!["unsupported_construct"]);
        assert!(codes("%{INT:n} [0-9] \\(").is_empty());
    }
    #[test]
    fn expansion_size_and_depth_are_limited() {
        let mut custom = BTreeMap::new();
        custom.insert("P0".to_owned(), "x".repeat(7000));
        for i in 1..12 {
            custom.insert(format!("P{i}"), format!("{}", (0..8).map(|_| format!("%{{P{}}}", i - 1)).collect::<String>()));
        }
        let patterns = vec!["%{P11}".to_owned()];
        let codes: Vec<_> = compile(&spec(&patterns, &custom)).err().unwrap().iter().map(|d| d.code).collect();
        assert!(codes.contains(&"expansion_size"), "{codes:?}");
    }
    #[test]
    fn anchors_are_an_explicit_policy() {
        let patterns = vec!["%{INT:n}".to_owned()];
        let custom = BTreeMap::new();
        let search = compile(&spec(&patterns, &custom)).unwrap();
        assert!(search.find("abc 12 def").unwrap().is_some());
        let full = compile(&Spec { anchor: Anchor::Full, ..spec(&patterns, &custom) }).unwrap();
        assert!(full.find("abc 12 def").unwrap().is_none());
        assert!(full.find("12").unwrap().is_some());
    }
    #[test]
    fn unicode_positions_repeated_values_and_optional_groups_use_real_ranges() {
        let text = "ação=ana ação=ana x=1";
        let patterns = vec!["x=%{INT:x}".to_owned()];
        let custom = BTreeMap::new();
        let compiled = compile(&spec(&patterns, &custom)).unwrap();
        let found = compiled.find(text).unwrap().unwrap();
        let (_, a, b) = found.fields[0].clone();
        assert_eq!(&text[a..b], "1");
        let patterns = vec!["ana(?: %{INT:opcional})?".to_owned()];
        let compiled = compile(&spec(&patterns, &custom)).unwrap();
        assert!(compiled.find("ana").unwrap().unwrap().fields.is_empty());
    }
    #[test]
    fn tokenizer_is_tolerant_and_utf16_based() {
        let tokens = tokenize("😀 %{IP:ip} x %{WORD");
        assert_eq!(tokens[1], Token::Pattern { name: "IP".into(), field: Some("ip".into()), conv: None, start: 3, end: 11 });
        assert!(matches!(tokens.last().unwrap(), Token::Literal { .. }));
    }
    #[test]
    fn common_log_format_pattern_recognises_a_real_line() {
        let line = "10.0.0.5 - ana [23/Oct/2026:17:05:09 +0000] \"GET /a?b=1 HTTP/1.1\" 200 512";
        let found = run("%{COMMONAPACHELOG}", line).unwrap();
        let get = |k: &str| found.iter().find(|f| f.0 == k).map(|f| f.1.clone());
        assert_eq!(get("clientip").as_deref(), Some("10.0.0.5"));
        assert_eq!(get("verb").as_deref(), Some("GET"));
        assert_eq!(get("response").as_deref(), Some("200"));
        assert_eq!(get("timestamp").as_deref(), Some("23/Oct/2026:17:05:09 +0000"));
    }
}
