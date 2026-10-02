# Validação essencial e latência de publicação

## O que é obrigatório

- Em todo PR: parse de JavaScript, **todos** os testes Node, versão e salvaguardas de release (`npm run check`). Os testes de integridade, assinatura, recusa de sobrescrita e preservação de dados continuam presentes.
- Mudança de módulo de interface: seis fluxos essenciais no Chromium, mais os testes dos subsistemas alterados. Os essenciais cobrem navegação/cancelamento, responsividade, salvamento antes da atualização e retomada, referências do Caso, abertura nativa simulada e Timeline nativa simulada. Não são validação do WebView instalado.
- Mudança de código compartilhado (`app.js`, `index.html`, `styles.css`, `analysis-context.js`, `performance-core.js`): navegador completo; não recompila Rust. Esses arquivos atravessam vários subsistemas.
- Mudança Rust, dependência raiz, instalador, publicação, build ou CI: suíte nativa completa Windows/Linux, serial, inclusive em PR rascunho; navegador completo. Não há corte de testes nativos relevantes.
- Caminho desconhecido ou comparação ausente: matriz completa. Falha ao calcular a comparação bloqueia a validação. O diff inclui remoções e os dois lados de renomeações.
- Apenas documentação: as verificações Node/versão/release continuam; navegador e nativo são declarados não aplicáveis.
- `Required validation` só passa quando todos os gates selecionados passaram. Cancelamento, falha ou um gate obrigatório pulado bloqueia o resultado. O relatório de escopo e `validation-plan.json` registram exatamente a seleção.

Marcar um PR como pronto para revisão não dispara de novo a mesma matriz: o escopo já é igual em draft e em revisão. Os eventos automáticos de validação são abertura, atualização de commits, reabertura e troca da branch base. Ajustar somente título/descrição não repete builds: esse evento usa nomes de checks separados, sem substituir um `Required validation` anterior por um resultado pulado. O disparo manual continua disponível.

O executável portátil Windows deixou de ser reconstruído em todo PR. Pode ser pedido em `Pull request checks` → **Run workflow** → `portable_windows`. Mantém identidade SHA-256/commit e verificação do manifesto quando solicitado. O manifesto do harness Windows continua obrigatório sempre que o nativo roda.

A regressão ampla não foi apagada: `npm run test:preview` mantém a lista completa existente, e o disparo manual de `Pull request checks` seleciona todas as plataformas e o navegador completo. Testes de navegador alterados entram diretamente na seleção. Ajustar apenas o seletor dispara a regressão de navegador completa; os testes Node verificam suas decisões sem exigir uma nova compilação Rust. Novas áreas devem ser mapeadas em `scripts/ci/validation-plan.mjs`; um módulo desconhecido força o conjunto completo até isso acontecer.

50M permanece fora do CI obrigatório e exige pedido explícito. Quando a escala é relevante, começar por 1M em janela coordenada; as fixtures ignoradas não são ligadas automaticamente.

## Medição anterior à mudança

Dados das etapas e logs do GitHub, sem confundir compilação com execução:

| Execução | Plataforma / trecho | Tempo observado |
| --- | --- | --- |
| [Checks #46](https://github.com/felipevasc/logs/actions/runs/36941005524) | Frontend total / navegador completo | 5m14 / 4m17 |
| Checks #46 | Windows aguardando runner | 15m09 |
| Checks #46 | Windows compilação dos testes / testes executando | 11m14 / aproximadamente 4m50 |
| Checks #46 | Windows build portátil adicional | 5m53 |
| [Build #47](https://github.com/felipevasc/logs/actions/runs/36941431305) | Windows total / Linux total | 53m00 / 29m51 |
| [Release #49](https://github.com/felipevasc/logs/actions/runs/36946317252) | Windows total / Linux total | 52m23 / 40m18 |
| Release #49 | Windows compilação dos testes / execução dos testes | 25m53 / aproximadamente 5m54 |
| Release #49 | Windows instaladores / atualização instalada | 5m48 / 13m02 |
| Release #49 | Linux compilação dos testes / execução dos testes | 18m04 / aproximadamente 2m02 |
| Release #49 | Publicação, depois dos builds | 27s |

O PR restaurou o cache `backend`; os builds #47 e #49 não encontraram seu cache `build`. O ambiente dos jobs também diferia em `RUST_TEST_THREADS`, incluído no hash padrão do rust-cache. Os dois workflows agora usam `shared-key: desktop-release-v1` e o mesmo ambiente Rust. Isso compartilha **dependências** compatíveis, respeitando versão do compilador, dependências, plataforma e isolamento de branches do GitHub. Não usa o cache como prova de teste nem promove binários de PR para release. A primeira execução da nova chave pode ser fria; o cache de um PR não fica disponível automaticamente para `main`.

A retirada do portátil elimina uma etapa que custou 5m53 naquela execução; isso não é promessa de redução percentual no próximo runner. Cada teste de navegador agora informa sua duração e grava `output/playwright/smoke-summary.json`, com scripts realmente executados, resultado, duração e identidade do commit quando disponível. O ganho do seletor e do cache precisa ser medido em runs comparáveis, separando espera, cache, compilação e execução. Não há resultado novo de Windows/Linux instalado atribuído a esta alteração.

## Publicação sem repetir builds aprovados

O workflow `Build Windows and Linux` mantém todos os gates existentes: testes nativos Windows/Linux, os dois manifestos Windows, os cinco instaladores assinados e a atualização instalada real. Os bytes de produção são copiados para `validated-installers` **antes** do E2E, que recompila aplicativos de teste e pode esvaziar as pastas de bundles. Depois do E2E aprovado, os hashes dessa cópia são novamente conferidos, a proveniência é gravada e só então os artefatos são enviados. Builds manuais de branches continuam possíveis, mas não se tornam elegíveis para promoção.

Para publicar um build já concluído, abrir **Promote validated Windows and Linux release** em `main`, informar `source_run_id` e `source_run_attempt` explícitos e escolher `publish_release`. O padrão `false` faz toda a verificação sem publicar; `true` chama o publicador existente. Não há compilação, instalação de dependências ou execução dos binários baixados nessa promoção. O único trabalho pesado que permanece é transferir e conferir os instaladores. Não rodar um segundo build completo apenas para ligar a publicação.

A promoção falha fechada:

1. O GitHub precisa confirmar repositório original `felipevasc/logs`, workflow ativo no caminho exato `.github/workflows/build.yml`, ID desse workflow, run bem-sucedido e última tentativa pedida. Fork, PR, workflow alternativo, branch de trabalho, tentativa antiga ou inconclusiva são rejeitados.
2. Ambos os jobs de plataforma e **cada** etapa obrigatória de build, teste nativo, manifesto, E2E, proveniência e upload precisam estar concluídos com sucesso na mesma tentativa. A ordem e os horários demonstram que o upload veio depois do E2E. Texto autodeclarado no artefato não substitui esses resultados do GitHub.
3. A origem deve ser `main`, ou push de tag da versão que ainda resolva para o commit validado e esteja contido em `main`. O SHA deve ser o de `main` atual ou ter a **árvore Git completa idêntica**, incluindo workflows, scripts e lockfiles. Os bytes de `build.yml` devem coincidir com o checkout confiável. `main` é conferido novamente antes de publicar; avanço, tag movida ou rerun bloqueiam a promoção.
4. Devem existir exatamente os dois artefatos de release esperados, sem duplicatas e sem expiração. IDs de repositório/run, SHA e branch são conferidos nos metadados do GitHub. O SHA-256 do ZIP baixado deve coincidir com o digest do GitHub. A extração rejeita caminhos aninhados/traversal, links, diretórios, nomes duplicados e expansão excessiva; nenhum conteúdo baixado é executado.
5. A proveniência de cada plataforma registra run/tentativa, SHA/árvore, versão, ref, workflow e hashes/tamanhos de todos os bytes. Cada campo é confrontado com evidência independente. `prepare-assets.mjs` conserva a validação dos cinco formatos, nomes, magic bytes, versão assinada, assinatura e manifesto do updater. `publish.mjs` continua inalterado, com as recusas de tag conflitante, versão antiga/já publicada, upload incompleto e feed não servido.
6. O resumo e o artefato `release-promotion-<run>-<attempt>` registram qual build testou os instaladores, o commit/árvore originais, o `main` de aprovação, os IDs/digests dos artefatos e o run de promoção. Se o publicador falhar depois de uma possível publicação, o resultado fica `unconfirmed`, exigindo conferir release e feed antes de repetir. A tag de release e o link “Build e testes” apontam ao commit/run originais.

As duas entradas de publicação compartilham o lock de job `release-publication`, sem cancelamento de uma publicação em andamento. Assim, promoção e release normal não podem alterar o mesmo draft simultaneamente. O cancelamento de builds obsoletos continua por plataforma, sem cancelar o publicador.

O novo job usa apenas `GITHUB_TOKEN` efêmero com `actions: read` e o mesmo `contents: write` já necessário para publicar. Não cria segredo, chave, PAT, credencial persistente ou grant adicional. As permissões de `build.yml` e `checks.yml` não foram ampliadas. O checkout de promoção não persiste credenciais.

Runs anteriores a esta implementação não têm a proveniência pós-E2E e são inelegíveis. Artefato ausente/expirado, versão incompatível ou mudança de qualquer parte da árvore exige um novo build completo solicitado explicitamente; não há fallback silencioso para recompilar. A fila de runners e o tempo do build inicial não desaparecem. A economia esperada está em eliminar a repetição de dezenas de minutos de compilação/testes antes da publicação; a latência real de transferência/verificação precisa ser medida no GitHub.

Os testes rápidos `scripts/tests/artifact-promotion.test.mjs` entram em `npm run check` e nos dois jobs de build. Incluem fixtures maliciosas de metadados, ZIP, árvore, assinatura e tentativa, mais uma promoção de validação ponta a ponta com chave descartável e GitHub simulado. Não substituem a primeira execução real de Windows/Linux após o merge.
