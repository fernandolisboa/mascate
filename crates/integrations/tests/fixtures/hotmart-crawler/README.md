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

Em 07/11/2026 (#220), com o leitor virando a fonte do Garimpo (ADR 0039), os arquivos ganharam à mão
os campos que a vitrine mostrou nesse dia a pedidos feitos à mão (uma página de categoria e uma de
produto): `ingressDate`, `totalClasses`, `totalHours`, `certification` e `videoLink` na lista, e o
`profile` da página do produto. Os valores e alguns formatos são inventados (ver abaixo).

| Arquivo | O que imita |
|---|---|
| `robots.txt` | o robots.txt da Hotmart no que importa ao crawler: tudo liberado, menos endereços com `q=` (a busca por palavras) e com `initialSelectedCategory=` |
| `best-sellers.html` | a página inicial da vitrine, com `bestSellers` no estado: produto com `ingressDate` em data ISO e `totalHours` em texto; um produto em `bannerSeason.products` (banner de campanha, não entra) e as `categories` internas, que não são produtos |
| `category.html` | a página de Culinária e gastronomia, com `allProducts.requestData.results`: produto completo (`ingressDate` em milissegundos, aulas, horas, certificado, vídeo); produto com `showRating: false` (entra sem nota), `ingressDate` com hora, aulas em texto, horas negativas (ficam vazias) e `videoLink: null`; produto com data, aulas e avaliações que não se leem (ficam vazias); produto com `salesEnabled: false` (fica de fora); produto com `slug` que é um link (fica de fora); produto sem `owner`, com nota zero; o primeiro produto repetido; e o bloco JSON-LD de breadcrumb |
| `product.html` | a página de produto `/produtos/escola-de-bolos-3-em-1/Q52300001P`: o produto de novo em `product`, o `profile` com `totalUsers`, `dataSheet.warranty`, `hotmarter` (`userSince`, `activeBadges` com `VERIFIED_PROFILE`) e `customerReview`, e um produto relacionado, que não é o da página |
| `product-odd.html` | uma página de produto sem o produto no estado, com alunos em texto, garantia "30 dias", `userSince` em data ISO, sem selos e uma avaliação zerada |
| `forbidden.html` | uma resposta 403 genérica, para o disjuntor do motor |

Como o leitor lê a vitrine:

- Sem categoria, lê os mais vendidos da página inicial (`{vitrine}`); com categoria, a página
  `{vitrine}/categorias/{categoria}`, só das 18 categorias da lista do leitor.
- Não há busca por palavras (o robots.txt da Hotmart proíbe `q=`) nem página seguinte: o estado traz
  só os 24 primeiros.
- Cada item de `bestSellers` ou `results` vira um Prospect com o id da Hotmart (`productId`), o
  título e os sinais: nota de `rating` quando `showRating` não é `false` (zero é sem nota),
  avaliações (`totalReviews`), quando entrou (`ingressDate`), aulas (`totalClasses`), horas
  (`totalHours`), certificado (`certification`), vídeo (`videoLink` preenchido), produtor
  (`owner.name`, `ownerName` ou `authorName`) e a página
  `https://hotmart.com/pt-br/marketplace/produtos/{slug}/{producerReferenceCode}`. Um campo que falta
  ou não se lê deixa o sinal vazio; o produto continua.
- A página do produto só é lida quando você pede, para um Prospect: soma alunos (`totalUsers`),
  garantia (`dataSheet.warranty`), desde quando o produtor está na Hotmart (`hotmarter.userSince`),
  perfil verificado (`VERIFIED_PROFILE` em `hotmarter.activeBadges`) e a nota de `customerReview`.
- A vitrine não traz preço, comissão, temperatura nem a página de afiliação: esses vêm do mercado
  logado (#221) ou digitados; a tela leva ao mercado da Hotmart para se afiliar.

Inventados ou completados (conferir no regression pass):

- O formato de `ingressDate` e de `userSince` (milissegundos, data ISO ou só o ano).
- O formato de `totalHours`, de `dataSheet.warranty` (número de dias ou texto) e de `activeBadges`.
- A forma de `customerReview` (`rating` e `totalReviews`) e onde o `profile` fica no estado.
- Se a resposta da Hotmart a um pedido bloqueado é 403 ou outra página.
- Ids, nomes, notas, fotos e links.
