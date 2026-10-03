# Modo Big Data

Este documento descreve as alterações e medições deste checkout, baseado na
versão 0.5.0. A linha publicada 0.12.0 já possui uma arquitetura posterior com
DuckDB e Tantivy; os números abaixo não medem ganhos sobre essa linha. A
reconciliação das versões e os critérios de entrega estão registrados em
[Prontidão da entrega](release-readiness.md).

O modo Big Data acrescenta um motor de busca embarcado ao Log Insight. O objetivo é reduzir o trabalho necessário para encontrar eventos em conjuntos grandes: consultar índices em disco, descartar candidatos impossíveis e aplicar os filtros atuais aos eventos restantes. A tabela, as buscas, as agregações, os detalhes, os Casos e as regras continuam usando os mesmos contratos.

## Como habilitar

1. Abra ou crie um Caso e carregue seus logs.
2. Clique em **Big Data**, no cabeçalho da interface.
3. Acompanhe **Preparando Big Data / Indexando Big Data** no status. A construção informa eventos processados e pode ser cancelada.
4. Quando ficar pronto, continue investigando normalmente. Clique novamente para desabilitar o modo.

A preferência fica salva no Caso. Ao reabrir ou trocar de Caso, a interface restaura essa preferência e prepara a geração correspondente aos dados. Um Caso ainda sem fonte pode habilitar a preferência antes de importar. A ativação só publica o motor depois que a construção termina; falhas ou cancelamentos preservam o funcionamento anterior. Casos enviados ao backend usam também um índice sobre seus eventos originais, com posições internas separadas dos IDs de evidência.

Alterações de parser, configuração de datas, campos derivados, catálogos ou fontes invalidam a geração associada. A próxima preparação compara a assinatura dos dados e dessas configurações para reutilizar um índice compatível ou construir outro.

## Mapa do código e gargalos encontrados

| Parte | Responsabilidade e impacto na escala |
| --- | --- |
| `model.rs`, `sources.rs`, `entities.rs` | O evento mantém os campos originais, texto bruto, identidade e colunas normalizadas. Os parsers reconhecem JSON, texto, CSV, syslog, Apache, firewall, EVTX e formatos personalizados. `FileIndex` guarda offsets/metadados sobre mmap; `event_at` materializa o evento e aplica catálogo, data, aliases, campos derivados e parâmetros de URL. Esse trabalho repetido era um custo importante; o modo novo também recupera sua representação persistida. |
| `index_cache.rs` | Reutiliza índices compactos de linhas por arquivo/configuração. O índice de linhas evita carregar a base inteira como objetos, mas ainda não localizava valores por postings. O novo cache é complementar. Há limpeza no início do aplicativo: manifestos sem uso por 30 dias e gerações órfãs com mais de um dia. |
| `query.rs`, `querylang.rs` | Operadores, linguagem booleana, ordenação, paginação, facetas e histogramas. A seleção compartilhada atende diferentes funcionalidades. O planejador novo gera somente condições necessárias e mantém esses matchers como autoridade. Cache de seleções limitado a 32 MiB; páginas ordenadas por data/nível no modo novo selecionam apenas o prefixo necessário, preservando desempate por posição. |
| `analysis.rs`, `distinct.rs`, `pivots.rs` | Séries, unidades numéricas, cruzamentos e rankings. Algumas análises fazem várias passagens para detectar unidades, grupos e limites. Rankings exatos já usam SQLite temporário ao atingir os limites de memória. O índice ajuda seleção e recuperação; não elimina todas essas passagens. |
| `workspace.rs`, `insights.rs`, `journeys.rs`, `discovery.rs` | Compartilham seleções e leitura por partes para resumos, períodos, timelines, jornadas e padrões. Perfis e descoberta têm amostragem explícita própria; o modo Big Data não altera sua política. |
| `detections.rs`, `evidence.rs`, `security_*`, `triage.rs`, `threats.rs`, `sigma.rs`, `attack.rs` | Correspondência de regras, correlação, evidências, participantes e conteúdo de segurança. Preservados os orçamentos e armazenamento em disco. Resultados completos não passam a ser amostras; filtros seletivos aproveitam a seleção indexada. As chaves de cache agora incluem configurações completas e conteúdo das evidências, evitando reaproveitar análises após alterações de campos ou datas. |
| `case_cache.rs`, `case_store.rs` | Transporte de uma versão do Caso e persistência em SQLite. As consultas agora compartilham `Arc<Vec<Event>>`, sem copiar todos os eventos a cada chamada. Índices de eventos preservados usam posições separadas dos IDs de evidência. |
| `lib.rs`, `operations.rs`, `mcp.rs`, `remote.rs` | Fronteiras Tauri/MCP, trabalho fora do thread da interface, cancelamento e importação de fontes locais/remotas. Ativação publica o índice completo; mudanças semânticas invalidam caches. A integração remota continua importando uma cópia local. |
| `frontend/app.js`, `workspace.js` e páginas especializadas | Seleção do Caso, transporte, filtros e visualizações. O botão persiste a preferência; consultas paralelas compartilham sincronização; fonte aberta e eventos preservados mostram prontidão separadamente. Os contratos das páginas permanecem iguais. |

Ainda existem custos fora da busca: salvar um Caso serializa seu conteúdo; a interface pode copiar eventos ao reconstruir a seleção de evidências; e resultados com muitas linhas/grupos consomem memória proporcional à saída. O modo não representa armazenamento ilimitado ou execução distribuída.

## Por que sistemas grandes são rápidos

Esses sistemas usam estruturas e planos diferentes para cargas diferentes. O tamanho da base, sozinho, não determina a latência: seletividade, organização física, memória disponível, armazenamento e distribuição do trabalho mudam o resultado.

| Sistema | Mecanismo principal | Aplicação ao Log Insight |
| --- | --- | --- |
| Hadoop / HDFS | Distribui blocos entre máquinas e aproxima a computação dos dados; privilegia vazão de processamento em lote. | Aproveitar processamento em partes e paralelismo local. Um cluster HDFS não traz automaticamente baixa latência a uma busca interativa no desktop. [Arquitetura oficial](https://hadoop.apache.org/docs/r3.1.4/hadoop-project-dist/hadoop-hdfs/HdfsDesign.html). |
| Greenplum | Um coordenador distribui o trabalho entre segmentos; o otimizador escolhe planos para consultas distribuídas. | Dividir operações independentes e combinar resultados. A infraestrutura de coordenador, segmentos e comunicação teria custo adicional na instalação local. [Arquitetura do fornecedor](https://www.vmware.com/docs/solution-brief-greenplum-database-on-vsphere), [GPORCA](https://knowledge.broadcom.com/external/article/295593/pivotal-query-optimizer-explained.html). |
| Elasticsearch | Índices invertidos encontram documentos por termos; `doc_values` armazenam colunas para ordenação e agregação; o campo `wildcard` usa n-grams e confirma os candidatos no valor original. | Essa última estratégia atende diretamente a busca de trechos de logs. Analisar palavras com um tokenizer comum mudaria a semântica de `contains`. [Tipos keyword/wildcard](https://www.elastic.co/guide/en/elasticsearch/reference/current/keyword.html/), [doc_values](https://www.elastic.co/guide/en/elasticsearch/reference/current/doc-values.html), [implementação de wildcard](https://www.elastic.co/blog/find-strings-within-strings-faster-with-the-new-elasticsearch-wildcard-field). |
| Apache Lucene | Segmentos imutáveis, listas de documentos por termo, colunas por documento e fusão de segmentos. | Buscar somente os documentos relacionados aos termos e manter IDs de evidência separados dos IDs internos que mudam durante fusões. [Arquitetura do índice](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/index/package-summary.html). |
| ClickHouse | Armazenamento por coluna, índice primário esparso e descarte de blocos; possui também índice invertido de texto, com tokenização configurável. | Projeções por coluna evitam materializar eventos para contagens simples. Para substrings, a escolha de n-grams continua relevante. As versões atuais recomendam o índice `text` em lugar dos antigos Bloom filters de texto. [Índices que descartam blocos](https://clickhouse.com/docs/concepts/features/performance/skip-indexes/skipping-indexes), [índice de texto atual](https://clickhouse.com/blog/full-text-search-ga-release). |
| DuckDB | Executa lotes de valores por coluna; zonemaps descartam grupos incompatíveis com filtros, especialmente quando os dados estão bem ordenados. É embarcado no processo. | Excelente referência para agregações amplas e projeções. Uma consulta arbitrária de substring ainda precisa de uma estrutura adequada de texto; zonemaps não substituem esse índice. [Arquitetura e objetivos](https://duckdb.org/why_duckdb), [zonemaps e ART](https://duckdb.org/docs/current/guides/performance/indexing). |
| Tantivy | Biblioteca Rust inspirada no Lucene, com índices invertidos compactos, segmentos imutáveis, arquivos mapeados em memória e colunas FAST. | Fornece esses mecanismos dentro do aplicativo, com suporte a Windows e Linux e sem instalar um servidor de busca. [Projeto](https://github.com/quickwit-oss/tantivy), [arquitetura](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md). |

A escolha desta implementação é o **Tantivy 0.26.2**, integrado ao backend Rust. Ele permite aplicar mecanismos de motores de busca ao fluxo atual, mantendo implantação local, cancelamento, evidências e interfaces existentes. Hadoop, Greenplum e Elasticsearch oferecem capacidades distribuídas que não são necessárias para construir este índice dentro de um único desktop. DuckDB e ClickHouse continuam sendo referências importantes para futuras operações analíticas por coluna.

## O que foi implementado

- **Índice invertido de trigrams:** sequências de três caracteres reduzem a lista de eventos possíveis para substrings, prefixos e regex com literais obrigatórios demonstráveis. O conjunto pesquisável contém texto bruto, mensagem parseada, colunas padrão, campos JSON, valores aninhados e entidades canônicas. A indexação inclui texto decodificado, como escapes JSON que não aparecem literalmente no arquivo.
- **Igualdade por valor completo:** um dicionário binário de chaves de 64 bits (prefixo de SHA-256) reúne as variantes exata, ASCII sem distinção de caixa e `trim`/minúsculas. Domínios e comprimentos separam valores globais e campos com delimitadores. São chaves de candidatos, não identidades de evidência ou prova de igualdade. Variantes idênticas compartilham termos; variantes diferentes e colisões só podem acrescentar candidatos, pois o matcher original confirma a igualdade. Fingerprints e identidades de evidência mantêm seus hashes completos.
- **Timestamp numérico:** campo indexado e coluna FAST para intervalos. O planejador arredonda limites candidatos de forma conservadora; o filtro original confirma os limites finais.
- **Colunas FAST de origem, código e nível:** filtros sobre esses campos confirmam os predicados originais em uma projeção, sem descomprimir o evento completo. Uma consulta reaproveita até 4.096 confirmações para tuplas idênticas de ordinais. Contagens agrupam ordinais compactos e decodificam cada valor uma vez por segmento, em vez de uma vez por evento.
- **Eventos parseados persistidos:** o documento original, seus campos e seus IDs são serializados e comprimidos com LZ4. O leitor recupera eventos para a página exibida, confirmação de filtros e operações que precisam de conteúdo completo.
- **Gerações imutáveis em disco:** cache em `big-data-v1` no diretório de configuração. Cada construção tem uma pasta própria. Um manifesto passa a apontar para ela somente depois do commit, conclusão das fusões e validação de schema, contagem e posições internas.

O n-gram é uma aproximação. Uma linha com `abc ... bcd` contém os trigrams de `abcd`, mas não contém essa substring. Por isso **todo candidato passa novamente pelo matcher original**. O índice nunca usa o complemento de uma aproximação para implementar `NOT`, `not_contains` ou outras negativas.

Expressões `AND` podem aproveitar as condições indexáveis e verificar as demais depois. Expressões `OR` só restringem candidatos quando todas as alternativas têm uma aproximação segura. Buscas muito curtas, regex sem literais adequados, CIDR, predicados complexos e condições não representadas seguem pelos caminhos existentes. Valores completos maiores que 16 KiB também mantêm a confirmação completa, com uma aproximação que não exclui linhas.

Estatísticas do dicionário podem provar que uma consulta é ampla: a interseção de termos tem pelo menos `max(0, N − Σ(N − frequência_do_termo))` candidatos. Se esse limite ultrapassa **`max(total / 4, 4096)`**, já se escolhe a varredura sem enumerar postings. Caso contrário, uma contagem confirma se vale a pena materializar IDs; acima do mesmo limite, mantém-se a varredura em partes. Isso evita alocar uma lista com quase toda a base e corresponde a 25% quando o limite de 4.096 não prevalece. Consultas sem restrições continuam favorecendo a leitura ampla e as projeções disponíveis.

Para fontes, a assinatura usa a identidade existente dos arquivos (caminho, tamanho, modificação e três amostras de 64 KiB), todos os metadados de linhas e configurações. Para Casos, inclui o conteúdo integral serializado de cada evento. Campos derivados condicionais incluem também conteúdo das configurações de detecção e regras locais/Sigma/ameaças. Mudanças nesses comandos invalidam o leitor e a interface prepara a nova assinatura; evidências já preservadas no Caso continuam na versão original.

O cache valida schema, quantidade de eventos, posições e tamanho/data dos arquivos persistidos. Alterações detectadas, arquivos ausentes ou truncados levam à reconstrução. As amostras dos logs e essa checagem rápida do índice não equivalem a recalcular o checksum de cada byte em toda reabertura; alterações/corrupção que preservem os metadados e as regiões amostradas não estão integralmente cobertas.

## Custos e limites

A primeira preparação tem custo de parsing, extração, escrita e fusão dos índices. Há espaço adicional em disco para postings, dicionários, colunas e eventos comprimidos. A proporção em relação aos logs varia conforme repetição, tamanho das linhas e cardinalidade. Desabilitar o modo retira o motor da fonte ativa; versões de Caso em cache podem reter seus leitores até a evicção. Gerações persistidas podem ser reutilizadas; o início do aplicativo remove manifestos sem uso por 30 dias e gerações órfãs com mais de um dia.

O orçamento do writer é **128 MiB**, dividido entre até quatro workers. Esse orçamento não é um teto para toda a memória do aplicativo: somam-se buffers de parsing/fusão, estruturas existentes, resultados e páginas mapeadas pelo sistema operacional. O índice de linhas já utiliza aproximadamente **32 bytes por evento** e a tabela de endereços do Tantivy acrescenta aproximadamente **8 bytes por evento**. Dez milhões de eventos representam cerca de 400 MB apenas nessas duas estruturas, antes dos demais custos. A seleção de candidatos é limitada pelo critério de custo acima, e as varreduras trabalham em partes de 8.192 linhas.

O cache de versões de Caso usa um orçamento estimado de **256 MiB para eventos** e mantém no máximo três versões. Leitores, endereços e mmap dos índices acrescentam custos. Versões antigas são descartadas ao exceder esse orçamento; a versão ativa pode ultrapassá-lo e continua completa. Essa política limita reaproveitamento de versões antigas, sem truncar eventos ou evidências. As colunas FAST são abertas uma vez por segmento e reutilizadas nas contagens.

O motor funciona numa máquina. Não acrescenta shards remotos, replicação ou execução distribuída. Regras de segurança, correlações temporais, agrupamentos de alta cardinalidade e agregações de todo o conjunto ainda podem exigir examinar muitos ou todos os eventos. O índice não transforma qualquer cálculo sobre a base em uma operação sublinear.

## Benchmark reproduzível

Na raiz do repositório, execute em PowerShell:

```powershell
cd src-tauri
$env:BIG_DATA_BENCH_EVENTS = "1000000"
cargo test --release --lib --no-default-features --locked benchmark_big_data -- --ignored --nocapture --test-threads=1
```

Use também 100.000 e 10.000.000 eventos, conforme RAM e espaço livre. Registre quantidade de linhas e bytes, tamanho do índice, preparação, reabertura e consultas. Separe consultas raras, frequentes, ausentes, igualdade, intervalos, filtros combinados e regex. As comparações devem exigir a mesma contagem e os mesmos eventos nos dois modos, limpar caches de resultado e distinguir reabertura de execução aquecida.

### Metodologia das medições locais

As consultas Elasticsearch foram medidas em **3 de outubro de 2026**, depois da compilação e antes do benchmark Rust, sem esses trabalhos pesados concorrentes. A máquina utiliza Intel Xeon E5-2660 v3 a 2,60 GHz, 10 núcleos/20 processadores lógicos e aproximadamente 48 GiB de RAM. O servidor rodou em Docker Engine 29.4.3 sobre WSL2/Linux, kernel `6.6.114.1-microsoft-standard-WSL2`; o Docker disponibilizava 20 processadores lógicos e aproximadamente 23,4 GiB de memória à VM. O cliente HTTP foi Node.js 24.15.0 no Windows. Portanto, seus tempos incluem o transporte local entre Windows e o servidor Linux.

Usou-se **Elasticsearch 9.5.4, Lucene 10.5.1**, confirmado pelo próprio servidor, com imagem `docker.elastic.co/elasticsearch/elasticsearch:9.5.4`. O contêiner temporário tinha limite de 2 GiB, heap JVM fixo de 1 GiB, um nó, um shard primário por índice e nenhuma réplica. A porta ficou restrita a `127.0.0.1:19200`, sem volume persistente. Ao terminar, o contêiner foi parado e removido. A imagem é distribuída pelo [registro oficial Elastic](https://www.docker.elastic.co/r/elasticsearch/elasticsearch).

O corpus sintético reproduz `big_data_tests::benchmark_big_data`: timestamp em milissegundos crescente, 1.000 origens, 5.000 usuários, ID de requisição único e uma mensagem rara a cada 10.000 eventos. A serialização e o volume de dados coincidiram com o benchmark Rust. Foram enviados lotes de 5.000 documentos, com um único produtor, refresh automático desligado durante a carga e refresh explícito depois dela. `message` foi mapeado como `wildcard`; origem, usuário, código, nível e ID de requisição como `keyword`; timestamp como `date`/`epoch_millis`; bytes como `long`. O `_source` foi mantido.

Cada combinação de consulta e tamanho teve **cinco execuções consecutivas**. A mediana é o terceiro valor ordenado; o P95 por posição mais próxima é o maior dos cinco valores. A primeira execução faz parte da amostra. Esse P95 resume uma amostra pequena; não representa um percentil de produção sob carga. Não houve limpeza forçada do cache de páginas do sistema operacional entre consultas. No Rust, o cache de seleções é limpo a cada execução. No Elasticsearch, `queries.cache.enabled` e `requests.cache.enable` foram desligados; as estatísticas finais confirmaram zero bytes, hits e misses nesses dois caches. O aquecimento interno do processo e das páginas continua possível.

Há uma diferença de trabalho entre as APIs: o Elasticsearch recebeu **HTTP `_count`**, que retorna uma contagem, enquanto `query::indexed_matches` no Rust materializa todos os IDs das linhas correspondentes e confirma os predicados originais. O tempo Elasticsearch inclui envio, recebimento e leitura da resposta JSON; o tempo Rust mede a chamada dentro do processo. Nenhum desses testes inclui renderização da interface, páginas ordenadas ou facetas. `track_total_hits` é uma opção de `_search`, não um parâmetro usado neste teste de contagem. [API oficial `_count`](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-count).

As consultas são equivalentes **neste corpus**, com contagens verificadas em todas as execuções. Os alvos das substrings aparecem somente nas mensagens; por isso consultar `message` no Elasticsearch produz as mesmas contagens das buscas da ferramenta, que também podem examinar texto bruto e outros valores conforme o filtro. A expressão `user:user-742 AND worker` combina igualdade do usuário com substring da mensagem. O intervalo temporal é inclusivo, do timestamp central até 100 ms depois, totalizando 101 eventos. A regex usa `rare-marker-7429.*failed`: as mensagens raras começam e terminam com esses literais, fazendo coincidir a busca Rust e o reconhecimento de campo completo do Lucene. Os dialetos de regex não são intercambiáveis para padrões arbitrários.

**O campo `message` tem apenas dois valores distintos neste corpus.** Embora o tipo `wildcard` reproduza o mecanismo de substring investigado, a documentação Elastic recomenda considerar `keyword` para valores curtos com baixa cardinalidade. Ela indica `wildcard` especialmente para mais de um milhão de valores distintos com padrões iniciados por curinga, ou valores acima de 32 KiB. Assim, esta medição não representa a melhor modelagem Elasticsearch para esse corpus de duas mensagens, nem uma comparação de todas as configurações disponíveis. Uma avaliação adicional deve medir `keyword` e logs com mensagens de alta cardinalidade. [Orientação oficial keyword/wildcard](https://www.elastic.co/docs/reference/elasticsearch/mapping-reference/keyword#wildcard-field-type).

Os esquemas persistidos também diferem: Elasticsearch indexa os campos mapeados e guarda o JSON original; o motor embarcado indexa o conjunto de valores pesquisáveis, entidades e eventos parseados, além do texto bruto. O espaço reportado abaixo é o `store` dos shards primários Elasticsearch, com `_source`, postings e colunas, sem translog. Não se deve interpretá-lo como comparação de dois índices com cobertura idêntica. A carga Elasticsearch ocorreu enquanto o Rust compilava; **seus tempos de preparação não são usados para comparar ingestão ou throughput**. As consultas temporizadas ocorreram depois, em uma janela isolada.

### Elasticsearch medido

| Consulta | Correspondências 100 mil / 1 milhão | Mediana 100 mil (ms) | P95 100 mil (ms) | Mediana 1 milhão (ms) | P95 1 milhão (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Substring `rare-marker-7429` | 10 / 100 | 19,45 | 98,21 | 12,69 | 13,98 |
| Substring ausente `absent-marker-963` | 0 / 0 | 12,50 | 17,15 | 10,12 | 12,72 |
| Origem exata `service-742` | 100 / 1.000 | 9,41 | 10,16 | 7,51 | 8,76 |
| `user:user-742 AND worker` | 20 / 200 | 11,63 | 17,92 | 9,88 | 12,04 |
| Regex `rare-marker-7429.*failed` | 10 / 100 | 13,50 | 15,71 | 10,54 | 12,05 |
| Intervalo temporal de 100 ms, inclusivo | 101 / 101 | 9,50 | 38,10 | 9,01 | 10,05 |
| Substring ampla `worker` | 99.990 / 999.900 | 44,69 | 78,64 | 85,71 | 143,02 |

| Eventos | JSONL de origem (bytes) | Índice primário durante as consultas (bytes) | Segmentos antes/depois |
| ---: | ---: | ---: | ---: |
| 100.000 | 18.356.770 | 10.050.934 | 1 / 1 |
| 1.000.000 | 183.567.700 | 98.908.697 | 4 / 4 |

Não havia fusões ativas nas verificações imediatamente anteriores e posteriores às consultas; o tamanho de cada índice permaneceu igual nesses dois pontos. Os 14 pares consulta/tamanho tiveram contagens corretas nas cinco execuções, totalizando 70 verificações. Amostras individuais, consultas JSON, metadados e estatísticas estão no relatório local `output/big-data/elasticsearch.json`, ignorado pelo Git. O produtor e o verificador estão em [scripts/benchmark-elasticsearch.mjs](../scripts/benchmark-elasticsearch.mjs).

Para repetir a medição com a mesma imagem e o mesmo contêiner descartável, a carga e as consultas podem ser separadas:

```powershell
node scripts/benchmark-elasticsearch.mjs --prepare-only --events=100000,1000000
# Aguarde o término de compilações e outras cargas pesadas.
node scripts/benchmark-elasticsearch.mjs --queries-only --events=100000,1000000
docker stop loginsight-big-data-bench
```

O cabeçalho do script contém o comando de criação do contêiner. Os índices devem permanecer disponíveis entre as duas etapas; `--queries-only` confere sua origem e quantidade de documentos antes de medir.

### Resultado final do motor embarcado

Medição concluída em **3 de outubro de 2026**, com Tantivy 0.26.2 e schema de índice **3**, no mesmo host descrito acima. O backend foi compilado em `release`, com Rust 1.95.0, e executado nativamente no Kali Linux/WSL2. Os arquivos sintéticos e índices ficaram no filesystem Linux temporário. A execução temporizada usou diretamente o binário de testes já compilado; compiladores, testes de navegador e contêiner Elasticsearch estavam encerrados.

Cada consulta abaixo compara o caminho anterior e o modo Big Data na mesma fonte. O cache de seleção foi limpo em cada execução; o cache de páginas permaneceu disponível. O teste exige igualdade dos vetores completos de posições retornados. O Explorer compara o snapshot completo, incluindo os primeiros 100 eventos ordenados por timestamp, total, facetas de origem/código e estatísticas. A ordem das entradas empatadas de níveis é normalizada apenas na comparação das estatísticas.

**Medianas, em milissegundos:**

| Consulta | Normal 100 mil | Big Data 100 mil | Ganho | Normal 1 milhão | Big Data 1 milhão | Ganho |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Substring rara | 6,66 | 0,657 | 10,13× | 36,72 | 1,432 | 25,64× |
| Substring ausente | 5,05 | 0,418 | 12,08× | 28,03 | 0,719 | 38,98× |
| Origem exata | 91,17 | 0,331 | 275,83× | 701,03 | 1,029 | 681,16× |
| Usuário + substring | 4,84 | 0,506 | 9,57× | 35,23 | 1,864 | 18,90× |
| Regex com literais obrigatórios | 87,89 | 0,577 | 152,24× | 836,41 | 1,511 | 553,64× |
| Intervalo temporal | 1,68 | 1,781 | 0,94× | 10,59 | 2,188 | 4,84× |
| Substring ampla | 3,67 | 3,921 | 0,94× | 35,12 | 32,046 | 1,10× |
| Explorer, primeira página e facetas | 1.603,94 | 26,904 | 59,62× | 15.723,66 | 297,928 | 52,78× |

**P95, em milissegundos** — máximo das cinco amostras, conforme a metodologia:

| Consulta | Normal 100 mil | Big Data 100 mil | Normal 1 milhão | Big Data 1 milhão |
| --- | ---: | ---: | ---: | ---: |
| Substring rara | 16,88 | 2,59 | 51,36 | 4,12 |
| Substring ausente | 5,39 | 0,82 | 30,19 | 0,93 |
| Origem exata | 92,22 | 0,54 | 742,76 | 1,87 |
| Usuário + substring | 7,70 | 1,06 | 36,33 | 4,73 |
| Regex com literais obrigatórios | 93,47 | 1,15 | 864,41 | 2,04 |
| Intervalo temporal | 1,75 | 2,12 | 11,97 | 2,41 |
| Substring ampla | 4,01 | 4,05 | 35,94 | 33,68 |

O benchmark do Explorer registrou a mediana, sem emitir o P95. Em 100 mil eventos, o intervalo temporal teve regressão de aproximadamente **0,10 ms** e a substring ampla de **0,25 ms**. Em 1 milhão, a busca ampla melhorou somente 1,10×: quase todas as linhas correspondem e a estratégia escolhe a varredura existente. Os maiores ganhos vieram de filtros seletivos e de eliminar parsing e decodificação repetidos nas facetas; não se deve aplicar o fator de uma consulta a todas as funcionalidades.

| Eventos | JSONL (bytes) | Índice adicional (bytes) | Índice de linhas inicial (s) | Preparação Big Data (s) | Reabertura do índice pronto (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 100.000 | 18.356.770 | 41.497.322 | 0,612 | 17,881 | 23,294 |
| 1.000.000 | 183.567.700 | 426.436.770 | 5,897 | 126,967 | 215,587 |

A preparação Big Data é adicional à indexação inicial de linhas. A reabertura mede o leitor Big Data com `FileIndex` já disponível; não mede a abertura completa do aplicativo ou de um Caso. A reutilização do manifesto foi confirmada nos dois volumes. O índice adicional ocupou aproximadamente 2,26× e 2,32× o tamanho do JSONL, respectivamente, neste corpus. O volume de eventos parseados, valores e cardinalidade muda essa proporção.

O processo de benchmark teve pico de RSS de **607.688 KiB (593,45 MiB)**, sem swap, conforme `/usr/bin/time -v`. Esse número abrange o benchmark nativo, incluindo preparação e consultas; não inclui a interface, a VM inteira ou Casos carregados separadamente. A execução completa dos dois volumes levou **4 min 13 s**, incluindo geração dos dados, caminhos normais, preparação, consultas e comparações. Foram validadas as 16 comparações de consultas/Explorer. Os relatórios locais `output/big-data/native-linux.log` e `native-linux.json` estão ignorados pelo Git.

No milhão de eventos, as sete consultas indexadas/varridas ficaram entre **0,719 e 32,046 ms** de mediana; as contagens HTTP Elasticsearch deste teste ficaram entre **7,511 e 85,711 ms**. Esses números precisam ser lidos com as diferenças de API, transporte, schema e cardinalidade descritas acima. Não demonstram superioridade universal, nem a melhor configuração possível de Elasticsearch.

**Limite de escala efetivamente medido:** 100 mil e 1 milhão de eventos, até 183,6 MB de JSONL. O comando permite volumes maiores, mas 10 milhões de eventos, logs reais de alta cardinalidade, concorrência de usuários e bases de muitos gigabytes não foram medidos nesta execução.

### Validação de funcionamento

- `cargo check --tests --no-default-features --locked` e `cargo check --locked` passaram no Windows.
- A suíte nativa Linux em `release` passou com **178 testes, zero falhas e cinco testes ignorados**. O benchmark Big Data foi executado separadamente e passou. A cobertura inclui filtros e linguagem booleana, Unicode, regex, agregações, paginação, identidade de eventos, formatos de entrada, corrupção/reutilização de cache e cancelamento.
- `scripts/preview/test-big-data.mjs` passou sem erros de página, usando navegador real no preview com respostas Tauri simuladas. Ele verifica ativação por Caso, persistência, troca de Caso, paginação/filtros preservados, falhas, cancelamento, solicitações concorrentes, Casos preservados e atualização de regras via interface/MCP, inclusive com o modo desligado. Não substitui um teste completo da interface conectada ao executável nativo.
- Os testes existentes de contexto/escopo do Workspace e `npm run prepare:frontend` passaram.

O Windows bloqueou a execução do binário de testes com erro 225; por isso os testes executáveis e tempos acima foram obtidos no Linux/WSL2. A compilação Windows foi verificada, mas não há medição nativa Windows ou de renderização da interface nestes resultados.

Não há uma garantia universal de superar Elasticsearch. A própria documentação destaca a influência de queries, concorrência, shards, modelagem, cache e hardware. O compromisso verificável é preservar os resultados e medir o ganho de cada carga relevante. [Orientações oficiais de desempenho](https://www.elastic.co/guide/en/elasticsearch/reference/current/tune-for-search-speed.html).

## Revisão adicional de performance

A revisão seguinte ampliou os cenários para filtros que correspondem a quase toda a base, filtros negativos e ordenação de páginas por origem/código. Essas operações não estavam representadas pelas consultas seletivas do primeiro relatório. A comparação usa um executável preservado antes das alterações e outro compilado depois, ambos em `release`, com o mesmo corpus determinístico de um milhão de eventos e sem compilação ou outros benchmarks em paralelo.

As mudanças desta revisão são:

- Filtros categóricos amplos usam as colunas FAST mesmo quando os postings deixam de ser seletivos. As decisões anteriores por metadados permanecem; a projeção substitui a materialização de um evento quando ele seria necessário para confirmar o filtro. Reutilização de confirmações continua limitada a 4.096 combinações por parte da varredura.
- A ordenação por `source` e `code` calcula as chaves numéricas/textuais por valor distinto, reúne chaves equivalentes entre segmentos e localiza a faixa da página por contagens. Só os eventos solicitados são descomprimidos. Números com unidades, Unicode e desempates por posição usam a comparação existente. Páginas profundas dispensam a ordenação de todo o prefixo.
- A preparação do índice usa até quatro workers próprios, além dos workers do Tantivy, conforme os processadores disponíveis. Cada worker prepara até 256 documentos ou um alvo de 2 MiB por rodada. Um documento indivisível pode ultrapassar esse alvo e permanece completo. Os lotes voltam à ordem das posições antes da escrita; cancelamento e publicação somente após conclusão são preservados. Entradas pequenas e máquinas sem processadores suficientes mantêm o caminho sequencial.
- Contagens distintas que excedem o orçamento de memória reutilizam a instrução preparada do SQLite, em vez de recompilar o mesmo `INSERT` para cada valor.
- Intervalos temporais com os dois limites e amplitude de até 4.096 ms usam o índice invertido. Há no máximo 4.097 valores inteiros distintos nesse intervalo por segmento; o plano percorre todos eles, sem truncar termos ou resultados. Intervalos maiores ou abertos continuam usando o plano FAST. Isso evita que uma janela pequena precise percorrer colunas de segmentos grandes com faixas temporais sobrepostas.

O alvo dos novos buffers de preparação não limita toda a memória do processo: ainda existem o evento em construção, as filas internas do writer, fusões e páginas de arquivos. A ordenação categórica continua sendo `O(N + K)` em memória: um identificador de chave de oito bytes por correspondência, mais estruturas dos valores distintos por segmento. Cardinalidade próxima de `N` reduz a vantagem dessa organização. O schema permanece compatível com os índices já preparados.

O benchmark ampliado pode ser reproduzido acrescentando `$env:BIG_DATA_REVIEW_CASES = "1"` ao comando anterior. A contagem distinta tem um teste separado:

```powershell
cargo test --release --lib --no-default-features --locked benchmark_counter_spill -- --ignored --nocapture --test-threads=1
```

Os testes de consultas exigem igualdade das posições completas; os de ordenação comparam os eventos completos da página, total e identidade. As cinco execuções por consulta geram mediana e P95 por posição mais próxima. A preparação é medida uma vez por execução do benchmark; seu tempo não é uma mediana de cinco construções.

A revisão também protegeu consultas cujo atalho antigo sobre os metadados trata espaços de forma diferente do matcher de eventos: buscas de trechos com espaços externos e filtros de código vazio preservam esse caminho, inclusive quando combinados com outro filtro seletivo. Essa proteção mantém compatibilidade, mesmo quando é necessário abrir mão de uma seleção indexada. Há testes de paridade sobre JSONL e snapshots.

Para comparar os planos temporais no **mesmo índice**, `BIG_DATA_BENCH_KEEP=1` preserva o diretório temporário ao terminar o benchmark e imprime `BIG_DATA_RETAINED` com o caminho. Aponte `BIG_DATA_RANGE_BENCH_MANIFEST` para o arquivo JSON do manifesto dentro de `cache` e execute:

```powershell
cargo test --release --lib --no-default-features --locked benchmark_timestamp_range_strategies -- --ignored --nocapture --test-threads=1
```

Esse diagnóstico alterna sete execuções de FAST e postings sobre o mesmo leitor, executando contagem e materialização das posições e exigindo igualdade dos 101 resultados. Sem manifesto, cria um conjunto de diagnóstico que contém apenas timestamps e posições; esse conjunto não representa o custo de indexação dos logs completos. O benchmark principal continua removendo seus arquivos automaticamente quando `BIG_DATA_BENCH_KEEP` não é `1`.

### Resultados desta revisão

Medição concluída em **3 de outubro de 2026**, no mesmo Xeon E5-2660 v3, com 20 processadores lógicos, em Linux/WSL2. O corpus contém **1.000.000 de eventos e 183.567.700 bytes de JSONL**. A coluna “Antes” já usa o modo Big Data: os ganhos abaixo são adicionais aos da primeira implementação. Os dois executáveis geraram seus próprios índices; a preparação e as fusões resultaram em 14 segmentos antes e um depois, nos cenários ampliados. Portanto, a comparação mede o conjunto das mudanças, incluindo a organização resultante do índice, e não isola cada alteração de código. Não houve limpeza do cache de páginas; o cache de seleções foi limpo entre execuções.

| Operação | Antes: mediana (ms) | Depois: mediana (ms) | Antes/depois | P95 antes → depois (ms) |
| --- | ---: | ---: | ---: | ---: |
| Substring rara | 1,256 | 1,394 | 0,90× | 3,226 → 5,532 |
| Substring ausente | 0,603 | 0,389 | 1,55× | 0,818 → 0,621 |
| Origem exata seletiva | 1,598 | 1,323 | 1,21× | 2,293 → 2,456 |
| Usuário + substring | 1,854 | 3,228 | 0,57× | 4,620 → 4,596 |
| Regex com literais obrigatórios | 1,613 | 2,847 | 0,57× | 1,894 → 5,099 |
| Intervalo temporal de 100 ms | 2,283 | 1,047 | 2,18× | 2,548 → 1,469 |
| Substring ampla | 33,103 | 28,599 | 1,16× | 43,170 → 32,226 |
| Explorer, primeira página e facetas | 287,946 | 211,463 | 1,36× | não registrado |
| Prefixo de origem, 1 milhão de resultados | 724,941 | 124,602 | 5,82× | 877,169 → 129,727 |
| Código exato, 990 mil resultados | 712,568 | 23,121 | 30,82× | 884,836 → 36,246 |
| Exclusão de origem, 999 mil resultados | 790,421 | 110,965 | 7,12× | 872,343 → 122,654 |
| Ordenação por origem, primeiros 100 eventos | 13.012,154 | 118,991 | 109,35× | 13.307,429 → 126,670 |
| Ordenação por código, primeiros 100 eventos | 9.021,466 | 104,943 | 85,97× | 9.577,397 → 141,672 |
| Ordenação por origem, 100 eventos após posição 500 mil | 13.184,292 | 113,319 | 116,35× | 13.273,691 → 137,618 |

Valores inferiores a 1× indicam regressão. Houve aumento de **0,139 ms** na substring rara, **1,374 ms** na consulta de usuário + substring e **1,234 ms** na regex. Esses resultados permanecem registrados; não foram descartados como ruído. Cinco amostras e uma construção por versão não permitem garantir o mesmo ganho ou explicar isoladamente essas regressões em outras cargas. O P95 é o máximo das cinco amostras, sem representar um percentil de produção.

| Preparação e consumo | Antes | Depois |
| --- | ---: | ---: |
| Preparação Big Data | 125,27 s | 93,27 s |
| Indexação inicial de linhas, separada da preparação | 5,81 s | 6,17 s |
| Reabertura do índice já preparado | 268,35 ms | 240,03 ms |
| Índice adicional no disco | 427.954.632 bytes | 430.398.964 bytes |
| Pico RSS do processo de benchmark | 842,00 MiB | 952,11 MiB |

A preparação final levou **25,5% menos tempo**, com aumento de **13,1% no pico de memória do processo completo do benchmark** e aproximadamente 0,6% no índice. O RSS inclui preparação, consultas e comparações, sem a interface ou a VM inteira; não atribui sozinho esse aumento a um buffer específico. Uma construção intermediária levou 89,27 s, mas a tabela usa a versão final medida. O paralelismo melhora o tempo de preparação com um custo de memória que deve ser considerado em máquinas menores.

No teste isolado de contagens distintas com SQLite, a mediana de cinco execuções caiu de **271,11 para 161,26 ms** para 100 mil valores únicos (**1,68×**) e de **2.583,37 para 1.117,94 ms** para um milhão de valores com 50 mil distintos (**2,31×**). A contagem final foi conferida contra um conjunto exato.

O diagnóstico temporal alternou sete execuções sobre **o mesmo índice final**, de um segmento. Contar e materializar os mesmos 101 resultados levou **10,312 ms de mediana pelo plano FAST e 0,618 ms pelo índice invertido**; os P95 foram 17,599 e 0,825 ms. Esse teste isola a escolha de plano, enquanto a tabela principal mede a consulta completa. O limite de 4.097 timestamps distintos limita a enumeração de termos, não o número de documentos associados: janelas pequenas com muitos eventos no mesmo milissegundo ainda precisam de avaliação própria.

As **14 comparações de consultas/páginas tiveram paridade**. A suíte Rust final passou em Linux/WSL2 com **202 testes, zero falhas e oito testes/benchmarks ignorados**, executando separadamente os benchmarks usados neste relatório. `cargo check --tests --examples --locked` passou no Windows. O modal de recursos passou no navegador com respostas Tauri simuladas, o bundle foi preparado e o coletor foi medido nativamente no Windows. Isso não equivale a medir toda a interface conectada ao backend nativo.

Relatórios brutos locais, ignorados pelo Git: `output/performance-before.log`, `output/performance-after.log`, `output/performance-review.json`, `output/performance-timestamp-plans.log` e `output/performance-counter-{before,after}.log`. Os números desta revisão continuam limitados a esse corpus sintético de 183,6 MB; não demonstram comportamento com dezenas de gigabytes, concorrência ou mensagens de alta cardinalidade. Não houve nova execução comparativa de Elasticsearch nesta rodada.

### Oportunidades que continuam abertas

Ainda não há evidência de que o aplicativo atingiu o máximo possível. Séries temporais com categorias podem fazer várias passagens pelos eventos; histogramas e seleções mantêm vetores proporcionais ao número de correspondências; exportação JSONL sem mascaramento pode evitar representações intermediárias; e salvar Casos ainda copia/serializa conteúdo que não mudou. Essas oportunidades exigem medições próprias e atenção à persistência e à completude dos resultados. O compartilhamento de consultas simultâneas dos gráficos já existe em `frontend/tasks.js` e foi confirmado no navegador, sem necessidade de outra camada de cache.

O gerenciador de recursos teve seu custo revisado separadamente, incluindo contagem de ações sem criar snapshots detalhados e reutilização do formatador numérico. Os procedimentos e limites estão em [resource-manager.md](resource-manager.md).

## Rodada cautelosa: menos trabalho repetido e proteção dos resultados

Esta rodada, também em 3 de outubro de 2026, usa como referência a versão ao final da revisão acima. O foco foi reduzir trabalho comprovadamente repetido e corrigir falhas reproduzíveis, com alterações pequenas e sem mudar o schema do índice ou acrescentar dependências.

### Correções de confiabilidade

- **Salvamento de Casos:** cada lote e seus solicitantes ficam associados à lista de Casos que originou a alteração. Uma lista substituída por recarga/importação não pode emprestar sua revisão a um snapshot antigo na fila. Respostas atrasadas também não atualizam a revisão da nova lista. O solicitante recebe `false` e uma mensagem quando sua versão foi substituída; falhas ao serializar passam a encerrar a espera com erro. O salvamento sequencial da mesma lista continua avançando a revisão normalmente.
- **Atualização manual dos resumos:** o botão “Atualizar” força uma consulta ao backend. Entrar novamente na mesma visão continua usando o cache. O teste conta as chamadas, verifica a retirada dos totais após erro e a recuperação com nova tentativa.
- **Cancelamento do histograma:** a agregação agora verifica o cancelamento a cada bloco de 2.048 entradas. O teste cancela na centésima leitura: a versão anterior percorreu as 100 mil entradas; a nova interrompe dentro do primeiro bloco. O invólucro da operação devolve erro de cancelamento, sem publicar estatísticas parciais.

O teste determinístico [test-case-autosave.mjs](../scripts/test-case-autosave.mjs) usa o código real de salvamento e substituição da lista com transporte simulado. A versão anterior falhou em seis dos nove cenários; a corrigida passou nos nove. A cobertura inclui substituição antes do debounce, enquanto o lote espera na fila e durante a requisição, revisões sequenciais da mesma lista, agrupamento de edições, falhas, cancelamento e nova tentativa.

### Otimizações mantidas

O histograma percorre duas vezes as projeções emprestadas da seleção existente e deixa de construir um vetor adicional com todos os timestamps. Preserva datas ausentes, níveis personalizados, limites dos buckets e aritmética saturada. A memória adicional dessa etapa depende dos até 60 buckets e dos níveis distintos; as seleções e estruturas criadas pelos chamadores continuam existindo.

Nas séries temporais, inferência de unidade, ranking das seis categorias e limites de tempo passam a compartilhar uma leitura. A segunda leitura acumula os resultados. Os primeiros 500 valores numéricos válidos, desempates, linhas sem data, unidades incompatíveis e armazenamento temporário do ranking mantêm as regras anteriores. Não são retidos eventos adicionais. Sem categorias nem inferência, o caminho original de limites permanece: a primeira candidata acrescentava trabalho a esse caso e foi restringida após a medição. Gráficos de ranking (`terms`) também preservam seu caminho anterior.

### Mudança avaliada e descartada

Foi testada a escrita direta do `Value` JSON no `BufWriter`, eliminando a string final de cada registro. Os quatro conjuntos exportados tiveram bytes e SHA-256 idênticos, incluindo máscaras. Porém, a mediana de um milhão de registros pequenos passou de 11.627 para 11.254 ms, enquanto a de oito mil registros com 32 KiB passou de 2.271 para 2.332 ms. O ganho foi pequeno e inconsistente; **a alteração de produção foi revertida**. O serializador, o mascaramento e a publicação atômica continuam no caminho anterior.

Ficaram os testes de regressão de exportação: JSONL byte a byte, Unicode, escapes, números extremos, objetos/arrays, valores nulos, máscaras recursivas, integridade dos eventos originais, CSV com campos tardios e proteção de fórmulas, erros parciais de escrita e cancelamento. O benchmark continua disponível para avaliar futuras propostas.

### Medições e validação desta rodada

Executáveis `release` antes/depois, no mesmo host Linux/WSL2, sem compilações ou testes de navegador concorrentes às medições. Os testes de séries usam eventos sintéticos já carregados e contam cada materialização: **não medem leitura de disco, parsing, descompressão Tantivy ou renderização**. São cinco amostras por cenário; os relatórios guardam mediana, P95 (máximo da amostra) e SHA-256 da resposta completa. As oito combinações de cenário/volume produziram respostas idênticas antes e depois, também na repetição de confirmação.

| Série temporal | Antes, 100 mil (ms) | Depois, 100 mil (ms) | Antes, 1 milhão (ms) | Depois, 1 milhão (ms) | Leituras antes → depois, 1 milhão |
| --- | ---: | ---: | ---: | ---: | ---: |
| Contagem por categoria | 153,53 | 126,95 | 1.491,22 | 1.146,87 | 3.000.000 → 2.000.000 |
| Média com unidade automática e categoria | 136,20 | 122,59 | 1.644,82 | 1.286,16 | 3.000.501 → 2.000.000 |
| Campo ausente com unidade automática e categoria | 213,56 | 115,07 | 2.196,30 | 1.164,92 | 4.000.000 → 2.000.000 |
| Controle: contagem sem categoria/inferência | 97,69 | 107,23 | 1.002,77 | 1.115,54 | 2.000.000 → 2.000.000 |

Foi feita uma segunda comparação em sequência dos mesmos executáveis porque o controle permaneceu mais lento. Em um milhão de eventos, contagem por categoria passou de **1.512,46 para 1.212,54 ms**, média de **1.726,03 para 1.318,82 ms** e campo ausente de **2.120,50 para 1.277,72 ms**: reduções de aproximadamente 20%, 24% e 40%. O controle passou de **1.029,69 para 1.106,94 ms**, uma regressão medida de **7,5%**. Em 100 mil, esse controle passou de 104,52 para 96,50 ms. Não há base para prometer ausência de regressões nem atribuir toda a diferença a ruído. O caminho simples mantém seu algoritmo anterior; a diferença de tempo no volume maior permanece sem causa isolada e merece perfilamento próprio. Os ganhos foram consistentes nos cenários que eliminam materializações.

O histograma foi medido separadamente, com **10 milhões de timestamps**, uma coluna de entrada de 80 MB, sete amostras e `/usr/bin/time -v`:

| Histograma isolado | Antes | Depois |
| --- | ---: | ---: |
| Mediana | 638,04 ms | 559,31 ms |
| P95 da amostra | 678,94 ms | 663,90 ms |
| Pico de memória residente do processo | 208.984 KiB / 204,09 MiB | 130.560 KiB / 127,50 MiB |

A versão final reduziu a mediana em aproximadamente **12,3%** e o pico de memória em **37,5%** nesse diagnóstico. Uma execução candidata anterior mediu 489,44 ms e o mesmo pico de memória; a tabela usa a medição final, sem escolher o melhor tempo. O consumo inclui a coluna de entrada e o executável do teste. **Não equivale a processar dez milhões de logs completos nem a reduzir a memória de todo o aplicativo nessa proporção.**

A versão final passou em **210 testes Rust, zero falhas e 11 testes/benchmarks ignorados**, com os benchmarks pertinentes executados separadamente. `cargo check --tests --examples --locked` passou no Windows. Os nove cenários de autosave e os testes de contexto do Workspace e Workbench passaram; os dois últimos usam navegador real com respostas Tauri simuladas e não registraram erros de página. O bundle foi preparado e seus arquivos alterados conferem com as fontes. A interface completa conectada ao backend nativo não foi exercitada nesta rodada.

Para repetir, execute na raiz `node scripts/test-case-autosave.mjs`. A partir de `src-tauri`:

```powershell
$env:ANALYSIS_BENCH_EVENTS = "100000,1000000"
cargo test --release --lib --no-default-features --locked benchmark_series_preflight_materialization -- --ignored --nocapture --test-threads=1
$env:HISTOGRAM_BENCH_EVENTS = "10000000"
cargo test --release --lib --no-default-features --locked benchmark_histogram_memory -- --ignored --nocapture --test-threads=1
cargo test --release --lib --no-default-features --locked benchmark_jsonl_export -- --ignored --nocapture --test-threads=1
```

Para tempos isolados e RSS, meça o executável já compilado, sem incluir a compilação. Os relatórios locais estão em `output/cautious-performance-review.json`, `output/cautious-{histogram,series}-{before,after}.log`, `output/cautious-series-confirm-{before,after}.log`, `output/cautious-export-before.log`, `output/cautious-export-candidate.log` e `output/case-autosave-regression.json`, ignorados pelo Git. A medição de exportação candidata está explicitamente marcada como **não mantida** no relatório; executar seu benchmark no código atual mede novamente o serializador original.

### Limites que permanecem

As séries continuam lineares e exigem duas leituras quando há dados com timestamp. A seleção de eventos ainda pode ocupar memória proporcional ao resultado, e as fontes indexadas podem manter outras estruturas intermediárias antes do histograma. Os Casos ainda são serializados por inteiro: em armazenamento muito lento, lotes de autosave podem se acumular enquanto uma gravação está em andamento. Esta rodada protege a associação entre snapshot e revisão, mas não redesenha essa fila. Esses pontos exigem medições e mudanças próprias.
