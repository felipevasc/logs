# Reutilização de programas de regex

A validação de filtros, a preparação para execução e a cópia usada pelo plano SQL construíam novamente a mesma regex. Condições de campos derivados também podem repetir essa preparação por registro. O cache de `query_regex` compartilha somente o programa imutável da regex; não guarda expressões de consulta, catálogos mutáveis, resultados, SQL ou permissões de um Caso.

A chave inclui o padrão exato e todas as opções do construtor: flags, limites de programa/DFA e sintaxe. Cada consumidor recebe um clone com seu próprio estado de busca. Compilação, clonagem e destruição de programas retirados acontecem fora do mutex. Erros não são armazenados; uma regex válida continua válida quando não cabe no cache e é executada sem retenção.

## Limites

A retenção usa até 64 entradas, um limite separado para os bytes das chaves e créditos conservadores de programa: duas vezes a soma dos limites configurados de programa e DFA. O total de créditos corresponde a um oitavo do orçamento efetivo configurado, limitado a 96 MiB. Essa contabilidade não mede o RSS e não inclui clones ativos, compilações em andamento ou toda a sobrecarga da biblioteca.

No perfil configurado de 128 MiB, os 16 MiB de créditos permitem reter um literal de consulta, mas não uma regex comum, cujo crédito é 24 MiB. Essa regex continua funcionando sem cache. Padrões diferentes acima da capacidade podem provocar recompilações sucessivas; o teste com três padrões e duas posições registra nove compilações para nove pedidos. Não há promessa de ganho nesse cenário.

## Evidência focada

No snapshot nativo `f10c6ee`, os testes percorrem a validação real, a preparação e o planejador SQL:

- Um padrão comum repetido nessas etapas precisa de uma compilação em oito solicitações quando os créditos permitem retenção; com 16 MiB de créditos, permanece em oito
- Uma expressão com literais e listas com curingas precisa de quatro compilações em 24 construções solicitadas
- Flags, mensagens de erro, Unicode, limites distintos, retirada de entradas, concorrência e cancelamento preservam o comportamento anterior

Esses números contam compilações. Não são medidas de latência da aplicação nem benchmarks com 50 milhões de registros. O cancelamento é observado antes/depois de cada construção e entre itens de lista; ele não interrompe internamente uma chamada síncrona ao construtor da biblioteca. O cache tampouco estabelece um limite agregado para todos os programas ativos de uma consulta.

Verificação: `query_regex::tests`, `query::regex_program_reuse_tests` e `engine::sql::regex_program_reuse_tests`, com `--test-threads=1` no binário de testes nativo.
