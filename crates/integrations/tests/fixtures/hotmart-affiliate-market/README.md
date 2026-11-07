# Fixtures do mercado de afiliação da Hotmart (escritas à mão)

O Mercado de Afiliação da Hotmart só abre com login, e os termos de uso da Hotmart (item 3.5(m))
proíbem robôs e scripts, então estas fixtures **não** foram gravadas de uma conta real: gravá-las
seria rodar o leitor que fica desligado atrás da flag `integrations.hotmart_affiliate_market`
(ADR 0039). Cada arquivo é o valor que o script de leitura do app (`market_script`, em
`src/affiliate_market.rs`, com o seletor de `src/hotmart_affiliate_market.rs`) devolveria numa página: o endereço em que a página parou, o
título, se há uma verificação contra robôs, os cartões de produto (cada um pelo link para a página
do produto no mercado, com o texto de cada linha) e as linhas da página inteira. O servidor de
depuração falso dos testes responde com esse valor no lugar de rodar o script.

O que veio da ajuda pública da Hotmart e do blog dela, conferidos em 07/11/2026 (sem login):

- o mercado fica em `app.hotmart.com`, em Produtos > Mercado de Afiliação;
- a temperatura vai de 0 a 150°, sobe com as vendas e mede também quantos afiliados concorrem;
- o Blueprint mede o material do produtor ao afiliado (a Hotmart recomenda acima de 80%);
- a satisfação vem das notas dos compradores (de 1 a 5);
- a comissão aparece em % e em R$, e a afiliação é imediata ou pede aprovação do produtor.

O que foi verificado à mão na vitrine pública em 07/11/2026 (#221): a página de um produto
(`hotmart.com/pt-br/marketplace/produtos/{slug}/{código}`) traz no estado o `ucode` do produto ao
lado do `productId`. O leitor da vitrine guarda com ele a página de afiliação
`https://app.hotmart.com/market/details?productUcode={ucode}`, e o leitor do mercado escreve a
mesma, então o mesmo produto lido pelos dois vira um Prospect só.

Inventado até o regression pass da fase 3 (#116):

- os endereços: a lista em `{mercado}?page={n}` e `{mercado}?category={categoria}&page={n}`, com as
  18 categorias da vitrine, e a página do produto em `{mercado}/details?productUcode={código}` (a
  Hotmart pode pedir também o `producerUcode`);
- que cada cartão tem um link com `productUcode=` e que o id do produto (`productId=` num link, ou
  "ID do produto" no texto) só às vezes aparece; sem ele o Prospect fica com o código do mercado
  até a página do produto na vitrine juntar os dois;
- os rótulos e formatos: "Temperatura" e "87°", "Comissão de até R$ 148,50", "Comissão: 50%",
  "Ganhe até US$ 30,00 (40 %)", "Preço", "Blueprint 85%", "Satisfação 4,6" (estrelas viram %),
  "Afiliação imediata", "Afiliação com aprovação do produtor", "Afilie-se agora", "Produtor";
- o login em `sso.hotmart.com/login`, a verificação contra robôs e a página de "Too Many Requests".

| Arquivo | O que imita |
|---|---|
| `listing.json` | a primeira página da lista: produto completo; produto com `productId=` num link, comissão em dólar e afiliação com aprovação; produto com temperatura acima de 150°, comissão "a combinar", Blueprint e satisfação que não se leem (ficam vazios); um link com código estranho (fica de fora); um cartão sem nome (fica de fora) |
| `listing-page-2.json` | a segunda página: um produto novo e um repetido |
| `listing-page-3.json` | uma página só com repetidos: a leitura para aí |
| `details.json` | a página de um produto: título, "ID do produto", produtor, os sinais com o valor na linha seguinte, satisfação "4,8/5" e um produto relacionado |
| `drawing.json` | a página ainda desenhando, sem cartões |
| `login.json` | o login da Hotmart |
| `captcha.json` | a verificação contra robôs |
| `blocked.json` | "429 Too Many Requests" |

Para regravar: ligue a flag, entre na Hotmart pela janela de **Entrar na Hotmart** no Garimpo, abra
o mercado e a página de um produto, abra as ferramentas do desenvolvedor (F12), cole o script
(o resultado de `market_script` com o seletor da Hotmart) envolto em `copy(...)` no console e cole o resultado aqui, trocando nomes e valores
de verdade por inventados. Confira também os endereços da lista, da categoria e da página do
produto, e se a página do produto abre só com o `productUcode`.
