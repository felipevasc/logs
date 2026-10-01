# Recuperação nativa dos Casos (integração em andamento)

A preparação usa a API de backup do SQLite para obter uma cópia consistente e verifica seus bytes, integridade e autoridade. O conjunto copiado inclui as imagens referenciadas, referências preparadas, todos os lotes e máscaras de exclusão, histórico de envelopes/manifests nativos e as definições legadas utilizadas na adoção.

Uma versão de referência inteiramente ausente é registrada como indisponível observada. Isso preserva o estado existente; não significa que a análise seja reproduzível sem aqueles bytes. Uma versão parcialmente presente ou com checksum inválido impede a preparação.

Os limites atuais são 1.000 Casos, 16.384 entradas de ativos, 8 MiB de metadados de recuperação e 32 GiB de conteúdo. A cópia é sequencial, com buffer de 256 KiB. São limites dos componentes, não um teto de RSS. Os originais de logs, índices regeneráveis e outras configurações da aplicação não fazem parte desta cópia dos Casos.

O banco ativo é aberto somente pelo SQLite. Abrir e fechar um `File` comum no mesmo inode poderia liberar locks POSIX mantidos por outra conexão do processo. A verificação inicial recusa caminhos que já sejam links ou arquivos especiais; substituições externas arbitrárias de inode durante o open não pertencem ao protocolo cooperativo do perfil.

A preparação e as operações do perfil usam uma lease de raiz por operação. A cópia publicada retém somente sua lease de geração e a identidade da pasta; um cache ou uma cópia selecionada não bloqueia restauração indefinidamente.

A restauração exposta pela interface prepara um perfil novo e verificado. O processo atual continua usando sua raiz original; somente uma nova abertura seleciona o destino. A escolha ocorre antes de carregar catálogos, preferências, MCP ou caches. O perfil anterior permanece intacto e pode ser selecionado novamente para o próximo reinício. Não há comando público de restauração sobre o banco em uso.

Esse fluxo recupera os Casos e seus ativos. Preferências globais, conexões remotas, credenciais e catálogos externos não são copiados para o perfil novo. Eles continuam no perfil original. A tela informa esse limite antes da preparação e mostra o destino e a necessidade de fechar e reabrir a aplicação.

O marcador de seleção é atômico. Uma falha após a publicação é reconciliada antes de responder; uma cópia alterada antes da seleção mantém o perfil anterior. O retorno ao original verifica uma cópia SQLite consistente e recusa a escolha se o banco original mudou desde a preparação. Nenhuma dessas ações troca a raiz de caches de um processo em execução.

A etapa integrada ainda exige a validação da aplicação completa e do Windows antes de ativação. O conjunto isolado de recuperação passou 89 verificações, incluindo 12 cenários de seleção, cancelamento, falhas tardias de persistência, corrupção e retorno ao perfil original.
