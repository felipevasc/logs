//! Small, deterministic differential diagnoses. A rule describes what the
//! observations support and which local check can distinguish the alternatives.
//! Scores order hypotheses; they are not probabilities or evidence grades.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DIAGNOSTIC_VERSION: &str = "diagnostics-1";
pub const DIAGNOSTIC_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum DiagnosticFeature {
    AcquireTimeout,
    PoolAtLimit,
    PoolQueued,
    PoolHasCapacity,
    ConnectionLimit,
    QuerySlow,
    LockWait,
    Deadlock,
    ConnectTimeout,
    ReadTimeout,
    ConnectionRefused,
    ConnectionReset,
    DependencyFailure,
    DnsFailure,
    TlsExpired,
    TlsUntrusted,
    TlsHostname,
    CredentialExpired,
    CredentialInvalid,
    AccessDenied,
    AuthenticationFailure,
    AuthenticationSuccess,
    DeployObserved,
    VersionAssociation,
    LatencyChange,
    FailureChange,
    RequestRateIncrease,
    RetryIncrease,
    QueryMeasured,
    AcquisitionMeasured,
    DependencyMeasured,
    VersionMeasured,
    PoolMeasured,
    TraceMeasured,
    ConnectionFailure,
    PoolExhaustedReported,
    LearnedDeviation,
}
pub const DIAGNOSTIC_FEATURES: &[DiagnosticFeature] = &[
    DiagnosticFeature::AcquireTimeout,
    DiagnosticFeature::PoolAtLimit,
    DiagnosticFeature::PoolQueued,
    DiagnosticFeature::PoolHasCapacity,
    DiagnosticFeature::ConnectionLimit,
    DiagnosticFeature::QuerySlow,
    DiagnosticFeature::LockWait,
    DiagnosticFeature::Deadlock,
    DiagnosticFeature::ConnectTimeout,
    DiagnosticFeature::ReadTimeout,
    DiagnosticFeature::ConnectionRefused,
    DiagnosticFeature::ConnectionReset,
    DiagnosticFeature::DependencyFailure,
    DiagnosticFeature::DnsFailure,
    DiagnosticFeature::TlsExpired,
    DiagnosticFeature::TlsUntrusted,
    DiagnosticFeature::TlsHostname,
    DiagnosticFeature::CredentialExpired,
    DiagnosticFeature::CredentialInvalid,
    DiagnosticFeature::AccessDenied,
    DiagnosticFeature::AuthenticationFailure,
    DiagnosticFeature::AuthenticationSuccess,
    DiagnosticFeature::DeployObserved,
    DiagnosticFeature::VersionAssociation,
    DiagnosticFeature::LatencyChange,
    DiagnosticFeature::FailureChange,
    DiagnosticFeature::RequestRateIncrease,
    DiagnosticFeature::RetryIncrease,
    DiagnosticFeature::QueryMeasured,
    DiagnosticFeature::AcquisitionMeasured,
    DiagnosticFeature::DependencyMeasured,
    DiagnosticFeature::VersionMeasured,
    DiagnosticFeature::PoolMeasured,
    DiagnosticFeature::TraceMeasured,
    DiagnosticFeature::ConnectionFailure,
    DiagnosticFeature::PoolExhaustedReported,
    DiagnosticFeature::LearnedDeviation,
];
impl DiagnosticFeature {
    pub fn bit(self) -> u64 {
        1u64 << self as u8
    }
    pub fn label(self) -> &'static str {
        use DiagnosticFeature::*;
        match self {
            AcquireTimeout => "Timeout para obter uma conexão relatado",
            PoolAtLimit => "Conexões ativas atingem o limite declarado do pool",
            PoolQueued => "Há espera na fila do pool",
            PoolHasCapacity => "Há observação de capacidade livre no pool",
            ConnectionLimit => "Limite de conexões do servidor relatado",
            QuerySlow => "Lentidão de consulta relatada ou duração acima do limite declarado",
            LockWait => "Espera por lock relatada",
            Deadlock => "Deadlock relatado",
            ConnectTimeout => "Timeout de conexão relatado",
            ReadTimeout => "Timeout de resposta relatado ou duração acima de limite declarado",
            ConnectionRefused => "Conexão recusada relatada",
            ConnectionReset => "Conexão encerrada pelo par relatada",
            ConnectionFailure => "Falha ao estabelecer conexão relatada",
            PoolExhaustedReported => "Esgotamento do pool relatado pelo componente",
            LearnedDeviation => "Desvio estatístico apontado pela referência local",
            DependencyFailure => "Resposta de falha da dependência observada",
            DnsFailure => "Falha de resolução DNS relatada",
            TlsExpired => "Certificado expirado relatado",
            TlsUntrusted => "Falha de confiança no certificado relatada",
            TlsHostname => "Nome do certificado incompatível relatado",
            CredentialExpired => "Credencial expirada relatada",
            CredentialInvalid => "Credencial inválida relatada",
            AccessDenied => "Operação negada por autorização relatada",
            AuthenticationFailure => "Falha de autenticação observada",
            AuthenticationSuccess => "Autenticação bem-sucedida observada no contexto",
            DeployObserved => "Mudança de implantação declarada",
            VersionAssociation => "Falhas se concentram em uma versão com controles conhecidos",
            LatencyChange => "Mudança coletiva de duração observada",
            FailureChange => "Mudança coletiva na taxa de falhas observada",
            RequestRateIncrease => "Demanda excedeu uma referência explicitamente declarada",
            RetryIncrease => "Repetições excederam um limite explicitamente declarado",
            QueryMeasured => "Duração de consulta disponível",
            AcquisitionMeasured => "Tempo para obter conexão disponível",
            DependencyMeasured => "Duração da dependência disponível",
            VersionMeasured => "Versão identificada",
            PoolMeasured => "Ocupação e limite do pool disponíveis",
            TraceMeasured => "Identificador de requisição/trace disponível",
        }
    }
    /// Dependent symptoms contribute at most one weight in each group. A code
    /// and the message reporting that code cannot inflate the score twice.
    pub fn evidence_group(self) -> u8 {
        use DiagnosticFeature::*;
        match self {
            PoolAtLimit | PoolQueued | PoolHasCapacity | PoolMeasured => 1,
            QuerySlow | QueryMeasured | AcquisitionMeasured | DependencyMeasured => 2,
            VersionAssociation | VersionMeasured => 3,
            LatencyChange | FailureChange | LearnedDeviation => 4,
            RequestRateIncrease | RetryIncrease => 5,
            DeployObserved => 6,
            TraceMeasured => 7,
            _ => 0,
        }
    }
}
pub fn valid_feature_mask(mask: u64) -> bool {
    mask & !((1u64 << DIAGNOSTIC_FEATURES.len()) - 1) == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Diagnosis {
    PoolExhaustion,
    DatabaseConnectionLimit,
    DatabaseLockContention,
    SlowDatabase,
    DependencyUnavailable,
    DependencySlow,
    DnsResolution,
    TlsCertificateExpired,
    TlsTrust,
    TlsHostnameMismatch,
    DeploymentRegression,
    ExpiredCredentials,
    InvalidCredentials,
    AuthorizationDenied,
}
impl Diagnosis {
    pub fn family(self) -> &'static str {
        use Diagnosis::*;
        match self {
            PoolExhaustion | DatabaseConnectionLimit | DatabaseLockContention | SlowDatabase => {
                "database"
            }
            DependencyUnavailable | DependencySlow => "dependency",
            DnsResolution | TlsCertificateExpired | TlsTrust | TlsHostnameMismatch => "dns_tls",
            DeploymentRegression => "deployment",
            ExpiredCredentials | InvalidCredentials | AuthorizationDenied => "authentication",
        }
    }
    pub fn title(self) -> &'static str {
        use Diagnosis::*;
        match self {
            PoolExhaustion => "Espera ou saturação do pool de conexões",
            DatabaseConnectionLimit => "Limite de conexões no servidor de banco",
            DatabaseLockContention => "Contenção ou deadlock no banco",
            SlowDatabase => "Lentidão na execução de consultas",
            DependencyUnavailable => "Falha ao estabelecer conexão com a dependência",
            DependencySlow => "Demora na resposta da dependência",
            DnsResolution => "Falha de resolução DNS",
            TlsCertificateExpired => "Certificado expirado no caminho TLS",
            TlsTrust => "Cadeia de confiança TLS não aceita",
            TlsHostnameMismatch => "Nome solicitado incompatível com o certificado",
            DeploymentRegression => "Regressão associada à mudança de versão",
            ExpiredCredentials => "Credencial expirada",
            InvalidCredentials => "Credencial inválida ou divergente",
            AuthorizationDenied => "Permissão insuficiente para a operação",
        }
    }
    pub fn missing(self) -> &'static [&'static str] {
        use Diagnosis::*;
        match self {
            PoolExhaustion => &[
                "Ocupação, limite e fila do pool no momento da falha",
                "Duração de aquisição versus execução de consulta",
                "Conexões liberadas por requisição e versão",
            ],
            DatabaseConnectionLimit => &[
                "Conexões por aplicação e limite do servidor",
                "Capacidade disponível e conexões ociosas",
            ],
            DatabaseLockContention => &[
                "Sessões bloqueadoras, transações e recursos envolvidos",
                "Tempos de espera e ordem de aquisição de locks",
            ],
            SlowDatabase => &[
                "Plano e duração das consultas comparáveis",
                "Carga, locks e tempo de aquisição de conexão",
            ],
            DependencyUnavailable => &[
                "Destino e porta efetivamente usados",
                "Saúde do serviço, conectividade e configuração no intervalo",
            ],
            DependencySlow => &[
                "Tempo de conexão versus espera da resposta",
                "Latência da dependência e limites de timeout",
            ],
            DnsResolution => &[
                "Nome consultado e resolvedor utilizado",
                "Resposta DNS, cache e configuração do mesmo host",
            ],
            TlsCertificateExpired => &[
                "Certificado apresentado, validade e relógio do cliente",
                "Proxy ou intermediário presente no caminho",
            ],
            TlsTrust => &[
                "Cadeia apresentada e repositório de confiança do cliente",
                "Alteração de autoridade, proxy ou configuração",
            ],
            TlsHostnameMismatch => &[
                "Nome solicitado e SAN do certificado apresentado",
                "SNI, proxy e endereço efetivo",
            ],
            DeploymentRegression => &[
                "Revisão e horário do deploy",
                "Operações, hosts, dependências e carga comparáveis",
                "Confirmação independente ou reversão controlada",
            ],
            ExpiredCredentials => &[
                "Razão explícita do provedor e validade da credencial",
                "Rotação, relógio e novas tentativas da mesma identidade",
            ],
            InvalidCredentials => &[
                "Razão do provedor e identidade utilizada",
                "Configuração, rotação e origem das tentativas",
            ],
            AuthorizationDenied => &[
                "Ação, recurso, identidade e política efetiva",
                "Mudança de permissão ou configuração",
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticQuery {
    Overview,
    Pool,
    Database,
    Dependency,
    DnsTls,
    Versions,
    Authentication,
}
impl DiagnosticQuery {
    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Conferir sintomas, resultados e cobertura deste episódio",
            Self::Pool => "Comparar aquisição, fila e ocupação do pool",
            Self::Database => "Conferir consultas, locks e limites do banco",
            Self::Dependency => "Separar falha de conexão de espera da resposta",
            Self::DnsTls => "Conferir os erros DNS/TLS e seus destinos",
            Self::Versions => "Comparar versões conhecidas antes e durante a falha",
            Self::Authentication => "Comparar razões de falha e sucessos da mesma identidade",
        }
    }
    pub fn interpretation(self) -> &'static str {
        match self {
            Self::Overview => "Os registros mostram o que foi relatado; campos ausentes permanecem desconhecidos. Verifique os originais antes de atribuir uma causa.",
            Self::Pool => "Aquisição lenta com fila e ocupação no limite sustenta saturação. Consulta lenta também exige investigar o banco. Capacidade livre no mesmo host e instante enfraquece saturação; amostras de outros instantes não a refutam.",
            Self::Database => "Lock/deadlock explícito distingue contenção. Limite de conexões explícito distingue recusa no servidor. Duração sem limite ou referência comparável não prova lentidão.",
            Self::Dependency => "Erro ao conectar é diferente de timeout aguardando resposta. Compare destino, fase e duração. A ausência de uma fase nos logs não demonstra que ela foi rápida.",
            Self::DnsTls => "Compare código, host e certificado/consulta mencionados. Confirme validade, relógio, resolvedor e configuração; mensagens de erro relatam a falha observada pelo componente.",
            Self::Versions => "Compare apenas versões conhecidas, operação e janela equivalentes. Mudanças simultâneas de host, dependência ou carga podem explicar a associação. Ausência de versões concorrentes impede distinguir efeito de versão e tempo.",
            Self::Authentication => "Razões de expiração, credencial inválida e autorização são hipóteses distintas. Sucessos de outras identidades não refutam uma falha. Falhas repetidas, sozinhas, não demonstram ataque.",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticWeight {
    pub feature: DiagnosticFeature,
    pub weight: i16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRule {
    pub diagnosis: Diagnosis,
    pub any_of: Vec<DiagnosticFeature>,
    pub all_of: Vec<DiagnosticFeature>,
    pub support: Vec<DiagnosticWeight>,
    pub contradictions: Vec<DiagnosticWeight>,
    pub minimum_score: u16,
    pub query: DiagnosticQuery,
}
impl DiagnosticRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.any_of.is_empty()
            || self.any_of.len() > 8
            || self.all_of.len() > 8
            || self.support.len() > 16
            || self.contradictions.len() > 8
            || !(20..=85).contains(&self.minimum_score)
            || self
                .support
                .iter()
                .chain(&self.contradictions)
                .any(|w| !(1..=50).contains(&w.weight))
        {
            return Err("Regra diagnóstica fora do vocabulário ou orçamento".into());
        }
        for features in [
            self.any_of.clone(),
            self.all_of.clone(),
            self.support.iter().map(|w| w.feature).collect(),
            self.contradictions.iter().map(|w| w.feature).collect(),
        ] {
            if features.iter().collect::<BTreeSet<_>>().len() != features.len() {
                return Err("Predicado diagnóstico repetido".into());
            }
        }
        Ok(())
    }
}
pub fn diagnostic_rules() -> Vec<DiagnosticRule> {
    use Diagnosis::*;
    use DiagnosticFeature::*;
    let rule = |diagnosis,
                any_of: &[DiagnosticFeature],
                all_of: &[DiagnosticFeature],
                support: &[(DiagnosticFeature, i16)],
                contradictions: &[(DiagnosticFeature, i16)],
                query| DiagnosticRule {
        diagnosis,
        any_of: any_of.to_vec(),
        all_of: all_of.to_vec(),
        support: support
            .iter()
            .map(|&(feature, weight)| DiagnosticWeight { feature, weight })
            .collect(),
        contradictions: contradictions
            .iter()
            .map(|&(feature, weight)| DiagnosticWeight { feature, weight })
            .collect(),
        minimum_score: 30,
        query,
    };
    vec![
        rule(
            PoolExhaustion,
            &[AcquireTimeout, PoolExhaustedReported],
            &[],
            &[
                (AcquireTimeout, 35),
                (PoolExhaustedReported, 40),
                (PoolAtLimit, 30),
                (PoolQueued, 20),
                (AcquisitionMeasured, 10),
            ],
            &[(PoolHasCapacity, 15)],
            DiagnosticQuery::Pool,
        ),
        rule(
            DatabaseConnectionLimit,
            &[ConnectionLimit],
            &[],
            &[(ConnectionLimit, 45), (FailureChange, 10)],
            &[],
            DiagnosticQuery::Database,
        ),
        rule(
            DatabaseLockContention,
            &[LockWait, Deadlock],
            &[],
            &[(Deadlock, 45), (LockWait, 35), (QuerySlow, 15)],
            &[],
            DiagnosticQuery::Database,
        ),
        rule(
            SlowDatabase,
            &[QuerySlow],
            &[],
            &[(QuerySlow, 40), (ReadTimeout, 20), (LatencyChange, 10)],
            &[],
            DiagnosticQuery::Database,
        ),
        rule(
            DependencyUnavailable,
            &[
                ConnectTimeout,
                ConnectionRefused,
                ConnectionReset,
                ConnectionFailure,
            ],
            &[],
            &[
                (ConnectTimeout, 35),
                (ConnectionRefused, 40),
                (ConnectionReset, 30),
                (ConnectionFailure, 30),
                (FailureChange, 10),
            ],
            &[],
            DiagnosticQuery::Dependency,
        ),
        rule(
            DependencySlow,
            &[ReadTimeout, DependencyFailure],
            &[],
            &[
                (ReadTimeout, 35),
                (DependencyFailure, 30),
                (DependencyMeasured, 15),
                (LatencyChange, 10),
            ],
            &[],
            DiagnosticQuery::Dependency,
        ),
        rule(
            DnsResolution,
            &[DnsFailure],
            &[],
            &[(DnsFailure, 45), (FailureChange, 10)],
            &[],
            DiagnosticQuery::DnsTls,
        ),
        rule(
            TlsCertificateExpired,
            &[TlsExpired],
            &[],
            &[(TlsExpired, 45), (FailureChange, 10)],
            &[],
            DiagnosticQuery::DnsTls,
        ),
        rule(
            TlsTrust,
            &[TlsUntrusted],
            &[],
            &[(TlsUntrusted, 45), (FailureChange, 10)],
            &[],
            DiagnosticQuery::DnsTls,
        ),
        rule(
            TlsHostnameMismatch,
            &[TlsHostname],
            &[],
            &[(TlsHostname, 45), (FailureChange, 10)],
            &[],
            DiagnosticQuery::DnsTls,
        ),
        rule(
            DeploymentRegression,
            &[VersionAssociation],
            &[DeployObserved],
            &[
                (VersionAssociation, 30),
                (DeployObserved, 15),
                (FailureChange, 10),
                (LatencyChange, 10),
            ],
            &[],
            DiagnosticQuery::Versions,
        ),
        rule(
            ExpiredCredentials,
            &[CredentialExpired],
            &[],
            &[(CredentialExpired, 45), (AuthenticationFailure, 20)],
            &[],
            DiagnosticQuery::Authentication,
        ),
        rule(
            InvalidCredentials,
            &[CredentialInvalid],
            &[],
            &[(CredentialInvalid, 40), (AuthenticationFailure, 20)],
            &[],
            DiagnosticQuery::Authentication,
        ),
        rule(
            AuthorizationDenied,
            &[AccessDenied],
            &[],
            &[(AccessDenied, 40)],
            &[],
            DiagnosticQuery::Authentication,
        ),
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticReference {
    pub event_ref: String,
    pub event_id: i64,
    pub timestamp: Option<i64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSupport {
    pub observations: u64,
    pub typed_observations: u64,
    pub event_refs: Vec<DiagnosticReference>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSummary {
    pub events: u64,
    pub failure_events: u64,
    pub change_events: u64,
    pub features: BTreeMap<DiagnosticFeature, DiagnosticSupport>,
}
impl DiagnosticSummary {
    pub fn mask(&self) -> u64 {
        self.features.keys().fold(0, |n, f| n | f.bit())
    }
    pub fn observe(
        &mut self,
        reference: DiagnosticReference,
        mask: u64,
        typed: u64,
        failure: bool,
    ) {
        self.events += 1;
        self.failure_events += u64::from(failure);
        self.change_events += u64::from(
            mask & (DiagnosticFeature::LatencyChange.bit()
                | DiagnosticFeature::FailureChange.bit())
                != 0,
        );
        for &feature in DIAGNOSTIC_FEATURES {
            if mask & feature.bit() == 0 {
                continue;
            }
            let entry = self.features.entry(feature).or_default();
            entry.observations += 1;
            entry.typed_observations += u64::from(typed & feature.bit() != 0);
            if entry.event_refs.len() < 8 {
                entry.event_refs.push(reference.clone());
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticObservation {
    pub feature: DiagnosticFeature,
    pub description: String,
    pub state: String,
    pub observations: u64,
    pub typed_observations: u64,
    pub event_refs: Vec<DiagnosticReference>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticHypothesis {
    pub diagnosis: Diagnosis,
    pub family: String,
    pub title: String,
    pub score: u16,
    pub score_kind: String,
    pub state: String,
    pub support: Vec<DiagnosticObservation>,
    pub counter_evidence: Vec<DiagnosticObservation>,
    pub missing_evidence: Vec<String>,
    pub next_query: DiagnosticQuery,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticAssessment {
    pub version: String,
    pub hypotheses: Vec<DiagnosticHypothesis>,
    pub abstention: Option<String>,
    pub next_queries: Vec<DiagnosticQuery>,
    pub measured_accuracy: Option<f64>,
}
fn observation(summary: &DiagnosticSummary, feature: DiagnosticFeature) -> DiagnosticObservation {
    let support = &summary.features[&feature];
    DiagnosticObservation {
        feature,
        description: feature.label().into(),
        state: "observed".into(),
        observations: support.observations,
        typed_observations: support.typed_observations,
        event_refs: support.event_refs.clone(),
    }
}
pub fn assess_diagnostic(
    summary: &DiagnosticSummary,
    rules: &[DiagnosticRule],
) -> DiagnosticAssessment {
    let mut hypotheses = Vec::new();
    for rule in rules {
        if summary.failure_events == 0
            && summary.change_events == 0
            && !summary.features.contains_key(&DiagnosticFeature::QuerySlow)
            && !summary.features.contains_key(&DiagnosticFeature::LockWait)
        {
            continue;
        }
        let has = |feature: &DiagnosticFeature| summary.features.contains_key(feature);
        if !rule.any_of.iter().any(has) || !rule.all_of.iter().all(has) {
            continue;
        }
        let mut groups = [0i16; 8];
        let mut penalties = [0i16; 8];
        let mut support = Vec::new();
        let mut counter_evidence = Vec::new();
        for weight in &rule.support {
            if has(&weight.feature) {
                let group = weight.feature.evidence_group() as usize;
                groups[group] = groups[group].max(weight.weight);
                support.push(observation(summary, weight.feature));
            }
        }
        for weight in &rule.contradictions {
            if has(&weight.feature) {
                let group = weight.feature.evidence_group() as usize;
                penalties[group] = penalties[group].max(weight.weight);
                counter_evidence.push(observation(summary, weight.feature));
            }
        }
        let support_score = groups.iter().sum::<i16>();
        if support_score < (rule.minimum_score as i16) {
            continue;
        }
        let score = (support_score - penalties.iter().sum::<i16>()).clamp(0, 85) as u16;
        hypotheses.push(DiagnosticHypothesis {
            diagnosis: rule.diagnosis,
            family: rule.diagnosis.family().into(),
            title: rule.diagnosis.title().into(),
            score,
            score_kind: "ordinal_support_not_probability".into(),
            state: "hypothesis_requires_confirmation".into(),
            support,
            counter_evidence,
            missing_evidence: rule
                .diagnosis
                .missing()
                .iter()
                .map(|s| (*s).into())
                .collect(),
            next_query: rule.query,
        });
    }
    hypotheses.sort_by(|a, b| b.score.cmp(&a.score).then(a.diagnosis.cmp(&b.diagnosis)));
    hypotheses.truncate(3);
    let abstention = hypotheses.is_empty().then(||"Há sintomas ou mudança no contexto, mas os dados disponíveis não distinguem uma causa das famílias suportadas. Confira os resultados, códigos e a cobertura do episódio.".into());
    let mut next_queries = Vec::new();
    for hypothesis in &hypotheses {
        if !next_queries.contains(&hypothesis.next_query) {
            next_queries.push(hypothesis.next_query);
        }
    }
    if next_queries.is_empty() {
        next_queries.push(DiagnosticQuery::Overview);
    }
    DiagnosticAssessment {
        version: DIAGNOSTIC_VERSION.into(),
        hypotheses,
        abstention,
        next_queries,
        measured_accuracy: None,
    }
}
