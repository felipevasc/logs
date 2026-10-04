# Casos, Achados, índices e conexões

Um **Caso** guarda a investigação inteira. **Dataset** é a fonte aberta para explorar; **Achados** é a seleção preservada de registros, correlações e outros itens. Os nomes internos legados do formato portátil continuam compatíveis.

## Isolamento

Criar um Caso abre um espaço vazio. Alternar Casos recupera a fonte e a área do destino, filtros, referências, interpretação, rascunhos, preferências e análises daquele Caso. Tema e zoom também são salvos por Caso. Uma operação admitida no Caso anterior não pode publicar respostas na tela do destino.

Limites de hardware, o serviço MCP e a biblioteca de acessos remotos são recursos do computador. A biblioteca só é consultada explicitamente; uma coleta mantém seus caminhos e rascunhos no Caso. Credenciais ficam fora dos arquivos portáteis de investigação.

## Índices opcionais

Na árvore de campos ou no nome de um campo do detalhe, use o botão direito e **Criar índice para este campo/token**. Um marcador suave indica o índice completo; o menu e a descrição ao passar o mouse confirmam seu estado.

O índice armazena valores da fonte original interpretada. Seu identificador inclui Caso, análise, interpretação e identidade/ordem dos arquivos. Mudanças nesses dados exigem um novo índice. Igualdade exata e igualdade sem distinção ASCII de maiúsculas usam candidatos seletivos do índice, preservando o predicado canônico e o recorte visível. Resultados frequentes usam o motor vetorizado, sem truncamento. Cancelamento ou falha nunca publica um índice parcial.

## SSH no Linux

Escolha **SSH (Linux)**, endereço `ssh://servidor:22`, usuário e chave privada local opcional. Sem chave específica, o cliente usa a configuração/agente OpenSSH do computador. O servidor precisa de Python 3. O host deve ter sua chave conhecida e validada no OpenSSH; a coleta não aceita automaticamente uma chave desconhecida ou alterada.

Selecione sugestões de logs de sistema, autenticação, nginx, Apache ou audit, ou informe caminhos absolutos, um por linha. Pastas são percorridas dentro dos limites da coleta. Arquivos inexistentes podem ser ignorados com aviso, desde que haja arquivos válidos. Links simbólicos não são seguidos. O teste de acesso verifica leitura; a importação copia bytes para uma fonte local e então abre a indexação.

## WinRM no Windows

Escolha **WinRM (Windows)** e um endpoint HTTPS, por exemplo `https://servidor:5986/wsman`. O cliente desta modalidade requer Windows com PowerShell e remoting configurado no servidor. Deixe o usuário vazio para usar a autenticação integrada ou informe usuário e senha. A validação de certificado permanece ativa.

As sugestões incluem os EVTX de System, Application e Security, a pasta de EVTX e IIS. Também é possível indicar múltiplos arquivos/pastas absolutos. Para EVTX ativos na pasta padrão do Windows, o coletor exporta o canal com `wevtutil` e copia o snapshot; os demais arquivos usam `Copy-Item -FromSession`. Links/reparse points são recusados.

O limite padrão é 512 MiB, ajustável entre 1 MiB e 4 GiB. Cada coleta admite até 64 caminhos, 2.048 arquivos e 8.192 subpastas. A coleta incompleta é descartada. Arquivos locais concluídos são publicados juntos e passam pela detecção automática de formato. Permissões de leitura/exportação devem existir no servidor.

## Validação

As verificações locais cobrem validação de caminhos, preservação binária do transporte, limites, isolamento e controles da interface. Não foi usado um servidor SSH/WinRM autenticado nesta implementação; configuração de domínio, certificados, políticas e permissões precisa ser conferida no ambiente de destino.

Referências: [OpenSSH](https://man.openbsd.org/ssh), [verificação de host](https://man.openbsd.org/OpenBSD-current/man/ssh_config) e [cópia por PowerShell remoting](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/copy-item?view=powershell-7.6).
