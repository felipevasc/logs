# Checkpoints de metadados: medição e gates da 0.12

Status: política incremental de durabilidade autorizada para a 0.12, com harness
local opt-in e isolado. A publicação continua usando o journal v1, hashes,
manifesto e leases existentes. O debounce de 200 ms dos Casos não é alterado.

## Política de produção e rollback

`LOGINSIGHT_METADATA_CHECKPOINT_POLICY=batched` (padrão) publica quando o delta
atinge **65.536 registros selados OU 16 MiB de fonte escaneada OU 2 segundos**
desde o último commit bem-sucedido. Os três critérios são avaliados somente na
fronteira segura entregue pelo parser. A contagem de bytes é de **fonte**, não do
payload de metadados de 27 bytes por registro. O relógio é reiniciado no commit e
na abertura/restauração; não existe timer ou thread de checkpoint em segundo plano.

`LOGINSIGHT_METADATA_CHECKPOINT_POLICY=wave` é o rollback explícito para a
cadência anterior, a cada wave segura. Selecionar antes de abrir/reabrir a fonte
ou iniciar o aplicativo. Valor desconhecido usa `wave` com aviso. Não apagar
caches: ambos os modos leem/escrevem exatamente o mesmo journal e cache key.
Não há configuração pública de thresholds arbitrários; os valores reduzidos do
testkit são apenas de diagnóstico, separados da opção `smallWaves`.

EOF e publicação das colunas sempre forçam seus checkpoints, inclusive delta
zero. Uma cauda multilinha ainda mutável fica no manifesto e nunca é contada como
registro selado; fonte escaneada/tempo também protegem esse progresso. Validação
da fonte e cancelamento cooperativo continuam em cada wave. Adiar persistência
não aumenta batches, nem retém registros adicionais em RAM: o spool continua com
buffer de 256 KiB e uma única cauda mutável. O append/hash usa blocos de 8.192
registros e buffer de journal de 1 MiB, sem alteração.

Os limites são gatilhos, **não teto estrito de trabalho perdido**: um wave ou
registro indivisível pode ultrapassá-los. Uma wave pode durar mais de 2 segundos;
nenhum checkpoint será feito no meio do parser, de um registro ou de um syscall.
Com várias CPUs, uma wave já pode superar 16 MiB: nesse caso os dois modos podem
ter a mesma quantidade de commits. Menos commits demonstram menos operações de
persistência, mas não provam menor latência total. Em troca, uma interrupção pode
exigir reparse de mais waves desde o último manifesto.

## Duas unidades diferentes

- O motor de consulta segmenta em 1 milhão de registros **OU** 256 MiB. Isso não
  define o tamanho do checkpoint de metadados
- O parser chama o sink de metadados em cada onda intermediária, ao fechar a
  leitura e ao publicar as colunas. A política decide quais chamadas intermediárias executam flush,
  sincronização do payload e publicação do manifesto
- Em linhas, cada chunk admite até 4 MiB **OU** 65.536 linhas físicas; a onda usa
  `workers * 2` chunks. Um registro indivisível pode ultrapassar o limite de
  bytes. Arrays JSON têm os seus próprios limites de registros/bytes
- O manifesto é o ponto de commit. Bytes sincronizados além do prefixo coberto
  pelo manifesto não são linhas reutilizáveis. A cauda multilinha tem tratamento
  separado e não deve ser contada como registro selado

Com oito workers, uma onda de linhas pode abranger cerca de 64 MiB. Um ensaio de
100k registros pode, portanto, ter somente as publicações de EOF e colunas. Não
interpretar isso como prova de que reduzir a frequência trará ganho. Não elevar
os limites deste harness para obter artificialmente mais amostras: o testkit
materializa todos os Events. Um ensaio maior exige primeiro um probe de abertura
sem essa materialização, revisão e orçamento de memória/disco explícitos.

## Artefatos

- `src-tauri/tests/metadata_checkpoint_measurement.rs`: teste ignorado por padrão;
  observa os progressos reais e grava um relatório JSON por processo
- `scripts/bench/summarize-metadata-checkpoints.mjs`: lê o relatório, valida a
  sequência dos estágios e resume chamadas, intervalos e trabalho não confirmado
- `scripts/tests/metadata-checkpoint-measurement.test.mjs`: testes do resumo com
  dados sintéticos, sem executar o backend ou alegar desempenho nativo
- `src-tauri/tests/metadata_recovery.rs`: suite existente de paridade,
  cancelamento, corrupção, geração da fonte, multiline e processos realmente
  mortos, que continua sendo gate obrigatório

Os modos `cold` e `warm` usam ondas de produção (`smallWaves: false`). O fator
`LOGINSIGHT_CHECKPOINT_POLICY=wave|batched` é obrigatório no harness e gravado com
os thresholds efetivos. `interrupt` e `resume` usam ondas pequenas e thresholds
explícitos de 16 registros/4 KiB/60 s no modo batched; wave mantém a cadência
anterior em cada fronteira. Esses limites tornam determinístico o ponto de
recuperação. Não comparar seus tempos com os de produção.

O progresso adicional `metadata-checkpoint-manifest-ready` ocorre depois do
sync do arquivo temporário de manifesto e antes de seu rename. Não confirma
linhas. O resumo permite separar esse intervalo de rename/sync do diretório;
relatórios legados sem essa fase mantêm esses subintervalos como `null`.

## O que os números significam

`callsReachingWrite` conta uma vez cada chamada que chega ao primeiro progresso
`metadata-checkpoint-write`; o segundo progresso dessa fase é o fim da escrita,
não outra chamada. Chamadas rejeitadas antes dessa fase não são observáveis aqui.
`committedCalls` conta progressos de commit; `metadataOnlyCommittedCalls` separa
publicações sem novas linhas, incluindo conclusão de colunas. Não eliminar essas
publicações só porque o delta de linhas é zero.

Os intervalos em `committedIntervalsMs` e `calls` são:

1. `write`: entrada do primeiro progresso de escrita até entrada do progresso de
   sync. Inclui leitura do spool, codificação, append, hash e callbacks. Exclui
   validação inicial e `LineBuilder::flush`, anteriores ao primeiro progresso
2. `syncAndValidation`: entrada de sync até entrada de publish. Inclui flush do
   buffer, `sync_all` do payload, verificações de cancelamento, revalidação da
   fonte e callbacks. **Não é tempo puro de fsync**
3. `publication`: entrada de publish até entrada de committed. Inclui criação e
   serialização do manifesto, escrita/fsync, rename, sincronização do diretório
   no Unix, verificações e callbacks. Não separa esses syscalls
4. `total`: soma dos três intervalos observados. **Não é toda a duração da chamada
   checkpoint_rows**, porque há trabalho antes/depois dos limites observados

Não gravamos em disco/stdout a cada callback; as observações são acumuladas e
salvas depois. Ainda há overhead de relógio, JSON e armazenamento das observações.
`probeElapsedMs` também inclui preparação da fonte, validação do journal e
materialização de todos os Events do testkit. Não é abertura pura, RSS do app,
IPC, pintura ou latência percebida. A pré-validação/hash da fixture ocorre antes
do cronômetro e aquece o cache de arquivos do SO. O oracle sem cache é executado
somente após a amostra e não entra em `probeElapsedMs`.

O resumo recusa persistência degradada, sequência incompleta num ensaio de
sucesso, regressão do relógio/cursor, falta de paridade e reparse no warm. Um
intervalo interrompido fica separado em `uncommittedCall`, sem duração inventada.
Uma quantidade pequena ou heterogênea de checkpoints não produz p95 significativo;
o resumo preserva amostras e totais, sem anunciar percentis.

## Gate 1: compilar e executar a recuperação existente

Usar a mesma toolchain compatível, `Cargo.lock`, perfil release e dependências
nativas do restante do projeto. Não instalar nem acionar CI remoto apenas porque
um comando abaixo está documentado. Se a toolchain/SDK não estiver disponível,
registrar o bloqueio e parar antes de qualquer mudança de cadência.

```sh
npm run prepare:frontend
cargo test --manifest-path src-tauri/Cargo.toml --release --locked \
  --lib metadata_checkpoint::tests -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --release --locked \
  --test metadata_recovery -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --release --locked \
  --test metadata_checkpoint_measurement --no-run --message-format=json \
  > /caminho/novo/checkpoint-build.jsonl
```

Selecionar o campo `executable` do registro `compiler-artifact` cujo
`target.name` seja `metadata_checkpoint_measurement`, sem escolher um binário
antigo por wildcard. Guardar o binário e seu SHA-256. As execuções abaixo usam esse
executável já compilado; a compilação nunca faz parte da amostra.

O teste `killed_process_resumes_without_reparsing_committed_rows_and_lock_protects_pruning`
exercita processos mortos depois do commit e depois do sync do payload, antes do
próximo manifesto. A matriz cobre antes do append, antes do sync do payload, depois do sync do
payload, depois do sync do manifesto temporário e depois do commit. Ela verifica
prefixo durável, resultado exato, truncamento do payload não confirmado, lock de
escritor/pruning e warm sem reparse. Uma segunda matriz de processos mortos cobre
a cauda multilinha; outra solicita cancelamento cooperativo nos mesmos gates. A suite
completa também deve passar: só executar esse teste não cobre todos os parsers,
cauda multilinha, colunas, corrupção e mudança da fonte. Morte de processo não é
simulação de corte de energia nem prova de durabilidade de qualquer filesystem.

## Gate 2: custos e contadores com ondas de produção

Criar manualmente um diretório pai novo. O gerador recusa sobrescrita. Limites do
harness: 100k objetos JSONL, 64 MiB totais, 1 MiB por linha física. Usar fontes
sintéticas, estáveis e a mesma localização para cada par de abertura/reabertura.

```sh
node scripts/bench/generate-logs.mjs --output /caminho/novo/100k.jsonl --rows 100000
node scripts/bench/generate-logs.mjs --output /caminho/novo/wide.jsonl --rows 100 --padding 65536

# BIN = caminho exato do executável compilado; cada comando é um processo novo
LOGINSIGHT_MEMORY_LIMIT_MB=512 LOGINSIGHT_CHECKPOINT_POLICY=batched \
LOGINSIGHT_CHECKPOINT_FILE=/caminho/novo/100k.jsonl \
LOGINSIGHT_CHECKPOINT_DIR=/caminho/novo/rodada-1 \
LOGINSIGHT_CHECKPOINT_MODE=cold \
"$BIN" --ignored --exact checkpoint_measurement_workload --nocapture --test-threads=1

LOGINSIGHT_MEMORY_LIMIT_MB=512 LOGINSIGHT_CHECKPOINT_POLICY=batched \
LOGINSIGHT_CHECKPOINT_FILE=/caminho/novo/100k.jsonl \
LOGINSIGHT_CHECKPOINT_DIR=/caminho/novo/rodada-1 \
LOGINSIGHT_CHECKPOINT_MODE=warm \
"$BIN" --ignored --exact checkpoint_measurement_workload --nocapture --test-threads=1

node scripts/bench/summarize-metadata-checkpoints.mjs /caminho/novo/rodada-1/cold.json
node scripts/bench/summarize-metadata-checkpoints.mjs /caminho/novo/rodada-1/warm.json
```

`rodada-1` deve estar ausente antes do cold; warm lê `cold.json`, exige a mesma
fonte/hash/binário/ambiente registrado e cria `warm.json` sem sobrescrever. Arquivo
vazio deixado por uma falha não é uma amostra; guardar o erro e usar uma rodada
nova. Não remover o diretório de outra pessoa nem caches pessoais. Repetir com a
fixture larga e cinco pares de processos/diretórios distintos. Guardar cada
relatório bruto, stdout/stderr, SO, CPU/RAM/disco/filesystem, toolchain, Cargo.lock,
SHA/diff da fonte, configuração efetiva de recursos e manifesto da fixture.

A paridade compara SHA-256 de formato, headers, colunas, todos os metadados e
Events completos. Cold compara com o oracle sem cache; warm compara com cold e
exige `parsedRows == 0`, `resumedRows == rows` e nenhuma nova escrita de checkpoint.
Isso não substitui testes de outros formatos ou de comportamento da UI.

Se algum dia houver candidato de política aprovado, compilar base e candidato
com o mesmo harness e executar em diretórios próprios, alternando A/B e B/A.
Comparar distribuições entre processos, quantidade e tamanho das publicações e
trabalho perdido, sem atribuir causalidade só a uma diminuição do tempo total.
Usar também a medição de abertura do harness de interação sem esse diagnóstico;
os números do probe que materializa Events não a substituem.

## Gate 3: contadores de trabalho descartado e retomada

Ensaio separado, no máximo 1.000 registros/1 MiB. A fixture de 73 linhas com thresholds explícitos permite
observar várias waves adiadas, uma publicação intermediária e a próxima tentativa
com linhas novas.
Os modos de custo e recuperação nunca compartilham diretório.

```sh
node scripts/bench/generate-logs.mjs --output /caminho/novo/recovery.jsonl --rows 73

LOGINSIGHT_MEMORY_LIMIT_MB=512 LOGINSIGHT_CHECKPOINT_POLICY=batched \
LOGINSIGHT_CHECKPOINT_FILE=/caminho/novo/recovery.jsonl \
LOGINSIGHT_CHECKPOINT_DIR=/caminho/novo/recovery-1 \
LOGINSIGHT_CHECKPOINT_MODE=interrupt \
"$BIN" --ignored --exact checkpoint_measurement_workload --nocapture --test-threads=1

LOGINSIGHT_MEMORY_LIMIT_MB=512 LOGINSIGHT_CHECKPOINT_POLICY=batched \
LOGINSIGHT_CHECKPOINT_FILE=/caminho/novo/recovery.jsonl \
LOGINSIGHT_CHECKPOINT_DIR=/caminho/novo/recovery-1 \
LOGINSIGHT_CHECKPOINT_MODE=resume \
"$BIN" --ignored --exact checkpoint_measurement_workload --nocapture --test-threads=1

node scripts/bench/summarize-metadata-checkpoints.mjs /caminho/novo/recovery-1/interrupt.json
node scripts/bench/summarize-metadata-checkpoints.mjs /caminho/novo/recovery-1/resume.json
```

O primeiro processo solicita cancelamento cooperativo quando a segunda onda com
linhas novas chega ao progresso `metadata-checkpoint-publish`, após o sync do
payload e antes da publicação. Não retorna índice parcial. O segundo processo
retoma e exige paridade completa com o oracle e:

- `resumedRows == durableRowsBeforeInterrupt`
- `parsedRowsAfterResume + resumedRows == sourceRows`
- `metadataRowsParsedButNotCommitted`: metadados interpretados antes da
  interrupção menos registros cobertos pelo último manifesto
- `sourceBytesScannedButNotCommitted`: cursor da última onda menos cursor do
  último commit. É trabalho de leitura a repetir, não perda de bytes da fonte

`parsedRowsAfterResume` inclui também o sufixo nunca interpretado; não chamar
esse total de trabalho perdido. `maxObservedUncommittedMetadataRows` e
`maxObservedUncommittedSourceBytes` são máximos nos pontos observados, limites
inferiores amostrados de trabalho em risco. Não são garantia de pior caso. O
cursor inicial de uma restauração não aparece no protocolo de progressos; por
isso risco de bytes fica `null` em warm/resume em vez de inventar zero.

Esse novo ensaio é cancelamento cooperativo, não substitui o teste existente com
morte real de processo. As fórmulas de linhas deste ensaio são específicas para
JSONL canônico, sem multilinha/linhas vazias/comentários. Não extrapolar a diferença
simples `parsedRows - checkpointRows` para uma cauda multilinha não selada.

## Comparação controlada após autorização da política

Usar o **mesmo novo executável** e a mesma fixture para `wave` e `batched`, com
`LOGINSIGHT_CHECKPOINT_POLICY` como fator explícito. Não confundir a comparação
com o binário antigo, que também difere no controle global de paralelismo. O
harness grava a política, os três thresholds e o perfil efetivo de recursos no
contexto; warm exige igualdade de contexto com seu cold.

Fixture bounded preparada: `checkpoint-4000-wide.jsonl`, 4.000 registros,
padding 14.336, 58.508.593 bytes (55,8 MiB), SHA-256
`ad8ed01c45cf5e5b97347df831d63281a1837167bbe1d9f82adfe5504289ae5a`.
É menor que os limites existentes de 100k/64 MiB. Executar:

1. `LOGINSIGHT_CHECKPOINT_WORKERS=1`, perfil salvo de startup, nos dois modos.
   É um cenário controlado de baixo paralelismo, com waves reais de produção;
   nunca usar `smallWaves` para fingir esse caso. Requer mais de um checkpoint de
   dados e a publicação final de colunas
2. `LOGINSIGHT_CHECKPOINT_WORKERS=default`, nos dois modos, sem forçar ganho.
   Conferir `resources.parserThreads` e `resources.globalParallelism` efetivos.
   Waves maiores podem tornar a quantidade de commits idêntica
3. Alternar ordem A/B e B/A em cinco pares de diretórios novos por cenário.
   Guardar relatórios brutos, resumos, stdout/stderr, SHA do binário e configuração
4. Comparar commits, registros/bytes por commit, intervalos observados, latência
   completa do probe e contadores de trabalho em risco. Separar cold e warm.
   Não transformar soma de fsyncs evitados em alegação de aceleração da UI
5. Executar cancel/recovery separadamente. Medir o prefixo durável realmente
   reutilizado e o sufixo reinterpretado, sem chamar sufixo nunca interpretado
   de trabalho perdido

A pré-validação aquece o cache do SO e o probe materializa todos os Events. O
orçamento de 512 MiB é de componentes do aplicativo, não um teto de RSS do teste.
Este ensaio pequeno não substitui abertura/interação nativas, testes completos,
50M, teste de energia interrompida nem hardware/filesystems adicionais. A
aceitação depende de paridade exata e limites honestos, não de ganho presumido.

## Resultado do ensaio inicial (2026-10-02)

Candidato integrado de validação, antes dos ajustes finais de isolamento dos
Casos. Linux x86_64, perfil release, orçamento de referência 512 MiB; executável
SHA-256 `6fd13c11f687edb2fd93960e1ab6c04ad0b9f2e3c277350fd254ccf7b723e7bc`.
A política de checkpoint e o parser não foram alterados entre este ensaio e os
ajustes posteriores de migração/isolamento. Cinco pares de processos por fixture:

| Fixture | Chamadas/commits por abertura | Mediana do intervalo observado de checkpoint | Faixa |
| --- | --- | --- | --- |
| 100.000 registros / 27.814.629 bytes | 2 / 2, sendo 1 só de metadados | 3,884 ms | 3,250–4,750 ms |
| 100 registros largos / 6.582.713 bytes | 2 / 2, sendo 1 só de metadados | 0,842 ms | 0,803–0,977 ms |

Todas as reaberturas tiveram zero reparse, zero escrita nova e paridade completa.
O ensaio separado de 73 linhas interrompeu após sincronizar dados ainda não
publicados: retomou as 4 linhas confirmadas, reprocessou 69 e coincidiu com o
oracle. Isso não simula queda de energia. Os 12 testes nativos existentes de
`metadata_recovery`, incluindo processos mortos, também passaram nesta rodada.

A mediana do probe completo foi 903,288 ms (100k) e 23,835 ms (largos), mas inclui
materialização de Events e não é a abertura do aplicativo. Não atribuir sua
latência inteira a checkpoints nem extrapolar este pequeno conjunto para 1M/50M.
Esse ensaio histórico não demonstrou vantagem em mudar a cadência; a decisão
inicial foi mantê-la. A autorização posterior ampliou o escopo, com comparação
controlada nova e rollback descritos acima. Não reutilizar esses números como
resultado da política batched.


## Resultado da política autorizada (2026-10-02, build12)

Linux x86_64; 9 CPUs disponíveis; orçamento de componentes de 512 MiB. Cenário
`1`: parser/global = 1/1. Cenário `default`: parser/global = 4/8. Ambos usam os
mesmos limites de chunks/waves de produção; nenhuma onda artificial nos tempos.
Executável SHA-256
`f45f7b1242a15e1888d02c7d9a3677efc6963fe8e0a3250222a31634ffb98ee5`.
Políticas comparadas no mesmo binário, cinco pares de processos por cenário e
fixture, alternando wave/batched e batched/wave. Sem compilação ou outra carga
nativa concorrente durante a comparação.

A suite nativa final passou com 997 unitários (também com afinidade de um núcleo)
e 54 integrações, sem exclusões; inclui os 16 testes de `metadata_recovery`.
O resumo JavaScript passou 9 testes e continuou aceitando os 22 relatórios
históricos da primeira rodada sem modificá-los. Esses números de testes não
substituem os limites de plataformas/hardware descritos acima.

Foram preservados 120 relatórios de cold/warm e quatro de recovery em
`native/checkpoint-policy-build12`, com proveniência, contexto de recursos,
stdout/stderr, relatórios brutos, resumos por processo e `comparison.json`.
Todas as aberturas cold coincidiram com o oracle; todos os fatores de uma mesma
fixture tiveram o mesmo hash semântico. As 60 reaberturas warm fizeram **zero
reparse e zero escrita de checkpoint**.

A tabela mostra commits totais (inclui EOF e colunas), mediana e faixa da soma
dos **intervalos observados de checkpoint por abertura**, e mediana do probe
completo. Unidade: ms. Não são tempos puros de fsync nem abertura da UI.

| Fixture | Workers | Política | Commits | Checkpoint observado, mediana (mín–máx) | Probe, mediana |
| --- | --- | --- | --- | --- | --- |
| 100k | 1 | wave | 5 | 5,441 (5,054–5,554) | 1163,636 |
| 100k | 1 | batched | 3 | 4,553 (4,250–5,059) | 1161,828 |
| 100k | default | wave | 2 | 3,757 (3,638–5,411) | 906,659 |
| 100k | default | batched | 2 | 3,942 (3,610–3,966) | 926,164 |
| wide-100 | 1 | wave | 2 | 0,943 (0,874–1,092) | 26,696 |
| wide-100 | 1 | batched | 2 | 1,026 (0,860–1,104) | 25,754 |
| wide-100 | default | wave | 2 | 0,916 (0,892–2,048) | 26,540 |
| wide-100 | default | batched | 2 | 1,006 (0,864–1,240) | 24,319 |
| checkpoint-4000-wide | 1 | wave | 8 | 2,681 (2,552–3,005) | 227,554 |
| checkpoint-4000-wide | 1 | batched | 5 | 1,974 (1,877–2,064) | 229,394 |
| checkpoint-4000-wide | default | wave | 3 | 1,780 (1,726–1,846) | 200,440 |
| checkpoint-4000-wide | default | batched | 3 | 1,632 (1,580–1,913) | 197,587 |

Conclusão limitada: com um worker, a política reduziu publicações de 5 para 3 na
fixture 100k e de 8 para 5 na fixture de 55,8 MiB. O intervalo observado mediano
diminuiu de 5,441 para 4,553 ms e de 2,681 para 1,974 ms, respectivamente. O probe
completo ficou praticamente igual no primeiro caso e ligeiramente maior no
segundo; **não foi demonstrado ganho geral de latência de abertura**. No cenário
default as quantidades de commits foram idênticas entre políticas em todas as
fixtures. Diferenças pequenas nos tempos com a mesma quantidade de commits não
são evidência causal a favor de batched.

O custo de reexecução cresce no cenário de um worker. Máximos observados nos
pontos de progresso (não garantias de pior caso):

- 100k: 30.160 → 60.320 registros não confirmados; 8.388.939 → 16.777.662 bytes de
  fonte desde o último commit
- 55,8 MiB: 574 → 1.148 registros; 8.396.021 → 16.791.971 bytes
- No default, ambos os modos têm os mesmos máximos: 100k/27.814.629 bytes na
  fixture 100k; 2.296/33.583.917 bytes na fixture de 55,8 MiB

O segundo exemplo também demonstra por que 16 MiB é gatilho, não teto: registros
indivisíveis e a fronteira de wave ultrapassam o valor nominal.

Recovery separado, fixture de 73 linhas e cancelamento depois do sync de dados,
antes do manifesto:

| Política diagnóstica | Prefixo reutilizado | Já interpretados mas não confirmados | Bytes escaneados não confirmados | Interpretados após retomada |
| --- | --- | --- | --- | --- |
| wave (cada wave pequena) | 4 | 4 | 1.110 | 69 |
| batched (16 registros/4 KiB/60 s) | 16 | 16 | 4.459 | 57 |

Ambos restauraram resultado exato e nunca reinterpretaram o prefixo confirmado.
57/69 incluem o sufixo nunca lido; a comparação de trabalho já feito e perdido é
**4 → 16 registros**, não 69 → 57. Os limites diagnósticos desse ensaio não
representam um teto de reparse da política de produção. A matriz nativa separada
matou processos nos cinco gates com JSONL e com cauda multilinha; isso verifica
recuperação após morte de processo, não corte de energia.

`/usr/bin/time` estava ausente. O runner registrou RSS como `null`, sem instalar
ferramentas ou inventar amostras. Não há conclusão empírica de pico de RAM neste
ensaio; os limites estruturais de buffers permanecem os já descritos. O rollback
`LOGINSIGHT_METADATA_CHECKPOINT_POLICY=wave` permanece disponível sem invalidar
ou apagar journals existentes.
