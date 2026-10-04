# Atualizações pelas GitHub Releases, com ureq e manifesto próprio

O app fala com a internet por um único cliente HTTP, o `ureq` (síncrono, rustls), chamado nas threads de fundo do executor do GPUI, sem runtime async extra. Os adapters das Platforms usam o mesmo cliente.

A atualização (ADR 0007) é código nosso no crate `platform`. Ela lê a última release na API pública do GitHub (sem token) e o manifesto `mascate-update.json` que o workflow de release publica em cada versão. O manifesto lista os instaladores, cada um com o SHA-256, e todas as migrações de risco publicadas até aquela versão. Cada `Migration` declara se é de risco (`risky`). A versão nova é de risco para uma instalação quando o manifesto lista uma migração de risco que o app instalado ainda não roda, então pular versões não esconde uma migração de risco intermediária.

O download só é aceito de dentro do repositório do app e por HTTPS, com tamanho e SHA-256 conferidos antes de rodar. No Windows o instalador NSIS roda em modo silencioso (`/S /R`) depois que o app fecha. No Linux o AppImage é trocado no lugar. Um pacote do Linux ou um build de desenvolvimento só recebe o aviso com o link da versão. Antes de migrar, o app faz um Backup. Se a migração falhar, o Backup volta para o lugar do banco e o app reinstala a versão de onde veio, que fica anotada na pasta de atualizações junto com a versão que falhou, para a atualização silenciosa não trazê-la de novo.

## Considered Options

- **`reqwest`:** o cliente async mais usado, mas exige um runtime tokio ao lado do executor do GPUI e compila bem mais.
- **`cargo-packager-updater`:** o atualizador do empacotador que o projeto já usa, com assinatura minisign. Exige uma chave de assinatura guardada como secret no repositório, traz `reqwest` e tokio, e não cobre a migração de risco cumulativa nem a volta de versão.
- **Cliente HTTP do GPUI:** prenderia o núcleo a um crate do snapshot semanal do GPUI.

## Consequences

- A autenticidade de uma versão depende do HTTPS do github.com e da conta do GitHub do dono, a mesma confiança de baixar o instalador à mão. Assinar os instaladores (minisign ou Authenticode) fica para quando houver uma chave para guardar.
- A API pública do GitHub aceita 60 consultas por hora por IP; o app consulta ao abrir e a cada 6 horas, e um 403 ou 429 só adia a próxima tentativa.
- Cada release publicada deixa um banco de exemplo em `crates/app/tests/released-databases/`, e um teste migra todos até a versão atual. O workflow de release recusa uma tag sem esse arquivo.
