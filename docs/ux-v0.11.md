# Experiência desktop da versão 0.11

## Direção

Refinar a organização atual, que o usuário quer preservar, com aparência escura, futurista e compacta de aplicativo desktop. A referência de interação é o Windows Explorer: clique, arraste e menu de contexto; capacidades acessíveis sem expor todos os controles permanentemente. O ganho de espaço vem da hierarquia e da redução de repetição, sem diminuir indiscriminadamente texto, zoom ou alvos.

Nas esperas, a animação é o foco visual, acompanhada de uma ação curta e contagens reais. Misturar objetos e um personagem discreto quando fizer sentido. Esperas curtas pedem gestos simples; esperas longas podem ter uma cena pequena, fluida e variada em loop. Informações técnicas ficam sob demanda, mantendo erros e ações importantes acessíveis.

Desenvolvimento autorizado em 02/10/2026. A release 0.11 requer autorização própria. Base inicial local `97e5ad388f2b96111e12933cbc2de3eb14c7939e`, árvore `458245af9c48ba4d62c6a5a5aec2bbf0f38807b0`, equivalente ao candidato remoto 0.10 `675d7798c561535ab0fa7d4c55727db59781b7ba`. A 0.10 foi integrada à main em `4399c7777d76097bd13e72fac92dd7723e301fab`, com a mesma árvore, e publicada; o checkpoint remoto da 0.11 parte dessa main sem duplicar seu histórico.

## Primeiro recorte

1. Menu compartilhado com foco, navegação de teclado, retorno ao alvo válido e fechamento de apenas uma camada por Escape
2. Componente leve de espera, guiado por estado real, com equivalente estático e controle de movimento
3. Pilotos em abertura/indexação, checkpoint e cálculo, para validar a linguagem visual antes de expandir
4. Testes focados de interação, cancelamento e contexto, além da comparação visual em janela compacta

O primeiro recorte não resolve sozinho o modelo de teclado de toda a tabela, a reorganização de todos os painéis ou o sistema visual completo. Essas mudanças seguem em jornadas verificáveis.

## Prioridades da experiência

- Acesso às ações por mouse e teclado, preservação de posição/foco e ampliação legível
- Feedback consistente: menos mensagens repetidas e distinção entre preparado, preservado, confirmado e disponível para análise
- Menu contextual com alvo claro; cópia e filtro usam o valor exato, nunca o preview
- Lista de registros como área principal, Timeline compacta e detalhe sob demanda
- Formulários e diálogos com estrutura/ações previsíveis e rascunho preservado em erro recuperável
- Tokens por função e estado, com contraste verificado nos temas claro e escuro
- Catálogo, painel e configuração de pivot comparados por leitura e próxima edição, sem mudança de layout baseada apenas em preferência estética
- Exatidão, remoção por ocorrência, Undo e exportação reutilizam os contratos nativos da 0.10

## Cobertura tela a tela

| Grupo | Superfícies | Verificação principal |
|---|---|---|
| Entrada e origem | Abrir/importar, Arquivos/Estrutura, Conexões | Próximo passo claro, origem válida preservada, cancelamento/erro localizado |
| Orientação | Resumo e Comprometimentos | Cobertura explícita; indício e evidência não confundidos |
| Investigação | Explorar, Resumir, Descobrir, Cruzar dados | Ações de valor alcançáveis, dados legíveis, configuração sem disputa de espaço |
| Tempo e relações | Timeline de volume, Timeline do Caso, possíveis trilhas, trilhas guardadas | Intervalo, chave e ocorrências identificáveis; proximidade não apresentada como causa |
| Evidência | Itens do Caso, detalhe/JSON/bruto, exceção Java | Original/preview/indisponível explícitos; leitura progressiva e retorno ao ponto |
| Configuração da análise | Transformação, referências, arquivo de exclusões | Prévia deliberada, rascunho preservado, alcance e reversibilidade corretos |
| Administração e saída | Configurações/catálogos/formatos, atualização, exportação/relatório/Caso portátil | Escala e recuperação claras; formato e proteção compatíveis; resultado confirmado |

## Esperas e personalidade

O estudo considera 67 pontos de tarefa e subetapas nativas em onze famílias: ler, organizar, guardar, conferir, relacionar, calcular, transformar, transferir, compor, retomar e esperar. Não equivale a 67 animações independentes: o repertório deve ser pequeno e reutilizável.

Variação segue tarefa e etapa reais; dentro da mesma família, poucas alternativas coerentes evitam repetição imediata. Não sortear uma nova cena a cada atualização do contador. Uma tarefa instantânea não ganha atraso para mostrar arte. Várias regiões simultâneas não devem produzir vários personagens competindo pela atenção.

- Indexação pode organizar cartões; cálculo pode agrupá-los numa grade
- Checkpoint pode guardar cartões numa bandeja, distinguindo gravação, sincronização e confirmação parcial
- Copiar pode duplicar uma folha num gesto curto; PDF pode compor páginas conforme as etapas efetivas
- Excluir logicamente usa arquivo/pasta, não triturador; restaurar retorna o marcador
- Fila, pausa, cancelamento pendente e erro não mantêm um gesto que pareça trabalho produtivo

O texto visível permanece curto. X/Y e etapa N/M só existem quando a origem fornece denominador válido. Sem percentuais fictícios, sequência por temporizador, sucesso antes do recibo ou resultado retido para terminar o loop.

## Contratos preservados

- Toda ação conserva identidade de fonte, Caso, ocorrência e revisão
- Resposta antiga não atua no novo contexto
- Preservação, legibilidade no detalhe e disponibilidade analítica são estados distintos
- Confirmação de salvamento perdida não implica rollback; repetição usa o pedido existente
- Undo só é oferecido enquanto o recibo ainda for válido
- JSON, LICASE e relatório têm capacidades diferentes; proteção de texto não é desligada automaticamente para permitir uma exportação
- Preparar recuperação não ativa o perfil no processo atual; a seleção depende da reabertura
- Os loadings não adicionam consultas nativas, cópias de registros ou polling por frame

## Aceite e validação

Testar menus e camadas por teclado, ação repetida, clique externo, Escape, alvo removido e troca de contexto. Avaliar tema claro/escuro, janela compacta e preferência de ampliação; não confundir preview Chromium com WebView instalado.

Comparar esperas curtas, longas, muitas etapas rápidas, atraso de progresso, cancelamento, falha e retomada com o mesmo roteiro. Medir compreensão, distração após repetição, acesso a ações e custo de renderização, além de preferência visual. O formato visual não tem aprovação definitiva até essa comparação.

Movimento reduzido preserva uma versão estática completa. Oferecer controle de animação decorativa sem cancelar o trabalho. Evitar flashes, sacudidas, câmera e deslocamento de grandes regiões. Anunciar estados importantes sem narrar cada frame. A ampliação instalada até 200% e o reflow em largura equivalente a 320 CSS px são verificações distintas.

Nesta fase, usar testes JS/Node e browser focados. Não criar workload de grande volume nem disparar compilação Rust para avaliar posição de botão ou movimento. O modo big data permanece fora do recorte.

## Referências

- [GitKraken e feedback de usuários](https://feedback.gitkraken.com/suggestions/348509/revert-loading-screen): há elogio ao mascote no uso diário e pedido de alternativa sem animação; são relatos, não teste controlado
- [GitHub Motion](https://brand.github.com/motion/principles): família visual consistente; inclui exemplos direcionais e de marketing
- [Duolingo e movimentos modulares](https://rive.app/blog/duolingo-s-ai-powered-video-call-brings-lily-to-life): variedade por componentes e contenção de expressões; relato de equipe publicado pelo fornecedor
- [Coffee2go](https://www.mdpi.com/2076-3417/13/2/865): experimento pequeno de espera curta, sem evidência de produtividade em logs
- [Motion in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/motion): coerência e personalidade contextual
- [WAI menus](https://www.w3.org/WAI/ARIA/apg/patterns/menubar/), [controle do movimento](https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide), [mensagens de estado](https://www.w3.org/WAI/WCAG22/Understanding/status-messages.html): contratos de interação e acesso

Usar esses casos como referência de comportamento. As ilustrações e animações do produto devem ser originais; não copiar mascotes ou assets sem licença apropriada.

## Primeiro checkpoint de desenvolvimento

Menus compartilhados e três famílias de espera estão implementados no frontend. Os pilotos usam abertura/indexação e checkpoint no overlay existente, além de Resumir e Cruzar dados. Controles de movimento não cancelam operações; cancelamento real interrompe a animação imediatamente, inclusive antes do primeiro progresso, mas aguarda a confirmação da tarefa.

Verificação local após os ajustes do primeiro CI: 701 testes Node, análise sintática dos scripts, consistência de versões, salvaguardas de release e preparação dos arquivos frontend aprovados. A revisão independente dos componentes e do vínculo entre tarefa e cena não deixou bloqueadores conhecidos. Nenhum backend Rust foi alterado ou compilado.

O primeiro CI de preview produziu capturas reais com transporte sintético e estados explicitamente atribuídos. O cenário dedicado do menu passou; sete dos treze roteiros encontraram regressões, incluindo seletores antigos, foco de retorno, scroll enfileirado e arredondamento do limiar de espera longa. Essas causas foram corrigidas com regressões locais. O CI49 confirmou os treze roteiros de preview aprovados, incluindo pausa/retomada, loop de espera longa, cancelamento pendente, descarte imediato, contexto e movimento reduzido, sem erros de página. As capturas foram conferidas visualmente nos temas e larguras dos pilotos. O ambiente local não permite iniciar o Chromium de testes, por isso a renderização é validada no CI. Essas capturas não validam o WebView instalado nem representam aprovação definitiva do desenho.

### Edição direta dos filtros

O rótulo de cada chip de filtro abre o editor existente por clique, Enter ou Espaço. A remoção permanece um botão irmão independente, e o menu contextual conserva inverter, editar e remover. Abrir e cancelar não iniciam consultas; salvar segue o fluxo de atualização atual. O foco acompanha a mesma referência de filtro e o mesmo contêiner, com fallback se forem removidos, sem substituir o foco que a pessoa moveu durante uma consulta pendente.

Operadores especiais já existentes, como listas exatas de ocorrências e detecções, são preservados na edição. Operador vazio ou não suportado não é aplicado e mantém o rascunho. O recorte não reduz texto ou altura dos alvos. Valores completos continuam disponíveis no editor e no nome acessível. O roteiro de preview compara 1440 e 1024 px. A verificação local deste incremento passou com 713 testes Node; o CI50 confirmou os treze roteiros de preview, incluindo a edição pelo chip e o operador de lista exata no select real. O roteiro das esperas também grava vídeo real com identificação do transporte sintético, sem alterar a duração das operações do produto.

### Integração da validação essencial

A infraestrutura já integrada à main foi incorporada preservando os dois pilotos no seletor de testes. A verificação local combinada passou com 725 testes Node. A comparação é acumulada contra a base do PR: este lote ainda requer a matriz nativa remota porque registra novos assets no empacotamento. A interface compartilhada conserva os dezenove roteiros completos de browser; alterações futuras realmente isoladas usam o conjunto essencial mais a área afetada. Não houve relaxamento das salvaguardas de instalação, atualização ou assinatura.

### Robô protagonista: direção aprovada

A primeira proposta de espera foi considerada discreta demais: o personagem parecia decorativo. A revisão mantém a identidade do robô e o torna agente da ação. O piloto de leitura/organização ganhou corpo, mãos, apoio alternado dos pés, contato com a folha, transporte, depósito e reação no visor. O ciclo longo dura 7,2 segundos; a espera curta tem gesto próprio de 2,4 segundos, sem acelerar o filme inteiro. A folha transportada é filha da mão, sem trajetória independente. Estados e contagens não são governados pelo movimento.

A direção do estudo isolado foi aprovada pelo usuário. O CI54 confirmou a integração com um ciclo completo gravado, comparação de dois cantos nas poses de contato e capturas escura/clara; os dezenove roteiros de preview passaram. As demais famílias só devem reutilizar essa linguagem após a validação do piloto. O tema claro tem contorno bronze de alto contraste e forced-colors conserva o desenho estático.

### Feedback de ações repetidas

Confirmações benignas equivalentes de cópia e filtro passam a ocupar um único aviso com contador discreto, sem narrar a mesma mensagem várias vezes. Erros, avisos de contexto e recibos não são agregados. O feedback gerenciado usa um contêiner separado, para não mover, encurtar ou ocultar o Undo; somente os sete avisos benignos são transparentes ao ponteiro. Mensagens de erro continuam selecionáveis e copiáveis.

O posicionamento evita foco, editor e ações quando houver espaço. Timers, frames, observer e listeners são encerrados ao remover o último aviso. A revisão independente fechou as regressões de geometria do Undo e seleção de erros; o roteiro próprio passou no CI55, inclusive geometria do Undo e seleção de mensagens de erro.

### Mais espaço no cruzamento

O construtor de Cruzar dados agora pode ser recolhido por uma escolha explícita. A apresentação fechada resume linhas, colunas e medidas e conserva a ação Editar configuração. O estado inicial continua expandido. Recolher não consulta novamente nem recria a tabela, e conserva foco e rolagem; a preferência é restaurada por contexto, com snapshots antigos assumindo o estado expandido.

A paleta local recolhe junto do construtor; o catálogo principal permanece disponível. Nenhuma capacidade de edição é removida. O CI55 mediu a área da tabela de 373 para 533 px (8 para 13 linhas) em 1440 px, e de 156 para 312 px (1 para 6 linhas) em 1024 px, mantendo a fonte e os dois temas. Encontrou também uma falha na reabertura do cache após trocar o contexto: uma linha ausente era tratada como uma coleção. A correção consulta a coleção de linhas real; regressões Node reproduzem o erro anterior. A repetição do fluxo completo no navegador está pendente.

### Ampliação acessível

Configurações e atalhos passam a oferecer escolhas até200%, mantendo o automático e Ctrl0 existentes. A preferência manual permanece ao redimensionar. Os grupos de tamanho/tema têm navegação por setas, Home/End e foco preservado; o trap do modal respeita os controles fora da ordem de Tab.

Escala solicitada e aplicada são distintas. Escritas nativas são serializadas; respostas antigas não anunciam sucesso nem substituem a intenção atual. Voltar para Automático durante uma escrita aguarda o recibo e recalcula pela escala confirmada e geometria observada. Uma falha mantém a preferência e informa que não foi aplicada.

O preview verifica seleção de200% e reflow dos controles em1024/640/320CSSpx. O transporte de preview retorna false para zoom; por isso não comprova zoom instalado a200% nem reflow global do app. Esses gates permanecem separados.

### Ações de registro por teclado

A tabela conserva HTML nativo e recebe uma coluna estreita de ações, com um botão nomeado por registro. Enter, Espaço e Shift+F10 abrem o mesmo menu contextual; Ver detalhes é a primeira ação, sem inventar um valor de célula. Uma ligação de salto permite alcançar a paginação sem atravessar os até 500 botões. A coluna de ações não participa de exportação, seleção de campos, ordenação ou larguras persistidas.

O retorno resolve a mesma ocorrência por referência e contexto, inclusive quando a consulta recria as linhas com o menu aberto. Registros removidos, referência substituída e contexto antigo usam a paginação. O detalhe focaliza seu controle de fechamento imediatamente enquanto carrega e não rouba o foco que a pessoa moveu. O catálogo também preserva se a origem era o nome do campo ou sua elipse. Este recorte não implementa uma grade completa nem seleção de intervalos por teclado.

### Novas ações do robô

O checkpoint usa a mesma personagem para segurar a gaveta, abrir, conferir e fechar; pasta e etiqueta reagem a essa interação. Cálculo usa uma ficha presa à mão, levada ao visor e depositada em um grupo. As cenas longas duram 6,8 e 6,4 segundos; os gestos curtos têm coreografia própria de 2,8 e 2,6 segundos. Nenhuma duração controla progresso ou retenção de resultado.

Os testes Node verificam a preservação do piloto de leitura, hierarquia dos objetos, contatos e estados. O roteiro de navegador foi ampliado para medir matrizes de contato no tamanho real de 240 × 120 CSS px, gravar ciclos completos e comparar temas, pausa, movimento reduzido e cores forçadas. Essa nova evidência visual aguarda o próximo CI. A gravação usa transporte sintético identificado e não valida o motor nativo.

### Regressões de continuidade

Os testes de menu passam a recriar deliberadamente o catálogo enquanto ele está aberto e exigem retorno ao controle lógico correspondente. O teste de edição por chip aguarda a barreira existente de contagens antes de medir consultas: tarefas ainda no debounce não estão incluídas em `Tasks.pending()`. A exigência de zero consultas ao abrir e cancelar continua estrita; não foi acrescentado atraso ao produto.

### Resultado do CI56 e ajustes localizados

O CI56 aprovou 809 testes Node e 19 dos 21 roteiros de navegador. Pivot, menus, chips e as novas ações de registro passaram. O seletor de tamanho revelou uma largura intrínseca maior que o painel; a correção limita a coluna e quebra os botões em linhas, sem reduzir texto ou alvos. O teste de pausa havia contado uma transição finita de 150 ms junto da cena; a medição passa a separar a arte das transições de interação e a registrar seu alvo. Não se mudou a duração ou o comportamento do robô para satisfazer o teste.

As novas cenas longas foram renderizadas em 240 × 120 CSS px. A mão manteve contato com a gaveta em 47 amostras do ciclo longo, com desvio máximo de 0,068 px. Na ficha, os dois cantos coincidiram nas poses de pegar e depositar. O gesto curto de checkpoint terminou em 2,8 segundos. Os estados posteriores do roteiro ainda aguardam a repetição completa. A captura também mostrou números antigos da tabela atrás da cena em Resumir: uma pequena superfície opaca local passa a separar arte e texto dos resultados anteriores.

### Diálogos, exportação e Desfazer

Referências e Transformação passam a manter status e ações em um rodapé estável enquanto o formulário rola. Cancelar/Salvar seguem a mesma ordem e Excluir permanece separado. Erros longos continuam selecionáveis, têm foco próprio e rolagem por teclado, sem roubar o foco quando chegam. Durante uma gravação, Fechar explicita que não interrompe nem desfaz a operação. Limites, tipos e avisos críticos não foram recolhidos.

A exportação agora calcula escopo e ajuda por formato tanto na abertura quanto na mudança: recorte filtrado, Markdown, PDF e investigação portátil têm descrições distintas. Contagem indisponível não vira zero. As diferenças de JSON nativo/legado, LICASE e proteção de textos ficam explícitas sem mudar valores de máscara, extensão, IPC ou recibos.

A confirmação de remoção nativa ganha Desfazer no próprio aviso, vinculado ao recibo exato. Ele permanece separado dos avisos benignos agregados. Sua validade continua pertencendo ao Caso, armazenamento e manifesto nativos; mudar a fonte analítica não invalida arbitrariamente o histórico. Cliques repetidos, confirmação perdida, recibo mais novo e troca de Caso têm regressões específicas. O prazo do aviso não altera a validade do comando existente no menu.

Esses três recortes passaram por testes Node e revisão de código. O próximo CI deve confirmar seus cenários reais de navegador, incluindo layouts compactos, foco, erros e cancelamento; isso ainda não equivale a validação no WebView instalado.

### Confirmações do CI57 e próximo piloto

O CI57 confirmou o reflow dos controles de tamanho em 1024, 640 e 320 CSS px, os rodapés e a navegação dos erros longos. O roteiro completo das três famílias de espera passou: ciclos curtos e longos, contato, pausa, cancelamento, descarte, temas, movimento reduzido e cores forçadas. A medição identificou a transição restante como `color` do botão por 150 ms; a arte pausada tem zero animações em execução. A base opaca de Resumir foi conferida nas capturas reais.

Dois roteiros novos pararam por inconsistências dos fixtures: um consultava a antiga lane de Undo para encontrar um erro de exportação; outro emitia uma preparação sintética sem os campos exigidos pelos leitores nativos. Ambos os testes foram corrigidos sem alterar o produto ou relaxar os validadores. A confirmação dos dois roteiros completos continua pendente.

No tema claro, a captura do pivot revelou rótulos azul-pálido. A correção usa o token funcional `accent-strong` da Análise ou do Caso, preservando o tema escuro e a geometria. O teste passa a medir o contraste composto sobre a superfície real dos chips, após terminar a transição de tema; a medição no navegador ainda será executada.

O próximo piloto acompanha somente a montagem real do PDF do Caso. O robô alinha uma folha em um gesto de 2,8 segundos; em esperas longas, alimenta e opera uma pequena prensa, recolhe e confere a prova em 7,6 segundos. As contagens vêm das ocorrências já desenhadas e trilhas já concluídas. O callback original de progresso permanece compatível; não foi criado denominador de páginas ou percentual por tempo.

Fechar, falhar ou terminar a montagem encerra timers e cena imediatamente. Ela sai antes da codificação e do salvamento, cujos contratos continuam próprios. Há regressões de cancelamento, retry, snapshot e save cancelado, além de um novo roteiro que gera bytes PDF reais com dados sintéticos. Esta composição ainda depende da primeira validação visual no CI.
