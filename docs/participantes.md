# Atacante e alvo

Os itens de **Comprometimentos** podem mostrar um minicard **Atacante** à esquerda e um minicard **Alvo** à direita, com a evidência no centro. Cada lado aparece somente quando há informações registradas que o identifiquem. Um tipo genérico de ativo, sozinho, não produz cartão. O espaço livre é aproveitado pela evidência. Em janelas estreitas, os minicards ficam acima do conteúdo.

Cada minicard mostra até três informações representativas. Clicar nele abre todos os valores, seus papéis, escopos e campos de origem. A procedência permite abrir o evento original, com campos e conteúdo bruto, dentro do modal. O mesmo endereço repetido é consolidado sem apagar suas referências. Valores provenientes de headers de proxy permanecem marcados como não validados.

## Interpretação

“Atacante” representa a possível origem ou identidade utilizada na atividade do achado. Pode ser um intermediário, uma conta comprometida ou infraestrutura remota. “Alvo” identifica contas, aplicações, recursos ou máquinas associados à operação; não confirma dano. Essas informações não alteram a força da evidência nem comprovam que vários endereços pertencem a uma só pessoa.

O catálogo versionado em `src-tauri/resources/participant-fields.json` contém aliases de IP, usuário, domínio, host, aplicação, conta e recurso. A extração percorre campos originais, objetos e arrays, aceitando wrappers de produtores diferentes. Valores conservam o caminho completo, inclusive o índice do array. Mapeamentos explícitos de origem, destino, ator, alvo, host, aplicação, serviço e recurso também conservam sua procedência.

Há suporte a campos ECS/Elastic, OCSF, Zeek, Windows, cloud e aplicações, além de padrões textuais Common/Combined de Apache/Nginx, erros web, SSH e registros chave/valor de audit/syslog/CEF. Isso é um catálogo extensível, não uma garantia de interpretação de qualquer formato sem contexto.

Distinções relevantes:

- IP de cliente e IP de servidor têm papéis diferentes; um endereço solto no texto não recebe um papel.
- A máquina que emite syslog não é automaticamente o atacante. Um observador, agente ou firewall permanece em contexto adicional.
- Em autenticações Windows, conta alvo e identidade que registrou a operação são separadas. Na autenticação SSH, a conta solicitada fica no alvo.
- Em conexão explicitamente de saída ligada a shell reverso, C2 ou exfiltração, os papéis de rede são interpretados nesse sentido e marcados como contextuais. Saídas genéricas e respostas HTTP sem direção de ataque demonstrada ficam em contexto.
- `X-Forwarded-For`, `Forwarded` e equivalentes não recebem confiança automática. São valores informados, sem configuração de proxies confiáveis nesta versão.
- IPs dentro de payloads, scripts e corpos de requisições/respostas não são promovidos a participantes. Trechos que parecem logs dentro de uma requisição não sobrepõem o cliente registrado na linha de acesso.
- Usuários, contas e recursos conservam namespaces. Um logger pode indicar um componente da aplicação, com esse papel explícito, sem afirmar a identidade de um servidor.

## Contrato e limites

Detecções e episódios transportam `participants`, com `version`, `facts` e `limited`. Cada fato tem `side`, `kind`, `value`, `role`, `certainty`, `namespace`, `origins` e `origins_limited`. O identificador interno `victim` corresponde ao rótulo **Alvo** na interface e nos relatórios. `certainty` caracteriza a extração/papel (`observed`, `contextual`, `unverified`); não é o nível E1–E5 do achado.

Cada origem preserva `event_ref`, `event_id`, `field` e `method`. O modal usa a consulta verificada de evidências, que confere pertencimento à análise. Segredos na apresentação do evento seguem a proteção existente; HTML do log é exibido como texto.

O resumo conserva até 64 fatos por lado e quatro origens de exemplo por fato. A extração limita profundidade a 12, arrays a 64 elementos, strings a 64 KiB e o percurso dos campos a 2.048 nós/256 KiB. Cortes ficam explícitos. Os registros completos continuam consultáveis pelas ocorrências. O resumo não é um inventário exaustivo nem realiza resolução DNS, reputação externa ou identificação de pessoas.

Os participantes são calculados no backend e acompanham o resultado em memória, SQLite, Caso e relatório de evidências. A agregação de um episódio examina todos os membros antes da paginação, inclusive componentes abaixo do nível mínimo exibido. A identidade da análise inclui a versão desta extração. Alterar o nível de exibição não recalcula personas.

O agrupamento visual considera os participantes e namespaces, além dos critérios anteriores. Identidades distintas permanecem separadas; uma extração limitada impede agrupar por semelhança como se a identidade estivesse completa. As origens dos membros são reunidas, sem elevar a classificação pela repetição.

## Ilustrações e validação

Quatro PNGs transparentes em `frontend/assets/personas/` representam atacante, servidor, estação de trabalho e aplicação. O tipo explícito de estação ou servidor e os campos de aplicação orientam a escolha. São símbolos genéricos. Os [prompts e a procedência da geração](../frontend/assets/personas/README.md) estão no repositório. O preparo do frontend copia esses arquivos para a distribuição.

Os testes nativos verificam aliases aninhados, IPv4/IPv6, múltiplos valores, procedência, proxy, Apache/Nginx/syslog, Windows, cloud, mapeamentos, direções de rede, conteúdo que não pode ser atribuído e limites. A integração cobre paridade memória/SQLite em grupos de 205 ocorrências e participantes de membros fora da prévia e do nível selecionado. O teste de navegador usa dados simulados para verificar minicards, lados ausentes, modal, evento exato, proteção de segredos, Caso, relatório, teclado e temas claro/escuro.

## Fontes de semântica dos campos

- [Apache mod_log_config](https://httpd.apache.org/docs/2.4/mod/mod_log_config.html): endereços de cliente/servidor e usuário autenticado.
- [Nginx Logging](https://docs.nginx.com/nginx/admin-guide/monitoring/logging/): formato de acesso e variáveis de log.
- [ECS Observer](https://www.elastic.co/docs/reference/ecs/ecs-observer) e [mapeamento de eventos de rede](https://github.com/elastic/ecs/blob/main/docs/reference/ecs-mapping-network-events.md): separação de participantes e observadores.
- [Windows 4624](https://learn.microsoft.com/windows/security/threat-protection/auditing/event-4624) e [Sysmon](https://learn.microsoft.com/en-us/sysinternals/downloads/sysmon): campos de identidade, host e conexão.
- [RFC 5424](https://www.rfc-editor.org/info/rfc5424/): host emissor do syslog.
- [CloudTrail userIdentity](https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-user-identity.html): identidade e conta da operação.
- [Linux audit_log_acct_message](https://man7.org/linux/man-pages/man3/audit_log_acct_message.3.html): conta e endereço em auditoria.
