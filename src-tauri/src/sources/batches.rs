//! Byte ranges into the original document, with one ordinal checkpoint per 64 records.
use super::{read_json_string, skip_json_value, skip_ws, Event, Map, Value, ENVELOPE_KEYS};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Layout {
    pub groups: Vec<Group>,
    pub choices: BTreeMap<String, String>,
}
pub(crate) struct Group {
    pub open: usize,
    pub(super) end: usize,
    pub(super) single: bool,
    pointer: String,
    schema: String,
    checkpoints: Vec<usize>,
    context: Map<String, Value>,
}
fn next(bytes: &[u8], i: usize) -> usize {
    let end = skip_ws(bytes, i);
    if bytes.get(end) == Some(&b',') {
        skip_ws(bytes, end + 1)
    } else {
        end
    }
}
fn walk(
    bytes: &[u8],
    at: usize,
    path: &str,
    schema: &str,
    inherited: &Map<String, Value>,
    out: &mut Vec<Group>,
    depth: usize,
) -> Option<()> {
    if depth > 12 {
        return Some(());
    }
    let mut members = Vec::new();
    let mut context = inherited.clone();
    let mut i = skip_ws(bytes, at + 1);
    while bytes.get(i) == Some(&b'"') {
        let (key, after) = read_json_string(bytes, i)?;
        let colon = skip_ws(bytes, after);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let start = skip_ws(bytes, colon + 1);
        let end = skip_json_value(bytes, start)?;
        if end <= start {
            return None;
        }
        if end - start <= 8192
            && (matches!(
                key.as_str(),
                "resource"
                    | "scope"
                    | "schemaUrl"
                    | "tenant"
                    | "host"
                    | "service"
                    | "source"
                    | "version"
                    | "environment"
            ) || !matches!(bytes[start], b'[' | b'{') && end - start <= 192)
        {
            if context.len() < 64 {
                if let Ok(value) = serde_json::from_slice(&bytes[start..end]) {
                    context.insert(key.clone(), value);
                }
            }
        }
        members.push((key, start, end));
        if members.len() > 1024 {
            return None;
        }
        i = next(bytes, end);
    }
    if bytes.get(i) != Some(&b'}') {
        return None;
    }
    for (key, start, end) in members {
        let component = key.replace('~', "~0").replace('/', "~1");
        let pointer = format!("{path}/{component}");
        let schema = format!("{schema}/{component}");
        match bytes[start] {
            b'{' if !matches!(key.as_str(), "resource" | "scope" | "attributes") => {
                walk(bytes, start, &pointer, &schema, &context, out, depth + 1)?
            }
            b'[' => {
                let mut pos = skip_ws(bytes, start + 1);
                if bytes.get(pos) != Some(&b'{') {
                    continue;
                }
                let first_end = skip_json_value(bytes, pos)?;
                let first = if first_end - pos <= 32768 {
                    serde_json::from_slice::<Value>(&bytes[pos..first_end]).ok()
                } else {
                    None
                };
                let object = first.as_ref().and_then(Value::as_object);
                let inferred = object.is_some_and(|m| {
                    m.keys().any(|k| {
                        matches!(
                            k.as_str(),
                            "message" | "timestamp" | "timeUnixNano" | "eventTime" | "severityText"
                        )
                    }) && !matches!(
                        key.as_str(),
                        "attributes" | "labels" | "tags" | "exceptions"
                    )
                });
                let wrappers = matches!(
                    key.as_str(),
                    "resourceLogs" | "scopeLogs" | "instrumentationLibraryLogs"
                ) || object.is_some_and(|m| {
                    m.iter().any(|(k, v)| {
                        v.is_array()
                            && (ENVELOPE_KEYS.contains(&k.to_ascii_lowercase().as_str())
                                || k == "logRecords")
                    })
                });
                let leaf = ENVELOPE_KEYS.contains(&key.to_ascii_lowercase().as_str())
                    || key == "logRecords"
                    || inferred;
                if !wrappers && !leaf {
                    continue;
                }
                let mut checkpoints = Vec::new();
                let mut ord = 0;
                while pos < end && bytes[pos] != b']' {
                    if ord % 64 == 0 {
                        if crate::operations::cancelled() {
                            return None;
                        }
                        checkpoints.push(pos)
                    }
                    if bytes[pos] != b'{' {
                        return None;
                    }
                    if wrappers {
                        walk(
                            bytes,
                            pos,
                            &format!("{pointer}/{ord}"),
                            &format!("{schema}/*"),
                            &context,
                            out,
                            depth + 1,
                        )?
                    }
                    let last = skip_json_value(bytes, pos)?;
                    if last <= pos {
                        return None;
                    }
                    pos = next(bytes, last);
                    ord += 1;
                }
                if !wrappers && ord > 0 {
                    out.push(Group {
                        open: start,
                        end,
                        single: false,
                        pointer,
                        schema,
                        context: context.clone(),
                        checkpoints,
                    })
                }
            }
            _ => {}
        }
    }
    Some(())
}
pub(crate) fn detect(bytes: &[u8], start: usize, choice: Option<&str>) -> Option<Layout> {
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut entries = Vec::new();
    let mut cursor = start;
    let mut document = 0;
    let mut seen_batch = false;
    let mut choices = BTreeMap::new();
    let first_end = skip_json_value(bytes, start)?;
    let stream = next(bytes, first_end) < bytes.len();
    while cursor < bytes.len() {
        if bytes.get(cursor) != Some(&b'{') || crate::operations::cancelled() {
            return None;
        }
        let end = skip_json_value(bytes, cursor)?;
        let path = if stream {
            format!("/{document}")
        } else {
            String::new()
        };
        let mut groups = Vec::new();
        walk(bytes, cursor, &path, "", &Map::new(), &mut groups, 0)?;
        let mut options = BTreeMap::new();
        for group in &groups {
            let preview = choices.entry(group.schema.clone()).or_insert_with(|| {
                let p = group.checkpoints[0];
                let end = skip_json_value(bytes, p).unwrap_or(p).min(p + 480);
                String::from_utf8_lossy(&bytes[p..end]).into_owned()
            });
            options
                .entry(group.schema.clone())
                .or_insert_with(|| preview.clone());
        }
        seen_batch |= !groups.is_empty();
        let mut context = Map::new();
        if options.len() > 1 {
            if let Some(selected) = choice.filter(|k| options.contains_key(*k)) {
                groups.retain(|g| g.schema == selected);
            } else {
                context.insert(
                    "log.batch.choices".into(),
                    serde_json::to_value(options).ok()?,
                );
                groups.clear();
            }
        }
        if groups.is_empty() {
            if !seen_batch && document == 0 && !stream {
                return None;
            }
            // A field value can contain a batch keyword in ordinary NDJSON.
            // Keep the normal streaming index in that case.
            if !seen_batch && choice.is_none() && cursor.saturating_sub(start) > 65536 {
                return None;
            }
            entries.push(Group {
                open: cursor,
                end,
                single: true,
                pointer: path,
                schema: String::new(),
                checkpoints: vec![cursor],
                context,
            });
        } else {
            entries.extend(groups);
        }
        cursor = next(bytes, end);
        document += 1;
    }
    if !seen_batch {
        return None;
    }
    entries.sort_by_key(|g| g.open);
    Some(Layout {
        groups: entries,
        choices,
    })
}
impl Layout {
    pub fn annotate(&self, bytes: &[u8], pos: usize, ev: &mut Event) {
        let n = self.groups.partition_point(|g| g.open <= pos);
        let Some(g) = n
            .checked_sub(1)
            .and_then(|n| self.groups.get(n))
            .filter(|g| pos < g.end)
        else {
            return;
        };
        if g.single {
            if let Some(choices) = g.context.get("log.batch.choices") {
                ev.fields
                    .insert("log.batch.choices".into(), choices.clone());
            }
            ev.fields
                .insert("log.record.path".into(), Value::from(g.pointer.clone()));
            return;
        }
        let block = g
            .checkpoints
            .partition_point(|&c| c <= pos)
            .saturating_sub(1);
        let mut cursor = g.checkpoints[block];
        let mut ordinal = 64 * block;
        while cursor < pos {
            let Some(end) = skip_json_value(bytes, cursor) else {
                return;
            };
            if end <= cursor {
                return;
            }
            cursor = next(bytes, end);
            ordinal += 1;
        }
        if cursor != pos {
            return;
        }
        ev.fields.insert(
            "log.record.path".into(),
            Value::from(format!("{}/{ordinal}", g.pointer)),
        );
        for (k, v) in &g.context {
            ev.fields
                .entry(format!("batch.{k}"))
                .or_insert_with(|| v.clone());
            if matches!(
                k.as_str(),
                "tenant"
                    | "tenant.id"
                    | "namespace"
                    | "host"
                    | "host.name"
                    | "service"
                    | "service.name"
                    | "version"
                    | "environment"
            ) && v.is_string()
            {
                ev.fields.entry(k.clone()).or_insert_with(|| v.clone());
            }
            if k == "source" && ev.source.is_empty() {
                if let Some(source) = v.as_str() {
                    ev.source = source.into()
                }
            }
        }
        if let Some(attrs) = g
            .context
            .get("resource")
            .and_then(|r| r["attributes"].as_array())
        {
            for a in attrs {
                if let (Some(k), Some(v)) = (a["key"].as_str(), a["value"]["stringValue"].as_str())
                {
                    ev.fields
                        .entry(format!("resource.{k}"))
                        .or_insert_with(|| Value::from(v));
                    if matches!(
                        k,
                        "service.name"
                            | "service.namespace"
                            | "service.version"
                            | "host.name"
                            | "tenant.id"
                            | "deployment.environment"
                            | "deployment.environment.name"
                    ) {
                        ev.fields
                            .entry(k.to_owned())
                            .or_insert_with(|| Value::from(v));
                    }
                    if k == "service.name" && (ev.source.is_empty() || ev.source == "JSON") {
                        ev.source = v.into()
                    }
                }
            }
        }
        if let Some(v) = ev.fields.get("body.stringValue").and_then(Value::as_str) {
            ev.message = v.into()
        }
        if let Some(v) = ev
            .fields
            .get("timeUnixNano")
            .or_else(|| ev.fields.get("observedTimeUnixNano"))
        {
            if let Some(n) = v
                .as_u64()
                .or_else(|| v.as_str()?.parse().ok())
                .filter(|&n| n > 0)
            {
                ev.timestamp = i64::try_from(n / 1_000_000).ok()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CodesConfig;
    fn rows(json: &str, format: &str) -> Vec<Event> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.json");
        std::fs::write(&path, json).unwrap();
        let idx =
            super::super::index_file(path.to_str().unwrap(), format, None, None, None).unwrap();
        let codes = CodesConfig::default();
        (0..idx.lines.len())
            .map(|i| super::super::event_at(&idx, i, &codes, &codes, &[]))
            .collect()
    }
    #[test]
    fn indexes_every_scope_and_keeps_source_bytes_and_pointer() {
        let s = r#"{"resourceLogs":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"coletor"}}]},"scopeLogs":[{"logRecords":[{"timeUnixNano":"1700000000000000000","body":{"stringValue":"um"}},{"body":{"stringValue":"dois"}}]},{"logRecords":[{"body":{"stringValue":"três"}}]}]}]}"#;
        let r = rows(s, "auto");
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].timestamp, Some(1700000000000));
        assert_eq!(r[2].message, "três");
        assert_eq!(r[2].fields["resource.service.name"], "coletor");
        assert_eq!(
            r[2].fields["log.record.path"],
            "/resourceLogs/0/scopeLogs/1/logRecords/0"
        );
        assert!(s.contains(&r[2].raw));
        assert_ne!(r[0].event_ref, r[1].event_ref);
    }
    #[test]
    fn mixed_stream_keeps_each_single_record_and_paths_across_documents() {
        let s="{\"events\":[{\"message\":\"um\"},{\"message\":\"dois\"}]}\n{\"message\":\"avulso\"}\n{\"events\":[{\"message\":\"fim\"}]}";
        let r = rows(s, "auto");
        assert_eq!(r.len(), 4);
        assert_eq!(
            r.iter()
                .map(|r| r.fields["log.record.path"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["/0/events/0", "/0/events/1", "/1", "/2/events/0"]
        );
        assert_eq!(r[2].message, "avulso");
        assert!(s.contains(&r[2].raw));
    }
    #[test]
    fn competing_lists_preserve_document_until_a_path_is_selected() {
        let s = r#"{"events":[{"message":"um"}],"records":[{"message":"dois"}]}"#;
        let r = rows(s, "auto");
        assert_eq!(r.len(), 1);
        assert!(r[0].fields["log.batch.choices"].get("/events").is_some());
        assert_eq!(r[0].raw, s);
        let r = rows(s, "json-list:/records");
        assert_eq!(r[0].message, "dois");
        assert_eq!(r[0].fields["log.record.path"], "/records/0");
        let stream = format!("{s}\n{}", r#"{"data":[{"message":"independente"}]}"#);
        let stream_rows = rows(&stream, "json-list:/records");
        assert_eq!(stream_rows.len(), 2);
        assert_eq!(stream_rows[1].message, "independente");
        let rotated=rows(r#"{"items":[{"message":"novo formato"}]}"#,"json-list:/records");
        assert_eq!(rotated[0].message,"novo formato");
    }
}
