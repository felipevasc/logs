# Desenvolvimento da versão 0.10

Base preservada: `363b11e` (v0.9.0). Este trabalho está em desenvolvimento; não é uma publicação da versão 0.10 nem uma promessa de latência universal. Os dados de escala são sintéticos, não arquivos privados do usuário.

## Direção e critérios

1. **Preservar o trabalho já carregado.** Importação, união, cancelamento ou resposta fora de ordem só podem substituir a fonte após preparação completa. Um recibo atômico identifica a geração realmente publicada; falhas transitórias de listagem não representam exclusão dos arquivos
2. **Interação deliberada.** A busca é um rascunho até Enter/Adicionar; cada expressão aplicada vira um filtro editável. O menu de célula abre o editor completo com o valor original. Contagens/histogramas podem pausar e retomar sem bloquear a página visível
3. **Atualização recuperável.** Salvar antes de instalar, aguardar trabalho pertencente à aplicação, reter pacote verificado em uma falha e permitir nova tentativa. Bloqueios do sistema e processos externos precisam de diagnóstico; não existe garantia de sucesso em todo ambiente
4. **RAM proporcional ao trabalho ativo.** Ler metadados de linhas pelo journal compacto mapeado, com decodificação portável e views compartilhadas. Ingestão e reinterpretação usam lotes/spools limitados; não devem esconder um retorno à cópia integral em RAM
5. **Preparar trabalho repetitivo.** Índices opcionais ordenados de timestamps e resumos por segmento podem trocar preparação/disco por contagens e histogramas exatos sem varredura completa. Aceleração só se aplica a predicados cuja equivalência foi comprovada
6. **Escolher pelo custo medido.** Consultas seletivas, amplas, inexistentes, múltiplas fontes e cardinalidade alta precisam de planos diferentes. Um índice não melhora automaticamente toda ordenação ou agregação

## Primeiros resultados, com limites

Estatísticas SQL sobre os mesmos 52 segmentos/50 milhões de registros, DuckDB 1.5.5, quatro threads, 830 MB e 438 MiB de spill, cinco amostras alternadas após aquecimento:

| Recorte | Mediana anterior | Duas passagens | Observação |
| --- | ---: | ---: | --- |
| Fonte + nível | 258,8 ms | 192,5 ms | Resultado exato idêntico |
| Janela temporal seletiva | 25,62 ms | 18,73 ms | Resultado exato idêntico |
| Sem filtros | 756,2 ms | 763,8 ms | Não melhorou; exige outro caminho |

Coleta textual isolada sobre os mesmos 52 índices existentes, nove amostras após a primeira, ordem antiga/nova alternada:

| Consulta | Count + DocSet | Uma passagem limitada |
| --- | ---: | ---: |
| Palavra rara, 500 candidatos | 565,9 ms | 348,4 ms |
| Hexadecimal completo, 1 candidato | 746,5 ms | 520,4 ms |
| Palavra inexistente | 629,6 ms | 408,5 ms |
| Palavra ampla, orçamento excedido | 9,0 ms | 9,9 ms |

Esta segunda medição cobre candidatos, não SQL, IPC, interface ou busca completa. Os pares preservaram contagem, decisão de seletividade e digest dos IDs ordenados. A coleta elimina a segunda travessia e o conjunto hash; a construção de um scorer regex ainda pode expandir o dicionário antes do primeiro candidato. Não é um limite rígido de toda a consulta textual.

## Metadados e disco

O formato residente anterior usa cerca de 32 bytes por linha: 50 milhões requerem aproximadamente 1,49 GiB de heap só nesse vetor, além de motores, índices e páginas de arquivos. O journal existente usa 27 bytes por registro. A nova representação consulta esse formato diretamente por acessos portáveis, evitando um segundo arquivo permanente de 32 bytes e uma cópia integral obrigatória na reabertura.

Mapeamento não significa RSS constante: verificação de hashes, consultas e pressão de cache precisam ser medidos. Locks consultivos protegem escritores e limpeza da própria aplicação; alteração manual externa de arquivos mapeados continua sendo um risco. O spool temporário de ingestão pode acrescentar 27 bytes por registro enquanto o journal é confirmado; configuração de timestamps pode precisar de mais 8 bytes por registro durante a transformação. Falha de disco deve preservar a geração anterior.

## Fundamentação e hipóteses seguintes

- [Elastic: ordenação e terminação antecipada](https://www.elastic.co/docs/reference/elasticsearch/index-settings/sorting). Páginas, contagens e agregações exigem trabalhos distintos; agregar todos os registros continua exigindo cobertura completa
- [Tantivy 0.26.2: Query e scoring opcional](https://docs.rs/tantivy/0.26.2/tantivy/query/enum.EnableScoring.html) e [fonte de AutomatonWeight](https://docs.rs/tantivy/0.26.2/src/tantivy/query/automaton_weight.rs.html). Coletar com limite não elimina o custo prévio de expansão de regex
- [DuckDB: gerenciamento de memória](https://duckdb.org/2024/07/09/memory-management) e [limitações de índices](https://duckdb.org/docs/current/guides/performance/indexing). Agregações de alta cardinalidade e ordenações podem precisar de spill; ART não é uma solução geral para todas as consultas
- [Arrow: formato colunar](https://arrow.apache.org/docs/format/Intro.html). Layout e acesso sem cópias precisam ser escolhidos conforme acesso aleatório, varredura e mutação; trocar a linguagem por si só não remove trabalho
- [ClickHouse: índices para pular blocos](https://github.com/ClickHouse/clickhouse-docs/blob/main/docs/guides/best-practices/skipping-indexes.md). Min/max, conjuntos, bloom e postings têm seletividades diferentes; o custo do índice deve ser menor que as leituras evitadas

Próxima etapa arquitetural: um sidecar opcional com timestamps ordenados por classe de nível, identidade e checksum vinculados ao segmento imutável. O limite simples é 8 bytes por timestamp não nulo, cerca de 400 MB para 50 milhões. Busca binária em bordas permite bins exatos; histogramas grossos fixos não substituem os 60 bins dinâmicos atuais para qualquer recorte. Filtros não suportados, índices ausentes ou corruptos continuam pelo SQL exato.

## Validação obrigatória antes da versão

- Paridade do motor de linhas e colunar; ordem/ties, zero/nulo, datas negativas, múltiplas fontes, filtros salvos e cancelamento
- Falha e resposta fora de ordem de importação, recuperação de recibo sem reingestão e lista preservada após falha de IPC
- Fluxos reais de interação no Chromium de CI e atualização instalada Windows, inclusive arquivo ocupado, salvamento recusado e nova tentativa
- Medições isoladas de heap/RSS, página visível, preparação, reabertura, artefatos e spill, com binário/commit/manifesto registrados
- Repetir a escala com orçamento reduzido antes de declarar suporte melhor a máquinas modestas

## Cadência de desenvolvimento

Pesquisa, implementação estrutural e experiência de uso avançam continuamente em mudanças pequenas. Cada mudança recebe verificações focadas e rápidas; os cenários de integridade de dados/atualização continuam obrigatórios quando afetados. No PR em rascunho, o CI executa os testes rápidos e os fluxos de interação alterados. A suíte completa nativa/navegador roda ao marcar para revisão ou disparar a validação completa explicitamente. Benchmarks grandes e atualização instalada assinada ficam para a etapa final solicitada; publicar uma alteração em desenvolvimento não significa que esses gates já passaram. O workflow de release mantém suas verificações completas.
