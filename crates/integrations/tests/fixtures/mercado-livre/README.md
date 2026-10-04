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

Campos que a documentação não mostra por inteiro e que foram completados pelo formato dos
recursos vizinhos (conferir no regression pass):

- `product-items-*.json`: a página mostra o recurso, mas não um exemplo completo; os campos
  usados aqui (`paging.total`, `results[].item_id`, `price`, `currency_id`, `category_id`) são os
  mesmos de `/items`.
- `listing-prices.json`: `sale_fee_details` vem do exemplo para outros sites; o app lê só
  `sale_fee_amount` e `currency_id`.
- `product-*.json`: `buy_box_winner` traz o preço do anúncio que vence a competição.
