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
| `error-400.json`, `error-403.json`, `error-404.json`, `error-429.json` | o formato de erro `{"errors":[{"code","message","details"}]}`: entrada inválida, token vencido, recurso inexistente e cota excedida | Error responses and schemas; Usage plans and rate limits |

Inventados ou completados (conferir no regression pass):

- Os nomes dos tipos de produto em português (`Fones de ouvido`, `Camisa`); a documentação só traz
  o exemplo em inglês.
- Se o preço da oferta vem como `currencyCode` (o esquema) ou `currency` (um exemplo do guia); o
  adapter aceita os dois, e o valor como texto ou número.
- A `PerItemFee` de R$ 2,00 em `fees-estimate.json`, que existe para testar a parte fixa da tarifa:
  o plano Profissional não cobra por item.
- A mensagem em português do problema `INVALID` e o texto dos erros.
- Se um item com quantidade 0 continua com `DISCOVERABLE` e sem `BUYABLE`, como aqui.
