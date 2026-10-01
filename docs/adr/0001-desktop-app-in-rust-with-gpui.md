# App desktop em Rust com GPUI

O Mascate é um app desktop nativo em Rust com interface em GPUI (o framework do Zed), rodando primeiro no Windows 11 e testado no Linux (Omarchy/Wayland); macOS fica para bem depois. Decisão do Fernando em 01/10/2026: ele quer gerenciar tudo localmente e não pretende ler muito código, então o critério foi um app só, local, sem servidor para manter.

## Considered Options

- **Web hospedada (.NET/Angular ou TypeScript):** recebe webhooks e abre no celular, mas exige servidor pago e tira o "local". Volta à mesa só se o acesso pelo celular virar necessidade (ver ADR 0004).
- **Tauri 2 (núcleo Rust + interface web):** era a recomendação inicial pelo ecossistema maduro de componentes; descartada porque o Fernando preferiu Rust nativo de ponta a ponta.
- **GPUI:** escolhido. O risco de ser pré-1.0 e ter pouca documentação é mitigado pelo `gpui-component` (Longbridge), com mais de 60 componentes desktop usados em produção.

## Consequences

- O núcleo (regras de negócio, integrações, persistência) não depende de GPUI. A interface é uma camada fina por cima, para que uma futura interface em macOS, celular ou web reaproveite o núcleo.
- Versões do GPUI e do `gpui-component` ficam fixadas e são atualizadas de forma deliberada, nunca por faixa de versão.
