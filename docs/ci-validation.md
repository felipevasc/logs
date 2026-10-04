# Validação essencial e latência de publicação

## Política vigente na 0.13

A pedido do usuário, a seleção automática foi reduzida aos contratos necessários. Esta seção substitui a política histórica registrada abaixo.

- `npm run check`: parse de JavaScript, 29 arquivos de testes essenciais (308 verificações), versões coordenadas e salvaguardas de publicação. O manifesto é `scripts/ci/essential-tests.json`.
- `npm run test:preview`: oito fluxos integrados: isolamento de Casos/índices/preferências, conexões, abertura nativa simulada, Timeline, arquivo de exclusões, campos transformados, investigação e atualização. Os quatro essenciais mais os módulos afetados são selecionados em mudanças locais; mudanças compartilhadas usam os oito.
- `npm run test:native`: 24 verificações unitárias selecionadas de publicação, cancelamento, recursos, isolamento, coleta remota e índices, mais os fluxos de paridade/build do motor, recuperação de metadados e importação estruturada. Windows/Linux executam a mesma seleção serial.
- `npm run test:extended` e `node scripts/ci/native-essential.mjs --extended`: testes especializados restantes sob demanda. Testes individuais de navegador podem ser passados a `npm run test:preview -- test-NOME.mjs`.
- Quatorze arquivos de fixtures, snapshots e verificações de apresentação redundantes foram retirados. A seleção foi consolidada em três contratos do plano de CI, em vez de uma matriz extensa de combinações equivalentes.
- Assinaturas, checksums, proteção de tags/versões, doze assets, instaladores, atualização instalada e aceite do executável real continuam obrigatórios para a release. Prévia simulada não substitui WebView/IPC reais.

Um gate selecionado que falha, é cancelado ou não executa bloqueia a validação. O seletor registra os caminhos e scripts em `validation-plan.json`; o navegador registra resultado e duração em `smoke-summary.json`. O limite do job do navegador é 20 minutos, com 150 segundos por script. A redução é de trabalho executado, sem promessa de duração dos runners ou da compilação Rust.

## Histórico até a 0.12.1

O registro a seguir explica medições e decisões anteriores; referências à suíte completa e ao limite de 40 minutos não são a política vigente.

## O que é obrigatório

- Em todo PR: parse de JavaScript, **todos** os testes Node, versão e salvaguardas de release (`npm run check`). Os testes de integridade, assinatura, recusa de sobrescrita e preservação de dados continuam presentes.
- Mudança de módulo de interface: seis fluxos essenciais no Chromium, mais os testes dos subsistemas alterados. Os essenciais cobrem navegação/cancelamento, responsividade, salvamento antes da atualização e retomada, referências do Caso, abertura nativa simulada e Timeline nativa simulada. Não são validação do WebView instalado.
- Mudança de código compartilhado (`app.js`, `index.html`, `styles.css`, `analysis-context.js`, `performance-core.js`): navegador completo; não recompila Rust. Esses arquivos atravessam vários subsistemas.
- Mudança Rust, dependência raiz, instalador, publicação, build ou CI: suíte nativa completa Windows/Linux, serial, inclusive em PR rascunho; navegador completo. Não há corte de testes nativos relevantes.
- Caminho desconhecido ou comparação ausente: matriz completa. Falha ao calcular a comparação bloqueia a validação. O diff inclui remoções e os dois lados de renomeações.
- Apenas documentação: as verificações Node/versão/release continuam; navegador e nativo são declarados não aplicáveis.
- `Required validation` só passa quando todos os gates selecionados passaram. Cancelamento, falha ou um gate obrigatório pulado bloqueia o resultado. O relatório de escopo e `validation-plan.json` registram exatamente a seleção.

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

## Orçamento da regressão integrada 0.12.1

O [Checks #84](https://github.com/felipevasc/logs/actions/runs/37132233211), no
commit `9205733`, chegou ao limite anterior de 20 minutos perto do último
teste. A integração ampliou a seleção para 51 scripts e a coleta de evidências
também usa tempo do job. Os jobs de navegador do PR e da release passam a ter
40 minutos; nenhum teste ou gate foi removido. O limite individual de 150
segundos, a limpeza de processos e a reprovação por falha continuam ativos.

A mesma execução expôs uma corrida no teste novo do inspetor: linhas já
carregadas antecedem a confirmação persistida da fonte e sua navegação final,
que fecha o detalhe. O teste agora aguarda a liberação da ação de carregar após
essa confirmação. Uma regressão com respostas controladas cobre a janela
intermediária; o fechamento e o restabelecimento da proteção do produto não
foram alterados.

## Uma só sequência de release

A publicação baixa os instaladores dos dois jobs da mesma execução, confere assinaturas/checksums e só publica depois de ambos passarem, incluindo atualização instalada real. A integração 0.12.1 também exige o job de regressão completa da interface e, no Windows, o aceite do executável Tauri com IPC real: paridade de consultas/exportações, reuso de índices, recursos, inspeção e persistência. O teste nativo não participa do executor de prévia simulada. A tag disparadora é verificada antes da instalação npm e novamente no publicador.

Evitar rodar uma validação manual completa com `publish_release=false` e depois repetir o mesmo build com `publish_release=true`. Quando a publicação estiver autorizada e o commit final estiver em `main`, disparar uma única execução com publicação habilitada (ou uma única tag). O workflow ainda roda automaticamente em push de `main`; não disparar outro build simultâneo sem necessidade.

### Próxima etapa: promover artefatos já validados

Uma promoção entre execuções deve falhar fechada e nunca recompilar silenciosamente:

1. Receber ID explícito de um run **deste repositório**, da definição `build.yml`, com conclusão `success`, ambos os jobs de plataforma e a atualização instalada concluídos.
2. Exigir origem autorizada (`main` ou tag já contida em `main`); nunca aceitar artefato de fork/PR. Confirmar SHA da execução. Para qualquer equivalência pós-merge, exigir árvore Git idêntica e registrar os dois SHAs, não apenas a versão.
3. Exigir exatamente os artefatos esperados, não expirados, acompanhados de identidade/proveniência com SHA/árvore, versão, plataforma, workflow e run. Conferir essa identidade com o GitHub e o checkout.
4. Revalidar bytes, versão assinada, assinatura, conjunto completo de formatos e SHA-256 pelo mesmo `prepare-assets.mjs`. Nome de artefato, cache hit ou texto de log não substitui essa prova.
5. Conservar proteção contra tag conflitante, versão publicada/antiga, commit fora de `main`, upload incompleto e feed não servido. Registrar qual run validou os instaladores e qual os publicou.
6. Artefato ausente/expirado, origem ambígua, identidade divergente, job não executado ou falha de assinatura bloqueia; pedir um novo build completo explicitamente.

Ler artefatos de outro run com `actions/download-artifact` exige `actions: read` no token efêmero do job de promoção. O job ainda precisa do `contents: write` já usado na publicação. Não exige segredo novo, chave nova, credencial persistente ou acesso a outro repositório. Essa etapa é proposta separada e não foi implementada neste lote.
