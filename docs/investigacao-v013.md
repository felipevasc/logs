# Investigação na 0.13

Abra **Comprometimentos**, escolha o conjunto ou Caso e use **Calcular comprometimentos**. A investigação aparece após o cálculo completo. Navegar, paginar ou abrir uma evidência consulta o resultado guardado; essas ações não iniciam outra análise. Os filtros de evidência continuam começando em E5.

## Entidades e próximos passos

A fila ordena entidades pela maior prioridade de sinal, acrescentando três pontos por grupo independente adicional, com teto de 85. Mesma família e membros compartilhados conectam os sinais em um grupo. O detalhamento mostra a fórmula e seus componentes. Essa ordem não é probabilidade de ataque, nível de evidência ou comprovação de efeito.

Selecione uma entidade para ver sinais, perfil, cronologia, relações, caças e cobertura. Cada hipótese traz observações, alternativas benignas e dados que faltam. Use **Ver eventos de apoio** e **Abrir evento original** para conferir a procedência. Prévias de achados têm até 128 membros e 256 entidades; `finding_members` e `finding_entities` fornecem a associação completa.

O grafo distingue vínculos explícitos, como IDs de sessão/credencial, de relações contextuais e hipotéticas. Uma conexão sem ID de processo pode ser ligada hipoteticamente a um único início de processo observado para o mesmo usuário e host nos 120 segundos anteriores. O vínculo declara essa condição e não atribui execução, autoria ou causalidade.

As próximas consultas têm uma rubrica ordinal: dez pontos por alternativa concorrente, até seis, mais trinta para conferir suporte original ainda incompleto ou dez para investigar a cobertura do cenário. Não há promessa de ganho probabilístico de informação.

## Configuração e significado das hipóteses

Em **Regras → Análise comportamental, IOC e políticas do ambiente**, configure o período de referência dentro da população atual e as políticas específicas do Caso. `baseline_days` aceita 1–365 dias; `minimum_history`, 5–100.000 observações; `rare_max`, 1–100. São limites explícitos para hipóteses, não calibração de precisão.

Relações novas e raridade usam os fatos completos, separados por namespace e entidade. Mudanças de volume usam intervalos comparáveis; latência exige unidade. Templates são uma transformação determinística de mensagem, limitada e identificada, não um modelo treinado de linguagem. Sequências web, cloud, diretório, pipeline e aplicação exigem os IDs correspondentes; proximidade textual não substitui esses vínculos.

### Referência histórica

Selecione um nome de `profile`, calcule o conjunto revisado e use **Usar este universo como referência**. O treinamento produz uma revisão congelada e seleciona essa revisão na configuração do Caso. Faça novo cálculo para aplicá-la. Não existe aprendizado automático de anomalias observadas.

Eventos sem horário confiável, membros E3 ou superior e eventos que sustentam hipóteses com prioridade pelo menos 35 são retidos fora do treinamento. O manifesto informa total elegível, excluído, fontes e período. A seleção de revisão é explícita; falta de revisão ou histórico resulta em cold start. Sazonalidade usa UTC e exige ao menos três semanas; comparação entre pares exige cinco entidades de um grupo declarado, sem inferir que têm a mesma função.

O `.licase` leva os bytes do perfil selecionado, SHA-256, identidade e revisão. A importação verifica formato, integridade e proprietário antes de vincular o perfil ao novo Caso. Mudança de normalização, mapeamentos ou interpretação impede aplicar uma referência incompatível.

### Negócio e telemetria

Uma política de negócio declara fonte, namespace, ação, moeda, janela e limites. Exemplo de um elemento de `business`:

```json
{
  "id": "pagamentos",
  "namespace": "tenant-a",
  "source": "payments",
  "action": "purchase",
  "currency": "BRL",
  "window_ms": 300000,
  "minimum_count": 3,
  "minimum_amount": 1000,
  "account_change_action": "password_change",
  "account_change_source": "accounts",
  "authentication_source": "identity"
}
```

Velocidade exige os limiares declarados. A hipótese de alteração de conta seguida de acesso e transação exige cronologia, conta, sessão, moeda e fontes compatíveis; sua existência não significa transação fraudulenta ou violação dos limites de velocidade. Use mapeamentos explícitos para quantidades e campos específicos da aplicação.

Um elemento de `telemetry` declara `source`, `namespace`, `maximum_gap_ms` e `sequence`. Um intervalo ou salto observado pede revisão de coleta, rollover, retenção e seleção. Presença de campos em **Cobertura** não prova coleta contínua nem completude das fontes.

### Catálogo IOC offline

`ioc` aceita um documento com `schema_version: 1`, `id`, `revision`, `source` e até 20.000 indicadores. Um indicador tem `id`, `kind`, `value`, `valid_from`, `expires` e `context`. Tipos disponíveis: `ip`, `domain`, `url`, `sha256`, `sha1` e `md5`. Horários são milissegundos UTC; limites ausentes podem ser `null`.

Somente campos tipados explícitos ou mapeados são comparados. Bloom elimina candidatos negativos; a tabela exata confirma cada correspondência. Horário desconhecido é declarado e impede confirmar a validade temporal. **IOCs confirmados** mostra valor, campo, catálogo, revisão, contexto e original. Um IOC não demonstra comprometimento nem determina a direção de um endpoint.

## Síntese e propostas

**Preparar síntese assistida** fornece observações e referências para seu cliente MCP. O Log Insight não chama um modelo automaticamente. Um rascunho pode conter `claims` com `kind`, `text`, `event_refs` e `signal_ids`. Tipos: `observation`, `hypothesis`, `alternative` e `missing_telemetry`. A revisão verifica referências na análise, namespace e entidade solicitados. Ela não verifica que o significado de uma frase é verdadeiro e não altera classificações.

**Propor regra** oferece seletores apenas para hipóteses que têm representação suportada; as demais pedem autoria manual. Antes de ativar, declare referências positivas, negativas e separação por origem/tempo, avalie o seletor na população e escreva o motivo da revisão. Controles falhos, referência estrangeira, resultado desatualizado ou hash de regra diferente impedem a ativação. Uma regra aceita continua com maturidade não avaliada. A avaliação é isolada da publicação da análise principal.

## Persistência, orçamento e limites

Fatos usam normalização `normalization-7`. Cache de interpretação e predicados tem assinatura do conteúdo e da interpretação. Apenas nós com identidade semântica estável entram no cache; matchers opacos usam confirmação nativa. Grupos de correlação têm checkpoints transacionais e remapeamento de IDs para as referências atuais. Eventos atrasados, removidos ou reordenados invalidam os grupos afetados. Estatísticas da população são reconciliadas por completo.

O resultado `results-3` guarda membros e entidades completos, fatos escalares consultáveis, dicionários de conteúdo, procedência comprimida sem perda e tempo original por evento. A transferência DuckDB→resultado lê lotes Arrow. Cache descartável pode ser evicto e recalculado; a evidência preservada do Caso não é removida por essa manutenção.

Uma faixa serial de trabalho controla análises; DuckDB usa um thread. A supervisão mede um orçamento adicional de 256 MiB sobre a memória inicial do processo. Dados originais já carregados fazem parte dessa base. Disco acompanha bancos, perfis, spools e resultados registrados, com limite de 16 GiB por padrão. `LOGINSIGHT_SECURITY_DISK_MB` ajusta esse limite entre 512 MiB e 256 GiB. O dataset e índices de entrada têm gerenciamento próprio. A reserva mínima de espaço disponível é 64 MiB. Arquivos temporários auxiliares internos do SQLite dependem também dos limites e espaço do sistema operacional.

Páginas aceitam 1–100 itens e até 4 MiB. Cancelamento e violações de orçamento falham explicitamente e não publicam análise parcial como completa. Scripts de escala seletiva/densa e corpus permanecem opt-in. Não há resultado de 1M, 10M ou 50M atribuído à versão final; não extrapole uma medição pequena.

Sigma declara especificação 2.1.0 e backend `loginsight-sigma-2`, incluindo matriz de modificadores e correlações suportadas. Regras externas sem avaliação permanecem não avaliadas; construções não suportadas são recusadas com diagnóstico. A referência pySigma 1.5.1 está fixada na revisão `f81e4f5ace2f444f76c5de03df8e0f181f85f6cc`; o corpus de comparação cobre interpretação de condições e modificadores, não equivalência universal de backends ou execução temporal externa.

A matriz declara uma diferença conservadora: `neq` exige presença do campo selecionado, e `fieldref|neq` exige ambos os campos. Um campo ausente não é tratado como valor observado diferente. A referência conserva separadamente a resposta da árvore `NOT` do pySigma e o resultado dessa política do backend, sem apresentar essa diferença como paridade universal. [Especificação de modificadores](https://sigmahq.io/sigma-specification/specification/sigma-appendix-modifiers.html).
