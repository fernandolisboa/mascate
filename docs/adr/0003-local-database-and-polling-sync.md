# Banco local SQLite-compatível e Sync por polling

Os dados ficam num banco local SQLite-compatível da família Turso (crate `turso` se a sincronização dele estiver estável quando a persistência for implementada, senão `libsql`), sem servidor. Como um app desktop não recebe webhooks, os Syncs com o Mercado Livre e as demais Platforms são por polling (pedidos recentes mais `/missed_feeds`) enquanto o app está aberto ou minimizado na bandeja, a cada 5 minutos por padrão. Pedido que chega com o computador desligado aparece no próximo Sync; o app do ML no celular cobre esse intervalo.

## Consequences

- O motor da Turso deixa a réplica gerenciada (primeira opção de sincronização entre máquinas, ADR 0004) ser configuração, não reescrita.
- Segredos nunca ficam no banco: tokens e chaves vão para o cofre do sistema (Windows Credential Manager, Secret Service no Linux, Keychain no macOS). Variáveis de ambiente servem só para desenvolvimento.
