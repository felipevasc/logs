//! Reconstruct only operations with producer identities. Similar text is not identity.
use crate::{
    model::Event,
    security_normalize::{event_ref, field_text},
};

pub fn key(event: &Event) -> Option<String> {
    if !matches!(crate::security_normalize::product(event), "powershell" | "auditd" | "kubernetes") {
        return None;
    }
    let (_, n) = crate::security_normalize::normalize(event, &[]);
    let scope = n.get("namespace").unwrap_or("");
    let host = n.get("host").unwrap_or("");
    let identity = match n.product.as_str() {
        "powershell" if !host.is_empty() => field_text(event, "ScriptBlockId").map(|v| ("powershell", v)),
        "auditd" if !host.is_empty() && event.timestamp.is_some() => {
            field_text(event, "audit_serial").map(|v| ("auditd", format!("{}:{v}", event.timestamp.unwrap())))
        }
        "kubernetes" if !scope.is_empty() => field_text(event, "auditID").map(|v| ("kubernetes", v)),
        _ => None,
    }?;
    Some(serde_json::to_string(&(identity.0, scope, host, identity.1)).unwrap())
}

pub struct Reconstructed {
    pub event: Event,
    pub members: Vec<(usize, String)>,
    pub limitations: Vec<String>,
}
pub fn assemble(mut events: Vec<Event>) -> Result<Reconstructed, String> {
    if events.is_empty() {
        return Err("Operação sem fragmentos".into());
    }
    events.sort_by_key(|e| {
        (field_text(e, "MessageNumber").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0), e.timestamp, e.id)
    });
    let mut event = events[0].clone();
    event.event_ref = event_ref(&event);
    let members = events.iter().map(|e| (e.id, event_ref(e))).collect();
    let mut limitations = Vec::new();
    let product = crate::security_normalize::product(&event);
    if product == "powershell" {
        let total = field_text(&event, "MessageTotal").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
        let mut parts = std::collections::BTreeMap::new();
        for e in &events {
            let index = field_text(e, "MessageNumber").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
            let text = field_text(e, "ScriptBlockText").unwrap_or_default();
            if let Some(previous) = parts.insert(index, text.clone()) {
                if previous != text {
                    limitations.push("Fragmentos PowerShell conflitantes".into());
                }
            }
        }
        if total > 4096 || parts.len() != total || !(1..=total).all(|i| parts.contains_key(&i)) {
            limitations.push("Script PowerShell incompleto".into());
        }
        let bytes: usize = parts.values().map(String::len).sum();
        if bytes > 512 * 1024 {
            return Err("Script reconstruído excede 512 KiB; análise interrompida sem executar conteúdo".into());
        }
        event.fields.insert("ScriptBlockText".into(), serde_json::Value::from(parts.into_values().collect::<String>()));
    } else {
        if product == "auditd" {
            if let Some(exec) = events.iter().find(|e| field_text(e, "type").as_deref() == Some("EXECVE")) {
                event = exec.clone();
            }
        }
        // The terminal Kubernetes stage supplies the outcome; RequestReceived cannot prove success.
        if product == "kubernetes" {
            if let Some(last) =
                events.iter().rev().find(|e| field_text(e, "stage").as_deref() == Some("ResponseComplete"))
            {
                event = last.clone();
            } else {
                limitations.push("Operação Kubernetes sem ResponseComplete".into());
            }
        }
        for e in &events {
            for (k, v) in &e.fields {
                event.fields.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        if product == "auditd" && !events.iter().any(|e| field_text(e, "type").as_deref() == Some("SYSCALL")) {
            limitations.push("Operação auditd sem resultado SYSCALL".into());
        }
        if product == "auditd" {
            if let Some(count) = field_text(&event, "argc").and_then(|v| v.parse::<usize>().ok()) {
                if count > 4096 {
                    return Err("Operação auditd excede 4096 argumentos".into());
                }
                let arguments: Vec<_> = (0..count).map(|i| field_text(&event, &format!("a{i}"))).collect();
                if arguments.iter().all(Option::is_some) {
                    let command = arguments.into_iter().flatten().collect::<Vec<_>>().join(" ");
                    if command.len() > 512 * 1024 {
                        return Err("Comando auditd reconstruído excede 512 KiB".into());
                    }
                    event.fields.insert("cmdline".into(), serde_json::Value::from(command));
                } else {
                    limitations.push("Argumentos EXECVE incompletos; fragmentos preservados".into());
                }
            }
        }
    }
    // Keep the first event's addressable identity, even when a terminal stage was selected.
    event.id = events[0].id;
    event.event_ref = event_ref(&events[0]);
    Ok(Reconstructed { event, members, limitations })
}
