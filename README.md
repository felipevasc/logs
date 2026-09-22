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

## Executar em desenvolvimento

Requisitos: Node.js, Rust e os [requisitos do Tauri 2](https://v2.tauri.app/start/prerequisites/) para Windows ou Linux.

```powershell
npm install
npm run dev
```

`npm run dev` prepara os arquivos estaticos do frontend e abre a aplicacao Tauri. Para gerar uma versao de distribuicao, execute:

```powershell
npm run build
```

Os exemplos menores em `exemplos/` podem ser usados para testar os parsers. O arquivo de carga `grande.jsonl`, saidas de teste e exportacoes de sessoes locais ficam fora do repositorio.

## Builds de Windows e Linux

O workflow [Build Windows and Linux](https://github.com/felipevasc/logs/actions/workflows/build.yml) executa os testes do backend e gera instaladores x64 a cada push de codigo para `main`. Tambem pode ser iniciado pela opcao **Run workflow** no GitHub Actions. Os downloads ficam nos artefatos da execucao por 30 dias:

- **Windows:** instalador `.exe` (NSIS) e `.msi`.
- **Linux:** `.deb`, `.rpm` e `.AppImage`, compilados no Ubuntu 22.04.

Para compilar localmente, instale as dependencias da plataforma, execute `npm ci` e `npm run build`. Os pacotes ficam em `src-tauri/target/release/bundle/`. O build Linux precisa de ambiente Linux; o de Windows precisa de Windows.

Os pacotes ainda nao possuem assinatura de codigo. A leitura nativa de canais Windows e arquivos EVTX requer Windows; os demais formatos de arquivo podem ser analisados nas duas plataformas.

Detalhes da interface, do motor e dos testes estao em [Reformulacao e validacao](docs/repaginacao-e-validacao.md).
