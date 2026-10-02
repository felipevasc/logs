# Limites de blocos no resumo temporal

O leitor do resumo temporal pode manter o maior timestamp de cada bloco de 512 valores. Uma busca localiza o bloco pelos seus limites e termina com a mesma busca binária exata dentro dele. Os limites são preparados durante a verificação já existente do arquivo; as consultas não precisam construí-los por meio de uma nova varredura.

O orçamento é de 64 KiB por leitor, incluindo o vetor e seus metadados. O tamanho do bloco dobra quando necessário; se a preparação opcional não couber, permanece a busca anterior. As posições dos sete níveis continuam separadas. Os arquivos v1, os checksums, as identidades de publicação e as proteções dos leitores são preservados.

Para 50 milhões de timestamps, blocos de 512 exigem aproximadamente 0,78 MB de valores de limite, além do arredondamento por segmento/nível e dos objetos dos leitores. Isso é uma conta de tamanho, não uma medição de RAM do aplicativo. Leitores simultâneos podem manter mais de uma cópia.

## Ensaio isolado

Um programa compilou o módulo efetivo do leitor, produziu arquivos v1 com DuckDB e comparou a busca anterior com a busca por blocos no mesmo mapeamento. Cada distribuição continha 131.072 timestamps; os limites retidos ocuparam 2.144 bytes.

Medianas de nove amostras de 20 mil buscas, após descartar a primeira repetição:

| Distribuição sintética | Busca anterior | Com limites |
|---|---:|---:|
| Dez registros por timestamp | 156,07 ns | 84,27 ns |
| Intervalos de 17 ms | 157,56 ns | 94,66 ns |
| Intervalos com saltos de dias | 159,31 ns | 94,46 ns |
| Timestamps negativos | 139,71 ns | 108,22 ns |
| Timestamp constante | 118,98 ns | 44,72 ns |

A ordem das variantes alternou entre repetições. A validação anterior às medições deixou os dados residentes em memória. Esses números não medem consultas completas, interface, leitura fria de disco ou uma importação Nginx de 50 milhões de registros.

As verificações incluíram os sete níveis, empates, limites de blocos, timestamps negativos e extremos de `i64`, zero/ausência, faixas do histograma, igualdade de contagens/estatísticas com os limites removidos, adaptação do orçamento, cancelamento, rejeição de leitor alterado e preservação dos bytes e da data de modificação do arquivo v1.

O experimento anterior também comparou compactação exata por referência ao menor valor do bloco. Ela reduziu o tamanho, mas frequentemente aumentou o tempo das buscas residentes. Por isso esta alteração conserva os valores `i64` do formato atual. As referências conceituais são o [armazenamento de sequências monotônicas do Lucene](https://github.com/apache/lucene/blob/main/lucene/core/src/java/org/apache/lucene/util/packed/DirectMonotonicWriter.java) e a [compactação leve do DuckDB](https://duckdb.org/2022/10/28/lightweight-compression).
