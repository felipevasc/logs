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

## Wazuh

O Wazuh grava alertas e eventos no **Wazuh indexer**, que é o que o painel exibe. O inventário de agentes fica na **API do servidor**. Por isso há duas modalidades, cada uma com seu endereço e sua credencial.

### Wazuh · indexer (alertas e eventos)

Endereço `https://servidor:9200`. Use usuário e senha de uma conta do indexer, como a do painel ou uma conta de leitura dedicada. A conta precisa ler os índices `wazuh-*`, com as permissões `read` e de scroll. A opção **Token (Bearer)** serve para indexer ou proxy configurado para aceitar tokens. O campo **Dados do Wazuh** preenche o índice e o campo de horário:

| Conjunto | Índice | Campo de horário | Versões |
| --- | --- | --- | --- |
| Alertas | `wazuh-alerts-*` | `timestamp` | 4.x |
| Todos os eventos (archives) | `wazuh-archives-*` | `timestamp` | 4.x, com arquivamento habilitado |
| Achados | `wazuh-findings-v5*` | `@timestamp` | 5.x |
| Eventos normalizados | `wazuh-events-v5*` | `@timestamp` | 5.x |
| Vulnerabilidades | `wazuh-states-vulnerabilities*` | `vulnerability.detected_at` | 4.8+ e 5.x |

Também é possível informar outro índice ou padrão. Período, Query DSL e máximo de registros funcionam como no Elasticsearch. A leitura usa a API de *scroll*, presente em todas as gerações: Elasticsearch 7+ (Wazuh 4.0–4.2 com Elastic Stack), OpenSearch 1.2–2.19 (Wazuh indexer 4.3–4.14) e OpenSearch 3.x (Wazuh 5.0). As APIs de PIT diferem entre Elasticsearch e OpenSearch e não existem antes do OpenSearch 2.4. Elasticsearch 6 ou anterior (Wazuh 3.x) é recusado.

O teste de acesso informa:

- a versão real do indexer: o Wazuh 4.x responde `7.10.2` na raiz por compatibilidade com o Filebeat, então a versão vem das informações do nó quando a conta tem permissão;
- quantos registros o índice escolhido tem e se o scroll funcionou;
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

### Wazuh · API do servidor (agentes)

Endereço `https://servidor:55000`, Wazuh 4.0 ou superior, incluindo 5.x. Usuário e senha geram um token JWT em `POST /security/user/authenticate`. As versões 4.0–4.3 só aceitam `GET` nesse endereço, e o aplicativo usa `GET` quando recebe 405. O token expira em 900 segundos por padrão e é renovado uma vez se expirar durante a importação. Também é possível colar um token, que não é renovado.

Os agentes vêm de `GET /agents`, em páginas de 500 (o limite da 4.0–4.2), ordenados por ID. Um agente repetido por deslocamento da paginação é gravado uma vez. Cada agente usa o último contato (`lastKeepAlive`) como horário. A conta precisa da permissão RBAC `agent:read`, e a API não filtra por período. Os logs do gerenciador ficaram de fora: `/manager/logs` troca `limit` por `log_lines` na 4.3, devolve só as últimas 2.000 linhas e não existe na 5.0.

### Certificados

Instalações padrão do Wazuh usam uma autoridade própria: o `root-ca.pem` gerado pelo `wazuh-certs-tool`, normalmente em `/etc/wazuh-indexer/certs/` ou no `wazuh-certificates.tar`. Informe-o em **Certificado CA**, em PEM ou DER. A validação continua ativa, inclusive do nome: o host da URL precisa constar no certificado, com o IP ou DNS usado ao gerá-lo. **Não validar o certificado do servidor** é uma escolha explícita para redes confiáveis, e o resultado do teste a sinaliza. As duas opções também valem para Elasticsearch e Kibana.

Senhas e tokens seguem as regras da biblioteca global. O modo de autenticação faz parte da identidade da credencial, então uma senha salva nunca é enviada como token.

## Validação

As verificações locais cobrem validação de caminhos, preservação binária do transporte, limites, isolamento e controles da interface. Não foi usado um servidor SSH/WinRM autenticado nesta implementação; configuração de domínio, certificados, políticas e permissões precisa ser conferida no ambiente de destino.

O conector Wazuh foi verificado com servidores HTTP locais que reproduzem:

- no indexer: raiz OpenSearch mascarada, versão do nó, scroll paginado e limitado, shards parciais com descarte do snapshot, fechamento do cursor, contas sem permissão de monitoramento e índices 5.x;
- na API do servidor: o recurso de `GET` após 405, a renovação do JWT, a deduplicação de agentes, o bloqueio de versões 3.x e as respostas com falha parcial. Os endpoints foram conferidos nas especificações OpenAPI das versões 4.0.0, 4.3.0, 4.14.8 e 5.0.0-rc1.

Os nomes e campos dos índices vêm dos modelos do Wazuh 4.14.8 e do `wazuh-indexer-plugins` 5.0.0-rc1. A 5.0 ainda era pré-lançamento em outubro de 2026. Não foi usado um Wazuh real.

Referências: [OpenSSH](https://man.openbsd.org/ssh), [verificação de host](https://man.openbsd.org/OpenBSD-current/man/ssh_config) e [cópia por PowerShell remoting](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/copy-item?view=powershell-7.6).
