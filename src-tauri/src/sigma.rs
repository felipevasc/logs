//! Sigma rules converted to the local detection engine. Supported:
//! selections (maps, lists of maps, keywords), modifiers contains, startswith,
//! endswith, all, re, cidr, exists, gt/gte/lt/lte, windash, cased; conditions
//! with and/or/not, parentheses, `1 of`, `all of`, `them`; and the legacy
//! `| count() by x > n` aggregation. Unsupported constructs are reported
//! per rule instead of being approximated.
use crate::detections::{compile_rule, Compiled, RuleDef};
use crate::entities::Role;
use crate::querylang::{self, Expr, Spec};
use serde_json::Value;
use base64::Engine as _;
use std::path::{Path, PathBuf};
pub const SPECIFICATION_VERSION:&str="2.1.0";
pub const BACKEND_VERSION:&str="loginsight-sigma-2";
pub fn compatibility()->Value {
    serde_json::json!({"specification_version":SPECIFICATION_VERSION,"backend_version":BACKEND_VERSION,"reference":{"pySigma":"1.5.1","revision":"f81e4f5ace2f444f76c5de03df8e0f181f85f6cc","fixture":"sigma-reference-1","scope":"modifier/boolean AST comparisons; temporal correlations use native independent slice oracle"},"status":"explicit_subset",
        "specification":"https://sigmahq.io/sigma-specification/specification/sigma-appendix-modifiers.html",
        "modifiers":["contains","startswith","endswith","all","re","i","m","s","cidr","exists","gt","gte","lt","lte","windash","cased","fieldref","neq","base64","base64offset","utf16","utf16le","utf16be","wide"],
        "correlations":["event_count","value_count","temporal","temporal_ordered","value_sum","value_avg"],
        "chaining":"temporal and temporal_ordered consume original members of referenced findings, anchored at completion time; aliases require unanimous projection",
        "restrictions":["Unsupported modifiers, combinations and taxonomy fail import with a reason","PCRE lookaround and backreferences are unsupported by the Rust regex backend","Binary UTF16 must result in valid log text or be followed by base64","Aggregate-on-aggregate event_count/value_count is unsupported","value_percentile and generate are unsupported","FieldRef inequality requires both fields present; ordinary neq requires the selected field present","Imported severity and ATT&CK tags never assign an evidence level"]})
}

pub fn rule_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    let mut directories = 0usize;
    while let Some(path) = pending.pop() {
        if crate::operations::cancelled() || directories >= 20_000 { break; }
        directories += 1;
        let Ok(entries) = std::fs::read_dir(&path) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() { continue; }
            if kind.is_dir() {
                pending.push(p);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml")) {
                out.push(p);
            }
        }
        if out.len() > 20_000 {
            break;
        }
    }
    out.sort();
    out
}

pub fn load_dir(dir: &Path) -> (Vec<Compiled>, Vec<String>) {
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    for file in rule_files(dir) {
        match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|t| documents(&t)) {
            Ok(mut values) => docs.append(&mut values),
            Err(e) => errors.push(format!("{}: {e}", file.display())),
        }
    }
    let mut rules = Vec::new();
    for (i, doc) in docs.iter().enumerate() {
        match resolve(i, &docs, &mut Vec::new()) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(format!("{}: {e}", doc["title"].as_str().unwrap_or("Sigma"))),
        }
    }
    (rules, errors)
}
fn documents(text: &str) -> Result<Vec<Value>, String> {
    if text.len() > 2 * 1024 * 1024 {
        return Err("arquivo maior que 2 MiB".into());
    }
    let mut out = Vec::new();
    for document in yaml_serde::Deserializer::from_str(text) {
        let value: Value = serde::Deserialize::deserialize(document).map_err(|e| format!("YAML: {e}"))?;
        if value.is_null() {
            continue;
        }
        if value.get("action").is_some() {
            return Err("action: global nao suportado".into());
        }
        if value["taxonomy"].as_str().is_some_and(|name|name!="sigma"){return Err("Taxonomia Sigma exige pipeline explícito; não foi traduzida implicitamente".into());}
        if value.get("generate").is_some(){return Err("generate Sigma não suportado por este backend".into());}
        out.push(value);
    }
    Ok(out)
}
pub fn convert_text(text: &str) -> Result<Vec<Compiled>, String> {
    convert_texts([text])
}
pub fn convert_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Result<Vec<Compiled>, String> {
    let mut docs = Vec::new();
    for text in texts {
        crate::operations::check()?;
        docs.extend(documents(text)?);
        if docs.len() > 4096 { return Err("O Caso excede 4.096 regras Sigma.".into()); }
    }
    if docs.is_empty() {
        return Err("nenhuma regra encontrada".into());
    }
    let mut ids = std::collections::HashSet::new();
    for doc in &docs {
        if let Some(id) = doc["id"].as_str() {
            if !ids.insert(id) {
                return Err(format!("ID Sigma duplicado: {id}"));
            }
        }
    }
    (0..docs.len()).map(|i| { crate::operations::check()?; resolve(i, &docs, &mut Vec::new()) }).collect()
}
fn resolve(index: usize, docs: &[Value], stack: &mut Vec<usize>) -> Result<Compiled, String> {
    if stack.len()>=64{return Err("Cadeia Sigma excede 64 dependências; não foi simplificada".into());}
    if stack.contains(&index) {
        return Err("dependencia circular entre regras Sigma".into());
    }
    stack.push(index);
    let doc = &docs[index];
    let result = if doc.get("correlation").is_some() { correlation(doc, docs, stack) } else { convert(doc) };
    stack.pop();
    result
}
fn correlation(doc: &Value, docs: &[Value], stack: &mut Vec<usize>) -> Result<Compiled, String> {
    let c = doc["correlation"].as_object().ok_or("correlation deve ser objeto")?;
    for key in c.keys() {
        if !["type", "rules", "group-by", "timespan", "condition", "aliases"].contains(&key.as_str()) {
            return Err(format!("construcao de correlacao nao suportada: {key}"));
        }
    }
    let kind = c.get("type").and_then(Value::as_str).ok_or("correlation.type ausente")?;
    if !["event_count", "value_count", "temporal", "temporal_ordered", "value_sum", "value_avg"].contains(&kind) {
        return Err(format!("tipo de correlacao nao suportado: {kind}"));
    }
    let refs = c.get("rules").and_then(Value::as_array).ok_or("correlation.rules ausente")?;
    if refs.is_empty() || refs.len() > 16 {
        return Err("correlacao precisa de 1 a 16 referencias".into());
    }
    let mut rules = Vec::new();
    let mut names = Vec::new();
    for reference in refs {
        let name = reference.as_str().ok_or("referencia nao textual")?;
        let candidates: Vec<_> = docs
            .iter()
            .enumerate()
            .filter(|(_, d)| d["id"].as_str() == Some(name) || d["name"].as_str() == Some(name))
            .map(|(i, _)| i)
            .collect();
        if candidates.len() != 1 {
            return Err(format!("referencia ausente ou ambigua: {name}"));
        }
        let rule = resolve(candidates[0], docs, stack)?;
        if rule.def.kind != "single" && !kind.starts_with("temporal") {
            return Err(format!("correlacao sobre agregado nao suportada: {name}"));
        }
        rules.push(rule);
        names.push(name);
    }
    let group: Vec<String> = c
        .get("group-by")
        .and_then(Value::as_array)
        .ok_or("group-by explicito obrigatorio")?
        .iter()
        .map(|v| v.as_str().map(str::to_string).ok_or("group-by invalido".to_string()))
        .collect::<Result<_, _>>()?;
    let window = c.get("timespan").and_then(Value::as_str).ok_or("timespan obrigatorio")?.to_string();
    if crate::detections::parse_duration(&window).is_none() {
        return Err("timespan invalido".into());
    }
    let mapped = |field: &str, name: &str| -> Result<String, String> {
        match c.get("aliases").and_then(|v| v.get(field)) {
            Some(alias) => alias
                .get(name)
                .and_then(Value::as_str)
                .map(|f| field_name(f).to_string())
                .ok_or_else(|| format!("alias {field} sem mapeamento para {name}")),
            None => Ok(field_name(field).into()),
        }
    };
    let mut def = rules[0].def.clone();
    def.id =
        format!("sigma:{}", doc["id"].as_str().or(doc["name"].as_str()).ok_or("correlacao precisa de id ou name")?);
    def.name = doc["title"].as_str().ok_or("title ausente")?.into();
    def.description = doc["description"].as_str().unwrap_or("").into();
    def.severity = severity(doc["level"].as_str());
    def.evidence = policy(doc)?;
    def.provenance = metadata(doc);
    def.window = Some(window);
    def.by = group.clone();
    def.steps.clear();
    def.distinct = None;
    def.references = names.iter().map(|s| s.to_string()).collect();
    let mut exprs = Vec::new();
    if kind.starts_with("temporal") {
        if refs.len() < 2 {
            return Err("correlacao temporal requer duas regras".into());
        }
        if c.get("condition").is_some() {
            return Err("condition temporal nao suportada; exige todas as etapas".into());
        }
        def.kind = if kind == "temporal_ordered" { "sequence" } else { "temporal" }.into();
        let chained=rules.iter().any(|r|r.def.kind!="single");
        if chained {def.provenance["chain_references"]=serde_json::json!(rules.iter().map(|r|&r.def.id).collect::<Vec<_>>());}
        for (rule, name) in rules.into_iter().zip(&names) {
            def.steps.push(crate::detections::StepDef {
                by: group.iter().map(|f| mapped(f, name)).collect::<Result<_, _>>()?,
                ..Default::default()
            });
            exprs.push(if chained{Expr::All}else{rule.gated_condition()?});
        }
    } else if matches!(kind, "value_sum" | "value_avg") {
        let condition = c.get("condition").and_then(Value::as_object).ok_or("condition ausente")?;
        if condition.keys().any(|k| !["gte", "lte", "field"].contains(&k.as_str())) {
            return Err("Agregação Sigma suporta gte/lte explícitos".into());
        }
        let field = condition.get("field").and_then(Value::as_str).ok_or("Agregação exige campo numérico")?;
        let numeric = mapped(field, names[0])?;
        def.by = group.iter().map(|f| mapped(f, names[0])).collect::<Result<_, _>>()?;
        for name in &names[1..] {
            if mapped(field, name)? != numeric
                || group.iter().map(|f| mapped(f, name)).collect::<Result<Vec<_>, _>>()? != def.by
            {
                return Err("Aliases agregados incompatíveis".into());
            }
        }
        def.kind = "aggregate".into();
        def.count = Some(1);
        def.aggregate = Some(crate::detections::AggregateDef {
            field: numeric,
            operation: if kind == "value_sum" { "sum" } else { "avg" }.into(),
            gte: condition.get("gte").map(|v| v.as_f64().ok_or("gte deve ser numérico")).transpose()?,
            lte: condition.get("lte").map(|v| v.as_f64().ok_or("lte deve ser numérico")).transpose()?,
        });
        exprs.push(querylang::or(rules.into_iter().map(|r| r.gated_condition()).collect::<Result<_, _>>()?));
    } else {
        let condition = c.get("condition").and_then(Value::as_object).ok_or("condition ausente")?;
        if condition.keys().any(|k| !["gte", "gt", "field"].contains(&k.as_str()))
            || condition.contains_key("gte") == condition.contains_key("gt")
        {
            return Err("somente uma condicao gte ou gt e suportada".into());
        }
        let count = condition.get("gte").or(condition.get("gt")).and_then(Value::as_u64).ok_or("contagem invalida")?;
        def.count = Some(
            usize::try_from(count)
                .map_err(|_| "contagem excedida")?
                .checked_add(usize::from(condition.contains_key("gt")))
                .ok_or("contagem excedida")?,
        );
        def.by = group.iter().map(|f| mapped(f, names[0])).collect::<Result<_, _>>()?;
        for name in &names[1..] {
            if group.iter().map(|f| mapped(f, name)).collect::<Result<Vec<_>, _>>()? != def.by {
                return Err("aliases diferentes entre regras de contagem nao suportados".into());
            }
        }
        def.kind = if kind == "event_count" { "threshold" } else { "distinct" }.into();
        if kind == "value_count" {
            let field = condition.get("field").and_then(Value::as_str).ok_or("value_count requer field")?;
            let mapped_field = mapped(field, names[0])?;
            for name in &names[1..] {
                if mapped(field, name)? != mapped_field {
                    return Err("alias distinto incompatível".into());
                }
            }
            def.distinct = Some(mapped_field);
        }
        exprs.push(querylang::or(rules.into_iter().map(|r| r.gated_condition()).collect::<Result<_, _>>()?));
    }
    compile_rule(def, "sigma", Some(exprs))
}
fn policy(doc: &Value) -> Result<crate::evidence::Policy, String> {
    let mut policy = match doc.get("x-loginsight-evidence") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("politica de evidencia: {e}"))?,
        None => crate::evidence::Policy { maturity: "unassessed".into(), ..Default::default() },
    };
    if let Some(fp) = doc["falsepositives"].as_array() {
        policy.benign = fp.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if doc["x-loginsight-evidence"].get("version").is_none() {
        policy.version = crate::evidence::stable_id("sigma", [doc.to_string()]);
    }
    policy.validate()?;
    Ok(policy)
}
fn metadata(doc: &Value) -> Value {
    serde_json::json!({"format":"sigma","specification_version":SPECIFICATION_VERSION,"backend_version":BACKEND_VERSION,"id":doc["id"],"name":doc["name"],"status":doc["status"],"logsource":doc["logsource"],"aliases":doc["correlation"]["aliases"],"author":doc["author"],"date":doc["date"],"modified":doc["modified"],"references":doc["references"],"falsepositives":doc["falsepositives"]})
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Canonical role for common Sigma field names, used when the event does not
/// carry the field under the same name.
fn sigma_role(field: &str) -> Option<Role> {
    Some(match field.to_ascii_lowercase().as_str() {
        "image" | "newprocessname" | "processname" | "process" => Role::Process,
        "parentimage" | "parentprocessname" => Role::ParentProcess,
        "commandline" | "processcommandline" | "scriptblocktext" => Role::CommandLine,
        "user" | "targetusername" | "subjectusername" | "username" | "accountname" => Role::User,
        "sourceip" | "src_ip" | "ipaddress" | "c-ip" | "clientip" | "sourceaddress" | "src" => Role::SrcIp,
        "destinationip" | "dst_ip" | "destaddress" | "dst" => Role::DstIp,
        "destinationport" | "dst_port" | "destport" | "dpt" => Role::DstPort,
        "destinationhostname" | "queryname" | "query" | "domain" => Role::Domain,
        "computer" | "computername" | "hostname" | "workstationname" => Role::Host,
        "targetfilename" | "targetfile" | "filename" | "objectname" => Role::File,
        "hashes" | "hash" | "sha256" | "md5" | "sha1" | "imphash" => Role::Hash,
        "c-uri" | "cs-uri-stem" | "cs-uri-query" | "uri" | "url" | "c-uri-query" => Role::Url,
        "cs-user-agent" | "c-useragent" | "user-agent" | "useragent" | "cs(user-agent)" => Role::UserAgent,
        "sc-status" | "status" | "statuscode" => Role::Status,
        _ => return None,
    })
}

fn field_name(field: &str) -> &str {
    match field.to_ascii_lowercase().as_str() {
        "eventid" | "event_id" | "event.code" => "code",
        "provider_name" | "provider" => "source",
        _ => field,
    }
}

fn windash(value: &str) -> Result<Vec<String>,String> {
    let chars=value.chars().collect::<Vec<_>>();let word=|c:char|c.is_alphanumeric()||c=='_';let mut out=vec![String::new()];
    for (i,&c) in chars.iter().enumerate(){
        if matches!(c,'-'|'/') && (i==0 || !word(chars[i-1])) && chars.get(i+1).is_some_and(|c|word(*c)) {
            if out.len()>4096/5{return Err("Expansão windash excede 4096 variantes; não foi truncada".into());}
            out=out.into_iter().flat_map(|prefix|['-','/','–','—','―'].map(|dash|format!("{prefix}{dash}"))).collect();
        }else{for prefix in &mut out{prefix.push(c);}}
    }Ok(out)
}

fn value_expr(field: Option<&str>, modifiers: &[String], value: &Value) -> Result<Expr, String> {
    let has = |m: &str| modifiers.iter().any(|x| x == m);
    let supported = [
        "fieldref", "neq", "base64", "base64offset", "utf16", "utf16le", "utf16be", "wide",
        "contains",
        "startswith",
        "endswith",
        "all",
        "re",
        "cidr",
        "exists",
        "gt",
        "gte",
        "lt",
        "lte",
        "windash",
        "cased",
        "i",
        "m",
        "s",
    ];
    if let Some(m) = modifiers.iter().find(|m| !supported.contains(&m.as_str())) {
        return Err(format!("modificador não suportado: {m}"));
    }
    if modifiers.iter().collect::<std::collections::HashSet<_>>().len()!=modifiers.len(){return Err("Modificador Sigma duplicado".into());}
    let transformations=modifiers.iter().filter(|m|!matches!(m.as_str(),"all"|"neq")).map(String::as_str).collect::<Vec<_>>();
    let exclusive=["exists","cidr","gt","gte","lt","lte"];
    if exclusive.iter().any(|m|has(m)) && (transformations.len()!=1 || field.is_none()){return Err("Modificador tipado exige campo e combinação exclusiva".into());}
    if has("re") && (transformations.first()!=Some(&"re") || transformations.iter().any(|m|!matches!(*m,"re"|"i"|"m"|"s"))){return Err("Regex Sigma exige re antes dos submodificadores i/m/s".into());}
    if !has("re") && ["i","m","s"].iter().any(|m|has(m)){return Err("Submodificador de regex exige re".into());}
    if ["contains","startswith","endswith"].iter().filter(|m|has(m)).count()>1{return Err("Combinação de âncoras Sigma não suportada".into());}
    let role = field.and_then(sigma_role);
    let resolved = field.map(field_name);
    if value.is_null() {
        if !transformations.is_empty(){return Err("null com modificador de transformação não suportado".into());}
        return querylang::term(resolved, role, Spec::Null);
    }
    if has("exists") {
        let expected = value.as_bool().ok_or("exists exige booleano")?;
        let exists = querylang::term(resolved, role, Spec::Present)?;
        return Ok(if expected { exists } else { querylang::not(exists) });
    }
    let raw = text(value).ok_or("valor não escalar")?;
    if has("fieldref") && (raw.contains(['*','?'])||!value.is_string()||field.is_none()){return Err("fieldref exige nomes de campos literais".into());}
    if transformations.iter().any(|m|matches!(*m,"contains"|"startswith"|"endswith"|"windash"|"cased"|"re"|"base64"|"base64offset"|"utf16"|"utf16le"|"utf16be"|"wide")) && !value.is_string(){return Err("Modificador de texto exige valor string".into());}
    if transformations.iter().any(|m|matches!(*m,"base64"|"base64offset")) && raw.contains(['*','?']){return Err("Codificação base64 de wildcards não suportada".into());}
    if has("fieldref") {
        if modifiers.iter().any(|m|!matches!(m.as_str(),"fieldref"|"neq")) { return Err("Combinação fieldref não suportada".into()); }
        return querylang::term(resolved,role,Spec::FieldRef(field_name(&raw).to_string(),has("neq")));
    }
    let transformed = encoded_variants(&raw,modifiers)?;
    if modifiers.iter().any(|m|matches!(m.as_str(),"base64"|"base64offset"|"utf16"|"utf16le"|"utf16be"|"wide")) {
        if modifiers.iter().any(|m|!matches!(m.as_str(),"base64"|"base64offset"|"utf16"|"utf16le"|"utf16be"|"wide"|"contains"|"startswith"|"endswith"|"all"|"cased")) {
            return Err("Combinação de codificação Sigma não suportada; a regra não foi aproximada".into());
        }
        return Ok(querylang::or(transformed.into_iter().map(|v| {
            let left=if has("contains")||has("endswith"){""}else{"^"};
            let right=if has("contains")||has("startswith"){""}else{"$"};
            querylang::term(resolved,role,Spec::Regex(format!("(?s){left}{}{right}",regex::escape(&v))))
        }).collect::<Result<_,_>>()?));
    }
    let raw=transformed.into_iter().next().ok_or("Transformação Sigma vazia")?;
    let variants = if has("windash") { windash(&raw)? } else { vec![raw.clone()] };
    let mut items = Vec::new();
    for v in variants {
        let spec = if has("re") {
            let mut pattern = String::new();
            if has("i") {
                pattern.push_str("(?i)");
            }
            if has("m") || has("multiline") {
                pattern.push_str("(?m)");
            }
            if has("s") || has("dotall") {
                pattern.push_str("(?s)");
            }
            pattern.push_str(&v);
            Spec::Regex(pattern)
        } else if has("cidr") {
            Spec::Cidr(v)
        } else if has("gt") || has("gte") || has("lt") || has("lte") {
            let n: f64 = v.trim().parse().map_err(|_| format!("número inválido: {v}"))?;
            if has("gt") {
                Spec::Gt(n)
            } else if has("gte") {
                Spec::Gte(n)
            } else if has("lt") {
                Spec::Lt(n)
            } else {
                Spec::Lte(n)
            }
        } else if has("cased") {
            let pattern=if has("contains") || field.is_none() {format!("*{v}*")} else if has("startswith") {format!("{v}*")} else if has("endswith") {format!("*{v}")} else {v};
            Spec::CasedWildcard(pattern)
        } else if has("contains") {
            if v.contains(['*', '?']) {
                Spec::Wildcard(format!("*{v}*"))
            } else {
                Spec::Contains(v)
            }
        } else if has("startswith") {
            Spec::StartsWith(v)
        } else if has("endswith") {
            Spec::EndsWith(v)
        } else if field.is_none() {
            // Keywords search every value.
            if v.contains(['*', '?']) {
                Spec::Wildcard(format!("*{v}*"))
            } else {
                Spec::Contains(v)
            }
        } else {
            Spec::Equals(v)
        };
        items.push(querylang::term(resolved, role, spec)?);
    }
    let expression=querylang::or(items);
    if has("neq") {
        Ok(querylang::and(vec![querylang::term(resolved,role,Spec::Present)?,querylang::not(expression)]))
    } else { Ok(expression) }
}

fn encoded_variants(raw:&str,modifiers:&[String])->Result<Vec<String>,String> {
    let mut variants=vec![raw.as_bytes().to_vec()];
    for modifier in modifiers {
        match modifier.as_str() {
            "utf16"|"utf16le"|"utf16be"|"wide" => {
                variants=variants.into_iter().map(|bytes| {
                    let text=String::from_utf8(bytes).map_err(|_|"Ordem de codificação Sigma inválida")?;
                    let mut out=Vec::new();
                    if modifier=="utf16" {out.extend_from_slice(&[0xff,0xfe]);}
                    for c in text.encode_utf16(){out.extend_from_slice(&if modifier=="utf16be" {c.to_be_bytes()}else{c.to_le_bytes()});}
                    Ok(out)
                }).collect::<Result<_,String>>()?;
            },
            "base64" => { variants=variants.into_iter().map(|bytes|base64::engine::general_purpose::STANDARD.encode(bytes).into_bytes()).collect(); },
            "base64offset" => {
                let mut out=Vec::new();
                for bytes in variants {
                    for offset in 0usize..3 {
                        let mut input=vec![b' ';offset];input.extend_from_slice(&bytes);
                        let encoded=base64::engine::general_purpose::STANDARD.encode(input);
                        let start=(offset*8).div_ceil(6);let end=((offset+bytes.len())*8)/6;
                        if start<end {out.push(encoded[start..end].as_bytes().to_vec());}
                    }
                }
                if out.is_empty(){return Err("base64offset exige um valor não vazio".into());}
                variants=out;
            },
            _=>{},
        }
    }
    variants.into_iter().map(|bytes|String::from_utf8(bytes).map_err(|_|"A codificação binária exige base64 para ser comparada com texto de log".into())).collect()
}

fn selection_map(map: &serde_json::Map<String, Value>) -> Result<Expr, String> {
    let mut all = Vec::new();
    for (key, value) in map {
        let mut parts = key.split('|');
        let field = parts.next().unwrap_or("").trim();
        let modifiers: Vec<String> = parts.map(|m| m.trim().to_ascii_lowercase()).collect();
        let field = (!field.is_empty() && field != "keywords").then_some(field);
        let values: Vec<&Value> = match value {
            Value::Array(list) => list.iter().collect(),
            other => vec![other],
        };
        if values.is_empty() {
            return Err(format!("lista vazia em {key}"));
        }
        if modifiers.iter().any(|m|m=="all") && (!value.is_array()||values.len()<2){return Err("all exige lista com pelo menos dois valores".into());}
        let references=if modifiers.iter().any(|m|m=="fieldref"){values.iter().filter_map(|v|v.as_str()).map(|name|querylang::term(Some(field_name(name)),None,Spec::Present)).collect::<Result<Vec<_>,_>>()?}else{vec![]};
        let positive=modifiers.iter().filter(|m|m.as_str()!="neq").cloned().collect::<Vec<_>>();
        let exprs = values.into_iter().map(|v| value_expr(field, &positive, v)).collect::<Result<Vec<_>, _>>()?;
        let expr=if modifiers.iter().any(|m| m == "all") { querylang::and(exprs) } else { querylang::or(exprs) };
        all.push(if modifiers.iter().any(|m|m=="neq"){if field.is_none(){return Err("neq exige campo explícito".into());}let mut guards=references;guards.push(querylang::term(field.map(field_name),field.and_then(sigma_role),Spec::Present)?);guards.push(querylang::not(expr));querylang::and(guards)}else{expr});
    }
    if all.is_empty() {
        return Err("seleção vazia".into());
    }
    Ok(querylang::and(all))
}

fn selection(value: &Value) -> Result<Expr, String> {
    match value {
        Value::Object(map) => selection_map(map),
        Value::Array(items) => {
            let exprs = items
                .iter()
                .map(|item| match item {
                    Value::Object(map) => selection_map(map),
                    other => value_expr(None, &[], other),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if exprs.is_empty() {
                return Err("seleção vazia".into());
            }
            Ok(querylang::or(exprs))
        }
        other => value_expr(None, &[], other),
    }
}

// ------------------------------------------------------------------ condition

struct Cond<'a> {
    tokens: Vec<String>,
    pos: usize,
    selections: &'a [(String, Value)],
}

impl Cond<'_> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }
    fn next(&mut self) -> Option<String> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.and()?];
        while self.peek().is_some_and(|t| t.eq_ignore_ascii_case("or")) {
            self.pos += 1;
            items.push(self.and()?);
        }
        Ok(querylang::or(items))
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.not()?];
        while self.peek().is_some_and(|t| t.eq_ignore_ascii_case("and")) {
            self.pos += 1;
            items.push(self.not()?);
        }
        Ok(querylang::and(items))
    }
    fn not(&mut self) -> Result<Expr, String> {
        if self.peek().is_some_and(|t| t.eq_ignore_ascii_case("not")) {
            self.pos += 1;
            return Ok(querylang::not(self.not()?));
        }
        self.primary()
    }
    fn matching(&self, pattern: &str) -> Vec<&(String, Value)> {
        if pattern == "them" {
            return self.selections.iter().filter(|(name, _)| !name.starts_with('_')).collect();
        }
        let re = pattern.replace('*', ".*");
        let re = regex::Regex::new(&format!("^{re}$")).ok();
        self.selections.iter().filter(|(name, _)| re.as_ref().is_some_and(|r| r.is_match(name))).collect()
    }
    fn primary(&mut self) -> Result<Expr, String> {
        let token = self.next().ok_or("condição incompleta")?;
        match token.to_ascii_lowercase().as_str() {
            "(" => {
                let inner = self.or()?;
                if self.next().as_deref() != Some(")") {
                    return Err("parêntese sem fechamento na condição".into());
                }
                Ok(inner)
            }
            "1" | "any" | "all" => {
                if !self.peek().is_some_and(|t| t.eq_ignore_ascii_case("of")) {
                    return Err(format!("condição inesperada: {token}"));
                }
                self.pos += 1;
                let target = self.next().ok_or("condição incompleta após 'of'")?;
                let found = self.matching(&target);
                if found.is_empty() {
                    return Err(format!("nenhuma seleção corresponde a {target}"));
                }
                let exprs = found.into_iter().map(|(_, v)| selection(v)).collect::<Result<Vec<_>, _>>()?;
                Ok(if token.eq_ignore_ascii_case("all") { querylang::and(exprs) } else { querylang::or(exprs) })
            }
            _ => {
                let (_, value) = self
                    .selections
                    .iter()
                    .find(|(name, _)| *name == token)
                    .ok_or_else(|| format!("seleção inexistente na condição: {token}"))?;
                selection(value)
            }
        }
    }
}

fn tokenize(condition: &str) -> Vec<String> {
    let spaced = condition.replace('(', " ( ").replace(')', " ) ");
    spaced.split_whitespace().map(str::to_string).collect()
}

struct Aggregation {
    distinct: Option<String>,
    by: Option<String>,
    op: String,
    value: usize,
}

fn aggregation(text: &str) -> Result<Aggregation, String> {
    // count() by Field > 10   |   count(Target) by Source >= 5
    let re = regex::Regex::new(r"(?i)^\s*count\(\s*([^)]*)\)\s*(?:by\s+([A-Za-z0-9_.\-]+))?\s*(>=|>|==|=)\s*(\d+)\s*$")
        .unwrap();
    let caps = re.captures(text).ok_or_else(|| format!("agregação não suportada: {text}"))?;
    let distinct = caps.get(1).map(|m| m.as_str().trim().to_string()).filter(|s| !s.is_empty());
    let value: usize = caps[4].parse().map_err(|_| "limite inválido")?;
    Ok(Aggregation { distinct, by: caps.get(2).map(|m| m.as_str().to_string()), op: caps[3].to_string(), value })
}

fn logsource_gate(logsource: Option<&Value>) -> Result<Option<Expr>, String> {
    let Some(ls) = logsource else { return Ok(None) };
    let get = |k: &str| ls.get(k).and_then(Value::as_str).map(str::to_ascii_lowercase);
    let category = get("category");
    let product = get("product");
    let service = get("service");
    let action = match category.as_deref() {
        Some("process_creation") => Some("@action:process_start"),
        Some("network_connection") => Some("@action:network_connection"),
        Some("dns_query") | Some("dns") => Some("@action:dns_query"),
        Some("file_event") => Some("@action:file_create"),
        Some("file_delete") => Some("@action:file_delete"),
        Some("registry_add") | Some("registry_set") | Some("registry_event") | Some("registry_delete") => {
            Some("@action:registry_change")
        }
        Some("image_load") => Some("@action:image_load"),
        Some("process_access") => Some("@action:process_access"),
        Some("create_remote_thread") => Some("@action:remote_thread"),
        Some("ps_script") => Some("@action:script_execution"),
        Some("webserver") | Some("proxy") => Some("(@status:* OR @url:*)"),
        _ => None,
    };
    let product_gate = match (product.as_deref(), service.as_deref()) {
        (_, Some("security")) => Some("(source:*security* OR channel:security OR winlog.channel:security)"),
        (_, Some("sysmon")) => Some("(source:*sysmon* OR channel:*sysmon*)"),
        (_, Some("system")) => Some("(channel:system OR source:\"Service Control Manager\" OR source:*eventlog*)"),
        (_, Some("powershell")) | (_, Some("powershell-classic")) => {
            Some("(source:*powershell* OR channel:*powershell*)")
        }
        (_, Some("auditd")) => Some("type:*"),
        (_, Some("sshd")) | (_, Some("auth")) => Some("(process:sshd OR process:sudo OR process:su OR @action:logon)"),
        (Some("aws"), _) => Some("eventSource:*"),
        (Some("okta"), _) => Some("eventType:*"),
        (Some("azure"), _) => Some("(operationName:* OR category:*)"),
        (Some("gcp"), _) => Some("(protoPayload.methodName:* OR logName:*)"),
        (Some("windows"), _) if action.is_none() => Some("(channel:* OR source:microsoft-windows* OR computer:*)"),
        (Some("linux"), _) if action.is_none() => Some("(process:* OR type:* OR @host:*)"),
        _ => None,
    };
    let mut parts = Vec::new();
    for query in [action, product_gate].into_iter().flatten() {
        parts.push(querylang::compile_rule(query)?);
    }
    Ok((!parts.is_empty()).then(|| querylang::and(parts)))
}

fn severity(level: Option<&str>) -> String {
    match level.map(str::to_ascii_lowercase).as_deref() {
        Some("critical") => "critical",
        Some("high") => "high",
        Some("medium") => "medium",
        Some("low") => "low",
        _ => "info",
    }
    .into()
}

pub fn convert(doc: &Value) -> Result<Compiled, String> {
    let title = doc.get("title").and_then(Value::as_str).ok_or("regra sem title")?.trim().to_string();
    let status = doc.get("status").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    if matches!(status.as_str(), "deprecated" | "unsupported") {
        return Err(format!("{title}: status {status}"));
    }
    let detection = doc.get("detection").and_then(Value::as_object).ok_or_else(|| format!("{title}: sem detection"))?;
    let condition_value = detection.get("condition").ok_or_else(|| format!("{title}: sem condition"))?;
    let condition = match condition_value {
        Value::String(s) => s.clone(),
        Value::Array(list) if list.len() == 1 => text(&list[0]).unwrap_or_default(),
        _ => return Err(format!("{title}: várias condições não são suportadas")),
    };
    let selections: Vec<(String, Value)> = detection
        .iter()
        .filter(|(k, _)| k.as_str() != "condition" && k.as_str() != "timeframe")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let (main, agg) = match condition.split_once('|') {
        Some((main, agg)) => (main.to_string(), Some(aggregation(agg).map_err(|e| format!("{title}: {e}"))?)),
        None => (condition.clone(), None),
    };
    let mut parser = Cond { tokens: tokenize(&main), pos: 0, selections: &selections };
    let expr = parser.or().map_err(|e| format!("{title}: {e}"))?;
    if parser.pos < parser.tokens.len() {
        return Err(format!("{title}: condição com trecho inesperado"));
    }
    let expr = match logsource_gate(doc.get("logsource")).map_err(|e| format!("{title}: {e}"))? {
        Some(gate) => querylang::and(vec![gate, expr]),
        None => expr,
    };
    let mut attack = Vec::new();
    let mut tactics = Vec::new();
    if let Some(tags) = doc.get("tags").and_then(Value::as_array) {
        for tag in tags.iter().filter_map(Value::as_str) {
            let Some(rest) = tag.strip_prefix("attack.") else { continue };
            if rest.len() > 1 && rest.starts_with(['t', 'T']) && rest[1..2].chars().all(|c| c.is_ascii_digit()) {
                attack.push(rest.to_uppercase());
            } else if let Some(t) = crate::attack::tactic(rest) {
                tactics.push(t.key.to_string());
            }
        }
    }
    let id = doc.get("id").and_then(Value::as_str).map(|s| format!("sigma:{s}")).unwrap_or_else(|| {
        format!("sigma:{}", title.to_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), "-"))
    });
    let description = doc.get("description").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let falsepositives: Vec<String> = doc
        .get("falsepositives")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let description = if falsepositives.is_empty() {
        description
    } else {
        format!("{description} Falsos positivos conhecidos: {}.", falsepositives.join("; "))
    };
    let window = detection
        .get("timeframe")
        .and_then(Value::as_str)
        .or_else(|| doc.get("timeframe").and_then(Value::as_str))
        .map(str::to_string);
    let mut def = RuleDef {
        id,
        name: title,
        description: description.chars().take(2000).collect(),
        severity: severity(doc.get("level").and_then(Value::as_str)),
        attack,
        kind: "single".into(),
        condition: String::new(),
        by: vec!["@host".into()],
        window: None,
        count: None,
        distinct: None,
        distinct_fallback: Vec::new(),
        steps: Vec::new(),
        summary: None,
        enabled: true,
        tactics,
        evidence: policy(doc)?,
        any: Vec::new(),
        unless: String::new(),
        provenance: metadata(doc),
        ratio: None,
        aggregate: None,
        coverage: None,
        bindings: Vec::new(),
        references: doc["references"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    };
    if let Some(agg) = agg {
        let by = agg.by.ok_or_else(|| format!("{}: agregação sem 'by' não é suportada", def.name))?;
        def.by = vec![by];
        def.window = Some(window.unwrap_or_else(|| "10m".into()));
        let limit = if agg.op == ">" { agg.value + 1 } else { agg.value };
        def.count = Some(limit.max(1));
        match agg.distinct {
            Some(field) => {
                def.kind = "distinct".into();
                def.distinct = Some(field);
            }
            None => def.kind = "threshold".into(),
        }
    }
    compile_rule(def, "sigma", Some(vec![expr]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Event;
    use serde_json::json;

    fn ev(source: &str, code: &str, fields: Value) -> Event {
        let mut e = Event::empty();
        e.source = source.into();
        e.code = code.into();
        if let Value::Object(map) = fields {
            e.fields = map;
        }
        e
    }

    #[test]
    fn process_creation_rule_with_modifiers_and_them() {
        let yaml = r#"
title: Whoami via web server
id: 1234
status: test
level: high
tags: [attack.discovery, attack.t1033]
logsource: { category: process_creation, product: windows }
detection:
  selection_parent:
    ParentImage|endswith: ['\w3wp.exe', '\httpd.exe']
  selection_child:
    Image|endswith: '\whoami.exe'
    CommandLine|contains|all: ['whoami', '/all']
  filter:
    User: 'NT AUTHORITY\SYSTEM'
  condition: all of selection_* and not filter
"#;
        let rules = convert_text(yaml).unwrap();
        let rule = &rules[0];
        assert_eq!(rule.def.severity, "high");
        assert_eq!(rule.def.attack, vec!["T1033"]);
        let hit = ev(
            "Microsoft-Windows-Sysmon",
            "1",
            json!({"ParentImage": "C:\\inetpub\\w3wp.exe", "Image": "C:\\Windows\\System32\\whoami.exe", "CommandLine": "whoami /all", "User": "IIS APPPOOL\\site"}),
        );
        assert!(rule.matches(&hit));
        let filtered = ev(
            "Microsoft-Windows-Sysmon",
            "1",
            json!({"ParentImage": "C:\\inetpub\\w3wp.exe", "Image": "C:\\Windows\\System32\\whoami.exe", "CommandLine": "whoami /all", "User": "NT AUTHORITY\\SYSTEM"}),
        );
        assert!(!rule.matches(&filtered));
        let other = ev(
            "Microsoft-Windows-Sysmon",
            "3",
            json!({"ParentImage": "C:\\inetpub\\w3wp.exe", "Image": "C:\\Windows\\System32\\whoami.exe", "CommandLine": "whoami /all"}),
        );
        assert!(!rule.matches(&other), "logsource gate requires a process creation");
    }

    #[test]
    fn eventid_keywords_windash_and_aggregation() {
        let yaml = r#"
title: Many failed logons
logsource: { product: windows, service: security }
detection:
  selection:
    EventID: 4625
  condition: selection | count(TargetUserName) by IpAddress > 5
timeframe: 10m
level: medium
---
title: Keyword rule
logsource: { product: linux }
detection:
  keywords:
    - 'Accepted password for root'
  condition: keywords
"#;
        let rules = convert_text(yaml).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].def.kind, "distinct");
        assert_eq!(rules[0].def.count, Some(6));
        let failed =
            ev("Microsoft-Windows-Security-Auditing", "4625", json!({"TargetUserName": "a", "IpAddress": "1.2.3.4"}));
        assert!(rules[0].matches(&failed));
        let mut linux = ev("srv", "", json!({"process": "sshd"}));
        linux.message = "Accepted password for root from 1.2.3.4 port 22".into();
        assert!(rules[1].matches(&linux));
        let dash = r#"
title: Dash
logsource: { category: process_creation }
detection:
  sel:
    CommandLine|windash|contains: ' -enc '
  condition: sel
"#;
        let rule = &convert_text(dash).unwrap()[0];
        let mut p = ev("Microsoft-Windows-Sysmon", "1", json!({"CommandLine": "powershell /enc AAAA"}));
        assert!(rule.matches(&p), "windash accepts slash variants");
        p.fields.insert("CommandLine".into(), json!("powershell -nop"));
        assert!(!rule.matches(&p));
    }

    #[test]
    fn unsupported_constructs_are_reported() {
        let yaml = "title: X\nlogsource: {}\ndetection:\n  sel:\n    CommandLine|expand: abc\n  condition: sel\n";
        assert!(convert_text(yaml).err().unwrap().contains("expand"));
        let yaml = "title: Y\ndetection:\n  sel:\n    a: b\n  condition: missing\n";
        assert!(convert_text(yaml).err().unwrap().contains("inexistente"));
        let yaml = "title: Z\nstatus: deprecated\ndetection:\n  sel:\n    a: b\n  condition: sel\n";
        assert!(convert_text(yaml).is_err());
    }

    #[test]
    fn encoded_modifiers_and_field_references_keep_their_semantics() {
        let rules=convert_text("title: Encoded\ndetection:\n  sel:\n    CommandLine|utf16le|base64offset|contains: whoami\n  condition: sel\n").unwrap();
        for prefix in ["","x","xx","xxx"] {
            let bytes=format!("{prefix}whoami suffix").encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>();
            let hit=ev("test","",json!({"CommandLine":base64::engine::general_purpose::STANDARD.encode(bytes)}));
            assert!(rules[0].matches(&hit));
        }
        assert!(!rules[0].matches(&ev("test","",json!({"CommandLine":"unrelated"}))));
        let rules=convert_text("title: Equal fields\ndetection:\n  sel:\n    TargetUserName|fieldref: SubjectUserName\n  condition: sel\n").unwrap();
        assert!(rules[0].matches(&ev("test","",json!({"TargetUserName":"Alice","SubjectUserName":"alice"}))));
        assert!(!rules[0].matches(&ev("test","",json!({"TargetUserName":"Alice"}))));
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_case_presence_null_and_fieldref_inequality_match_the_declared_specification(){
        let rule=|key:&str,value:&str|convert_text(&format!("title: Test\ndetection:\n  sel:\n    {key}: {value}\n  condition: sel\n")).unwrap().remove(0);
        let re=rule("value|re","'Admin'");assert!(re.matches(&ev("test","",json!({"value":"Admin"}))));assert!(!re.matches(&ev("test","",json!({"value":"admin"}))));
        assert!(rule("value|re|i","'Admin'").matches(&ev("test","",json!({"value":"ADMIN"}))));
        let cased=rule("value|cased|contains","'Admin'");assert!(!cased.matches(&ev("test","",json!({"value":"admin"}))));assert!(cased.matches(&ev("test","",json!({"value":"xAdminY"}))));
        let present=rule("value|exists","true");for value in [json!(""),Value::Null]{assert!(present.matches(&ev("test","",json!({"value":value}))));}assert!(!present.matches(&ev("test","",json!({}))));
        let null=rule("value","null");assert!(null.matches(&ev("test","",json!({}))));assert!(null.matches(&ev("test","",json!({"value":null}))));assert!(!null.matches(&ev("test","",json!({"value":""}))));
        let neq=rule("left|fieldref|neq","right");assert!(neq.matches(&ev("test","",json!({"left":"a","right":"b"}))));assert!(!neq.matches(&ev("test","",json!({"left":"a"}))));assert!(!neq.matches(&ev("test","",json!({"left":"A","right":"a"}))));
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_sigma_compatibility_rejects_ambiguous_combinations_and_negates_a_complete_list(){
        let convert=|key:&str,value:&str|convert_text(&format!("title: Matrix\ndetection:\n  sel:\n    {key}: {value}\n  condition: sel\n"));
        for (key,value) in [("x|exists|contains","true"),("x|gte|lt","10"),("x|re|contains","'abc'"),("x|i","'abc'"),("x|all","'abc'"),("x|base64|contains","'a*'"),("x|contains","42"),("x|fieldref","'a*'"),("x|cidr|re","'10.0.0.0/8'"),("x|contains|startswith","'abc'")]{assert!(convert(key,value).is_err(),"{key}");}
        let neq=convert("x|neq","['alice','bob']").unwrap().remove(0);
        for x in ["alice","bob","ALICE"]{assert!(!neq.matches(&ev("test","",json!({"x":x}))));}assert!(neq.matches(&ev("test","",json!({"x":"carol"}))));assert!(!neq.matches(&ev("test","",json!({}))));
        let dash=convert("CommandLine|windash|contains","' -enc -no-logo '").unwrap().remove(0);
        assert!(dash.matches(&ev("test","",json!({"CommandLine":"pwsh /enc —no-logo value"}))));assert!(!dash.matches(&ev("test","",json!({"CommandLine":"pwsh /enc /no/logo value"}))));
        assert_eq!(neq.def.provenance["specification_version"],SPECIFICATION_VERSION);
    }
}

#[cfg(test)]mod external_reference_tests{
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_sigma_matches_pinned_pysigma_condition_and_modifier_fixtures(){
        let fixture:Value=serde_json::from_str(include_str!("../tests/fixtures/security/sigma-reference.json")).unwrap();assert_eq!(fixture["upstream_revision"],"f81e4f5ace2f444f76c5de03df8e0f181f85f6cc");
        for case in fixture["cases"].as_array().unwrap(){let rules=convert_text(case["yaml"].as_str().unwrap()).unwrap();let expr=&rules[0].conds[0];
            let mut plan=crate::security_plan::Plan::default();let node=plan.add(expr);let db=duckdb::Connection::open_in_memory().unwrap();crate::engine::udf::register(&db).unwrap();let mut columnar=plan.columnar(&db).unwrap();
            let events=case["events"].as_array().unwrap().iter().map(|fields|{let mut event=crate::model::Event::empty();event.fields=fields.as_object().unwrap().clone();event}).collect::<Vec<_>>();let mut memo=vec![vec![None;plan.len()];events.len()];columnar.evaluate(&db,&events,&mut memo).unwrap();
            for (i,event) in events.iter().enumerate(){let expected=if case["backend_expected"].is_array(){assert_eq!(case["id"],"numeric_neq");assert!(case["backend_policy"].as_str().is_some());case["backend_expected"][i].as_bool().unwrap()}else{case["expected"][i].as_bool().unwrap()};assert_eq!(expr.matches(event),expected,"{} event {i}",case["id"]);assert_eq!(plan.matches(node,&crate::querylang::Ctx::new(event),&mut memo[i]),expected,"columnar {} event {i}",case["id"]);}
        }
    }
}
