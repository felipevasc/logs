# Formato portátil de Caso (.licase), versão 1

O contêiner transporta o documento de investigação, imagens referenciadas,
payloads de exclusão e os bytes JSONL das referências disponíveis. Os logs
originais não são incluídos nem alterados. SHA-256
detecta alterações nos conteúdos; ele não autentica quem produziu o arquivo.

O formato usa uma sequência sem compressão, sem caminhos de arquivos e sem
extração de diretórios. Isso permite impor limites antes de ler cada conteúdo e
copiar payloads com um buffer de 64 KiB, independentemente da quantidade de
registros excluídos.

## Estrutura binária

| Ordem | Conteúdo |
| --- | --- |
| 1 | 16 bytes de assinatura: `4c 4f 47 53 43 41 53 45 0d 0a 1a 0a 01 00 00 00` |
| 2 | Inteiro sem sinal de 64 bits, little-endian: tamanho do manifesto em bytes |
| 3 | Manifesto JSON UTF-8, exatamente o tamanho declarado |
| 4 | Conteúdos brutos das entradas, exatamente na ordem do manifesto |

Não existem separadores, alinhamento, nomes de arquivos ou conteúdo adicional
entre as entradas. Cada tamanho é contado em bytes. Dados após a última entrada
ou tamanho total diferente do manifesto tornam o arquivo inválido.

O manifesto contém `formatVersion: 1`, `metadata` e `entries`. Cada entrada tem
`asset`, `bytes` e `sha256`. `sha256` usa 64 caracteres hexadecimais minúsculos.
O objeto `asset` tem `kind`; imagens, exclusões e referências possuem `id`:

```json
{
  "formatVersion": 1,
  "metadata": {},
  "entries": [
    {"asset": {"kind": "document"}, "bytes": 123, "sha256": "..."},
    {"asset": {"kind": "image", "id": "..."}, "bytes": 456, "sha256": "..."},
    {"asset": {"kind": "exclusion", "id": "12345678-1234-4234-8234-123456789abc"}, "bytes": 8192, "sha256": "..."},
    {"asset": {"kind": "reference", "id": "23456789-1234-4234-8234-123456789abc"}, "bytes": 456, "sha256": "..."}
  ]
}
```

O documento é obrigatoriamente a primeira e única entrada `document`. A imagem
usa seu próprio SHA-256 como identificador. Exclusões e referências usam UUID canônico,
minúsculo, com hífens, diferente de zero. Identificadores repetidos dentro do
mesmo tipo são rejeitados. Campos desconhecidos no envelope ou nas entradas são
rejeitados. Os pontos no exemplo representam hashes completos, não são valores
válidos.

### Metadados de visibilidade

Na aplicação, `metadata` contém `schemaVersion: 1` (sem referências) ou `2`
(com referências) e `ledgers`, com exatamente um
objeto por Caso do documento. Cada objeto contém `schemaVersion`, `identity` e
`batches`. A identidade (`caseId`, `analysisId`, `configRevision` e
`visibilityRevision`) deve coincidir integralmente com o `analysisContext` do
respectivo Caso. O exemplo acima mostra apenas o envelope do transporte.

Cada item de `batches` contém `batch` e `masks`. O lote mantém o identificador,
identidade da análise, data, descrição, motivo, recorte, recibo da fonte,
quantidade de membros, estado ativo e eventual data de restauração. Seu campo
`payload` e os itens de `masks` usam o descritor `formatVersion`, `id`, `sha256`,
`bytes`, `members` e `dictionaryBytes`. Cada descritor corresponde a exatamente
uma entrada `exclusion` do contêiner; entradas ausentes, adicionais ou
compartilhadas entre proprietários são rejeitadas.

Lotes restaurados e máscaras de restauração parcial fazem parte do histórico.
O documento não pode anunciar uma revisão de visibilidade e omitir o histórico
que a implementa. Textos de proveniência podem conter caminhos históricos, mas
esses textos nunca são usados para escolher arquivos locais durante a importação.

### Metadados e bytes de referências

Metadados de versão 2 incluem `referenceSets`, com exatamente um conjunto por
Caso, incluindo conjuntos vazios. Cada conjunto contém `owner` (`caseId` e
`analysisId`) e `references`. Cada referência contém o `descriptor` completo da
configuração e `state`. Proprietário, identificador, hash de conteúdo, formato,
colunas, chaves e política de duplicação devem coincidir com a configuração
capturada do Caso; não existe resolução por nome global ou caminho estrangeiro.

Uma referência disponível declara
`state: {availability: "available", assetId: "UUID", sourceBytes: 123, schemaSha256: "..."}`.
Ela corresponde a exatamente uma entrada `reference`, com o mesmo tamanho e
SHA-256 de conteúdo do descritor. O conteúdo é o JSONL original, incluindo sua
codificação, espaçamento e quebras de linha. O índice SQLite local não é exportado.
Na importação, o índice é reconstruído pelo validador limitado da aplicação, que
confere esquema, chaves e duplicações, além dos hashes de conteúdo e esquema.

Uma referência sem bytes locais declara `state: {availability: "unavailable"}`.
Seus metadados continuam presentes, com diagnóstico explícito de indisponibilidade
no Caso importado. Corrupção ou hash inválido interrompem a exportação; não são
convertidos silenciosamente em indisponibilidade. Arquivos antigos com metadados
de versão 1 continuam aceitos, mas suas referências declaradas sem bytes recebem
o mesmo diagnóstico. Entradas extras, compartilhadas entre proprietários ou
declaradas disponíveis sem conteúdo são recusadas.

`available` descreve a presença dos bytes verificados, não a admissão de uma
consulta. Antes de publicar o Caso, o importador prepara as projeções usadas
pelas definições ativas. O runtime admite até 8 MiB de chaves, valores e índices
retidos e 100.000 linhas agregadas; esse orçamento depende das colunas usadas,
não apenas do tamanho do JSONL. Se for excedido, os bytes continuam preservados
e as definições de lookup recebem diagnóstico com seu índice original, sendo
desativadas junto com seus dependentes. A análise dos campos brutos continua
disponível. Referências ausentes desativam somente as definições afetadas e seus
dependentes; referências saudáveis continuam ativas. O recibo de salvamento
retorna os motivos explícitos no `analysisContext`.

## Limites

| Limite | Máximo |
| --- | --- |
| Arquivo completo, incluindo cabeçalho e manifesto | 32 GiB |
| Manifesto JSON | 8 MiB |
| Documento de investigação | 64 MiB |
| Imagem individual | 16 MiB |
| Imagens | 1.000 |
| Payload individual | 4 GiB |
| Payloads | 4.096 |
| JSONL individual de referência | 256 MiB |
| Referências, disponíveis ou indisponíveis | 4.096 |

Referências JSONL vazias são válidas e preservam o esquema com zero registros;
as demais entradas vazias são rejeitadas. O leitor também valida os limites de imagens,
configuração e visibilidade da aplicação antes de publicar uma investigação. Um
arquivo dentro dos limites do contêiner pode ser recusado se exceder o orçamento
de leitura de um Caso.

O documento contém no máximo 1.024 Casos. O orçamento de visibilidade de cada Caso
também é aplicado à importação: atualmente, até 128 payloads ativos entre lotes e
máscaras, com até 8 MiB de dicionários agregados. Até 1.024 Casos e 128 MiB de
metadados de importações podem aguardar publicação local ao mesmo tempo.

## Publicação e compatibilidade

A exportação é escrita em um arquivo temporário no mesmo diretório do destino.
Cada conteúdo é verificado durante a cópia. O destino só é substituído depois de
todas as verificações, sincronização do arquivo e checagem de cancelamento.

A importação grava cada entrada em um diretório temporário privado com nomes
inteiros gerados localmente. Falha de integridade ou cancelamento remove a
extração parcial. Os identificadores de origem nunca viram caminhos locais.
Verificar hashes não torna um banco SQLite de terceiros confiável: a validação
de esquema, limites e conteúdo dos payloads é uma etapa independente.

O importador atribui novos identificadores locais ao Caso, à análise, aos lotes
e às máscaras. A preparação dos payloads ocorre antes da transação de salvamento;
o documento retornado à interface contém somente um token local de preparação,
sem uma identidade de análise ainda não publicada. A interface mantém seu fluxo
de acrescentar os Casos importados aos Casos abertos e salvar o conjunto.

O salvamento grava corpo, contexto e propriedade dos lotes na mesma transação.
O token só é retirado da preparação depois do `COMMIT`. Uma falha pode ser
repetida sem perder a preparação; uma confirmação perdida depois do `COMMIT`
mantém as revisões posteriores do mesmo Caso. Tokens de outro Caso, de outro
armazenamento ou de uma preparação ausente são recusados. Após reiniciar a
aplicação antes de salvar, é necessário importar o arquivo novamente. Os tokens
não são gravados no corpo persistido nem exportados.

Uma importação interrompida nunca publica metade da visibilidade. Arquivos de
imagem e payloads imutáveis preparados antes de uma interrupção podem ficar sem
referência, mas não substituem fontes, evidências ou lotes existentes.

As referências são preparadas sob o novo proprietário local de Caso e análise,
o mesmo usado pelos lotes de exclusão. As verificações completas e a construção
dos índices ocorrem antes da transação. A preparação retém leases de arquivos,
e o salvamento confere suas identidades e tamanhos sem ler registros nem fazer
hashes dentro da transação. Apenas a configuração confirmada no mesmo `COMMIT`
torna os descritores acessíveis pela admissão do Caso. Falha ou cancelamento
preservam o estado anterior; ativos imutáveis preparados podem ficar sem referência.

Arquivos com histórico de exclusões incluem proveniência original imutável. A
exportação recusa a opção de mascarar senhas e tokens para esse formato, antes
de substituir o destino. A pessoa precisa desmarcar essa opção explicitamente
para exportar o arquivo completo; relatórios com textos mascarados continuam
disponíveis. A mesma escolha explícita se aplica a Casos que declaram referências:
alterar seus bytes ou esquema para mascará-los poderia mudar os resultados dos
lookups. Casos sem exclusões nem referências mantêm a opção de mascaramento.

O JSON de investigação continua sendo um formato legado aceito para Casos sem
histórico de exclusões. Ele não contém payloads de visibilidade e não pode ser
usado para descartar silenciosamente esse histórico. A configuração exportada
vem do armazenamento autoritativo do Caso, mesmo que a interface tenha uma
versão desatualizada.
