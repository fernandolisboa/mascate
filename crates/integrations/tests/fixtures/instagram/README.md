# Respostas da API do Instagram com login do Instagram

Respostas escritas a partir da documentação oficial em
https://developers.facebook.com/docs/instagram-platform/ (primeiros passos,
publicação de conteúdo, `content_publishing_limit`, contêiner, mídia,
`refresh_access_token` e códigos de erro), lida em 21/10/2026, sem conta
real. O app usa `graph.instagram.com` na v25.0, como o ADR 0035 decidiu; a
documentação de hoje já mostra exemplos na v26.0. Os formatos seguem os
exemplos da documentação; os valores são inventados.

- `me.json`: `GET /me?fields=user_id,username`. A referência mostra os campos
  no topo e o guia de primeiros passos dentro de `data`; o app lê os dois.
- `limit.json`, `limit-used.json`: `content_publishing_limit` com
  `quota_usage` e `config.quota_total` (a documentação fala em 100 posts em
  24 horas e, na parte de carrossel, em 50).
- `container.json`: o id do contêiner criado por `POST /{ig-user-id}/media`.
- `container-*.json`: `GET /{container-id}?fields=status_code,status` em
  cada estado. Com `ERROR`, a referência diz que `status` traz o subcódigo
  do erro; o formato exato do texto não aparece.
- `media-published.json`: o id da mídia publicada por `media_publish`.
- `media.json`: `GET /{media-id}?fields=permalink`.
- `token-renewed.json`: `GET /refresh_access_token`, com `expires_in` de 60
  dias.
- `error-*.json`: o envelope `{"error": {message, type, code,
  error_subcode, is_transient, error_user_msg, fbtrace_id}}` com o token
  vencido (190), limite de chamadas (4), cota de posts (9, 2207042),
  proporção da foto (36003, 2207009), menções demais (100, 2207040) e falha
  do lado deles (2).

A regression pass da fase 3 (#116) regrava estes arquivos com a conta real e
confere o que a documentação deixa em aberto: se `graph.instagram.com` aceita
o token no cabeçalho `Authorization: Bearer` também nos `GET` (a
documentação mostra o cabeçalho nos `POST` e `access_token` na query nos
`GET`); o formato de `/me`; o texto de `status` num contêiner com `ERROR`;
se `media_publish` repetido com o mesmo contêiner responde erro; e a cota
real da conta.
