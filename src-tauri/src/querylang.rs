//! Search language shared by the search box, saved queries, detections and Sigma.
//!
//! `texto livre`, `campo:valor`, `campo="exato"`, `campo!=valor`, `campo>10`,
//! `campo:10.0.0.0/8`, `campo:adm*`, `campo:(a OR b)`, `campo:/regex/`,
//! `campo:*`, `a..b`, `NOT`, `-termo`, `AND`, `OR` and parentheses.
//! Text without syntax keeps the phrase semantics of the original quick search.
use crate::entities::{self, Role};
use crate::model::{label_class, Event, LineMeta, LV_OTHER};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::net::IpAddr;

#[derive(Debug)]
pub enum Expr {
    All,
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Term(Term),
}

#[derive(Debug)]
pub struct Term {
    field: Option<Field>,
    matcher: Matcher,
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    role: Option<Role>,
    text: bool,
    /// Sigma field names: case-insensitive lookup before the role fallback.
    ci: bool,
}

#[derive(Debug, Clone, Copy)]
enum Cmp {
    Gt,
    Gte,
    Lt,
    Lte,
}

pub struct Threat(crate::threats::RuleMatcher);
impl std::fmt::Debug for Threat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Threat")
    }
}

/// `deteccao:<id>`: records a detection rule selects (any of its steps).
pub struct Detection(std::sync::Arc<crate::detections::RuleSet>, usize);
impl std::fmt::Debug for Detection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Detection")
    }
}

#[derive(Debug)]
enum Matcher {
    Contains(String),
    Equals(String),
    Exact(String),
    Wildcard(regex::Regex),
    Regex(regex::Regex),
    Cidr(Vec<IpNet>),
    Cmp(Cmp, f64),
    Range(f64, f64),
    Exists,
    Set(HashSet<String>),
    Level(String),
    Threat(Threat),
    Detection(Detection),
}

#[derive(Debug, Clone, Copy)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    pub fn parse(text: &str) -> Option<IpNet> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) => (a.parse::<IpAddr>().ok()?, p.parse::<u8>().ok()?),
            None => {
                let addr = text.parse::<IpAddr>().ok()?;
                (addr, if addr.is_ipv4() { 32 } else { 128 })
            }
        };
        let max = if addr.is_ipv4() { 32 } else { 128 };
        (prefix <= max).then_some(IpNet { addr, prefix })
    }
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix) };
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - self.prefix) };
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            (IpAddr::V4(_), IpAddr::V6(ip)) => ip.to_ipv4_mapped().is_some_and(|v4| self.contains(IpAddr::V4(v4))),
            _ => false,
        }
    }
}

/// Text that the original quick search treated as a phrase.
pub fn is_plain(text: &str) -> bool {
    if text.contains([':', '=', '<', '>', '"', '(', ')']) {
        return false;
    }
    !text.split_whitespace().any(|word| {
        matches!(word, "AND" | "OR" | "NOT" | "E" | "OU" | "NÃO" | "NAO" | "&&" | "||")
            || (word.len() > 1 && (word.starts_with('-') || word.starts_with('!')))
    })
}

pub struct Options<'a> {
    pub threats: Option<&'a std::sync::Arc<crate::threats::CompiledCatalog>>,
    /// Resolve `deteccao:<id>` against the active rule set. Off while the
    /// rules themselves are compiled, so a rule can never reference a rule.
    pub detections: bool,
}

pub fn compile(text: &str) -> Result<Expr, String> {
    compile_with(text, &Options { threats: None, detections: true })
}

/// Conditions inside detection and Sigma rules.
pub fn compile_rule(text: &str) -> Result<Expr, String> {
    compile_with(text, &Options { threats: None, detections: false })
}

pub fn compile_with(text: &str, options: &Options<'_>) -> Result<Expr, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Expr::All);
    }
    if trimmed.len() > 1_000_000 {
        return Err("A consulta excede 1 MB.".into());
    }
    if is_plain(trimmed) {
        return Ok(Expr::Term(Term { field: None, matcher: Matcher::Contains(trimmed.to_lowercase()) }));
    }
    let mut parser = Parser { chars: trimmed.chars().collect(), pos: 0, options, depth: 0 };
    let expr = parser.parse_or()?;
    parser.skip_ws();
    if parser.pos < parser.chars.len() {
        return Err(parser.error(if parser.chars[parser.pos] == ')' {
            "Parêntese fechado sem abertura correspondente"
        } else {
            "Trecho inesperado"
        }));
    }
    Ok(simplify(expr))
}

fn simplify(expr: Expr) -> Expr {
    match expr {
        Expr::And(mut items) if items.len() == 1 => simplify(items.remove(0)),
        Expr::Or(mut items) if items.len() == 1 => simplify(items.remove(0)),
        Expr::And(items) => Expr::And(items.into_iter().map(simplify).collect()),
        Expr::Or(items) => Expr::Or(items.into_iter().map(simplify).collect()),
        Expr::Not(inner) => Expr::Not(Box::new(simplify(*inner))),
        other => other,
    }
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    options: &'a Options<'a>,
    depth: usize,
}

const TEXT_COLUMNS: &[&str] = &["message", "_all", "raw", "description", "name", "@cmdline", "@url", "@user_agent", "@file"];

fn resolve_field(name: &str) -> Field {
    let lower = name.to_lowercase();
    let standard = match lower.as_str() {
        "nivel" | "nível" | "level" | "severidade" => Some("level"),
        "origem" | "source" | "fonte" => Some("source"),
        "codigo" | "código" | "code" | "evento" | "eventid" => Some("code"),
        "mensagem" | "msg" | "message" => Some("message"),
        "nome" | "name" => Some("name"),
        "descricao" | "descrição" | "description" => Some("description"),
        "data" | "hora" | "horario" | "horário" | "timestamp" | "time" => Some("timestamp"),
        "bruto" | "raw" => Some("raw"),
        "tudo" | "_all" | "*" => Some("_all"),
        _ => None,
    };
    let column = standard.map(str::to_string).unwrap_or_else(|| name.to_string());
    let role = if column.starts_with('@') {
        entities::role_of_column(&column)
    } else if standard.is_none() {
        entities::role_alias(name)
    } else {
        None
    };
    let text = TEXT_COLUMNS.contains(&column.as_str());
    Field { name: column, role, text, ci: false }
}

fn is_field_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '@' | '-' | '$')
}

fn is_keyword(word: &str, set: &[&str]) -> bool {
    set.contains(&word)
}

const OR_WORDS: &[&str] = &["OR", "OU", "||"];
const AND_WORDS: &[&str] = &["AND", "E", "&&"];
const NOT_WORDS: &[&str] = &["NOT", "NÃO", "NAO"];

impl Parser<'_> {
    fn error(&self, message: &str) -> String {
        let column = self.pos + 1;
        format!("{message} (posição {column}).")
    }
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn peek_word(&self) -> String {
        let mut end = self.pos;
        while end < self.chars.len() && !self.chars[end].is_whitespace() && !matches!(self.chars[end], '(' | ')') {
            end += 1;
        }
        self.chars[self.pos..end].iter().collect()
    }
    fn take_keyword(&mut self, set: &[&str]) -> bool {
        let word = self.peek_word();
        if is_keyword(&word, set) {
            let next = self.pos + word.chars().count();
            // A keyword must stand alone ("OR" but not "ORACLE").
            if next >= self.chars.len() || self.chars[next].is_whitespace() || self.chars[next] == '(' {
                self.pos = next;
                return true;
            }
        }
        false
    }
    fn parse_or(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(self.error("Consulta com aninhamento excessivo"));
        }
        let mut items = vec![self.parse_and()?];
        loop {
            self.skip_ws();
            if self.take_keyword(OR_WORDS) {
                self.skip_ws();
                items.push(self.parse_and()?);
            } else {
                break;
            }
        }
        self.depth -= 1;
        Ok(if items.len() == 1 { items.remove(0) } else { Expr::Or(items) })
    }
    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.parse_unary()?];
        loop {
            self.skip_ws();
            if self.pos >= self.chars.len() || self.peek() == Some(')') {
                break;
            }
            let word = self.peek_word();
            if is_keyword(&word, OR_WORDS) {
                break;
            }
            self.take_keyword(AND_WORDS);
            self.skip_ws();
            items.push(self.parse_unary()?);
        }
        Ok(if items.len() == 1 { items.remove(0) } else { Expr::And(items) })
    }
    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        if self.take_keyword(NOT_WORDS) {
            self.skip_ws();
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        if let Some(c) = self.peek() {
            let next = self.chars.get(self.pos + 1).copied();
            if (c == '-' || c == '!') && next.is_some_and(|n| !n.is_whitespace() && n != '=') {
                self.pos += 1;
                return Ok(Expr::Not(Box::new(self.parse_unary()?)));
            }
        }
        self.parse_primary()
    }
    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        match self.peek() {
            None => Err(self.error("Consulta incompleta")),
            Some('(') => {
                self.pos += 1;
                let inner = self.parse_or()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    return Err(self.error("Falta fechar o parêntese"));
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(')') => Err(self.error("Parêntese fechado sem abertura correspondente")),
            Some(_) => self.parse_term(),
        }
    }
    fn field_ahead(&self) -> Option<(String, usize)> {
        let start = self.pos;
        let first = *self.chars.get(start)?;
        if !(first.is_alphabetic() || first == '_' || first == '@') {
            return None;
        }
        let mut end = start;
        while end < self.chars.len() && is_field_char(self.chars[end]) {
            end += 1;
        }
        let op = *self.chars.get(end)?;
        if !matches!(op, ':' | '=' | '!' | '<' | '>') {
            return None;
        }
        if op == '!' && self.chars.get(end + 1) != Some(&'=') {
            return None;
        }
        // "http://", "C:\\" and "fe80::1" are values, not fields.
        if op == ':' {
            let after = self.chars.get(end + 1).copied();
            let second = self.chars.get(end + 2).copied();
            if matches!(after, Some('\\') | Some(':')) || (after == Some('/') && second == Some('/')) {
                return None;
            }
        }
        Some((self.chars[start..end].iter().collect(), end))
    }
    fn regex_literal_ahead(&self, at: usize) -> bool {
        // "campo:/regex/" versus a path: the literal closes at a term boundary.
        let mut i = at + 1;
        if self.chars.get(i) == Some(&'/') {
            return false;
        }
        while i < self.chars.len() {
            match self.chars[i] {
                '\\' => i += 2,
                '/' => {
                    let next = self.chars.get(i + 1);
                    if i > at + 1 && next.is_none_or(|c| c.is_whitespace() || *c == ')') {
                        return true;
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        false
    }
    fn read_quoted(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                '\\' => {
                    if let Some(n) = self.peek() {
                        self.pos += 1;
                        out.push(n);
                    }
                }
                '"' => return Ok(out),
                other => out.push(other),
            }
        }
        Err(self.error("Aspas sem fechamento"))
    }
    fn read_regex(&mut self) -> Result<regex::Regex, String> {
        // Same boundary rule as `regex_literal_ahead`: the literal ends at a
        // slash followed by whitespace, ')' or the end of the query.
        let open = self.pos;
        let mut i = open + 1;
        let mut close = None;
        while i < self.chars.len() {
            match self.chars[i] {
                '\\' => i += 2,
                '/' => {
                    let next = self.chars.get(i + 1);
                    if i > open + 1 && next.is_none_or(|c| c.is_whitespace() || *c == ')') {
                        close = Some(i);
                        break;
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        let Some(close) = close else {
            return Err(self.error("Expressão regular sem a barra final"));
        };
        let mut pattern = String::new();
        let mut j = open + 1;
        while j < close {
            let c = self.chars[j];
            if c == '\\' && j + 1 < close {
                // Escape pairs stay intact, except "\/" which only protects the delimiter.
                let next = self.chars[j + 1];
                if next != '/' {
                    pattern.push('\\');
                }
                pattern.push(next);
                j += 2;
                continue;
            }
            pattern.push(c);
            j += 1;
        }
        self.pos = close + 1;
        regex::RegexBuilder::new(&pattern)
            .case_insensitive(true)
            .size_limit(1 << 22)
            .build()
            .map_err(|e| format!("Expressão regular inválida: {e}"))
    }
    fn read_bare(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == ')' || (c == '(' && self.pos > start) {
                break;
            }
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }
    fn parse_term(&mut self) -> Result<Expr, String> {
        if let Some((name, end)) = self.field_ahead() {
            self.pos = end;
            let op = self.read_op();
            let field = resolve_field(&name);
            return self.parse_value(field, op);
        }
        match self.peek() {
            Some('"') => {
                let text = self.read_quoted()?;
                Ok(Expr::Term(Term { field: None, matcher: Matcher::Contains(text.to_lowercase()) }))
            }
            Some('/') if self.regex_literal_ahead(self.pos) => {
                let re = self.read_regex()?;
                Ok(Expr::Term(Term { field: None, matcher: Matcher::Regex(re) }))
            }
            _ => {
                let word = self.read_bare();
                if word.is_empty() {
                    return Err(self.error("Termo vazio"));
                }
                if word == "*" {
                    return Ok(Expr::All);
                }
                if word.contains(['*', '?']) {
                    return Ok(Expr::Term(Term { field: None, matcher: Matcher::Wildcard(wildcard(&word, false)?) }));
                }
                Ok(Expr::Term(Term { field: None, matcher: Matcher::Contains(word.to_lowercase()) }))
            }
        }
    }
    fn read_op(&mut self) -> &'static str {
        let two: String = self.chars[self.pos..(self.pos + 2).min(self.chars.len())].iter().collect();
        let op = match two.as_str() {
            ">=" => ">=",
            "<=" => "<=",
            "!=" => "!=",
            "==" => "=",
            _ => match self.chars[self.pos] {
                ':' => ":",
                '=' => "=",
                '>' => ">",
                _ => "<",
            },
        };
        self.pos += if op.len() == 2 || two == "==" { 2 } else { 1 };
        op
    }
    fn parse_value(&mut self, field: Field, op: &'static str) -> Result<Expr, String> {
        if self.peek().is_none_or(|c| c.is_whitespace() || c == ')') {
            return Err(self.error(&format!("Informe um valor para {}", field.name)));
        }
        if self.peek() == Some('(') && op == ":" {
            self.pos += 1;
            let mut items = Vec::new();
            loop {
                self.skip_ws();
                match self.peek() {
                    None => return Err(self.error("Falta fechar a lista de valores")),
                    Some(')') => {
                        self.pos += 1;
                        break;
                    }
                    _ => {}
                }
                if self.take_keyword(OR_WORDS) || self.peek() == Some(',') {
                    if self.peek() == Some(',') {
                        self.pos += 1;
                    }
                    continue;
                }
                let (value, quoted) = self.read_value_token()?;
                items.push((value, quoted));
            }
            if items.is_empty() {
                return Err(self.error("Lista de valores vazia"));
            }
            return self.list_expr(field, items);
        }
        if op == ":" && self.peek() == Some('/') && self.regex_literal_ahead(self.pos) {
            let re = self.read_regex()?;
            return Ok(Expr::Term(Term { field: Some(field), matcher: Matcher::Regex(re) }));
        }
        let (value, quoted) = self.read_value_token()?;
        let matcher = match op {
            "=" => Matcher::Exact(value),
            "!=" => {
                let inner = self.smart(&field, &value, true)?;
                return Ok(Expr::Not(Box::new(Expr::Term(Term { field: Some(field), matcher: inner }))));
            }
            ">" | ">=" | "<" | "<=" => {
                let number = number_for(&field.name, &value)
                    .ok_or_else(|| self.error(&format!("Valor numérico inválido: {value}")))?;
                let cmp = match op {
                    ">" => Cmp::Gt,
                    ">=" => Cmp::Gte,
                    "<" => Cmp::Lt,
                    _ => Cmp::Lte,
                };
                Matcher::Cmp(cmp, number)
            }
            _ => self.smart(&field, &value, quoted)?,
        };
        Ok(Expr::Term(Term { field: Some(field), matcher }))
    }
    fn read_value_token(&mut self) -> Result<(String, bool), String> {
        if self.peek() == Some('"') {
            return Ok((self.read_quoted()?, true));
        }
        if self.peek() == Some('[') {
            // [a TO b]
            let start = self.pos;
            while let Some(c) = self.peek() {
                self.pos += 1;
                if c == ']' {
                    break;
                }
            }
            let inner: String = self.chars[start + 1..self.pos.saturating_sub(1)].iter().collect();
            if let Some((a, b)) = inner.split_once(" TO ").or_else(|| inner.split_once(" ATÉ ")) {
                return Ok((format!("{}..{}", a.trim(), b.trim()), false));
            }
            return Ok((inner, false));
        }
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == ')' || c == ',' {
                break;
            }
            self.pos += 1;
        }
        let value: String = self.chars[start..self.pos].iter().collect();
        if value.is_empty() {
            return Err(self.error("Valor vazio"));
        }
        Ok((value, false))
    }
    fn smart(&self, field: &Field, value: &str, quoted: bool) -> Result<Matcher, String> {
        if field.name == "level" {
            return Ok(Matcher::Level(crate::sources::normalize_level(value)));
        }
        if matches!(field.name.as_str(), "regra" | "rule" | "ameaca" | "ameaça" | "threat") {
            let matcher = crate::threats::matcher(value, self.options.threats.cloned())?;
            return Ok(Matcher::Threat(Threat(matcher)));
        }
        if matches!(field.name.as_str(), "deteccao" | "detecção" | "detection") {
            if !self.options.detections {
                return Err("deteccao: não pode ser usado dentro de uma regra.".into());
            }
            let set = crate::detections::ruleset()?;
            let index = set
                .rules
                .iter()
                .position(|r| r.def.id == value)
                .ok_or_else(|| format!("Regra de detecção não encontrada: {value}."))?;
            return Ok(Matcher::Detection(Detection(set, index)));
        }
        if !quoted {
            if value == "*" {
                return Ok(Matcher::Exists);
            }
            if value.contains('/') && !value.starts_with('/') {
                if let Some(net) = IpNet::parse(value) {
                    return Ok(Matcher::Cidr(vec![net]));
                }
            }
            if let Some((a, b)) = value.split_once("..") {
                if let (Some(lo), Some(hi)) = (number_for(&field.name, a), number_for(&field.name, b)) {
                    if lo > hi {
                        return Err(format!("O início do intervalo deve ser menor que o fim em {}.", field.name));
                    }
                    return Ok(Matcher::Range(lo, hi));
                }
            }
            if value.contains(['*', '?']) {
                return Ok(Matcher::Wildcard(wildcard(value, true)?));
            }
        }
        Ok(if field.text || field.name == "_all" {
            Matcher::Contains(value.to_lowercase())
        } else {
            Matcher::Equals(value.to_lowercase())
        })
    }
    fn list_expr(&self, field: Field, items: Vec<(String, bool)>) -> Result<Expr, String> {
        let simple = !field.text
            && field.name != "level"
            && items.iter().all(|(v, quoted)| {
                *quoted || !(v.contains(['*', '?']) || v.contains("..") || (v.contains('/') && IpNet::parse(v).is_some()) || v == "*")
            });
        if simple {
            return Ok(Expr::Term(Term {
                field: Some(field),
                matcher: Matcher::Set(items.into_iter().map(|(v, _)| v.to_lowercase()).collect()),
            }));
        }
        let mut nets = Vec::new();
        let mut rest = Vec::new();
        for (value, quoted) in items {
            match self.smart(&field, &value, quoted)? {
                Matcher::Cidr(mut found) => nets.append(&mut found),
                other => rest.push(Expr::Term(Term { field: Some(field.clone()), matcher: other })),
            }
        }
        if !nets.is_empty() {
            rest.push(Expr::Term(Term { field: Some(field), matcher: Matcher::Cidr(nets) }));
        }
        Ok(if rest.len() == 1 { rest.remove(0) } else { Expr::Or(rest) })
    }
}

fn wildcard(pattern: &str, anchored: bool) -> Result<regex::Regex, String> {
    let mut re = String::from(if anchored { "(?is)^" } else { "(?is)" });
    for c in pattern.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            other => re.push_str(&regex::escape(&other.to_string())),
        }
    }
    if anchored {
        re.push('$');
    }
    regex::Regex::new(&re).map_err(|e| format!("Curinga inválido: {e}"))
}

fn number_for(field: &str, value: &str) -> Option<f64> {
    let value = value.trim();
    if let Ok(n) = value.parse::<f64>() {
        return n.is_finite().then_some(n);
    }
    if field == "timestamp" {
        return crate::sources::parse_timestamp(value).map(|ms| ms as f64);
    }
    crate::analysis::parse_num_unit(value).map(|(n, _)| n).filter(|n| n.is_finite())
}

// ---------------------------------------------------------------- evaluation

const SKIP_IN_FREE_TEXT: &[&str] = &["arquivo", "caminho"];

fn contains_ci(hay: &str, needle_lower: &str) -> bool {
    crate::query::ci_contains_bytes(hay.as_bytes(), needle_lower.as_bytes())
}

/// Roles whose canonical value is copied verbatim from the record.
pub(crate) fn literal_role(role: Role) -> bool {
    !matches!(role, Role::Action | Role::Outcome | Role::SrcScope | Role::DstScope | Role::Tool)
}

fn any_value(ev: &Event, test: &dyn Fn(&str) -> bool) -> bool {
    if test(&ev.message) || test(&ev.source) || test(&ev.code) || test(&ev.name) || test(&ev.description) {
        return true;
    }
    ev.fields.iter().any(|(key, value)| {
        if SKIP_IN_FREE_TEXT.contains(&key.as_str()) {
            return false;
        }
        match value {
            Value::String(s) => test(s),
            Value::Number(n) => test(&n.to_string()),
            Value::Bool(b) => test(if *b { "true" } else { "false" }),
            Value::Array(_) | Value::Object(_) => test(&value.to_string()),
            Value::Null => false,
        }
    })
}

/// Per-event evaluation context: canonical roles are computed once and
/// shared by every term and rule evaluated against the same event.
pub struct Ctx<'a> {
    pub ev: &'a Event,
    fields: std::cell::OnceCell<[Option<Cow<'a, str>>; 19]>,
    fallbacks: [std::cell::OnceCell<Option<Cow<'a, str>>>; 19],
    action: std::cell::OnceCell<(Option<&'static str>, Option<&'static str>)>,
    tool: std::cell::OnceCell<Option<&'static str>>,
}

impl<'a> Ctx<'a> {
    pub fn new(ev: &'a Event) -> Self {
        Ctx {
            ev,
            fields: std::cell::OnceCell::new(),
            fallbacks: Default::default(),
            action: std::cell::OnceCell::new(),
            tool: std::cell::OnceCell::new(),
        }
    }
    pub fn role(&self, role: Role) -> Option<&str> {
        match role {
            Role::Action => return self.action_outcome().0,
            Role::Outcome => return self.action_outcome().1,
            Role::SrcScope => {
                return self.role(Role::SrcIp).and_then(entities::parse_ip).map(entities::ip_scope)
            }
            Role::DstScope => {
                return self.role(Role::DstIp).and_then(entities::parse_ip).map(entities::ip_scope)
            }
            Role::Tool => {
                return *self.tool.get_or_init(|| {
                    self.role(Role::UserAgent)
                        .and_then(entities::tool_in_user_agent)
                        .or_else(|| self.role(Role::Process).and_then(entities::tool_in_process))
                        .or_else(|| self.role(Role::CommandLine).and_then(entities::tool_in_process))
                        .map(|t| t.name)
                })
            }
            _ => {}
        }
        let index = role as usize;
        let fields = self.fields.get_or_init(|| entities::scan_fields(self.ev));
        if let Some(found) = fields[index].as_deref() {
            return Some(found);
        }
        self.fallbacks[index]
            .get_or_init(|| entities::fallback_value(self.ev, role))
            .as_deref()
    }
    fn action_outcome(&self) -> (Option<&'static str>, Option<&'static str>) {
        *self.action.get_or_init(|| entities::action_outcome(self.ev))
    }
    /// Value of a column or role by name (grouping keys, placeholders).
    pub fn get(&self, column: &FieldRef) -> Option<Cow<'a, str>> {
        self.field(&column.0)
    }
    fn field(&self, field: &Field) -> Option<Cow<'a, str>> {
        let name = field.name.as_str();
        if let Some(role) = name.strip_prefix('@').and_then(|_| entities::role_of_column(name)) {
            if !self.ev.fields.contains_key(name) {
                return self.role(role).map(|v| Cow::Owned(v.to_string()));
            }
        }
        if let Some(found) = self.ev.col_ref(name) {
            return Some(found);
        }
        if field.ci {
            if let Some((_, value)) = self.ev.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
                return Some(match value {
                    Value::String(s) => Cow::Borrowed(s.as_str()),
                    other => Cow::Owned(other.to_string()),
                });
            }
        }
        field.role.and_then(|role| self.role(role)).map(|v| Cow::Owned(v.to_string()))
    }
    fn number(&self, field: &Field) -> Option<f64> {
        if field.name == "timestamp" {
            return self.ev.timestamp.map(|t| t as f64);
        }
        let text = self.field(field)?;
        let text = text.trim();
        text.parse::<f64>()
            .ok()
            .or_else(|| crate::analysis::parse_num_unit(text).map(|(n, _)| n))
            .filter(|n| n.is_finite())
    }
}

impl Expr {
    pub fn matches(&self, ev: &Event) -> bool {
        self.matches_ctx(&Ctx::new(ev))
    }

    pub fn matches_ctx(&self, ctx: &Ctx<'_>) -> bool {
        match self {
            Expr::All => true,
            Expr::And(items) => items.iter().all(|e| e.matches_ctx(ctx)),
            Expr::Or(items) => items.iter().any(|e| e.matches_ctx(ctx)),
            Expr::Not(inner) => !inner.matches_ctx(ctx),
            Expr::Term(term) => term.matches(ctx),
        }
    }

    /// Decides from the raw line and index metadata when possible:
    /// Some(true) pass, Some(false) fail, None needs the parsed event.
    pub fn line_check(&self, meta: &LineMeta, line: &[u8], escaped: bool) -> Option<bool> {
        match self {
            Expr::All => Some(true),
            Expr::And(items) => {
                let mut unknown = false;
                for item in items {
                    match item.line_check(meta, line, escaped) {
                        Some(false) => return Some(false),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown { None } else { Some(true) }
            }
            Expr::Or(items) => {
                let mut unknown = false;
                for item in items {
                    match item.line_check(meta, line, escaped) {
                        Some(true) => return Some(true),
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if unknown { None } else { Some(false) }
            }
            Expr::Not(inner) => inner.line_check(meta, line, escaped).map(|v| !v),
            Expr::Term(term) => term.line_check(meta, line, escaped),
        }
    }

    /// Free-text needles, which could also match catalog enrichment.
    pub fn free_text_needles(&self, out: &mut Vec<String>) {
        match self {
            Expr::And(items) | Expr::Or(items) => items.iter().for_each(|e| e.free_text_needles(out)),
            Expr::Not(inner) => inner.free_text_needles(out),
            Expr::Term(Term { field: None, matcher: Matcher::Contains(needle) }) => out.push(needle.clone()),
            _ => {}
        }
    }

    /// Lowercase literals of which at least one must occur in the event's
    /// values for the expression to match; `None` when no such set exists.
    pub fn required_literals(&self) -> Option<Vec<String>> {
        match self {
            Expr::All | Expr::Not(_) => None,
            Expr::And(items) => items
                .iter()
                .filter_map(|e| e.required_literals())
                .min_by_key(|set| (set.len(), usize::MAX - set.iter().map(|s| s.len()).min().unwrap_or(0))),
            Expr::Or(items) => {
                let mut all = Vec::new();
                for item in items {
                    all.extend(item.required_literals()?);
                }
                Some(all)
            }
            Expr::Term(term) => term.required_literals(),
        }
    }
}

fn longest_literal(pattern: &str) -> Option<String> {
    pattern
        .split(['*', '?'])
        .max_by_key(|chunk| chunk.len())
        .map(str::to_lowercase)
        .filter(|chunk| chunk.chars().count() >= 3)
}

impl Term {
    fn matches(&self, ctx: &Ctx<'_>) -> bool {
        let ev = ctx.ev;
        let Some(field) = &self.field else {
            return match &self.matcher {
                Matcher::Contains(needle) => any_value(ev, &|v| contains_ci(v, needle)),
                Matcher::Wildcard(re) | Matcher::Regex(re) => any_value(ev, &|v| re.is_match(v)),
                _ => false,
            };
        };
        if field.name == "_all" {
            return match &self.matcher {
                Matcher::Contains(needle) | Matcher::Equals(needle) => any_value(ev, &|v| contains_ci(v, needle)) || contains_ci(&ev.raw, needle),
                Matcher::Wildcard(re) | Matcher::Regex(re) => any_value(ev, &|v| re.is_match(v)) || re.is_match(&ev.raw),
                Matcher::Exact(value) => any_value(ev, &|v| v == value),
                Matcher::Threat(threat) => threat.0.matches(ev),
                Matcher::Detection(d) => d.0.rules[d.1].matches(ev),
                _ => false,
            };
        }
        match &self.matcher {
            Matcher::Threat(threat) => threat.0.matches(ev),
            Matcher::Detection(d) => d.0.rules[d.1].matches(ev),
            Matcher::Cmp(cmp, bound) => ctx.number(field).is_some_and(|n| match cmp {
                Cmp::Gt => n > *bound,
                Cmp::Gte => n >= *bound,
                Cmp::Lt => n < *bound,
                Cmp::Lte => n <= *bound,
            }),
            Matcher::Range(lo, hi) => ctx.number(field).is_some_and(|n| n >= *lo && n <= *hi),
            _ => {
                let Some(value) = ctx.field(field) else { return false };
                match &self.matcher {
                    Matcher::Contains(needle) => contains_ci(&value, needle),
                    Matcher::Equals(needle) => {
                        let v = value.trim();
                        v.len() == needle.len() && v.eq_ignore_ascii_case(needle) || v.to_lowercase() == *needle
                    }
                    Matcher::Exact(expected) => value.as_ref() == expected,
                    Matcher::Wildcard(re) | Matcher::Regex(re) => re.is_match(&value),
                    Matcher::Cidr(nets) => entities::parse_ip(&value).is_some_and(|ip| nets.iter().any(|n| n.contains(ip))),
                    Matcher::Exists => !value.trim().is_empty(),
                    Matcher::Set(set) => {
                        let v = value.trim();
                        set.contains(v) || set.contains(&v.to_lowercase())
                    }
                    Matcher::Level(label) => value.as_ref() == label,
                    _ => false,
                }
            }
        }
    }

    fn required_literals(&self) -> Option<Vec<String>> {
        let literal = |field: &Option<Field>| match field {
            None => true,
            Some(f) if f.name == "_all" => true,
            Some(f) => {
                !matches!(f.name.as_str(), "level" | "timestamp" | "name" | "description" | "arquivo" | "caminho")
                    && f.role.is_none_or(literal_role)
                    && entities::role_of_column(&f.name).is_none_or(literal_role)
            }
        };
        if !literal(&self.field) {
            return None;
        }
        let usable = |v: &str| v.chars().count() >= 3 && !v.contains(':');
        match &self.matcher {
            Matcher::Contains(v) | Matcher::Equals(v) => usable(v).then(|| vec![v.to_lowercase()]),
            Matcher::Exact(v) => usable(v).then(|| vec![v.to_lowercase()]),
            Matcher::Set(values) => {
                let list: Vec<String> = values.iter().map(|v| v.to_lowercase()).collect();
                list.iter().all(|v| usable(v)).then_some(list)
            }
            Matcher::Wildcard(re) => {
                // Recover the literal chunks from the anchored wildcard source.
                let source = re.as_str().trim_start_matches("(?is)^").trim_start_matches("(?is)").trim_end_matches('$');
                let plain: String = source.replace(".*", "*").replace('.', "?");
                let unescaped = plain.replace('\\', "");
                longest_literal(&unescaped).filter(|v| !v.contains(':')).map(|v| vec![v])
            }
            _ => None,
        }
    }

    fn line_check(&self, meta: &LineMeta, line: &[u8], escaped: bool) -> Option<bool> {
        match (&self.field, &self.matcher) {
            (None, Matcher::Contains(needle)) => {
                if escaped || needle.is_empty() {
                    return None;
                }
                // Values are substrings of the raw record; absence is certain.
                if crate::query::ci_contains_bytes(line, needle.as_bytes()) { None } else { Some(false) }
            }
            (Some(field), Matcher::Level(label)) if field.name == "level" => {
                let class = label_class(label)?;
                if meta.level == LV_OTHER {
                    None
                } else {
                    Some(meta.level == class)
                }
            }
            (Some(field), Matcher::Cmp(cmp, bound)) if field.name == "timestamp" => {
                if meta.ts == 0 {
                    return Some(false);
                }
                let t = meta.ts as f64;
                Some(match cmp {
                    Cmp::Gt => t > *bound,
                    Cmp::Gte => t >= *bound,
                    Cmp::Lt => t < *bound,
                    Cmp::Lte => t <= *bound,
                })
            }
            (Some(field), Matcher::Range(lo, hi)) if field.name == "timestamp" => {
                if meta.ts == 0 {
                    return Some(false);
                }
                Some((meta.ts as f64) >= *lo && (meta.ts as f64) <= *hi)
            }
            (Some(field), Matcher::Set(_)) | (Some(field), Matcher::Equals(_)) if field.role.is_some_and(literal_role) => {
                // An entity value appears literally in the record; a missing
                // candidate excludes the event without parsing it.
                if escaped {
                    return None;
                }
                let candidates: Vec<&String> = match &self.matcher {
                    Matcher::Set(set) => set.iter().collect(),
                    Matcher::Equals(v) => vec![v],
                    _ => return None,
                };
                if candidates.len() > 64 || candidates.iter().any(|c| c.contains(':')) {
                    return None;
                }
                if candidates.iter().any(|c| crate::query::ci_contains_bytes(line, c.as_bytes())) { None } else { Some(false) }
            }
            _ => None,
        }
    }
}

/// Resolved column reference, reusable across events.
pub struct FieldRef(Field);

pub fn field_ref(name: &str) -> FieldRef {
    FieldRef(resolve_field(name))
}

// ---------------------------------------------------------------- builders

/// Condition built programmatically (Sigma conversion).
pub enum Spec {
    Contains(String),
    StartsWith(String),
    EndsWith(String),
    Equals(String),
    /// Sigma wildcard pattern: `*`, `?`, `\*` escapes.
    Wildcard(String),
    Regex(String),
    Cidr(String),
    Exists,
    Gt(f64),
    Gte(f64),
    Lt(f64),
    Lte(f64),
}

pub fn and(items: Vec<Expr>) -> Expr {
    if items.len() == 1 { items.into_iter().next().unwrap() } else { Expr::And(items) }
}
pub fn or(items: Vec<Expr>) -> Expr {
    if items.len() == 1 { items.into_iter().next().unwrap() } else { Expr::Or(items) }
}
pub fn not(item: Expr) -> Expr {
    Expr::Not(Box::new(item))
}

fn sigma_wildcard(pattern: &str) -> Result<regex::Regex, String> {
    let mut re = String::from("(?is)^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if matches!(chars.peek(), Some('*') | Some('?') | Some('\\')) => {
                re.push_str(&regex::escape(&chars.next().unwrap().to_string()));
            }
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            other => re.push_str(&regex::escape(&other.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re).map_err(|e| format!("Curinga inválido: {e}"))
}

/// Field term for converted rules; `field = None` searches every value.
pub fn term(field: Option<&str>, role: Option<Role>, spec: Spec) -> Result<Expr, String> {
    let field = field.map(|name| {
        let mut resolved = resolve_field(name);
        resolved.ci = true;
        if role.is_some() {
            resolved.role = role;
        }
        resolved
    });
    let matcher = match spec {
        Spec::Contains(v) => Matcher::Contains(v.to_lowercase()),
        Spec::Equals(v) => {
            if v.contains(['*', '?']) {
                Matcher::Wildcard(sigma_wildcard(&v)?)
            } else {
                Matcher::Equals(v.to_lowercase())
            }
        }
        Spec::StartsWith(v) => Matcher::Wildcard(sigma_wildcard(&format!("{v}*"))?),
        Spec::EndsWith(v) => Matcher::Wildcard(sigma_wildcard(&format!("*{v}"))?),
        Spec::Wildcard(v) => Matcher::Wildcard(sigma_wildcard(&v)?),
        Spec::Regex(v) => Matcher::Regex(
            regex::RegexBuilder::new(&v)
                .size_limit(1 << 22)
                .build()
                .map_err(|e| format!("Expressão regular inválida: {e}"))?,
        ),
        Spec::Cidr(v) => Matcher::Cidr(vec![IpNet::parse(&v).ok_or_else(|| format!("Rede inválida: {v}"))?]),
        Spec::Exists => Matcher::Exists,
        Spec::Gt(n) => Matcher::Cmp(Cmp::Gt, n),
        Spec::Gte(n) => Matcher::Cmp(Cmp::Gte, n),
        Spec::Lt(n) => Matcher::Cmp(Cmp::Lt, n),
        Spec::Lte(n) => Matcher::Cmp(Cmp::Lte, n),
    };
    // Free-text contains without a field uses the same semantics as the search box.
    Ok(Expr::Term(Term { field, matcher }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(message: &str, fields: Value) -> Event {
        let mut e = Event::empty();
        e.message = message.into();
        e.raw = message.into();
        if let Value::Object(map) = fields {
            e.fields = map;
        }
        e
    }

    #[test]
    fn detection_terms_select_rule_records() {
        let failure = ev("Failed password for root from 45.90.12.3 port 22 ssh2", json!({}));
        let accepted = ev("Accepted password for root from 45.90.12.3 port 22 ssh2", json!({}));
        let expr = compile("deteccao:\"auth.bruteforce.source\" AND @src_ip:\"45.90.12.3\"").unwrap();
        assert!(expr.matches(&failure));
        assert!(!expr.matches(&accepted));
        assert!(compile("deteccao:regra.inexistente").is_err());
        // Rules cannot reference rules.
        assert!(compile_rule("deteccao:auth.bruteforce.source").is_err());
    }

    #[test]
    fn plain_text_keeps_phrase_semantics() {
        let e = ev("Timeout for request 42", json!({}));
        assert!(compile("timeout for request").unwrap().matches(&e));
        assert!(!compile("request for timeout").unwrap().matches(&e));
        assert!(compile("/api/login").unwrap().matches(&ev("POST /api/login", json!({}))));
        assert!(compile("10:30").unwrap().matches(&ev("at 10:30 ok", json!({}))));
        // Free text also searches field values, not keys.
        let json_event = ev("ok", json!({"client": "empresa-a", "error": null}));
        assert!(compile("empresa-a").unwrap().matches(&json_event));
        assert!(!compile("client").unwrap().matches(&json_event));
    }

    #[test]
    fn fields_booleans_and_negation() {
        let mut e = ev("Failed password for root from 45.90.12.3 port 22", json!({"status": "500", "path": "/api/pay"}));
        e.level = "Erro".into();
        assert!(compile("level:error").unwrap().matches(&e));
        assert!(compile("nivel:erro AND status:500").unwrap().matches(&e));
        assert!(compile("status:(404 OR 500)").unwrap().matches(&e));
        assert!(!compile("status:(404 OR 403)").unwrap().matches(&e));
        assert!(compile("-status:404 path:/api/*").unwrap().matches(&e));
        assert!(compile("NOT level:aviso").unwrap().matches(&e));
        assert!(compile("status>=500 status<600").unwrap().matches(&e));
        assert!(compile("status:400..599").unwrap().matches(&e));
        assert!(compile("ip:45.90.0.0/16").unwrap().matches(&e));
        assert!(!compile("ip:10.0.0.0/8").unwrap().matches(&e));
        assert!(compile("user:root @src_scope:público").unwrap().matches(&e));
        assert!(compile("(status:404 OR level:erro) -\"port 23\"").unwrap().matches(&e));
        assert!(compile("message:/fail\\w+ password/").unwrap().matches(&e));
        let mut p = ev("x", json!({"Image": "C:\\Windows\\System32\\cmd.exe", "path": "/usr/bin/ssh"}));
        p.source = "h".into();
        assert!(compile(r"Image:/(^|[\\/])cmd\.exe$/").unwrap().matches(&p));
        assert!(!compile(r"path:/(^|[\\/])sh$/").unwrap().matches(&p));
        assert!(compile(r"path:/(^|[\\/])ssh$/").unwrap().matches(&p));
        assert!(compile("path=\"/api/pay\"").unwrap().matches(&e));
        assert!(!compile("path=\"/API/pay\"").unwrap().matches(&e));
        assert!(compile("path!=/x").unwrap().matches(&e));
        assert!(compile("status:*").unwrap().matches(&e));
        assert!(!compile("ausente:*").unwrap().matches(&e));
    }

    #[test]
    fn errors_are_explained() {
        assert!(compile("status:(500").unwrap_err().contains("lista"));
        assert!(compile("(a OR b").unwrap_err().contains("parêntese"));
        assert!(compile("a )").unwrap_err().contains("Parêntese"));
        assert!(compile("x:\"open").unwrap_err().contains("Aspas"));
        assert!(compile("n>abc").unwrap_err().contains("numérico"));
        assert!(compile("ip:1.2.3.4/40").is_ok()); // not a CIDR: compared as text
        assert!(compile("m:/[/").unwrap_err().contains("regular"));
    }

    #[test]
    fn line_prefilter_is_conservative() {
        let meta = LineMeta { ts: 1000, level: crate::model::LV_ERR, ..Default::default() };
        let expr = compile("timeout AND level:erro").unwrap();
        assert_eq!(expr.line_check(&meta, b"{\"msg\":\"ok\"}", false), Some(false));
        assert_eq!(expr.line_check(&meta, b"{\"msg\":\"timeout\"}", false), None);
        assert_eq!(compile("level:aviso").unwrap().line_check(&meta, b"x", false), Some(false));
        assert_eq!(compile("-timeout").unwrap().line_check(&meta, b"ok", false), Some(true));
        assert_eq!(compile("user:(alice OR bob)").unwrap().line_check(&meta, b"carol", false), Some(false));
    }

    #[test]
    fn cidr_and_sets_scale() {
        let many: Vec<String> = (0..5000).map(|i| format!("10.{}.{}.{}", i / 65536, (i / 256) % 256, i % 256)).collect();
        let query = format!("ip:({})", many.join(" OR "));
        let expr = compile(&query).unwrap();
        assert!(expr.matches(&ev("x", json!({"src_ip": "10.0.19.135"}))));
        assert!(!expr.matches(&ev("x", json!({"src_ip": "11.0.0.1"}))));
        let v6 = IpNet::parse("2001:db8::/32").unwrap();
        assert!(v6.contains("2001:db8::5".parse().unwrap()));
    }
}
