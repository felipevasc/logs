# Leitura de formatos de log

Todo arquivo de texto é lido por camadas. O formato principal é detectado nas
primeiras linhas; cada linha que o leitor desse formato não reconhece passa
pelos demais leitores e, por fim, pelo leitor genérico, que estrutura qualquer
texto. O registro original (`raw`) é sempre preservado como foi gravado.

## Data e hora

Reconhecida em qualquer posição da linha (vence a primeira), nos campos JSON,
logfmt e colunas:

- ISO 8601/RFC 3339 e variantes: `2026-10-12T00:00:00.123Z`, `2026/10/12 00:00:00,123 -03:00`,
  `2026.10.12 00:00:00`, `20261012T000000Z`, `2026-10-12 00:00:00:123`, `+0000 UTC` (Go),
  `-03:00[America/Sao_Paulo]` (Java).
- Common Log Format e meses por nome: `12/Oct/2026:00:00:00 -0300`, `12-Oct-2026 00:00:00.123`,
  `12 Oct 2026 00:00:00,123`, `Mon, 12 Oct 2026 00:00:00 GMT`, `Oct 12 00:00:00` (syslog),
  `Oct 12 2026 00:00:00` (Cisco), `Oct 12, 2026 12:00:00 AM` (Java), `Mon Oct 12 00:00:00.123456 2026`.
- Datas numéricas: `12/10/2026`, `10/12/2026 12:00:00 AM`, `12.10.2026`, `10/12/26,00:00:00` (DHCP),
  `10/12/2026-00:00:00.123456` e `10/12-00:00:00` (Snort), `10-12 00:00:00.123` (Android).
- Epoch em segundos ou milissegundos no início da linha; `/Date(1700000000000)/` (.NET).
- Meses e dias da semana em português, inglês, espanhol, francês, alemão e italiano.
- Fusos: `Z`, deslocamentos (`-03`, `-0300`, `-03:00`), siglas sem ambiguidade (UTC, BRT, EST,
  CEST…) e nomes IANA comuns, com horário de verão europeu e americano. Sem fuso, vale o horário
  local da máquina — exceto em logs W3C do IIS e do CloudFront (campos `cs-`, `sc-`, `s-`), gravados
  em UTC por definição; o log do Firewall do Windows continua em horário local.

Datas sem ano (syslog, klog, Android) ficam no ano atual. A data no nome do arquivo e a da última
modificação não mudam o ano: um nome pode trazer qualquer dia do log e cópias alteram a
modificação. Para um log de outro ano, defina-o na configuração de data/hora da fonte (Arquivos →
Formato e data/hora). Linhas só com hora (WildFly `server.log.2026-06-24`) recebem a data de
referência do arquivo — a data no nome (`access_log.2026-10-12`, `messages-20261012`,
`u_ex261012.log`) ou, sem ela, o dia da última modificação — e o campo `data_inferida`. Datas numéricas ambíguas
(`01/02/2026`) seguem a ordem que o próprio arquivo mostra (`25/02` ou `02/25` em outra linha); sem
evidência, dia primeiro, exceto com AM/PM. A configuração de data/hora da fonte continua tendo
prioridade.

## Formatos com leitor dedicado

| Família | Exemplos |
|---|---|
| Access log | Apache/Nginx common e combined, prefixos de virtual host e X-Forwarded-For, requisição **com ou sem aspas** (Tomcat/JBoss `%r`), tamanho opcional, colunas extras nomeadas (`duration_ms`/`duration_s` de `%D %T`, `thread`, `duration`, user agent sem aspas) ou em ordem (`access.extra.N`) |
| Erros de servidor web | Nginx `error.log`, Apache `error_log` 2.2/2.4 |
| Syslog | RFC 3164 com qualquer data (clássica, com ano, ISO do rsyslog/journald), sequência e `*` da Cisco, mnemônicos `%ASA-6-302013`, RFC 5424 com *structured data* (`sd.<id>.<param>`), prioridade → nível |
| Segurança | CEF e LEEF (também após cabeçalho syslog), iptables/UFW, auditd, Zeek, Suricata/CloudTrail/Okta/Wazuh em JSON |
| Aplicações | Log4j/Logback, JBoss/WildFly, Spring Boot, Python `logging`, Serilog, NLog, Rails, Elasticsearch, PostgreSQL, MySQL, Redis, MongoDB, Tomcat `catalina.out`, java.util.logging (duas linhas), PHP |
| Plataformas | Kubernetes klog, prefixo CRI de contêiner (`stdout F`), Android logcat, Squid, AWS ALB/ELB, FortiGate e outros `chave=valor` |
| Tabular | CSV/TSV/`;`/`|` com cabeçalho (também após linhas de preâmbulo, como o log DHCP do Windows), sem cabeçalho (colunas `coluna N`), W3C/IIS/CloudFront |
| JSON | JSONL, arrays, envelopes (`Records`, `value`, `hits`…), objetos seguidos sem array (inclusive formatados em várias linhas), JSON embutido no texto da mensagem, chaves de Serilog, journald, GELF, pino e log4j2; exportações interrompidas no meio mantêm todos os registros completos |
| XML | Eventos do Windows salvos como XML (Visualizador de Eventos, `wevtutil /f:xml`, PowerShell), com a mensagem renderizada quando existe; log4j `XMLLayout`; java.util.logging `XMLFormatter`; qualquer arquivo de registros XML repetidos (o elemento do registro é descoberto no próprio arquivo) |

## Leitor genérico

Para qualquer outra linha: data e hora; nível em `[ERROR]`, `ERROR:`, `<warn>`, `|WARN|`, `[INF]`,
letras únicas (`I`, `E`) onde são usadas; itens de cabeçalho (`[thread]`, `(thread)`, logger
`com.exemplo.Classe`, `processo[pid]:`, pid/tid, componente, códigos `AH00126`/`MY-010116`); na
mensagem, requisições HTTP com status e tamanho, pares `chave=valor` (inclusive com aspas; URLs e
trechos entre aspas são ignorados) e JSON embutido. Registros de várias linhas (stack traces Java,
Python e .NET, linhas indentadas) são agrupados quando as linhas começam pela mesma forma de data e
as demais parecem continuação. Cores ANSI e caracteres de controle não atrapalham a leitura.

## Clientes (user agent)

Todo registro com user agent, em qualquer formato (`agent`, `user_agent`, `http_user_agent`,
`cs(User-Agent)` com `+` no lugar de espaços…), recebe `ua.class`, `ua.family`, `ua.version`,
`ua.os` e, quando a declaração de navegador é impossível na data do registro, `ua.anomaly` e
`ua.anomaly_detail`. Os campos podem ser filtrados, agrupados e exportados como os demais; veja
[Clientes e navegadores](comprometimentos.md#clientes-e-navegadores).

## Codificação, compressão e fim de linha

- UTF-8 (com ou sem BOM), UTF-16 e UTF-32 (com ou sem BOM), code pages legadas detectadas
  automaticamente (Windows-125x, ISO-8859-x, KOI8, Shift_JIS, GBK, Big5, EUC…) e CP850 do console
  do Windows em português. Uma linha em code page legada dentro de um arquivo UTF-8 é lida como
  Windows-1252, sem caracteres de substituição.
- gzip, bzip2, xz e zstd reconhecidos pelo conteúdo, inclusive em arquivos rotacionados sem extensão
  (`access.log.1`, `messages-20261012`).
- Fim de linha LF, CRLF ou só CR; linhas vazias ou só com bytes NUL (arquivos pré-alocados ou
  interrompidos) não viram registros.
- Ao abrir uma pasta, entram os arquivos com extensão de log, as rotações (`.1`, `.old`,
  `.2026-10-12`, `.20261012`), os sem extensão e qualquer outro cujo conteúdo seja texto ou log
  compactado; executáveis, imagens e documentos são ignorados. Em pacotes `.zip`/`.tar`, entram os
  de extensão de log, rotações e sem extensão.

Fontes já abertas com versões anteriores são reprocessadas uma vez ao reabrir (a revisão do leitor
faz parte da identidade dos índices); evidências capturadas em Casos permanecem como foram salvas.
