# Bancos de exemplo por versão

Um banco por versão publicada, escrito pela própria versão com
`cargo test -p mascate -- --ignored write_this_versions_sample_database` antes da tag.
O teste `every_released_database_migrates_to_this_version` migra cada um até a versão
atual, com Backup antes, e confere que o que o dono salvou continua lá.

- `0.1.0.db`: a `main` antes das atualizações (#8), que nunca virou release. É o banco que
  quem rodou o app em desenvolvimento tem hoje.
- `0.2.0.db`: a primeira release, com a fase 1 inteira.
- `0.3.0.db`: a fase 2, com comissões, Payouts, regras de divulgação, Affiliate Links,
  Offer Posts na fila do Telegram (pausada) e o calendário de Sales Events.
- `0.4.0.db`: o Merge Import, com uma mesclagem já vista de uma cópia do outro computador
  que trouxe um Fornecedor e perdeu o conflito da alíquota de imposto.
- `0.5.0.db`: os canais de venda novos e a fase 3, com o Produto anunciado também na Shopee
  com o preço baixado pelo dono, um canal do Pinterest com a pasta escolhida e o link curto
  do Affiliate Link com os seus cliques.
