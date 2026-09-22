# LogInsight — avaliação e proposta de evolução

Registro do diagnóstico anterior às mudanças. A implementação e os testes realizados estão em [Reformulação e validação](repaginacao-e-validacao.md).

Avaliação em 22/09/2026. Prioridade explicitada pelo usuário: analisar arquivos grandes, facilitar a visualização, produzir insights sobre possíveis ocorridos e maximizar a praticidade.

## Direção recomendada

O objetivo do produto é reduzir o esforço entre receber logs heterogêneos e entender o que pode ter acontecido. A experiência principal deve ser: **abrir arquivos → enxergar o panorama → identificar algo relevante → conferir os eventos → guardar ou compartilhar a conclusão**.

O projeto já contém boa parte das ferramentas necessárias: parsers de diversos formatos, leitura de Windows Event Log, índice por offsets, materialização sob demanda, filtros, facetas, enriquecimento de códigos, campos derivados, agrupamentos, séries, cubo, trilhas e casos. A evolução deve organizar essas capacidades em um fluxo mais simples e corrigir diferenças de comportamento entre os caminhos de análise.

Recomendo manter Tauri/Rust e aproveitar o motor existente. Reescrever a interface inteira ou trocar o motor por outra tecnologia antes de medir os gargalos acrescentaria risco sem benefício demonstrado.

## O que foi verificado

- Leitura do frontend, modelos, parsers, consultas, análises, persistência e servidor MCP.
- Inspeção visual do explorador e do caso pelo preview existente, com dados simulados. Isso avalia a interface, não comprova o comportamento do backend nem o desempenho do aplicativo nativo.
- `cargo test --lib -- --list`: quatro testes identificados.
- `cargo test --lib parse_`: dois testes passaram, sem falhas. A compilação apresentou um aviso de `unused_mut`.
- O benchmark dependente de arquivo externo e a extração de catálogo do Windows não foram executados. Não houve benchmark com arquivos grandes nem validação de ponta a ponta no Tauri.
- Nenhuma mudança funcional foi realizada nesta avaliação.

## 1. Experiência e organização das telas

### Entrada: abrir e entender

A tela inicial deve ter uma ação dominante: **Abrir logs**, aceitando arrastar arquivos e pastas. Oferecer também sessões recentes e Windows Event Log. Criar uma sessão temporária automaticamente; dar nome e salvar como investigação pode acontecer depois.

A importação começa com autodetecção e uma prévia. Só pedir intervenção quando houver ambiguidade: formato, encoding, fuso, ano ausente ou agrupamento de linhas. Oferecer presets compreensíveis antes de regex.

Mostrar separadamente: bytes lidos, eventos reconhecidos, cobertura temporal, fontes e avisos de leitura. Uma leitura parcial deve permitir exploração parcial claramente identificada. Cancelamento deve interromper o trabalho do backend, mantendo o último estado válido.

### Navegação principal sugerida

| Área | Pergunta que responde | Conteúdo |
|---|---|---|
| Resumo | O que merece atenção? | Período, volume, fontes, qualidade, evolução temporal e principais achados |
| Explorar | Quais eventos sustentam isso? | Busca, filtros, tabela, padrões, contexto anterior/posterior e detalhes |
| Comparar | O que mudou? | Antes/depois, períodos equivalentes, fontes ou ambientes |
| Evidências | O que já descobri? | Eventos e recortes salvos, hipóteses, notas e conclusão |

Fontes ficam em um painel acessível e persistente: arquivo, formato, tamanho, período, host/serviço, qualidade e estado de indexação. Selecionar duas fontes deve permitir pesquisá-las juntas sem uma operação explícita de materialização chamada “unir”.

“Agrupamento”, “Painéis” e “Cubo” passam a ser modos de análise dentro de Explorar, com nomes como **Agrupar**, **Gráfico** e **Tabela dinâmica**. A trilha passa a ser uma ação do evento: **Ver contexto**. Linha do tempo horizontal e vertical podem ser opções de visualização da mesma informação.

O caso continua útil como organização persistida da investigação. Entretanto, começar uma análise simples não deve exigir entender casos, artefatos, estações e itens técnicos. “Estações” pode evoluir para “Hosts e serviços”, acomodando containers, aplicações e dispositivos.

### Uma única área de exploração

Estrutura sugerida:

```text
Sessão: Falha de pagamentos       Fontes: 3 de 5       Período: 14:00–15:00
[Resumo] [Explorar] [Comparar] [Evidências]
Buscar nos logs…                  [Filtros] [Salvar recorte]
Histograma compacto, com seleção de intervalo
Campos e filtros | Eventos / Padrões / Gráfico | Detalhes da seleção
Estado da operação · cobertura dos dados · resultados · cancelar
```

O escopo precisa acompanhar qualquer tabela ou gráfico: todas as fontes selecionadas, fonte individual ou evidências salvas. Exibir separadamente “eventos encontrados”, “eventos exibidos” e “eventos analisados”.

Melhorias práticas:

- Mensagem como coluna dominante; data, nível e origem compactos; descrição de código no detalhe.
- Densidades confortável e compacta, quebra de linha opcional e coluna fixa de horário.
- Histograma aproveitando a largura disponível e recolhível; seleção temporal por arraste.
- Painel de detalhes redimensionável, sem perder a tabela e o filtro atual.
- Campos favoritos, busca de campos e recomendação por formato; evitar uma árvore extensa como ponto de partida.
- Ações visíveis: incluir/excluir valor, ver contexto, seguir requisição, salvar evidência. Menu contextual como atalho adicional.
- Atalhos: buscar, abrir arquivos, próximo/anterior, desfazer filtro e ir para horário/linha. Preservar posição ao abrir um detalhe.
- Visualizações salvas junto da busca; filtros e navegação sem mudanças de escopo implícitas.
- Ordenação, facetas e gráficos com estados de carregamento independentes. Resultados antigos só permanecem visíveis se claramente marcados como desatualizados.

## 2. Insights que realmente facilitam a investigação

A primeira versão deve funcionar localmente, com regras e estatísticas verificáveis. IA pode explicar resultados e sugerir consultas posteriormente, usando o mesmo motor.

| Insight | Como apresentar | Próxima ação |
|---|---|---|
| Aumento de erros | Contagem, taxa sobre o volume total e comparação com intervalo equivalente | Abrir pico e comparar com período anterior |
| Padrões repetidos | Mensagem normalizada, ocorrências, primeira/última aparição e exemplos | Expandir instâncias preservadas |
| Padrão novo | “Primeira aparição no conjunto carregado”, com período efetivamente observado | Ver início e eventos próximos |
| Mudança de latência | p50/p95/p99, unidade, cobertura do campo e volume | Abrir requisições lentas |
| Falhas em sequência | Tentativas, timeout, repetição e eventual sucesso ligados por identificador | Seguir a requisição |
| Reinícios ou exceções | Evidência de inicialização/finalização e padrões de stack trace | Inspecionar antes e depois |
| Lacuna temporal | Intervalo sem registros e fontes afetadas | Conferir rotação, cobertura e outros arquivos |
| Concentração de falhas | Serviço, endpoint, host, usuário ou origem com maior mudança | Filtrar entidade e comparar |

Priorizar explicações como “a taxa de erros passou de 1% para 18% neste intervalo” sobre “há 8.000 erros”. Agrupar vários alertas relativos ao mesmo episódio para evitar excesso de cartões.

Cada achado deve ter: observação, regra usada, período, volume e cobertura, eventos de apoio, possíveis explicações e próximo passo. Distinguir **fato observado**, **correlação** e **hipótese**. Não apresentar coincidência temporal como causa comprovada. Ausência de registros pode indicar um arquivo incompleto, não necessariamente indisponibilidade.

Exemplo ilustrativo, sem representar dados reais do projeto:

> Entre 14:32 e 14:38, aumentaram os timeouts da API de pagamentos. Eles se concentram em duas instâncias. Há recusas de conexão no mesmo período. Hipótese: indisponibilidade de uma dependência. Ver os eventos, comparar com o período anterior ou investigar a conexão.

Recursos de alto retorno:

1. **Agrupamento de mensagens por padrão**, substituindo parâmetros variáveis apenas na representação agrupada; preservar os registros originais e permitir desfazer agrupamentos ruins.
2. **Comparação antes/depois**, destacando padrões novos, padrões que desapareceram e mudanças de frequência. Normalizar duração e volume; tratar baseline zero e pouco histórico explicitamente.
3. **Seguir uma transação**, por `trace_id`, `request_id`, sessão ou outra chave escolhida. Relação por host/tempo deve aparecer como relação provável, não identidade comprovada.
4. **Receitas prontas**, como “investigar pico de erros”, “encontrar reinício” e “comparar execução boa e ruim”. Executam filtros e análises transparentes, que o usuário pode alterar.
5. **Guardar um achado inteiro**, incluindo filtro, fontes, período e eventos de apoio, com exportação de relatório e dados.

## 3. Arquivos grandes: arquitetura e precisão

### Preservar a leitura sob demanda entre várias fontes

`load_files_impl` e o caminho de união em `load_file_impl` materializam os arquivos em `Vec<Event>`. Assim, um mecanismo econômico para arquivo individual perde essa característica justamente quando a investigação reúne várias origens.

Evoluir para um conjunto de fontes independentes: cada arquivo mantém seu índice e identidade. A consulta aplica filtros nas fontes selecionadas e combina somente os resultados necessários. Agregações devem ser incrementais, sem construir previamente todos os eventos completos.

Persistir índices reutilizáveis com versão do parser e identidade do arquivo. Na reabertura, validar tamanho, modificação e verificação de conteúdo conforme o modo de uso. Se o arquivo mudou, invalidar ou atualizar o índice. Para leitura ao vivo, tratar append, truncamento, substituição e rotação explicitamente.

### Reduzir o trabalho por interação

O motor já aproveita metadados e `explore_snapshot` reutiliza uma seleção. Entretanto, consultas ainda podem percorrer todos os eventos, ordenar todos os candidatos e materializar eventos para facetas como `source` a cada atualização.

Evolução recomendada:

- Índice temporal para restringir a janela antes da busca textual.
- Metadados por bloco e índices seletivos para campos frequentes; construídos conforme necessidade.
- Cache limitado por memória, com invalidação por versão do conjunto, parser e campos derivados.
- Separar carregamento das linhas visíveis de contagens e facetas globais mais custosas.
- Paginação por cursor e ordenação estável, evitando refazer ordenações globais a cada página.
- Agregações em fluxo e processamento em disco quando exceder o orçamento de RAM.
- Trabalhos identificados por operação, progresso, cancelamento cooperativo e limites de concorrência. Consultas antigas devem parar de consumir recursos quando substituídas.
- Perfil de memória que diferencie heap, memória residente e páginas mapeadas. `mmap` não significa consumo de memória constante.

A pré-alocação atual de metadados usa `mmap.len() / 48`; medir seu impacto em arquivos com linhas longas antes de mantê-la como heurística universal.

### Resultados completos, progressivos ou aproximados

O código limita a análise de arquivos indexados e eventos de caso aos primeiros 50.000 eventos filtrados. Fontes em memória usam outro caminho, sem o mesmo corte. Isso pode fazer o resultado variar conforme o modo de carregamento. O corte inicial também pode ocultar um incidente no final do arquivo.

O contrato de resposta deve informar:

```text
eventos encontrados / eventos examinados / cobertura temporal
resultado completo? / estimado? / limite atingido?
versão das fontes / tempo gasto / motivo da limitação
```

Para contagem, soma e histogramas, preferir cálculo completo em fluxo. Para perfis exploratórios, uma amostra pode ser aceitável se for representativa, claramente indicada e substituível por análise completa. Distinguir top N exibido de dados efetivamente descartados no cálculo.

Há ainda limite interno de 50.000 valores distintos no cubo. Ele exige indicação específica de limite ou substituição por cálculo exato em disco/estimativa com erro declarado; não deve parecer contagem completa.

## 4. Confiabilidade do significado dos dados

### Qualidade da leitura como parte do produto

Falhas de parsing atualmente podem cair para texto puro. Isso preserva legibilidade, mas não informa claramente quanto do arquivo foi interpretado. Exibir diagnóstico por fonte: registros reconhecidos, registros genéricos, datas ausentes, campos inválidos e exemplos problemáticos.

A detecção inicial usa até 60 linhas, e a descoberta de colunas indexadas observa os primeiros 2.000 eventos. Usar amostras distribuídas, descoberta incremental e aviso de campos ainda não catalogados. Um campo raro no final do arquivo pode ser precisamente o mais importante.

Ampliar entrada em ordem de utilidade: arquivos rotacionados/pastas, `.gz` e pacotes, `.evtx` exportados, encodings frequentes no Windows e multiline configurável. Já existe tratamento multiline para famílias Java; a evolução deve generalizá-lo para outros formatos, não recriá-lo.

### Tempo e campos canônicos

Datas sem fuso usam o horário local da máquina que analisa o arquivo. A configuração por origem deve permitir fuso explícito, ano inferido quando ausente e ajuste documentado de relógio, preservando sempre o valor original.

Separar: arquivo de origem, host, serviço, processo, parser, nível original e nível normalizado. O campo `source` hoje concentra conceitos diferentes. Mapear aliases como `src`, `client_ip` e `ip_cliente` para campos comparáveis, mantendo atributos originais.

Usar OpenTelemetry como referência de interoperabilidade para horário do evento, horário observado, severidade, recurso e contexto de rastreamento, adaptando o modelo às necessidades locais. Isso facilita relacionar logs distintos sem obrigar todos a terem o mesmo formato. Referência: [Logs Data Model](https://opentelemetry.io/docs/specs/otel/logs/data-model/).

### Identidade e evidências

IDs de eventos são posições locais. A deduplicação do caso combina ID, horário, fonte, código e mensagem sem incluir a identidade do arquivo. Eventos iguais em arquivos diferentes podem colidir.

Criar referência estável com identidade/versão da origem e offset ou identidade nativa do registro. Preservar referências ao conteúdo bruto, parser e transformações usadas. Separar evidência capturada de consulta salva: uma captura não muda; uma consulta pode ser reexecutada.

A inclusão de grupo no caso preserva até 500 resultados e já informa a contagem incluída. Evoluir para a escolha explícita entre guardar exemplos, guardar o recorte consultável e exportar todos os resultados. Não copiar grandes conjuntos de eventos para o estado JavaScript.

## 5. Ajustes concretos identificados no código

As indicações abaixo vêm de leitura estática; não são reproduções de ponta a ponta no aplicativo nativo.

| Prioridade | Evidência | Consequência e ação |
|---|---|---|
| Alta | `src-tauri/src/lib.rs:247`, `sources.rs:1482`, `sources.rs:1296` | O carregador resolve `custom:nome`, mas passa esse identificador adiante; o parser seleciona a regra customizada apenas para `custom`. O fluxo indica queda para texto genérico. Normalizar o identificador e testar salvar → carregar → consultar, além do botão Testar. |
| Alta | `src-tauri/src/lib.rs:1283`, `analysis.rs:246` | Corte de 50.000 no conjunto de trabalho sem metadados de cobertura em séries. Tornar precisão explícita e uniformizar caminhos. |
| Alta | `src-tauri/src/lib.rs:337` e caminho de merge em `load_file_impl` | União materializa eventos completos; substituir por conjunto de índices consultáveis. |
| Alta | `frontend/app.js:5388` | Deduplicação omite identidade do arquivo. Usar referência estável de origem e evento. |
| Alta | `src-tauri/src/lib.rs:1068` | A trilha em memória compara o centro com a posição no vetor, não com `Event.id`. Em subconjuntos de caso, posições e IDs podem divergir. Resolver pela referência estável; se o centro não existir, informar em vez de cair silenciosamente no último evento. |
| Média | `src-tauri/src/query.rs:32` e `:114` | Regex inválida vira busca “contém”. Retornar erro claro, preservando a última consulta válida. Validar também operadores desconhecidos, que hoje podem não restringir o resultado. |
| Média | `frontend/app.js:2665`, `src-tauri/src/lib.rs:1522` | Cada salvamento copia e regrava todo o armazenamento de casos. Há fila, mutex e backup, que são pontos positivos; faltam operações incrementais e controle de versão entre clientes. |

## 6. Organização interna e integração

O frontend concentra aproximadamente 6.800 linhas em `app.js`, com estado global, renderização, navegação, regras de caso e chamadas de backend. Separar gradualmente por responsabilidade: fontes, consultas, insights, evidências, navegação e componentes de interface. Contratos tipados entre frontend, backend e MCP ajudam mais imediatamente que uma troca de framework.

Casos devem evoluir de JSON opaco para um domínio versionado no backend, com migrações, operações de atualização específicas e persistência transacional. Avaliar SQLite para metadados e evidências após definir requisitos; não implica armazenar todo o conteúdo bruto nele.

O MCP já reaproveita implementações do backend, o que é uma boa base. Hoje opera sobre a fonte global carregada e pode mutar esse estado. A evolução deve aceitar `dataset_id`, `query_id` e referência de evento, evitando que uma consulta automatizada dependa da última fonte aberta na interface.

Priorizar ferramentas orientadas a tarefa: resumo do conjunto, padrões de mensagem, comparação de períodos, eventos relacionados e gravação de achados. A interface humana e os agentes devem receber os mesmos números e avisos de cobertura.

O servidor já usa loopback, mas vem habilitado por padrão. No código da aplicação não há autenticação explícita; os defaults de proteção do SDK não foram auditados aqui. Recomendo tornar a ativação visível, separar acesso de leitura e mutação e verificar autenticação e validação de origem. Referência: [MCP Streamable HTTP — proteções de transporte](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

Para assistência por IA: enviar agregados e exemplos delimitados, com referência aos eventos, não arquivos inteiros. Exibir a consulta executada. Conteúdo de logs é dado não confiável, não instrução para o agente. Mascaramento de segredos precisa abranger exportações, bruto e respostas MCP quando habilitado.

Integrações remotas com coletores e plataformas de observabilidade podem vir depois, conforme fontes realmente usadas. A integração mais urgente é tornar arquivos de origens diferentes comparáveis e pesquisáveis juntos.

## 7. Ordem de implementação e critérios de sucesso

| Etapa | Entrega | Critério de conclusão |
|---|---|---|
| 1 — Confiar nos resultados | Corrigir formato customizado e identidade/trilha; contratos de precisão e parsing; testes determinísticos | Mesmos registros produzem mesmos filtros e agregações nos caminhos individual, múltiplos arquivos e evidências; limites aparecem na interface |
| 2 — Sustentar volume | Conjunto de índices, orçamento de memória, consultas canceláveis e cache limitado | Arquivos maiores que a RAM podem ser explorados sem materialização integral; cancelamento interrompe trabalho; reabertura reutiliza índices válidos |
| 3 — Simplificar o caminho | Abertura direta, Resumo, explorador único e escopo visível | Usuário abre arquivos e chega aos eventos de um achado sem configurar gráfico ou entender conceitos internos |
| 4 — Entregar insights | Padrões, picos, antes/depois e sequência de requisição | Todo achado mostra evidência, cobertura e método; ruído e falsos positivos são avaliados em casos conhecidos |
| 5 — Preservar e integrar | Exportação, pacote de investigação, persistência incremental e MCP por conjunto | Reabrir ou compartilhar preserva fontes, referências, recortes e limitações; agentes não trocam o contexto da UI implicitamente |

Algumas melhorias visuais podem ocorrer junto das etapas 1 e 2. Insights definitivos dependem primeiro da confiabilidade do universo analisado.

### Medições propostas

Fixar uma máquina de referência e usar um corpus versionado: 1 GB, 10 GB e 50 GB, mais muitos arquivos menores, JSONL, texto e stack traces. Incluir campos tardios, datas fora de ordem, registros inválidos, linhas grandes, alta cardinalidade e arquivos rotacionados. São cenários de teste propostos, não capacidade já demonstrada.

Medir tempo até a primeira prévia, indexação completa, primeira página, filtro temporal, busca textual completa, pico de memória, tamanho do índice e reabertura. Separar execução fria e quente.

Metas iniciais de UX, a validar nessa máquina: feedback de ação abaixo de 100 ms; prévia em poucos segundos quando tecnicamente possível; interações já indexadas com p95 abaixo de 1 s; cancelamento reconhecido em até 1 s. Buscas completas de dezenas de GB terão duração dependente de armazenamento e formato e devem mostrar progresso real.

Para usabilidade, usar tarefas: localizar o primeiro erro de um episódio, comparar antes/depois, seguir uma requisição e exportar a evidência. Medir tempo, erros de interpretação e necessidade de ajuda. O sucesso principal é o tempo até encontrar um achado útil e verificável, combinado com a precisão dos resultados.

Para testes, acrescentar fixtures pequenas e determinísticas para parsers, paridade memória/índice, timezone, multilinha, consultas, persistência e conflitos. Manter benchmarks e integrações dependentes do Windows separados da suíte básica. Os scripts atuais de preview são úteis para exploração visual, mas usam mocks e não substituem esses testes.

**Próxima versão recomendada:** abrir vários logs grandes, receber um resumo automático confiável, enxergar padrões e mudanças, clicar para investigar e salvar um achado. Esse conjunto entrega o maior avanço no objetivo descrito.
