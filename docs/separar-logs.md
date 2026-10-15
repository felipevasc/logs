# Separar logs

**Estrutura → Separar logs** divide um arquivo de log grande em dois: os itens que atendem
aos critérios e, opcionalmente, o restante. O arquivo é lido em fluxo, em blocos, sem ser
carregado na análise nem na memória; arquivos de dezenas de gigabytes são tratados do mesmo
jeito que arquivos pequenos.

## 1. Arquivo

O arquivo e os destinos são escolhidos em janelas do sistema. A página mostra tamanho,
codificação, quebra de linha, primeira data reconhecida, espaço livre no destino e as primeiras
linhas. Se as datas do início do arquivo estiverem em ordem, a página avisa (veja
[ordem cronológica](#ordem-cronológica)).

Codificações: UTF-8 (com ou sem BOM), code pages legadas (Windows-1252, ISO-8859-x, IBM850…)
e UTF-16 LE/BE, como o PowerShell grava. Os arquivos gerados mantêm a codificação e o BOM do
original byte a byte. Linhas terminadas só em CR também funcionam. Arquivos compactados
(gzip, bzip2, xz, zstd, zip) e EVTX precisam ser descompactados ou exportados antes, e UTF-32
não é aceito.

## 2. O que é um item do log

Os critérios valem para **itens**, não necessariamente para linhas: num log Java, uma mensagem
de erro e todo o seu stack trace formam um item só, e vão juntos para o mesmo arquivo.

| Opção | Um item novo começa… |
| --- | --- |
| Automático | Numa linha com o mesmo tipo de data que inicia a maioria das linhas, quando o arquivo tem linhas de continuação (stack traces, texto recuado). Sem isso, em cada linha que não é continuação. A página mostra o que foi detectado. |
| Cada linha | Em toda linha. |
| Começa com data e hora | Em toda linha iniciada por data (opcionalmente após `[`), em qualquer formato reconhecido. |
| Recuos e stack traces continuam | Em toda linha que não é recuada, não é `at …`, `Caused by:`, `... N more`, cabeçalho de exceção nem linha em branco. |
| Começa com um padrão | Em toda linha que corresponde à expressão regular. |
| Termina com um padrão | Depois da linha que corresponde à expressão regular (por exemplo `^</event>$`). |
| Separado por linha em branco | Na primeira linha não vazia depois de uma ou mais linhas vazias. |
| Separador personalizado | Depois de cada ocorrência do texto informado, em qualquer posição (`\n\n`, `\x1e`, `;;`…). Escapes: `\n`, `\r`, `\t`, `\0`, `\\`, `\xNN`. |

Só os primeiros 64 KB de cada linha são examinados para decidir se ela inicia ou encerra um
item. Um item maior que 16 MB (uma linha gigante, um padrão que nunca aparece) não é cortado:
ele é decidido pelos primeiros 16 MB e gravado inteiro no mesmo arquivo; o resultado informa
quantos itens assim houve.

## 3. Critérios

Cada critério pode ser invertido (**NÃO**), e os critérios se combinam com **E** (todos) ou
**OU** (qualquer um).

- **Texto** — contém, começa com, termina com ou é igual a; vários textos (um por linha) valem
  como “qualquer um deles”. Opcionalmente ignora maiúsculas e minúsculas (inclusive acentuadas)
  e olha só a primeira linha do item — útil para achar `ERROR` no cabeçalho sem casar com a
  mensagem de uma exceção. “Termina com” e “é igual a” desconsideram espaços e quebras finais.
- **Expressão regular** — sintaxe do Rust (`regex`), no item inteiro ou só na primeira linha;
  `^` e `$` marcam início e fim de cada linha do item.
- **Data e hora** — “a partir de” (inclusive) e/ou “antes de” (exclusive).
  - *Onde está a data*: a primeira data do item em qualquer formato (padrão; procura na primeira
    linha e, se não houver, nas seguintes até 8 KB), uma data no início da linha, o texto
    capturado por uma expressão regular (grupo `(?P<data>…)`, `(?P<date>…)` ou o primeiro grupo)
    ou um campo de JSON por caminho com pontos (`@timestamp`, `event.created`).
  - *Formato* (opcional): strftime (`%d/%m/%Y %H:%M:%S`), padrão estilo Java
    (`dd/MM/yyyy HH:mm:ss.SSS`, `XXX` para fuso) ou `epoch`, `epoch_s`, `epoch_ms`. Sem formato, a
    data é reconhecida como na leitura de logs: ISO, Apache, syslog, meses por nome em seis
    idiomas, datas numéricas, epoch. Veja [Formatos reconhecidos](formatos-de-log.md#data-e-hora).
  - *Datas numéricas*: a ordem dia/mês é aprendida com o início do arquivo, ou pode ser fixada.
  - *Horários sem fuso*: no fuso deste computador ou em UTC. Os campos “a partir de” e
    “antes de” são lidos no mesmo fuso. Datas com fuso explícito são sempre exatas.
  - *Datas sem ano* usam o ano do nome do arquivo ou da última modificação, como na leitura.
  - *Item sem data*: usa a data do item anterior (padrão — continuações sem data ficam no
    mesmo período), não entra ou entra.
- **Posição** — do número N ao número M (inclusive), contando **itens** (registros, com o stack
  trace contando como um) ou **linhas** do arquivo (o item entra pela linha em que começa).

**Testar no início do arquivo** aplica tudo aos primeiros 4 MB (até 5.000 itens) e mostra cada
item com número, linha, data reconhecida e resultado — o jeito de conferir a separação em itens
e a leitura das datas antes de processar o arquivo inteiro.

### Ordem cronológica

Com um critério de data, a opção **O arquivo está em ordem cronológica** acelera muito arquivos
grandes:

- o início do período é localizado por **busca binária** — o que vem antes é copiado em bloco,
  sem ser lido item a item;
- quando a regra é só um intervalo de datas, o fim também é localizado por busca binária e o
  arquivo inteiro é separado sem avaliar nenhum item;
- com outros critérios, a leitura para (ou o resto é copiado em bloco) assim que um item passa
  do fim do período.

Critérios de posição também param a leitura assim que o intervalo é ultrapassado, com ou sem
essa opção. Em arquivos com pequenas inversões (vários threads gravando), o corte acontece no
primeiro item que alcança a data; itens fora de ordem depois dele não são revistos. Sem a
opção, todo item é avaliado.

## 4. Resultado

- **Só o arquivo filtrado** — o original fica intacto.
- **Filtrado e restante** — dois arquivos; o original fica intacto. Exige espaço livre próximo
  do tamanho do original.
- **Filtrado e restante, removendo o original** — no fim o original é apagado.

Os nomes padrão ficam ao lado do original (`app.filtrado.log`, `app.restante.log`; para nomes
como `server.log.2026-10-12`, `server.log.2026-10-12.filtrado.log`) e nunca substituem um
arquivo existente; outro nome pode ser escolhido. Enquanto trabalha, cada saída é um arquivo
oculto `.loginsight-separando-*.parcial` na pasta de destino: cancelar, faltar espaço ou ocorrer
um erro remove os parciais e deixa o original como estava. A gravação para quando o disco de
destino fica com menos de 256 MB livres.

Execução, progresso e cancelamento aparecem também em **Em andamento**; trocar de página não
interrompe a separação.

### Pouco espaço livre: separação em partes

Ao remover o original sem espaço para duplicá-lo — e com os dois destinos no mesmo disco do
original — a separação é feita sem ocupar mais do que algumas dezenas de MB extras:

1. **Análise** (só leitura): o arquivo é percorrido uma vez e é gravado um mapa compacto de
   quais trechos vão para cada saída (8 bytes por trecho contínuo). Nada é alterado.
2. **Corte do fim para o início**: um bloco do final do original (até 256 MB, conforme o espaço
   livre) é copiado para duas partes temporárias e o original é **truncado** antes dele; o uso
   do disco fica praticamente constante. O trecho inicial que vai inteiro para uma só saída
   nunca é copiado.
3. **Montagem**: o original encurtado é renomeado como a saída do seu trecho inicial e as partes
   são acrescentadas às saídas em ordem, cada uma apagada logo depois.

Com um corte por data em arquivo cronológico (“tudo antes de X”), a análise é feita por busca
binária e só o trecho depois de X é copiado.

Segurança: no Windows o original é aberto com acesso exclusivo (nenhum programa pode gravar
nele, e ele não pode estar carregado na análise do LogInsight); no Linux, uma mudança de
tamanho feita por outro programa interrompe o corte. Um diário em
`%APPDATA%\LogInsight\log-split` é gravado antes de cada passo irreversível.
**Cancelar durante o corte devolve os trechos ao original, byte a byte.** Na montagem o
cancelamento não é possível. Se o aplicativo fechar no meio, a página mostra a separação
interrompida com **Concluir** e, durante o corte, **Desfazer**. A pasta de trabalho
`.loginsight-separacao-*` fica ao lado do original até o fim.

## Desempenho

O enquadramento em itens é sequencial; textos, expressões e datas são avaliados em paralelo em
lotes de 8 MB, dentro do limite de threads configurado em Recursos. Regiões decididas por busca
binária ou por um intervalo já ultrapassado são copiadas em blocos de 4 MB, na velocidade do
disco.

Medição em 2026-10-15 (log Java de 2 GiB gerado, 15,2 milhões de itens com stack traces,
gravando no mesmo disco; release):

| Separação (filtrado + restante) | Tempo | Vazão |
| --- | ---: | ---: |
| `ERROR` na primeira linha | 10,6 s | 194 MiB/s |
| Expressão regular no item | 11,9 s | 173 MiB/s |
| Antes de uma data, todo item avaliado | 15,6 s | 131 MiB/s |
| Antes de uma data, ordem cronológica (nenhum item avaliado) | 15,4 s | 133 MiB/s |
| Removendo o original, em partes | 17,2 s | 119 MiB/s |

Os tempos são dominados pela gravação no disco: com dois arquivos de saída, todo o volume é
regravado. A ordem cronológica rende mais quando só uma parte pequena é gravada (só o
filtrado, ou um período curto). Para repetir: `cargo test --release --lib
log_split::tests::bench_large_file -- --ignored --nocapture` (tamanho em
`LOGINSIGHT_SPLIT_BENCH_MB`, pasta em `LOGINSIGHT_SPLIT_BENCH_DIR`).
