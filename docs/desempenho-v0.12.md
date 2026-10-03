# Desempenho 0.12: primeiro incremento

Status: primeira rodada A/B concluída. A ampliação posterior de paralelismo global e cadência de checkpoints está em validação; os números dessa primeira rodada ainda não representam o candidato ampliado. O [relatório inicial](performance/v0.12-validation.md) registra método, ganhos e limites. Nenhum ganho percentual do aplicativo inteiro é alegado.

## Escopo

- Seleção automática de colunas: retirar somente a varredura cujo resultado não era usado. Preferências, ordem, timestamp vazio e fallbacks permanecem iguais
- Validação de filtros: retirar o IPC preliminar de `query_page`, `count_filtered`, `stats_events` e `tree_aggs`. Cada endpoint continua chamando o mesmo validador obrigatório no backend. Outros comandos e validação do editor mantêm o caminho anterior; não há cache de validade de regras mutáveis
- Páginas: consultar seleções completas já prontas no cache canônico somente quando isso elimina confirmação residual no Event. Predicados exatos em SQL mantêm o caminho direto, preservando namespace, lease e visibilidade. Um miss não inicia nem espera seleção global. Resíduos continuam sendo confirmados no Event canônico; predicados já provados pelo SQL não são repetidos
- Diagnóstico de página opt-in e harness de comparação nativa, com fonte sintética e diretórios isolados

A remoção do preflight muda a precedência quando escopo e filtro são inválidos ao mesmo tempo: a admissão de escopo pode rejeitar primeiro. A preparação de um Caso também pode acontecer antes da rejeição de filtro. A validação obrigatória e os erros retornados pelo comando permanecem.

## Limites conservadores

O verificador analítico possui um atalho de bytes/metadados para alguns predicados `_all`. Esse atalho não constitui prova de equivalência para espaços em `contains/not_contains`, mensagens geradas pelo parser e registros de snapshot. Nesta etapa, páginas não reaproveitam uma seleção completa cujo plano contenha esses resíduos, nem usam esse atalho para confirmar os candidatos. O caminho canônico foi preservado; corrigir a semântica analítica é um trabalho separado.

A projeção de células não foi ligada à tabela. Ainda depende de ocultação antes do corte, orçamento para páginas largas e recuperação exata para detalhe/cópia/filtro/evidência. Abertura, hashes e projeção não receberam otimização adicional. O agendamento global e a cadência de checkpoints foram acrescentados por escopo posterior e exigem nova validação medida. 50M continua desativado.

## Contadores do frontend

O script `scripts/bench/frontend-repeated-work.mjs` executa as funções reais de `app.js` num VM Node, com invoke/DOM simulados:

- Fixture com 200 campos estruturados ocultos: 200 serializações antes; zero depois; mesmas colunas selecionadas
- Página + contagem + estatísticas + facets: 8 invokes antes; 4 depois, eliminando quatro preflights

Esses contadores provam trabalho removido no wrapper. Não medem a latência Rust/SQL/disco/Tauri/IPC/WebView ou o ganho percebido pelo usuário.

## Baseline reproduzível

`src-tauri/tests/engine_interaction.rs` continua ignorado por padrão. O harness mede metadados, preparação, primeira/segunda página, contagem, estatísticas e codificação JSON separada. Cada workload é executado antes e depois dos resumos exatos, com aquecimento explícito. O contrato `identityVersion: 3` mantém o contrato original de dados das duas páginas: Events completos, ordem, total, `hasMore` e cursor. `fullSha256` conserva o cursor opaco original; `sha256` é o hash semântico A/B desse mesmo payload. O único trecho excluído do hash A/B são os 64 bytes hexadecimais validados do campo `fingerprint` do cursor, pois a identidade de SourceSpec muda entre revisões de parser/cache. A forma JSON do cursor, versão positiva, posição inteira não negativa e chaves tipadas Integer/Text são validadas antes de mascarar esse trecho na string original. Nenhum outro byte do cursor é reserializado ou removido: versão, posição, chaves, campos adicionais e formatação continuam no hash.

O hash completo dos dados e cursores deve permanecer igual entre repetições e fases antes/depois dos resumos, além dos processos e reaberturas da mesma versão. O A/B compara o hash semântico e os conjuntos completos de amostras. Ausência de `identityVersion: 3`, de qualquer hash ou da auditoria operacional rejeita resultados de harness antigo (inclusive v2), exigindo recompilar ambos os binários. Diferenças anteriores no hash opaco não provam equivalência de linhas; essa prova depende da nova comparação.

Correção auditada do contrato: a primeira implementação v2 ampliou inadvertidamente o contrato original ao incluir `engine`/`warning` no hash de dados. O dump do workload `cached_raw_regex` mostrou Events, ordem, total, `hasMore` e cursores idênticos nas duas páginas antes/depois da contagem; somente o aviso exato `Este filtro exige confirmação nos registros; consultas amplas podem demorar mais.` mudou para `null` após a seleção completa pronta. Esse aviso descreve se o caminho da página exige confirmação residual; não é um indicador de perda de dados. A versão 3 separa apenas esses dois campos operacionais conhecidos, sem ignorar avisos genericamente e sem alterar a produção para forçar igualdade artificial.

Cada resposta deve ter exatamente os seis campos esperados (`rows`, `total`, `hasMore`, `nextCursor`, `engine`, `warning`); campos adicionais, inclusive diagnósticos de truncamento, incompletude, erro, cancelamento ou limite, rejeitam a execução. `engine` deve ser `columnar`; `warning` deve ser `null` ou a única frase literal acima. Na primeira amostra medida de cada workload/fase, `explain_page_at` sem ANALYZE audita cada página usando seu offset e cursor de entrada real, fora dos timers. `exactPredicate: true` ou `verified_singleton` exige aviso nulo; `exactPredicate: false` exige exatamente a frase residual. Metadados devem permanecer iguais nas demais repetições daquela fase; mudança exige nova auditoria, não é aceita silenciosamente. Ambos os lados do A/B passam por essa prova.

Os campos operacionais continuam registrados por página em `diagnostics`; `pathAudit` conserva offset, cursor de entrada e plano completo da primeira amostra de cada fase. `responseSha256` identifica a resposta original completa, incluindo metadados, e os dumps continuam contendo as respostas originais. Esses diagnósticos não precisam ser iguais entre caminhos comprovadamente exatos/residuais, mas os hashes de dados e cursores continuam estritos. O stdout/dump registra a prova antes de uma eventual rejeição por aviso incompatível com o caminho.

Fora dos timers, o primeiro cursor do workload `all` também é submetido ao `page_seek` pelo `explain_page_at`: alterar somente o fingerprint para outro 64hex ou usar versão não suportada deve retornar `PAGINATION_RESET_REQUIRED`. Fixtures sem paginação registram explicitamente que esse gate não se aplica.

O conjunto inclui texto comum/raro/ausente, ID exato, filtros simples, um resíduo canônico excluído do cache e um predicado elegível ao reaproveitamento na página (`raw regex`) e um controle SQL exato (`message contains`), que mantém o caminho direto mesmo após a contagem. Antes de concluir que houve hit, verificar `completeSelectionHit` no diagnóstico; não inferir hit somente porque uma contagem terminou.

1. Criar dois checkouts isolados: base publicada e candidato. Copiar apenas o mesmo harness `engine_interaction.rs` para a base, sem copiar as otimizações
2. Compilar os dois executáveis de teste com a mesma toolchain, Cargo.lock, perfil e configurações, guardando cópias e SHA-256 distintos. Compilação fica fora das amostras
3. Gerar explicitamente as fixtures; a geração nunca sobrescreve arquivos existentes:

```sh
node scripts/bench/generate-logs.mjs --output /caminho/100k.jsonl --rows 100000
node scripts/bench/generate-logs.mjs --output /caminho/wide-100.jsonl --rows 100 --padding 65536
```

4. Executar uma comparação para cada fixture e direção. O output deve ser um diretório novo:

```sh
LOGINSIGHT_MEMORY_LIMIT_MB=512 node scripts/bench/compare-performance.mjs \
  --baseline /caminho/binario-base \
  --candidate /caminho/binario-candidato \
  --baseline-source /caminho/checkout-base \
  --candidate-source /caminho/checkout-candidato \
  --fixture /caminho/100k.jsonl \
  --output /caminho/resultado-novo \
  --processes 5 --repeats 30 --warmup 3 --sort-dir asc
```

O runner não instala, compila, gera cargas ou modifica arquivos existentes. Limita esta rodada a 100 mil linhas; alterna a ordem A/B, usa cinco diretórios próprios por versão e reabre cada um em processo novo. Identifica binários, fontes (incluindo conteúdos ainda não rastreados pelo Git), manifesto e ambiente; preserva stdout/stderr e amostras brutas. Com cinco processos, publica mediana e faixa, sem p95 de abertura. O p95 de interação exige ao menos 30 amostras.

A memória RSS é amostrada a cada 100 ms e reportada como limite inferior do pico, não pico exato do SO. `cacheBytes` separa dados/metadados e motor, excluindo relatórios. O cache do SO é não controlado: o hash da fixture aquece leituras. Os tempos de página incluem a conversão do testkit em `serde_json::Value`; `json_encode` mede a codificação em bytes separadamente, sem IPC ou pintura.

## Diagnóstico causal separado

`LOGINSIGHT_PAGE_TRACE=1` habilita linhas JSON `PAGE_TRACE` em stderr com seleção, scope, SQL, avaliação residual e hidratação; inclui número de lotes, candidatos, resíduos e indicador de seleção completa reaproveitada. O runner de latência remove essa variável e `LOGINSIGHT_BENCH_DUMP_PAGES`/`LOGINSIGHT_BENCH_DUMP_PHASE` do processo filho para não misturar diagnóstico com A/B.

Para inspecionar diferenças reais, há dump opt-in das respostas originais da primeira amostra, incluindo seus dois cursores, separado de stdout. Na execução diagnóstica direta do binário, usar `LOGINSIGHT_BENCH_DUMP_PAGES=all` para selecionar apenas o workload `all` e `LOGINSIGHT_BENCH_DUMP_PHASE=before_exact_summary` (padrão) para selecionar apenas a primeira fase. Uma lista separada por vírgulas seleciona workloads específicos; `*` seleciona todos e `both` seleciona ambas as fases, somente quando necessário. Cada arquivo novo fica em `LOGINSIGHT_BENCH_DIR/<cache-state>.<workload>.<phase>.pages.json` e nunca sobrescreve um existente. O dump exige manifesto sintético generator-v2 com linhas, tamanho e SHA-256 conferidos, e permanece desabilitado por padrão. Hashing, validação e escrita ocorrem fora dos timers das operações; a execução diagnóstica inteira, incluindo RSS, não serve como amostra de latência A/B. Para fixture larga, selecionar só `all`/primeira fase evita cópias grandes por workload.

Para conferir o caminho, executar o binário candidato separadamente com as variáveis `LOGINSIGHT_BENCH_*`, `LOGINSIGHT_PAGE_TRACE=1`, `LOGINSIGHT_BENCH_REPEATS=1` e `LOGINSIGHT_BENCH_WARMUP=0`. Isso não é uma amostra de latência comparável. As provas de cancelamento, visibilidade, eviction e regras mutáveis ficam nos testes nativos focados.

## Validação e gates restantes

Os quatro cenários A/B (100k/largo, asc/desc), 966 testes unitários nativos e 18 integrações focadas passaram. A paridade inclui dados completos, cursores, auditoria dos diagnósticos e reabertura. O ganho residual foi confirmado; o relatório registra também a dispersão localizada em texto comum e a correção de uma regressão do caminho SQL exato.

- Validar os fluxos em navegador real e a matriz nativa Linux/Windows no commit final
- Antes de publicação, validar instaladores, assinatura e atualização pelos gates obrigatórios do repositório
- Não atribuir ganho ponta a ponta até medir IPC e WebView instalado; nenhum teste de preview substitui essa prova
