# Respostas do Mercado Livre (da documentação)

Estes arquivos **não** foram gravados com uma conta real: seguem os exemplos e a lista de campos
da documentação oficial do Mercado Livre em pt-BR, consultada em 04/10/2026 (via Context7,
`/websites/developers_mercadolivre_br_pt_br`). O regression pass da fase 1 (#37) os regrava com a
conta de vendedor do Fernando.

| Arquivo | Recurso | Página da documentação |
|---|---|---|
| `token.json` | `POST /oauth/token` com `grant_type=refresh_token` | Autenticação e autorização |
| `token-invalid-grant.json` | o mesmo, com refresh token já usado (400) | Autenticação e autorização |
| `categories.json` | `GET /sites/MLB/categories` | Categorias e atributos |
| `category-MLB3697.json` | `GET /categories/{id}` (`path_from_root`) | Categorias e atributos |
| `highlights-MLB1000.json` | `GET /highlights/MLB/category/{id}` | Mais vendidos no Mercado Livre |
| `product-MLB19615318.json` | `GET /products/{id}` (`buy_box_winner`) | Buscador de produtos |
| `user-product-MLBU3013800008.json` | `GET /user-products/{id}` | Preço por variação |
| `items-multiget.json` | `GET /items?ids=...&attributes=...` | Produtos classificados por ID do vendedor |
| `products-search.json` | `GET /products/search` | Buscador de produtos |
| `product-items-MLB19615318.json` | `GET /products/{id}/items` | Concorrência em catálogo |
| `listing-prices.json` | `GET /sites/MLB/listing_prices` com `listing_type_id` | Comissão por vender |
| `error-403.json`, `error-429.json` | formato de erro da API | Boas práticas / erros |
| `users-me.json` | `GET /users/me` (o id do vendedor) | Consulta de usuários |
| `user-items-active.json`, `user-items-active-end.json`, `user-items-paused.json` | `GET /users/{id}/items/search` com `status` e `search_type=scan` (página com `scroll_id` e a última, vazia) | Itens e buscas |
| `items-seller.json` | `GET /items?ids=...&include_attributes=all` dos anúncios do vendedor: simples, com variações e pausado | Itens e buscas; Variações |
| `items-seller-gone.json` | o mesmo multiget depois: um anúncio encerrado e outro apagado (`code` 404) | Itens e buscas |

Campos que a documentação não mostra por inteiro e que foram completados pelo formato dos
recursos vizinhos (conferir no regression pass):

- `product-items-*.json`: a página mostra o recurso, mas não um exemplo completo; os campos
  usados aqui (`paging.total`, `results[].item_id`, `price`, `currency_id`, `category_id`) são os
  mesmos de `/items`.
- `listing-prices.json`: `sale_fee_details` vem do exemplo para outros sites; o app lê só
  `sale_fee_amount` e `currency_id`.
- `product-*.json`: `buy_box_winner` traz o preço do anúncio que vence a competição.
- `items-seller*.json`: a página de variações diz que o SKU do vendedor vem, nesta ordem, do
  atributo `SELLER_SKU` da variação, do `seller_custom_field` da variação, do `SELLER_SKU` do
  anúncio e do `seller_custom_field` do anúncio; os anúncios cobrem esses casos. O corpo do 404
  no multiget segue o formato de erro da API.
- `user-items-*.json`: a busca por `scan` devolve `scroll_id` até a última página, que vem sem
  resultados; o valor do `scroll_id` aqui é inventado.
