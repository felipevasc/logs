# Verificação da integração 0.12.1

Integração autorizada em 3 de outubro de 2026 com fix/v0.12.1-corrections.
O trabalho anterior da base 0.5 foi preservado no commit b899f40 antes do merge.
A arquitetura vigente da 0.12.1 é a base desta entrega: DuckDB/Tantivy, Casos
nativos, contextos de análise, cotas, checkpoints e atualização assinada.

## Escopo

- Gerenciador de recursos com histórico, processos próprios, ações e comparação
  com o sistema hospedeiro; cotas contábeis separadas da memória medida.
- Revelação explícita e temporária no detalhe, inspeção local de JSON/JWT/Base64
  e preservação de valores numéricos que perderiam precisão.
- Salvamento coalescido preservando a autoridade, revisão e recuperação da
  sessão nativa do Caso.
- Limpeza de snapshots cancelados, inserções preparadas nas contagens e
  substituição atômica dos metadados Tantivy no Windows.
- Processos de testes supervisionados, validação da tag antes da publicação e
  regressão completa da interface como requisito da release.
- Correções originais da branch: análise de comprometimentos sob demanda,
  registros estruturados encapsulados e continuidade das animações.

## Evidências

A validação da árvore integrada está em andamento. Resultados da antiga base
0.5 não são considerados aprovação da 0.12.1. A publicação depende de validações
na árvore integrada e das verificações obrigatórias do GitHub Actions.

Os comandos de referência são npm run check, npm run test:preview,
cargo test --manifest-path src-tauri/Cargo.toml --release --locked --tests --
--test-threads=1 e o aceite desktop scripts/preview/test-native-desktop.mjs.
O workflow de release ainda verifica os instaladores, suas assinaturas e a
atualização de ponta a ponta nas duas plataformas antes de publicar.

## Checkpoint enviado ao GitHub

Por solicitação do usuário, o commit e o push foram priorizados antes da
conclusão da matriz completa. A compilação de checagem Rust, incluindo alvos de
teste, passou. Os testes offline de publicação passaram e npm audit não
reportou vulnerabilidades conhecidas.

A rodada de navegador encontrou falhas no mock de recursos, em caminhos Windows
de screenshots e na confirmação da revisão ao criar um Caso. As causas foram
corrigidas; a repetição completa ainda precisa terminar. O último ajuste captura
a revisão esperada antes de entregar o snapshot ao transporte, impedindo que
uma mutação do adaptador altere o critério de confirmação. A análise sob demanda
também precisa ser revalidada após esse ajuste. O aceite do executável integrado
e as suítes completas Windows/Linux ainda não têm aprovação desta árvore.

Este checkpoint preserva o trabalho e permite ao CI continuar a validação; não
atesta uma release pronta nem uma publicação concluída. O merge na main e a
publicação devem aguardar os gates da árvore final.
