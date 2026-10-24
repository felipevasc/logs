# Exploração local: entrega integrada

Escopo aprovado: visualização direta de arquivos grandes; contexto e destaques;
console SQL; pesquisa e extração por expressão (texto literal, Rust e PCRE2); mapas declarativos de enriquecimento; árvores de processos. A timeline
existente permanece como está. Integrações, monitoramento e entradas de rede não
fazem parte desta entrega.

## Contratos

- Resultados e sessões pertencem ao Caso, revisão de configuração, visibilidade
  e geração da fonte capturados. Uma troca invalida os resultados anteriores.
- O visualizador de arquivos abre sem parsing, contagem integral ou preparação do DuckDB
  e nunca altera a análise por conta própria.
  Índices e resultados são incrementais e ficam em disco; páginas são limitadas.
- Uma busca incompleta, cancelada ou que atingiu um limite nunca equivale a zero
  ocorrências. O leitor informa cobertura, limites e trechos omitidos.
- Contexto pode ignorar filtros de apresentação, mas nunca exclusões do Caso.
- SQL consulta apenas `events` e CTEs da própria consulta. Exportação percorre o
  resultado completo no processo nativo; números exatos atravessam IPC como texto.
- Rust regex continua sendo a opção rápida. PCRE2 é explícito, limitado e cancelável;
  os intervalos nativos são a autoridade para os destaques e o PCRE2 embarcado é a
  autoridade sobre a validade de uma expressão. Uma expressão aplicada a Eventos é
  um filtro real (campo, motor e opções), não uma filtragem visual.
- Enriquecimento preserva o original, registra mapa/versão/regra e resolve
  especificidade de provedor, canal, código e versão de evento antes de prioridade.
- Árvores distinguem relações explícitas, inferidas, ausentes e ambíguas. PID sem
  máquina e intervalo de vida nunca identifica sozinho uma instância de processo.

## Uso

O menu **Explorar** reúne **Eventos**, **SQL** e **Processos**. O
visualizador de texto fica em **Estrutura → Visualizar arquivos** e os mapas de
enriquecimento em **Estrutura → Extração e normalização**
([guia](extracao-normalizacao.md)); a pesquisa por
expressão e a extração de campos ficam em **Eventos**. Não há mais as abas **Texto** e
**Regex**. A timeline continua usando os controles anteriores.

### Visualizar arquivos (Estrutura)

**Estrutura → Visualizar arquivos** examina o texto original de um arquivo antes de
qualquer preparação de eventos. A tela abre pelo menu de Estrutura, pelo botão
**Visualizar arquivos** da tela inicial ou pela ação **Visualizar arquivo** do menu
de cada linha de **Estrutura → Arquivos**. Escolher o arquivo nunca o carrega na
análise e nunca substitui a fonte que já está sendo investigada.

O cabeçalho descreve somente este arquivo: caminho, tamanho, linhas lidas, cobertura
da leitura e da pesquisa, estado (lendo, pausado, concluído) e codificação, com
aviso quando bytes inválidos em UTF-8 foram substituídos. O número de eventos da
análise não aparece aqui.

A coluna esquerda mostra as correspondências e a direita mostra a origem. F3/Shift+F3
percorrem resultados, Ctrl+G seleciona a linha e Alt+← volta à posição anterior. O
minimapa mostra ocorrências por posição aproximada; a área hachurada identifica
cobertura ainda pendente. Regras de texto, Rust regex ou PCRE2 podem receber rótulos
e cores próprios; as cores de erro e aviso vêm como regras editáveis. **Expressão
assistida…** abre o mesmo editor de **Eventos**, com teste em amostra, e pesquisa o
arquivo ao aplicar (cada linha física é testada separadamente).

**Abrir na análise** faz a transição. Se o arquivo já faz parte da fonte ativa, a
análise apenas abre o evento da linha selecionada. Se há outra fonte aberta e o
arquivo não está nela, uma pergunta oferece **Unir à análise atual**, **Substituir a
análise** ou **Cancelar**; nada muda sem a escolha. Um clique no número de uma linha
abre o evento estruturado que contém aquele offset, quando o arquivo está na
análise e o evento é visível; sem isso o visualizador explica por que não há vínculo.

O visualizador recusa Casos com exclusões ativas (não contorna a visibilidade), lê em
etapas canceláveis e mantém índices e resultados em disco. UTF-16 e formatos
binários ou compactados devem ser abertos pela análise, que já converte. Páginas
salvas por versões anteriores (`text` e `regex`) abrem em Visualizar arquivos e em
Eventos, respectivamente; consultas, regras e campos salvos são preservados.

### Contexto dos eventos

Em **Explorar → Eventos**, abra um evento e escolha **Ver contexto** (ou a aba
**Contexto** da gaveta de detalhes). O painel lateral mostra os registros vizinhos
na mesma fonte, uma janela de tempo ± N segundos ou os eventos que compartilham o
valor de um identificador, e o texto exibido pode ser a mensagem, o registro
completo ou a linha física. Os filtros de apresentação são ignorados; as exclusões e
a visibilidade obrigatória continuam valendo. Cada linha mantém a referência
original do evento e abre o seu detalhe. O contexto acompanha a investigação: ao
passar para outro evento com a aba aberta, ele é recarregado, e respostas atrasadas
de um evento, fonte ou Caso anterior são descartadas. O contexto do arquivo bruto
continua no visualizador de Estrutura; Eventos mostra o contexto dos eventos
investigados.

### SQL

**Atualizar campos** mostra nomes e tipos; clique para inserir um identificador ou
use Ctrl+Espaço. Ctrl+Enter executa. Consultas e histórico ficam nas preferências
do Caso e da área. A caixa **Aplicar filtros atuais** é opcional; os resultados
informam quantos filtros foram capturados.

`events` expõe `id`, `event_ref`, `timestamp` (milissegundos UTC), os campos padrão
e os campos dos logs. Campos dinâmicos são texto; `TRY_CAST` permite tipos SQL
explícitos. `raw` fornece o registro original também no caminho colunar: sua leitura
é feita sob demanda quando a consulta o utiliza, respeitando a população visível.
Consultas que usam apenas os campos preparados dispensam essa leitura.

O editor traz exemplos de CTEs, agregações, ranking e janelas de tempo. O resultado
é materializado uma vez e paginado; cancelar não substitui o último resultado
concluído. Números e decimais atravessam a interface como representação textual
exata. **Exportar tudo** percorre o snapshot inteiro no processo nativo, com
publicação atômica no destino. JSONL inclui valores textuais e seus tipos; SQL NULL
permanece null. CSV usa campos entre aspas e representação textual das células.
Resultados com `id` e `event_ref` podem abrir a origem, após verificação nativa.
As funções SQL `regexp_*` seguem os recursos do DuckDB. Para usar PCRE2 em uma
consulta, aplique a expressão em **Eventos** (com “Aplicar filtros atuais” ativo
no SQL) ou extraia um campo por captura e consulte esse campo.

Uma AST validada permite SELECT e CTEs sobre `events`; arquivos externos, tabelas
internas, extensões, SQL dinâmico e funções fora da lista de análise são recusados.
O DuckDB usa os limites do motor e spill; a recuperação por eventos usa um snapshot
com memória limitada a até 64 MiB e até 4 GiB de spill/dados de entrada.

### Pesquisa por expressão em Eventos

**Explorar → Eventos → Expressão** abre um editor delimitado, separado da
linguagem de busca (parênteses, barras e operadores da expressão nunca são lidos
como operadores da consulta). Escolha o **campo** (mensagem, conteúdo bruto ou outro
campo), o **motor** (texto literal, regex Rust ou PCRE2), a sensibilidade a
maiúsculas e, em **Avançado**, `^`/`$` por linha, ponto incluindo quebra de linha e
o escopo (registro completo ou linha física). O motor nunca é trocado em silêncio.

Rascunho, teste e aplicação são etapas separadas:

1. **Rascunho:** digitar ou colar não pesquisa nada. A expressão é validada pelo
   PCRE2/Rust embarcado, com *debounce* e uma requisição por vez; uma resposta de
   uma versão anterior nunca atualiza o editor atual e a prévia antiga é marcada.
2. **Teste:** a amostra editável (ou o texto completo do evento aberto, obtido pelo
   caminho nativo) mostra ocorrências, capturas com nome, número, valor e posição, e
   diagnósticos de compilação (com posição) e de execução. Testa só a amostra; “nenhuma
   ocorrência nesta amostra” não diz nada sobre o conjunto.
3. **Aplicação:** **Aplicar ao conjunto de eventos** adiciona um filtro `search` ao
   estado real de filtros (chip editável). Página, contagem, agrupamentos,
   exportação, Resumos, SQL com “aplicar filtros atuais” e as ações sobre a seleção
   usam o mesmo conjunto. Uma expressão inválida, ou que atinge um limite do motor,
   não é aplicada e nunca vira “zero resultados”; uma falha durante a consulta
   interrompe a consulta com a mensagem do motor.

O filtro carrega campo, motor e opções. `_all` não é aceito: campos não são
concatenados, para que uma expressão não atravesse a fronteira entre eles. Rust e
texto literal rodam como teste de coluna no motor colunar; PCRE2 é verificado nativamente
nos candidatos, sem enviar eventos ou IDs ao navegador. O contexto obrigatório de
Caso, revisão, fonte e visibilidade é preservado.

### Editor assistido

A expressão é um texto livre e a única fonte da verdade. O editor é uma área de
texto nativa (cursor, seleção, composição de texto, copiar/colar e desfazer/refazer
do sistema) com uma camada visual abaixo dela, reconstruída a cada alteração sem
mexer no cursor. Nada é normalizado, reformatado nem “corrigido”. Cada ação visual é
uma única substituição mínima, portanto um passo de desfazer.

As ações ficam em **Ajudar a montar** (ou botão direito, a tecla de menu e
Ctrl+Espaço, sem depender do mouse):

| Ação | Origem da seleção | Resultado |
| --- | --- | --- |
| Encontrar este texto | exemplo | `a+b(1).log` vira `a\+b\(1\)\.log` (texto literal, escapado conforme o contexto; sem escape no motor literal) |
| Encontrar o que vem depois | exemplo ou expressão | `user=` + Palavra → `(?<=user=)\w+` |
| Encontrar o que vem antes | exemplo ou expressão | valor + `(?=sufixo)` |
| Encontrar entre dois trechos | exemplo ou expressão | `(?<=prefixo)valor(?=sufixo)` |
| Capturar trecho | exemplo ou expressão | `(?<nome>…)` ou `(…)`, com generalização só se você escolher |
| Repetir valor capturado | cursor | `\k<usuario>` (ou `\1`, `\g{1}` antes de um dígito) |
| Renomear / ir à definição / ver referências / salvar como campo | grupo sob o cursor | ver abaixo |

Uma seleção em **texto de exemplo** é sempre literal. Uma seleção na **própria
expressão** é sintaxe e só vira grupo ou condição quando cobre tokens inteiros (a
seleção que corta um escape, classe, grupo ou quantificador é recusada com o motivo).
O tipo de valor é Palavra (`\w+`), Número (`-?\d+(?:[.,]\d+)?`), até espaço (`\S+`),
até um delimitador (`[^;\r\n]+`; vários caracteres usam `(?:(?!&&)[^\r\n])+`, nunca
uma classe) ou uma expressão sua (agrupada em `(?:…)` se tiver `|` no nível de
topo). A ajuda mostra a expressão, a explicação (“Exige user= antes do valor, sem
incluir esse trecho no resultado.”) e um teste nativo na amostra antes de inserir. Um
prefixo é sempre texto literal escapado, portanto de comprimento limitado.

Inserir nunca substitui a expressão sem pedir: com texto no editor, os botões são
**Inserir no cursor**, **Acrescentar ao final** e **Substituir a expressão**. Se a
expressão gerada exige PCRE2 (lookbehind, lookahead, backreference) e o motor atual é
outro, a ajuda avisa e oferece **… e usar PCRE2** ou **Inserir mantendo o motor atual**.
Uma ação sobre uma seleção antiga é recusada se o conteúdo, a expressão ou o Caso
mudaram depois dela.

#### Grupos, condições e referências

O analisador é estrutural (escapes, classes `[…]`, `\Q…\E`, comentários `(?#…)` e do
modo `(?x)`, grupos aninhados, nomeados e numerados, `(?:…)`, atômicos, condicionais,
verbos, opções `(?i)`/`(?n)`, referências nomeadas e numéricas, relativas e chamadas
de subrotina). Ele é tolerante a expressões incompletas e nunca usa o RegExp do
navegador. Exemplos que não criam grupo: `\(`, `[()]`, `(?:abc)`, `(?<=user=)`,
`(?<usuario>\w+)` (cria um), `\Q(a+b)\E`.

- **Capturas:** cada grupo tem uma cor, um sublinhado diferente, número e nome (legenda
  sob o editor, com o valor observado na amostra e a contagem de referências). A cor
  é estável durante pequenas edições; com mais de oito grupos as cores se repetem e a
  legenda e o foco identificam cada um. O grupo e suas referências compartilham a cor:
  `user=(\w+)\s+confirmado=\1\b` e `user=(?<usuario>\w+)\s+confirmado=\k<usuario>\b`.
  Ao posicionar o cursor ou passar o mouse, o grupo e as referências são destacados
  com uma explicação curta; **Alt+↓/↑** alterna entre a definição e as referências.
- **Condições de contexto (lookbehind/lookahead):** fundo listrado suave, sem reduzir a
  opacidade do texto. Explicação: “participa do teste, mas não é consumida como parte
  do trecho encontrado”. Uma captura dentro da condição mantém sua identidade.
- **Backreference × subrotina:** `\1`, `\k<n>`, `\g{n}`, `(?P=n)` exigem o mesmo
  texto capturado; `(?1)`, `(?&n)`, `\g<n>` repetem a regra e são marcadas à parte.
- **Referências inválidas:** indicação de erro com a explicação (“Não existe o grupo
  2”) e a lista de capturas válidas em **Repetir valor capturado**; o motor nativo
  também recusa a expressão e ela não é aplicada.
- **Numeração:** inserir ou mover grupos numerados nunca reescreve `\1`, `\2`. Se uma
  referência passa a apontar para outro grupo, o editor avisa na hora.
- **Renomear captura** altera só o nome do grupo e das referências sintáticas
  (`\k<…>`, `\g{…}`, `(?P=…)`, `(?&…)`, condições), em um passo de desfazer; literais,
  comentários e blocos `\Q…\E` ficam intactos.

O que o assistente não consegue garantir fica neutro, sem inventar cores ou relações,
e a edição manual e a validação nativa continuam disponíveis: grupos com numeração
reiniciada `(?|…)` (números repetidos), nomes duplicados `(?J)`, construções `(?…)`
fora do subconjunto e expressões acima de 8 KiB (a marcação visual é desligada para
manter o custo previsível; o limite do motor continua sendo 16 KiB). Nessas
regiões as transformações visuais (capturar, renomear) são desligadas, mas a
expressão continua válida e aplicável se o PCRE2 a aceita. O compilador nativo é
sempre a autoridade. A versão embarcada é a PCRE2 10.46 (lookbehind de comprimento
variável até 255 caracteres por alternativa; `(?<=a+)b` é recusado); o status do
editor informa a versão em uso e nada além dela é presumido.

#### Posições e capturas nativas

A resposta do teste traz, por ocorrência, a faixa de cada grupo em bytes e em UTF-16
(o editor converte posições de bytes do PCRE2 corretamente: Unicode, emoji e CRLF),
o nome ou nomes ligados ao número, a participação e o valor. Um grupo opcional que
não participou aparece como “não participou”, nunca como captura vazia; uma
ocorrência vazia tem marcador próprio; em repetições vale o que o motor devolve (a
última iteração). A posição consumida por uma backreference não é informada pelo
motor e por isso não é destacada na amostra. As faixas nunca são reconstruídas
procurando o valor no texto. Cada resposta é limitada a 512 ocorrências, 8.192
faixas de grupo e 2 MiB de valores; ao atingir um limite, a resposta diz qual e é
marcada como incompleta.

### Campos derivados por captura

Uma captura vira um campo do Caso pelo mesmo editor, sem outra tela de regex:
**Salvar captura como campo** (menu do grupo), **Usar expressão avançada…** no
diálogo de novo campo a partir de uma seleção, ou editar um campo já salvo. O
diálogo pede o campo de origem, o nome do campo novo e a captura (por nome ou número),
testa na amostra e salva a definição `captures` no Caso. A extração usa a primeira
correspondência de cada registro; registros sem correspondência ficam sem o campo e
falhas de limite aparecem no diagnóstico do registro. O campo de origem permanece
intacto, nomes de campos originais ou reservados são recusados e o novo campo pode
ser usado em filtros, no SQL e em todas as análises depois de reexecutar.

### Limites

PCRE2 interpretado usa callouts de cancelamento, prazo de 100 ms, limite de um
milhão de passos de backtracking, profundidade 1.000 e heap de 8 MiB por avaliação.
Padrão de até 16 KiB, 128 grupos, 8 MiB por registro, amostra de até 1 MiB (o texto
de um evento é obtido com até 256 KiB; recortes são declarados). A compilação de
campos compartilha o orçamento do Caso. Um limite é uma busca incompleta ou um erro,
nunca um zero confirmado. As funções `regexp_*` do DuckDB continuam com a semântica
do DuckDB: para usar PCRE2 em SQL, aplique a expressão como filtro (com “aplicar
filtros atuais”) ou extraia um campo.

### Mapas

Os mapas de enriquecimento agora são editados em **Estrutura → Extração e normalização**, com
amostras reais, extração visual, Grok, tipos, conferência no conjunto e índices. A
definição declarativa, a precedência (especificidade, prioridade, id), a proveniência em
`norm._provenance` e os limites (128 regras, 256 KiB, 128 saídas e 512 KiB por registro)
continuam os mesmos; mapas antigos são abertos sem alteração. Veja
[extracao-normalizacao.md](extracao-normalizacao.md).

### Processos

**Construir árvore** lê os eventos visíveis, mantém instâncias e evidências em
SQLite temporário e carrega os filhos sob demanda. A busca aceita imagem,
comando, usuário, PID e GUID e preserva os ancestrais dos resultados. O detalhe
mostra comando, usuário, intervalo observado, ancestrais e eventos de origem.
Comandos longos aparecem como prévia, com acesso ao evento original.

Sysmon 1/5 e Security 4688/4689 têm semântica própria; a identificação exige
provedor e canal. `NewProcessId` identifica o processo criado em 4688 e o PID do
cabeçalho do provedor não vira o processo investigado. Campos normalizados de
mapas também são reconhecidos.

GUID é combinado com a máquina. PID exige início e máquina; inferir o pai exige
também boot conhecido e intervalo de vida compatível. Boot pode vir do registro
ou de um marcador EventLog 6005 no canal System. Ambiguidade, pai ausente, boot
desconhecido e ciclo/profundidade excessiva ficam explícitos. O filtro opcional
seleciona processos com eventos no recorte e preserva seus ancestrais visíveis.

## Armazenamento e limites

Índices de linhas, hits e grafos são temporários e limitados em disco. Há até seis
sessões de texto, quatro buscas estruturadas, seis resultados SQL e três grafos;
novas sessões removem as mais antigas, e sessões inativas são coletadas nas
próximas aberturas. O usuário pode precisar reexecutar um resultado expirado.
As páginas de texto, busca e SQL têm orçamento de 2 MiB. Linhas físicas acima de
8 MiB continuam navegáveis por janelas, mas ficam fora da cobertura regex.
Mesmo uma primeira linha maior que esse limite é percorrida em blocos de até
8 MiB, com uma prévia disponível antes de encontrar sua quebra de linha.

## Validação de aceite

1. Abrir e pesquisar antes de percorrer o fim do arquivo; navegar enquanto indexa;
   cancelar e retomar; linha longa, UTF-8, CRLF, arquivo alterado e último registro.
2. Trecho centrado no match, intervalos Unicode, regras de cor e contexto físico,
   temporal e por identificador, com exclusões e troca de fonte.
3. CTE, agregação, janela, schema, paginação, números grandes, exportação completa,
   cancelamento e rejeição de SQL com efeitos externos.
4. Lookaround, backreference, grupos nomeados, match vazio, limite de trabalho,
   cancelamento, amostra declarada e extração persistida.
5. Especificidade, conflitos, precedência, reprocessamento sem acumular valores,
   proveniência e uso dos campos no SQL e nos processos.
6. GUID, PID reutilizado, outra máquina/boot, pai ausente/ambíguo, ciclos, filtros,
   paginação de filhos e abertura das evidências originais.

## Verificações executadas em 23/10/2026

Reorganização (Visualizar arquivos, pesquisa por expressão, editor assistido,
contexto em Eventos, extração de campos), executada sobre o estado atual da árvore.

- Rust (compilação de desenvolvimento): 289 testes unitários das unidades essenciais
  (consulta, motor colunar, SQL, mapas, processos, leitura direta, filtros, regressões)
  mais os 32 de `search_regex`, `regex_lab`, `search_predicate`, `event_search` e
  `local_analysis` aprovados, incluindo posições de grupos com Unicode, emoji e CRLF,
  grupos opcionais, capturas vazias, em repetição e em condições de contexto,
  orçamento de faixas e escopo SQL com o filtro de expressão.
- Paridade do motor colunar com o motor de linhas (`engine_parity`, 3.270 s): todos os
  filtros anteriores e 12 casos novos do operador `search` (PCRE2, Rust e literal,
  campos `message`/`raw`/`source`/derivado, escopo de linha, filtro combinado)
  selecionam as mesmas linhas, contagens, páginas, estatísticas, agrupamentos e
  comparações; um teste de integração confirma a seleção do conjunto completo.
- JavaScript: 349 testes essenciais aprovados, incluindo 26 do analisador estrutural
  (parênteses falsos, classes, `\Q…\E`, `(?x)`, `(?n)`, referências relativas e
  nomeadas, `(?|…)`, nomes duplicados, condicionais, escape literal, ajudas,
  renomeação e custo com milhares de grupos). A suíte estendida repete as falhas que
  já existiam em `HEAD` (`frontend-search-draft`, `frontend-source-intent`).
- Interface com transporte sintético (não é PCRE2): os 12 fluxos obrigatórios
  passaram, entre eles `test-regex-workbench.mjs` (navegação sem as abas Texto/Regex,
  edição livre, desfazer/refazer, cores de grupo e referência, aviso de numeração,
  ações assistidas por seleção, aplicar/recusar, respostas atrasadas, teclado, contexto
  na gaveta, extração de campo, 1024/1440 px, temas claro e escuro) e
  `test-local-explorer.mjs`. Falhavam antes da mudança, em `HEAD`, e seguem iguais:
  `test-ui-scale`, `test-responsiveness`, `test-workspace-scope`.
- Executável Windows real (WebView2 e IPC reais, `test-local-native.mjs`, 200.000
  linhas): visualizador sem carregar a análise, abertura antes do índice completo,
  PCRE2 até o fim do arquivo, SQL, mapas, processos, contexto; e, novos, a expressão
  `(?<=sequence=)(?<n>1\d)` com emoji e CRLF na amostra (versão PCRE2 10.46 informada),
  filtro aplicado com contagem, página, SQL com filtros, agrupamento e exportação
  JSONL coincidindo em 110 eventos, recusa de expressão inválida e de sintaxe PCRE2
  no motor Rust, limite do motor como erro (duas vezes, sem reutilizar seleção
  parcial), contexto na gaveta e captura salva como campo usada em filtro e SQL.

Na medição local com a compilação de desenvolvimento do Windows, o arquivo de
1.000.000 de linhas tinha 79.888.896 bytes. Uma sessão, já com o Caso ativo, abriu
em 1,06 s após indexar 1.378.184 bytes (aproximadamente 1,7% do arquivo). A busca
PCRE2 `(?<=needle )ação` encontrou a ocorrência no fim e concluiu em 99,5 s,
em 107 etapas. Antes da reutilização das instruções SQLite, a mesma busca levou
285,9 s. São medições pontuais de desenvolvimento, sem equivalência prometida com
um executável de release ou arquivos de outros formatos/tamanhos.

O aceite nativo pode ser repetido, com as dependências de `scripts/preview`
instaladas, por:

```powershell
npm run prepare:frontend
cargo build --manifest-path src-tauri/Cargo.toml --locked --bin loginsight
$env:LOCAL_NATIVE_LINES = '1000000'
node scripts/preview/test-local-native.mjs src-tauri/target/debug/loginsight.exe
node scripts/preview/test-regex-workbench.mjs http://127.0.0.1:4173   # sintético; após node scripts/preview/serve.mjs 4173
```

Esse teste usa perfil, dados e cache isolados em `output/local-native-*`, preserva
um `results.json` e capturas de tela e encerra somente os processos que criou.
