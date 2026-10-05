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
| `orders-search-returned.json`, `shipment-44100000005.json` | a busca de Orders com um pedido entregue que tem duas reclamações em `mediations`, e o seu envio entregue | Gerenciamento de vendas; Gerenciamento de envios |
| `claim-5298178312-returns.json` | `GET /post-purchase/v2/claims/{id}/returns` de uma devolução parcial a caminho do vendedor | Gerenciar devoluções |
| `shipment-44100000001-costs.json` | `GET /shipments/{id}/costs` (o que o comprador e o vendedor pagam do envio) | Gerenciamento de envios; Custos e cotações |
| `billing-order-details.json` | `GET /billing/integration/group/ML/order/details?order_ids=...` com dois Orders faturados (o terceiro pedido ainda não foi faturado e fica de fora) | Provisões (Relatórios de faturamento por Orders e Packs); Boas práticas dos relatórios de faturamento |
| `shipment-label-44100000001.pdf` | `GET /shipment_labels?shipment_ids=...&response_type=pdf` | Mercado Envios 2 (Imprimir etiquetas) |
| `item-MLB4100000001-performance.json`, `error-404-performance.json` | `GET /item/{id}/performance` com regras pendentes e concluídas, e o item ainda sem qualidade calculada (404) | Qualidade das publicações |
| `item-MLB4100000001-visits.json` | `GET /items/{id}/visits/time_window?last=30&unit=day` | Visitas |
| `questions-search-unanswered.json`, `questions-search-unanswered-end.json`, `questions-search-empty.json` | `GET /questions/search?seller_id=...&status=UNANSWERED&api_version=4&sort_fields=date_created&sort_types=ASC` em duas páginas (`offset`), e uma busca sem perguntas | Gerenciamento de perguntas e respostas |
| `question-13001000001.json`, `question-13001000004-banned.json`, `error-404-question.json` | `GET /questions/{id}?api_version=4` de uma pergunta respondida, de uma removida (`BANNED`, texto vazio) e de uma que não existe mais (404) | Gerenciamento de perguntas e respostas |
| `answer-posted.json`, `error-400-answer.json` | `POST /answers` com `question_id` e `text` (200) e o corpo recusado (400, `invalid_post_body`) | Gerenciamento de perguntas e respostas (Responder; Referência de códigos de erro) |
| `users-me-reputation.json` | `GET /users/me` de um vendedor protegido, com `seller_reputation` (`level_id`, `real_level`, `protection_end_date`, `transactions`, `metrics` com `excluded`) | Reputação de vendedores; Recuperação de reputação |
| `reviews-MLB4100000001.json`, `reviews-MLB4100000001-end.json`, `reviews-MLB4100000003.json`, `error-404-reviews.json` | `GET /reviews/item/{id}?limit=50&offset=...` em duas páginas com avaliações baixas, uma página só com notas altas, e um anúncio sem avaliações (404) | Opiniões de produtos |
| `seller-promotions-user.json`, `seller-promotions-user-end.json` | `GET /seller-promotions/users/{id}?app_version=v2` em duas páginas: campanha do vendedor, cupom sem os termos, convite do Mercado Livre, campanha encerrada e cupom percentual com os termos | Gerenciar ofertas; Campanhas do vendedor; Cupons do vendedor |
| `seller-promotion-C-MLB1081.json` | `GET /seller-promotions/promotions/{id}?promotion_type=SELLER_COUPON_CAMPAIGN&app_version=v2` (os termos do cupom) | Cupons do vendedor |
| `seller-promotions-item-MLB4100000001.json`, `error-404-seller-promotions.json` | `GET /seller-promotions/items/{id}?app_version=v2` com desconto individual, campanha, cupom, convite (`candidate`) e oferta relâmpago, e um anúncio sem promoções (404) | Gerenciar ofertas |
| `seller-promotion-created.json` | `POST /seller-promotions/promotions?app_version=v2` (e a resposta do `PUT` da mesma campanha) | Campanhas do vendedor |
| `seller-promotion-item-joined.json`, `error-400-promotion.json` | `POST /seller-promotions/items/{id}?app_version=v2` e o preço recusado (400, `ERROR_CREDIBILITY_DISCOUNTED_PRICE`) | Desconto individual; Campanhas tradicionais |

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
- Etiquetas, atrasos, cancelamentos e devoluções (#21): a etiqueta vem de `GET /shipment_labels`
  com `response_type=pdf` (a página também oferece `zpl2`, que o app não usa), só para envios
  `ready_to_ship` com `substatus` `ready_to_print` ou `printed`; o PDF aqui é um arquivo mínimo
  feito à mão, não uma etiqueta real. As devoluções vêm das reclamações que o pedido lista em
  `mediations` (campo do recurso de Order que a página de vendas não mostra no exemplo: conferir
  no regression pass), cada uma lida em `/post-purchase/v2/claims/{id}/returns`; uma reclamação
  sem devolução responde 404. O app lê `status` e os itens de `orders[]` do próprio pedido, com
  `return_quantity` escrito como texto ("2.0"), como no exemplo da página. A lista completa de
  status da devolução não aparece na documentação consultada: o app trata `delivered` como
  entregue, `cancelled` e `expired` como desistência e qualquer outro como a caminho; conferir no
  regression pass. Também fica para o regression pass se o `last_updated` do Order muda quando a
  reclamação ou a devolução muda (a busca de Orders depende disso).
- Tarifas e margem realizada (#22): as cobranças de cada Order vêm do faturamento por Order
  (`details[]` com `charge_info.detail_id`, `detail_amount`, `detail_type` `CHARGE` ou `BONUS` e
  `detail_sub_type`, mais `marketplace_info` e `currency_info`), no formato do exemplo da página
  Provisões; os valores, ids e o texto de `transaction_detail` são inventados. A página lista `CV`,
  `BV`, `CXD` e `BXD` nos exemplos de filtro, mas não a lista inteira de subtipos: o app trata `CV`
  e `BV` como tarifa de venda, o marketplace `SHIPPING` e os subtipos `CX…`/`BX…` como frete e o
  resto como outra tarifa, e uma bonificação como cobrança devolvida (valor negativo); conferir no
  regression pass. A resposta 206 (dados incompletos) vem do guia de boas práticas. Os pedidos
  ganharam `transaction_amount_refunded` nos pagamentos (campo do exemplo da página de vendas); o
  pedido com devolução de duas unidades traz 99,8 reembolsado. O custo do envio para o vendedor vem
  de `senders[].cost` de `/shipments/{id}/costs`, no formato do exemplo da página de envios; um
  envio sem custos responde 404 (o envio do pedido devolvido não tem o arquivo servido).
- Qualidade do anúncio (#23): `item-*-performance.json` é o exemplo da página (um item `MLA` de
  nível "Profesional") traduzido para o pt-BR, com o id e os links trocados para um anúncio
  `MLB`: o texto das regras (`wordings`) do MLB não aparece na documentação e foi escrito no
  mesmo sentido do original. O app mostra `wordings.title`, `label` e `link` de cada regra
  `PENDING`, trata `mode` `WARNING` como problema que derruba a nota e `OPPORTUNITY` como
  sugestão, e só abre links `https` do Mercado Livre. A página mostra `level` "Good" com
  `level_wording` por site (Básica, Satisfatória, Profissional no MLB); os outros valores de
  `level` não aparecem, então o app lê o nível pelo nome e, sem um nome conhecido, pela faixa de
  pontuação de `/sites/MLB/health_levels` (abaixo de 50 básica, de 66 profissional). O corpo do
  404 segue o formato de erro da API com a mensagem da tabela de erros da página. Conferir no
  regression pass: os textos e os valores de `level` reais, se o 401 "Caller must be the seller
  of the item" acontece com anúncios próprios, e se a janela de visitas sem `ending` termina hoje.
  `item-*-visits.json` segue o exemplo da página de Visitas, sem o detalhe por dia, que o app não
  lê.
- Perguntas (#24): a busca segue o exemplo de `/my/received_questions/search` da documentação
  (`total`, `limit`, `questions[]` com `id`, `item_id`, `seller_id`, `status`, `text`,
  `date_created`, `answer` com `text`, `status` e `date_created`, e `from.id`), que a página de
  perguntas descreve como o mesmo formato de `/questions/search` com `api_version=4`. Os valores de
  `available_filters` (os status `ANSWERED`, `BANNED`, `CLOSED_UNANSWERED`, `DELETED`, `DISABLED`,
  `UNANSWERED` e `UNDER_REVIEW`) vêm da mesma página; os filtros `status`, `sort_fields` e
  `sort_types` estão descritos, mas a página não mostra uma busca com todos juntos. A primeira
  página foi cortada em duas perguntas para exercitar a paginação (o app soma o que veio, não o
  `limit`). A página diz que perguntas e respostas `BANNED` voltam com texto vazio e que o limite
  de uma resposta é 2.000 caracteres. O corpo de sucesso de `POST /answers` não é lido (o próximo
  Sync lê a pergunta); o exemplo segue o recurso de pergunta. O texto de `error-400-answer.json` é
  inventado com o código `invalid_post_body` da tabela de erros, e o 404 segue o formato de erro da
  API. Ids, textos e compradores são inventados. Conferir no regression pass: a busca com `status`
  e ordenação juntos, o erro de responder uma pergunta já respondida, se a moderação aceita links
  do Mercado Livre na resposta e se `UNDER_REVIEW` volta a `UNANSWERED`.
- Reputação e avaliações (#25): `users-me-reputation.json` segue o exemplo de `seller_reputation` da
  página de reputação (o mesmo recurso de `GET /users/{id}`, lido em `/users/me`), com os números de
  um vendedor novo e protegido: a página diz que, durante a proteção, as taxas contadas ficam em
  zero e as reais vêm em `excluded` (`real_rate` como fração, `real_value` em vendas), que são as
  que o app guarda. `level_id` vem como `5_green` e `real_level` como `yellow`; o app lê as cores
  com ou sem o número. Os limites de cada cor (reclamações 2%, cancelamentos 1,5% e atrasos 10% para
  a verde; 4,5%, 3,5% e 18% para a amarela; 8%, 4% e 22% para a laranja) vêm da tabela "Limites para
  cada variável" do MLB, que não tem coluna para a verde-clara. As páginas de avaliações seguem o
  exemplo de `/reviews/item/{id}` (`reviews[]` com `id`, `date_created`, `status`, `title`,
  `content`, `rate`, `rating_average` e `rating_levels` de `one_star` a `five_star`); o exemplo
  traz `paging` vazio, e o app lê `paging.total` como nas outras buscas. O app só pagina enquanto
  faltam avaliações de 3 estrelas ou menos que `rating_levels` contou. O corpo do 404 é inventado
  no formato de erro da API. Textos, ids e datas são inventados. Conferir no regression pass: os
  limites reais de cada cor, o requisito de Product Ads (amarela e vendas mínimas), se Promoções
  exige a verde ou aceita a verde-clara, se anúncios de catálogo precisam de `catalog_product_id`
  para trazer as avaliações e se um anúncio sem avaliações responde 404 ou uma página vazia.
- Promoções do vendedor (#26): os recursos seguem as páginas de desconto individual, campanhas do
  vendedor, cupons do vendedor e gerenciar ofertas, todas com `app_version=v2`. Criar campanha manda
  `promotion_type`, `name`, `sub_type` (`FLEXIBLE_PERCENTAGE`) e os dias como `AAAA-MM-DDT00:00:00`
  (a página diz que o Mercado Livre conta o primeiro dia desde a meia-noite e o último até
  23:59:59, como mostra a resposta do `PUT`); cupom manda `FIXED_AMOUNT` com `fixed_amount` ou
  `FIXED_PERCENTAGE` com `fixed_percentage` e `max_purchase_amount`, mais `min_purchase_amount` e
  `budget`, sem código (visível a todos). Entrar numa promoção é `POST /seller-promotions/items/{id}`
  com `deal_price` e os dias (desconto individual), `promotion_id` e `deal_price` (campanha) ou só
  `promotion_id` (cupom); sair é `DELETE` com `promotion_type` e, numa campanha ou cupom,
  `promotion_id`; encerrar é `DELETE /seller-promotions/promotions/{id}` com `promotion_type`. As
  listas trazem datas com fuso (`...T03:00:00Z`) e as respostas de criar e mudar trazem a data local
  (`...T00:00:00`); o app lê as duas no horário de Brasília. A página mostra a paginação da lista do
  vendedor (`paging.offset`, `limit`, `total`), mas não uma segunda página: `offset` e `limit` na
  segunda são do formato das outras buscas. A página de gerenciar ofertas não mostra `id`,
  `start_date` e `finish_date` no exemplo do item; foram completados pelos campos da lista do
  vendedor. Os termos de um cupom vêm na lista quando o Mercado Livre os manda, ou do recurso do
  cupom. A API não manda moeda: o app usa reais (MLB). Erros de promoções trazem `error_message`
  em `cause`. Ids, nomes, preços e datas são inventados. Conferir no regression pass: se Promoções
  aceita a verde-clara, se a lista do vendedor traz campanhas e cupons próprios (ou só convites), o
  formato real das datas e da paginação, se o prazo de 14 e 31 dias conta o primeiro e o último
  dia, se o Mercado Livre cobra a tarifa sobre o preço antes ou depois do cupom, e os campos
  `min_discounted_price`/`suggested_discounted_price` que ele devolve para os convites.
