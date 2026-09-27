# Detecções avançadas — política e motor v2

Atualização de interface e conteúdo: [Comprometimentos](comprometimentos.md). Os nomes exibidos agora são Quase confirmado, Forte indício, Indício, Suspeita e Inconclusivo; os números do contrato permanecem compatíveis. A normalização atual é `normalization-3`.

## Força da evidência e resultado

E5 significa evidência extremamente forte da hipótese escrita na regra. Pode ser uma tentativa inequívoca: um payload completo de shell reverso registrado em um parâmetro de comando de uma requisição, mesmo que o WAF bloqueie a operação. O resultado continua **bloqueado**, e a hipótese continua **tentativa**, sem afirmar execução ou comprometimento.

`claim` explicita o que foi observado: `attempt`, `execution`, `effect` ou `activity`. Regras que afirmam execução/efeito continuam limitadas quando o resultado é falha, bloqueio ou desconhecido. Maturidade (`experimental` etc.) e dados de avaliação estatística são campos separados. E5 não significa probabilidade individual de 99%. Casos salvos com a política anterior conservam seus níveis e versões.

As regras E5 de shell reverso reconhecem gramáticas completas de shell POSIX, netcat/ncat e socat, com conexão e redirecionamento. Combinações de palavras em scripts Python/PowerShell, sem vínculo sintático demonstrado, não recebem E5 por esta regra. Não basta `bash`, `/dev/tcp`, IP externo, uso de PowerShell ou codificação. Campos de resposta, documentação marcada e comandos que apenas imprimem o texto não satisfazem a regra de tentativa na requisição. A extração cobre comando estruturado, JSON no corpo e parâmetros `cmd`, `command`, `exec`, `execute`, `shell` na URL; transformações são limitadas e nenhum conteúdo é executado.

## Cadeias novas e vínculos

| Cenário | Identidade necessária | Comportamento exigido |
|---|---|---|
| OAuth → encaminhamento oculto | tenant, aplicação e concessão | Consentimento com leitura/escrita de e-mail, depois regra que encaminha para fora e oculta mensagens |
| OAuth → transferência sensível | tenant, aplicação, token e objeto | Leitura de objeto marcado sensível e transferência externa do mesmo objeto |
| Workflow → segredo → envio | tenant, repositório, revisão e segredo; mesma execução nas duas últimas etapas | Alteração com acesso a segredo e envio HTTP, leitura do segredo e transferência externa registrada |
| Token de publicação | tenant, repositório e token | Proteção desativada e pacote publicado com divergência de atestação explicitamente registrada |
| RBCD → uso | domínio, SID beneficiado/delegador e recurso SPN | Delegação adicionada e ticket delegado utilizado pelo principal beneficiado |
| Certificado → autenticação | tenant/domínio e fingerprint ou emissor+serial | Certificado emitido com solicitante/identidade divergentes e autenticação com o mesmo certificado; exclui enrollment agent explicitamente autorizado |
| Shadow credential → uso | domínio, identidade alvo e ID da chave | Chave adicionada e autenticação por key trust com aquela chave |
| Movimento lateral Windows | host/namespace, logon e principal; mesmo hash nas etapas de arquivo e execução | Logon remoto, transferência por compartilhamento administrativo e execução de shell reverso |
| Movimento lateral SSH | host/namespace, sessão SSH/audit e principal; mesmo hash | Login, transferência e execução de shell reverso na mesma sessão |

Os campos devem existir na telemetria ou ser mapeados explicitamente. Por exemplo, um evento AD que registra somente a alteração binária de um descritor não identifica, sozinho, o SID delegado e o SPN; o motor não os inventa. Da mesma forma, auditoria de GitHub sem eventos do job não prova leitura/envio de um segredo. A aplicação informa cobertura insuficiente quando faltam vínculos. Isso evita transformar toda alteração administrativa em ataque.

Os aliases abrangem IDs de aplicação/concessão/token, repositório/revisão/execução, certificado, chave, logon, principal, origem e artefato. `bindings: [{"field":"_sec.artifact","steps":[1,2]}]` exige igualdade adicional nas etapas indicadas. O motor rejeita construções fora do suporte atual: um vínculo adicional por sequência, além dos campos arbitrários de `by`/`steps.by`.

Referências de campos e semântica: [Microsoft — evento 4768](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/auditing/event-4768), [Microsoft — evento 4769](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/auditing/event-4769), [GitHub — ações por token](https://docs.github.com/en/enterprise-cloud%40latest/admin/monitoring-activity-in-your-enterprise/reviewing-audit-logs-for-your-enterprise/identifying-audit-log-events-performed-by-an-access-token).

## Novos operadores

### Proporção

`where` define **todo o denominador**; `ratio.numerator` seleciona seu subconjunto. `count` define o mínimo de amostras, e `gte` varia de 0 a 1. Os campos de agrupamento precisam identificar o contexto real. Este exemplo é apenas a parte quantitativa de uma regra; uma regra publicada também deve declarar contexto de suspeita e política de evidência.

```json
{
  "kind": "ratio",
  "where": "_sec.action:logon",
  "by": ["_sec.identity", "_sec.host", "_sec.service"],
  "window": "10m",
  "count": 20,
  "ratio": {"numerator": "_sec.outcome:failure", "gte": 0.9}
}
```

### Agregação numérica

`aggregate` suporta `sum`, `avg`, `min` e `max`, com `gte` e/ou `lte`. Campos ausentes ou não numéricos não são zero: limitam a cobertura e não entram na medida. Use um campo com unidade única explícita, como bytes, e selecione operações relevantes antes de agregar. Volume isolado não caracteriza exfiltração.

```json
{
  "kind": "aggregate",
  "where": "_sec.action:object_transfer data.classification:sensitive destination.external:true",
  "by": ["_sec.identity", "_sec.session"],
  "window": "15m",
  "count": 2,
  "aggregate": {"field": "network.bytes", "operation": "sum", "gte": 104857600}
}
```

As medidas usam janelas móveis completas: emitir um achado não remove fatos do denominador. Uma nova passagem de falso para verdadeiro emite outra evidência; repetições contínuas dentro da partição não geram um achado por evento. Eventos com o mesmo horário são medidos juntos. Numerador, denominador, valor, operação e amostras ficam em `measurements`. Cada partição mantém a janela anterior como contexto, inclusive eventos benignos; IDs repetidos são deduplicados no armazenamento final.

### Ausência com cobertura comprovada

Uma regra `absence` tem exatamente duas etapas: âncora suspeita e evento esperado. A inexistência do segundo evento só pode produzir achado se `settings.coverage` declarar cobertura completa daquela fonte/namespace/categoria durante toda a janela. A declaração exige justificativa e o `dataset_fingerprint` retornado pela análise; não é reutilizada automaticamente para outra importação ou Caso.

```json
{
  "kind": "absence",
  "by": ["_sec.host", "_sec.process"],
  "window": "5m",
  "coverage": "process",
  "steps": [
    {"where": "event.action:suspicious_operation_started"},
    {"where": "event.action:expected_operation_completed"}
  ]
}
```

Campos da declaração: `dataset_fingerprint`, `source`, `namespace`, `category`, `start`, `end`, `complete`, `justification`. Os horários são epoch em milissegundos. A interface de configuração permite editar as declarações. Um evento esperado com cronologia ambígua ou identidade insuficiente para associá-lo impede concluir sua ausência. Sem cobertura, a regra fica `missing_coverage`.

Sigma também aceita `value_sum` e `value_avg`, além das correlações já existentes. Modificadores e operadores não implementados são rejeitados. [Especificação Sigma](https://sigmahq.io/sigma-specification/specification/sigma-correlation-rules-specification.html).

## Consulta de resultados grandes

O cálculo completo escreve achados e referências em SQLite, com agrupamento de episódios por fatos compartilhados. Contagens, filtros e rigidez são consultados no banco. O frontend não recebe todos os achados de uma vez. O cache mantém até quatro análises em disco; expiração exige uma nova análise e não retorna evidências parciais como completas.

`triage` retorna `page.next_offset`. Episódios com muitos componentes indicam `members_complete: false`, `detection_count` e `record_count`. Use `triage_episode(analysis_id, episode_id, offset, limit)` até `next_offset: null`. A interface oferece controles para isso. Abrir/salvar o episódio coleta os membros exatos de todas as páginas. Os originais e a classificação não mudam entre páginas. `triage_timeline` retorna contagens por nível em até 201 posições no intervalo solicitado, sobre todos os achados; a página aberta não limita os marcadores da timeline. No detalhe do evento, `related_findings_total` informa quando a lista é uma prévia limitada.

O supervisor de memória mede o crescimento do processo nativo em Windows/Linux, incluindo overhead de SQLite e do alocador, sobre o baseline dos dados já carregados. O orçamento de trabalho é 256 MiB. Páginas, caches e partições têm limites próprios; falhas cancelam a operação com mensagem explícita. Não há limite global de quantidade de achados nem truncamento silencioso da saída completa. Uma partição individual excepcionalmente densa ainda pode exceder seu limite de 32 MiB e encerrar a análise; isso é diferente de limitar o conjunto inteiro a 100 mil achados.

## Validação

Os testes verificam as cadeias completas, remoção de etapas, namespaces, artefatos, sessão de execução, bloqueio, janelas, proporções, valores ausentes, cobertura, paridade entre armazenamento e apresentação, além de navegação em um episódio com 205 componentes na interface. O benchmark `benchmark_security_result_store` passou com 100.001 achados em 111.763 ms, com crescimento observado de 4.411.392 bytes de memória nativa. Essa medição isola resultados já classificados; não mede a varredura dos logs. Separadamente, a análise completa de 100 mil logs JSONL passou em 169.947 ms, mais 11.493 ms de indexação, encontrou o indicador no último evento e consultou os cinco níveis em 70 ms. Crescimento observado de memória: 23.580.672 bytes. Medições feitas no perfil de desenvolvimento Windows, sem truncamento. Os ensaios de 1 e 10 milhões de eventos ainda não foram executados; não se afirma validação nesses volumes. Os 133 testes nativos e três testes da interface passaram.

Esta validação verifica os contratos e cenários implementados. Não demonstra precisão estatística em todos os ambientes. A medição em corpus representativo continua separada da classificação E1–E5.
