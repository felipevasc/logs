# Validação dos diagnósticos locais

Execução em 18/10/2026, a partir da v0.14.0 (`c9598dd`). Estes resultados
verificam implementação e orçamento de execução; não estabelecem precisão de
causa em incidentes reais.

## Verificações executadas

| Verificação | Resultado |
|---|---|
| Núcleo `log-learning`, release | 22 testes passaram: aprendizado, sinais tipados, abstenção, ranking e protocolo público. |
| Seleção nativa essencial, Linux, debug | 146 testes passaram, 2 opt-in ignorados. Inclui persistência, isolamento, aprendizado, diagnóstico e contratos existentes de dados. |
| Módulos alterados após os ajustes finais | 24 testes passaram; benchmark separado. |
| JavaScript | 317 arquivos analisados sintaticamente; 315 testes essenciais compatíveis com este executor passaram. |
| Navegador Chromium | Evidência/original, consulta, resolução, publicação explícita, credencial temporária, texto não confiável e rejeição de análise desatualizada passaram. |
| Versão e publicação | Verificação de versões e contratos de assinatura/publicação passaram, sem publicar release. |

O teste existente `process-supervision.test.mjs` falhou neste executor ao limpar
processos descendentes (`Process tree cleanup timed out`), tanto no workspace
quanto no container. Foi registrado separadamente e não foi alterado para
contornar a falha. A suíte compatível de 315 testes exclui somente esse arquivo;
o CI do repositório executa a suíte integral e as verificações de Windows.

A seleção nativa local usou os mesmos filtros de unidades de
`scripts/ci/native-essential.mjs`, acrescentando os testes dos novos módulos e de
precedência de mapeamentos. Os alvos de integração de arquivo separados e a
compilação de release/Windows ficam no CI. Não se atribui a eles execução local.

## Regressões reproduzidas e corrigidas

| Caso | Contrato verificado |
|---|---|
| SQLSTATE de unicidade e de conexão | Códigos e negações conservam significados distintos no agrupamento. |
| Versão B versus registros sem versão | Desconhecidos ficam separados e não se tornam controles de outra versão. |
| Degradação moderada persistente | A mudança coletiva conserva seus membros, mesmo sem extremos individuais, e forma episódio contíguo. |
| Código nativo numérico | Não adquire semântica HTTP só porque excede 500. |
| Pool saudável perto de um timeout | A medição aparece como contraponto; snapshots não recrutam uma cadeia de vizinhos. |
| Campo de duração personalizado | Segundos são convertidos para ms com procedência; unidade ausente/incompatível permanece desconhecida. |
| Revisão corrigida/inconclusiva | O rótulo anterior sai do treinamento e uma revisão pública vazia pode retraí-lo. |
| Ganho isolado de ranking | Uma única melhora não promove o modelo; pares do mesmo episódio não simulam validações independentes. |
| Histórico compartilhado crescente | 2.560 episódios de 32 origens continuam treináveis com orçamento fixo e origens reservadas inteiras. |
| Base corrompida/relatório forjado | Importação inválida preserva a anterior; uma importação válida recupera arquivo corrompido e mantém retorno disponível. |

## Corpus sintético

O [relatório reproduzível](diagnostics-synthetic.json) contém 50 contratos:
32 positivos com a causa esperada em primeiro lugar e 18 benignos/desconhecidos
com abstenção. Nenhum desses 18 recebeu diagnóstico. As cinco famílias estão
cobertas, mas há apenas um contrato positivo de deploy no núcleo, complementado
por controles concorrentes e deploy prévio nos testes nativos.

As origens de desenvolvimento e avaliação têm identificadores separados. Esse
corpus foi construído junto das regras e não é um teste cego ou representativo.
O arquivo serve para regressões, recusa de bases incompatíveis e reprodução.
Não comprova ganho percentual sobre a v0.14.0, precisão de campo ou calibração.
A base distribuída não contém resoluções de usuários; o ranking começa inativo.

## Um milhão de fatos normalizados

O [resultado bruto](diagnostics-million.json) foi obtido no teste nativo opt-in,
em container Linux com cota equivalente a 2 CPUs, memória de 4 GiB e sem swap.
O executável usa debug com símbolos da aplicação desativados. Esses limites
não simulam a velocidade de um computador físico de entrada.

| Medida | Resultado |
|---|---:|
| Registros / contextos | 1,000,000 / 128 |
| Episódios / eventos omitidos por limite | 1004 / 0 |
| Tempo da camada de diagnóstico | 69.94 s |
| Pico residente do processo | 101.3 MiB |
| Aumento do pico durante o diagnóstico | 16.8 MiB |
| SQLite, população e resultados | 669.3 MiB |

O tempo inclui projeção dos fatos, índices, episódios, hipóteses e resultados em
SQLite. A preparação da população sintética acontece antes da medição. Importação
dos logs originais, demais detectores, treinamento de perfis e interface estão
fora desse número. O teste não mede latência p95 ou consumo completo do aplicativo.

Reprodução em Linux com as dependências nativas do Tauri instaladas:

```sh
cargo test --locked --release --manifest-path crates/log-learning/Cargo.toml
cargo run --locked --release --manifest-path crates/log-learning/Cargo.toml -- diagnostic-evaluate
cargo test --locked --config profile.dev.package.loginsight.debug=0 --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1 diagnostic_tests:: local_learning::tests:: learning_commands::tests::
DIAGNOSTIC_BENCH_OUTPUT=/tmp/diagnostics-million.json cargo test --locked --config profile.dev.package.loginsight.debug=0 --manifest-path src-tauri/Cargo.toml --lib diagnostics_million_rows_resource_profile -- --ignored --test-threads=1 --nocapture
```

O próximo ensaio de eficácia deve usar incidentes locais resolvidos, separados
por ambiente, episódio e tempo. É necessário comparar regras, anomalias,
episódios e ranking no mesmo conjunto reservado antes de atribuir melhoria
representativa de sugestões ou redução de falsos diagnósticos.
