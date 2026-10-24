//! Ready-made starting points for the extraction workbench. Each template is a complete
//! definition (the same declaration an advanced user would write) plus real example lines, so
//! it can be tried at once; none is applied until the user saves it.
use crate::enrichment_maps::{self, Definition};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Template {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub samples: Vec<String>,
    pub map: Definition,
}

fn build(id: &str, title: &str, description: &str, rules: Value, grok: Value) -> Definition {
    let mut value = json!({"id":id,"version":"1","title":title,"description":description,"schema":enrichment_maps::SCHEMA,"rules":rules});
    if !grok.is_null() {
        value["grokPatterns"] = grok;
    }
    serde_json::from_value(value).expect("template definition")
}
fn text(ty: &str, from: &[&str], description: &str) -> Value {
    json!({"from":from,"type":ty,"description":description})
}

pub(crate) fn all() -> Vec<Template> {
    let mut windows = enrichment_maps::built_in();
    windows.title = "Windows, Sysmon e auditd".into();
    windows.description = Some("Normaliza processo, pai, executável, usuário e host para os mesmos nomes em eventos do Windows (Sysmon 1, Security 4688) e do auditd.".into());
    vec![
        Template {
            id: "windows-sysmon-auditd",
            title: "Windows, Sysmon e auditd",
            description: "Nomes únicos para PID, PID do pai, executável, linha de comando, usuário e host. É o mapa padrão do aplicativo.",
            samples: vec![],
            map: windows,
        },
        Template {
            id: "ip-usuario-acao-duracao",
            title: "IP, usuário, ação e duração",
            description: "Linha de texto com IP, usuário, ação e duração em milissegundos, extraída com Grok.",
            samples: vec![
                "falha 10.0.0.5 usuario=ana acao=login duracao=12.5ms".into(),
                "ok 192.168.1.20 usuario=bruno acao=logout duracao=3ms".into(),
                "falha 10.0.0.9 usuario=carla acao=login duracao=250ms".into(),
            ],
            map: build(
                "ip-usuario-acao-duracao",
                "IP, usuário, ação e duração",
                "Extrai quem fez o quê, de onde e em quanto tempo.",
                json!([{
                    "id": "acesso",
                    "description": "Linha “resultado IP usuario=… acao=… duracao=…ms”.",
                    "extract": {"kind":"grok","patterns":["%{WORD:resultado} %{IP:ip_origem} usuario=%{WORD:usuario} acao=%{WORD:acao} duracao=%{NUMBER:duracao}ms"],"anchor":"search","source":"message"},
                    "fields": {
                        "resultado": text("text", &["resultado"], "Resultado da operação."),
                        "ip_origem": text("ip", &["ip_origem"], "Endereço de origem."),
                        "usuario": {"from":["usuario"],"type":"text","steps":[{"op":"lowercase"}],"description":"Usuário, sempre em minúsculas."},
                        "acao": text("text", &["acao"], "Ação executada."),
                        "duracao_ms": {"from":["duracao"],"type":"decimal","description":"Duração em milissegundos."}
                    }
                }]),
                Value::Null,
            ),
        },
        Template {
            id: "http-access-log",
            title: "Log de acesso HTTP (Apache e Nginx)",
            description: "Formato combined: cliente, data, método, caminho, status, bytes, referrer e agente.",
            samples: vec![
                "10.0.0.5 - ana [23/Oct/2026:17:05:09 +0000] \"GET /index.html HTTP/1.1\" 200 5120 \"-\" \"curl/8.4\"".into(),
                "192.168.1.20 - - [23/Oct/2026:17:05:11 -0300] \"POST /api/login HTTP/1.1\" 401 87 \"https://x.example/\" \"Mozilla/5.0\"".into(),
                "10.0.0.7 - - [23/Oct/2026:17:06:00 +0000] \"GET /missing HTTP/1.1\" 404 - \"-\" \"Wget/1.21\"".into(),
            ],
            map: build(
                "http-access-log",
                "Log de acesso HTTP",
                "Apache e Nginx no formato combined.",
                json!([{
                    "id": "combined",
                    "extract": {"kind":"grok","patterns":["%{COMBINEDAPACHELOG}"],"anchor":"full","source":"message"},
                    "fields": {
                        "cliente": text("ip", &["clientip"], "Endereço do cliente."),
                        "usuario": {"from":["auth"],"type":"text","onEmpty":"skip","description":"Usuário autenticado (“-” quando não há)."},
                        "quando": {"from":["timestamp"],"type":"datetime","datetime":{"format":"%d/%b/%Y:%H:%M:%S %z"},"description":"Instante da requisição, em UTC."},
                        "metodo": text("text", &["verb"], "Método HTTP."),
                        "caminho": text("text", &["request"], "Caminho solicitado."),
                        "status": text("integer", &["response"], "Código de resposta."),
                        "bytes": {"from":["bytes"],"type":"integer","description":"Bytes enviados (ausente quando o log traz “-”)."},
                        "agente": {"from":["agent"],"type":"text","steps":[{"op":"strip_prefix","value":"\""},{"op":"strip_suffix","value":"\""}]}
                    }
                }]),
                Value::Null,
            ),
        },
        Template {
            id: "chave-valor",
            title: "Pares chave=valor",
            description: "Linhas com chaves como user=, src= e action=, respeitando aspas.",
            samples: vec![
                "ts=2026-10-23T17:05:09Z user=ana src=10.0.0.5 action=login status=ok".into(),
                "ts=2026-10-23T17:06:12Z user=\"bruno silva\" src=192.168.1.20 action=logout status=ok".into(),
            ],
            map: build(
                "chave-valor",
                "Pares chave=valor",
                "Lê pares chave=valor sem expressão regular.",
                json!([{
                    "id": "pares",
                    "extract": {"kind":"key_value","separator":"=","quote":"\"","keys":{"ts":"ts","user":"user","src":"src","action":"action","status":"status"},"source":"message"},
                    "fields": {
                        "quando": {"from":["ts"],"type":"datetime","datetime":{"format":"iso8601"}},
                        "usuario": text("text", &["user"], "Usuário informado."),
                        "origem": text("ip", &["src"], "Endereço de origem."),
                        "acao": text("text", &["action"], ""),
                        "resultado": text("text", &["status"], "")
                    }
                }]),
                Value::Null,
            ),
        },
        Template {
            id: "colunas-delimitadas",
            title: "Colunas delimitadas (CSV)",
            description: "Linhas separadas por vírgula, com aspas, em colunas nomeadas.",
            samples: vec![
                "2026-10-23 17:05:09,ana,10.0.0.5,login,\"sem erro, ok\",12".into(),
                "2026-10-23 17:06:12,bruno,192.168.1.20,logout,,3".into(),
            ],
            map: build(
                "colunas-delimitadas",
                "Colunas delimitadas",
                "Linhas CSV sem cabeçalho.",
                json!([{
                    "id": "csv",
                    "extract": {"kind":"delimited","delimiter":",","quote":"\"","columns":["quando","usuario","origem","acao","observacao","duracao"],"source":"message"},
                    "fields": {
                        "quando": {"from":["quando"],"type":"datetime","datetime":{"format":"%Y-%m-%d %H:%M:%S","timezone":"utc"}},
                        "usuario": text("text", &["usuario"], ""),
                        "origem": text("ip", &["origem"], ""),
                        "acao": text("text", &["acao"], ""),
                        "observacao": {"from":["observacao"],"type":"text","onEmpty":"skip"},
                        "duracao": text("integer", &["duracao"], "")
                    }
                }]),
                Value::Null,
            ),
        },
        Template {
            id: "json-nomes",
            title: "Nomes de propriedades JSON",
            description: "Linhas JSON (mesmo após um prefixo): leva propriedades aninhadas para nomes planos e tipados.",
            samples: vec![
                "{\"time\":\"2026-10-23T17:05:09Z\",\"user\":{\"name\":\"Ana\"},\"net\":{\"src\":\"10.0.0.5\",\"port\":443},\"ok\":true}".into(),
                "app: {\"time\":\"2026-10-23T17:06:12Z\",\"user\":{\"name\":\"Bruno\"},\"net\":{\"src\":\"192.168.1.20\",\"port\":8080},\"ok\":false}".into(),
            ],
            map: build(
                "json-nomes",
                "Nomes de propriedades JSON",
                "Propriedades aninhadas viram campos planos.",
                json!([{
                    "id": "json",
                    "extract": {"kind":"json","paths":{"time":"time","usuario":"user.name","origem":"net.src","porta":"net.port","ok":"ok"},"source":"message"},
                    "fields": {
                        "quando": {"from":["time"],"type":"datetime","datetime":{"format":"iso8601"}},
                        "usuario": {"from":["usuario"],"type":"text","steps":[{"op":"lowercase"}]},
                        "origem": text("ip", &["origem"], ""),
                        "porta": text("integer", &["porta"], ""),
                        "sucesso": text("boolean", &["ok"], "")
                    }
                }]),
                Value::Null,
            ),
        },
        Template {
            id: "xml-windows",
            title: "Eventos XML do Windows",
            description: "Lê o XML original do evento (Sysmon, Security) e normaliza processo, pai e executável.",
            samples: vec![
                "<Event><System><Provider Name='Microsoft-Windows-Sysmon'/><EventID>1</EventID></System><EventData><Data Name='ProcessId'>4242</Data><Data Name='ParentProcessId'>0x3e8</Data><Data Name='Image'>C:\\Windows\\System32\\cmd.exe</Data></EventData></Event>".into(),
            ],
            map: build(
                "xml-windows",
                "Eventos XML do Windows",
                "Extrai dados de EventData com um analisador XML, sem regex.",
                json!([{
                    "id": "xml",
                    "extract": {"kind":"xml","paths":{"pid":"EventData/Data[@Name='ProcessId']","ppid":"EventData/Data[@Name='ParentProcessId']","exe":"EventData/Data[@Name='Image']","provider":"System/Provider@Name"},"source":"raw"},
                    "fields": {
                        "processo.pid": {"from":["pid"],"type":"integer","steps":[{"op":"hex_to_int"}],"description":"PID (aceita decimal ou 0x…)."},
                        "processo.pid_pai": {"from":["ppid"],"type":"integer","steps":[{"op":"hex_to_int"}]},
                        "processo.executavel": {"from":["exe"],"type":"text","steps":[{"op":"basename"},{"op":"lowercase"}],"description":"Nome do executável, sem a pasta."},
                        "processo.caminho": text("text", &["exe"], "Caminho completo, como no original."),
                        "provedor": text("text", &["provider"], "")
                    }
                }]),
                Value::Null,
            ),
        },
        Template {
            id: "auditd-grok",
            title: "auditd (linha SYSCALL)",
            description: "Cabeçalho do auditd (tipo, instante em segundos e sequência) mais o restante como pares.",
            samples: vec![
                "type=SYSCALL msg=audit(1729702509.123:4521): arch=c000003e syscall=59 success=yes pid=4242 ppid=1 uid=1000 exe=\"/usr/bin/curl\"".into(),
                "type=EXECVE msg=audit(1729702510.456:4522): argc=2 a0=\"curl\" a1=\"http://x.example\"".into(),
            ],
            map: build(
                "auditd-grok",
                "auditd",
                "Cabeçalho do auditd com Grok.",
                json!([{
                    "id": "cabecalho",
                    "extract": {"kind":"grok","patterns":["type=%{WORD:tipo} msg=audit\\(%{NUMBER:epoch}:%{INT:sequencia}\\): %{GREEDYDATA:resto}"],"anchor":"search","source":"message"},
                    "fields": {
                        "auditd.tipo": text("text", &["tipo"], "Tipo do registro."),
                        "auditd.quando": {"from":["epoch"],"type":"datetime","datetime":{"format":"epoch_s"},"description":"Instante do registro (segundos desde 1970), em UTC."},
                        "auditd.sequencia": text("integer", &["sequencia"], "Número de sequência.")
                    }
                }]),
                Value::Null,
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Event;

    #[test]
    fn every_template_validates_compiles_and_recognizes_its_own_examples() {
        let all = all();
        assert!(all.len() >= 8);
        for t in &all {
            enrichment_maps::validate(&t.map).unwrap_or_else(|e| panic!("{}: {e}", t.id));
            let compiled = enrichment_maps::Compiled::new(t.map.clone()).unwrap();
            for sample in &t.samples {
                let mut event = Event::empty();
                event.message = sample.clone();
                event.raw = sample.clone();
                let mut trace = enrichment_maps::Trace { detail: true, ..Default::default() };
                let out = compiled.evaluate(&event, Some(&mut trace)).unwrap_or_else(|e| panic!("{}: {e}", t.id));
                assert!(out.is_some(), "{} does not recognize {sample}: {:?}", t.id, trace.rules);
            }
        }
    }

    #[test]
    fn the_spec_example_and_the_access_log_produce_typed_values() {
        let all = all();
        let find = |id: &str| all.iter().find(|t| t.id == id).unwrap();
        let ip = enrichment_maps::Compiled::new(find("ip-usuario-acao-duracao").map.clone()).unwrap();
        let mut event = Event::empty();
        event.message = "falha 10.0.0.5 usuario=Ana acao=login duracao=12.5ms".into();
        let out = ip.evaluate(&event, None).unwrap().unwrap();
        assert_eq!(out["ip_origem"], "10.0.0.5");
        assert_eq!(out["usuario"], "ana");
        assert_eq!(out["duracao_ms"], 12.5);
        let http = enrichment_maps::Compiled::new(find("http-access-log").map.clone()).unwrap();
        event.message = find("http-access-log").samples[1].clone();
        let out = http.evaluate(&event, None).unwrap().unwrap();
        assert_eq!(out["status"], 401);
        assert_eq!(out["quando"], "2026-10-23T20:05:11.000Z");
        assert_eq!(out["metodo"], "POST");
    }
}
