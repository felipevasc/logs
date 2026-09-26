//! MITRE ATT&CK tactics and the techniques referenced by the local rules.
//! Labels are Portuguese; identifiers follow ATT&CK Enterprise.
use serde::Serialize;

pub struct Tactic {
    pub id: &'static str,
    pub key: &'static str,
    pub label: &'static str,
}

/// Kill-chain order used for sorting and the tactics strip.
pub const TACTICS: &[Tactic] = &[
    Tactic { id: "TA0043", key: "reconnaissance", label: "Reconhecimento" },
    Tactic { id: "TA0042", key: "resource-development", label: "Preparação" },
    Tactic { id: "TA0001", key: "initial-access", label: "Acesso inicial" },
    Tactic { id: "TA0002", key: "execution", label: "Execução" },
    Tactic { id: "TA0003", key: "persistence", label: "Persistência" },
    Tactic { id: "TA0004", key: "privilege-escalation", label: "Escalada de privilégio" },
    Tactic { id: "TA0005", key: "defense-evasion", label: "Evasão de defesa" },
    Tactic { id: "TA0006", key: "credential-access", label: "Acesso a credenciais" },
    Tactic { id: "TA0007", key: "discovery", label: "Descoberta" },
    Tactic { id: "TA0008", key: "lateral-movement", label: "Movimento lateral" },
    Tactic { id: "TA0009", key: "collection", label: "Coleta" },
    Tactic { id: "TA0011", key: "command-and-control", label: "Comando e controle" },
    Tactic { id: "TA0010", key: "exfiltration", label: "Exfiltração" },
    Tactic { id: "TA0040", key: "impact", label: "Impacto" },
];

pub struct Technique {
    pub id: &'static str,
    pub name: &'static str,
    pub tactics: &'static [&'static str],
}

macro_rules! t {
    ($id:expr, $name:expr, [$($tactic:expr),*]) => {
        Technique { id: $id, name: $name, tactics: &[$($tactic),*] }
    };
}

pub const TECHNIQUES: &[Technique] = &[
    t!("T1595", "Varredura ativa", ["reconnaissance"]),
    t!("T1595.002", "Varredura de vulnerabilidades", ["reconnaissance"]),
    t!("T1592", "Coleta de informações do host", ["reconnaissance"]),
    t!("T1588.002", "Obtenção de ferramentas", ["resource-development"]),
    t!("T1190", "Exploração de aplicação exposta", ["initial-access"]),
    t!("T1133", "Serviços remotos externos", ["initial-access", "persistence"]),
    t!("T1566", "Phishing", ["initial-access"]),
    t!("T1078", "Contas válidas", ["initial-access", "persistence", "privilege-escalation", "defense-evasion"]),
    t!("T1078.004", "Contas de nuvem", ["initial-access", "persistence", "privilege-escalation", "defense-evasion"]),
    t!("T1059", "Interpretador de comandos e scripts", ["execution"]),
    t!("T1059.001", "PowerShell", ["execution"]),
    t!("T1059.003", "Shell de comandos do Windows", ["execution"]),
    t!("T1059.004", "Shell Unix", ["execution"]),
    t!("T1059.006", "Python", ["execution"]),
    t!("T1059.007", "JavaScript", ["execution"]),
    t!("T1203", "Exploração para execução no cliente", ["execution"]),
    t!("T1204", "Execução pelo usuário", ["execution"]),
    t!("T1047", "Windows Management Instrumentation", ["execution"]),
    t!("T1569.002", "Execução de serviço", ["execution"]),
    t!("T1609", "Comando em contêiner", ["execution"]),
    t!("T1610", "Implantação de contêiner", ["execution", "defense-evasion"]),
    t!("T1053", "Tarefa agendada", ["execution", "persistence", "privilege-escalation"]),
    t!("T1053.003", "Cron", ["execution", "persistence", "privilege-escalation"]),
    t!("T1053.005", "Tarefa agendada do Windows", ["execution", "persistence", "privilege-escalation"]),
    t!("T1543", "Criação ou alteração de processo do sistema", ["persistence", "privilege-escalation"]),
    t!("T1543.002", "Serviço systemd", ["persistence", "privilege-escalation"]),
    t!("T1543.003", "Serviço do Windows", ["persistence", "privilege-escalation"]),
    t!("T1136", "Criação de conta", ["persistence"]),
    t!("T1136.001", "Conta local", ["persistence"]),
    t!("T1136.002", "Conta de domínio", ["persistence"]),
    t!("T1136.003", "Conta de nuvem", ["persistence"]),
    t!("T1098", "Manipulação de conta", ["persistence", "privilege-escalation"]),
    t!("T1098.001", "Credenciais adicionais de nuvem", ["persistence", "privilege-escalation"]),
    t!("T1098.004", "Chaves SSH autorizadas", ["persistence", "privilege-escalation"]),
    t!("T1505.003", "Web shell", ["persistence"]),
    t!("T1547.001", "Chaves Run e pasta de inicialização", ["persistence", "privilege-escalation"]),
    t!("T1546", "Execução acionada por evento", ["persistence", "privilege-escalation"]),
    t!("T1548.003", "Sudo e cache do sudo", ["privilege-escalation", "defense-evasion"]),
    t!("T1068", "Exploração para escalada de privilégio", ["privilege-escalation"]),
    t!("T1611", "Escape para o host", ["privilege-escalation"]),
    t!("T1134", "Manipulação de token de acesso", ["defense-evasion", "privilege-escalation"]),
    t!("T1055", "Injeção em processo", ["defense-evasion", "privilege-escalation"]),
    t!("T1070", "Remoção de indicadores", ["defense-evasion"]),
    t!("T1070.001", "Limpeza de logs de eventos do Windows", ["defense-evasion"]),
    t!("T1070.002", "Limpeza de logs do Linux ou macOS", ["defense-evasion"]),
    t!("T1070.004", "Remoção de arquivos", ["defense-evasion"]),
    t!("T1562", "Enfraquecimento de defesas", ["defense-evasion"]),
    t!("T1562.001", "Desativação de ferramentas de segurança", ["defense-evasion"]),
    t!("T1562.002", "Desativação do log de eventos", ["defense-evasion"]),
    t!("T1562.008", "Desativação de logs de nuvem", ["defense-evasion"]),
    t!("T1027", "Ofuscação", ["defense-evasion"]),
    t!("T1140", "Desofuscação ou decodificação", ["defense-evasion"]),
    t!("T1218", "Execução por binário assinado do sistema", ["defense-evasion"]),
    t!("T1112", "Alteração do registro", ["defense-evasion"]),
    t!("T1036", "Mascaramento", ["defense-evasion"]),
    t!("T1564", "Ocultação de artefatos", ["defense-evasion"]),
    t!("T1484", "Alteração de política de domínio", ["defense-evasion", "privilege-escalation"]),
    t!("T1110", "Força bruta", ["credential-access"]),
    t!("T1110.001", "Adivinhação de senha", ["credential-access"]),
    t!("T1110.003", "Pulverização de senhas", ["credential-access"]),
    t!("T1110.004", "Reuso de credenciais vazadas", ["credential-access"]),
    t!("T1003", "Despejo de credenciais do sistema", ["credential-access"]),
    t!("T1003.001", "Memória do LSASS", ["credential-access"]),
    t!("T1003.006", "DCSync", ["credential-access"]),
    t!("T1552", "Credenciais desprotegidas", ["credential-access"]),
    t!("T1552.001", "Credenciais em arquivos", ["credential-access"]),
    t!("T1552.005", "API de metadados de nuvem", ["credential-access"]),
    t!("T1555", "Credenciais de repositórios de senhas", ["credential-access"]),
    t!("T1558", "Roubo ou forja de tickets Kerberos", ["credential-access"]),
    t!("T1558.003", "Kerberoasting", ["credential-access"]),
    t!("T1558.004", "AS-REP Roasting", ["credential-access"]),
    t!("T1528", "Roubo de token de aplicação", ["credential-access"]),
    t!("T1539", "Roubo de cookie de sessão", ["credential-access"]),
    t!("T1087", "Descoberta de contas", ["discovery"]),
    t!("T1082", "Descoberta de informações do sistema", ["discovery"]),
    t!("T1083", "Descoberta de arquivos e diretórios", ["discovery"]),
    t!("T1016", "Descoberta de configuração de rede", ["discovery"]),
    t!("T1049", "Descoberta de conexões de rede", ["discovery"]),
    t!("T1057", "Descoberta de processos", ["discovery"]),
    t!("T1069", "Descoberta de grupos de permissão", ["discovery"]),
    t!("T1018", "Descoberta de sistemas remotos", ["discovery"]),
    t!("T1046", "Descoberta de serviços de rede", ["discovery"]),
    t!("T1033", "Descoberta do usuário do sistema", ["discovery"]),
    t!("T1482", "Descoberta de relações de confiança do domínio", ["discovery"]),
    t!("T1580", "Descoberta de infraestrutura de nuvem", ["discovery"]),
    t!("T1613", "Descoberta de contêineres", ["discovery"]),
    t!("T1021", "Serviços remotos", ["lateral-movement"]),
    t!("T1021.001", "Área de trabalho remota (RDP)", ["lateral-movement"]),
    t!("T1021.002", "Compartilhamentos administrativos SMB", ["lateral-movement"]),
    t!("T1021.004", "SSH", ["lateral-movement"]),
    t!("T1021.006", "Windows Remote Management", ["lateral-movement"]),
    t!("T1570", "Transferência lateral de ferramentas", ["lateral-movement"]),
    t!("T1550.002", "Pass the hash", ["lateral-movement", "defense-evasion"]),
    t!("T1005", "Dados do sistema local", ["collection"]),
    t!("T1530", "Dados de armazenamento em nuvem", ["collection"]),
    t!("T1560", "Arquivamento dos dados coletados", ["collection"]),
    t!("T1071", "Protocolo de camada de aplicação", ["command-and-control"]),
    t!("T1071.001", "Protocolos web", ["command-and-control"]),
    t!("T1071.004", "DNS", ["command-and-control"]),
    t!("T1090", "Proxy", ["command-and-control"]),
    t!("T1572", "Tunelamento de protocolo", ["command-and-control"]),
    t!("T1105", "Transferência de ferramentas", ["command-and-control"]),
    t!("T1219", "Software de acesso remoto", ["command-and-control"]),
    t!("T1571", "Porta não padrão", ["command-and-control"]),
    t!("T1573", "Canal criptografado", ["command-and-control"]),
    t!("T1568", "Resolução dinâmica", ["command-and-control"]),
    t!("T1041", "Exfiltração pelo canal de C2", ["exfiltration"]),
    t!("T1048", "Exfiltração por protocolo alternativo", ["exfiltration"]),
    t!("T1567", "Exfiltração para serviço web", ["exfiltration"]),
    t!("T1537", "Transferência para outra conta de nuvem", ["exfiltration"]),
    t!("T1486", "Criptografia de dados para impacto", ["impact"]),
    t!("T1490", "Inibição da recuperação do sistema", ["impact"]),
    t!("T1489", "Interrupção de serviço", ["impact"]),
    t!("T1485", "Destruição de dados", ["impact"]),
    t!("T1496", "Sequestro de recursos", ["impact"]),
    t!("T1531", "Remoção de acesso a contas", ["impact"]),
    t!("T1499", "Negação de serviço no endpoint", ["impact"]),
    t!("T1498", "Negação de serviço de rede", ["impact"]),
];

#[derive(Clone, Serialize)]
pub struct AttackRef {
    pub id: String,
    pub name: String,
    pub tactics: Vec<String>,
}

pub fn tactic(key: &str) -> Option<&'static Tactic> {
    let key = key.trim().to_lowercase().replace(['_', ' '], "-");
    TACTICS.iter().find(|t| t.key == key || t.id.eq_ignore_ascii_case(&key))
}

pub fn tactic_order(key: &str) -> usize {
    TACTICS.iter().position(|t| t.key == key).unwrap_or(TACTICS.len())
}

pub fn technique(id: &str) -> Option<&'static Technique> {
    let id = id.trim().to_uppercase();
    TECHNIQUES
        .iter()
        .find(|t| t.id == id)
        .or_else(|| id.split('.').next().and_then(|parent| TECHNIQUES.iter().find(|t| t.id == parent)))
}

/// Reference for a technique id, keeping unknown ids visible.
pub fn reference(id: &str, extra_tactics: &[String]) -> AttackRef {
    let upper = id.trim().to_uppercase();
    match technique(&upper) {
        Some(found) => AttackRef {
            id: upper.clone(),
            name: if found.id == upper {
                found.name.to_string()
            } else {
                format!("{} ({})", found.name, upper)
            },
            tactics: found.tactics.iter().map(|t| t.to_string()).collect(),
        },
        None => AttackRef { id: upper.clone(), name: upper, tactics: extra_tactics.to_vec() },
    }
}

/// Techniques for threat-catalog categories whose rules carry no explicit mapping.
pub fn for_threat_category(category: &str) -> &'static [&'static str] {
    match category {
        "XSS" | "Injeção SQL" | "LFI/RFI" | "XXE" | "Erros de banco com contexto" | "Exposição web" => &["T1190"],
        "Injeção de comandos" | "Injeção de templates" => &["T1190", "T1059"],
        "Traversal de caminhos" => &["T1190", "T1083"],
        "SSRF" => &["T1190", "T1552.005"],
        "Desserialização" => &["T1190", "T1203"],
        "Upload e webshell" => &["T1505.003"],
        "Shell reverso" => &["T1059", "T1071"],
        "C2 e túneis" => &["T1071", "T1572"],
        "Exfiltração" => &["T1048"],
        "Credenciais e autenticação" => &["T1110", "T1552"],
        "Nuvem" => &["T1078.004", "T1580"],
        "Kubernetes" => &["T1609", "T1611"],
        "Mineração" => &["T1496"],
        "PowerShell" => &["T1059.001", "T1027"],
        "Credenciais Windows" => &["T1003"],
        "Active Directory" => &["T1558", "T1087"],
        "Evasão e impacto" => &["T1070", "T1562", "T1490"],
        "Persistência Windows" => &["T1543.003", "T1053.005", "T1547.001"],
        "Shell e persistência Unix" => &["T1059.004", "T1053.003", "T1098.004"],
        "Movimento lateral e túneis" => &["T1021", "T1570"],
        "Configurações e arquivos expostos" => &["T1552.001"],
        "Dumps e resultados de dados" => &["T1005"],
        "Respostas de metadados e identidade cloud" => &["T1552.005"],
        "Tokens e material secreto expostos" => &["T1552", "T1528"],
        "Tracebacks e respostas de execução" => &["T1203"],
        "Saídas Unix" | "Saídas Windows" => &["T1082", "T1033", "T1087"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for technique in TECHNIQUES {
            assert!(seen.insert(technique.id), "duplicate {}", technique.id);
            assert!(!technique.tactics.is_empty());
            for tactic_key in technique.tactics {
                assert!(tactic(tactic_key).is_some(), "{} has unknown tactic {tactic_key}", technique.id);
            }
        }
        assert_eq!(reference("t1110.003", &[]).name, "Pulverização de senhas");
        assert_eq!(reference("T1110.999", &[]).tactics, vec!["credential-access"]);
        assert_eq!(reference("T9999", &["impact".into()]).tactics, vec!["impact"]);
        assert_eq!(tactic("Credential_Access").unwrap().label, "Acesso a credenciais");
    }
}
