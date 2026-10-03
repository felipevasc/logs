# Chave das atualizações

`signing.key` e `signing.key.pub` assinam os pacotes que o LogInsight baixa ao se atualizar.

**A chave privada é pública de propósito.** Ela fica no repositório, sem senha, para que qualquer pessoa com permissão de publicar no GitHub gere uma versão sem configurar segredos. O build (`npm run build`) a usa automaticamente.

Por isso a assinatura não prova quem publicou. Ela garante que o arquivo baixado está íntegro e é da versão anunciada no manifesto (`requireSignedVersion`). Quem garante que a atualização é legítima é o GitHub: HTTPS e a permissão de escrita no repositório.

Não troque nem apague estes arquivos: as instalações existentes só aceitam pacotes assinados com esta chave. Veja [docs/atualizacoes.md](../../docs/atualizacoes.md).
