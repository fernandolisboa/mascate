# Monólito modular como workspace Cargo

Afiliação e venda própria vivem no mesmo app, divididos nos módulos Catalog & Discovery, Commerce, Inventory, Affiliate, Marketing, Integrations e Finance & Reporting (ver `docs/commerce-integrations.md`, seção 5). Cada módulo é um crate do workspace Cargo com interface pública explícita; nenhum módulo lê tabelas de outro, e a comunicação é por chamada à interface pública ou por evento interno. Um crate por módulo faz o compilador barrar dependências proibidas, o que substitui os testes de arquitetura que um monólito em outra linguagem precisaria.

## Considered Options

- **Apps separados para afiliação e comércio:** duplicaria catálogo, conexões e painel para um único usuário.
- **App sem fronteiras internas:** mais rápido no início, mas acopla estoque e afiliação pelo banco.
