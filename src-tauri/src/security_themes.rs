//! Sections of the compromise analysis. Every finding belongs to exactly one
//! section, chosen by what the analyst investigates next: who connected
//! (clients), what they tried against web applications, whether code ran,
//! files were planted, credentials were taken, and so on. The section never
//! changes the evidence level; it only groups findings.
use crate::attack::AttackRef;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Theme {
    pub key: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
}

pub const THEMES: &[Theme] = &[
    Theme { key: "clients", label: "Clientes e navegadores", icon: "fa-user-secret", description: "Quem acessou: navegadores forjados ou impossíveis, ferramentas de ataque, bibliotecas, linha de comando, automação e robôs." },
    Theme { key: "web", label: "Ataques a aplicações web", icon: "fa-bug", description: "Injeção SQL, XSS, traversal, LFI/RFI, SSRF, XXE, templates, desserialização e busca de arquivos sensíveis." },
    Theme { key: "execution", label: "Execução de código", icon: "fa-terminal", description: "Comandos e shells disparados, shells reversos, PowerShell, binários do sistema e saídas de comandos em respostas." },
    Theme { key: "upload", label: "Upload e webshells", icon: "fa-file-arrow-up", description: "Envio de arquivos executáveis e webshells, e processos iniciados por eles." },
    Theme { key: "credentials", label: "Credenciais e contas", icon: "fa-key", description: "Força bruta, spraying, extração de credenciais, Kerberos e Active Directory." },
    Theme { key: "persistence", label: "Persistência e privilégios", icon: "fa-anchor", description: "Serviços, tarefas, chaves de inicialização, cron, contas criadas e elevação de privilégio." },
    Theme { key: "discovery", label: "Reconhecimento e varreduras", icon: "fa-satellite-dish", description: "Varreduras de portas, de hosts e de aplicações, e rajadas de comandos de descoberta." },
    Theme { key: "lateral", label: "Movimento lateral e acesso remoto", icon: "fa-shuffle", description: "Acesso remoto, saltos entre máquinas e túneis internos." },
    Theme { key: "c2", label: "Comando e controle e exfiltração", icon: "fa-tower-broadcast", description: "Beacons, túneis para fora, canais de controle e envio de dados para destinos externos." },
    Theme { key: "exposure", label: "Vazamento de dados", icon: "fa-file-shield", description: "Respostas com arquivos do sistema, chaves, segredos, dumps de banco, configurações e erros com detalhes internos." },
    Theme { key: "defense", label: "Evasão e impacto", icon: "fa-eraser", description: "Logs apagados, auditoria e defesas desligadas, destruição de recuperação e mineração." },
    Theme { key: "cloud", label: "Nuvem, contêineres e SaaS", icon: "fa-cloud", description: "Contas de nuvem, Kubernetes, contêineres, OAuth, CI/CD e caixas de e-mail." },
    Theme { key: "other", label: "Outros indícios", icon: "fa-circle-question", description: "Alertas de terceiros e regras sem seção específica." },
];

pub fn find(key: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.key == key)
}

/// Builtin and Case rules, by id.
fn by_rule(rule: &str) -> Option<&'static str> {
    let exact = match rule {
        "exec.webshell-child" => Some("upload"),
        "attempt.reverse-shell.request" | "attempt.reverse-shell.process" | "content.reverse_payload" | "c2.reverse-shell" | "c2.socat-exec" => Some("execution"),
        "web.scan" | "web.success-after-scan" | "net.portscan" | "net.hostscan" | "exec.discovery-burst" => Some("discovery"),
        "net.sensitive-port.public" | "auth.rdp.public" => Some("lateral"),
        "net.beacon" => Some("c2"),
        "content.sensitive_read" | "content.web_process_read" => Some("exposure"),
        "collect.domain-secrets" => Some("credentials"),
        "chain.web.command" | "chain.download.execution" => Some("execution"),
        "chain.persistence.payload" => Some("persistence"),
        "chain.recovery.destruction" => Some("defense"),
        "ids.alert" | "av.detection" => Some("other"),
        _ => None,
    };
    if exact.is_some() {
        return exact;
    }
    let prefixes: &[(&str, &str)] = &[
        ("client.", "clients"),
        ("tool.", "clients"),
        ("content.passwd_", "exposure"),
        ("content.shadow_", "exposure"),
        ("content.private_key_", "exposure"),
        ("content.secrets_", "exposure"),
        ("content.", "web"),
        ("web.upload.", "upload"),
        ("web.cmd.", "execution"),
        ("web.", "web"),
        ("auth.", "credentials"),
        ("cred.", "credentials"),
        ("ad.", "credentials"),
        ("chain.ad.", "credentials"),
        ("account.", "persistence"),
        ("persist.", "persistence"),
        ("linux.", "persistence"),
        ("exec.", "execution"),
        ("evasion.", "defense"),
        ("defense.", "defense"),
        ("stealth.", "defense"),
        ("impact.", "defense"),
        ("cloud.", "cloud"),
        ("chain.cloud.", "cloud"),
        ("chain.oauth.", "cloud"),
        ("chain.cicd.", "cloud"),
        ("container.", "cloud"),
        ("k8s.", "cloud"),
        ("saas.", "cloud"),
        ("chain.lateral.", "lateral"),
        ("exfil.", "c2"),
        ("c2.", "c2"),
    ];
    prefixes
        .iter()
        .filter(|(prefix, _)| rule.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, theme)| *theme)
}

/// Signals of the threat catalog, by category.
fn by_category(category: &str) -> Option<&'static str> {
    let lower = category.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    Some(if has(&["upload", "webshell"]) {
        "upload"
    } else if has(&["saídas", "saidas", "traceback", "respostas de execução", "shell reverso", "powershell", "injeção de comandos", "injecao de comandos", "shell e persistência unix"]) {
        "execution"
    } else if has(&["xss", "sql", "traversal", "lfi", "rfi", "ssrf", "xxe", "templates", "desserializa"]) && !has(&["erros de banco"]) {
        "web"
    } else if has(&["expost", "exposição", "exposicao", "dumps", "erros de banco", "metadados", "secreto", "segredo"]) {
        "exposure"
    } else if has(&["credencia", "active directory", "autentica"]) {
        "credentials"
    } else if has(&["persistência", "persistencia"]) {
        "persistence"
    } else if has(&["movimento lateral"]) {
        "lateral"
    } else if has(&["c2", "túne", "tune", "exfiltra"]) {
        "c2"
    } else if has(&["nuvem", "cloud", "kubernetes", "contêiner", "container"]) {
        "cloud"
    } else if has(&["evas", "impacto", "minera"]) {
        "defense"
    } else {
        return None;
    })
}

/// Rules without a known id (Sigma, custom): what their techniques describe.
fn by_attack(attack: &[AttackRef], tactics: &[String]) -> &'static str {
    let ids: Vec<&str> = attack.iter().map(|a| a.id.as_str()).collect();
    let has = |prefixes: &[&str]| ids.iter().any(|id| prefixes.iter().any(|p| id.starts_with(p)));
    if has(&["T1505.003", "T1608.002"]) {
        return "upload";
    }
    if has(&["T1078.004", "T1530", "T1537", "T1580", "T1538", "T1610", "T1611", "T1613", "T1525", "T1552.005"]) {
        return "cloud";
    }
    if has(&["T1046", "T1595", "T1590", "T1592", "T1018", "T1135"]) {
        return "discovery";
    }
    if has(&["T1190", "T1189"]) {
        return "web";
    }
    if has(&["T1110", "T1003", "T1558", "T1555", "T1552", "T1212"]) {
        return "credentials";
    }
    if has(&["T1059", "T1203", "T1204", "T1218", "T1047"]) {
        return "execution";
    }
    let first = tactics.first().map(String::as_str).or_else(|| attack.iter().find_map(|a| a.tactics.first().map(String::as_str)));
    match first {
        Some("execution") => "execution",
        Some("persistence" | "privilege-escalation") => "persistence",
        Some("credential-access") => "credentials",
        Some("discovery" | "reconnaissance") => "discovery",
        Some("lateral-movement") => "lateral",
        Some("command-and-control" | "exfiltration") => "c2",
        Some("collection") => "exposure",
        Some("stealth" | "defense-impairment" | "defense-evasion" | "impact") => "defense",
        Some("initial-access") => "web",
        _ => "other",
    }
}

/// The section of one finding.
pub fn of(rule: &str, attack: &[AttackRef], tactics: &[String]) -> &'static str {
    if let Some(category) = rule.strip_prefix("threat:") {
        if let Some(theme) = by_category(category) {
            return theme;
        }
    } else if let Some(theme) = by_rule(rule) {
        return theme;
    }
    by_attack(attack, tactics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_rule_and_threat_category_has_a_specific_section() {
        let mut missing = Vec::new();
        for file in [
            include_str!("../resources/detection-rules.json"),
            include_str!("../resources/detection-advanced.json"),
            include_str!("../resources/detection-content.json"),
            include_str!("../resources/detection-expansion.json"),
            include_str!("../resources/detection-clients.json"),
        ] {
            let rules: serde_json::Value = serde_json::from_str(file).unwrap();
            for rule in rules["rules"].as_array().unwrap() {
                let id = rule["id"].as_str().unwrap();
                if by_rule(id).is_none() {
                    missing.push(id.to_string());
                }
            }
        }
        let threats: serde_json::Value = serde_json::from_str(include_str!("../resources/threat-rules.json")).unwrap();
        for rule in threats["rules"].as_array().unwrap() {
            let category = rule["category"].as_str().unwrap();
            if by_category(category).is_none() && !missing.contains(&category.to_string()) {
                missing.push(category.to_string());
            }
        }
        assert!(missing.is_empty(), "sem seção: {missing:?}");
    }

    #[test]
    fn sections_follow_what_the_finding_describes() {
        assert_eq!(of("exec.webshell-child", &[], &[]), "upload");
        assert_eq!(of("threat:Upload e webshell", &[], &[]), "upload");
        assert_eq!(of("threat:Injeção SQL", &[], &[]), "web");
        assert_eq!(of("threat:Erros de banco com contexto", &[], &[]), "exposure");
        assert_eq!(of("threat:Injeção de comandos", &[], &[]), "execution");
        assert_eq!(of("threat:Saídas Unix", &[], &[]), "execution");
        assert_eq!(of("content.passwd_response", &[], &[]), "exposure");
        assert_eq!(of("content.sqli_request", &[], &[]), "web");
        assert_eq!(of("client.forged-browser", &[], &[]), "clients");
        assert_eq!(of("auth.bruteforce.source", &[], &[]), "credentials");
        assert_eq!(of("chain.ad.rbcd-use", &[], &[]), "credentials");
        let sigma = |id: &str| crate::attack::reference(id, &[]);
        assert_eq!(of("4f2a-sigma", &[sigma("T1505.003")], &[]), "upload");
        assert_eq!(of("4f2a-sigma", &[sigma("T1059.001")], &[]), "execution");
        assert_eq!(of("4f2a-sigma", &[], &["lateral-movement".into()]), "lateral");
        assert_eq!(of("4f2a-sigma", &[], &[]), "other");
        assert!(THEMES.iter().all(|t| find(t.key).is_some()));
    }
}
