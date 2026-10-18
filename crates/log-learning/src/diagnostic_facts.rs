//! Typed measurements and narrowly recognized error reports. Text produces
//! reported symptoms, never inferred resource measurements or successful effects.
use crate::DiagnosticFeature as F;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticMetric {
    PoolActive,
    PoolLimit,
    PoolPending,
    AcquireMs,
    QueryMs,
    QueryLimitMs,
    DependencyMs,
    DependencyLimitMs,
    LockWaitMs,
    RequestRate,
    RequestRateBaseline,
    RetryCount,
    RetryLimit,
}
pub const DIAGNOSTIC_METRICS: &[DiagnosticMetric] = &[
    DiagnosticMetric::PoolActive,
    DiagnosticMetric::PoolLimit,
    DiagnosticMetric::PoolPending,
    DiagnosticMetric::AcquireMs,
    DiagnosticMetric::QueryMs,
    DiagnosticMetric::QueryLimitMs,
    DiagnosticMetric::DependencyMs,
    DiagnosticMetric::DependencyLimitMs,
    DiagnosticMetric::LockWaitMs,
    DiagnosticMetric::RequestRate,
    DiagnosticMetric::RequestRateBaseline,
    DiagnosticMetric::RetryCount,
    DiagnosticMetric::RetryLimit,
];
impl DiagnosticMetric {
    pub fn role(self) -> &'static str {
        use DiagnosticMetric::*;
        match self {
            PoolActive => "pool_active",
            PoolLimit => "pool_limit",
            PoolPending => "pool_pending",
            AcquireMs => "acquire_ms",
            QueryMs => "query_ms",
            QueryLimitMs => "query_limit_ms",
            DependencyMs => "dependency_ms",
            DependencyLimitMs => "dependency_limit_ms",
            LockWaitMs => "lock_wait_ms",
            RequestRate => "request_rate",
            RequestRateBaseline => "request_rate_baseline",
            RetryCount => "retry_count",
            RetryLimit => "retry_limit",
        }
    }
    pub fn unit(self) -> &'static str {
        use DiagnosticMetric::*;
        match self {
            AcquireMs | QueryMs | QueryLimitMs | DependencyMs | DependencyLimitMs | LockWaitMs => {
                "ms"
            }
            RequestRate | RequestRateBaseline => "1/s",
            _ => "count",
        }
    }
    pub fn label(self) -> &'static str {
        use DiagnosticMetric::*;
        match self {
            PoolActive => "Conexões ativas",
            PoolLimit => "Limite do pool",
            PoolPending => "Fila do pool",
            AcquireMs => "Aquisição de conexão (ms)",
            QueryMs => "Execução de consulta (ms)",
            QueryLimitMs => "Limite declarado de consulta (ms)",
            DependencyMs => "Duração da dependência (ms)",
            DependencyLimitMs => "Limite declarado da dependência (ms)",
            LockWaitMs => "Espera por lock (ms)",
            RequestRate => "Requisições/s",
            RequestRateBaseline => "Referência declarada de requisições/s",
            RetryCount => "Repetições",
            RetryLimit => "Limite declarado de repetições",
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticRecord {
    pub message: String,
    pub code: String,
    pub exception: String,
    pub auth_reason: String,
    pub product: String,
    pub failure: bool,
    pub success: bool,
    pub documentation: bool,
    pub deployment: bool,
    pub metrics: BTreeMap<DiagnosticMetric, f64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticEventFacts {
    pub mask: u64,
    pub typed: u64,
    pub failure: bool,
    pub metrics: BTreeMap<DiagnosticMetric, f64>,
    pub sources: BTreeMap<String, String>,
}
impl DiagnosticEventFacts {
    pub fn validate(&self) -> Result<(), String> {
        if !crate::valid_feature_mask(self.mask)
            || self.typed & !self.mask != 0
            || self.metrics.len() > 13
            || self
                .metrics
                .values()
                .any(|n| !n.is_finite() || *n < 0.0 || *n > 1e15)
            || self.sources.len() > 48
            || self
                .sources
                .iter()
                .any(|(k, v)| k.len() > 64 || v.len() > 256)
        {
            return Err("Fatos diagnósticos incompatíveis ou acima do orçamento".into());
        }
        Ok(())
    }
    pub fn add(&mut self, feature: F, typed: bool) {
        self.mask |= feature.bit();
        if typed {
            self.typed |= feature.bit();
        }
    }
}
pub fn extract_diagnostic(record: &DiagnosticRecord) -> DiagnosticEventFacts {
    use DiagnosticMetric::*;
    let mut facts = DiagnosticEventFacts::default();
    if record.documentation {
        return facts;
    }
    facts.failure = record.failure && !record.success;
    facts.metrics = record
        .metrics
        .iter()
        .filter(|(_, n)| n.is_finite() && **n >= 0.0 && **n <= 1e15)
        .map(|(&k, &v)| (k, v))
        .collect();
    let get = |key| facts.metrics.get(&key).copied();
    let mut measured = Vec::new();
    if let (Some(active), Some(limit)) = (get(PoolActive), get(PoolLimit)) {
        if limit > 0.0 {
            measured.push(F::PoolMeasured);
            measured.push(if active >= limit {
                F::PoolAtLimit
            } else {
                F::PoolHasCapacity
            });
        }
    }
    if get(PoolPending).is_some_and(|n| n > 0.0) {
        measured.push(F::PoolQueued);
    }
    if get(AcquireMs).is_some() {
        measured.push(F::AcquisitionMeasured);
    }
    if get(QueryMs).is_some() {
        measured.push(F::QueryMeasured);
    }
    if get(DependencyMs).is_some() {
        measured.push(F::DependencyMeasured);
    }
    if get(QueryMs)
        .zip(get(QueryLimitMs))
        .is_some_and(|(v, limit)| limit > 0.0 && v > limit)
    {
        measured.push(F::QuerySlow);
    }
    if get(DependencyMs)
        .zip(get(DependencyLimitMs))
        .is_some_and(|(v, limit)| limit > 0.0 && v > limit)
    {
        measured.push(F::ReadTimeout);
    }
    if get(LockWaitMs).is_some_and(|v| v > 0.0) {
        measured.push(F::LockWait);
    }
    if get(RequestRate)
        .zip(get(RequestRateBaseline))
        .is_some_and(|(v, base)| base > 0.0 && v > base * 2.0)
    {
        measured.push(F::RequestRateIncrease);
    }
    if get(RetryCount)
        .zip(get(RetryLimit))
        .is_some_and(|(v, limit)| v > limit)
    {
        measured.push(F::RetryIncrease);
    }
    for feature in measured {
        facts.add(feature, true);
    }
    if record.deployment {
        facts.add(F::DeployObserved, true);
    }
    if record.success
        && matches!(
            record.product.as_str(),
            "authentication" | "sshd" | "okta" | "entra"
        )
    {
        facts.add(F::AuthenticationSuccess, true);
    }
    // Successes and documentation cannot manufacture failures from quoted text.
    if !facts.failure {
        return facts;
    }
    let code = record.code.trim().to_ascii_uppercase();
    let auth = record.auth_reason.trim().to_ascii_lowercase();
    let exception = record.exception.to_ascii_lowercase();
    for (codes, feature) in [
        (
            &["ECONNREFUSED", "WSAECONNREFUSED"][..],
            F::ConnectionRefused,
        ),
        (&["ECONNRESET"][..], F::ConnectionReset),
        (
            &[
                "ENOTFOUND",
                "EAI_AGAIN",
                "EAI_NONAME",
                "DNS_ERROR_RCODE_NAME_ERROR",
            ][..],
            F::DnsFailure,
        ),
        (
            &[
                "CERT_HAS_EXPIRED",
                "X509_V_ERR_CERT_HAS_EXPIRED",
                "CERT_E_EXPIRED",
            ][..],
            F::TlsExpired,
        ),
        (
            &[
                "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
                "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
                "DEPTH_ZERO_SELF_SIGNED_CERT",
                "CERT_E_UNTRUSTEDROOT",
            ][..],
            F::TlsUntrusted,
        ),
        (
            &["ERR_TLS_CERT_ALTNAME_INVALID", "CERT_E_CN_NO_MATCH"][..],
            F::TlsHostname,
        ),
        (
            &[
                "EXPIREDTOKEN",
                "EXPIRED_TOKEN",
                "AADSTS7000222",
                "AADSTS700082",
            ][..],
            F::CredentialExpired,
        ),
    ] {
        if codes.contains(&code.as_str()) {
            facts.add(feature, true);
        }
    }
    if matches!(
        record.product.as_str(),
        "database" | "postgresql" | "mysql" | "application" | "java" | "dotnet"
    ) {
        match code.as_str() {
            "53300" => facts.add(F::ConnectionLimit, true),
            "40P01" => facts.add(F::Deadlock, true),
            "1213" if record.product == "mysql" => facts.add(F::Deadlock, true),
            "55P03" => facts.add(F::LockWait, true),
            "08001" | "08006" => facts.add(F::ConnectionFailure, true),
            "28P01" => facts.add(F::CredentialInvalid, true),
            _ => (),
        }
    }
    if record.product == "web" {
        match code.as_str() {
            "401" => facts.add(F::AuthenticationFailure, true),
            "403" => facts.add(F::AccessDenied, true),
            _ => (),
        }
    }
    match auth.as_str() {
        "expired" | "credential_expired" | "password_expired" | "token_expired" | "0xc0000071" => {
            facts.add(F::CredentialExpired, true)
        }
        "invalid_password" | "invalid_credentials" | "bad_credentials" | "0xc000006a" => {
            facts.add(F::CredentialInvalid, true)
        }
        "access_denied" | "permission_denied" | "insufficient_scope" | "0xc000015b" => {
            facts.add(F::AccessDenied, true)
        }
        _ => (),
    }
    if matches!(
        record.product.as_str(),
        "authentication" | "sshd" | "okta" | "entra"
    ) {
        facts.add(F::AuthenticationFailure, true);
    }
    if exception.ends_with("unknownhostexception") {
        facts.add(F::DnsFailure, true);
    }
    if exception.ends_with("certificateexpiredexception") {
        facts.add(F::TlsExpired, true);
    }
    let text = record
        .message
        .chars()
        .take(8192)
        .collect::<String>()
        .to_ascii_lowercase();
    let quoted = [
        "example:",
        "sample:",
        "documentation:",
        "expected error:",
        "simulated error:",
        "request payload:",
    ]
    .iter()
    .any(|p| text.trim_start().starts_with(p))
        || text.contains("request.body=")
        || text.contains("user_input=");
    if quoted {
        return facts;
    }
    let phrases: &[(F, &[&str], &[&str])] = &[
        (
            F::PoolExhaustedReported,
            &[
                "connection pool exhausted",
                "pool is exhausted",
                "pool exhausted",
                "no available connection in pool",
                "pool de conexões esgotado",
            ],
            &[
                "pool not exhausted",
                "pool is not exhausted",
                "no pool exhausted",
            ],
        ),
        (
            F::AcquireTimeout,
            &[
                "timeout acquiring connection",
                "timeout waiting for connection from pool",
                "timed out waiting for a connection",
                "connection is not available, request timed out",
                "timeout ao obter conexão",
            ],
            &["no timeout acquiring", "without timeout acquiring"],
        ),
        (
            F::ConnectionLimit,
            &[
                "too many connections",
                "remaining connection slots are reserved",
            ],
            &["not too many connections"],
        ),
        (
            F::Deadlock,
            &["deadlock detected", "deadlock found"],
            &["no deadlock detected", "no deadlock found"],
        ),
        (
            F::LockWait,
            &[
                "lock wait timeout exceeded",
                "could not obtain lock",
                "waiting for lock",
            ],
            &["not waiting for lock"],
        ),
        (
            F::QuerySlow,
            &[
                "slow query:",
                "slow query detected",
                "query exceeded duration limit",
            ],
            &["no slow query", "not a slow query"],
        ),
        (
            F::ConnectionRefused,
            &["connection refused", "econnrefused"],
            &["no connection refused", "not connection refused"],
        ),
        (
            F::ConnectionReset,
            &["connection reset by peer", "econnreset"],
            &["no connection reset"],
        ),
        (
            F::ConnectTimeout,
            &[
                "connect timed out",
                "connection timed out",
                "connect timeout",
            ],
            &["no connect timeout", "without connect timeout"],
        ),
        (
            F::ReadTimeout,
            &[
                "read timed out",
                "read timeout",
                "upstream timed out while reading response",
            ],
            &["no read timeout", "without read timeout"],
        ),
        (
            F::DnsFailure,
            &[
                "getaddrinfo enotfound",
                "getaddrinfo eai_again",
                "name or service not known",
                "temporary failure in name resolution",
                "could not resolve host",
                "unknownhostexception",
            ],
            &["no unknownhostexception"],
        ),
        (
            F::TlsExpired,
            &[
                "certificate has expired",
                "certificate expired",
                "certificateexpiredexception",
                "certificado expirado",
            ],
            &[
                "certificate not expired",
                "certificate is not expired",
                "no certificate expired",
                "certificate expired=false",
            ],
        ),
        (
            F::TlsUntrusted,
            &[
                "unable to get local issuer certificate",
                "unable to verify the first certificate",
                "self signed certificate",
                "pkix path building failed",
            ],
            &["no self signed certificate"],
        ),
        (
            F::TlsHostname,
            &[
                "hostname mismatch",
                "no subject alternative dns name matching",
                "does not match the certificate's altnames",
            ],
            &["no hostname mismatch", "without hostname mismatch"],
        ),
        (
            F::CredentialExpired,
            &[
                "password has expired",
                "password expired",
                "token has expired",
                "token expired",
                "credentials have expired",
            ],
            &[
                "password not expired",
                "token not expired",
                "token is not expired",
            ],
        ),
        (
            F::CredentialInvalid,
            &[
                "password authentication failed",
                "invalid credentials",
                "invalid password",
                "bad credentials",
            ],
            &[
                "no invalid credentials",
                "not invalid credentials",
                "invalid credentials count=0",
            ],
        ),
        (
            F::AccessDenied,
            &[
                "insufficient permissions",
                "insufficient_scope",
                "permission denied for",
                "authorization denied",
            ],
            &["no insufficient permissions"],
        ),
    ];
    for &(feature, positive, negative) in phrases {
        if positive.iter().any(|p| text.contains(p)) && !negative.iter().any(|p| text.contains(p)) {
            facts.add(feature, false);
        }
    }
    if (text.contains("while reading response") || text.contains("awaiting response headers"))
        && (text.contains("timed out") || text.contains("timeout"))
    {
        facts.add(F::ReadTimeout, false);
        // Nginx includes the operating-system phrase "Connection timed out"
        // even when the actual phase is response reading. Preserve that phase.
        if !record.code.eq_ignore_ascii_case("ETIMEDOUT_CONNECT") {
            facts.mask &= !F::ConnectTimeout.bit();
            facts.typed &= !F::ConnectTimeout.bit();
        }
    }
    if text.contains("certificate signed by unknown authority") {
        facts.add(F::TlsUntrusted, false);
    }
    if text.contains("too many clients already") {
        facts.add(F::ConnectionLimit, false);
    }
    if text.contains("no such host") && text.contains("lookup ") {
        facts.add(F::DnsFailure, false);
    }
    facts
}
