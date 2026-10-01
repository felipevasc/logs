# Reutilizar o resultado completo antes das buscas de texto

O cache de termos e o cache do filtro completo compartilham um LRU limitado a oito entradas. Antes, cada cálculo de contagem, estatística ou agregação resolvia os termos de texto antes de procurar o resultado completo. Oito termos podiam renovar essas entradas e expulsar o próprio resultado que a consulta seguinte pretendia reutilizar.

A consulta agora procura primeiro uma seleção completa já existente, quando o plano pode produzir esse tipo de seleção. Predicados nativos simples continuam sem serializar uma chave desnecessária. Não há índice novo, preenchimento antecipado do cache de páginas nem alteração dos limites de retenção.

A chave continua vinculada ao conjunto inteiro de filtros, catálogo e contexto admitido da análise. A seleção retém sua tabela por `Arc`; o caminho de acerto continua verificando a visibilidade obrigatória. Regras mutáveis de detecção/ameaça continuam fora desse cache, e cancelamentos são verificados antes do acerto.

## Verificação focada

Um teste nativo usa 80 registros, oito termos obrigatórios e uma condição adicional que exige conferir a linha original. A primeira contagem encontra exatamente 40 registros e executa oito sondagens de texto. A segunda retorna os mesmos 40 registros com zero novas sondagens, preservando as oito entradas do LRU. O teste também verifica cancelamento e diferenças de chave para alterações de filtro e catálogo.

Esse teste e outros quatro testes de cache/cancelamento passaram. Os 21 testes existentes de admissão e visibilidade também passaram no mesmo binário. A nova fixture não mede latência, RAM ou arquivos de 50 milhões de registros, e não substitui a suíte completa.
