# Aprendizado local e conhecimento incremental

Os dois incrementos são executados em CPU. O aplicativo não chama um LLM e
não envia logs para treinamento externo. Não é necessário rotular os registros.

## Uso

Abra **Comprometimentos** e calcule a análise. A investigação oferece
**Aprendizado local sem rótulos** e **Diagnósticos aprendidos**. Cada diagnóstico
reúne os desvios do mesmo contexto em um intervalo de cinco minutos, apresenta
comparações entre grupos e permite abrir todos os eventos de apoio.

Ocorrências isoladas também ficam acessíveis, com prioridade menor e indicação
de que falta suporte adicional. A raridade não transforma uma observação em
incidente confirmado.

As opções ficam em **Regras → Análise comportamental, IOC e políticas do
ambiente**. Aprendizado e atualização incremental são habilitados por padrão;
ambos podem ser desativados por Caso. Navegar ou paginar um resultado não
inicia treinamento. Uma nova análise explícita aplica o perfil disponível no
início do cálculo.

**Aprender mensagens e medidas deste recorte** é uma revisão opcional de um
diagnóstico, depois de conferir seus originais. Ela incorpora os padrões
selecionados ao perfil, conserva controles independentes e continua excluindo
evidências E3+ e sinais prioritários. A ação não torna a operação benigna, não
apaga o diagnóstico original e não desativa outros detectores, como mudanças
temporais. Não é preciso revisar ou rotular os registros para usar o aprendizado
automático; a revisão permite acrescentar novidades que ele manteve separadas.

**Voltar à revisão anterior** restaura as contagens e os recibos correspondentes.
São conservadas três revisões anteriores. **Reiniciar perfil local** guarda uma
cópia recuperável do perfil antes da limpeza; o botão de retorno também pode
recuperar essa cópia. Essas ações não modificam os arquivos de log.

## Incremento 1: aprender e investigar

O núcleo independente está em `crates/log-learning`. Ele combina:

- Agrupamento incremental de mensagens por tokens. O agrupamento aprende
  posições variáveis; preserva palavras que distinguem falha, sucesso, início,
  conclusão e timeout. Não é uma implementação de Drain.
- Histogramas logarítmicos de duração e bytes, frequências de resultados e
  combinações entre família da mensagem, resultado e medidas. Valores ausentes
  permanecem ausentes; somente quantidades com unidades normalizadas entram.
- Transições de primeira ordem com suavização de Jeffreys. São consideradas
  apenas sequências com trace/request/correlation ID ou sessão explícitos
  (sessões são separadas por host conhecido),
  horários distintos e distância de até 30 minutos. Eventos sem vínculo não
  são convertidos em sequências por proximidade temporal.
- Detecção cumulativa de mudanças de duração e comparação de taxas de falha
  em janelas cronológicas de um minuto. Uma mudança moderada persistente pode
  aparecer antes de ultrapassar o limite de extremos individuais.

O perfil pertence ao Caso, à área de análise e à interpretação dos campos.
Namespaces explícitos permanecem separados. Quando existe serviço identificado,
seu aprendizado continua entre arquivos rotacionados do mesmo serviço e
operação. As comparações de diagnóstico conservam fonte, namespace, serviço,
operação e intervalo: hosts, versões ou dependências são comparados com os demais
grupos presentes naquele mesmo recorte. São mostrados denominadores e limites
de Wilson; ausência de controles é declarada.

Uma associação indica onde investigar. Não comprova causa, ataque ou efeito.
Os diagnósticos aprendidos têm paginação própria, para que milhares de episódios
não inflem a fila de segurança nem seu cálculo de prioridade. Nenhum detector
promove o nível de evidência.

## Incremento 2: continuar aprendendo

Uma cópia congelada do modelo avalia a população inteira dos contextos
admitidos. Na primeira coleta, os primeiros 60% do intervalo de cada contexto
formam uma referência exploratória; o trecho posterior é avaliado separadamente.
Sem quantidade suficiente de dados, a interface declara falta de referência.

A atualização candidata usa apenas observações elegíveis dos primeiros 80%
do intervalo. Os últimos 20% ficam reservados para validação, junto com exemplos
históricos congelados. Esses exemplos históricos também ficam fora do treinamento.
São excluídos eventos com tempo ou leitura inválidos, membros E3+, hipóteses
prioritárias existentes e desvios apontados pelo modelo congelado. Menos de 90%
de registros válidos impede promoção automática da atualização.

O candidato exige pelo menos vinte observações reservadas e não pode aumentar
os alertas nos controles cuja referência já estava estabelecida. Essa é uma
verificação de consistência, **não uma medição da precisão em incidentes reais**.
Falhas frequentes no período escolhido ainda podem contaminar uma referência
não supervisionada; conhecer a cobertura e escolher dados representativos
continua necessário.

O candidato só é salvo depois da publicação completa da análise. Contagens,
revisão e recibos são gravados em uma transação SQLite com comparação da revisão
anterior. Cancelamento, análise descartada, avaliação de proposta ou candidato
concorrente não aplicam silenciosamente uma atualização. Um problema no perfil
local deixa a investigação tradicional disponível e aparece na interface.

O recibo inclui identidade original, horário e conteúdo interpretado. Repetir a
mesma análise não incrementa novamente as contagens. Recibos antigos podem ser
compactados em um Bloom de 2 MiB; falsos positivos apenas deixam de admitir
algumas observações para treinamento. A taxa estimada é informada e bloqueia
novas atualizações ao atingir 2%. Os recibos recentes necessários ao retorno de
revisão continuam exatos.

## Compartilhamento pelo Git

O perfil local contém padrões de mensagens e nunca é o documento exportado.
O protocolo público aceita somente domínio enumerado, estados estruturais de
vocabulário fechado e contagens inteiras. Não aceita mensagens, nomes de campos,
IPs, usuários, hosts, URLs, timestamps, IDs de templates ou hashes de valores
privados. O ID da contribuição é calculado somente sobre seu conteúdo público.
Contagens agregadas também descrevem atividade; o compartilhamento é opcional.

1. Configure `learning.github_repository` como `proprietario/projeto`. A branch
   padrão é `learning-models`.
2. Depois de uma atualização local aceita, revise **Base compartilhada e
   contribuições**. É possível exportar um JSON e mantê-lo offline.
3. **Publicar contribuição no Git** usa um token temporário informado para
   aquela operação. O token não é salvo no Caso ou no perfil. A branch dedicada
   é criada a partir da principal quando ainda não existe. Nenhum log é enviado.
4. O workflow `learning.yml` valida o protocolo e os testes do núcleo,
   consolida as contribuições em `models/shared-model.json` e grava a nova
   revisão na branch dedicada. Uma repetição do mesmo ID não duplica contagens.
5. **Baixar base do Git** ou **Importar base compartilhada** valida esquema,
   orçamentos e coerência das contagens antes de substituir o arquivo local.
   A próxima análise explícita usa essa revisão. O aplicativo funciona offline
   depois disso.

A base compartilhada usa domínios separados e oferece candidatos estruturais
quando falta referência local; ela não determina o comportamento normal de uma
empresa nem substitui seu perfil. A base inicial está vazia: não se atribui
treinamento ou eficácia a um corpus que não foi fornecido.

O núcleo também pode consolidar documentos exportados sem abrir o aplicativo:

```sh
cargo test --manifest-path crates/log-learning/Cargo.toml --locked
cargo run --manifest-path crates/log-learning/Cargo.toml --locked -- consolidate models/contributions models/shared-model.json
cargo run --manifest-path crates/log-learning/Cargo.toml --locked -- check models/shared-model.json
```

Arquivos individuais e pacotes exportados são aceitos na pasta de contribuições.
A consolidação é determinística e independente da ordem dos arquivos. O workflow
automático opera na branch `learning-models`; um destino com outra branch precisa
configurar seu workflow correspondente. Uma falha de rede não interrompe a
análise local. Publicação parcial pode ser repetida: IDs existentes são ignorados.

## Orçamentos e validação

Os padrões são limitados a 48 tokens de tamanho limitado. Os padrões padrão
de configuração admitem 32 contextos, 128 famílias por contexto, 512 transições,
256 combinações, 10.000 observações novas por análise e 256 exemplos de validação.
O modelo individual tem teto de 16 MiB; o SQLite do perfil tem teto de 128 MiB,
além da cópia recuperável de reinicialização. Contagens locais antigas perdem
peso por redução periódica. Saturação do vocabulário é declarada e desativa
inferências que poderiam confundir limite de armazenamento com novidade.

As contribuições são limitadas a 10.000 observações cada; a base compartilhada
aceita até 4.096 contribuições por época. Uma base saturada exige revisão de
época, preservando a anterior no histórico Git.

O custo de aprendizado é limitado; a leitura, ordenação e confirmação dos
eventos completos ainda dependem do volume dos logs e podem usar disco. Não
há promessa de latência ou RAM em computadores simples sem medição nesses
equipamentos. Os testes cobrem descoberta sem rótulos, sequências, mudanças,
combinações, limites, deduplicação, controles de diagnóstico, isolamento,
transações, retorno de revisão e recusa de conteúdo privado no protocolo público.
