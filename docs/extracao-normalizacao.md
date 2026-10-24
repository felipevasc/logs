# Extração e normalização

Ambiente visual, em **Estrutura → Extração e normalização**, para definir como os dados
dos arquivos são extraídos, interpretados, normalizados e (opcionalmente) indexados. Ele
substitui a aba **Mapas** de Explorar: os mapas antigos continuam funcionando e são
abertos nesta página. Campos customizados (campos derivados por captura ou fórmula) não
mudaram. Os arquivos originais nunca são alterados: a definição é salva no Caso ativo.

## Uma definição, várias telas

Tudo o que as telas editam é **uma única definição declarativa** (a aba *Definição* mostra
o JSON). Editar o JSON atualiza as telas e vice-versa; desfazer/refazer (Ctrl+Z / Ctrl+Y)
percorrem o mesmo histórico. Rascunhos não salvos ficam no espaço de trabalho do Caso,
separados do mapa salvo, e a barra mostra “Alterações não salvas”.

Estrutura (esquema 2, aditivo; mapas do esquema 1 são lidos e gravados sem mudança):

- **regra**: `id`, `selector` (provedor, canal, código, versão), `priority`, `when`
  (condições extras), `extract` (a extração), `fields` (saídas) e `enabled`.
- **extração** (`kind`): `grok`, `dissect`, `json`, `xml`, `delimited`, `key_value`,
  `regex` (PCRE2 ou Rust). Dados estruturados usam analisadores próprios, não regex.
- **saída**: `from` (campos extraídos ou do registro, em ordem de preferência) ou
  `literal`; `type` (`text`, `integer`, `decimal`, `boolean`, `datetime`, `ip`); `steps`
  (aparar, minúsculas/maiúsculas, nome do arquivo, hexadecimal→decimal, remover prefixo/
  sufixo/caracteres, substituir); `datetime` (formato e fuso); `onMissing`/`default`,
  `onEmpty`, `onInvalid`, `conflict`; `index` (preferência); `description`.
- **`grokPatterns`**: padrões reutilizáveis da definição; **`ui`**: cores e notas.

Reconhecer não é converter: uma saída só é convertida quando pede um tipo ou etapas.
O valor original fica na proveniência (`<campo>._provenance`). Ausente, vazio, inválido,
zero e falso são estados distintos (zero e falso são valores). Inteiros acima de 2^53
são emitidos como texto para não perder dígitos. Datas exigem formato e fuso explícitos
(ou deslocamento no formato) e saem em ISO-8601 UTC.

Alterar título, notas, cores ou preferências de índice **não** invalida caches nem
índices; alterar qualquer coisa que muda um valor, sim (assinatura semântica).

## Modo simples (padrão)

A página abre no **modo simples**, pensado para ensinar pelo exemplo, com quase nenhum texto: as
explicações ficam atrás de um ícone ⓘ que abre um modal.

- **Tipos de linha** (à esquerda): o motor sorteia amostras de todas as fontes e as agrupa pela
  aparência. Cada tipo mostra um anel com a cobertura e, quando se encaixa num formato pronto (acesso
  HTTP, chave=valor, JSON, CSV…), oferece “Usar este formato”. Botão direito no tipo: renomear ou
  recomeçar.
- **Linha de exemplo**: os pedaços que o motor reconheceu (IP, datas, valores de `chave=valor`, com a
  unidade e as aspas de fora) aparecem sublinhados. Um clique vira coluna; outro clique ignora.
  Qualquer trecho selecionado abre, com o botão direito, um menu curto: coluna, o que vem depois/antes,
  entre marcadores, ignorar, texto fixo, permitir variação, opcional, separar por delimitador.
  Reconhecidos entre colunas (um IP no meio da linha) viram padrões e não o texto daquela linha.
- **Tabela ao vivo**: calculada pelo motor nativo sobre as amostras. Cabeçalho: renomear, tipo (data
  pede formato e fuso), normalizações (minúsculas, sem espaços, só o nome do arquivo, hex→decimal),
  valores frequentes, criar índice, duplicar, remover. Célula: copiar, ver na linha e, quando a
  conversão falha, o motivo e o conserto (“trocar vírgula por ponto”).
- **Cobertura e sobras**: estimativa sobre as amostras do tipo (a contagem completa está em
  “Conferir no conjunto”, no diálogo de aplicar). O selo vermelho lista as linhas que sobraram; cada
  uma pode ser ensinada como um tipo próprio.
- **Aplicar**: resume os campos, deixa conferir no conjunto e salva no Caso. Desfazer/refazer e os
  rascunhos são os mesmos do modo avançado.

O modo simples escreve na mesma definição: a linha de exemplo e as marcações ficam em
`ui.easy` da definição, para reabrir e continuar editando. Regras criadas no modo avançado aparecem
como “+N” e só se editam lá. O botão `</>` abre o **modo avançado**, com tudo que existia antes.

## Modo avançado: fluxo com amostras reais

1. **Sortear amostras**: amostra estratificada e repetível (mesma semente, mesmas
   amostras) de todos os arquivos abertos; cerca de 70% seguem os formatos predominantes
   e cerca de 30% vêm dos mais raros. A tela diz a estratégia, quantos registros foram
   examinados e a distribuição por arquivo. Sem arquivos, cole linhas ou use um modelo.
2. **Selecionar um trecho** (botões visíveis ou menu de contexto): extrair como campo,
   reconhecer formato, depois de…, antes de…, entre marcadores, separar por delimitador,
   ignorar, texto fixo, permitir variação, tornar opcional. As marcações geram a
   expressão Grok da regra; o texto continua editável.
3. **Sugestões locais** explicáveis (“encontrado em X de N amostras”): IP, datas, UUID,
   caminhos, URL, e-mail, números, chave=valor, JSON, XML e colunas. Aceitar, ajustar ou
   descartar; nada é aplicado sozinho.
4. **Prévia rápida**: a definição roda no motor nativo sobre as amostras e mostra, por
   registro, o texto com os trechos coloridos (cor estável por campo), o valor original,
   o extraído (com posição), o normalizado e o estado. É rotulada como amostra.
5. **Conjunto completo** (“Conferir no conjunto”): passa por todos os registros (ou pelo
   recorte de Explorar), em lotes, cancelável, e mostra contagens exatas por regra,
   saída e arquivo, com exemplos que funcionaram e que falharam (abrir registro/arquivo).
6. **Efeito da edição**: campos novos/removidos/alterados e **quais índices precisam ser
   recriados**, antes de salvar.

## Grok: compatibilidade e limites

O Grok aqui é um motor próprio sobre o PCRE2 já embutido (limites, prazo de 100 ms e
cancelamento). A sintaxe `%{PADRÃO}`, `%{PADRÃO:campo}` e `%{PADRÃO:campo:int|float}`,
padrões aninhados e personalizados são expandidos em PCRE2; a prévia e a aplicação usam
**o mesmo motor nativo** (nunca `RegExp` do JavaScript). Capturas entre `%{}` são as únicas
que viram campos (grupos internos não nomeados não capturam).

- **Nível de compatibilidade**: sintaxe e vocabulário públicos do Grok; **não** é um
  clone do Oniguruma/Joni do Elastic/Logstash. Construções cujo significado difere entre
  os motores são **recusadas com diagnóstico** (por exemplo `\h`/`\H`, `\y`/`\Y`,
  referências por nome `\k<…>`/`\g<…>`) em vez de aceitas com outra semântica.
- **Biblioteca de padrões**: escrita para este projeto (nomes seguem o vocabulário
  público), sem copiar os arquivos de padrões Apache-2.0 do Logstash; cada entrada é
  compilada e testada com seu exemplo.
- **Limites**: profundidade 24, expansão de 64 KiB, 16 alternativas, 64 campos por
  extração, 128 padrões personalizados. Ciclos, padrões desconhecidos, nomes duplicados
  e expansão excessiva viram diagnósticos com posição no texto.
- **Âncora**: procurar em qualquer ponto, começar no início ou casar o texto inteiro.
- **Alternativas ordenadas**: a primeira que reconhece vence e a tela mostra qual
  reconheceu quantos registros no conjunto.

## Índices

Três tipos, sempre opcionais e por Caso/fonte/interpretação (uma alteração de mapa que
muda valores troca a chave e o índice antigo deixa de ser usado):

| tipo | acelera | tipos de saída |
| --- | --- | --- |
| Igualdade (`exact`) | `=` exato e sem diferenciar maiúsculas ASCII | qualquer |
| Intervalos (`range`) | `>`, `>=`, `<`, `<=`, entre — números e datas | inteiro, decimal, data/hora |
| Palavras (`terms`) | operador novo `term` (palavras inteiras, todas obrigatórias) | texto |

O índice só fornece um superconjunto de linhas candidatas; o predicado canônico roda
sobre elas, então **o resultado é igual com ou sem índice** (testado). Consultas SQL não
usam estes índices. Cada índice mostra tamanho em disco, pode ser cancelado durante a
criação, é publicado de forma atômica e removido pela mesma tela. Listas densas (mais de
4.096 linhas para igualdade, 50.000 para intervalos/palavras) mantêm o caminho vetorizado.

## Modelos incluídos

Windows/Sysmon/auditd (PID, PID do pai, executável, usuário, host), IP-usuário-ação-
duração, log de acesso HTTP (Apache/Nginx combined), chave=valor, colunas delimitadas,
nomes de propriedades JSON, eventos XML do Windows e cabeçalho do auditd. Cada um traz
linhas de exemplo que rodam na hora, sem arquivos abertos.

## Isolamento e segurança

Amostras, sugestões e conferências leem apenas a fonte admitida do Caso ativo; mudar de
Caso, fonte ou revisão descarta respostas atrasadas e resultados. Nada sai da máquina e
nenhum arquivo original é modificado. A definição aceita até 128 regras e 256 KiB; cada
registro aceita até 128 saídas e 512 KiB.

## Testes e como subir

- Rust: `cargo test --lib -- --test-threads=1 grok:: map_values:: map_extract::
  enrichment_maps:: map_lab:: map_templates:: field_indexes::` e `cargo test --test
  engine_parity`.
- JavaScript: `npm test` (inclui `extraction-model.test.mjs`).
- Prévia: `node scripts/preview/serve.mjs` e `node scripts/preview/test-extraction.mjs`
  (motor simulado; a semântica nativa é coberta pelos testes Rust e
  `test-local-native.mjs`).
- Desempenho: `LOGINSIGHT_BENCH_LINES=1000000 cargo test --release --lib scan_benchmark
  -- --ignored --nocapture` imprime tamanho, registros, modo de build e taxas.

## Desempenho medido

`scan_benchmark` (arquivo de texto sintético, 300.000 linhas, 16,6 MiB, Grok com 4 campos
tipados, 2% das linhas fora do formato), 2026-10-24, nesta máquina de desenvolvimento:

| build | carga | conferência completa | taxa |
| --- | --- | --- | --- |
| release | 2,6 s | 10,8 s | ≈ 27.900 registros/s (1,5 MiB/s) |
| debug | 15,1 s | 156 s | ≈ 1.900 registros/s |

Só ler os registros (mapa sem regras) leva 2,9 s na release (≈ 101.700 registros/s); a
extração responde por cerca de 73% do restante. A conferência processa em fluxo, um
registro por vez, e a memória não cresce com o arquivo (as contagens são acumuladores; os
exemplos são limitados a 40 de cada tipo). A taxa de extração ainda pode melhorar (a
conferência constrói a proveniência de cada registro, que ela própria não usa).
Para reproduzir: `LOGINSIGHT_BENCH_LINES=1000000 cargo test --release --lib scan_benchmark
-- --ignored --nocapture`.
