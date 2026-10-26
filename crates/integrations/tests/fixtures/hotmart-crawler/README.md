# Páginas da vitrine da Hotmart (escritas à mão)

Estes arquivos **não** foram gravados do site: gravá-los seria rodar o crawler da Hotmart, que fica
desligado atrás da flag `integrations.hotmart_crawler` (ADR 0035), porque os termos de uso da Hotmart,
item 3.5(m), proíbem sistemas automatizados, robôs e scripts. Foram escritos à mão em 16/10/2026,
seguindo a estrutura que a vitrine pública mostrou a alguns pedidos HTTP simples feitos à mão nesse
dia (o `robots.txt`, a página inicial `hotmart.com/pt-br/marketplace`, duas páginas de categoria, uma
de subcategoria e uma de produto): o estado da página num
`<script id="__NEXT_DATA__" type="application/json" nonce="...">` do Next.js. Ids, nomes, fotos,
notas e preços são inventados. Quem ligar a flag confere estes arquivos pelo regression pass da fase
3 (#116) e corrige o leitor onde a página de verdade tiver mudado.

| Arquivo | O que imita |
|---|---|
| `robots.txt` | o robots.txt da Hotmart no que importa ao crawler: tudo liberado, menos endereços com `q=` (a busca por palavras) e com `initialSelectedCategory=` |
| `best-sellers.html` | a página inicial da vitrine, com `bestSellers` no estado: produto com foto no S3 da Hotmart e produtor sem página própria; produto com foto fora da Hotmart (vale a do `static-media.hotmart.com`) e produtor com `creatorPage`; um produto em `bannerSeason.products` (banner de campanha, não entra) e as `categories` internas, que não são produtos |
| `category.html` | a página de Culinária e gastronomia, com `allProducts.requestData.results`: produto completo com `price` vazio, como a Hotmart manda; produto com `showRating: false` (entra sem nota); produto com `price` em número e `creatorPage` fora da Hotmart (o link do produtor é montado); produto com `salesEnabled: false` (fica de fora); produto com `slug` que é um link (fica de fora); produto sem `owner`, com nota zero e sem foto; o primeiro produto repetido; e o bloco JSON-LD de breadcrumb |
| `subcategory.html` | a subcategoria Confeitaria, com um produto de id e nota em texto e só `authorName` |
| `forbidden.html` | uma resposta 403 genérica, para o disjuntor do motor |

Como o crawler lê a vitrine:

- Sem categoria, lê os mais vendidos da página inicial (`{vitrine}`); com categoria, a página
  `{vitrine}/categorias/{categoria}` ou `{vitrine}/categorias/{categoria}/{subcategoria}`. A categoria
  vem como slug ou como o link da página da categoria na Hotmart.
- A busca por palavras é recusada sem pedido nenhum: o robots.txt da Hotmart proíbe `q=`. Não há
  busca de lojas, ordem além da do site nem página seguinte: o estado traz só os 24 primeiros, e os
  próximos vêm de uma busca que o crawler não faz.
- Cada item de `bestSellers` ou `results` vira uma oferta com o id da Hotmart (`productId`), o título,
  o link `https://hotmart.com/pt-br/marketplace/produtos/{slug}/{producerReferenceCode}`, a foto
  (`avatar` ou `finalAvatar`, só em `hotmart.com` ou `hotmart.s3.amazonaws.com`), a nota de `rating`
  quando `showRating` não é `false` (zero é sem nota) e o produtor como loja: id `owner.publicId`
  (ou `owner.id`, ou o nome), nome `owner.name`, `ownerName` ou `authorName`, link `creatorPage` só em
  `hotmart.com`, senão `/especialista/{owner.slug}/{owner.publicId}`.
- A vitrine não traz preço: o campo `price` vem vazio e a página pede o preço ao checkout depois de
  aberta, o que o crawler não faz. Item sem preço aparece na lista mas não pode ser guardado nas
  ofertas. Se o estado trouxer `price`, ele vale.

Inventados ou completados (conferir no regression pass):

- O formato de `price` quando não vem vazio (número ou texto com ponto).
- Se a resposta da Hotmart a um pedido bloqueado é 403 ou outra página.
- Ids, nomes, notas, fotos e links.
