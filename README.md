# Log Insight

Aplicacao desktop para investigar logs, feita com Tauri 2: o backend em Rust indexa e analisa as fontes, enquanto a interface permite explorar, filtrar, agrupar e documentar eventos.

## Recursos

- Carregamento de arquivos de log (inclusive varios arquivos em conjunto), pacotes `.zip`/`.tar`/`.tgz`, arquivos `.evtx` em Windows e Linux e canais do Windows Event Log.
- Página **Comprometimentos** sobre o conjunto completo ou Caso ativo, com cinco níveis de evidência, correlações com vínculos explícitos, táticas MITRE ATT&CK, cobertura por regra e Sigma. Ocorrências semelhantes são agrupadas; trechos destacados levam aos eventos originais e minicards de **Atacante** e **Alvo** mostram as identidades observadas e sua procedência. O Resumo exibe as contagens por nível. “Quase confirmado” pode representar tentativa inequívoca bloqueada; resultados completos ficam em disco e são paginados. [Uso, cobertura e limites](docs/comprometimentos.md), [participantes](docs/participantes.md) e [política de evidências](docs/deteccao-evidencias.md).
- Linguagem de busca com campos, comparações, redes CIDR, listas, regex e `AND`/`OR`/`NOT`, com validação e complemento enquanto digita; campos canônicos (`@user`, `@src_ip`, `@host`, `@action`…) que atravessam todas as fontes. [Busca, entidades e detecções](docs/busca-e-deteccoes.md).
- Menu de entidades em usuários, IPs, hosts e processos, paleta de comandos (Ctrl+K) e detalhe do registro com ação normalizada, regras e conteúdo decodificado.
- Valores sensíveis ocultos por padrão no detalhe, com revelação temporária e reversível. O inspetor local apresenta subcampos de JSON, JWT e Base64 UTF-8 em árvore, com cópia explícita de caminho ou valor. A interpretação não verifica assinatura/autenticidade nem executa conteúdo; limita-se a 256 KiB, 1.000 nós, profundidade 12 e três transformações. Fechar ou trocar de registro oculta novamente; exportações mantêm a proteção padrão.
- Tamanho da interface automático para janelas pequenas e telas com escala do sistema, ajustável em Configurações → Interface ou com Ctrl + / Ctrl − / Ctrl 0.
- Três áreas no rodapé do menu, cada uma com sua cor: Análise (amarelo), Caso (roxo) e Estrutura (laranja: arquivos, conexões e abertura de logs). Menus e abas mostram quando estão carregando; o status lista o que está em andamento e permite cancelar cada item.
- Indicadores, hipóteses e cadeia de custódia (SHA-256) no Caso, incluídos nos relatórios PDF e Markdown.
- Enriquecimento de codigos com nome e descricao configuraveis.
- Filtros por campos, busca textual, faixas numericas e arvore de facetas.
- Modo Big Data habilitável por Caso, com índices invertidos embarcados, cache em disco e confirmação dos filtros originais. [Arquitetura, uso e benchmark](docs/big-data.md).
- Gerenciador de recursos com CPU, RAM, I/O, processos próprios, inventário interno e histórico de 15 minutos, comparados à capacidade da máquina. [Medições e limites](docs/resource-manager.md).
- Tabela com colunas selecionaveis, redimensionaveis e reordenaveis por arrastar e soltar.
- Visualizacoes persistidas por Caso: o recorte atual e salvo automaticamente; tambem e possivel criar, renomear, atualizar e excluir visualizacoes nomeadas.
- Agrupamentos, agregacoes, paineis, cubo de dados, linha do tempo e trilhas de investigacao.
- Casos de analise com itens, comentarios, anotacoes e restauracao da sessao ao reabrir.
- Descoberta local de padrões: distribuições, mensagens semelhantes, desvios numéricos e mudanças temporais condicionadas a combinações de campos, com amostragem explícita e filtros de evidência.
- Frequências cruzadas, mapa temporal por categoria com círculos de volume e duas medidas numéricas, investigação dos grupos presentes em um pico e Top 10 pelo menu contextual.
- Timeline horizontal com linhas por evento/origem, marcadores proporcionais ao horário, rótulos fixos, zoom e edição de notas/setas pelo mouse ou teclado; separação opcional da atividade por origem, usuário, IP ou host.
- Notas com 1.994 opções de ícones e marcas, matriz pesquisável, categorias e recursos de TI; exportação de timelines completas em PNG e PDF com nomes amigáveis.
- Resumos e cruzamentos de dados com atalhos, busca, ordenação e paginação.
- Arrays JSON, envelopes (CloudTrail, `hits.hits`, `value`…), Zeek, Suricata EVE, auditd, Okta, GCP e Kubernetes audit, campos aninhados e normalização de aliases e timestamps; rankings de contagem exatos com armazenamento temporário em disco para alta cardinalidade.
- Conexões Elasticsearch e Kibana Console com autenticação Basic, consulta por período/Query DSL e importação paginada para uma cópia local.
- Catálogo editável com 378 sinais textuais de ameaças, incluindo tentativas, bloqueios, saídas de comandos e conteúdo exposto (arquivos de sistema, configurações, dumps e credenciais). [Cobertura, interpretação e falsos positivos](docs/regras-ameacas.md).
- Jornadas por identificador exato entre fontes, com ordem temporal, duração observada e contagens; investigação por usuário/IP delimitada por período.

## Executar em desenvolvimento

Requisitos: Node.js, Rust e os [requisitos do Tauri 2](https://v2.tauri.app/start/prerequisites/) para Windows ou Linux.

```powershell
npm install
npm run dev
```

Para testar a interface no navegador com dados de demonstração, execute `npm run preview` e abra `http://127.0.0.1:4173`. Após atualizar o código com uma página já aberta, use **Ctrl+F5**. O preview simula as operações nativas; não substitui o aplicativo Tauri para importar ou investigar dados reais.

Para executar as regressões, use `npm run check`. A suíte completa de interface
usa `npm ci --prefix scripts/preview`, a instalação do Chromium com
`npx --prefix scripts/preview playwright install chromium` e `npm run test:preview`.
O runner usa Chromium por padrão e respeita `PLAYWRIGHT_CHANNEL` quando definido.
No Linux, as regressões de processos e o runner também precisam de Python 3.9+
e kernel com pidfd (Ubuntu 22.04 atende); no Windows, usam PowerShell 5.1.
O runner inicia um servidor em uma porta livre, executa os cenários e grava o
resumo em `output/playwright/smoke-summary.json`. Um cenário específico pode ser
executado com `npm run test:preview -- test-big-data.mjs`. Em timeout ou interrupção,
ele encerra e aguarda apenas a árvore criada para aquele teste antes de continuar.
Se não conseguir confirmar a limpeza, interrompe a suíte com erro.

No Windows, o teste do desktop real usa
`node scripts/preview/test-native-desktop.mjs src-tauri/target/release/loginsight.exe`
depois da compilação. Ele cria dados e perfil WebView isolados em `output/`,
verifica consultas, exportações, recursos, revelação de valores, inspeção de JWT,
fechamento e reabertura por IPC real.
Exige WebView2 Evergreen Runtime, uma sessão desktop interativa e as dependências
de `scripts/preview`; não precisa baixar Chromium, pois conecta ao WebView2 por CDP.
Não aponta para a configuração pessoal do aplicativo. O acompanhamento desta
revisão está em [Prontidão da entrega](docs/release-readiness.md).

As regressões de substituição atômica de metadados no Windows também podem ser
executadas com `cargo test --manifest-path src-tauri/Cargo.toml --release --locked --test atomic_metadata_windows -- --test-threads=1`.
Esse alvo usa o mesmo código de produção e complementa a suíte completa do backend.

`npm run dev` prepara os arquivos estaticos do frontend e abre a aplicacao Tauri. Para gerar uma versao de distribuicao, execute:

```powershell
npm run build
```

Os exemplos menores em `exemplos/` podem ser usados para testar os parsers. O arquivo de carga `grande.jsonl`, saidas de teste e exportacoes de sessoes locais ficam fora do repositorio.

## Downloads e builds de Windows e Linux

A versão publicada, com os instaladores de **Windows e Linux**, fica em [GitHub Releases](https://github.com/felipevasc/logs/releases/latest). Cada Release inclui `.exe`, `.msi`, `.AppImage`, `.deb`, `.rpm` e `SHA256SUMS.txt` para conferir a integridade dos arquivos.

O workflow [Build Windows and Linux](https://github.com/felipevasc/logs/actions/workflows/build.yml) executa as regressões Node, a suíte de interface, os testes do backend e gera instaladores x64 a cada push para `main`. Após compilar Windows, executa também o teste nativo; sua falha bloqueia a publicação. Também pode ser iniciado pela opção **Run workflow** no GitHub Actions. Os downloads ficam nos artefatos da execução por 30 dias:

- **Windows:** instalador `.exe` (NSIS) e `.msi`.
- **Linux:** `.deb`, `.rpm` e `.AppImage`, compilados no Ubuntu 22.04.

O job Windows usa a imagem [windows-2022](https://github.com/actions/runner-images/blob/main/images/windows/Windows2022-Readme.md). Ele verifica a sessão interativa e o WebView2 separadamente do navegador Edge, seguindo a [detecção oficial da Microsoft](https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution). Se faltar o runtime, instala o bootstrapper oficial após validar sua assinatura Microsoft. A versão encontrada, logs e resultados nativos são guardados em `native-windows-validation` por 14 dias, inclusive nas falhas; as evidências do navegador ficam em `browser-validation`. Esses artefatos não entram nos instaladores publicados.

Para publicar uma versão, alinhe a versão em `package.json`, nos lockfiles, em `Cargo.toml` e em `tauri.conf.json`, e registre as notas em `docs/releases/vX.Y.Z.md`. Inicie o workflow em `main` com **publish_release** habilitado, ou envie a tag `vX.Y.Z` correspondente. Uma tag de gatilho diferente da versão declarada é rejeitada antes de instalar dependências ou compilar, e o publicador repete essa checagem antes de acessar a API. A publicação só acontece após os testes e builds das duas plataformas: o job confere versão, formato e integridade dos cinco pacotes, envia todos para uma Release em rascunho e só então a publica. Uma tag existente de outro commit ou uma versão já publicada interrompe a operação, sem sobrescrevê-la.

Para compilar localmente, instale as dependencias da plataforma, execute `npm ci` e `npm run build`. Os pacotes ficam em `src-tauri/target/release/bundle/`. O build Linux precisa de ambiente Linux; o de Windows precisa de Windows.

Os pacotes ainda nao possuem assinatura de codigo. A leitura de canais ao vivo do Windows Event Log requer Windows; arquivos `.evtx` e os demais formatos podem ser analisados nas duas plataformas.

As funcionalidades novas, seus limites, o contrato das conexões e os benchmarks medidos estão em [Evolução da análise genérica](docs/evolucao-analise-generica.md). O histórico anterior permanece em [Reformulacao e validacao](docs/repaginacao-e-validacao.md).
