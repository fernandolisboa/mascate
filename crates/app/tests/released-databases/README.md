# Bancos de exemplo por versão

Um banco por versão publicada, escrito pela própria versão com
`cargo test -p mascate -- --ignored write_this_versions_sample_database` antes da tag.
O teste `every_released_database_migrates_to_this_version` migra cada um até a versão
atual, com Backup antes, e confere que o que o dono salvou continua lá.

- `0.1.0.db`: a `main` antes das atualizações (#8), que nunca virou release. É o banco que
  quem rodou o app em desenvolvimento tem hoje.
