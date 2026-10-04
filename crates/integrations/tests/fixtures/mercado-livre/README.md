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
| `listing-prices-fixed-fee.json` | o mesmo, abaixo de R$ 79, com `fixed_fee` | Comissão por vender |
| `listing-prices-no-details.json` | o mesmo, sem `sale_fee_details` | Comissão por vender |
| `item-MLB4100000001.json`, `item-MLB4100000003.json` | `GET /items/{id}` antes de mudar o preço: sem variações e com três variações (uma criada depois do último Sync) | Variações (Modificar preço) |
| `error-400-price.json` | `PUT /items/{id}` com preço recusado (400, com `cause`) | Variações / erros |
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
- `listing-prices*.json`: `sale_fee_details` vem do exemplo para outros sites. A sugestão de
  preço lê `percentage_fee` e `fixed_fee` (sem eles, `sale_fee_amount` como parte do preço); a
  documentação recomenda mandar `logistic_type` e `shipping_mode` para a tarifa fixa sair
  exata, o que o app ainda não faz. Os valores de `fixed_fee` são inventados.
- `PUT /items/{id}`: a página de variações manda enviar o mesmo preço para todas as variações e
  avisa que uma variação omitida é apagada; por isso o app lê as variações logo antes de mudar o
  preço. A resposta de sucesso não é lida. O texto do erro 400 é inventado no formato de erro da
  API.
- `product-*.json`: `buy_box_winner` traz o preço do anúncio que vence a competição.
- `items-seller*.json`: a página de variações diz que o SKU do vendedor vem, nesta ordem, do
  atributo `SELLER_SKU` da variação, do `seller_custom_field` da variação, do `SELLER_SKU` do
  anúncio e do `seller_custom_field` do anúncio; os anúncios cobrem esses casos. O corpo do 404
  no multiget segue o formato de erro da API.
- `user-items-*.json`: a busca por `scan` devolve `scroll_id` até a última página, que vem sem
  resultados; o valor do `scroll_id` aqui é inventado.
