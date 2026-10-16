# Páginas de busca do Mercado Livre (escritas à mão)

Estes arquivos **não** foram gravados do site: gravá-los seria rodar o crawler do Mercado Livre,
que os Termos de desenvolvedor (7.6) proíbem e que fica desligado atrás da flag
`integrations.mercado_livre_crawler` (ADR 0035). Foram escritos à mão, em 16/10/2026, a partir da
estrutura pública conhecida das páginas de busca (`lista.mercadolivre.com.br/{termo}`): o estado
pré-carregado da busca (`__PRELOADED_STATE__`, com os resultados em cartões `polycard`) e os blocos
JSON-LD de schema.org. Quem ligar a flag regrava estes arquivos pelo regression pass da fase 3
(#116) e corrige o leitor onde a página de verdade for diferente.

| Arquivo | O que imita |
|---|---|
| `robots.txt` | um robots.txt de busca que deixa ler tudo menos `/jms/` e `*_NoIndex_True`; não é a cópia do robots.txt do Mercado Livre |
| `search.html` | a primeira página de `fone bluetooth`, com o estado num `<script id="__PRELOADED_STATE__" type="application/json">`: cartão com vendedor, nota e vendas (`+10mil`), cartão de loja oficial com ícone no texto do vendedor e só `+500 vendidos`, cartão sem preço (fica de fora), cartão com link fora do Mercado Livre (fica de fora), o primeiro cartão repetido como anúncio (sai uma vez), cartão sem vendedor nem foto com preço em texto e sem moeda; `next_page.show: true`; e um JSON-LD de BreadcrumbList que não é produto |
| `search-page-2.html` | a página seguinte, com o estado atribuído em `window.__PRELOADED_STATE__ = {...};`: o primeiro item de novo e um item com chaves e aspas no título e `4,5 \| +1,5mil vendidos`; `next_page.show: false` |
| `search-json-ld.html` | uma busca sem estado pré-carregado, só com JSON-LD: `ItemList` com um Product com `Offer`, vendedor e nota, um Product de catálogo (`/p/MLB...`) com `AggregateOffer` e foto como `ImageObject`, e um Product numa moeda que o app não conhece (fica de fora); `rel="next"` no corpo |
| `verification.html` | uma página de verificação de robô com reCAPTCHA, respondida com 200 |

Como o crawler lê a busca:

- O endereço é `{busca}/{palavras-com-traços}`, as palavras sem acento e em minúsculas; a página N
  acrescenta `_Desde_{(N-1)×48+1}` e a ordem por menor preço `_OrderId_PRICE`. Sem categoria, sem
  loja e sem as ordens por vendas e por comissão, que a página não tem.
- Cada cartão vira uma oferta com o id `MLB` + dígitos de `metadata.id`, o link de `metadata.url`
  (com `https://` na frente e sem o que vem depois de `#`, só em `mercadolivre.com.br`), o título
  do componente `title`, o preço de `price.current_price`, o vendedor do componente `seller` (o
  texto com `{seller}` preenchido pelo valor de mesma chave e os ícones tirados), a nota e as vendas
  do componente `review_compacted` e a foto `https://http2.mlstatic.com/D_NQ_NP_{id}-O.jpg` da
  primeira de `pictures`. Sem vendedor, a oferta fica com o fornecedor "Mercado Livre".
- Sem cartões, lê os Products dos blocos JSON-LD: `name`, `url`, `image` (texto, lista ou
  `ImageObject`, só de `mlstatic.com`), `offers.price` ou `lowPrice`/`highPrice`, `priceCurrency`,
  `offers.seller.name` e `aggregateRating.ratingValue`.

Inventados ou completados (conferir no regression pass):

- Os nomes e o formato dos campos do estado pré-carregado (`polycard`, `metadata`, `components`,
  `current_price`, `review_compacted`, `next_page.show`), se ele vem num script com id ou atribuído
  numa variável, e se a busca traz JSON-LD de produtos.
- O endereço das fotos a partir do id da foto, o tamanho da página (48) e o formato de
  `_Desde_` e `_OrderId_PRICE`.
- O texto das vendas (`+10mil vendidos`, `+500 vendidos`) e da nota.
- Ids, nomes, preços, vendas e fotos.
