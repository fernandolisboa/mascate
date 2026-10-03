# Cofre do sistema via keyring-core

Os segredos das Connections (ADR 0003) chegam ao cofre do sistema pelo `keyring-core` com uma loja por sistema: `windows-native-keyring-store` (Gerenciador de Credenciais) no Windows e `zbus-secret-service-keyring-store` (Secret Service: GNOME Keyring, KWallet) no Linux, este com D-Bus e criptografia em Rust puro, sem libdbus nem OpenSSL. O resto do app só conhece a interface `SecretStore` do crate `platform`; os testes usam um cofre em memória. Em build de desenvolvimento, uma variável de ambiente com o nome do segredo tem precedência sobre o cofre; em build de release o ambiente é ignorado.

## Considered Options

- **`keyring` 4 (crate guarda-chuva):** uma dependência só, mas traz lojas que o app não usa; os mantenedores indicam `keyring-core` mais as lojas escolhidas para apps.
- **Implementação própria** (API do Windows e crate `secret-service`): mais código nosso e exige `unsafe`, que o workspace proíbe.

## Consequences

- No Linux o cofre depende de um serviço de Secret Service rodando e destravado; sem ele, as Connections aparecem "com erro" com o motivo, e o app segue funcionando.
- macOS ainda não tem loja: o cofre responde que não há suporte até a fase em que o macOS entrar.
