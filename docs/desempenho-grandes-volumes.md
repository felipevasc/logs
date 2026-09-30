# Desempenho em grandes volumes

O alvo de validação desta melhoria é **50 milhões de registros**. Os testes pequenos demonstram correção e contratos de interação; não comprovam latência, consumo de memória ou estabilidade nessa escala. Nenhum ganho percentual é presumido.

## Executar os testes de regressão

```sh
npm ci
npm run check
npm ci --prefix scripts/preview
npx --prefix scripts/preview playwright install chromium
npm run test:preview
npm run prepare:frontend
cargo test --manifest-path src-tauri/Cargo.toml --release --locked --tests -- --test-threads=1
```

O preview usa dados e operações nativas simulados. Ele testa navegação, persistência, agendamento e renderização; os testes Rust exercitam os parsers, os índices reais e a paridade entre os motores. `PLAYWRIGHT_EXECUTABLE_PATH` ou `PLAYWRIGHT_CHANNEL` permitem escolher um navegador local explicitamente. Sem essas variáveis, o teste usa a versão do Chromium fixada pelo Playwright.

`nginx_acceptance` cobre fixtures pequenas de acesso combinado, o mesmo arquivo em gzip e um mapeamento JSON explícito. Compara filtros e páginas com o motor de linhas e reabre cada fonte em outro processo, verificando que os artefatos persistidos não foram reescritos. Esses formatos representativos não substituem a validação do `log_format` real do usuário.

O workflow `Pull request checks` roda nos PRs para `main`. Ele não publica instaladores, releases ou atualizações.

## Dados reproduzíveis, sem alocar o arquivo todo

O gerador grava lotes limitados por registros e bytes e recusa sobrescrever arquivos existentes. Exige tamanho explícito; nunca gera 50M por padrão. Reserve espaço para a fonte, índice de linhas, DuckDB, Tantivy e trabalho temporário, além do próprio arquivo de teste.

```sh
mkdir -p output/bench
node scripts/bench/generate-logs.mjs --rows 1000000 --output output/bench/1m.jsonl
# Só depois de medir espaço/RAM e concluir 1M/10M:
node scripts/bench/generate-logs.mjs --rows 50000000 --output output/bench/50m.jsonl
```

O arquivo `.manifest.json` ao lado registra versão do gerador, seed, número de registros, bytes e SHA-256. A mesma seed e parâmetros reproduzem o conteúdo. O conjunto contém horários ordenados com empates, 32 fontes, identificadores hexadecimais únicos em ordem pseudoaleatória reproduzível, campos aninhados, valores de ordenação mistos e textos raros/comuns. O manifesto inclui um identificador conhecido da linha central para busca pontual; evite consultar só o menor ID, que pode favorecer artificialmente a poda por zonemaps. `--id-order ordered` permite comparar explicitamente o caso favorável de IDs ordenados. `--padding 1024` permite avaliar registros mais largos. Teste também formatos reais representativos, vários arquivos, timestamps configurados e alta cardinalidade; um JSONL sintético não substitui esses casos.

## Medição do backend

Execute em build `--release`, com os mesmos recursos e versões para comparar commits. Use um diretório de benchmark isolado. Não apague índices pessoais nem cache do sistema para simular uma execução fria.

```sh
LOGINSIGHT_BENCH_FILE="$PWD/output/bench/1m.jsonl" \
LOGINSIGHT_BENCH_ROWS=1000000 \
LOGINSIGHT_BENCH_DIR="$PWD/output/bench/candidate-1m" \
LOGINSIGHT_BENCH_CACHE_STATE=cold-application-cache \
LOGINSIGHT_BENCH_REPEATS=20 \
cargo test --manifest-path src-tauri/Cargo.toml --release --locked \
  --test engine_interaction -- --ignored --nocapture --test-threads=1 \
  2>&1 | tee output/bench/candidate-1m.log
```

Linhas prefixadas por `BENCH` contêm JSON com tempos separados para abertura do índice de linhas, preparação, primeira página, próxima página por cursor, contagem exata e estatísticas. O primeiro ensaio de cada filtro inclui inicialização de suas estruturas; as repetições medem reaproveitamento. As estatísticas não fazem parte do tempo de primeira página. Esse teste mede backend, não IPC/renderização ou responsividade da janela. Com `LOGINSIGHT_BENCH_EXPLAIN=1`, registra também SQL e `EXPLAIN ANALYZE` após as amostras de latência; confira linhas lidas, poda de segmentos e planos antes de atribuir ganhos a índices.

Registre junto: SHA do commit, sistema, processador, núcleos, RAM, SSD, versões de Rust/DuckDB, manifesto do arquivo e configuração de recursos. Meça pico de RSS, CPU, I/O, uso temporário e tamanho recursivo dos índices com ferramentas do sistema; não some apenas os arquivos do primeiro nível, pois o índice textual contém subdiretórios.

Separe os estados de cache:

1. Preparação inicial em diretório de índices vazio
2. Reabertura em **novo processo**, com índices persistidos
3. Filtro novo com processo aquecido
4. Repetição do mesmo filtro com cache da aplicação

“Frio” deve informar se o cache de arquivos do sistema estava aquecido. Limpar o cache de resultados da aplicação não limpa DuckDB nem o cache do sistema operacional. Para a comparação do commit anterior, use um checkout e diretório de índices próprios, com a mesma fonte e sem jobs concorrentes.

## Validação de interação e correção

Na aplicação nativa, anote p50/p95 do gesto até a primeira renderização e, separadamente, até estatísticas completas. Não use apenas a duração do SQL.

- Filtrar um identificador exato raro, período curto, nível/fonte, texto comum/raro e campo aninhado
- Trocar ordenação e navegar próximo/anterior com empates, valores vazios e múltiplas fontes; comparar IDs e ordem com o motor de linhas em fixtures menores
- Adicionar/remover 5–30 colunas existentes, inclusive com casos grandes abertos; isso deve continuar sendo local
- Modificar campos derivados e continuar consultas que só dependem de campos-base; comparar resultados derivados, busca livre e filtros dependentes após preparação
- Interromper carregamento durante leitura, construção e finalização; reiniciar e conferir segmentos reaproveitados, identidade da fonte/configuração, total e resultados
- Alterar a fonte/configuração entre execuções; caches incompatíveis devem ser rejeitados
- Cancelar uma análise lenta enquanto outra consulta continua; repetir filtros rapidamente e confirmar que a intenção mais recente prevalece
- Exercitar falha de disco/permissão e modo de linhas: o estado degradado e sua causa precisam aparecer, sem percentuais inventados

Metas propostas para escolher hardware e investigar regressões, **não resultados medidos**: adicionar coluna existente em até 100ms p95; primeira página seletiva/navegação aquecida em até 500ms p95; primeira página não depender de contagem global; cancelamento liberar trabalho prontamente. Agregações globais precisam de orçamento medido separadamente.

## Comparações com Elasticsearch

Compare trabalho equivalente: mesmas fontes, filtros e semântica de texto, ordenação, página, agregações, exatidão de contagens, estado de cache e recursos. Busca por substring não equivale automaticamente a busca por tokens analisados. Consultar um cluster já indexado não equivale a baixar e preparar uma cópia local. Registre ambas as fases e a política de totais exatos/deferidos. Esta mudança não aumenta o limite de importação remota nem introduz execução remota de consultas.

## Recursos e limites de recuperação

Os pools do motor agora partem de um orçamento coordenado: por padrão, um terço da memória efetiva (até 8GiB), dividido entre até três bancos DuckDB simultâneos (base, derivados e preparação), o indexador textual e lotes limitados por bytes. O detector respeita limites de memória de contêiner Linux quando disponíveis. A configuração avançada `LOGINSIGHT_MEMORY_LIMIT_MB` altera esse orçamento, com mínimo de 128MiB e teto de metade da memória efetiva/8GiB. Metadados de linhas, páginas mapeadas, estruturas do parser e bibliotecas são adicionais; esse orçamento **não é um limite rígido de RSS**. Meça antes de escolher um valor.

Checkpoints são segmentos completos do DuckDB e Tantivy, com manifesto publicado por último. A validação dos artefatos inclui integridade e quantidade de registros. A identidade estável da fonte usa caminho, tamanho, data de modificação e amostras de conteúdo; as chaves de cache e validação ativa acrescentam a identidade do arquivo subjacente (inode/dispositivo ou ID de arquivo no Windows), rejeitando substituição atômica mesmo com tamanho/data preservados. Não é um SHA-256 integral da fonte. Alteração da configuração de parser, timestamp ou campos derivados invalida a variante correspondente.

A fonte mapeada precisa permanecer estável enquanto estiver aberta. As verificações detectam mudanças observadas antes de ler ou publicar um checkpoint, mas não bloqueiam processos externos: uma gravação ou truncamento entre a verificação e o acesso ao `mmap` ainda pode produzir leitura inconsistente ou falha do processo. Trabalhe com uma cópia/snapshot estável de logs que continuam sendo gravados. Alterações no mesmo arquivo que preservem tamanho, data e as regiões amostradas também podem escapar à identidade parcial; não há garantia de detecção integral de corrupção da fonte.

Limpar eventos ou concluir a troca de fonte libera as sessões antigas e descarta trabalho de preparação ainda pendente. Um segmento antigo em execução para no próximo ponto cooperativo; uma chamada nativa já iniciada pode precisar retornar primeiro. Consultas já em execução mantêm suas próprias referências até finalizar. Segmentos já publicados permanecem disponíveis para reutilização, e a ativação da mesma fonte preserva sessões e trabalho já preparados para ela.

A leitura inicial agora mantém um journal de metadados com limites de registros concluídos e marcadores verificados. Uma interrupção permite reutilizar esses metadados e os segmentos completos de consulta/texto; a cauda ainda não confirmada é reinterpretada para preservar registros multilinha e limites JSON. Se o carregamento de um arquivo novo for cancelado antes de substituir a fonte ativa, reabra esse mesmo arquivo para aproveitar seus checkpoints. “Tentar preparação novamente” atua na fonte que está carregada no momento.

A conversão de gzip, arquivos compactados, codificação e planilhas tem publicação própria verificada. Se for interrompida antes dessa publicação, essa conversão pode reiniciar; os checkpoints de leitura e consulta atuam sobre a fonte convertida disponível. EVTX/Event Log ainda podem produzir uma nova captura ao abrir. Não se promete retomada de toda chamada de conversão externa.

Depois de uma atualização que invalide o formato de cache, pode haver uma reconstrução inicial. Nas próximas aberturas com fonte/configuração inalteradas, os índices retidos são reutilizados. A fase “Validando índices salvos” inclui leitura dos hashes e conferência dos bancos, portanto ainda pode demorar com muitos dados; ela não significa que os registros estão sendo indexados novamente. Partes ausentes, inválidas ou removidas pela política de cache precisam ser preparadas de novo.

Timestamps configurados têm um overlay verificado próprio, incluindo configuração e caminho/nome exibidos. Reabrir a mesma fonte válida restaura esse overlay em lotes, sem reinterpretar todos os registros. A troca só vale para o novo índice ainda não publicado; cancelar não altera a fonte atualmente carregada.
