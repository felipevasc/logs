# Detecção por força da evidência

O backend calcula a análise completa do conjunto carregado ou do Caso antes dos filtros de apresentação. Não há aprendizado entre importações, consulta de reputação, LLM ou coleta adicional. Os logs continuam sendo dados inertes.

## Níveis e rigidez

| Rigidez | Achados visíveis | Nome do nível mínimo |
|---|---|---|
| 5 — Máxima | E5 | Evidência muito forte |
| 4 — Alta | E4–E5 | Suspeita forte |
| 3 — Intermediária | E3–E5 | Suspeita relevante |
| 2 — Ampliada | E2–E5 | Suspeita contextual |
| 1 — Exploratória | E1–E5 | Pista exploratória |

Abrir o aplicativo, carregar outro conjunto ou abrir/reabrir um Caso começa em 5. Filtros, paginação e navegação no mesmo universo preservam a seleção. Reduzir a rigidez não recalcula as regras, não modifica níveis e não altera IDs ou episódios. O estado vazio não reduz o nível automaticamente.

Nível, impacto potencial, resultado da operação e avaliação do analista são independentes. E5 nunca confirma um comprometimento automaticamente. No Caso, a confirmação fica em `analyst_state`; os níveis calculados são preservados.

**E5 representa sustentação extremamente forte da atividade ou tentativa descrita, sem exigir destruição ou comprometimento consumado.** Uma requisição com payload completo de shell reverso pode ser E5 mesmo bloqueada. O contrato separa `claim` (`attempt`, `execution`, `effect`, `activity`), `outcome`, maturidade da regra e confirmação do analista. Não há mais redução automática a E4 por falta de avaliação estatística; esta continua explicitamente não demonstrada. Severidade, volume, codificação ou nome de ferramenta não concedem E5.

## Catálogo e relevância

Há 87 definições nativas: 52 no pacote principal, 24 na expansão e 11 no pacote avançado. Vinte e três regras genéricas anteriores foram retiradas da triagem por insuficiência de contexto; permanecem no pacote para rastreabilidade. As 64 definições ativas possuem política explícita. A expansão tem um positivo e um controle administrativo próximo por regra em `detection-validation.json`; `detection-advanced-validation.json` contém cenários completos para o novo pacote.

O catálogo de 378 padrões textuais continua disponível para busca e explicação. Apenas 69 padrões explicitamente revisados em `threat-evidence-policy.json` podem gerar pistas textuais independentes E1. Modificar a expressão de um desses IDs localmente não herda sua revisão. Respostas normais, erros genéricos, criação de chaves e uso comum de ferramentas não entram automaticamente como suspeita.

As cinco correlações de referência são:

| Regra | Vínculos obrigatórios | Janela | Limite atual |
|---|---|---|---|
| `chain.web.command` | payload completo de shell reverso recebido e executado; mesmo request/trace, host, serviço e comando exato | 5 min | E5 |
| `chain.download.execution` | arquivo baixado e executado com shell reverso; mesmo artefato, host e processo criador/pai | 15 min | E5 |
| `chain.cloud.credential` | chave criada, usada para desligar auditoria e acessar segredo; mesma chave e conta | 24 h | E4 |
| `chain.persistence.payload` | destino de persistência efetiva e shell reverso executado pelo mesmo artefato no host | 24 h | E5 |
| `chain.recovery.destruction` | enfraquecimento de recuperação e comando destrutivo na mesma árvore de processos | 15 min | E4; efeito destrutivo ainda não comprovado |

A expansão cobre comportamentos específicos em credenciais, AD, certificados, Windows, Unix/macOS, containers, Kubernetes, S3, e-mail, ocultação, enfraquecimento de defesas, coleta, C2, exfiltração e destruição. O pacote avançado acrescenta OAuth (consentimento/encaminhamento e token/leitura/transferência), CI/CD (workflow/segredo/envio e token/publicação), RBCD, shadow credentials, certificado/autenticação e movimento lateral Windows/SSH. Cada cadeia exige identidades observadas; campos ausentes não são inventados. Os vínculos, exemplos e configuração dos operadores estão em [Detecções avançadas v2](deteccoes-avancadas-v2.md).

## Normalização e procedência

`security_normalize` produz uma cópia derivada: `_sec.*` nunca vem de campos fornecidos pelo log. Nome e descrição enriquecidos pela aplicação não alimentam as regras. Originais são preservados.

São separados produto, produtor, ator, alvo, namespace, host, serviço, processo/pai, sessão, conexão, requisição, credencial, recurso, arquivo, comando, resultado e ação da proteção. Cada valor informa campo de origem e método. IDs de processo e sessão são contextualizados; PID sem início da instância não é identidade suficiente. Nomes iguais em tenants diferentes não se tornam a mesma entidade.

O adaptador conhecido tem precedência. Mapeamentos explícitos preenchem lacunas ou substituem aliases de fontes genéricas. Conflitos limitam a classificação. Resultado HTTP não é sucesso de autenticação. Bloqueio não prova execução. Campos ausentes geram lacunas de cobertura.

Configurações → Detecção → Mapeamento de fontes permite editar e pré-visualizar o mapeamento no evento aberto. Exemplo:

```json
[
  {
    "source": "aplicacao-customizada",
    "fields": {
      "timestamp": "when",
      "namespace": "tenant",
      "host": "node",
      "actor": "principal",
      "action": "operation",
      "outcome": "result",
      "request": "trace_id",
      "command": "executed_command"
    },
    "timestamp_unit": "ms",
    "timezone": "-03:00",
    "actions": {"spawn": "process_start"},
    "outcomes": {"ok": "success", "denied": "blocked"}
  }
]
```

Datas numéricas mapeadas exigem unidade `s`, `ms`, `us` ou `ns`. Datas textuais exigem RFC3339 com fuso ou deslocamento explicitamente configurado. Sem isso, não entram em sequências temporais. A prévia mostra original, epoch, fuso, resolução disponível e ambiguidades. O parser de importação continua responsável pelas configurações temporais já existentes.

## Reconstrução, conteúdo e identidade dos fatos

- PowerShell: `ScriptBlockId`, host e sequência de fragmentos; lacunas e conflitos limitam a evidência.
- auditd: serial, host e instante da operação; junção de SYSCALL/EXECVE e recomposição dos argumentos disponíveis.
- Kubernetes: `auditID` no cluster; somente `ResponseComplete` fornece resultado terminal.
- CloudTrail: identidade de evento, conta e destinatário; perspectivas distintas não são descartadas como cópias.
- Requisição/efeito: vínculo explícito por request/trace e conteúdo correspondente. Não há junção genérica de todos os eventos de um host.

Estágios reconstruídos contam como um fato e conservam todas as referências originais. Eventos semelhantes sem identidade de produtor não são eliminados. A mesma ocorrência não pode preencher duas etapas independentes.

O corpus textual é segmentado por campo e caminho de array, com direção, posição e transformação. Correspondências não atravessam campos. A decodificação limitada admite percent-encoding, escapes Unicode, entidades HTML comuns e Base64/UTF-16 em comandos. Nunca executa o conteúdo. Documentação explicitamente marcada, campos de descrição e comandos que apenas imprimem conteúdo são excluídos da triagem. Não se trata de um interpretador completo de shell.

## Contrato dos achados

`evidence_level`, `claim`, `evidence_reasons`, `conditions_satisfied`, `missing_evidence`, `benign_alternatives`, `outcome`, `evaluation`, `maturity`, `validation`, `measurements`, `event_refs`, `evidence_members`, `relationships`, `rule_version`, `normalization_version` e `policy_version` acompanham o achado. Membros incluem campo, procedência e informação temporal. A análise inclui `analysis_id`, `dataset_fingerprint`, `attack_version`, `counts_by_level`, cobertura por regra, duplicatas e limitações.

`minimum_evidence` aceita 1–5 e tem padrão 5 nas consultas públicas. `visible_detections` e `available_detections` são contagens de achados. `context_only` identifica membros de episódios preservados como contexto. Referências são independentes de filtros e de alterações futuras da regra.

**Ver registros** usa `event_ref` com `in_exact`. Não refaz uma busca por eventos parecidos e não limita o salvamento aos primeiros 40 membros. O Caso preserva análise, níveis, relações, versões, limitações e todos os eventos acessíveis. Falha na obtenção de evidência interrompe o salvamento.

Detalhes de evento e MCP expõem achados do universo completo que usam aquele registro. Timeline do Caso, PDF e Markdown usam os níveis salvos, sem promover componentes. Interface e relatórios mascaram segredos textuais por padrão; os eventos armazenados permanecem originais.

## Regras locais e Sigma

O pacote distribuído é separado de `detection-rules.json` local, que substitui por ID ou acrescenta regras. Política ausente permanece **nível não avaliado** (`0` no contrato), sem conversão de severidade. Casos antigos não são reclassificados.

Política mínima de uma regra nova:

```json
{
  "level": 2,
  "rationale": "Característica concreta que torna este comportamento suspeito",
  "required": ["_sec.host", "_sec.command"],
  "products": ["ecs", "sysmon"],
  "missing": ["Efeito ou artefato relacionado ainda não observado"],
  "benign": ["Procedimento administrativo que apresenta os mesmos fatos"],
  "maturity": "experimental",
  "version": "1"
}
```

Regras nativas aceitam `single`, `threshold`, `distinct`, `sequence`, `temporal`, `beacon`, `ratio`, `aggregate` e `absence`. `aggregate` suporta soma, média, mínimo e máximo de valores numéricos finitos. `ratio` utiliza todos os eventos do denominador selecionado. `absence` exige cobertura explícita completa da fonte e janela, vinculada ao fingerprint do conjunto atual. `steps.by` vincula campos entre etapas; `bindings` acrescenta igualdade entre etapas específicas; `steps.rules` referencia seletores simples. Ciclos, agregados referenciados e condições contextuais não suportadas são rejeitados. `any: [{field, where}]` preserva a associação entre campos de cada objeto de array; `unless` representa uma alternativa benigna explícita.

Sigma admite `event_count`, `value_count`, `temporal`, `temporal_ordered`, `value_sum` e `value_avg`, com `group-by`, `timespan` e aliases. Contagens aceitam `gte`/`gt`; somas e médias aceitam `gte`/`lte`. `logsource`, status de origem, autoria, referências e falsos positivos são preservados. Dependências são resolvidas entre arquivos do lote antes da publicação. Construções desconhecidas são recusadas, incluindo correlação de agregados, aliases incompatíveis em contagens e condições temporais opcionais. Os limites atuais são 16 etapas e 512 fatos obrigatórios. Sem `x-loginsight-evidence`, a regra fica não avaliada.

Exceções novas exigem justificativa, namespace e validade. Configurações anteriores recebem backup `detections.json.bak` ao salvar; excluir a exceção restaura a regra. Atualizações de padrões já usam comparação por ID e backup do catálogo; os ajustes locais não são sobrescritos.

## ATT&CK

`attack-enterprise-v19.2.json` contém objetos STIX oficiais de táticas, técnicas, identidades, marcações e relações de revogação. `attack-manifest.json` registra origem e hashes do original e do subconjunto distribuído. Stealth/Ocultação e Defense Impairment/Enfraquecimento de defesas têm táticas separadas. Técnicas revogadas preservam a referência original e resolvem o sucessor quando disponível. Itens históricos dos Casos conservam o mapeamento salvo.

Fonte: [STIX oficial Enterprise 19.2](https://github.com/mitre-attack/attack-stix-data/tree/master/enterprise-attack), [especificação de correlação Sigma](https://sigmahq.io/sigma-specification/specification/sigma-correlation-rules-specification.html).

## Limites e escala

A análise não possui mais os cortes globais de 500/100 mil achados nem de 128 MiB de saída completa. Intermediários, reconstruções e resultados finais usam arquivos/SQLite temporários. Grupos temporais são particionados por identidade e janela, com sobreposição que preserva vínculos na fronteira; sinais individuais são separados por fato. Há limites defensivos por unidade: 24 MiB antes de transferir grupos para disco, 4 MiB para identidades de deduplicação e 32 MiB por partição de correlação reconstruída. Estatísticas auxiliares têm orçamento de 32 MiB e sinalização de limitação.

A interface consulta até 20 episódios por página; o MCP usa `episode_offset` e `episode_limit` (padrão 100, máximo 500). Episódios grandes apresentam prévia de até 100 componentes e `members_complete=false`; `triage_episode` percorre seus membros completos com `next_offset`. Contagens, níveis, IDs e relações são calculados sobre todos os resultados em disco. A interface permite percorrer componentes e coleta todas as páginas antes de salvar/abrir as evidências. Trocar a rigidez consulta o resultado pronto, sem varrer os logs. `triage_timeline` agrega o resultado completo por intervalo e nível; os marcadores não dependem da página de episódios aberta.

O processamento nativo é serializado por um coordenador comum. Windows e Linux supervisionam o crescimento de memória do processo em relação ao conjunto já carregado, com orçamento de trabalho de 256 MiB, incluindo SQLite/alocador; `memory` registra baseline e pico observado. Não é um limite rígido do sistema operacional nem um limite sobre o WebView ou sobre os logs já carregados. Páginas de transporte têm orçamento de texto de 8 MiB e limite individual de 16 MiB; unidades excepcionalmente maiores interrompem a operação com erro explícito. Cache de quatro análises mantém os bancos em disco e remove arquivos ao liberar a análise. Cancelamentos, falhas e limites nunca são publicados como análise limpa.

Benchmark da política v2 com armazenamento em disco, executado em Windows, perfil de desenvolvimento: 100 mil eventos JSONL, 100 hosts e indicador no último registro. O indicador final foi encontrado:

| Medida | Resultado observado |
|---|---:|
| Indexação | 11.493 ms |
| Análise e armazenamento | 169.947 ms |
| Consulta dos cinco níveis cumulativos | 70 ms |
| Achados | 2; sem truncamento |
| Crescimento observado da memória nativa | 23.580.672 bytes |

É uma medição de desenvolvimento, não um SLA de produção. A execução do binário otimizado foi bloqueada pelo Windows com erro 225. Os benchmarks de 1 e 10 milhões estão preparados, mas não foram executados nesta entrega; também falta perfil de pico de memória em produção. Não foi alterada proteção do sistema.

Teste adicional do armazenamento completo: **100.001 achados já classificados**, cada um com episódio e referências, em **111.763 ms**. Primeira e última páginas verificadas, inclusive o achado final. Crescimento observado da memória nativa: **4.411.392 bytes** sobre o baseline de 16.338.944 bytes. Esta medição isola gravação, agrupamento e consulta de resultados; não representa tempo de varredura de logs.

## Validação e reprodução

```powershell
cd src-tauri
cargo test --lib
$env:RESULT_FINDINGS='100001'
cargo test --lib benchmark_security_result_store -- --ignored --nocapture
$env:BENCH_EVENTS='100000,1000000,10000000'
cargo test --release --lib benchmark_security_evidence -- --ignored --nocapture
```

```powershell
node scripts/preview/serve.mjs 4173
node scripts/preview/test-security-triage.mjs
node scripts/preview/test-evidence-levels.mjs
node scripts/preview/test-security-pagination.mjs
node scripts/preview/validate-threat-catalog.mjs
node scripts/security/evaluate.mjs --self-test
```

Testes incluem os cinco cenários com remoção de etapas, bloqueio, ordem, janelas, tenant, processo, duplicação, mais de 500 achados, membros exatos, paridade memória/índice, objetos de arrays, reconstrução, decodificação, SQLite e fontes genéricas. Os testes da interface usam respostas sintéticas; não são uma avaliação de precisão do motor.

Verificação desta entrega: 133 testes nativos aprovados, seis testes de execução opcional ignorados na suíte padrão; os três testes de interface aprovados; 378 padrões compilados, 151 exemplos positivos e 22 controles benignos aprovados no validador textual. A interface também verifica que tabela, entidades e timeline ocultam segredos reconhecíveis sem modificar os eventos originais. Isso não representa cobertura completa de todos os formatos de segredo nem validação estatística das regras.

O avaliador offline recebe `scenarios` com `id`, `origin`, `family`, `split`, `malicious` e `telemetry_sufficient`, e `findings` com `id`, `scenario`, `evidence_level`, `verdict` (`malicious`, `benign`, `unresolved`) e `review_minutes` opcional. Produz precisão, intervalo Wilson de 95%, volume de revisão, recall condicionado à telemetria e resultados por família, nível e seleção cumulativa. Rejeita mistura da mesma origem entre desenvolvimento e avaliação. `entity_periods`, quando conhecido, permite normalizar falsos positivos. Não promove regras automaticamente.

Continua pendente a validação representativa com incidentes e atividades benignas rotulados, separada por origem e cenário. [OTRF Security-Datasets](https://github.com/OTRF/Security-Datasets) é uma fonte possível de capturas, não evidência de precisão por si só. As metas E5 ≥99% e E4 ≥95% ainda não foram demonstradas.
