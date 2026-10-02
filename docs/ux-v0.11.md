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

O posicionamento evita foco, editor e ações quando houver espaço. Timers, frames, observer e listeners são encerrados ao remover o último aviso. A revisão independente fechou as regressões de geometria do Undo e seleção de erros; o roteiro de navegador próprio ainda depende de CI.
