# Casos, Achados, índices e conexões

Um **Caso** guarda a investigação inteira. **Dataset** é a fonte aberta para explorar; **Achados** é a seleção preservada de registros, correlações e outros itens. Os nomes internos legados do formato portátil continuam compatíveis.

## Isolamento

Criar um Caso abre um espaço vazio. Alternar Casos recupera a fonte e a área do destino, filtros, referências, interpretação, rascunhos, preferências e análises daquele Caso. Tema e zoom também são salvos por Caso. Uma operação admitida no Caso anterior não pode publicar respostas na tela do destino.

Limites de hardware, o serviço MCP e a biblioteca de acessos remotos são recursos do computador. A biblioteca só é consultada explicitamente; uma coleta mantém seus caminhos e rascunhos no Caso. Credenciais ficam fora dos arquivos portáteis de investigação.

## Índices opcionais

Na árvore de campos ou no nome de um campo do detalhe, use o botão direito e **Criar índice para este campo/token**. Um marcador suave indica o índice completo; o menu e a descrição ao passar o mouse confirmam seu estado.

O índice armazena valores da fonte original interpretada. Seu identificador inclui Caso, análise, interpretação e identidade/ordem dos arquivos. Mudanças nesses dados exigem um novo índice. Igualdade exata e igualdade sem distinção ASCII de maiúsculas usam candidatos seletivos do índice, preservando o predicado canônico e o recorte visível. Resultados frequentes usam o motor vetorizado, sem truncamento. Cancelamento ou falha nunca publica um índice parcial.

## SSH no Linux

Escolha **SSH (Linux)**, endereço `ssh://servidor:22`, usuário e chave privada local opcional. Sem chave específica, o cliente usa a configuração/agente OpenSSH do computador. O servidor precisa de Python 3. O host deve ter sua chave conhecida e validada no OpenSSH; a coleta não aceita automaticamente uma chave desconhecida ou alterada.

Selecione sugestões de logs de sistema, autenticação, nginx, Apache ou audit, ou informe caminhos absolutos, um por linha. Pastas são percorridas dentro dos limites da coleta. Arquivos inexistentes podem ser ignorados com aviso, desde que haja arquivos válidos. Links simbólicos não são seguidos. O teste de acesso verifica leitura; a importação copia bytes para uma fonte local e então abre a indexação.

## WinRM no Windows

Escolha **WinRM (Windows)** e um endpoint HTTPS, por exemplo `https://servidor:5986/wsman`. O cliente desta modalidade requer Windows com PowerShell e remoting configurado no servidor. Deixe o usuário vazio para usar a autenticação integrada ou informe usuário e senha. A validação de certificado permanece ativa.

As sugestões incluem os EVTX de System, Application e Security, a pasta de EVTX e IIS. Também é possível indicar múltiplos arquivos/pastas absolutos. Para EVTX ativos na pasta padrão do Windows, o coletor exporta o canal com `wevtutil` e copia o snapshot; os demais arquivos usam `Copy-Item -FromSession`. Links/reparse points são recusados.

O limite padrão é 512 MiB, ajustável entre 1 MiB e 4 GiB. Cada coleta admite até 64 caminhos, 2.048 arquivos e 8.192 subpastas. A coleta incompleta é descartada. Arquivos locais concluídos são publicados juntos e passam pela detecção automática de formato. Permissões de leitura/exportação devem existir no servidor.

## Filtro das importações

Em **Filtro**, nas conexões Elasticsearch, Kibana e Wazuh, escreva a condição como na barra de busca do painel Wazuh (DQL/KQL) ou do aplicativo. O aplicativo converte o texto em Query DSL e mostra, enquanto você digita, a consulta que será enviada. Um filtro que começa com `{` continua sendo lido como Query DSL em JSON.

| Escrita | Significado | Query DSL enviada |
| --- | --- | --- |
| `campo: valor` | todas as palavras no campo, em qualquer ordem | `multi_match` com `operator: and` |
| `campo: "frase exata"` ou `campo = valor` | a frase exata | `multi_match` do tipo `phrase` |
| `campo != valor` | qualquer valor diferente da frase | `must_not` |
| `campo: web*` | curinga, com `*` e `?` | `query_string` |
| `campo: *` | o campo existe | `exists` |
| `campo >= 10`, `>`, `<`, `<=` | comparação | `range` |
| `campo: [10 TO 20]` ou `campo: 10..20` | faixa; `{ }` exclui o limite e `*` deixa um lado aberto | `range` |
| `campo: 10.0.0.0/8` | bloco de endereços | termo para campos IP e prefixo `10.*` para campos de texto |
| `campo: (a or b, c)` | lista de valores | `should` |
| `valor`, sem campo | o texto nos campos padrão do índice | `multi_match` sem `fields` |

Os operadores são `and`, `or` e `not` em qualquer caixa; `E`, `OU` e `NÃO` em maiúsculas, como na busca do aplicativo; `&&` e `||`; e `-` ou `!` antes de uma condição. `not` vale antes de `and`, que vale antes de `or`, e parênteses agrupam. Condições lado a lado valem como `and`. Palavras seguidas sem operador formam um só valor: `rule.description: falha de login` procura as três palavras no campo. Use aspas ou `\` para escrever um caractere especial literalmente.

Os nomes são os campos do documento remoto, como `rule.level`, `agent.name` e `data.srcip`; os apelidos da busca do aplicativo, como `ip`, `usuario` e `nivel`, não se aplicam. Sem os mapeamentos do índice, as consultas de texto são tolerantes (`lenient`): um valor de tipo incompatível com o campo não encontra nada, em vez de falhar nos shards e descartar a importação. Comparações e faixas não têm essa tolerância e precisam de valores compatíveis com o campo. Expressões regulares (`/.../`) não são aceitas, porque Lucene, Rust e JavaScript usam sintaxes diferentes; use curingas.

Exemplos no Wazuh: `rule.level >= 10 and not agent.name: wazuh-manager`, `rule.groups: (authentication_failed or invalid_login) and data.srcip: 10.0.0.0/8` e `rule.id: 5710 or "Invalid user"`.

## Wazuh

O Wazuh grava alertas e eventos no **Wazuh indexer**, que é o que o painel exibe. O inventário de agentes fica na **API do servidor**. Há três modalidades, cada uma com seu endereço e sua credencial: o indexer direto, o mesmo indexer pelo painel web e a API do servidor.

### Wazuh · indexer (alertas e eventos)

Endereço `https://servidor:9200`. Use usuário e senha de uma conta do indexer, como a do painel ou uma conta de leitura dedicada. A conta precisa ler os índices `wazuh-*`, com as permissões `read` e de scroll. A opção **Token (Bearer)** serve para indexer ou proxy configurado para aceitar tokens. O campo **Dados do Wazuh** preenche o índice e o campo de horário:

| Conjunto | Índice | Campo de horário | Versões |
| --- | --- | --- | --- |
| Alertas | `wazuh-alerts-*` | `timestamp` | 4.x |
| Todos os eventos (archives) | `wazuh-archives-*` | `timestamp` | 4.x, com arquivamento habilitado |
| Achados | `wazuh-findings-v5*` | `@timestamp` | 5.x |
| Eventos normalizados | `wazuh-events-v5*` | `@timestamp` | 5.x |
| Vulnerabilidades | `wazuh-states-vulnerabilities*` | `vulnerability.detected_at` | 4.8+ e 5.x |

Também é possível informar outro índice ou padrão. Período, filtro e máximo de registros funcionam como no Elasticsearch. A leitura usa a API de *scroll*, presente em todas as gerações: Elasticsearch 7+ (Wazuh 4.0–4.2 com Elastic Stack), OpenSearch 1.2–2.19 (Wazuh indexer 4.3–4.14) e OpenSearch 3.x (Wazuh 5.0). As APIs de PIT diferem entre Elasticsearch e OpenSearch e não existem antes do OpenSearch 2.4. Elasticsearch 6 ou anterior (Wazuh 3.x) é recusado.

Cada shard lido mantém um contexto de scroll, e o indexer recusa uma leitura acima de `search.max_open_scroll_context` (500 por nó por padrão). Os índices diários do Wazuh passam desse número em poucos anos: um padrão como `wazuh-alerts-*` pode abranger milhares de shards. Antes de abrir o scroll, o aplicativo conta os shards que a consulta alcança, com uma busca sem resultados (`size: 0`, `pre_filter_shard_size=1`) que não abre contextos. Até 100 shards, a leitura é única. Acima disso, a importação lê janelas de tempo, das mais recentes às mais antigas, cada uma com até 100 shards; o indexer pula os shards fora da janela. A ordem final continua do mais recente ao mais antigo, e o limite de registros para na janela em que é atingido. Sem campo de horário não há janelas, e o aplicativo pede um campo de horário ou um índice mais específico. Registros sem o campo de horário ficam fora das janelas e são contados no aviso da importação.

Contas de leitura costumam não ter `indices:data/read/scroll/clear`. Sem essa permissão o scroll não é fechado e expira sozinho em cerca de 1 a 2 minutos; o aplicativo não trata isso como erro e espera esses contextos expirarem antes de abrir mais de 300 deles.

O teste de acesso informa:

- a versão real do indexer: o Wazuh 4.x responde `7.10.2` na raiz por compatibilidade com o Filebeat, então a versão vem das informações do nó quando a conta tem permissão;
- quantos registros o índice escolhido tem e se o scroll funcionou, na janela mais recente quando a leitura é feita por janelas de tempo;
- outros conjuntos Wazuh com dados e os que a conta não pode ler;
- a geração detectada pelos índices (Wazuh 4.x ou 5.x).

Uma URL na porta 55000 recebe a orientação de usar o indexer.

Os alertas 4.x são interpretados assim:

- `rule.level` vira o nível pelas faixas do painel: 0–3 Informação, 4–6 Aviso, 7–11 Erro e 12–15 Crítico;
- `rule.id` vira o código, `rule.description` a mensagem e `agent.name` a origem;
- o horário com fuso sem dois-pontos (`+0000`) é respeitado;
- `data.srcip`, `data.srcuser`, `data.dstuser`, `data.win.eventdata.*`, `syscheck.*` e `agent.name` alimentam `@src_ip`, `@user`, `@host`, `@file`, `@hash` e os demais campos canônicos;
- os grupos `authentication_failed` e `authentication_success` classificam logons;
- o `id` do alerta e o `full_log` original permanecem como campos.

Os dados 5.x seguem o esquema comum do Wazuh, baseado em ECS, e usam os campos canônicos existentes.

### Wazuh · painel web (alertas e eventos)

Use o endereço do painel, o mesmo aberto no navegador, como `https://wazuh.empresa`. Ao colar um link copiado do navegador, o trecho a partir de `/app/` é descartado. Os conjuntos, o período, o filtro e o teste são os mesmos do indexer. As consultas passam pelo console de Dev Tools do painel (`POST /api/console/proxy`, com o cabeçalho `osd-xsrf`), presente no OpenSearch Dashboards de todas as versões do Wazuh 4.x e 5.x.

Informe usuário e senha do painel. A conta precisa ler os índices `wazuh-*` e usar o Dev Tools. Login SSO (SAML ou OpenID) do navegador não é reutilizado; nesse caso use uma conta interna ou um token aceito pelo painel. Use esta opção quando só o painel é acessível; com acesso à porta 9200, a conexão direta ao indexer evita um salto.

### Wazuh · API do servidor (agentes)

Endereço `https://servidor:55000`, Wazuh 4.0 ou superior, incluindo 5.x. Usuário e senha geram um token JWT em `POST /security/user/authenticate`. As versões 4.0–4.3 só aceitam `GET` nesse endereço, e o aplicativo usa `GET` quando recebe 405. O token expira em 900 segundos por padrão e é renovado uma vez se expirar durante a importação. Também é possível colar um token, que não é renovado.

Os agentes vêm de `GET /agents`, em páginas de 500 (o limite da 4.0–4.2), ordenados por ID. Um agente repetido por deslocamento da paginação é gravado uma vez. Cada agente usa o último contato (`lastKeepAlive`) como horário. A conta precisa da permissão RBAC `agent:read`, e a API não filtra por período. Os logs do gerenciador ficaram de fora: `/manager/logs` troca `limit` por `log_lines` na 4.3, devolve só as últimas 2.000 linhas e não existe na 5.0.

### Certificados

Instalações padrão do Wazuh usam uma autoridade própria: o `root-ca.pem` gerado pelo `wazuh-certs-tool`, normalmente em `/etc/wazuh-indexer/certs/` ou no `wazuh-certificates.tar`. Informe-o em **Certificado CA**, em PEM ou DER. A validação continua ativa, inclusive do nome: o host da URL precisa constar no certificado, com o IP ou DNS usado ao gerá-lo. **Não validar o certificado do servidor** é uma escolha explícita para redes confiáveis, e o resultado do teste a sinaliza. As duas opções também valem para Elasticsearch e Kibana.

Senhas e tokens seguem as regras da biblioteca global. O modo de autenticação faz parte da identidade da credencial, então uma senha salva nunca é enviada como token.

## Validação

As verificações locais cobrem validação de caminhos, preservação binária do transporte, limites, isolamento e controles da interface. Não foi usado um servidor SSH/WinRM autenticado nesta implementação; configuração de domínio, certificados, políticas e permissões precisa ser conferida no ambiente de destino.

O conector Wazuh foi verificado com servidores HTTP locais que reproduzem:

- no indexer: raiz OpenSearch mascarada, versão do nó, scroll paginado e limitado, shards parciais com descarte do snapshot, fechamento do cursor, contas sem permissão de monitoramento ou de fechar scrolls, leitura por janelas de tempo com 1.971 shards e índices 5.x;
- na API do servidor: o recurso de `GET` após 405, a renovação do JWT, a deduplicação de agentes, o bloqueio de versões 3.x e as respostas com falha parcial. Os endpoints foram conferidos nas especificações OpenAPI das versões 4.0.0, 4.3.0, 4.14.8 e 5.0.0-rc1.

Os nomes e campos dos índices vêm dos modelos do Wazuh 4.14.8 e do `wazuh-indexer-plugins` 5.0.0-rc1. A 5.0 ainda era pré-lançamento em outubro de 2026. Em outubro de 2026, as requisições de pré-contagem e de scroll por janela foram conferidas no painel de um Wazuh 4.14.5 (OpenSearch 2.19.5) com 1.971 shards em `wazuh-alerts-*`, onde o scroll sem janela era recusado com 429 e a conta de leitura não podia fechar scrolls.

Referências: [OpenSSH](https://man.openbsd.org/ssh), [verificação de host](https://man.openbsd.org/OpenBSD-current/man/ssh_config) e [cópia por PowerShell remoting](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/copy-item?view=powershell-7.6).
