# Evolução da análise genérica

Esta revisão aproxima a análise dos registros: distribuições, picos, padrões e desvios levam a filtros que abrem suas evidências. As descobertas são calculadas localmente. Não há envio dos logs a um serviço de IA.

Este documento registra implementação, limites e medições de desenvolvimento. Em 23/09/2026, um build local da revisão anterior (0.1.0) gerou os instaladores Windows x64 `.exe` (NSIS) e `.msi`, incluindo o seletor de ícones. Foi usado `CARGO_TARGET_DIR=src-tauri/target/verification` para preservar o executável já aberto. Esses instaladores não foram instalados nem executados como parte da validação local. A versão 0.2.0 acrescenta a separação de áreas descrita abaixo e passa pelo workflow de testes e builds Windows/Linux antes da publicação em [Releases](https://github.com/felipevasc/logs/releases). A limitação de execução dos testes nativos neste computador está descrita na seção de validação.

## Análise e Caso

O alternador global distingue **Análise**, com os registros das fontes abertas, de **Caso**, com os registros selecionados como relevantes. Os destaques são amarelos na Análise e roxos no Caso, com variantes para os temas claro e escuro. A transição lateral respeita a preferência do sistema por movimento reduzido. Na tabela da Análise, uma marca roxa identifica registros já incluídos no Caso.

Filtros, busca, colunas, agrupamentos, configurações de visualização e posição de navegação são preservados por área. A troca também altera o conjunto consultado por Resumo, Explorar, Resumir, Descobrir, Cruzar dados, Linha do tempo, Jornadas, Comparar e exportação de registros. Um Caso vazio não consulta os arquivos como alternativa. Abrir arquivos e conexões leva à Análise; incluir um registro no Caso preserva uma cópia e não altera o arquivo de origem.

Os comandos nativos usados pelas duas áreas recebem `caseEvents` opcional: ausência mantém a fonte indexada; uma lista, inclusive vazia, limita a operação àqueles registros. Isso evita usar os números das fontes em painéis do Caso. Respostas assíncronas antigas são descartadas ao trocar de área ou caso. As anotações e a montagem da cronologia continuam pertencendo ao Caso.

## Fluxo de investigação

| Área | Uso |
| --- | --- |
| Registros | Explorar o recorte, selecionar colunas e filtrar a partir dos valores. |
| Resumir | Escolher um campo sugerido, obter grupos automaticamente, buscar, ordenar e comparar medidas. |
| Descobrir | Alternar entre Visão geral, Frequências, No tempo, Padrões, Desvios, Mudanças, Ameaças e Meus gráficos. |
| Cruzar dados | Combinar dimensões e medidas com busca, paginação e intensidade de cor nas células. |
| Jornadas | Ligar registros pelo mesmo identificador e investigar a sequência entre origens. |
| Conexões | Consultar Elasticsearch ou o proxy do Console do Kibana e importar uma cópia local. |

O menu contextual permite consultar o Top 10 de um campo e aproveitar os valores como filtros. Frequências cruzadas mostram a distribuição **dentro de cada grupo**, com acesso aos registros por segmento. A apresentação limita grupos/categorias e identifica os valores restantes como “Outros”.

Em gráficos temporais, selecionar um ponto abre a comparação dos grupos naquele intervalo. Para campos numéricos, são comparadas contagem, média e máximo; a referência inclui o período datado completo do gráfico, inclusive o próprio pico. Isso ajuda a localizar diferenças sem atribuir causalidade. Unidades incompatíveis são informadas, impedindo gráficos numéricos que misturem, por exemplo, duração e bytes.

O mapa temporal coloca o **tempo no eixo horizontal e as categorias nas linhas**. Cada círculo reúne os registros daquele grupo e intervalo: tamanho representa contagem, cor representa uma medida numérica e borda representa uma segunda medida numérica. Seletores permitem escolher os campos para categoria, cor e borda. Assim, volume e medidas diferentes podem ser comparados na mesma faixa temporal. A seleção de um ponto leva à categoria e ao intervalo correspondentes.

Séries numéricas distinguem um intervalo sem valores válidos de um zero medido, evitando que lacunas apareçam como quedas para zero. Cliques sobre categorias usam igualdade exata, preservando diferenças de caixa e espaços.

### Timeline

A orientação horizontal organiza eventos em linhas por título e origem, com marcadores individuais posicionados proporcionalmente ao horário. Rótulos fixos, régua temporal e zoom ajudam a acompanhar a mesma linha ao percorrer períodos extensos. A orientação vertical conserva a leitura compacta, com marcas de horário visíveis e contagens pequenas.

Notas e setas continuam editáveis pelo botão direito; Enter abre a edição do elemento selecionado e Shift+F10 oferece o menu pelo teclado. A matriz, a navegação e essas interações foram verificadas nos testes de preview; capturas locais estão em `output/playwright/timeline-matrix-*`.

O seletor de notas usa os **1.422 ícones sólidos e 572 marcas** do Font Awesome Free 7.3.1 já incluído localmente. Apresenta uma matriz com rolagem, busca em português/inglês, categorias e nome no hover; termos de TI como firewall, VPN, malware, servidor e backup também encontram opções relacionadas. São materializados até 96 botões por página, com navegação por teclado. O catálogo só é carregado ao abrir o editor, funciona sem rede e preserva os ícones das notas anteriores. A origem, a licença e o comando de atualização estão em `frontend/vendor/fa/README.md`.

As timelines podem ser exportadas em PNG ou PDF, incluindo o conteúdo fora da área visível. O nome contém o caso, a orientação e a data, com extensão explícita. No aplicativo, o arquivo passa pelo diálogo nativo de salvamento; no preview, o servidor envia `Content-Disposition` e uma URL terminada em `nome.ext`, evitando nomes de UUID em navegadores que ignoram o atributo de download de URLs Blob. PNG tem teto de 24 milhões de pixels e 16.000 pixels por lado; PDFs extensos são divididos em páginas, repetindo cabeçalhos e rótulos. Cancelamento e limites de tamanho aparecem antes de concluir a exportação.

### Jornadas e investigação a partir de um registro

O backend de Jornadas reúne registros com o mesmo **campo e valor exatos**, inclusive entre fontes. Ele sugere campos de trace, request, correlation e session por seus nomes, sem confundir `event_id` com identificador de jornada. Os nomes concretos permanecem distintos: `trace.id` e `trace_id` não são fundidos implicitamente. Caixa e espaços no valor também são preservados.

O índice apresenta quantidade, início/fim, duração observada, erros, avisos, origens e registros sem horário. Agrupar pelo identificador não prova uma relação causal; a duração é a distância entre o primeiro e o último horário disponível. O detalhe ordena registros por horário crescente e deixa os sem data ao final. Investigar usuário ou IP exige início e fim explícitos, evitando tratá-los como identificadores de uma única transação.

Agrupamento e paginação usam tabelas SQLite temporárias com cache de 2 MiB; os eventos são lidos individualmente e apenas a página solicitada é materializada. O limite padrão é de 50 grupos ou 100 eventos por página, com tetos de 200 e 500. Prévias de eventos são limitadas a 64 KiB serializados; `rows_clipped` informa cortes e o detalhe recupera o original. Identificadores acima de 4.096 bytes são recusados no detalhe ou contados como `skipped_keys` no índice. São exibidas até 12 origens por grupo, com indicador de truncamento. Usuário e IP exigem a janela tanto na lista quanto no detalhe.

O perfil de campos conta cobertura no recorte completo, considera até 128 campos e limita o conjunto de valores distintos a 512 por campo e aproximadamente 8 MiB no total. Quando esse orçamento acaba, `distinct` fica indisponível em vez de declarar uma contagem aproximada como exata. Cancelar uma operação descarta seu resultado incompleto. O recorte de um caso é respeitado sem incluir registros externos a ele.

## Descobertas e seus limites

Ameaças usa um [catálogo JSON editável](regras-ameacas.md), com 378 regras de tentativas, respostas e indicadores. Inclui saídas de comandos, conteúdo de arquivos como passwd, erros de banco com contexto, dumps, configurações, metadados cloud e formatos de credenciais expostas. Os painéis apresentam registros únicos no tempo, categorias, origens e regras, com inspeção paginada das evidências. O texto é examinado localmente; nenhuma string encontrada é executada. A análise distingue registros de correspondências: um registro pode acionar várias regras. Limites de texto e registros sem horário ficam explícitos. Erros do catálogo interrompem a análise e informam a causa; não são convertidos em “nenhuma ameaça encontrada”. Correspondência indica conteúdo para examinar; não estabelece a direção da resposta, uma invasão ou a validade de uma credencial.

`discover_patterns` aceita os filtros atuais e, opcionalmente, os eventos de um caso. O total, os erros, os avisos, a ausência de horário e os extremos temporais são contados no recorte completo. Para descobrir padrões, uma amostragem por reservatório com semente fixa distribui a seleção ao longo de toda a passagem pelos registros, mantendo no máximo **6.000 referências**. Os registros amostrados são materializados depois. Repetir a mesma entrada na mesma ordem produz a mesma amostra.

O resultado distingue `total`, `sample_count`, `limited` e `complete`. `fields_limited` e `temporal_limited` informam limites da busca por candidatos; concluir a leitura não significa examinar toda combinação possível. As contagens das descobertas são da amostra. Os filtros de evidência são aplicados ao recorte completo, que pode conter mais ocorrências. Um evento raro pode não entrar na amostra.

### Padrões de mensagens, categorias e números

- O perfil examina até 128 nomes de campos e seleciona até 24 pelo preenchimento. Nomes acima de 160 bytes e valores acima de 200 bytes não entram no perfil de categorias.
- Categorias precisam repetir valores e ter até 64 valores distintos. Campos constantes, quase únicos ou duplicados têm utilidade reduzida na seleção das análises.
- Templates normalizam parâmetros como números, IPs e identificadores na primeira linha da mensagem, limitada a 400 caracteres. O motor retorna até 20 templates repetidos; a visão atual apresenta até 12. Essa semelhança textual não implica significado idêntico.
- Desvios numéricos exigem pelo menos 20 números, 90% de preenchimentos numéricos e unidades compatíveis. O intervalo usa mediana ± `max(3,5 × 1,4826 × MAD; 3 × IQR)`. Só há sugestão quando existe ao menos um valor fora dele, sem ultrapassar 20% dos números; até oito campos são apresentados. Identificadores e códigos são excluídos dessa interpretação numérica.

### Desvios condicionais e mudanças no tempo

A análise temporal divide a amostra datada em até **12 janelas de duração igual**, incluindo o meio do período. Precisa de pelo menos 24 registros datados e duas janelas. Os resultados possíveis são nível, código ou template da mensagem.

**Desvios** procura combinações de um a três campos que normalmente produzem um resultado e passam a produzir outro em uma janela. Exige pelo menos 20 ocorrências fora da janela, resultado esperado em pelo menos 80% delas, três ocorrências do resultado divergente na janela e aumento de sua participação de pelo menos 35 pontos percentuais.

Exemplo da demonstração: `operação=Checkout + região=Sul + canal=Web` tem 66 mensagens normais fora de uma janela intermediária e seis timeouts dentro dela. Os cartões apresentam contexto, resultado esperado/observado, contagens, percentuais e intervalo; “Ver desvio” filtra as ocorrências, enquanto “Ver contexto” preserva os demais resultados daquele contexto e horário.

**Mudanças** compara a distribuição global dos resultados entre uma janela e o restante do recorte, sem exigir resultado dominante de 80%. Um template sem ocorrências fora da janela pode aparecer como novo quando reúne pelo menos três ocorrências e 15% da janela. “Novo” significa ausente no restante da amostra examinada, não inédito em todo o histórico.

A referência usa **todos os outros intervalos do recorte, anteriores e posteriores**. É uma comparação retrospectiva; não é previsão, teste de causalidade ou detector com taxa de falsos positivos calibrada.

Para limitar custo, são considerados até seis campos de contexto, cada um com 2–12 valores distintos e pelo menos 40 preenchimentos. São excluídos constantes, identificadores, medidas predominantemente numéricas e duplicações de campos. O contador de contextos tem teto de 32.768 entradas; até 256 contextos com suporte mínimo de 24 são examinados. Cada tipo de resultado conserva até 64 rótulos candidatos, mantendo os demais no denominador para não criar uma falsa maioria. A pontuação prioriza aumento de participação e número de ocorrências; episódios e contextos redundantes são deduplicados. Há no máximo oito sugestões em cada lista.

## Importação e agregações

Arrays JSON de objetos, compactos ou formatados em várias linhas, são reconhecidos por offsets sobre o arquivo mapeado em memória. Não é necessário desserializar todo o array de uma vez. O leitor respeita strings/escapes, aceita BOM UTF-8 e rejeita arrays estruturalmente truncados. Objetos aninhados são achatados até 12 níveis; uma chave explicitamente pontuada prevalece sobre o caminho aninhado equivalente. Arrays internos permanecem como valores.

Aliases são reconhecidos sem diferenciar maiúsculas/minúsculas e preferem valores escalares úteis: um objeto `service`, por exemplo, não deve ocultar `service.name`. Epochs em segundos, milissegundos, microssegundos ou nanossegundos são identificados por magnitude e normalizados para milissegundos. Isso é uma heurística, não reconhecimento universal de datas. Textos UTF-8 multibyte não causam mais falha na detecção de prefixos. O cache de índices passou a `indexes-v4` para refazer metadados antigos.

Rankings de **contagem por valor** conservam resultados exatos mesmo com alta cardinalidade. Acima de 25.000 chaves ou aproximadamente 8 MiB de armazenamento estimado, usam um SQLite temporário em disco, com cache de páginas de 2 MiB. O Top N é ordenado por contagem e desempate pelo valor; um valor frequente encontrado no final não se perde. A seleção das séries temporais por categoria usa o mesmo contador.

Esse limite não torna toda operação da aplicação constante em memória: o índice cresce com os registros, alguns caminhos existentes guardam referências dos resultados, e outras agregações numéricas mantêm sua arquitetura anterior. O marcador legado `(vazio)` ainda pode colidir com um valor literal de mesmo texto. Séries por categoria exibem até seis categorias; suas contagens não representam necessariamente o total do recorte. Cancelamentos descartam resultados parciais em vez de apresentá-los como contagens completas.

## Conexões Elasticsearch e Kibana

A configuração salva nome, tipo, URL base, índice/padrão de índices, campo temporal (padrão `@timestamp`), usuário, limite e uma cláusula opcional Query DSL. Exemplo aceito: `{"match":{"service.name":"api"}}`, sem o envelope `query` ou opções de paginação. O filtro tem teto de 64 KiB. Datas inicial/final são opcionais.

A implementação oferece **Basic com usuário/senha**, ou acesso anônimo quando permitido pelo servidor. API key, Bearer e SSO não estão implementados, embora o Elasticsearch ofereça outros métodos de autenticação. [Autenticação oficial](https://www.elastic.co/docs/api/doc/elasticsearch/authentication).

| Transporte | Contrato implementado |
| --- | --- |
| Elasticsearch direto | Abre `POST /{index}/_pit?keep_alive=2m`, consulta `POST /_search` e fecha `DELETE /_pit`. Usa o PIT mais recente retornado e `search_after`. |
| Kibana | Encaminha as mesmas operações por `POST {base}/api/console/proxy?path=…&method=…`, preservando o caminho base/espaço. Envia `kbn-xsrf` e verifica também o status do proxy. |

PIT mantém uma visão consistente para paginar, e `search_after` evita depender de deslocamentos crescentes; o cliente atualiza o identificador retornado e tenta encerrar o PIT ao terminar, falhar ou cancelar. [PIT](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-open-point-in-time) e [paginação oficial](https://www.elastic.co/docs/reference/elasticsearch/rest-apis/paginate-search-results).

O caminho do Kibana é uma integração com o proxy do Console, não uma garantia de API pública estável em todas as versões. Depende da disponibilidade, permissões e configuração desse recurso. O Console é a interface do Kibana para enviar requisições às APIs Elastic. [Documentação do Console](https://www.elastic.co/docs/explore-analyze/query-filter/tools/console). O contrato concreto está em [remote.rs](../src-tauri/src/remote.rs).

As páginas têm até 500 registros e respostas de até 32 MiB. O limite inicial é 100.000 registros, configurável entre 1 e 5.000.000. Com campo temporal, a ordenação é decrescente, com desempate `_shard_doc`; sem ele, não há promessa de trazer os mais recentes. `count`, `total`, `totalRelation` e `limited` distinguem a cópia importada da quantidade remota conhecida.

Os documentos `_source` viram JSONL local, com metadados remotos em campo separado sem sobrescrever o original. O arquivo só é publicado após concluir a transferência. Respostas parciais por timeout ou falha de shards/clusters são rejeitadas; atingir o limite escolhido é informado como recorte limitado. TLS usa a validação de certificados do sistema; redirecionamentos não são seguidos. A conexão tem timeout de 10 segundos e cada requisição de 30 segundos.

Configurações ficam em SQLite local. No Windows, salvar a senha é opcional e usa DPAPI vinculado ao usuário; nos demais sistemas, a senha permanece apenas na sessão. **Não houve teste contra servidor Elasticsearch/Kibana real nesta revisão**: a validação remota usou fixtures HTTP locais, incluindo paginação, PIT, proxy, erros e cancelamento.

## Validação e medições

A execução completa já confirmada de `cargo test --lib -- --test-threads=1` teve **42 testes aprovados, nenhum com falha e três ignorados**. Inclui arrays JSON, aliases, Unicode, épocas, contagens com spill, unidades em campos esparsos, ausência de medidas versus zero legítimo, filtros de igualdade exata, bordas temporais, desvios contextuais, conjunto estável, ausência de timestamp, truncamento de candidatos e fixtures remotas. Os filtros exatos foram conferidos tanto em memória como no índice, incluindo caixa, espaços, Unicode e escapes JSON; o comportamento anterior sem diferenciação de caixa continua disponível nos operadores existentes.

`cargo clippy --lib --message-format short` terminou com sucesso e 22 avisos de estilo na revisão final; não é uma execução sem avisos. A demonstração passou pelos sete modos de descoberta no navegador. Sua fixture conserva 6.000 registros em cinco fontes após importar Firewall (2.000 registros); os filtros de evidência dos sinais temporais foram conferidos contra os próprios registros da fixture.

Após acrescentar o catálogo de ameaças e Jornadas, `cargo check --tests` passou, incluindo os novos testes. A execução do novo binário de testes foi impedida pelo Windows com erro 225 de proteção antivírus; portanto, os 42 testes acima são a última execução completa anterior a esses módulos, não validação executada deles. O mock de Jornadas passou por verificações de igualdade exata, campos distintos, ordem temporal, ausência de horário e exigência de janela para usuário/IP. Consulte também a [validação do catálogo de ameaças](regras-ameacas.md#validação).

Os testes de Resumir reproduzem duas medidas simultâneas (soma de códigos e quantidade de nomes distintos), comparando os resultados com os registros de origem. Falhas de cálculo limpam os totais antigos e oferecem detalhes e nova tentativa. O preview antigo tinha um fallback de agregação incompatível, removido nesta revisão; o servidor agora deve ser iniciado por `npm run preview`, com reinício automático quando seu código muda. Os testes de exportação verificam também o nome efetivamente sugerido pelo navegador, a assinatura PNG/PDF, múltiplas páginas e conteúdo completo.

Os testes de navegador cobrem também os oito modos de Descobrir (Ameaças em uma suíte separada), Jornadas, conexões, timelines e o seletor de ícones. Ameaças passou nos dois endereços de preview anteriormente usados, incluindo atualização aditiva que preserva regras personalizadas e desativadas. Os ícones foram conferidos no tamanho mínimo de janela de 1024 × 680, incluindo pesquisa, teclado, notas antigas, persistência de marcas e ausência de sobreposição entre as células. A ajuda fica em um ícone que abre um modal; fechar a explicação devolve o foco ao editor e preserva o rascunho da nota. As exportações incluem as fontes locais solid, regular e brands; a presença de um ícone AWS foi conferida no PNG e no PDF. Esses testes usam os adaptadores de preview e não substituem a execução nativa bloqueada descrita acima.

Benchmarks locais **debug, Windows**, com 128.807 registros sintéticos por arquivo, mensagens únicas, campos aninhados e texto de aproximadamente 900 caracteres. Tempos de uma execução; não são garantia de desempenho em outra máquina ou em release.

| Medida | JSONL | Array JSON |
| --- | ---: | ---: |
| Tamanho | 128,43 MiB | 128,55 MiB |
| Indexação inicial | 12,18 s | 14,87 s |
| Reabertura com cache | 0,082 s | 0,084 s |
| Primeira página | 0,302 s | 0,232 s |
| Página seguinte | 0,052 s | 0,032 s |
| Visão geral existente, varredura completa | 26,87 s | 26,87 s |
| Top 10 exato de mensagens únicas | 13,73 s | 13,74 s |
| Descoberta, amostra de 6.000 | 2,15 s | 2,16 s |
| Pico do working set do processo | 156,68 MiB | 156,75 MiB |
| Pico observado de memória privada | 40,32 MiB | 40,76 MiB |

Memória corresponde ao processo de todo o benchmark, incluindo páginas do arquivo mapeado, não apenas à descoberta. A memória privada foi amostrada a cada 200 ms e pode não capturar picos mais curtos. A visão geral completa e a contagem exata de texto único continuam custosas; a amostra limita o custo da descoberta, não elimina a leitura necessária às contagens exatas.

Para reproduzir o benchmark ignorado, a partir de `src-tauri/`:

```powershell
$env:BENCH_MB = "128"
$env:BENCH_FORMAT = "jsonl" # use "array" para o segundo formato
cargo test --lib --no-default-features benchmark_large_index -- --ignored --nocapture --test-threads=1
```

O teste cria dados sintéticos temporários; as métricas de memória acima vieram de monitoramento externo do processo a cada 200 ms. Relatórios e runners locais ficam em `output/backend-bench/`, fora do versionamento. O histórico anterior permanece em [Reformulação e validação](repaginacao-e-validacao.md).
