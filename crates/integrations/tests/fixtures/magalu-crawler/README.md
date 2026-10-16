# Páginas de busca da Magalu (escritas à mão)

Estes arquivos **não** foram gravados do site: gravá-los seria rodar o crawler da Magalu, que fica
desligado atrás da flag `integrations.magalu_crawler` (ADR 0035). Os termos de uso da Magalu não
foram lidos porque o site recusa acesso automatizado até ao robots.txt, o que conta como proibição
(ADR 0006). Foram escritos à mão, em 16/10/2026, a partir da estrutura pública conhecida da busca
(`www.magazineluiza.com.br/busca/{termo}/`): o estado da página num
`<script id="__NEXT_DATA__" type="application/json">` e os blocos JSON-LD de schema.org. Quem ligar
a flag regrava estes arquivos pelo regression pass da fase 3 (#116) e corrige o leitor onde a
página de verdade for diferente.

| Arquivo | O que imita |
|---|---|
| `robots.txt` | um robots.txt que deixa ler `/busca/` e fecha o checkout, a área do cliente e buscas com filtros; não é a cópia do robots.txt da Magalu |
| `search.html` | a primeira página de `fone bluetooth`, com `products` no estado: produto da Magalu com `bestPrice`, foto com `{w}x{h}`, nota e vendedor; produto de lojista com só `price`, nota zero e caminho com `/` na frente; produto sem preço, com foto fora da Magalu e lojista só com id (entra sem preço e sem foto); produto com link para outro site (fica de fora); produto sem vendedor (fica com a Magazine Luiza); o primeiro produto repetido em `ads` (anúncio, não entra); filtros com id e título que não são produtos; `pagination` com página 1 de 60 em 143 |
| `search-page-3.html` | a página 3 da mesma busca, com `pagination` na última página |
| `search-json-ld.html` | uma busca sem estado, só com JSON-LD: `ItemList` com dois Products (foto em texto e como `ImageObject`, vendedor em `offers.seller.name`, nota num deles), um `BreadcrumbList` que não é produto e `rel="next"` |
| `access-denied.html` | a página "Access Denied" que a Magalu responde com 403 a um pedido automatizado |
| `verification.html` | uma verificação de robô com hCaptcha, respondida com 200 |

Como o crawler lê a busca:

- O endereço é `{site}/busca/{palavras+com+mais}/`, as palavras sem acento e em minúsculas; a
  página N acrescenta `page=N`, a ordem por menor preço `sortOrientation=asc&sortType=price` e a por
  mais vendidos `sortOrientation=desc&sortType=soldQuantity`. Sem categoria, sem loja e sem a ordem
  por comissão.
- Cada item de `products` vira uma oferta com o id da Magalu (`id`, letras e dígitos), o link
  `https://www.magazineluiza.com.br/{path}` (só em `magazineluiza.com.br`), o título, o preço de
  `price.bestPrice` ou `price.price`, a foto `image` com `{w}x{h}` trocado por `800x560` (só em
  `mlcdn.com.br`), a nota de `rating.score` (zero é sem nota) e o lojista de `seller.id` e
  `seller.description`. Sem vendedor, o fornecedor é a Magazine Luiza. Item sem preço entra sem
  preço e não pode ser guardado nas ofertas.
- Há próxima página quando `pagination.page × pagination.size < pagination.records`, ou com
  `rel="next"`.
- Sem `products`, lê os Products dos blocos JSON-LD: o id vem do trecho depois de `/p/` no `url`.

Inventados ou completados (conferir no regression pass):

- Se a busca é mesmo uma página Next.js com `__NEXT_DATA__`, e os nomes e o formato dos campos
  (`products`, `ads`, `path`, `price.bestPrice`, `rating.score`, `seller.description`,
  `pagination.records`) e do `{w}x{h}` das fotos.
- Os parâmetros `page`, `sortOrientation` e `sortType`, e se a busca traz JSON-LD.
- A resposta da Magalu a um pedido automatizado (403 "Access Denied" ou outra verificação).
- Ids, nomes, preços, notas e fotos.
