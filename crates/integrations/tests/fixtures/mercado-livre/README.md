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
| `domain-discovery.json` | `GET /sites/MLB/domain_discovery/search` (preditor de categorias) | Primeiros passos; Categorias e atributos |
| `category-attributes-MLB196208.json` | `GET /categories/{id}/attributes`, com as tags `required`, `catalog_required`, `conditional_required`, `new_required` e `read_only` | Atributos; Identificadores de produtos (GTIN) |
| `picture-upload.json` | `POST /pictures/items/upload` (multipart, campo `file`) | Trabalhar com imagens |
| `validate-errors.json` | `POST /items/validate` com um erro e um aviso (400); sem problemas a resposta é 204 sem corpo | Validador de publicações |
| `item-created.json` | `POST /items` (201) | Publicação de produtos |
| `item-description.json`, `error-400-description.json` | `POST /items/{id}/description` (201) e a descrição que já existe (400) | Descrição de produtos |
| `users-me-user-products.json` | `GET /users/me` de um vendedor com a tag `user_product_seller` | Preço por variação; User products |
| `error-400-stock.json` | `PUT /items/{id}` com `available_quantity` recusado (400, com `cause`) | Sincronização de publicações / erros |
| `user-items-by-sku.json`, `items-states.json` | `GET /users/{id}/items/search?seller_sku=...` e o multiget com `attributes=id,status,permalink` | Itens e buscas |
| `orders-search.json`, `orders-search-end.json`, `orders-search-empty.json` | `GET /orders/search?seller=...&order.date_last_updated.from=...&sort=date_asc` em duas páginas (`offset`), e uma busca sem resultados | Gerenciamento de vendas (Filtrar orders, Status da order); Gestão de packs |
| `shipment-44100000001.json` | `GET /shipments/{id}` com `x-format-new: true` | Gerenciamento de envios |

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
- Publicação (#16): o preditor e os atributos da categoria seguem os exemplos da documentação,
  com valores de uma categoria de fones inventados. Um vendedor com a tag `user_product_seller`
  manda `family_name` em vez de `title` (a página de preço por variação diz que o título é gerado
  pelo Mercado Livre). A garantia vai em `sale_terms` com `WARRANTY_TYPE` "Garantia do vendedor"
  e `WARRANTY_TIME` pelo nome do valor; conferir no regression pass se o Mercado Livre aceita só
  o nome ou exige o `value_id`. O texto dos erros de `validate-errors.json` e
  `error-400-description.json` é inventado no formato de erro da API; o `type` `warning` vem do
  exemplo de descrição. O app sobe as fotos antes de validar, porque o validador lê as fotos.
- Estoque, pausa e reativação (#18): `PUT /items/{id}` com `available_quantity` (anúncio sem
  variações) ou `variations[]` com o id de cada variação e `available_quantity` só nas que mudam
  (página Variações, "Exemplo de atualização correta de variante"); `{"status":"paused"}` e
  `{"status":"active"}` (Atualiza tuas publicações). A página de sincronização diz que estoque zero
  pausa o anúncio com `sub_status` `out_of_stock` e que repor reativa, exceto o pausado pelo
  vendedor. O app lê as variações com `GET /items/{id}` (os mesmos `item-*.json`) antes do envio.
  O texto de `error-400-stock.json` é inventado no formato de erro da API.
- Orders (#20): a busca devolve o recurso completo de cada Order em `results`, como
  `/orders/{id}`, com `paging.total`. Os pedidos cobrem um item simples, um carrinho (`pack_id`)
  com duas variações e um cancelado com pagamento recusado e `shipping.id` nulo (a página de
  packs diz que o envio pode não existir ainda). O app usa `last_updated` (ou
  `date_last_updated`) como hora da última mudança, `unit_price` (com desconto, como manda a
  página de nota fiscal), `sale_fee` por unidade e o `shipping_cost` dos pagamentos aprovados. O
  comprador traz só `id` e `nickname`; nome e endereço de quem recebe vêm de `destination` no
  envio, e o prazo de despacho de `lead_time.estimated_handling_limit.date` (a página de envios
  descreve os campos, mas não mostra o JSON inteiro: o formato foi completado e deve ser
  conferido no regression pass). O segundo envio do carrinho (`/shipments/44100000002`) não é
  servido, como um envio ainda não criado (404). Ids, nomes e endereços são inventados.
