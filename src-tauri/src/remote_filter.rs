//! Text filters for remote imports, converted to Query DSL. Accepts the dashboard search
//! syntax (DQL/KQL: `campo: valor`, and, or, not) and the app's own search language
//! (`campo:valor`, E, OU, NÃO, `-`, lists, ranges, CIDR). Index mappings are unknown here,
//! so text leaves are lenient: a value of the wrong type matches nothing instead of
//! failing the shard, which would discard the whole snapshot.
use serde_json::{json, Map, Value};

const MAX_DEPTH: usize = 64;
const MAX_TEXT: usize = 64 * 1024;
/// Characters query_string reads as syntax; `*` and `?` are kept as wildcards when unescaped.
const LUCENE_SPECIAL: &str = "+-=&|><!(){}[]^\"~*?:\\/";

/// Converts a text filter; an empty one or `*` matches everything.
pub(crate) fn to_dsl(text: &str) -> Result<Value, String> {
    if text.len() > MAX_TEXT {
        return Err("O filtro excede 64 KiB.".into());
    }
    let mut parser = Parser { chars: text.chars().collect(), pos: 0, depth: 0 };
    parser.ws();
    if parser.done() {
        return Ok(json!({"match_all": {}}));
    }
    let query = parser.or()?;
    parser.ws();
    if !parser.done() {
        return Err(parser.fail(if parser.peek() == Some(')') { "Parêntese fechado sem abertura correspondente" } else { "Trecho inesperado" }));
    }
    Ok(query)
}

/// The Query DSL a text filter becomes, shown while it is typed.
#[tauri::command]
pub fn remote_filter_preview(text: String) -> Result<Value, String> {
    to_dsl(&text)
}

#[derive(Clone, Copy, PartialEq)]
enum Keyword {
    And,
    Or,
    Not,
}

/// Lowercase and/or/not as in the dashboard; uppercase E/OU/NÃO as in the app's search.
fn keyword(word: &str) -> Option<Keyword> {
    match word {
        "&&" | "E" => Some(Keyword::And),
        "||" | "OU" => Some(Keyword::Or),
        "NÃO" | "NAO" => Some(Keyword::Not),
        _ => match word.to_ascii_lowercase().as_str() {
            "and" => Some(Keyword::And),
            "or" => Some(Keyword::Or),
            "not" => Some(Keyword::Not),
            _ => None,
        },
    }
}

/// A value as typed, and its query_string form with unescaped `*` and `?` as wildcards.
#[derive(Default)]
struct Literal {
    text: String,
    pattern: String,
    wild: bool,
}
impl Literal {
    fn push(&mut self, c: char, escaped: bool) {
        self.text.push(c);
        if !escaped && (c == '*' || c == '?') {
            self.wild = true;
        } else if LUCENE_SPECIAL.contains(c) || c.is_whitespace() {
            self.pattern.push('\\');
        }
        self.pattern.push(c);
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}
impl Parser {
    fn fail(&self, message: &str) -> String {
        format!("Filtro: {message} (posição {}).", self.pos + 1)
    }
    fn done(&self) -> bool {
        self.pos >= self.chars.len()
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }
    fn peek_word(&self) -> String {
        self.chars[self.pos..]
            .iter()
            .take_while(|c| !c.is_whitespace() && **c != '(' && **c != ')')
            .collect()
    }
    fn at(&self, kind: Keyword) -> bool {
        keyword(&self.peek_word()) == Some(kind)
    }
    fn take(&mut self, kind: Keyword) -> bool {
        let word = self.peek_word();
        let found = keyword(&word) == Some(kind);
        if found {
            self.pos += word.chars().count();
        }
        found
    }
    /// `-term` and `!term` negate, as in the app's search; `!=` is an operator.
    fn negation_ahead(&self) -> bool {
        matches!(self.peek(), Some('-' | '!'))
            && self.chars.get(self.pos + 1).is_some_and(|n| !n.is_whitespace() && *n != '=')
    }
    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.fail("Aninhamento excessivo"));
        }
        Ok(())
    }

    fn or(&mut self) -> Result<Value, String> {
        self.enter()?;
        let mut items = vec![self.and()?];
        loop {
            self.ws();
            if !self.take(Keyword::Or) {
                break;
            }
            self.ws();
            items.push(self.and()?);
        }
        self.depth -= 1;
        Ok(any(items))
    }
    /// Conditions side by side are joined by and.
    fn and(&mut self) -> Result<Value, String> {
        let mut items = vec![self.unary()?];
        loop {
            self.ws();
            if self.done() || self.peek() == Some(')') || self.at(Keyword::Or) {
                break;
            }
            self.take(Keyword::And);
            self.ws();
            items.push(self.unary()?);
        }
        Ok(all(items))
    }
    fn unary(&mut self) -> Result<Value, String> {
        self.ws();
        if self.take(Keyword::Not) {
            self.ws();
            return Ok(negate(self.unary()?));
        }
        if self.negation_ahead() {
            self.pos += 1;
            return Ok(negate(self.unary()?));
        }
        self.primary()
    }
    fn primary(&mut self) -> Result<Value, String> {
        self.ws();
        match self.peek() {
            None => Err(self.fail("Filtro incompleto")),
            Some('(') => {
                self.pos += 1;
                let inner = self.or()?;
                self.ws();
                if self.peek() != Some(')') {
                    return Err(self.fail("Falta fechar o parêntese"));
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(')') => Err(self.fail("Parêntese fechado sem abertura correspondente")),
            _ if keyword(&self.peek_word()).is_some() => Err(self.fail("Falta uma condição antes do operador")),
            _ => self.term(),
        }
    }

    /// A field name followed by an operator, with optional spaces between them.
    fn field_ahead(&self) -> Option<(String, &'static str, usize)> {
        let first = self.peek()?;
        if !(first.is_alphabetic() || first == '_' || first == '@') {
            return None;
        }
        let mut end = self.pos;
        while self.chars.get(end).is_some_and(|c| c.is_alphanumeric() || "_.@-$*".contains(*c)) {
            end += 1;
        }
        let name: String = self.chars[self.pos..end].iter().collect();
        if keyword(&name).is_some() {
            return None;
        }
        let mut at = end;
        while self.chars.get(at).is_some_and(|c| c.is_whitespace()) {
            at += 1;
        }
        let next = self.chars.get(at + 1).copied();
        let (op, width) = match (self.chars.get(at).copied()?, next) {
            // IPv6 addresses, URLs and Windows paths are values, not fields.
            (':', Some(':' | '\\')) => return None,
            (':', Some('/')) if self.chars.get(at + 2) == Some(&'/') => return None,
            (':', _) => (":", 1),
            ('=', Some('=')) => ("=", 2),
            ('=', _) => ("=", 1),
            ('!', Some('=')) => ("!=", 2),
            ('>', Some('=')) => (">=", 2),
            ('<', Some('=')) => ("<=", 2),
            ('>', _) => (">", 1),
            ('<', _) => ("<", 1),
            _ => return None,
        };
        Some((name, op, at + width))
    }

    fn term(&mut self) -> Result<Value, String> {
        if let Some((field, op, end)) = self.field_ahead() {
            self.pos = end;
            self.ws();
            if self.done() || self.peek() == Some(')') {
                return Err(self.fail(&format!("Informe um valor para {field}")));
            }
            return match op {
                ":" if self.peek() == Some('(') => {
                    self.pos += 1;
                    self.list(&field)
                }
                ":" => self.field_value(&field),
                "=" => Ok(text_query(Some(&field), &self.literal(false, false)?.0, true)),
                "!=" => Ok(negate(text_query(Some(&field), &self.literal(false, false)?.0, true))),
                _ => {
                    let bound = match op { ">" => "gt", ">=" => "gte", "<" => "lt", _ => "lte" };
                    let (value, _) = self.literal(true, false)?;
                    Ok(range(&field, &[(bound, value.text.as_str())]))
                }
            };
        }
        self.no_regex()?;
        let (value, quoted) = self.literal(false, false)?;
        Ok(if quoted {
            text_query(None, &value, true)
        } else if value.wild && value.text == "*" {
            json!({"match_all": {}})
        } else if value.wild {
            wildcard_query(None, &value)
        } else {
            text_query(None, &value, false)
        })
    }
    fn field_value(&mut self, field: &str) -> Result<Value, String> {
        self.no_regex()?;
        if let Some(range) = self.bracket_range(field) {
            return Ok(range);
        }
        let (value, quoted) = self.literal(false, false)?;
        Ok(leaf(field, &value, quoted))
    }
    /// Lucene, Rust and JavaScript regular expressions differ, so none is guessed.
    fn no_regex(&self) -> Result<(), String> {
        if self.peek() != Some('/') {
            return Ok(());
        }
        let mut i = self.pos + 1;
        while i < self.chars.len() {
            match self.chars[i] {
                '\\' => i += 1,
                '/' if i > self.pos + 1 && self.chars.get(i + 1).is_none_or(|n| n.is_whitespace() || *n == ')') => {
                    return Err(self.fail("Expressões regulares não são aceitas no filtro remoto; use curingas, como web*"));
                }
                _ => {}
            }
            i += 1;
        }
        Ok(())
    }
    /// `[a TO b]` and `{a TO b}`, inclusive and exclusive; `*` leaves a side open.
    fn bracket_range(&mut self, field: &str) -> Option<Value> {
        let open = self.peek().filter(|c| *c == '[' || *c == '{')?;
        let close_at = (self.pos + 1..self.chars.len()).find(|i| matches!(self.chars[*i], ']' | '}'))?;
        let inner: String = self.chars[self.pos + 1..close_at].iter().collect();
        let (low, high) = inner.split_once(" TO ").or_else(|| inner.split_once(" ATÉ "))?;
        let close = self.chars[close_at];
        let mut bounds = Vec::new();
        if low.trim() != "*" {
            bounds.push((if open == '[' { "gte" } else { "gt" }, low.trim()));
        }
        if high.trim() != "*" {
            bounds.push((if close == ']' { "lte" } else { "lt" }, high.trim()));
        }
        let query = range(field, &bounds);
        self.pos = close_at + 1;
        Some(query)
    }

    /// Values after `campo: (`, separated by `,`, or, and, not.
    fn list(&mut self, field: &str) -> Result<Value, String> {
        self.enter()?;
        let mut items = vec![self.list_and(field)?];
        loop {
            self.ws();
            if self.peek() == Some(',') {
                self.pos += 1;
            } else if !self.take(Keyword::Or) {
                break;
            }
            self.ws();
            items.push(self.list_and(field)?);
        }
        self.ws();
        if self.peek() != Some(')') {
            return Err(self.fail("Falta fechar a lista de valores"));
        }
        self.pos += 1;
        self.depth -= 1;
        Ok(any(items))
    }
    fn list_and(&mut self, field: &str) -> Result<Value, String> {
        let mut items = vec![self.list_item(field)?];
        loop {
            self.ws();
            if !self.take(Keyword::And) {
                break;
            }
            self.ws();
            items.push(self.list_item(field)?);
        }
        Ok(all(items))
    }
    fn list_item(&mut self, field: &str) -> Result<Value, String> {
        self.ws();
        if self.take(Keyword::Not) {
            self.ws();
            return Ok(negate(self.list_item(field)?));
        }
        match self.peek() {
            None => Err(self.fail("Falta fechar a lista de valores")),
            Some('(') => {
                self.pos += 1;
                self.list(field)
            }
            Some(')' | ',') => Err(self.fail("Valor vazio na lista")),
            _ => {
                let (value, quoted) = self.literal(false, true)?;
                Ok(leaf(field, &value, quoted))
            }
        }
    }

    /// A quoted phrase, or words joined while the next one is plain text.
    fn literal(&mut self, single: bool, list: bool) -> Result<(Literal, bool), String> {
        if self.peek() == Some('"') {
            return Ok((self.quoted()?, true));
        }
        let mut value = Literal::default();
        loop {
            self.word(&mut value, list)?;
            let before = self.pos;
            self.ws();
            if single || self.pos == before || !self.joinable(list) {
                self.pos = before;
                break;
            }
            value.push(' ', true);
        }
        Ok((value, false))
    }
    fn joinable(&self, list: bool) -> bool {
        match self.peek() {
            None | Some('(' | ')' | '"') => false,
            Some(',') if list => false,
            Some(_) => keyword(&self.peek_word()).is_none() && self.field_ahead().is_none() && !self.negation_ahead(),
        }
    }
    fn word(&mut self, value: &mut Literal, list: bool) -> Result<(), String> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '(' | ')' | '"') || (list && c == ',') {
                break;
            }
            self.pos += 1;
            if c == '\\' {
                let escaped = self.peek().ok_or_else(|| self.fail("Barra invertida sem caractere"))?;
                self.pos += 1;
                value.push(escaped, true);
            } else {
                value.push(c, false);
            }
        }
        if self.pos == start {
            return Err(self.fail("Valor vazio"));
        }
        Ok(())
    }
    fn quoted(&mut self) -> Result<Literal, String> {
        self.pos += 1;
        let mut value = Literal::default();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                '"' => return Ok(value),
                '\\' if !self.done() => {
                    value.push(self.chars[self.pos], true);
                    self.pos += 1;
                }
                _ => value.push(c, true),
            }
        }
        Err(self.fail("Aspas sem fechamento"))
    }
}

/// `campo: valor`: exists for `*`, ranges for `a..b`, CIDR blocks, wildcards, or text.
fn leaf(field: &str, value: &Literal, quoted: bool) -> Value {
    if quoted {
        return text_query(Some(field), value, true);
    }
    if value.wild && value.text == "*" {
        return json!({"exists": {"field": field}});
    }
    if let Some((low, high)) = dotted_range(&value.text) {
        return range(field, &[("gte", low), ("lte", high)]);
    }
    if let Some(block) = cidr(field, &value.text) {
        return block;
    }
    if value.wild {
        wildcard_query(Some(field), value)
    } else {
        text_query(Some(field), value, false)
    }
}

/// All words, in any order, or the exact phrase. Without a field, the index default fields.
fn text_query(field: Option<&str>, value: &Literal, phrase: bool) -> Value {
    let mut query = Map::new();
    query.insert("query".into(), json!(value.text));
    if phrase {
        query.insert("type".into(), json!("phrase"));
    } else {
        query.insert("operator".into(), json!("and"));
    }
    query.insert("lenient".into(), json!(true));
    if let Some(field) = field {
        query.insert("fields".into(), json!([field]));
    }
    json!({"multi_match": query})
}

fn wildcard_query(field: Option<&str>, value: &Literal) -> Value {
    string_query(field, &value.pattern)
}

fn string_query(field: Option<&str>, pattern: &str) -> Value {
    let mut query = Map::new();
    query.insert("query".into(), json!(pattern));
    query.insert("analyze_wildcard".into(), json!(true));
    query.insert("lenient".into(), json!(true));
    if let Some(field) = field {
        query.insert("fields".into(), json!([field]));
    }
    json!({"query_string": query})
}

/// Numbers are sent as numbers; dates and date math as text.
fn scalar(text: &str) -> Value {
    if let Ok(n) = text.parse::<i64>() {
        return json!(n);
    }
    match text.parse::<f64>() {
        Ok(n) if n.is_finite() => json!(n),
        _ => json!(text),
    }
}

fn range(field: &str, bounds: &[(&str, &str)]) -> Value {
    let bounds: Map<String, Value> = bounds.iter().map(|(key, value)| ((*key).to_owned(), scalar(value))).collect();
    json!({"range": {field: bounds}})
}

/// `10..20` or `2026-10-01..2026-10-02`, as in the app's search.
fn dotted_range(text: &str) -> Option<(&str, &str)> {
    let (low, high) = text.split_once("..")?;
    let scalar_like = |side: &str| {
        let side = side.strip_prefix('-').unwrap_or(side);
        side.starts_with(|c: char| c.is_ascii_digit()) || side.starts_with("now")
    };
    (scalar_like(low) && scalar_like(high) && !high.contains("..")).then_some((low, high))
}

/// An address block. IP fields take it as a term; keyword fields, holding addresses as
/// text, match an octet-aligned IPv4 block by prefix.
fn cidr(field: &str, text: &str) -> Option<Value> {
    let (address, prefix) = text.split_once('/')?;
    let address: std::net::IpAddr = address.parse().ok()?;
    let prefix: u8 = prefix.parse().ok()?;
    let bits = if address.is_ipv4() { 32 } else { 128 };
    if prefix > bits {
        return None;
    }
    let mut block = Literal::default();
    text.chars().for_each(|c| block.push(c, true));
    let mut options = vec![string_query(Some(field), &block.pattern)];
    if let std::net::IpAddr::V4(v4) = address {
        if prefix % 8 == 0 && (8..32).contains(&prefix) {
            let kept: Vec<String> = v4.octets()[..usize::from(prefix / 8)].iter().map(u8::to_string).collect();
            options.push(string_query(Some(field), &format!("{}.*", kept.join("."))));
        }
    }
    Some(any(options))
}

/// The clauses of `{"bool": {key: [...]}}` when that is all the query holds.
fn sole(query: &Value, key: &str) -> Option<Vec<Value>> {
    let bool = query.get("bool")?.as_object()?;
    let should = key == "should";
    let expected = if should { 2 } else { 1 };
    if bool.len() != expected || (should && bool.get("minimum_should_match") != Some(&json!(1))) {
        return None;
    }
    bool.get(key)?.as_array().cloned()
}

fn all(items: Vec<Value>) -> Value {
    combine(items, "filter")
}

fn any(items: Vec<Value>) -> Value {
    combine(items, "should")
}

fn combine(mut items: Vec<Value>, key: &str) -> Value {
    if items.len() == 1 {
        return items.remove(0);
    }
    let mut flat = Vec::with_capacity(items.len());
    for item in items {
        match sole(&item, key) {
            Some(inner) => flat.extend(inner),
            None => flat.push(item),
        }
    }
    if key == "should" {
        json!({"bool": {"should": flat, "minimum_should_match": 1}})
    } else {
        json!({"bool": {"filter": flat}})
    }
}

fn negate(item: Value) -> Value {
    match sole(&item, "must_not") {
        Some(mut inner) if inner.len() == 1 => inner.remove(0),
        _ => json!({"bool": {"must_not": [item]}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(query: &str, field: Option<&str>) -> Value {
        let mut value = json!({"multi_match": {"query": query, "operator": "and", "lenient": true}});
        if let Some(field) = field {
            value["multi_match"]["fields"] = json!([field]);
        }
        value
    }
    fn phrase(query: &str, field: Option<&str>) -> Value {
        let mut value = json!({"multi_match": {"query": query, "type": "phrase", "lenient": true}});
        if let Some(field) = field {
            value["multi_match"]["fields"] = json!([field]);
        }
        value
    }
    fn pattern(query: &str, field: &str) -> Value {
        json!({"query_string": {"query": query, "analyze_wildcard": true, "lenient": true, "fields": [field]}})
    }

    #[test]
    fn dashboard_syntax_becomes_query_dsl() {
        assert_eq!(
            to_dsl("rule.level >= 10 and agent.name: web*").unwrap(),
            json!({"bool": {"filter": [{"range": {"rule.level": {"gte": 10}}}, pattern("web*", "agent.name")]}})
        );
        // Precedence: not, then and, then or; parentheses group.
        assert_eq!(
            to_dsl("a: 1 or b: 2 and not c: 3").unwrap(),
            json!({"bool": {"should": [text("1", Some("a")), {"bool": {"filter": [text("2", Some("b")), {"bool": {"must_not": [text("3", Some("c"))]}}]}}], "minimum_should_match": 1}})
        );
        assert_eq!(
            to_dsl("(a:1 OR b:2) AND c:3").unwrap(),
            json!({"bool": {"filter": [{"bool": {"should": [text("1", Some("a")), text("2", Some("b"))], "minimum_should_match": 1}}, text("3", Some("c"))]}})
        );
        // Value lists, quotes, words of one value and presence.
        assert_eq!(
            to_dsl("rule.groups: (sshd or \"authentication failed\", pam)").unwrap(),
            json!({"bool": {"should": [text("sshd", Some("rule.groups")), phrase("authentication failed", Some("rule.groups")), text("pam", Some("rule.groups"))], "minimum_should_match": 1}})
        );
        assert_eq!(to_dsl("rule.description: falha de login").unwrap(), text("falha de login", Some("rule.description")));
        assert_eq!(to_dsl("data.srcuser: *").unwrap(), json!({"exists": {"field": "data.srcuser"}}));
        assert_eq!(to_dsl("  ").unwrap(), json!({"match_all": {}}));
        assert_eq!(to_dsl("*").unwrap(), json!({"match_all": {}}));
    }

    #[test]
    fn app_search_syntax_becomes_query_dsl() {
        // Side by side means and; E, OU and NÃO only in capitals, so "e" stays a word.
        assert_eq!(
            to_dsl("agent.id:001 -rule.level:3").unwrap(),
            json!({"bool": {"filter": [text("001", Some("agent.id")), {"bool": {"must_not": [text("3", Some("rule.level"))]}}]}})
        );
        assert_eq!(
            to_dsl("x:1 OU y:2 E NÃO z:3").unwrap(),
            to_dsl("x:1 or y:2 and not z:3").unwrap()
        );
        assert_eq!(to_dsl("falha e erro").unwrap(), text("falha e erro", None));
        assert_eq!(to_dsl("\"Invalid user\"").unwrap(), phrase("Invalid user", None));
        assert_eq!(to_dsl("agent.name=web01").unwrap(), phrase("web01", Some("agent.name")));
        assert_eq!(to_dsl("agent.name != web01").unwrap(), json!({"bool": {"must_not": [phrase("web01", Some("agent.name"))]}}));
        assert_eq!(to_dsl("not not a:1").unwrap(), text("1", Some("a")));
        // Ranges in both notations, numbers as numbers and dates as text.
        assert_eq!(to_dsl("rule.level:[10 TO 15}").unwrap(), json!({"range": {"rule.level": {"gte": 10, "lt": 15}}}));
        assert_eq!(to_dsl("rule.level:[12 TO *]").unwrap(), json!({"range": {"rule.level": {"gte": 12}}}));
        assert_eq!(to_dsl("timestamp:2026-10-01..2026-10-02").unwrap(), json!({"range": {"timestamp": {"gte": "2026-10-01", "lte": "2026-10-02"}}}));
        assert_eq!(to_dsl("timestamp < now-1d").unwrap(), json!({"range": {"timestamp": {"lt": "now-1d"}}}));
        // An octet-aligned IPv4 block also matches keyword fields by prefix.
        assert_eq!(
            to_dsl("data.srcip:10.1.0.0/16").unwrap(),
            json!({"bool": {"should": [pattern("10.1.0.0\\/16", "data.srcip"), pattern("10.1.*", "data.srcip")], "minimum_should_match": 1}})
        );
        assert_eq!(to_dsl("data.srcip:2001:db8::/32").unwrap(), pattern("2001\\:db8\\:\\:\\/32", "data.srcip"));
    }

    #[test]
    fn values_are_escaped_and_mistakes_explained() {
        // Wildcards keep * and ?, everything else is literal; escaped characters stay literal.
        assert_eq!(to_dsl("full_log: *Invalid user*").unwrap(), pattern("*Invalid\\ user*", "full_log"));
        assert_eq!(to_dsl("full_log: \"*Invalid user\"").unwrap(), phrase("*Invalid user", Some("full_log")));
        assert_eq!(to_dsl("location: C:\\\\Windows\\*").unwrap(), text("C:\\Windows*", Some("location")));
        assert_eq!(to_dsl("data.url: /admin/*.php").unwrap(), pattern("\\/admin\\/*.php", "data.url"));
        // Addresses, URLs and paths are values, not fields.
        assert_eq!(to_dsl("fe80::1").unwrap(), text("fe80::1", None));
        assert_eq!(to_dsl("https://example.test/login").unwrap(), text("https://example.test/login", None));
        for (filter, message) in [
            ("rule.level:", "Informe um valor para rule.level"),
            ("a:1 and", "Filtro incompleto"),
            ("and a:1", "Falta uma condição antes do operador"),
            ("(a:1", "Falta fechar o parêntese"),
            ("a:1)", "Parêntese fechado sem abertura correspondente"),
            ("a:(1, )", "Valor vazio na lista"),
            ("a:\"open", "Aspas sem fechamento"),
            ("full_log:/inval.d/", "Expressões regulares"),
        ] {
            let error = to_dsl(filter).unwrap_err();
            assert!(error.starts_with("Filtro: ") && error.contains(message), "{filter}: {error}");
        }
        assert!(to_dsl(&"(".repeat(70)).unwrap_err().contains("Aninhamento excessivo"));
    }
}
