# Comprometimentos

A navegação de **Análise** e **Caso** contém a página **Comprometimentos**. Ela usa todos os registros carregados na Análise ou todos os registros preservados no Caso ativo, inclusive de outras estações. Filtros de coluna, busca, período e estação do Explorar não selecionam os achados desta página. Os demais painéis continuam com seus recortes habituais.

## Cálculo sob demanda

Importar logs, abrir ou trocar de Caso e navegar pelo Resumo, Comprometimentos ou Timeline **não inicia o cálculo**. Use **Calcular comprometimentos** no Resumo ou na página, ou a ação **Recalcular triagem** da paleta de comandos. Enquanto não houver cálculo, a interface informa **ainda não calculados**, sem confundir isso com uma análise concluída sem indícios.

O cálculo tem estado de execução e cancelamento; cliques repetidos e voltar à página compartilham a mesma operação. Cancelar interrompe cooperativamente o trabalho nativo e nunca aplica uma resposta parcial ou atrasada. Trocar de Caso ou alterar dados/regras impede que a resposta antiga entre no contexto novo. Falhas têm uma ação explícita para tentar novamente.

Durante o cálculo, a página e a janela **Em andamento** mostram a leitura dos registros, a correlação, a regra em conferência e a organização final. A leitura compartilhada conserva uma única passagem pelos registros; depois, os indícios e suas evidências são conferidos por regra. O painel conta verificações concluídas e restantes, além dos indícios aceitos até aquele momento. A lista permite filtrar todas as verificações, verificadas, restantes ou com indícios; cada item abre sua cobertura, registros elegíveis e contagens por nível. Regras desabilitadas não entram no total; sinais do catálogo são apresentados por categoria.

Essas contagens são uma **prévia provisória**, sem publicar eventos ou uma análise incompleta. Uma regra sem indícios pode ter cobertura parcial, campos ausentes ou não ser aplicável; concluir a verificação não comprova ausência de comprometimento. Os eventos completos ficam disponíveis ao concluir o cálculo. Navegar e reabrir a janela recupera a mesma prévia; cancelar ou trocar o contexto impede atualizações atrasadas. A janela **Em andamento** exibe as animações existentes para cada tarefa, respeita movimento reduzido e encerra suas animações ao fechar ou concluir a tarefa.

Resultados completos são salvos automaticamente, tanto para fontes quanto para registros preservados no Caso. Ao voltar à página, trocar de Caso ou reabrir a aplicação, a interface busca o resultado salvo com uma consulta **somente ao cache**, sem recalcular. A ausência de resultado mantém o botão de cálculo explícito. Dados, regras, referências, interpretação de horários e visibilidade fazem parte da identidade; mudanças reais exigem nova análise, mas navegação, contadores da sessão e atualizações compatíveis da aplicação não a invalidam. Análises de outros Casos não são apagadas ao concluir novos cálculos. Paginação e detalhes reabrem o armazenamento quando necessário, sem depender de um resultado ainda presente na memória. Um resultado nunca aparece em outro Caso.

Depois do cálculo, o Resumo exibe um gráfico dos indícios de comprometimento por nível. Cada barra conta **achados naquele nível**, não eventos nem episódios. O botão abre a página; clicar em uma barra escolhe aquele nível mínimo. A página abre inicialmente em **Quase confirmado**. O controle é cumulativo e preserva sua seleção no mesmo universo. A área de cada Caso conserva seu nível mínimo e sua seleção de tática ao alternar contextos e reabrir a aplicação.

| Valor do contrato | Nome na interface e nos relatórios |
|---|---|
| 5 | Quase confirmado |
| 4 | Forte indício |
| 3 | Indício |
| 2 | Suspeita |
| 1 | Inconclusivo |

O valor numérico do contrato permanece compatível. Força da evidência, impacto potencial, hipótese (tentativa, execução, efeito, atividade) e resultado permanecem separados. **Quase confirmado não equivale à confirmação do analista**: uma tentativa inequívoca, inclusive bloqueada, pode atingir esse nível.

## Da evidência ao evento

Quando há identificação nos logs, o item mostra minicards de **Atacante** e **Alvo** nas extremidades, com a evidência no centro. Cada minicard abre os valores completos, seus papéis e os eventos de origem. Lados sem informações ficam ocultos. Veja [extração e procedência dos participantes](participantes.md).

O episódio apresenta o trecho principal que motivou o alerta. Ao abrir o episódio e usar **Ver eventos**, o recorte principal passa a representar a detecção selecionada e aparecem seus membros exatos, sem duplicar o trecho. Cada evento abre dentro da página, com metadados, todos os campos (inclusive objetos) e conteúdo bruto. Listas grandes carregam os membros progressivamente. Botão direito no evento oferece **Filtrar este evento no Explorar**; no episódio ou indício permite filtrar todos os seus membros.

O backend fornece `excerpts`: referência de evento, campo, transformação, offsets em bytes, contexto anterior/posterior, correspondência e indicadores de corte. Os offsets referem-se ao valor após a transformação indicada. Não se inventam trechos para negações, ausência de eventos ou junções. Quando um recorte não está disponível, a interface informa isso e apresenta os eventos e as condições. Os recortes são limitados a oito por achado, 80 caracteres de contexto de cada lado e 400 caracteres de correspondência. Isso não limita o acesso ao evento original.

HTML e comandos dos logs são tratados como texto. Senhas, hashes de `shadow` e blocos de chaves privadas são protegidos na apresentação; o evento original não é modificado. O endpoint `triage_evidence_event` verifica análise, pertencimento e referência antes de retornar o evento. Salvar no Caso usa a mesma verificação e preserva achado, recortes, relações e limitações.

## Trinta regras contextuais

O pacote `src-tauri/resources/detection-content.json` é gerado por `scripts/security/content-catalog.py` e usa os seletores derivados em `security_content.rs`.

| Comportamento | Nível inicial |
|---|---|
| Arquivo de contas, chave privada, histórico de shell ou credenciais em requisição | Indício |
| Configuração ambígua (.env, wp-config, config.dev.yml) isolada | Inconclusivo |
| Dump SQL ou configuração de banco em requisição | Suspeita |
| Navegação com vários segmentos ../ fora do diretório | Indício |
| Declaração XXE em mensagem / em requisição | Indício / Forte indício |
| Resolução XML de arquivo sensível explicitamente bloqueada | Forte indício, tentativa bloqueada |
| Payload de XSS ou SQLi em requisição (2 regras) | Indício |
| XSS ou tautologia com quebra de contexto SQL em campo sem direção (2 regras) | Suspeita |
| Traversal direcionado a arquivo sensível em campo sem direção | Indício |
| Shell reverso com sintaxe completa em campo sem direção | Forte indício, presença de payload |
| Três famílias diferentes de arquivos procuradas em dez minutos | Indício |
| Traversal ou inclusão direcionada ao caminho sensível | Indício |
| Comando de leitura de alvo sensível em requisição | Forte indício |
| Comando de processo lendo alvo sensível | Inconclusivo |
| Processo com pai web registrado e comando de leitura sensível | Indício |
| Resposta com estrutura de passwd, shadow, chave privada ou configuração com credenciais (4 regras) | Forte indício |
| Requisição sensível e corpo correspondente no mesmo evento (4 regras) | Quase confirmado |
| Requisição e resposta em eventos distintos, vinculadas explicitamente (4 regras) | Quase confirmado |

As correlações entre eventos exigem request ID, host, namespace/conta e serviço compatíveis, janela de cinco minutos e ordem temporal demonstrável. Um host compartilhado não basta. A falta de fuso/resolução limita a força da correlação. O campo ECS `cloud.account.id` participa do namespace.

Entre os alvos estão `/etc/passwd`, `/etc/shadow` (e a tentativa com o nome plural), `/etc/gshadow`, `/proc/.../environ`, `.ssh/id_rsa`, `.ssh/id_ed25519`, `.ssh/id_ecdsa`, `.aws/credentials`, `.env`, `wp-config.php` e hives Windows SAM/SECURITY/SYSTEM. A tentativa de acessar um nome não demonstra existência nem exposição do arquivo. A leitura local de `passwd` pode ser administração comum e permanece Inconclusivo.

O reconhecimento de respostas verifica estruturas, não apenas nomes: várias linhas de contas Unix, registros de shadow com formato de hash e campos de envelhecimento, delimitadores correspondentes e corpo de chave privada, ou pares coerentes de configuração com valores. Respostas negadas, marcadas como bloqueadas, reflexão direta do corpo da requisição, documentação explícita, trechos HTML/code e placeholders não sustentam exposição. Esses controles não provam a intenção do usuário nem substituem revisão de autorização.

Fontes customizadas podem mapear `url`, `command`, `request_command`, `request_body` e `response_body`, além de ação/resultado/identidades. Mensagens HTTP com URI entre colchetes e linhas de acesso HTTP têm a URI extraída com procedência, sem exigir adaptação a um produto específico. Diagnósticos explícitos do parser XML preservam o bloqueio. Payloads XXE em texto sem direção permanecem Indício; o DOCTYPE e DTDs comuns não bastam. Padrões textuais revisados de XSS/SQLi/traversal passam a Suspeita quando falta direção e a Indício quando há requisição demonstrada. Regras contextuais substituem os sinais textuais equivalentes para evitar duplicação. Não se atribui exposição forte a uma palavra isolada ou a `cat arquivo` genérico.

## Limites e validação

A normalização é versionada como `normalization-4`. A inspeção contextual de conteúdo usa até 64 KiB por campo. A varredura genérica percorre todos os campos originais candidatos, inclusive objetos, arrays, mensagem e conteúdo bruto, com orçamento de 256 KiB, 128 segmentos, 1.024 nós e 12 níveis por evento. Cada segmento conserva sua origem e nunca é concatenado a outro. Percent-encoding é decodificado em até duas passagens; a transformação fica registrada. Um corte fica explícito na cobertura e limita os achados contextuais a Indício. Não se executa conteúdo analisado. Alterar o nível não recalcula os logs; a paginação consulta o resultado já armazenado em SQLite. O cache de Comprometimentos independe dos filtros do Explorar.

Há testes nativos de positivos, negativos próximos, bloqueio, documentação, reflexão, texto Unicode, limites, mapeamento customizado, comandos legítimos, request IDs/hosts/contas/serviços diferentes, cronologia e recuperação exata do evento. Os testes de navegador verificam gráfico, nomes, níveis cumulativos, cache, Caso completo, abertura inline, destaque único, modal de verificações, foco/fechamento por teclado, conteúdo escapado, proteção de segredos e loading acessível. Os testes nativos também cobrem campos desconhecidos e aninhados, arrays, texto bruto, consultas SQL legítimas, limites de tamanho/quantidade de campos e padrões divididos entre campos (que não podem gerar uma correspondência artificial).

As novas regras permanecem **experimentais**. Os testes sintéticos não demonstram precisão de 99% nem substituem um corpus representativo rotulado. Os benchmarks de milhões de eventos não foram refeitos para esta entrega.

Referências: [passwd(5), Linux man-pages](https://man7.org/linux/man-pages/man5/passwd.5.html), [shadow(5), shadow-utils](https://man7.org/linux/man-pages/man5/shadow.5.html), [Path Traversal, OWASP](https://owasp.org/www-community/attacks/Path_Traversal).

## Verificações e agrupamento

A política `evidence-3` fornece `checks` com status, critério, valor observado, campos e uma amostra limitada das referências exatas. O botão de informação abre esses fatos num modal, com critérios técnicos recolhidos. Bloqueio/falha é resultado da operação, não prova de execução. Campos ausentes, ambiguidade de tempo e requisitos de validação não são apresentados como testes reprovados. Alternativas e limitações ficam em seções recolhidas. Achados históricos sem `checks` são identificados; não se inventa uma avaliação retroativa.

A correlação de buscas conta famílias distintas (ambiente, chaves, históricos, dumps, credenciais, contas e configuração), separa fonte/arquivo, namespace, host, serviço e cliente quando registrado, e exige horário. Repetições de um caminho ou variantes de `.env` não aumentam a diversidade. Sem cliente registrado, descreve o conjunto de buscas contra o escopo observado, sem atribuir todas a um mesmo autor. Seus componentes mantêm a classificação original.

Carregamentos usam um pequeno indicador circular, preservando o texto do controle, com `aria-busy` e alternativa estática para preferência de movimento reduzido.

Referências adicionais: [XXE, OWASP](https://cheatsheetseries.owasp.org/cheatsheets/XML_External_Entity_Prevention_Cheat_Sheet.html), [SQL Injection, OWASP](https://owasp.org/www-community/attacks/SQL_Injection).

## Ocorrências semelhantes

Ocorrências individuais com o mesmo padrão ficam reunidas em um cartão, com quantidade, período completo e um trecho representativo. O botão **Ver ocorrências** abre uma lista compacta; **Ver evento** mostra o registro original, seus campos e a verificação daquela ocorrência. Grupos extensos têm paginação interna de 20 indícios. A seleção de uma ocorrência atualiza o trecho destacado e o botão de explicação do cartão.

O agrupamento usa a regra e suas versões, o conteúdo que casou, campo/transformação, nível, resultado, tipo de atividade, origem/arquivo e identidades/contexto registrados. Horários, threads e texto de log ao redor do trecho não multiplicam cartões. Payloads, níveis, resultados e contextos diferentes permanecem separados. Trechos truncados, achados não avaliados e correlações de vários eventos não são agrupados por semelhança. Não há comparação fuzzy que apague diferenças no payload.

Trata-se de organização visual: repetição não aumenta a força da evidência nem afirma que os eventos pertencem ao mesmo ataque. Detecções, referências, relações e contagens por nível ficam intactas. Uma mesma ocorrência pode conservar várias verificações. Resumo e seletor continuam contando indícios; a lista identifica separadamente o número de cartões e de registros.

O agrupamento é calculado no backend sobre o universo completo, antes de paginar, tanto em memória quanto em SQLite, sem varredura adicional ao mudar o nível. A política `pattern-groups-1` participa da identidade da análise. No contrato, `episodes` pode conter um cartão com `grouping.kind = "pattern"`, `occurrence_count` e versão; os episódios originais continuam consultáveis pelo `source_episode_id` dos membros retornados do armazenamento. Salvar no Caso e filtrar no Explorar coletam todos os membros exatos, inclusive os de outras páginas.

Testes cobrem 205 ocorrências, paridade memória/SQLite, ordem invertida, filtros, níveis, todas as páginas, acesso aos episódios originais e separação por contexto. O teste de navegador verifica lista compacta, destaque da ocorrência escolhida, preservação de 45 eventos ao salvar/filtrar e layout estreito.

## Busca em qualquer campo

Nome de campo e formato conhecido não são requisitos para encontrar uma estrutura específica. Um XML XXE em `vendorBlob`, um payload XSS em `opaque.items.0` ou uma linha HTTP dentro de um objeto desconhecido são examinados com procedência completa. O contexto é usado para qualificar a afirmação: payload presente, tentativa em requisição, diagnóstico de bloqueio, execução ou efeito. Uma ocorrência em campo opaco não herda o sentido de outro campo do mesmo evento.

Campos semanticamente identificados como resposta ou comando conservam esse papel. Consultas SQL normais, scripts devolvidos por uma aplicação, saídas literais (`echo`), exemplos explícitos e nomes produzidos pelo enriquecimento não se tornam tentativas de exploração. Campos originais chamados `description` continuam pesquisáveis; somente a descrição gerada pela aplicação fica fora. Dados `_sec.*` fornecidos pelo log não podem simular sinais internos.
