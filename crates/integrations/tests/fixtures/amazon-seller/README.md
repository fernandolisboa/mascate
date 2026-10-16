# Respostas da Selling Partner API da Amazon (da documentação)

Estes arquivos **não** foram gravados com uma conta real: seguem os exemplos e os esquemas da
documentação oficial da Selling Partner API (`developer-docs.amazon/sp-api`), consultada em
16/10/2026 (via Context7, `/websites/developer-docs_amazon_sp-api`, e as páginas de referência de
cada operação). Os valores (SKUs, ASINs, preços, tarifas e o id do vendedor) foram inventados para
a loja amazon.com.br (`A2Q3Y263D00KWC`, moeda BRL). O regression pass da fase 3 (#116) os regrava
com a conta de vendedor do Fernando.

| Arquivo | Recurso | Página da documentação |
|---|---|---|
| `token.json` | `POST https://api.amazon.com/auth/o2/token` com `grant_type=refresh_token` (1 hora) | Connecting to the Selling Partner API (Step 1: Request a Login with Amazon access token) |
| `token-invalid-client.json` | o mesmo, com o client secret vencido ou trocado (401 `invalid_client`) | o mesmo |
| `listings-page-1.json`, `listings-page-2.json` | `GET /listings/2021-08-01/items/{sellerId}` (`searchListingsItems`) com `includedData=summaries`, por `createdDate` crescente, 20 por página: a primeira com `pagination.nextToken`, a última sem | searchListingsItems; Search for listings items by ID |
| `listings-by-sku.json` | o mesmo com `identifiers` e `identifiersType=SKU` e `includedData=summaries,offers,fulfillmentAvailability,relationships`: um à venda, um com quantidade 0, um SKU pai de variação (com `childSkus`, sem oferta), um filho (com `parentSkus`, ofertas B2C e B2B) e um enviado pela Amazon (`AMAZON_NA`) | searchListingsItems; Building listings management workflows |
| `listings-page-1-caneca-gone.json`, `listings-by-sku-caneca-gone.json` | os mesmos, depois que a caneca foi apagada no Seller Central | — |
| `patch-accepted.json` | `PATCH /listings/2021-08-01/items/{sellerId}/{sku}` (`patchListingsItem`) aceito (`ACCEPTED`) | patchListingsItem; Merge a listing; Manage purchasable offer |
| `patch-invalid.json` | o mesmo, inválido (`INVALID`), com um problema `ERROR` e outro `WARNING` | Manage listings issues |
| `product-type-HEADPHONES.json`, `product-type-SHIRT.json` | `GET /definitions/2020-09-01/productTypes/{productType}` com `requirements=LISTING_OFFER_ONLY` e `locale=pt_BR` (`displayName`) | getDefinitionsProductType |
| `fees-estimate.json` | `POST /products/fees/v0/listings/{SellerSKU}/feesEstimate` (`getMyFeesEstimateForSKU`) com `Status: Success` e `FeeDetailList` | getMyFeesEstimateForSKU |
| `fees-estimate-client-error.json` | o mesmo com `Status: ClientError` | getMyFeesEstimateForSKU |
| `orders-page-1.json`, `orders-page-2.json` | `GET /orders/2026-01-01/orders` (`searchOrders`) com `fulfilledBy=MERCHANT`, `lastUpdatedAfter` e `includedData=RECIPIENT,PROCEEDS,FULFILLMENT,PACKAGES`, 100 por página: a primeira com `pagination.nextToken`; a segunda repete um pedido da primeira, como quando ele muda entre as páginas | searchOrders; Orders API v2026-01-01 migration guide |
| `orders-without-recipient.json` | o mesmo sem `RECIPIENT`, quando falta o papel Direct-to-Consumer Shipping: só cidade e estado | searchOrders; Roles in the Selling Partner API |
| `orders-empty.json` | o mesmo sem pedidos | searchOrders |
| `order-701.json`, `order-704.json` | `GET /orders/2026-01-01/orders/{orderId}` (`getOrder`) com `includedData=FULFILLMENT,PACKAGES`: um a enviar e um já entregue | getOrder |
| `catalog-one.json`, `catalog-several.json`, `catalog-empty.json` | `GET /catalog/2022-04-01/items` (`searchCatalogItems`) com `identifiers` (o GTIN), `identifiersType` (`EAN`, `UPC` ou `GTIN`), `includedData=summaries,productTypes` e `locale=pt_BR`: um produto, três (um sem tipo de produto) e nenhum | searchCatalogItems; Catalog Items API v2022-04-01 reference |
| `put-preview-valid.json` | `PUT /listings/2021-08-01/items/{sellerId}/{sku}` (`putListingsItem`) com `requirements=LISTING_OFFER_ONLY` e `mode=VALIDATION_PREVIEW`: válido (`VALID`), com um aviso | putListingsItem; Preview errors before creating a listing |
| `put-accepted.json`, `put-invalid.json` | o mesmo sem `mode`: aceito (`ACCEPTED`) e inválido (`INVALID`), com um problema `ERROR` e outro `WARNING` | putListingsItem; Submit listings data (offer-only) |
| `listing-item.json` | `GET /listings/2021-08-01/items/{sellerId}/{sku}` (`getListingsItem`) com `includedData=summaries,fulfillmentAvailability`, a oferta criada | getListingsItem |
| `error-400-page-token.json` | `searchOrders` com o `paginationToken` vencido (vale 24 horas) | searchOrders |
| `transactions-701-page-1.json`, `transactions-701-page-2.json` | `GET /finances/2024-06-19/transactions` (`listTransactions`) com `relatedIdentifierName=ORDER_ID`: a primeira com `nextToken` e `breakdowns` em lista, a segunda com `breakdowns` dentro de um objeto, como em alguns exemplos | listTransactions; Finances API v2024-06-19 |
| `transactions-empty.json` | o mesmo antes de a Amazon lançar as transações do pedido (até 48 horas) | listTransactions |
| `error-400.json`, `error-403.json`, `error-404.json`, `error-429.json` | o formato de erro `{"errors":[{"code","message","details"}]}`: entrada inválida, token vencido, recurso inexistente e cota excedida | Error responses and schemas; Usage plans and rate limits |
| `featured-offer-expected-price.json` | `POST /batches/products/pricing/2022-05-01/offer/featuredOfferExpectedPrice` (`getFeaturedOfferExpectedPriceBatch`, Automatic Repricing, #135): um preço esperado válido (`VALID_FOEP`), sem concorrência (`NO_COMPETING_OFFERS`, no plural, como no exemplo da página), uma oferta que não pode ficar em destaque (`OFFER_NOT_ELIGIBLE`) e um SKU que a Amazon não conhece (400 `INVALID_SKU` dentro da resposta 200) | getFeaturedOfferExpectedPriceBatch (consultada em 16/10/2026) |
| `listing-issues.json`, `listing-issues-none.json` | `GET /listings/2021-08-01/items/{sellerId}/{sku}` com `includedData=issues` e `issueLocale=pt_BR` (Listing Quality, #136): um aviso em dois atributos, um erro que tira a oferta da busca (`SEARCH_SUPPRESSED`), um aviso com isenção até uma data e um erro de outra loja (amazon.com, deixado de fora); e o mesmo item sem problemas | getListingsItem; Listings Items issues (consultadas em 16/10/2026) |

Inventados ou completados (conferir no regression pass):

- Os nomes dos tipos de produto em português (`Fones de ouvido`, `Camisa`); a documentação só traz
  o exemplo em inglês.
- Se o preço da oferta vem como `currencyCode` (o esquema) ou `currency` (um exemplo do guia); o
  adapter aceita os dois, e o valor como texto ou número.
- A `PerItemFee` de R$ 2,00 em `fees-estimate.json`, que existe para testar a parte fixa da tarifa:
  o plano Profissional não cobra por item.
- A mensagem em português do problema `INVALID` e o texto dos erros.
- Se um item com quantidade 0 continua com `DISCOVERABLE` e sem `BUYABLE`, como aqui.
- Os ids dos pedidos (`701-…` a `705-…`), os SKUs, os valores e o nome do comprador.
- Se um item cancelado vem com `quantityOrdered` 0, como o pedido 703; o adapter ignora a linha e
  mantém a quantidade guardada para devolver as unidades.
- Se o `DISCOUNT` de um item vem negativo ou positivo; o adapter desconta o valor absoluto.
- Os tipos dos `breakdowns` em `Expenses` (`Commission`, `ShippingChargeback`) e se um estorno de
  tarifa vem positivo dentro de `Expenses`, como na segunda página.
- O texto do erro do `paginationToken` vencido.
- Os ASINs, marcas e classificações das buscas no catálogo, a mensagem em português de cada
  problema do `putListingsItem` (pedida com `issueLocale=pt_BR`) e o código do problema de marca
  que pede aprovação.
- Se um produto do catálogo pode vir sem tipo de produto, como o terceiro de
  `catalog-several.json`; o adapter o deixa de fora, porque a oferta precisa do tipo.
- Se, sem o papel Direct-to-Consumer Shipping, a Amazon recusa a busca com `RECIPIENT` (403, como
  aqui) ou só omite o endereço.
- Reprecificação (#135): se `resultStatus` sem concorrência vem no singular (a lista do esquema) ou
  no plural (o exemplo da página); o adapter aceita os dois. Se o limite de uma chamada a cada 30
  segundos vale para a conta do Fernando (o cabeçalho `x-amzn-RateLimit-Limit`), e se o preço
  esperado de uma oferta em destaque já leva em conta o frete do vendedor.
- Qualidade (#136): se os textos de `issues` chegam em português com `issueLocale=pt_BR`, e se o
  link de edição `sellercentral.amazon.com.br/abis/listing/edit?sku=...` abre o anúncio certo.
