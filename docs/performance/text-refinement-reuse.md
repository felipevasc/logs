# Reutilização de candidatos completos durante refinamento textual

Validação pequena em 2026-10-01, sobre `3fd8df07db96f7067b3e56ca49a0c32dd70c19b9`.

O índice textual pode testar duas partes obrigatórias de uma busca. Cada parte completa fornece um superconjunto dos resultados; o leitor canônico ainda verifica a busca inteira. Antes desta mudança, esgotar o orçamento de trabalho na segunda parte descartava também o conjunto completo da primeira e exigia o caminho geral.

A mudança conserva somente esse conjunto anterior, já completo e dentro do limite existente. Nenhum ID da segunda parte incompleta é publicado. Cancelamento, dados inválidos, primeira parte incompleta e duas partes individualmente acima do limite continuam sem produzir candidatos. Não há novo índice, formato ou aumento de orçamento.

## Evidência

Um índice Tantivy real com 4.504 registros produz 4.500 candidatos completos para a primeira parte e 3.000 correspondências exatas da busca inteira. Foram exercitados dois limites durante o refinamento:

| Limite atingido | Termos visitados | Postings visitados | Candidatos completos preservados | Correspondências após confirmação | Maior bitmap temporário |
| --- | ---: | ---: | ---: | ---: | ---: |
| Dicionário | 4 | 4.500 | 4.500 | 3.000 | 568 bytes |
| Postings | 6 | 4.507 | 4.500 | 3.000 | 568 bytes |

O segundo cenário visitou sete postings adicionais antes de parar. Esses resultados parciais não substituem o conjunto anterior. O teste de reutilização falha com a implementação anterior e passa com a nova.

Também foram verificados cancelamento nomeado real durante o segundo passo e exatamente na saída por orçamento, documento sem identificador de origem alcançado apenas pelo refinamento, primeira parte incompleta e limite de candidatos insuficiente. Os testes existentes preservam Unicode, marcadores LONG/HEX, documentos removidos, palavras hexadecimais e interseção com uma parte menor rara.

O teste isolado inclui os módulos de produção `engine/text.rs` e `operations.rs`, usando as dependências já compiladas: 24 testes passaram, nenhum falhou e dois benchmarks explícitos ficaram ignorados. A admissão de análise é representada como ausente no adaptador; não é uma execução Tauri completa. Nenhum benchmark de 50 milhões de registros ou ganho de latência é afirmado. Outros segmentos, catálogos ou predicados ainda podem exigir o caminho geral.

## Fundamentação

A confirmação exata de um superconjunto completo segue o contrato de duas fases descrito pelo Lucene. O refinamento pode reduzir o trabalho, mas não precisa terminar para tornar um superconjunto anterior válido. Isso não autoriza usar uma coleta interrompida como resposta. [Código primário: TwoPhaseIterator](https://raw.githubusercontent.com/apache/lucene/main/lucene/core/src/java/org/apache/lucene/search/TwoPhaseIterator.java)

Artefatos locais reproduzíveis: `output/text-refinement/check.sh`, `harness.rs`, `tests.log` e `before.log`. O ajuste da ordem de consulta ao cache de filtros completos é uma proposta separada.
