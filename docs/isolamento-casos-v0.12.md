# Interpretação independente por Caso (0.12)

Cada Caso conserva sua própria interpretação junto do contexto persistido:

- Catálogo de códigos do usuário e catálogo extraído do sistema operacional
- Configuração de data/hora por caminho de arquivo
- Definições de formatos personalizados
- Regras/ajustes de detecção, fontes Sigma e catálogo de ameaças
- Preferências de cotas de trabalho do Caso

Os campos derivados, as referências e a visibilidade continuam usando seus domínios anteriores. Arquivos, filtros e estado de navegação também pertencem ao Caso. Um Caso novo não recebe fontes, filtros, ajustes ou definições mutáveis do Caso anterior. As definições internas fornecidas com o programa são modelos imutáveis.

## Consultas e editores

A admissão captura a identidade completa e um snapshot imutável antes de enfileirar trabalho. Consultas não voltam ao catálogo global depois disso. Os editores exigem `caseId`, `analysisId`, `configRevision` e `visibilityRevision`; o salvamento compara a revisão na mesma transação e devolve `analysisContext` atualizado. Uma resposta atrasada de A não altera o estado de B.

Um catálogo salvo só afeta consultas futuras daquele Caso. A extração do catálogo do sistema grava no Caso escolhido. O arquivo global `codes.json` exposto como caminho legado/modelo não é o catálogo efetivo dos Casos.

A alteração de data/hora prepara metadados substitutos antes de salvar. Falha/cancelamento anterior ao commit preserva a fonte publicada e a configuração anterior. Uma fonte aberta de outro Caso é recusada. Sem fonte aberta, pode-se salvar a configuração para a próxima abertura. Fontes antigas em memória exigem reabrir o original para remover um ajuste, pois o timestamp original não pode ser reconstruído com segurança apenas pelo valor ajustado.

Formatos personalizados salvos passam a valer na próxima abertura de suas fontes. Um índice já aberto mantém o parser com que foi produzido até essa reabertura; a interface informa essa necessidade.

A referência forense `event_ref` continua baseada na origem. Trocar parser, catálogo ou timezone não altera essa referência. Mapas de arquivos e caches imutáveis podem ser compartilhados quando as assinaturas de conteúdo e interpretação coincidem. Nenhum original é copiado, movido ou reescrito automaticamente.

## Biblioteca remota compartilhada

A biblioteca de conexões e o cofre de credenciais são uma exceção técnica explícita:
continuam no perfil do aplicativo. A lista global só é exibida ao abrir a biblioteca.
Ao restaurar um Caso, o aplicativo também consulta a biblioteca para resolver exclusivamente
uma conexão que já foi escolhida e salva naquele Caso; não escolhe nem exibe outra conexão.
Outro Caso não recebe automaticamente a conexão, endpoint ou rascunho do anterior.
A seleção da conexão e a consulta são do Caso; uma conexão removida não é substituída
silenciosamente por outra. Importações concluídas depois de uma troca ou remontagem
não abrem fontes no Caso que estiver ativo por acaso.

A persistência portátil contém apenas a referência da conexão e os campos permitidos
da consulta. Endpoint, usuário, senha e rascunhos de credenciais não são serializados
nem copiados ao duplicar/exportar um Caso. A biblioteca compartilhada não constitui
isolamento físico de todas as configurações do aplicativo.

## Migração e arquivos portáteis

Na atualização local de 0.11, os valores globais existentes são copiados uma única vez para snapshots independentes dos Casos preexistentes. Mudanças posteriores nos arquivos globais não alteram esses snapshots. Arquivos legados inválidos ou grandes demais ficam intactos. O Caso recebe diagnóstico por domínio e as consultas ficam bloqueadas com `CASE_INTERPRETATION_REPAIR_REQUIRED` até o editor correspondente salvar uma configuração válida. Um ajuste de outro domínio não elimina esse bloqueio.

A visualização de gerenciamento e seus recibos transportam identidade, campos/referências e diagnósticos, sem repetir catálogos grandes para todos os Casos. Editores buscam seus dados dedicados; o backend e a exportação portátil conservam o snapshot completo. Abrir ou exportar um Caso nativo0.11 inicia a migração antes da leitura, mesmo sem uma consulta analítica anterior.

Recuperações novas incluem os arquivos legados de interpretação e Sigma como ativos verificados. Uma recuperação antiga que não os registrou continua íntegra e pode ser restaurada/adotada, consultada na visualização preservada e exportada sem alterar os registros. A ausência de configuração original produz `legacy_interpretation_unavailable_origin` e bloqueia apenas a reanálise dependente dela. O perfil de destino não é usado como substituto. É necessário recuperar a interpretação do perfil original para reativar essa análise; isso não impede inspecionar ou preservar os bytes.

Uma importação antiga sem interpretação embutida recebe somente padrões internos e um diagnóstico. Nunca herda ajustes do perfil de destino. Exportações 0.12, duplicações explícitas, recuperação e transferências nativas carregam a interpretação junto com o contexto e atribuem a identidade local conforme o fluxo existente.

O esquema de campos derivados/referências permanece v1; o envelope do contexto adiciona `interpretation` tipado e opcional para leitura de arquivos antigos. O leitor nativo mantém seus codecs numéricos protegidos. A compatibilidade é de leitura de arquivos anteriores pela 0.12: leitores 0.11 com rejeição de campos desconhecidos podem rejeitar exportações 0.12. Não se deve apagar o campo para forçar a abertura, pois isso descartaria interpretação.

A interpretação admite até 3 MiB serializados, dentro do limite existente de 4 MiB do contexto completo; as definições de segurança têm limite próprio de 2 MiB. O salvamento excessivo é recusado sem substituir a configuração anterior.

## Limites de recursos

As cotas por Caso cobrem trabalho contabilizado pelo programa, não memória RSS nem reserva exclusiva de DuckDB, threads de parser ou disco de spill. Padrões do processo e tetos de segurança da máquina continuam globais. Consulte [configuração de recursos](configuracao-recursos.md) para a abrangência precisa.

## Verificação

Os testes nativos incluem o mesmo arquivo em dois Casos com catálogos, timestamps e parsers distintos; identidade forense estável; snapshot admitido imutável; CAS atrasado; reabertura; duplicação/importação anterior sem herança do destino; migração local única; preservação e bloqueio de legado inválido. Os testes de frontend cobrem respostas e salvamentos A→B, recuperação de fontes e novos Casos vazios.

## Preferências visuais legadas

Favoritos, densidade e modo compacto são restaurados do contexto pertencente ao Caso. Um Caso novo ou sem contexto salvo começa com favoritos vazios, densidade confortável e modo compacto desligado. As chaves globais antigas permanecem preservadas, mas não são copiadas automaticamente para um Caso sem atribuição comprovada. Cliques e salvamentos atrasados conferem o proprietário antes de alterar o contexto.
