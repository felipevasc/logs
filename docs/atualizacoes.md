# Atualizações

O LogInsight se atualiza a partir das [Releases do GitHub](https://github.com/felipevasc/logs/releases). A verificação é automática; o download e a instalação só acontecem com a confirmação de quem usa.

## Como funciona

1. Alguns segundos depois de abrir, o aplicativo consulta `https://github.com/felipevasc/logs/releases/latest/download/latest.json`, em segundo plano e com limite de 20 segundos. Sem internet ou com erro, nada interrompe o trabalho; o resultado aparece em **Configurações → Atualizações**.
2. Se houver versão maior, um aviso mostra as notas da versão e oferece **Atualizar agora**, **Mais tarde** ou **Pular esta versão**.
3. O download mostra o progresso e permite continuar trabalhando. O pacote é conferido pela assinatura e pela versão assinada antes de qualquer instalação.
4. Com o pacote pronto, **Reiniciar e instalar** salva os casos, fecha o aplicativo, instala e abre a nova versão. **Instalar ao fechar** deixa a instalação para quando a janela for fechada.
5. Na primeira abertura de uma versão diferente, o aplicativo copia os arquivos de dados (casos, configurações e bancos SQLite, sem caches, índices e imagens) para `backups/` na pasta de dados e mantém as três cópias mais recentes. Depois avisa **Atualizado para X**, ou que a instalação não foi concluída, com o caminho para baixar manualmente.

Em **Configurações → Atualizações** ficam a versão e o tipo de instalação, a última verificação, **Verificar agora**, a opção de verificar ao abrir e a versão pulada, que pode voltar a ser oferecida.

## Por tipo de instalação

Cada instalação recebe o pacote do seu próprio formato; o executável registra qual instalador o gerou.

| Instalado com | Como atualiza | Administrador |
|---|---|---|
| Windows `.exe` (instalação por usuário) | Executa o novo instalador em modo passivo e reabre o aplicativo | Não |
| Windows `.msi` | Executa o novo MSI | Sim (UAC) |
| Linux `.AppImage` | Substitui o arquivo (com restauração se falhar) e reinicia | Não, se a pasta for do usuário |
| Linux `.deb` / `.rpm` | `pkexec dpkg -i` / `pkexec rpm -U` | Sim (senha) |
| `npm run dev` ou executável fora de um instalador | Desativado; atualizar baixando a nova versão | — |

- Se o aplicativo foi aberto como administrador com outra conta no Windows, ele pede para ser reaberto normalmente, pois o instalador por usuário atualizaria o perfil errado.
- Se o AppImage estiver numa pasta sem permissão de escrita, o aviso sugere movê-lo para uma pasta do usuário ou baixar manualmente.
- `dpkg -i` e `rpm -U` não instalam dependências novas. Se uma versão mudar as dependências do Linux, as notas da versão devem pedir a atualização manual desses pacotes.

## Desativar

- Por usuário: desmarque **Verificar se há uma versão nova** em Configurações → Atualizações.
- Por máquina (ambientes isolados ou geridos por TI): defina a variável de ambiente `LOGINSIGHT_UPDATES=0`. O aplicativo deixa de consultar o GitHub e de oferecer instalações.

A verificação envia ao GitHub apenas a requisição do arquivo `latest.json` (endereço IP e identificação do atualizador, como qualquer download).

## Publicar uma versão

Qualquer pessoa com permissão de escrita no repositório publica; não há segredos a configurar.

```powershell
npm run release -- 0.6.1
```

Na primeira execução o comando cria `docs/releases/v0.6.1.md`. Escreva as novidades, que aparecem no aviso de atualização, e rode o comando de novo. Ele confere se a versão é maior que a atual, se não há alterações pendentes e se a branch contém a `main` do GitHub; então alinha a versão em `package.json`, `package-lock.json`, `Cargo.toml`, `Cargo.lock` e `tauri.conf.json`, cria o commit `release: v0.6.1` e a tag `v0.6.1`. Para publicar:

```powershell
git push --follow-tags origin HEAD:main
```

A tag inicia o workflow **Build Windows and Linux**:

1. testes do backend e dos scripts de publicação;
2. instaladores de Windows e Linux, cada um com a sua assinatura (`.sig`);
3. teste de atualização ponta a ponta nas duas plataformas (abaixo);
4. conferência de que o commit da tag está na `main`;
5. geração do `latest.json` e conferência de cada pacote como os aplicativos instalados farão: assinatura, versão assinada, nome e endereço;
6. Release em rascunho com os 12 arquivos, verificação dos arquivos enviados e publicação;
7. conferência de que `releases/latest/download/latest.json` já anuncia a nova versão.

Uma falha em qualquer etapa impede a publicação. A publicação também recusa versão menor que a última publicada, versão já publicada e tag de outro commit.

## Controle

- **Pausar uma versão com problema:** no GitHub, marque a Release anterior como *Latest*. Os aplicativos deixam de oferecer a versão com problema; quem já a instalou permanece nela.
- **Corrigir:** publique uma versão nova com a correção. O aplicativo não instala versões menores.
- Não apague Releases antigas: elas permitem instalar uma versão anterior manualmente.
- Para conferir o que os aplicativos recebem: `curl -L https://github.com/felipevasc/logs/releases/latest/download/latest.json`.

## Assinatura

Os pacotes são assinados no build com a chave em [`src-tauri/updater/`](../src-tauri/updater/README.md). Essa chave é pública de propósito, para dispensar segredos: a assinatura garante que o arquivo baixado está íntegro e é da versão anunciada (`requireSignedVersion`). Quem garante que a atualização é legítima é o GitHub: HTTPS e a permissão de escrita no repositório. Mantenha essa permissão restrita.

## O que não pode mudar

As instalações existentes dependem destes pontos. Mudá-los deixa essas instalações sem atualização automática:

- o endereço `https://github.com/felipevasc/logs/releases/latest/download/latest.json` e o nome `latest.json`;
- a chave de assinatura (`src-tauri/updater/signing.key` e a `pubkey` em `tauri.conf.json`);
- os nomes das plataformas no manifesto (`windows-x86_64-nsis`, `windows-x86_64-msi`, `linux-x86_64-appimage`, `linux-x86_64-deb`, `linux-x86_64-rpm`);
- `identifier` e `productName` em `tauri.conf.json`, e `installMode: currentUser` do instalador NSIS;
- versões sempre crescentes;
- o runner Ubuntu 22.04 dos pacotes Linux, que define a versão mínima da glibc: trocá-lo pode entregar um binário que não abre em distribuições mais antigas.

## Teste ponta a ponta

```powershell
node scripts/release/update-e2e.mjs
```

Compila o código atual e uma versão `99.0.0` com outro nome de produto (`LogInsightE2E`), outro identificador e outra pasta de dados, instala a primeira (NSIS no Windows, AppImage no Linux), serve a segunda em `http://127.0.0.1:47831` e confere que o aplicativo instalado encontra a atualização, baixa, confere a assinatura, instala, reabre na nova versão e avisa que foi atualizado. Ao final desinstala a versão de teste. A instalação real do LogInsight não é afetada. O fluxo automático sem confirmação existe apenas nesses builds de teste (`--features update-e2e`).
