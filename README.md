# Log Insight

Aplicacao desktop para investigar logs, feita com Tauri 2: o backend em Rust indexa e analisa as fontes, enquanto a interface permite explorar, filtrar, agrupar e documentar eventos.

## Recursos

- Carregamento de arquivos de log (inclusive varios arquivos em conjunto) e do Windows Event Log.
- Enriquecimento de codigos com nome e descricao configuraveis.
- Filtros por campos, busca textual, faixas numericas e arvore de facetas.
- Tabela com colunas selecionaveis, redimensionaveis e reordenaveis por arrastar e soltar.
- Visualizacoes persistidas por Caso: o recorte atual e salvo automaticamente; tambem e possivel criar, renomear, atualizar e excluir visualizacoes nomeadas.
- Agrupamentos, agregacoes, paineis, cubo de dados, linha do tempo e trilhas de investigacao.
- Casos de analise com itens, comentarios, anotacoes e restauracao da sessao ao reabrir.
- Descoberta local de padrões: distribuições, mensagens semelhantes, desvios numéricos e mudanças temporais condicionadas a combinações de campos, com amostragem explícita e filtros de evidência.
- Frequências cruzadas, mapa temporal por categoria com círculos de volume e duas medidas numéricas, investigação dos grupos presentes em um pico e Top 10 pelo menu contextual.
- Timeline horizontal com linhas por evento/origem, marcadores proporcionais ao horário, rótulos fixos, zoom e edição de notas/setas pelo mouse ou teclado.
- Notas com 1.994 opções de ícones e marcas, matriz pesquisável, categorias e recursos de TI; exportação de timelines completas em PNG e PDF com nomes amigáveis.
- Resumos e cruzamentos de dados com atalhos, busca, ordenação e paginação.
- Arrays JSON, campos aninhados e normalização de aliases e timestamps; rankings de contagem exatos com armazenamento temporário em disco para alta cardinalidade.
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

`npm run dev` prepara os arquivos estaticos do frontend e abre a aplicacao Tauri. Para gerar uma versao de distribuicao, execute:

```powershell
npm run build
```

Os exemplos menores em `exemplos/` podem ser usados para testar os parsers. O arquivo de carga `grande.jsonl`, saidas de teste e exportacoes de sessoes locais ficam fora do repositorio.

## Downloads e builds de Windows e Linux

A versão publicada, com os instaladores de **Windows e Linux**, fica em [GitHub Releases](https://github.com/felipevasc/logs/releases/latest). Cada Release inclui `.exe`, `.msi`, `.AppImage`, `.deb`, `.rpm` e `SHA256SUMS.txt` para conferir a integridade dos arquivos.

O workflow [Build Windows and Linux](https://github.com/felipevasc/logs/actions/workflows/build.yml) executa os testes do backend e gera instaladores x64 a cada push de codigo para `main`. Tambem pode ser iniciado pela opcao **Run workflow** no GitHub Actions. Os downloads ficam nos artefatos da execucao por 30 dias:

- **Windows:** instalador `.exe` (NSIS) e `.msi`.
- **Linux:** `.deb`, `.rpm` e `.AppImage`, compilados no Ubuntu 22.04.

Para publicar uma versão, alinhe a versão em `package.json`, nos lockfiles, em `Cargo.toml` e em `tauri.conf.json`, e registre as notas em `docs/releases/vX.Y.Z.md`. Inicie o workflow em `main` com **publish_release** habilitado. A publicação só acontece após os testes e builds das duas plataformas: o job confere versão, formato e integridade dos cinco pacotes, envia todos para uma Release em rascunho e só então a publica. Uma tag existente de outro commit ou uma versão já publicada interrompe a operação, sem sobrescrevê-la.

Para compilar localmente, instale as dependencias da plataforma, execute `npm ci` e `npm run build`. Os pacotes ficam em `src-tauri/target/release/bundle/`. O build Linux precisa de ambiente Linux; o de Windows precisa de Windows.

Os pacotes ainda nao possuem assinatura de codigo. A leitura nativa de canais Windows e arquivos EVTX requer Windows; os demais formatos de arquivo podem ser analisados nas duas plataformas.

As funcionalidades novas, seus limites, o contrato das conexões e os benchmarks medidos estão em [Evolução da análise genérica](docs/evolucao-analise-generica.md). O histórico anterior permanece em [Reformulacao e validacao](docs/repaginacao-e-validacao.md).
