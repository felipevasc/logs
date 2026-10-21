# Diagnósticos locais com evidências

A análise oferece **Diagnósticos e próximas verificações** em Comprometimentos.
Cada episódio apresenta até três explicações, os registros que as sustentam,
contrapontos, dados faltantes e consultas locais para distinguir as alternativas.
Tudo funciona em CPU e offline; não há LLM nem envio automático de logs.

## O que muda na prática

| Situação | Resultado |
|---|---|
| Timeout ao obter conexão, com pool no limite e fila | Sugere espera/saturação do pool, mostra as medidas e oferece comparar aquisição e execução da consulta. |
| Timeout sem ocupação ou fila medidas | Relata o sintoma e pede essas medidas; não inventa saturação comprovada. |
| Pool com capacidade livre no mesmo host e momento | Expõe o contraponto e reduz o suporte da hipótese; intermitência ainda pode existir. |
| Timeout ao conectar versus aguardar resposta | Separa indisponibilidade/conexão de demora da dependência. |
| SQLSTATE, DNS, TLS ou razão de autenticação conhecidos | Distingue limite de conexões, deadlock, falha de resolução, certificado expirado, confiança, nome e credenciais. |
| Mais falhas na versão nova | Só oferece regressão associada ao deploy com versões conhecidas concorrentes, hosts comuns, controles suficientes em ao menos dois minutos e deploy anterior conhecido. A associação ainda exige confirmação. |
| Erro frequente desde o começo do arquivo | Pode oferecer diagnóstico pelos fatos, mesmo sem referência de normalidade. |
| Sintoma sem causa distinguível | Declara a insuficiência e abre a consulta de sintomas e cobertura. |
| Horário ausente ou ambíguo | Conserva as evidências qualitativas; não cria cronologia, comparação temporal ou horário fictício. |

As cinco famílias iniciais são banco/conexões/locks, dependências, DNS/TLS,
deploy e autenticação. Formatos desconhecidos continuam contando com descoberta
de desvios; conhecer a sintaxe de um arquivo não garante conhecer sua semântica.

Os episódios separam namespace, serviço (ou fonte), operação, dependência e,
na autenticação, identidade. Uma pausa de mais de dois minutos ou seis horas de
duração separa episódios. Referências explícitas preservam o acesso ao original.
Medições contextuais exigem mesmo host conhecido, distância de até cinco segundos
e trace compatível quando ambos existem. Essa proximidade não demonstra causa.

A pontuação é ordinal, não uma probabilidade. Repetir uma mensagem não aumenta
indefinidamente sua força; sinais dependentes compartilham limite de contribuição.
O impacto distingue linhas de falha de requisições com trace conhecido. Nenhum
diagnóstico ou retorno de revisão promove o nível de evidência de segurança.

A representação corrigida usa identidades novas de normalização e aprendizado;
os arquivos dos perfis anteriores são preservados. Análises salvas continuam
acessíveis e recebem o novo diagnóstico somente após recálculo explícito.

## Campos próprios da aplicação

Os adaptadores reconhecem campos explícitos como `db.pool.active`, `db.pool.max`,
`db.pool.pending`, `db.pool.acquire_ms`, `db.query.duration_ms`, `db.query.limit_ms`,
`dependency.duration_ms` e `dependency.timeout_ms`. As medidas permanecem ligadas
a seus campos originais. Durações genéricas sem unidade ficam desconhecidas.

O mapeamento por fonte aceita os papéis `error_code`, `sqlstate`, `exception_type`,
`auth_reason`, `operation`, `service_version`, `dependency` e as medidas abaixo:

| Papéis numéricos | Unidades aceitas no mapeamento |
|---|---|
| `acquire_ms`, `query_ms`, `query_limit_ms`, `dependency_ms`, `dependency_limit_ms`, `lock_wait_ms` | `s`, `ms`, `us`, `ns`; convertidas para ms |
| `pool_active`, `pool_limit`, `pool_pending`, `retry_count`, `retry_limit` | `count` |
| `request_rate`, `request_rate_baseline` | `1/s` ou `rps` |

Exemplo de entrada em `settings.mappings` (o mesmo array de
mapeamentos da investigação):

```json
{
  "source": "minha-aplicacao.log",
  "fields": {"query_ms": "tempoConsulta", "sqlstate": "codigoBanco"},
  "units": {"query_ms": "s"}
}
```

## Aprender com a resolução

Em **Registrar resultado da investigação**, informe causa confirmada, causa
descartada, comportamento esperado ou inconclusivo, com justificativa. Não é
necessário rotular linhas. Só esse retorno explícito fornece supervisão de causa;
abrir uma sugestão ou não responder não confirma nada.

A memória fica neste computador, isolada por Caso, área e interpretação. Uma nova
análise pode recuperar até cinco precedentes compatíveis, com a justificativa
original. Similaridade não substitui as evidências atuais. Editar uma resolução
substitui seu rótulo, sem contar o mesmo episódio outra vez. “Comportamento
esperado” ou “inconclusivo” substitui os julgamentos causais anteriores do episódio;
uma contribuição já exportada recebe uma revisão vazia para retrair os rótulos na
próxima publicação explícita. Marcar comportamento esperado não incorpora
automaticamente mensagens ao perfil sem rótulos; essa
continua sendo uma ação separada.

Um classificador linear pequeno pode ajustar a ordem de hipóteses já sustentadas,
no máximo oito pontos. Ele não cria causas nem remove contrapontos. A promoção
local exige ao menos 32 episódios independentes, validação nos 25% mais recentes,
ao menos oito episódios com pares causa confirmada/descartada, ganho consistente
em ao menos seis episódios e um limite conservador de consistência por episódio,
além de nenhuma regressão por família. Pares de um mesmo episódio não contam como
validações independentes. Com pouco retorno, os critérios explícitos continuam
ativos. O treinamento usa no máximo 2.048 exemplos e a memória tem teto de 64 MiB.
Episódios sem horário confiável não treinam esse ranking cronológico.

## Conhecimento opcional pelo Git

O painel **Memória e conhecimento diagnóstico** permite exportar, importar,
restaurar, baixar e publicar. Revise o JSON exportado antes da publicação. Ele
contém apenas sinais enumerados, hipóteses explicitamente revisadas, pontuações e
recibos públicos aleatórios. Não contém logs, justificativas, nomes, horários ou
referências dos originais. Mesmo essas estruturas descrevem atividade: compartilhar
é uma escolha explícita, com token temporário e destino configurado.

As contribuições vão para `models/diagnostic-contributions/`; o workflow da branch
`learning-models` consolida `models/diagnostic-knowledge.json`. Revisões posteriores
do mesmo episódio substituem as anteriores. Limites por origem reduzem a influência
de ambientes volumosos; origens inteiras ficam reservadas para avaliar o ranking.
Esses recibos não autenticam pessoas ou organizações, portanto só incorpore
contribuições de procedência confiável no repositório que você administra.

Uma branch criada em versão anterior precisa receber o código e o workflow novos
antes de publicar diagnósticos. Para atualizar, integre a revisão principal à branch
de aprendizado e resolva os modelos gerados; não apague suas contribuições.
Branches com outro nome precisam de workflow correspondente.

A importação valida esquema, limites, compatibilidade e reproduz os contratos
sintéticos locais. Um relatório recebido não é aceito apenas pelo que declara.
A base anterior válida fica disponível para retorno, e a análise salva registra as
revisões utilizadas. Uma atualização só se aplica na próxima análise explícita.

## Reprodução e limites de avaliação

```sh
cargo test --manifest-path crates/log-learning/Cargo.toml --locked --release
cargo run --manifest-path crates/log-learning/Cargo.toml --locked --release -- diagnostic-evaluate
cargo run --manifest-path crates/log-learning/Cargo.toml --locked --release -- diagnostic-consolidate models/diagnostic-contributions models/diagnostic-knowledge.json
cargo run --manifest-path crates/log-learning/Cargo.toml --locked --release -- diagnostic-check models/diagnostic-knowledge.json
```

O corpus versionado tem 50 contratos de avaliação sintéticos e um de
desenvolvimento: 32 positivos e 18 benignos/desconhecidos. Ele verifica mecanismos,
abstenção e regressões de implementação. Não é um conjunto representativo de
incidentes reais nem um teste cego independente do desenvolvimento das regras.
Por isso, seus resultados não estabelecem precisão de campo ou probabilidades
calibradas. A base distribuída começa sem resoluções de usuários e com ranking
aprendido inativo. Medir ganho real requer incidentes locais resolvidos, separados
por ambiente, episódio e tempo, sem duplicatas entre treino e avaliação.

O processamento da população usa SQLite e disco; as estruturas em RAM, páginas,
referências e treinamento têm limites. A análise admite até 10.000 episódios e
informa os registros que excederem o limite. O benchmark opt-in
`diagnostics_million_rows_resource_profile` mede esta camada sobre fatos já
normalizados; não representa o consumo completo de importação e interface.
Resultados, reprodução e limitações estão no [relatório de validação](validation/diagnostics.md).
