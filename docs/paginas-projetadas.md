# Páginas projetadas: contrato experimental

Esta interface nativa é uma etapa de desenvolvimento da versão 0.10. A tabela da aplicação continua usando `query_page` e registros completos. Os novos comandos ainda não são usados pela interface, pelo MCP nem pelo arquivo de exclusões.

## Problema observado

Limitar uma página a 100 linhas não limita seus bytes. Em um fixture sintético de 100 registros com um campo não exibido de 64 KiB, a resposta existente ocupa 6.625.157 bytes. Uma projeção de cinco colunas do mesmo fixture ocupa 46.546 bytes. Essa comparação mede serialização, não latência de WebView, RSS ou desempenho com 50 milhões de registros.

A correção imediata libera a capacidade de `raw` nas páginas que já omitiam esse conteúdo. No mesmo fixture, as alocações pertencentes aos registros caem de 13.280.900 para 6.706.300 bytes, com resposta serializada idêntica. A estimativa de payload também passa a contar originais de campos derivados, diagnósticos, proveniência e capacidades retidas de strings; ela não representa todas as alocações do processo.

## Contrato separado de evidências

`query_projected_page` devolve uma estrutura com tipo e versão explícitos, recibo da análise, referências das linhas e células ordenadas pelas colunas solicitadas. Cada célula distingue ausência, nulo, valor completo tipado, prévia e indisponibilidade. Uma prévia nunca é um `Event` e não pode entrar em um slot de evidência ao salvar um Caso.

O planejamento da seleção compartilha filtros, ordem, desempate, cursor e verificação canônica com a página completa. A projeção não descarta linhas para caber no orçamento: uma célula pode ficar indisponível, ou a requisição falha explicitamente quando seu envelope é inviável. O primeiro caminho hidrata um registro por vez. Ele ainda não lê colunas canônicas diretamente do armazenamento colunar, portanto não elimina todo parsing de origem.

O recibo vincula a identidade/revisões do Caso, a geração da fonte ou publicação das evidências, e o conteúdo/época dos catálogos. A leitura de catálogo abrange sua publicação e o enriquecimento de fontes em memória. Mudanças durante uma ação invalidam a resposta, inclusive uma sequência de catálogo A → B → A.

## Limites da primeira etapa

| Componente | Limite |
| --- | ---: |
| Colunas solicitadas | 64 |
| Célula serializada | 4 KiB |
| Resposta projetada serializada | 2 MiB |
| Reserva de cursor serializado | 128 KiB |
| Registro de origem antes da hidratação projetada | 2 MiB |
| Payload estimado de um registro hidratado | 4 MiB |
| Estrutura inspecionada | 16.384 nós, profundidade 32 |
| Hidratação de registros completos | 16 linhas e 2 MiB, integral ou erro |

Os limites do projetor não incluem toda a memória de parsing, a seleção SQL/residual, nem a captura de uma fonte já residente em memória. Os motores continuam sujeitos aos seus próprios orçamentos. Pedidos com combinações inviáveis de linhas, colunas e bytes são recusados antes da seleção.

## Ações exatas e precisão

`hydrate_projected_rows` resolve referências completas apenas dentro da admissão correspondente, confere visibilidade/proveniência e retorna todos os registros pedidos ou erro. Não elimina originais, diagnósticos ou valores grandes para fazê-los caber.

O transporte JSON comum perde precisão em inteiros maiores que o intervalo seguro de JavaScript e muda a representação de números como `1.0` e `-0.0`. Isso também pode mudar uma chave tipada de referência. A nova hidratação integral recusa essas formas recursivamente. Um `request_time` válido de Nginx igual a `1.000` é um exemplo comum dessa limitação, não apenas um caso extremo.

`hydrate_projected_field` oferece uma ação separada para cópia/filtro: retorna presença, tipo e texto canônico produzido pelo mesmo resolvedor de campos da consulta. O texto preserva números, ordenação de chaves estruturadas, timestamps e aliases de entidades sem depender da reserialização de um objeto JavaScript. O envelope `exact_field` também é recusado em slots de evidência.

Ativar páginas projetadas na interface exige adaptar seleção, detalhes, cópia, filtros, trilhas, amostras de transformação e envio ao Caso para essas ações. Registros numéricos que exigem preservação sem perdas precisam de um caminho nativo de transferência de evidência antes dessa ativação. A API atual de registros completos não foi substituída.

## Validação

Os testes focados cobrem identidade/cursor/ordem, recuperação pelo motor de linhas, campos largos ocultos, Unicode e escaping, tipos nulo/ausente/vazio, limites reais de serialização, roles antes da abreviação visual e precisão numérica. Os resultados de harnesses isolados não substituem a compilação e os testes de admissão integrados. Suíte completa, medição de RSS e comparação de escala continuam sendo gates da validação final.
