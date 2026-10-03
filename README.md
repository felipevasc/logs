# Log Insight

Aplicacao desktop para investigar logs, feita com Tauri 2: o backend em Rust indexa e analisa as fontes, enquanto a interface permite explorar, filtrar, agrupar e documentar eventos.

## Recursos

- Carregamento de arquivos de log (inclusive varios arquivos em conjunto), pacotes `.zip`/`.tar`/`.tgz`, arquivos `.evtx` em Windows e Linux e canais do Windows Event Log.
- Planilhas Excel (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`) e OpenDocument (`.ods`): cada linha de cada aba vira um evento, com cabeçalho detectado mesmo abaixo de títulos, datas e horas das células (inclusive em colunas separadas) e colunas de nível, mensagem, origem e código reconhecidas em português e inglês; uma coluna única com linhas de log coladas é lida pelos formatos de log. CSV e TSV com vírgula, ponto e vírgula (Excel em português), tabulação ou barra vertical, e texto em UTF-16 ou Windows-1252, são detectados automaticamente.
- Consultas em arquivos grandes com motor colunar embutido (DuckDB, sem servidor), páginas independentes das contagens/estatísticas e testes de paridade com o leitor linha a linha. A primeira abertura prepara índices em segmentos reaproveitáveis, com progresso por fase e retomada dos checkpoints concluídos. [Como funciona e espaço em disco](docs/motor-de-consultas.md) · [Validação e benchmark de grandes volumes](docs/desempenho-grandes-volumes.md).
- Página **Comprometimentos** sobre o conjunto completo ou Caso ativo, com cinco níveis de evidência, correlações com vínculos explícitos, táticas MITRE ATT&CK, cobertura por regra e Sigma. Ocorrências semelhantes são agrupadas; trechos destacados levam aos eventos originais e minicards de **Atacante** e **Alvo** mostram as identidades observadas e sua procedência. O Resumo exibe as contagens por nível. “Quase confirmado” pode representar tentativa inequívoca bloqueada; resultados completos ficam em disco e são paginados. [Uso, cobertura e limites](docs/comprometimentos.md), [participantes](docs/participantes.md) e [política de evidências](docs/deteccao-evidencias.md).
- Linguagem de busca com campos, comparações, redes CIDR, listas, regex e `AND`/`OR`/`NOT`, com validação e complemento enquanto digita; campos canônicos (`@user`, `@src_ip`, `@host`, `@action`…) que atravessam todas as fontes. [Busca, entidades e detecções](docs/busca-e-deteccoes.md).
- Menu de entidades em usuários, IPs, hosts e processos, paleta de comandos (Ctrl+K) e detalhe do registro com ação normalizada, regras e conteúdo decodificado.
- Gerenciador de recursos com histórico de CPU, RAM, I/O, processos e ações do aplicativo, comparados à capacidade do sistema; cotas e estimativas identificadas separadamente. [Medidas e limites](docs/resource-manager.md).
- Valores sensíveis ocultos por padrão com opção de revelação temporária no detalhe; inspetor local de JSON, JWT e Base64 com subcampos e preservação de números sem arredondamento. A decodificação de um token não verifica sua assinatura.
- Tamanho da interface automático para janelas pequenas e telas com escala do sistema, ajustável em Configurações → Interface ou com Ctrl + / Ctrl − / Ctrl 0.
- Três áreas no rodapé do menu, cada uma com sua cor: Análise (amarelo), Caso (roxo) e Estrutura (laranja: arquivos, conexões e abertura de logs). Menus e abas mostram quando estão carregando; o status lista o que está em andamento e permite cancelar cada item.
- Indicadores, hipóteses e cadeia de custódia (SHA-256) no Caso, incluídos nos relatórios PDF e Markdown.
- Enriquecimento de codigos com nome e descricao configuraveis.
- Filtros por campos, busca textual, faixas numericas e arvore de facetas.
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
- Atualização pelo GitHub: ao abrir, o aplicativo verifica se há versão nova e pergunta antes de baixar e instalar. [Como funciona e como publicar](docs/atualizacoes.md).

## Executar em desenvolvimento

Requisitos: Node.js, Rust e os [requisitos do Tauri 2](https://v2.tauri.app/start/prerequisites/) para Windows ou Linux.

```powershell
npm install
npm run dev
```

Para testar a interface no navegador com dados de demonstração, execute `npm run preview` e abra `http://127.0.0.1:4173`. Após atualizar o código com uma página já aberta, use **Ctrl+F5**. O preview simula as operações nativas; não substitui o aplicativo Tauri para importar ou investigar dados reais.

`npm run dev` prepara os arquivos estaticos do frontend e abre a aplicacao Tauri. Para gerar uma versao de distribuicao, execute:

```powershell
npm run build
```

Os exemplos menores em `exemplos/` podem ser usados para testar os parsers. O arquivo de carga `grande.jsonl`, saidas de teste e exportacoes de sessoes locais ficam fora do repositorio.

## Downloads e builds de Windows e Linux

A versão publicada, com os instaladores de **Windows e Linux**, fica em [GitHub Releases](https://github.com/felipevasc/logs/releases/latest). Cada Release inclui `.exe`, `.msi`, `.AppImage`, `.deb`, `.rpm`, as assinaturas do atualizador (`.sig`), o manifesto `latest.json` e `SHA256SUMS.txt` para conferir a integridade dos arquivos.

A partir da 0.6.0 o LogInsight se atualiza: ao abrir, consulta a última Release e, se houver versão nova, pergunta antes de baixar e instalar. Prefira o `.exe` no Windows e o `.AppImage` no Linux, que atualizam sem senha de administrador; `.msi`, `.deb` e `.rpm` também atualizam, mas pedem a senha. Quem usa a 0.5.x instala a 0.6.0 manualmente uma vez. Detalhes em [Atualizações](docs/atualizacoes.md).

As verificações de PR usam [validação essencial e por área alterada](docs/ci-validation.md): testes rápidos sempre, navegador essencial mais subsistemas afetados e suíte nativa completa quando Rust/build/instalador mudam. O build portátil Windows é opcional; o disparo manual permite regressão completa.

O workflow [Build Windows and Linux](https://github.com/felipevasc/logs/actions/workflows/build.yml) executa os testes do backend e gera instaladores x64 a cada push de codigo para `main`. Tambem pode ser iniciado pela opcao **Run workflow** no GitHub Actions. Os downloads ficam nos artefatos da execucao por 30 dias:

- **Windows:** instalador `.exe` (NSIS) e `.msi`.
- **Linux:** `.deb`, `.rpm` e `.AppImage`, compilados no Ubuntu 22.04.

Para publicar uma versão, qualquer pessoa com permissão de escrita no repositório executa `npm run release -- X.Y.Z` (alinha a versão nos cinco arquivos, usa as notas de `docs/releases/vX.Y.Z.md`, cria o commit e a tag) e envia com `git push --follow-tags origin HEAD:main`. A tag inicia o workflow: testes, instaladores das duas plataformas, teste de atualização ponta a ponta, assinatura e manifesto. O job de publicação confere versão, formato, assinatura e integridade dos pacotes, envia tudo para uma Release em rascunho e só então a publica; em seguida confere se o manifesto já é servido aos aplicativos instalados. Uma tag de commit fora da `main`, de outro commit ou de versão já publicada ou menor que a atual interrompe a operação, sem sobrescrever nada. O workflow também pode ser iniciado em `main` com **publish_release** habilitado.

Para compilar localmente, instale as dependencias da plataforma, execute `npm ci` e `npm run build`. Os pacotes ficam em `src-tauri/target/release/bundle/`. O build Linux precisa de ambiente Linux; o de Windows precisa de Windows.

Os instaladores ainda não têm assinatura de código do Windows (Authenticode); as atualizações são conferidas pela assinatura do atualizador. A leitura de canais ao vivo do Windows Event Log requer Windows; arquivos `.evtx` e os demais formatos podem ser analisados nas duas plataformas.

As funcionalidades novas, seus limites, o contrato das conexões e os benchmarks medidos estão em [Evolução da análise genérica](docs/evolucao-analise-generica.md). O histórico anterior permanece em [Reformulacao e validacao](docs/repaginacao-e-validacao.md).
