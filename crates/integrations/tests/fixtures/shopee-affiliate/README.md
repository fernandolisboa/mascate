# Respostas da Shopee Affiliate Open API (da documentação)

Estes arquivos **não** foram gravados com uma conta real: o acesso à Open API de afiliados da
Shopee ainda aguarda aprovação. A documentação oficial só abre dentro do painel de afiliado, então
os campos seguem a lista de `productOfferV2`, `shopOfferV2` e dos códigos de erro publicada a partir
dela no repositório público `bcat95/shopee-aff` e o exemplo de assinatura do mesmo repositório,
consultados em 05/10/2026, além de `docs/platform-integrations.md`. O regression pass da fase 1
(#37) os regrava quando a Shopee aprovar o acesso.

| Arquivo | Recurso | Origem |
|---|---|---|
| `product-offers.json` | `POST /graphql` com `productOfferV2` (`nodes` e `pageInfo.hasNextPage`): item simples, item com faixa de preço (variações), item sem `priceMin` e item com ids em texto e um link que só parece da Shopee | Get Product Offer List (v2) |
| `product-offers-end.json` | a página seguinte, com o mesmo item de novo e `hasNextPage: false` | Get Product Offer List (v2) |
| `shop-offers.json` | `POST /graphql` com `shopOfferV2`: loja com `originalLink` e nota, e loja sem nota com link em `http` (o app só abre `https` da Shopee e usa a página da loja) | Get Shop Offer List (v2) |
| `error-10020.json` | erro de identidade (`Invalid Signature`) | Lista de códigos de erro |
| `error-10030.json` | limite de requisições (`Rate limit exceeded`) | Lista de códigos de erro |
| `error-10035.json` | conta sem acesso à Open API | Lista de códigos de erro |

Como o app chama a API:

- Toda requisição é `POST` com corpo `{"query": "..."}` e o cabeçalho
  `Authorization: SHA256 Credential={AppID}, Timestamp={segundos Unix}, Signature={hex}`, em que a
  assinatura é o SHA256 de `AppID + Timestamp + corpo exato enviado + Secret`. O teste da assinatura
  usa um vetor calculado à parte com o `hashlib` do Python.
- Os argumentos vão escritos dentro da consulta, como nos exemplos da documentação; o texto da busca
  é escapado como string JSON, que vale como string GraphQL.
- `sortType` de `productOfferV2`: 1 relevância, 2 mais vendidos, 4 menor preço, 5 maior comissão;
  `shopOfferV2` usa 3 (lojas populares). Página de 20 itens.
- Preços vêm como texto em reais (`"29.90"`), comissão como fração (`"0.1"` = 10%) e nota como
  texto de 0 a 5. Um item sem `priceMin` legível fica de fora. `itemId` e `shopId` são Int64 e o
  app aceita número ou texto.
- Erros vêm com HTTP 200 em `errors[].extensions.code`: 10030 e 10000 são tentados de novo depois
  de 2, 4 e 8 segundos (a Shopee não publica o limite), assim como HTTP 429 e 5xx; 10020 conta como
  chaves não aceitas; 10031 a 10035 e 11000 a 11999 são recusas com o texto da Shopee.

Inventados ou completados (conferir no regression pass):

- O texto de `message` dos erros, que a documentação só dá em `extensions.message`.
- O link da loja num item (`https://shopee.com.br/shop/{shopId}`): `productOfferV2` não traz o link
  da loja. O link do item é `productLink`; o `offerLink` (link de afiliado) não é usado.
- Ids, nomes, preços e vendas.
- Se a API do Brasil está em `https://open-api.affiliate.shopee.com.br/graphql`, se aceita
  `sortType: 3` em `shopOfferV2`, se `pageInfo` traz `hasNextPage` nas duas consultas e se a
  tolerância do `Timestamp` é de alguns minutos.
