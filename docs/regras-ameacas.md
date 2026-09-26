# Catálogo de sinais de ameaças

O catálogo distribuído contém **378 regras em 33 famílias**, disponíveis em [threat-rules.json](../src-tauri/resources/threat-rules.json). São heurísticas próprias para localizar evidências textuais em logs heterogêneos. As referências explicam as técnicas ou os campos de auditoria; **não são uma certificação das regex pela OWASP, MITRE ou pelos fornecedores**.

Uma correspondência é um sinal para investigar. Texto citado, testes de segurança, ferramentas administrativas, backups e ações autorizadas podem corresponder às mesmas regras. Ausência de correspondência não significa ausência de ameaça.

## Interpretação

| Tipo (`kind`) | Quantidade | Significado |
| --- | ---: | --- |
| `attempt` | 167 | Texto com estrutura de tentativa, comando ou payload; não comprova execução. |
| `indicator` | 76 | Artefato, ferramenta, configuração, exposição ou operação sensível que precisa de contexto. |
| `response` | 135 | Erro, bloqueio, saída de comando, conteúdo de arquivo ou resposta estruturada relevante. A regex reconhece o formato; direção e exposição precisam ser confirmadas. |

Há **149 regras high, 144 medium e 85 low**. Severidade representa a prioridade inicial do comportamento descrito, não probabilidade estatística de ataque nem confirmação de impacto. Eventos administrativos legítimos, especialmente em cloud e Kubernetes, são deliberadamente classificados como indicadores; confirme ator, autorização, recurso, resultado e sequência temporal. Uma proteção que bloqueou algo não equivale a invasão. Uma saída de diagnóstico legítimo também não comprova execução indevida.

## Cobertura

| Família | Regras | O que procura |
| --- | ---: | --- |
| XSS | 12 | HTML ativo, execução de script em atributos/URLs e respostas de bloqueio. |
| Injeção SQL | 18 | Mudança de consulta, atrasos, acesso a arquivos/processos e erros do banco. |
| Injeção de comandos | 10 | Separadores, substituições, download seguido de execução e SSI. |
| Traversal de caminhos | 10 | Subidas de diretório associadas a arquivos sensíveis e bloqueios. |
| LFI/RFI | 8 | Wrappers, inclusão de arquivos, arquivos compactados e conteúdo remoto. |
| SSRF | 12 | URLs para metadados, serviços locais, sockets e esquemas inesperados. |
| XXE | 7 | Entidades externas, inclusões, leitura local e rejeições do parser. |
| Injeção de templates | 10 | Acesso a runtime e funções sensíveis em mecanismos de template. |
| Desserialização | 10 | Formatos de objetos, classes sensíveis e filtros de rejeição. |
| Upload e webshell | 10 | Extensões executáveis, scripts que usam entrada HTTP e alteração de handlers. |
| Shell reverso | 8 | Combinações de socket, redirecionamento e interpretador. |
| C2 e túneis | 8 | Configurações específicas de túnel e classificações de sensores. |
| Exfiltração | 8 | Transferência de arquivos, compactação e destinos remotos. |
| Credenciais e autenticação | 10 | Segredos em logs, ferramentas de tentativas e respostas de autenticação. |
| Nuvem | 20 | Auditoria, credenciais, permissões e exposição em AWS, Azure e Google Cloud. |
| Kubernetes | 14 | Execução em pods, segredos, RBAC, privilégios, namespaces e auditoria. |
| Mineração | 5 | Ferramentas, protocolo, destinos de pools e saída de trabalho aceito. |
| PowerShell | 16 | Execução codificada, download/execução, memória e alterações de proteção. |
| Credenciais Windows | 12 | Acesso e extração de credenciais e artefatos relacionados. |
| Active Directory | 12 | Operações sobre tickets, replicação, reconhecimento e identidades. |
| Evasão e impacto | 12 | Remoção de rastros, alterações de proteção e recuperação. |
| Persistência Windows | 12 | Serviços, tarefas, inicialização e chaves sensíveis. |
| Shell e persistência Unix | 12 | Interpretadores, cron, chaves de acesso e artefatos sensíveis. |
| Movimento lateral e túneis | 8 | Ferramentas e mecanismos de acesso remoto com contexto. |
| Exposição web | 12 | Índices de diretório, Git, páginas de diagnóstico, debug e artefatos de aplicação. |
| Configurações e arquivos expostos | 11 | Conteúdo de env/configuração, autenticação de ferramentas, estado Terraform e Ansible Vault. |
| Erros de banco com contexto | 12 | Consultas, valores ou detalhes internos devolvidos por bancos e drivers específicos. |
| Dumps e resultados de dados | 10 | Cabeçalhos de dumps, exportações de contas, hashes, documentos e resultados de serviços de dados. |
| Respostas de metadados e identidade cloud | 10 | Credenciais, tokens e documentos de identidade retornados por AWS, Google Cloud e Azure. |
| Tokens e material secreto expostos | 12 | Formatos de JWT, OAuth, tokens de fornecedores, chaves e strings de conexão. |
| Tracebacks e respostas de execução | 6 | Falhas de subprocessos, funções bloqueadas, parsers e atributos internos em templates. |
| Saídas Unix | 21 | Conteúdo de passwd/group/shadow, identidade, sistema, processos, arquivos, rede e privilégios. |
| Saídas Windows | 20 | Identidade, grupos, privilégios, sistema, processos, serviços, registro, rede e inventário. |

A expansão inclui 114 regras de respostas e conteúdo, cada uma com exemplo positivo próprio. Por exemplo, `response.host.passwd` reconhece a estrutura de sete campos de uma linha como `root:x:0:0:root:/root:/bin/bash`, inclusive seguida de quebra real ou dos caracteres literais `\n`/`\r`. Mencionar apenas `/etc/passwd` não corresponde a essa regra. Conteúdo citado em documentação, diagnóstico autorizado ou captura de teste pode corresponder: o usuário precisa verificar em qual campo e direção apareceu. O mesmo cuidado vale para índices de diretório, dumps legítimos e respostas normais de emissão de tokens.

Os formatos de origem precisam registrar o conteúdo relevante. Um log apenas com horário e código HTTP não revela um corpo de requisição omitido; alguns padrões exigem argumentos, campos de auditoria ou mensagens do mecanismo de proteção. Não há decodificação de executáveis, base64 arbitrário, reconstrução de sessões de rede ou detecção comportamental automática neste arquivo. Correlações entre registros (força bruta, varreduras, sequências, comunicação periódica) ficam nas [regras de detecção](busca-e-deteccoes.md), que usam os sinais deste catálogo como parte da triagem.

O motor reúne mensagem, descrição, texto original e campos estruturados em um corpus de até 64 KiB por registro. Visita no máximo 1.000 nós e 12 níveis de campos aninhados. Mantém o original e acrescenta versões com percent-decode e escapes `\uXXXX`, em até duas passagens, dentro do mesmo orçamento. Não interpreta `+` como espaço de formulário nem combina pares substitutos UTF-16. Evidências que usam a versão transformada são identificadas como normalizadas. Truncamento marca o resultado como limitado; `clipped_records` conta candidatos examinados com texto parcial e pode exceder o total após aplicar uma regra específica, porque candidatos truncados não permitem excluir possíveis correspondências com certeza.

## Edição

O JSON usa `version: 1`, `name` e uma lista `rules`. Cada regra possui:

- `id`: identificador ASCII estável, usado para filtrar e relacionar evidências. Não reutilize o mesmo ID para significados diferentes.
- `name`, `category`, `description`: texto apresentado ao usuário, incluindo interpretação e alternativas benignas.
- `severity`: `high`, `medium` ou `low`.
- `kind`: `attempt`, `indicator` ou `response`.
- `pattern`: expressão regular compatível com `regex::bytes`, com Unicode desativado por padrão.
- `enabled`: ativa ou desativa a regra.
- `references`: URLs de fontes primárias que fundamentam o contexto técnico.
- `attack` (opcional): técnicas MITRE ATT&CK, como `["T1059.001"]`. Sem o campo, a técnica é inferida da categoria. Ela aparece na triagem, no detalhe do registro e no relatório do Caso.

As regex usam flags inline, como `(?i)` para ignorar caixa. Classes, limites de palavras e comparação sem diferenciar caixa operam em ASCII por padrão; use `(?u)` explicitamente quando precisar de classes ou caixa Unicode. Não há lookaround ou backreferences. A barra invertida precisa ser escapada no JSON: `\\b` representa um limite de palavra. Mantenha limites nos trechos variáveis, por exemplo `[^\r\n]{0,160}`, para não juntar conteúdos distantes sem contexto. Várias regras podem corresponder ao mesmo registro; isso não representa incidentes distintos.

Inclua uma nova regra quando houver um sinal específico com utilidade de investigação. Evite palavras isoladas como `admin`, `shell`, `error` ou `password`. Prefira contexto de comando, argumentos, caminho sensível, estrutura de payload ou resposta explícita. Para operações comuns de administração, use indicador e descreva o uso legítimo. Revise a configuração usada pelo aplicativo antes de substituir o catálogo editável; o JSON no repositório é a base distribuída.

O arquivo editável é criado na pasta de configuração local do aplicativo na primeira utilização, sem sobrescrever um catálogo existente; a API informa seu caminho. Em **Regras**, o botão **Adicionar N regras novas** aparece quando há IDs distribuídos ausentes na cópia local. A ação é explícita: acrescenta somente esses IDs, preserva nome do catálogo, regras customizadas, padrões alterados e regras desativadas. Antes de substituir o arquivo de forma atômica, valida o catálogo combinado e guarda o conteúdo original em um arquivo `threat-rules.backup-*.json` na mesma pasta. Não reaplica mudanças do fornecedor sobre IDs existentes. Uma nova execução sem IDs faltantes não reescreve o arquivo.

O motor aceita até 4 MiB de JSON, 1.000 regras e 8 KiB de expressão por regra. Regras inválidas, IDs duplicados e padrões que correspondem a texto vazio são rejeitados. Uma edição inválida preserva o arquivo e apresenta erro, em vez de substituir silenciosamente as regras do usuário. Na prévia do navegador, o caminho exibido é o arquivo base real do projeto; a simulação de atualização é identificada e fica somente na sessão do navegador.

## Validação

O catálogo foi validado como JSON, com IDs únicos e 378 padrões sem duplicação textual exata. A checagem estática com `rg 15 --no-unicode --multiline` aceitou os 378 padrões, com orçamento de 256 KiB por expressão e 32 MiB para o conjunto: 151 exemplos positivos (incluindo três normalizados) e 22 controles benignos passaram. Uma verificação preliminar em JavaScript também compilou os padrões, adaptando as flags inline. **Essas verificações não substituem executar o motor integrado**. `cargo check --tests` passou após a atualização aditiva, incluindo regressão de preservação de customizações, backup e arquivo inválido. Os testes foram compilados, mas o Windows impediu iniciar o executável de testes com erro 225 de proteção antivírus na tentativa anterior. Nenhum resultado de execução dessa rodada Rust está confirmado, e a proteção não foi desativada nem contornada.

Foram verificados 148 exemplos sintéticos positivos, três exemplos normalizados e 22 controles benignos, sem falhas nessa verificação preliminar. Os controles incluem autenticação válida de `admin`, mensagem benigna contendo `error` e `shell`, consulta SQL comum, upload de PDF, consulta de saúde, requisições sem conteúdo de resposta e segredos mascarados. O conjunto cobre tentativas, indicadores e saídas de ataques; nem toda regra do catálogo possui uma fixture individual. Os exemplos são strings inertes: nenhum comando ou payload foi executado. As [fixtures versionadas](../src-tauri/resources/threat-examples.json) são compartilhadas pelas verificações, e o [validador estático](../scripts/preview/validate-threat-catalog.mjs) pode ser executado com Node.js e ripgrep instalados.

Esses testes não medem precisão/recall em tráfego real. Os padrões têm janelas limitadas e podem perder conteúdo truncado, distribuído entre eventos, reordenado ou ofuscado. A normalização e o limite de texto examinável pertencem ao motor, não à regex isolada. Campos ausentes, aliases, idioma da mensagem e serialização do produtor também afetam a cobertura. Confirme os testes do motor após editar o arquivo.

## Referências técnicas

As referências acompanham cada regra. Para manutenção das famílias:

- Web: [OWASP XSS](https://community.owasp.org/attacks/xss/), [SQL injection](https://community.owasp.org/attacks/SQL_Injection), [command injection](https://community.owasp.org/attacks/Command_Injection), [path traversal](https://community.owasp.org/attacks/Path_Traversal), [SSRF](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html), [XXE](https://cheatsheetseries.owasp.org/cheatsheets/XML_External_Entity_Prevention_Cheat_Sheet.html), [templates](https://owasp.org/www-project-web-security-testing-guide/latest/4-Web_Application_Security_Testing/07-Injection_Testing/18-Testing_for_Server-side_Template_Injection), [desserialização](https://cheatsheetseries.owasp.org/cheatsheets/Deserialization_Cheat_Sheet.html) e [upload](https://cheatsheetseries.owasp.org/cheatsheets/File_Upload_Cheat_Sheet.html).
- Host e rede: [MITRE interpretadores](https://attack.mitre.org/techniques/T1059/), [webshell](https://attack.mitre.org/techniques/T1505/003/), [protocolos de C2](https://attack.mitre.org/techniques/T1071/), [proxy/túneis](https://attack.mitre.org/techniques/T1090/), [exfiltração](https://attack.mitre.org/techniques/T1048/), [credenciais](https://attack.mitre.org/techniques/T1552/), [força bruta](https://attack.mitre.org/techniques/T1110/) e [uso indevido de recursos](https://attack.mitre.org/techniques/T1496/).
- Auditoria de nuvem: [AWS CloudTrail](https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html), [Azure Activity Log](https://learn.microsoft.com/en-us/azure/azure-monitor/fundamentals/activity-log-schema), [Google Cloud Audit Logs](https://docs.cloud.google.com/logging/docs/audit), [segurança Kubernetes](https://kubernetes.io/docs/concepts/security/security-checklist/) e [schema de auditoria Kubernetes](https://kubernetes.io/docs/reference/config-api/apiserver-audit.v1/).
- Saídas e conteúdo exposto: [formato passwd](https://man7.org/linux/man-pages/man5/passwd.5.html), [formato shadow](https://man7.org/linux/man-pages/man5/shadow.5.html), [GNU id](https://www.gnu.org/software/coreutils/manual/html_node/id-invocation.html), [Microsoft whoami](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/whoami), [Microsoft systeminfo](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/systeminfo), [OWASP vazamento de informação](https://owasp.github.io/www-project-web-security-testing-guide/latest/4-Web_Application_Security_Testing/01-Information_Gathering/05-Review_Web_Page_Content_for_Information_Leakage) e [gestão de segredos](https://cheatsheetseries.owasp.org/cheatsheets/Secrets_Management_Cheat_Sheet.html).
- Formatos de serviços e dados: [mysqldump](https://dev.mysql.com/doc/refman/8.4/en/mysqldump.html), [SQLite CLI](https://www.sqlite.org/cli.html), [credenciais de metadados AWS](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instance-metadata-security-credentials.html), [metadados Google Cloud](https://docs.cloud.google.com/compute/docs/metadata/querying-metadata), [token de identidade gerenciada Azure](https://learn.microsoft.com/en-us/entra/identity/managed-identities-azure-resources/how-to-use-vm-token) e [kubeconfig](https://kubernetes.io/docs/concepts/configuration/organize-cluster-access-kubeconfig/).
