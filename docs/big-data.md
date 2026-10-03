# Grandes volumes na linha 0.12

A integração da 0.12.1 preserva o motor colunar DuckDB e a busca Tantivy já
incorporados à linha principal. A preparação dos índices é automática, embutida
no aplicativo e reaproveita checkpoints. Não é necessário instalar um servidor
Elasticsearch nem ativar o botão experimental da antiga base 0.5.

Consulte [Motor de consultas](motor-de-consultas.md),
[Desempenho da 0.12](desempenho-v0.12.md) e
[Validação de grandes volumes](desempenho-grandes-volumes.md) para a arquitetura,
as rotas de consulta, as medições e seus limites. O orçamento global e por Caso
fica em [Configuração de recursos](configuracao-recursos.md); o consumo observado
fica no [Gerenciador de recursos](resource-manager.md).

O protótipo da base 0.5 foi preservado no commit b899f40. Seus módulos
big_data.rs, comandos de ativação e benchmarks daquele motor não fazem parte
da versão integrada. Os números daquele experimento não representam medições
da 0.12.1, e não demonstram superioridade universal sobre Elasticsearch.

A revisão integrada acrescenta limpeza de snapshots interrompidos, cancelamento
antes da publicação de metadados, substituição atômica dos metadados Tantivy no
Windows e observação de recursos sem executar consultas nem percorrer eventos.
As regressões comparam resultados e exportações entre a rota de referência e o
motor atual. Ganhos de desempenho devem ser medidos na mesma carga, máquina,
configuração e estado de cache; equivalência de resultados é condição prévia.
