//! Conservative investigation packs. Exact links produce hypotheses with
//! original membership; they never reuse imported severity as evidence strength.
use crate::investigation::Population;
use serde_json::{json, Value};
pub const VERSION: &str = "packs-1";
pub fn catalog() -> Value {
    json!({"version":VERSION,"packs":["identity","process","web_api","network_dns","directory","cloud_saas","ci_cd","operations","business"],"policy":"Exact observed links, competing explanations and scenario prerequisites; no automatic compromise classification"})
}
fn pair(first: &str, second: &str, link: &str, window: i64, entity: &str) -> String {
    format!("WITH a AS(SELECT ns,{entity},ref,eid,ts,host,metadata FROM facts WHERE time_ok AND ({first})),b AS(SELECT ns,{entity},ref,eid,ts,host,actor,metadata FROM facts WHERE time_ok AND ({second})), linked AS(SELECT a.ns,a.{entity} AS entity,a.ref AS a_ref,a.eid AS a_eid,a.ts AS a_ts,b.ref AS b_ref,b.eid AS b_eid,b.ts AS b_ts FROM a JOIN b ON a.ns=b.ns AND b.ts>a.ts AND b.ts-a.ts<={window} AND ({link}) WHERE a.{entity}<>'') SELECT ns,entity,a_ref AS ref,a_eid AS eid,a_ts AS ts FROM linked UNION SELECT ns,entity,b_ref,b_eid,b_ts FROM linked")
}
pub fn run(p: &Population) -> Result<(), String> {
    p.support_signal("lolbin_context","SELECT ns,process AS entity,ref,eid,ts FROM facts WHERE time_ok AND action IN ('process_start','script_execution') AND json_extract_string(metadata,'$.flags.suspicious_command')='true' AND json_extract_string(metadata,'$.flags.documentation')<>'true' AND regexp_matches(lower(process),'(^|[\\\\/])(mshta|rundll32|regsvr32|certutil|powershell|pwsh|bash|sh|curl|wget)(\\.exe)?$')","@process","Ferramenta legítima observada com combinação específica de comando, inclusive transformação estática quando disponível. Administração autorizada e testes continuam possíveis; correlacionar processo, arquivo e conexão por IDs.",35)?;
    p.support_signal("web_request_chain",&pair("action='http_request' AND json_extract_string(metadata,'$.flags.suspicious_request')='true'","action IN ('http_response','http_request') AND outcome='success'","json_extract_string(a.metadata,'$.semantic.request')<>'' AND json_extract_string(a.metadata,'$.semantic.request')=json_extract_string(b.metadata,'$.semantic.request')",300000,"source"),"source","Requisição com conteúdo específico e resposta de sucesso ligada pelo mesmo request ID. Status de sucesso confirma resposta; execução ou exposição de dados ainda exige outra fonte.",40)?;
    p.support_signal("cloud_control_chain",&pair("action='credential_create' AND outcome='success'","outcome='success' AND action IN ('secret_access','privilege_grant','role_assume','resource_access','credential_use')","json_extract_string(a.metadata,'$.semantic.created_credential')<>'' AND json_extract_string(a.metadata,'$.semantic.created_credential')=json_extract_string(b.metadata,'$.semantic.credential')",86400000,"actor"),"@user","Credencial criada e usada em ação relevante, ligada pelo ID exato da credencial e tenant. Conferir rotação, provisionamento, escopo e autorização; o criador pode diferir do usuário posterior.",45)?;
    p.support_signal("directory_control_chain",&pair("action IN ('delegation_change','privilege_grant','group_member_add','directory_acl_change') AND outcome='success'","action='logon' AND outcome='success'","json_extract_string(a.metadata,'$.semantic.target')<>'' AND json_extract_string(a.metadata,'$.semantic.target')=b.actor AND a.host=b.host AND a.host<>''",3600000,"actor"),"@user","Mudança declarada no diretório seguida de autenticação do alvo no mesmo host e domínio. A identidade do alvo é explícita; revisão da ACL, delegação e autorização é necessária.",35)?;
    p.support_signal("pipeline_control_chain",&pair("action IN ('workflow_change','pipeline_change','secret_change') AND outcome='success'","action IN ('pipeline_run','artifact_publish','workflow_run') AND outcome='success'","json_extract_string(a.metadata,'$.semantic.repository')<>'' AND json_extract_string(a.metadata,'$.semantic.repository')=json_extract_string(b.metadata,'$.semantic.repository') AND json_extract_string(a.metadata,'$.semantic.pipeline')<>'' AND json_extract_string(a.metadata,'$.semantic.pipeline')=json_extract_string(b.metadata,'$.semantic.pipeline') AND json_extract_string(a.metadata,'$.semantic.revision')<>'' AND json_extract_string(a.metadata,'$.semantic.revision')=json_extract_string(b.metadata,'$.semantic.revision')",86400000,"actor"),"@user","Mudança em workflow ou secret seguida de run/publicação, ligada por repositório, pipeline e revisão exatos. Conferir aprovação, diferença do código e artefato; sequência autorizada pode ter a mesma forma.",35)?;
    p.support_signal("application_flow_change","WITH ordered AS (SELECT ns,source,ref,eid,ts,action,coalesce(nullif(session,''),nullif(json_extract_string(metadata,'$.semantic.request'),''),nullif(json_extract_string(metadata,'$.semantic.connection'),'')) AS flow,lag(action) OVER(PARTITION BY ns,source,coalesce(nullif(session,''),nullif(json_extract_string(metadata,'$.semantic.request'),''),nullif(json_extract_string(metadata,'$.semantic.connection'),'')) ORDER BY ts,ref) AS previous_action FROM facts WHERE time_ok AND action<>'' AND (session<>'' OR coalesce(json_extract_string(metadata,'$.semantic.request'),'')<>'' OR coalesce(json_extract_string(metadata,'$.semantic.connection'),'')<>'')), pairs AS (SELECT * FROM ordered WHERE flow<>'' AND previous_action<>''), history AS (SELECT p.ns,p.source,count(*) n FROM pairs p JOIN bounds b USING(ns) WHERE p.ts<b.cutoff GROUP BY p.ns,p.source HAVING count(*)>=20) SELECT p.ns,p.source AS entity,p.ref,p.eid,p.ts FROM pairs p JOIN history h ON p.ns=h.ns AND p.source=h.source JOIN bounds b ON b.ns=p.ns WHERE p.ts>=b.cutoff AND NOT EXISTS(SELECT 1 FROM pairs old WHERE old.ns=p.ns AND old.source=p.source AND old.ts<b.cutoff AND old.previous_action=p.previous_action AND old.action=p.action)","source","Transição de ações ausente no histórico suficiente da fonte, observada dentro de session/request/connection declarado. Verificar deploy, versão, requisições paralelas e cobertura antes de atribuir desvio operacional.",20)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Event, security_normalize::Normalized};
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_context_packs_confirm_exact_links_time_and_benign_neighbours() {
        for (kind, first, second, keys) in [
            (
                "web_request_chain",
                "http_request",
                "http_response",
                vec![("request", "request")],
            ),
            (
                "directory_control_chain",
                "privilege_grant",
                "logon",
                vec![("target", "actor")],
            ),
            (
                "pipeline_control_chain",
                "workflow_change",
                "artifact_publish",
                vec![
                    ("repository", "repository"),
                    ("pipeline", "pipeline"),
                    ("revision", "revision"),
                ],
            ),
        ] {
            for variant in [
                "positive",
                "wrong_id",
                "wrong_namespace",
                "reverse_time",
                "expired",
                "missing_link",
                "failed_result",
            ] {
                let mut p = Population::new(&Default::default()).unwrap();
                for id in 0..2 {
                    let mut e = Event::empty();
                    e.id = id;
                    e.event_ref = format!("{kind}:{variant}:{id}");
                    e.source = "fixture".into();
                    e.timestamp = Some(
                        1700000000000
                            + if id == 1 {
                                match variant {
                                    "reverse_time" => -1000,
                                    "expired" => 172800000,
                                    _ => 1000,
                                }
                            } else {
                                0
                            },
                    );
                    e.fields=json!({"_sec.namespace":if id==1&&variant=="wrong_namespace"{"b"}else{"a"},"user.name":if kind=="directory_control_chain"&&id==1{"linked-object"}else{"operator"},"host.name":"directory-server","_sec.suspicious_request":"true"}).as_object().unwrap().clone();
                    let mut n = Normalized::default();
                    n.time.epoch_ms = e.timestamp;
                    n.put(
                        "action",
                        "explicit.action",
                        if id == 0 { first } else { second }.into(),
                        "mapping",
                    );
                    n.put(
                        "outcome",
                        "explicit.outcome",
                        if id == 1 && variant == "failed_result" {
                            "failure"
                        } else {
                            "success"
                        }
                        .into(),
                        "mapping",
                    );
                    for (a, b) in &keys {
                        let key = if id == 0 { a } else { b };
                        if id == 1 && variant == "missing_link" {
                            continue;
                        }
                        n.put(
                            key,
                            key,
                            if id == 1 && variant == "wrong_id" {
                                "other-object"
                            } else {
                                "linked-object"
                            }
                            .into(),
                            "mapping",
                        );
                    }
                    if kind == "directory_control_chain" && id == 1 && variant == "wrong_id" {
                        e.fields.insert("user.name".into(), json!("other-object"));
                    }
                    if kind == "directory_control_chain" && id == 1 && variant == "missing_link" {
                        e.fields.remove("host.name");
                    }
                    p.observe(&e, &n).unwrap();
                }
                p.finish().unwrap();
                let signals: i64 =
                    p.db.query_row("SELECT count(*) FROM signals WHERE kind=?", [kind], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(
                    signals,
                    if variant == "positive" { 1 } else { 0 },
                    "{kind} {variant}"
                );
                if variant == "positive" {
                    let n:i64=p.db.query_row("SELECT count(*) FROM signal_members m JOIN signals s USING(id) WHERE kind=?",[kind],|r|r.get(0)).unwrap();
                    assert_eq!(n, 2);
                }
            }
        }
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_cloud_pack_keeps_both_actors_and_rejects_another_credential_or_tenant() {
        let evaluate = |tenant: &str, credential: &str| {
            let mut p = Population::new(&Default::default()).unwrap();
            for (id, ns, actor, action, key, value) in [
                (
                    0,
                    "a",
                    "admin",
                    "credential_create",
                    "created_credential",
                    "key-a",
                ),
                (
                    1,
                    tenant,
                    "service",
                    "secret_access",
                    "credential",
                    credential,
                ),
            ] {
                let mut e = Event::empty();
                e.id = id;
                e.timestamp = Some(1700000000000 + id as i64 * 1000);
                e.event_ref = format!("pack:{id}");
                e.source = "fixture".into();
                e.fields = json!({"_sec.namespace":ns,"user.name":actor})
                    .as_object()
                    .unwrap()
                    .clone();
                let mut n = Normalized::default();
                n.time.epoch_ms = e.timestamp;
                n.put("action", "event.action", action.into(), "mapping");
                n.put("outcome", "event.outcome", "success".into(), "mapping");
                n.put(key, key, value.into(), "mapping");
                p.observe(&e, &n).unwrap();
            }
            p.finish().unwrap();
            let signals: i64 =
                p.db.query_row(
                    "SELECT count(*) FROM signals WHERE kind='cloud_control_chain'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let members:i64=p.db.query_row("SELECT count(*) FROM signal_members m JOIN signals s USING(id) WHERE s.kind='cloud_control_chain'",[],|r|r.get(0)).unwrap();
            (signals, members)
        };
        assert_eq!(evaluate("a", "key-a"), (1, 2));
        assert_eq!(evaluate("a", "key-b"), (0, 0));
        assert_eq!(evaluate("b", "key-a"), (0, 0));
    }
}
