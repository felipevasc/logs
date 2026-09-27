"""Reviewed contextual content rules; no confidence derived from severity."""
import json
from pathlib import Path

rules = []
def add(key, name, level, rationale, claim="attempt", attack=None, condition=None):
    rules.append({"id": "content." + key, "name": name, "description": rationale,
        "kind": "single", "severity": "high" if level >= 4 else "medium", "by": [],
        "attack": attack or ["T1190", "T1005"],
        "where": condition or f"_sec.content.{key}:true",
        "evidence": {"level": level, "claim": claim, "rationale": rationale,
            "required": [], "maturity": "experimental", "version": "2",
            "missing": ["Autorização da atividade e contexto do solicitante; o nível não confirma comprometimento.",
                        "Precisão em corpus representativo ainda não avaliada."],
            "benign": ["Teste de segurança autorizado ou ambiente de laboratório com dados simulados."]},
        "references": ["https://owasp.org/www-community/attacks/Path_Traversal",
            "https://man7.org/linux/man-pages/man5/passwd.5.html", "https://man7.org/linux/man-pages/man5/shadow.5.html"]})

add("sensitive_request", "Busca de arquivo sensível em requisição", 3,
    "Caminho específico de credenciais, contas ou configuração em URI ou corpo de requisição; não demonstra leitura.")
add("traversal_request", "Traversal ou inclusão de arquivo direcionada a conteúdo sensível", 3,
    "Traversal, file:// ou php://filter junto a caminho sensível no mesmo campo da requisição, após decodificação limitada.")
add("read_request", "Comando de leitura de arquivo sensível na requisição", 4,
    "Sintaxe de comando de leitura com alvo sensível dentro de um campo de requisição; execução não observada.")
add("sensitive_read", "Leitura de arquivo sensível registrada em comando", 1,
    "Comando registrado lê arquivo de contas, credenciais ou ambiente. Pode ser administração legítima, especialmente /etc/passwd.",
    "activity", ["T1005", "T1552.001"], "_sec.action:process_start _sec.content.sensitive_read:true")
add("web_process_read", "Processo web inicia leitura de arquivo sensível", 3,
    "Evento de processo com comando de leitura sensível e pai web registrado; autorização e origem da requisição não demonstradas.",
    "activity", ["T1005", "T1552.001"])
add("ambiguous_request", "Busca de configuração possivelmente exposta", 1,
    "Requisição aponta a configuração de aplicação (.env, wp-config ou .git/config). Isoladamente pode ser uso da própria aplicação; não demonstra leitura.")
add("backup_request", "Busca de dump ou configuração de banco", 2,
    "Requisição procura nome específico de dump SQL ou configuração de banco; ausência de rota não demonstra exposição.")
add("path_traversal", "Tentativa de navegação fora do diretório publicado", 3,
    "Dois ou mais segmentos ../ ou ..\\ na mesma requisição, inclusive codificação dupla. O efeito depende da validação do servidor.", condition="_sec.content.path_traversal:true NOT _sec.content.traversal_request:true")
add("xxe_request", "Tentativa de XXE direcionada a arquivo ou metadados sensíveis", 4,
    "Declaração XML de entidade externa SYSTEM/PUBLIC aponta para credenciais locais ou serviço de metadados dentro de uma requisição. Não comprova expansão da entidade.")
add("xxe_payload", "Payload de XXE direcionado a arquivo ou metadados sensíveis", 3,
    "Mensagem contém declaração XML de entidade externa com destino sensível. A direção e o processamento ainda não estão demonstrados.", "activity")
add("xxe_blocked", "Resolução de entidade XML sensível bloqueada", 4,
    "Diagnóstico do parser registra tentativa de resolver entidade externa de arquivo sensível e recusa explícita. A tentativa é sustentada; a leitura foi bloqueada.")
add("xss_request", "Payload de XSS em requisição", 3,
    "Código de script, atributo de evento ou URI JavaScript com chamada executável em entrada HTTP; não demonstra execução no navegador.")
add("sqli_request", "Payload de injeção SQL em requisição", 3,
    "Sintaxe de UNION SELECT, tautologia com fechamento/comentário SQL ou injeção de atraso em entrada HTTP; erros e palavras SQL isoladas não satisfazem a regra.")
add("xss_payload", "Sintaxe de payload XSS em campo sem direção conhecida", 2,
    "Campo original contém sintaxe executável de XSS. Sem contexto de requisição, resposta ou navegador, descreve presença do payload e não sua utilização.", "activity")
add("sqli_payload", "Tautologia com quebra de contexto SQL em campo original", 2,
    "Aspas, operador booleano, comparação verdadeira e comentário SQL no mesmo campo; uma consulta SQL comum ou um erro de banco não bastam.", "activity")
add("traversal_payload", "Traversal direcionado a arquivo sensível em campo original", 3,
    "Vários segmentos de retorno de diretório conduzem a arquivo sensível no mesmo campo. A direção e o efeito ainda não são demonstrados.", "activity")
add("reverse_payload", "Payload completo de shell reverso em campo original", 4,
    "Sintaxe de shell com redirecionamento de entrada/saída e conexão de retorno, validada como comando e não como echo ou mera referência a /dev/tcp. Presença do payload não demonstra execução.", "activity", ["T1059", "T1071"])
add("sensitive_probe_set", "Busca combinada por diferentes famílias de arquivos sensíveis", 3,
    "Ao menos três famílias distintas de arquivos sensíveis procuradas em dez minutos no mesmo escopo de fonte, host e serviço. Origens registradas são separadas; sem origem registrada não se atribui a sequência ao mesmo cliente.")
rules[-1].update(kind="distinct", by=["_sec.content.probe_scope"], distinct="_sec.content.probe_family", count=3, window="10m", where="_sec.content.probe_family:*")
rules[-1]["evidence"]["required"]=["_sec.content.probe_scope", "_sec.content.probe_family"]
rules[-1]["evidence"]["missing"].append("Identidade/autorização do cliente; o agrupamento descreve buscas contra o escopo, não uma identidade de atacante.")
for key, label in [("passwd", "contas Unix no formato passwd"), ("shadow", "hashes no formato shadow"),
                   ("private_key", "bloco completo de chave privada"), ("secrets", "configuração com credenciais")]:
    add(key + "_response", "Resposta contém " + label, 4,
        "Estrutura de " + label + " registrada no corpo de resposta, sem bloqueio, conteúdo HTML ou reflexão direta do corpo enviado. A origem do conteúdo exige investigação.", "activity")
    add(key + "_disclosure", "Busca de arquivo com retorno de " + label, 5,
        "O mesmo evento registra requisição de arquivo sensível e corpo de resposta com estrutura correspondente de " + label + ". Sem bloqueio ou reflexão direta. Evidência muito específica de exposição, inclusive em teste autorizado.", "activity")
    add(key + "_linked_response", "Requisição ligada ao retorno de " + label, 5,
        "Eventos distintos da mesma requisição, host, serviço e namespace registram a busca sensível e o retorno de " + label + ", em até cinco minutos. A relação exige identificador de requisição e horários suficientes.", "activity")
    linked=rules[-1]
    linked.pop("where")
    linked.update(kind="sequence", by=["_sec.request", "_sec.host"], window="5m", steps=[
        {"where": f"_sec.content.{key}_request:true NOT _sec.outcome:(blocked OR failure)"},
        {"where": f"_sec.content.{key}_response:true"}])
    linked["evidence"]["required"]=["_sec.request", "_sec.host"]

target = Path(__file__).resolve().parents[2] / "src-tauri/resources/detection-content.json"
target.write_text(json.dumps({"version": 1, "rules": rules}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"{len(rules)} contextual content rules")
