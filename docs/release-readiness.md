# Verificação de prontidão

Revisão iniciada em 3 de outubro de 2026. Este documento registra evidências de
validação; não representa uma publicação nem uma garantia de desempenho para
qualquer carga.

## Base da entrega

O checkout iniciou em `4fe0d85` (versão declarada 0.5.0), com alterações locais de
Big Data, recursos e confiabilidade. O remoto já contém `v0.12.0` em `650dbe0`.
A escolha entre integrar essas alterações à linha atual e manter uma linha
separada está sendo confirmada antes de numerar ou empacotar uma release.
Publicar a base antiga como sucessora sem essa reconciliação perderia recursos.

## Evidências desta rodada

| Verificação | Resultado |
| --- | --- |
| Salvaguardas de publicação | Aprovadas: cinco formatos, conflito de tag, release já publicada, digest e publicação apenas após seis uploads. Teste sem rede. |
| Dependências npm | DOMPurify transitivo atualizado; `npm audit` terminou com zero avisos conhecidos. |
| Regressões JavaScript | `npm run check` aprovado; 21 cenários de autosave, escopo, inspetor de valores, supervisão de processos e salvaguardas de publicação. A correção final de precisão passou também pelo teste unitário e por 300 casos decimais adicionais conferidos independentemente. |
| Interface | A rodada completa executou 32 cenários: 31 aprovados, incluindo recursos, Big Data, exportações, autosave e inspetor. O cenário de Descoberta encontrou a árvore vazia ao abrir dados em cache; após a correção, ele e os cenários relacionados de contexto e análise passaram. A regressão reproduz a falha anterior e comprova que a correção não acrescenta consultas. |
| Backend Windows | A execução anterior aprovou 221 testes e ignorou 16 testes manuais/condicionais. O binário da suíte completa mais recente compilou, mas sua execução foi bloqueada pelo Defender (erro 225); essa execução não foi aprovada. |
| Backend Linux | 225 testes aprovados; 14 testes manuais/condicionais ignorados. Inclui regressões para diagnóstico e preservação de I/O do índice. |
| I/O focado no Windows | Três testes aprovados usando os mesmos módulos de produção: substituição com leitor aberto, recusa de compartilhamento incompatível e cancelamento. Esse alvo complementa a suíte completa; não elimina seu bloqueio registrado acima. |
| Automação desktop no Windows | Doze ciclos completos consecutivos aprovados após a correção de I/O. O executável final, recompilado com o inspetor e a correção da árvore, passou por mais um ciclo completo: nove consultas em ambos os motores, quatro exportações idênticas byte a byte, histórico/processos, fechamento com falha de gravação e reabertura após gravação real. Sem erros JavaScript. |
| Inspetor no desktop real | JWT sintético carregado por IPC real: ocultação inicial, revelação, Unicode, arrays de permissões, ID `9007199254740993` sem arredondamento, limpeza ao ocultar e retorno à proteção ao fechar/trocar registro. |
| Executável final e Defender | A varredura personalizada do executável de produção terminou sem ameaças detectadas (código 0). Hash SHA-256 idêntico antes/depois; modo de relatório, sem alteração das configurações de proteção. Isso não resolve nem substitui o bloqueio separado da suíte completa de testes. |
| Falha intermitente nativa | O diagnóstico capturou `atomic_write` de `.managed.json` com erro Windows 5. Um teste nativo isolado reproduziu a falha de `MoveFileExW` com leitor aberto e confirmou que `std::fs::rename` substitui atomicamente mantendo o conteúdo do leitor anterior. A falha não se repetiu nos doze ciclos completos após a correção. |
| Bloqueio da suíte no Windows | O Defender detectou `HackTool:Win32/Mimikatz.NPTT` no executável completo de testes (evento 1116). O bloqueio reapareceu após remover o probe temporário; sua causa não foi presumida como falso positivo. Nenhuma proteção foi desativada e nenhum arquivo foi liberado/excluído da proteção. O aplicativo de produção executou os ciclos registrados acima. |

## Medições desta rodada

Executadas sem outras compilações ou testes de navegador deste trabalho. São
medições da base local 0.5 com as alterações, não da versão publicada 0.12.

- Controle temporal, dez pares alternando AB/BA no mesmo binário: razão mediana
  atual/referência de 1,0052 em 100 mil eventos e 0,9934 em um milhão. Duas
  passagens em ambas as implementações; igualdade completa do resultado. A
  regressão de aproximadamente 7,5% observada entre binários separados não se
  reproduziu nesse controle.
- Cache com um milhão de posições: elimina uma cópia de 8.000.000 bytes por
  consulta que apenas lê a seleção. A primeira página de 25 registros passou
  de mediana 0,897 ms para 0,197 ms, em 31 amostras. A referência reproduz a cópia
  anterior no mesmo binário; esses tempos não incluem IPC ou renderização.
- Histograma indexado de um milhão de registros: mediana de 136,24 ms para
  52,05 ms; pico RSS do processo de 235.328 KiB para 201.088 KiB. Sete amostras
  por rota, em processos separados; igualdade dos resultados verificada. O
  temporário por registro datado passou de 40 para 8 bytes.

Os benchmarks do cache comparam JSON completo das duas rotas dentro de cada
execução. Os hashes entre execuções podem diferir porque as referências dos
eventos incluem a identidade dos arquivos temporários exclusivos.

## Correções implementadas

- Drenagem do autosave no fechamento, coalescência limitada e validação de revisão.
- Integridade do manifesto de índice e limpeza de snapshots interrompidos.
- Histórico de recursos consistente após recuo do relógio do sistema.
- Reuso da seleção em cache sem copiar todos os identificadores em cada página.
- Histograma com retenção temporária reduzida e cancelamento preservado.
- Cache do Cubo restaurado sem acessar `.length` em um elemento nulo.
- Árvore de campos redesenhada ao abrir Explorar com resultados em cache, mesmo
  quando os metadados terminaram de carregar com a tela de Resumo aberta.
- Limpeza automática de arquivos pendentes do cache do parser após falha.
- Cancelamento revalidado depois de adquirir o lock de publicação de bundles.
- Revelação temporária no detalhe e inspetor local de JSON, JWT e Base64, com
  subcampos, limites de processamento, cópia explícita e limpeza ao ocultar/fechar.
  Lexemas numéricos que sofreriam arredondamento são preservados como texto;
  runtimes sem suporte necessário recusam essa interpretação e mantêm o original.
- Suíte de navegador reproduzível e obrigatória no fluxo de publicação.
- Aceite nativo Windows obrigatório no fluxo de publicação, tag de gatilho
  conferida antes de rede/build e encerramento supervisionado dos processos de teste.
- Contexto da primeira falha de escrita do índice preservado quando o motor
  encerra um worker; gravação, buffers e descarte mantêm o comportamento original.
- Substituição atômica dos metadados Tantivy no Windows usa o caminho de
  `std::fs::rename`, com arquivo normal temporário, `sync_data` e limpeza RAII.
  Não remove o destino antes de substituí-lo, não altera permissões e continua
  rejeitando leitores que proíbem compartilhamento para exclusão.

O mecanismo de substituição é descrito na [documentação do Rust](https://doc.rust-lang.org/std/fs/fn.rename.html)
e na [correção incorporada à biblioteca padrão](https://github.com/rust-lang/rust/pull/131072).
O processo que mantinha o handle aberto no erro intermitente não foi identificado;
os resultados não atribuem esse bloqueio ao antivírus.

## Pendências reais para a entrega

- Esclarecer o bloqueio do executável da suíte completa pelo Defender e obter
  uma execução completa aprovada no Windows, mantendo as proteções ativas.
  O aplicativo final desta base passou no aceite nativo e na varredura específica.
- Confirmar a base da publicação. A 0.12 possui DuckDB, Casos nativos, cotas e
  garantias de cache que precisam ser preservados se esta for uma atualização.
  As melhorias locais não devem sobrescrever esses módulos inteiros.
- Gerar e validar os instaladores da base escolhida, com versão inédita e
  notas correspondentes. Nenhuma publicação, tag ou push foi realizado.

Os resultados locais detalhados ficam em `output/`; eles não são incluídos no
pacote distribuído. Os testes e procedimentos reproduzíveis devem permanecer
versionados, e esta tabela será atualizada ao concluir a integração.

## Evidências finais locais

- `output/readiness-final-check.log`: verificações JavaScript, processos e release.
- `output/readiness-final-linux.log`: suíte final do backend Linux.
- `output/readiness-windows-atomic-focused.log`: três regressões nativas de I/O.
- `output/readiness-final-preview-summary.json`: rodada completa de 32 cenários,
  incluindo a falha de Descoberta que motivou a correção.
- `output/readiness-discovery-correction-summary.json`: três cenários aprovados
  após corrigir a árvore; o diagnóstico anterior foi preservado.
- `output/readiness-final-native-build.log`: build de produção da base local.
- `output/readiness-final-native-acceptance.log` e
  `output/native-desktop-eUE6Au/results.json`: aceite do executável final com IPC real.
- `output/readiness-production-defender-scan.{log,json}`: resultado e identidade
  do arquivo examinado, SHA-256
  `2E718A47F689EA1D2084DEC8B6A6C823AFDCED9C72EFE5A127215DD9D80E3567`.
