# Configuração de recursos

Em **Configurações → Recursos → Orçamento de memória do aplicativo**, escolha **Automático** (padrão recomendado) ou
**Personalizado** para o orçamento de referência do processamento. O valor é em
MiB, de 128 até a memória total detectada na máquina, sem teto fixo de 8 GiB
ou redução do máximo à metade da RAM. Por exemplo, 64 GiB permitem 65.536 MiB
e 128 GiB permitem 131.072 MiB. O detector usa a RAM total, não a RAM livre, e
considera limites de contêiner Linux, inclusive de grupos ancestrais visíveis.

Automático usa aproximadamente um terço dessa memória (mínimo de 128 MiB,
sem ultrapassar o total detectado), deixando folga para o sistema e outras
alocações. Personalizado permite escolher todo o total detectado, com aviso
sem bloquear a escolha: usar esse máximo pode causar lentidão ou encerramento
por falta de memória. O aviso também aparece se o orçamento ativo usa o total.

O orçamento distribui referências para buffers de DuckDB, indexação textual,
lotes e cache de seleções. A memória é usada sob demanda: não há pré-alocação ou
reserva desse valor. **Não é um limite rígido de RSS nem da memória total do
processo.** Arquivos mapeados, metadados, bibliotecas e sessões SQL simultâneas
podem usar memória adicional. Limites DuckDB por instância não devem ser somados
como garantia de um teto global de memória. O paralelismo tem um controle agregado
separado, descrito abaixo. O writer textual também conserva um limite próprio
inferior a 4 GiB porque a arena do Tantivy usa endereços internos de 32 bits;
esse limite aparece no valor efetivo do componente, sem reduzir o máximo
configurável do aplicativo. Conversões para tamanhos de alocação são limitadas
à capacidade de endereçamento do processo, sem truncar valores grandes.

## Salvar e aplicar

- O painel distingue os valores **ativos nesta sessão** da preferência **salva
  para o próximo início**
- Salvar preserva os buffers e trabalhadores da sessão; não reinicia consultas,
  não reabre fontes e não invalida o conjunto de dados
- Feche e abra o LogInsight quando concluir as tarefas em andamento para aplicar
- Voltar a Automático e salvar pode cancelar uma alteração ainda pendente
- A preferência fica no perfil selecionado, em `resource-settings.json`. A
  escrita usa um arquivo temporário no mesmo diretório e substituição atômica;
  configuração inválida é ignorada com aviso, sem sobrescrever o original
  A confirmação de gravação não promete durabilidade contra queda de energia;
  o conteúdo temporário é sincronizado, mas o diretório não tem sync garantido
  em todas as plataformas
- Se um perfil for aberto em máquina com menos memória, o orçamento efetivo é
  reduzido ao teto local, mantendo a preferência original
- Orçamento pequeno também ativa a preparação conservadora (um gravador e
  lotes menores), mesmo em computador com muita RAM

`LOGINSIGHT_MEMORY_LIMIT_MB` continua disponível e tem prioridade sobre o painel.
O painel informa essa prioridade e o valor efetivo. Remova a variável antes de
reiniciar se quiser aplicar a preferência salva. As demais variáveis avançadas
de buffers e spill permanecem independentes e podem alterar os valores exibidos.

## Paralelismo global

O painel também permite escolher **Automático** ou um **Limite personalizado**
independente do modo de memória. O valor inteiro vai de 1 ao número de processadores
lógicos disponíveis para o processo, no máximo 64. Automático reserva capacidade
para a interface usando um a menos que o disponível, nunca abaixo de 1. Um perfil
movido para máquina menor mantém a preferência salva e reduz o valor ativo.
A mudança vale no próximo início.

O agendador debita unidades de trabalho CPU gerenciado em um teto compartilhado
entre Casos, consultas e preparação em segundo plano no mesmo processo do
LogInsight. Outras instâncias do aplicativo não compartilham esse contador.
Cada raiz entra antes dos
locks de execução; filhos de streaming transferem uma faixa contabilizada quando
o pai aguarda, e ondas paralelas só emprestam capacidade global ociosa. SQL e
indexação textual não criam pools de execução que multipliquem o limite por sessão.
O tamanho de lotes continua limitado separadamente pelo orçamento de memória.

Página e detalhe têm prioridade de admissão, com envelhecimento de fila para que
trabalhos normais e background não sejam adiados indefinidamente. O background cede
entre lotes seguros. A espera na fila observa cancelamento; tarefas já em execução
continuam usando seus pontos cooperativos. Esperas HTTP sem locks/filhos cedem a
capacidade global e readquirem antes do processamento, mantendo os timeouts de rede
existentes. Isso não torna uma chamada HTTP ou SQL não cooperativa instantaneamente
interrompível.

O limite é de **trabalho CPU coberto pelo agendador**, não da contagem total de
threads do sistema operacional: runtime, interface, I/O, watchers e manutenção
interna das bibliotecas continuam existindo. Ele tampouco reserva CPU ou limita RSS.
A validação de concorrência, captura de Caso e plataformas é obrigatória antes da
publicação; veja o relatório de validação da versão.

## Cota independente por Caso

A seção **Cota lógica deste Caso** oferece **Herdar limites do aplicativo** ou uma
cota personalizada de **volume lógico contabilizado**, em MiB. A cota é salva
junto da interpretação do Caso, participa de sua revisão/CAS e acompanha sua
exportação portátil. Não é uma preferência global de perfil.

- Herdar mantém os limites individuais existentes de materialização, trabalho,
  IDs completos, valores analíticos, seleção SQL e cache de seleção
- Personalizar aceita uma preferência de 8 MiB até a memória total detectada,
  incluindo valores de 64/128 GiB quando a máquina possui essa capacidade
- O painel distingue a **preferência salva** da **cota lógica efetiva**. A efetiva
  é limitada pela soma real dos pools globais de trabalho e seleção lógica,
  pela memória detectada e pela capacidade de endereçamento do processo.
  Uma preferência maior fica salva sem ampliar implicitamente os limites
  individuais de materialização, IDs, DuckDB ou cache
- O orçamento de memória do aplicativo fica no mesmo painel e pode ser ajustado
  para o próximo início. Os limites individuais e de seleção lógica continuam
  independentes; aumentar a preferência do Caso não os remove
- Uma preferência portátil maior que a capacidade local é mantida no arquivo,
  com valor efetivo reduzido e aviso explícito no painel
- A política efetiva é capturada antes de enfileirar a consulta. Alterações valem
  para novas admissões, sem editar a política de trabalhos já admitidos
- Todas as revisões de um mesmo Caso compartilham o débito enquanto existirem
  consumidores. Outro Caso tem seu próprio contador, sob os tetos compartilhados
  do aplicativo. A identidade inclui a raiz do perfil/armazenamento
- IDs de consulta, orçamento de agregações/rankings e materialização analítica
  das evidências usam leases. Seleções SQL contam 8 bytes lógicos por ID, inclusive
  quando o DuckDB armazena/spilla esses IDs fora da RAM
- Uma seleção removida do cache continua debitada até seu último consumidor e o
  DROP efetivo da tabela, ou fechamento definitivo da base. Caches inativos
  elegíveis são liberados antes de uma recusa por pressão; consumidores ativos
  mantêm crédito. Sob pressão apenas de um Caso, o cache de outro Caso é
  preservado. Outros Casos participam da remoção somente quando o teto agregado
  compartilhado correspondente é atingido
- A soma agregada é separada: trabalho de evidências/resultados conserva seu teto
  existente (até 128 MiB); o pool de seleções lógicas usa o limite existente de
  seleção (normalmente 1–8 GiB). A soma é um volume contabilizado, **não RSS**

O controle cobre as agregações/rankings e coleções de IDs debitados pela admissão,
além da materialização analítica nativa. Paginação, hidratação de registros/campos
e seus clones conservam os limites específicos já existentes; esses buffers não
são incluídos nesta cota por Caso.
Histórico/prévias preservadas, recuperação, migração, importação/exportação e
metadados de configuração anteriores à admissão continuam com os limites
agregados existentes do aplicativo. Buffers de bibliotecas/Arrow, índices, fonte
mapeada e memória do motor não entram nessa cota. As reservas analíticas de trabalho permanecem
retidas durante sua admissão; elas não declaram um limite de memória da
serialização IPC ou do navegador.

**Não há controle por Caso da RAM do DuckDB, número de instâncias, threads dos
motores ou espaço de spill.** Os controles globais continuam explicitamente
separados. Uma recusa `CASE_WORK_BUSY` não inutiliza uma sessão SQL. Pode exigir concluir
outras consultas, liberar consumidores em uso ou aumentar a cota do Caso antes
de repetir uma operação cujo volume não cabe no limite.

## Limites deste incremento

O agendador limita execução compartilhada, não a quantidade de sessões SQL
residentes nem a soma de suas alocações. SQL já iniciado, finalização de segmento
textual e outros trechos sem ponto cooperativo não são preemptados por uma página
nova. Com limite 1 isso pode aumentar a espera até a próxima fronteira segura.
Reduzir o orçamento ou o paralelismo não estabelece um limite absoluto de memória
do processo.

Pastas de índices/cache e temporários, migração entre volumes e quotas globais de
disco não são alteradas por este painel. Esses controles dependem de validação de
caminhos, leases e recovery próprios; não são apresentados como parcialmente
funcionais. Checkpoints de metadados usam limiares em fronteiras seguras, com
rollback por wave; veja a [política e medição](performance/metadata-checkpoint-measurement.md).

## Verificação

- `node --test scripts/tests/resource-settings.test.mjs`
- `node scripts/preview/run-smoke.mjs test-resource-settings.mjs`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib resource_settings::tests`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib resources::tests`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib global_scheduler::tests`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib case_resources::tests`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib case_work_budget::tests`

O preview usa transporte sintético e simula reinício ao recarregar a página.
Ele verifica o comportamento da interface, sem validar o consumo real de RAM,
persistência nativa, desempenho ou o WebView instalado.
