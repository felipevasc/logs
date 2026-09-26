# Busca, entidades e detecções

Este documento descreve a linguagem de busca, os campos canônicos que unificam fontes diferentes, a triagem de segurança do **Resumo** e o formato das regras de detecção e das regras Sigma importadas.

## Linguagem de busca

A caixa de busca aceita texto simples e expressões. Texto sem operadores continua funcionando como antes: procura a frase em qualquer campo. A expressão só é aplicada quando está completa; enquanto houver erro, o último filtro válido permanece ativo e a mensagem aparece ao lado da caixa. **Tab** completa campos e valores, e o ícone **?** mostra exemplos clicáveis.

| Expressão | Significado |
|---|---|
| `falha login` | frase em qualquer campo |
| `user:admin` | o campo contém o valor (sem diferenciar maiúsculas) |
| `user="Admin"` / `user!="Admin"` | igualdade exata / diferente |
| `status>=500`, `status<400` | comparação numérica (também datas em `timestamp`) |
| `status:500..599`, `latencia:[100 TO 900]` | intervalo |
| `ip:10.0.0.0/8` | endereço dentro da rede (IPv4 ou IPv6) |
| `host:web*`, `user:adm?n` | curingas |
| `user:(ana OR bruno)` | lista de valores |
| `path:/\.php$/` | expressão regular |
| `campo:*` | o campo existe e não está vazio |
| `-level:info`, `NOT level:info` | exclusão |
| `a AND b`, `a OR b`, parênteses | combinação (`E`, `OU` e `NÃO` também são aceitos) |
| `level:erro` | nível normalizado (erro, aviso, informação…) |
| `regra:sql.union` | registros que correspondem a uma regra do catálogo de ameaças |
| `deteccao:auth.bruteforce.source` | registros selecionados por uma regra de detecção |

A mesma linguagem é usada nos filtros salvos, nas regras de detecção, nas regras Sigma convertidas e pelo servidor MCP (`op: "query"`). Os filtros também aceitam `in`/`not_in` (um valor por linha) e `cidr`/`not_cidr`.

## Campos canônicos

Cada família de log dá um nome diferente ao mesmo conceito (`TargetUserName`, `user.name`, `usuario`, `suser`…). Os campos iniciados por `@` resolvem esses nomes e padrões de mensagem conhecidos (Windows Security e Sysmon, Linux auth e auditd, servidores web, firewalls, CloudTrail, Okta, Azure AD, Suricata, Zeek), de modo que uma única busca atravessa todas as fontes:

`@user`, `@src_ip`, `@dst_ip`, `@host`, `@process`, `@parent_process`, `@cmdline`, `@url`, `@domain`, `@hash`, `@dst_port`, `@user_agent`, `@file`, `@status`, `@action`, `@outcome`, `@src_scope`, `@dst_scope`, `@tool`.

Aliases curtos também funcionam: `user:`, `ip:`, `host:`, `processo:`, `porta:`… Um campo real com o mesmo nome sempre tem prioridade sobre o canônico.

- `@action` normaliza o que aconteceu: `logon`, `logoff`, `logon_explicit`, `privileged_logon`, `process_start`, `service_install`, `task_create`, `account_create`, `group_member_add`, `password_change`, `kerberos_service_ticket`, `registry_change`, `network_connection`, `http_request`, `log_clear`, `audit_policy_change`, `protection_disabled`, `malware_detected`, `ids_alert`, `privilege_use`, `secret_access`, entre outros.
- `@outcome` vale `success` ou `failure`.
- `@src_scope`/`@dst_scope` classificam o endereço: `privado`, `público`, `loopback`, `multicast` ou `reservado`.
- `@tool` identifica ferramentas de varredura e ataque pelo user agent ou pelo processo (sqlmap, Nikto, Nmap, Mimikatz, Rubeus…).

Os campos canônicos aparecem no detalhe do registro. Um clique (ou botão direito) em usuário, IP, host ou processo abre as ações: filtrar, ocultar, ver na linha do tempo, procurar em todas as fontes, seguir entre fontes e adicionar aos indicadores do Caso.

## Triagem

O card **Atenção** do Resumo roda as regras sobre o recorte atual (fontes e filtros) e mostra primeiro o que merece análise:

- **Episódios**: detecções que compartilham uma entidade ou registros, com até 1 h de intervalo, são reunidas em um episódio com a cadeia de táticas ATT&CK em ordem cronológica. Os três mais graves aparecem abertos; os demais ficam em **Mostrar mais**. Cada episódio abre exatamente os registros das suas detecções, pode ser levado à linha do tempo ou salvo no Caso com as técnicas ATT&CK.
- **Entidades em destaque**: usuários, IPs e hosts ordenados por risco (gravidade e quantidade de detecções, falhas e alcance).
- **Raridades**: valores pouco comuns de processos, user agents, ferramentas e destinos, para revisar o que destoa.
- As detecções também aparecem como marcas discretas sob os gráficos do Resumo e da Linha do tempo; uma marca seleciona o intervalo correspondente.

A triagem examina até 3 milhões de correspondências e apresenta até 500 detecções, cada uma com até 50 registros de amostra. O resultado fica em cache até o recorte ou as regras mudarem; **Recalcular triagem** na paleta de comandos (Ctrl+K) força uma nova leitura.

Detecções indicam padrões que merecem verificação, não conclusões. Ocultar uma detecção (ícone de olho) registra uma supressão por regra e entidade; a lista pode ser revista em **Configurações → Detecção**.

## Regras de detecção

As 47 regras distribuídas estão em [detection-rules.json](../src-tauri/resources/detection-rules.json). Para criar ou substituir regras, grave um arquivo `detection-rules.json` com o mesmo formato na pasta de configuração do aplicativo: uma regra com `id` existente substitui a distribuída; um `id` novo é acrescentado.

```json
{
  "version": 1,
  "rules": [
    {
      "id": "auth.spraying",
      "name": "Pulverização de senhas",
      "description": "A mesma origem falhou ao autenticar em várias contas.",
      "severity": "high",
      "attack": ["T1110.003"],
      "kind": "distinct",
      "where": "@action:logon @outcome:failure",
      "by": ["@src_ip"],
      "distinct": "@user",
      "window": "30m",
      "count": 8,
      "summary": "{@src_ip} tentou {distinct} contas diferentes"
    }
  ]
}
```

| `kind` | Dispara quando |
|---|---|
| `single` | um registro satisfaz `where` |
| `threshold` | `count` registros de `where` com o mesmo `by` dentro de `window` |
| `distinct` | `count` valores diferentes de `distinct` para o mesmo `by` dentro de `window` (`distinct_fallback` lista campos alternativos) |
| `sequence` | os `steps` ocorrem em ordem para o mesmo `by` dentro de `window`; cada passo tem `where` e `count` opcional |
| `beacon` | ao menos `count` conexões entre o mesmo `by` com intervalos regulares |

- `where` usa a linguagem de busca; `window` aceita `ms`, `s`, `m`, `h` e `d` (`90s`, `10m`, `2h`).
- `severity`: `critical`, `high`, `medium` ou `low`.
- `attack`: técnicas MITRE ATT&CK; a tática é derivada automaticamente.
- `summary` aceita `{count}`, `{distinct}`, `{period}` e qualquer campo, como `{@user}`.
- `enabled: false` distribui a regra desativada.

Regras são ativadas, desativadas e pesquisadas em **Configurações → Detecção**, que também controla se os sinais do [catálogo de ameaças](regras-ameacas.md) entram na triagem. As preferências ficam em `detections.json` na pasta de configuração.

## Regras Sigma

**Configurações → Detecção → Importar Sigma** copia arquivos `.yml`/`.yaml` para a pasta `sigma` da configuração; **Pasta Sigma** abre essa pasta. Cada regra é convertida para a linguagem de busca e passa a participar da triagem com as táticas das suas tags `attack.*`.

São aceitos: seleções com listas e mapas, os modificadores `contains`, `startswith`, `endswith`, `all`, `re` (com `i`, `m`, `s`), `cidr`, `gt`/`gte`/`lt`/`lte`, `exists`, `windash` e `cased`; condições com `and`, `or`, `not`, parênteses, `1 of`, `all of` e `them`; agregação `| count() by campo > N` com `timeframe`; e `logsource` (produto, categoria e serviço) como filtro das fontes. Regras com modificadores de codificação (`base64`, `base64offset`, `utf16*`, `wide`), `expand` ou `fieldref` são recusadas com a causa exibida, em vez de convertidas de forma imprecisa.

## Pacotes, formatos e cadeia de custódia

- Arquivos `.zip`, `.tar`, `.tar.gz`/`.tgz` são abertos como um conjunto: cada membro de log vira uma fonte (`pacote.zip!/caminho/membro.log`). Caminhos inseguros são ignorados; a extração respeita os limites de 64 GB e 10 mil membros.
- JSON com envelope (`Records`, `events`, `hits.hits`, `value`, `data`…) é lido como a lista de eventos interna. Formatos reconhecidos: CloudTrail, Suricata EVE, Zeek (JSON e TSV), Okta, GCP Audit, Kubernetes audit e auditd (com `proctitle` decodificado).
- Arquivos `.evtx` são lidos em Windows e Linux. A leitura de canais ao vivo do Windows Event Log continua exclusiva do Windows.
- **SHA-256**: na página de arquivos, o link **SHA-256** calcula o hash de cada fonte e, para pacotes, também o do arquivo original. Quando uma fonte passa a contribuir com um Caso, os hashes são calculados em segundo plano e guardados nos dados do Caso. Eles aparecem em **Dados do Caso**, no relatório PDF e no relatório Markdown.

## Caso

- **Indicadores**: valores adicionados pelo menu de entidades ou extraídos dos registros do Caso (IPs públicos, domínios, hashes e URLs). **Onde aparecem** conta as ocorrências de cada indicador nas fontes abertas.
- **Hipóteses**: arraste um item de evidência para uma hipótese para ligá-los; o estado (aberta, confirmada, descartada) muda com um clique.
- O relatório PDF e o Markdown incluem uma **Síntese da investigação** com hipóteses e evidências, técnicas ATT&CK observadas nos itens salvos a partir da triagem, indicadores e a integridade das fontes.

## MCP

O servidor MCP expõe 53 ferramentas. As novas são `triage` (detecções, episódios e entidades de risco do recorte), `event_insights` (ação normalizada, entidades, regras e conteúdo decodificado de um registro) e `detection_rules` (regras ativas, Sigma importado e erros de conversão).
