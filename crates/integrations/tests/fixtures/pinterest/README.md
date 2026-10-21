# Respostas da API v5 do Pinterest

Respostas escritas a partir da documentação oficial em
https://developers.pinterest.com/docs/ (autenticação e autorização, boards,
pins, níveis de acesso e limites), lida em 21/10/2026, sem conta real. Os
formatos seguem os esquemas e os exemplos da documentação (OpenAPI 5.28.0);
os valores são inventados.

- `token.json`: `POST /v5/oauth/token` com o código (`authorization_code`).
- `token-renewed.json`: a renovação (`refresh_token`), com o refresh token
  novo do refresh contínuo de 60 dias.
- `token-refused.json`: o código ou o refresh token recusado. A documentação
  não mostra o corpo desse erro; o app lê `message`, `error_description` ou
  `error`, o que vier.
- `boards-page-1.json`, `boards-page-2.json`: `GET /v5/boards`, duas páginas
  ligadas pelo `bookmark`; a segunda termina com `bookmark: null`.
- `board.json`: `GET /v5/boards/{board_id}`.
- `pin.json`: o 201 de `POST /v5/pins`.
- `error-*.json`: `{code, message}` de 400, 401 (código 2, token vencido ou
  inválido), 403, 404, 429 e 500.

A regression pass da fase 3 (#116) regrava estes arquivos com a conta real e
confere o que a documentação deixa em aberto: o corpo do erro do endpoint de
token; se um app com acesso Trial cria Pins em `api.pinterest.com` (visíveis
só para o dono) ou precisa do sandbox; se o 429 traz `Retry-After`; e o
tamanho máximo de `image_base64`.
