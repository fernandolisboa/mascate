# Fixtures da Vitrine de afiliados da Eduzz (escritas à mão)

A Vitrine da Eduzz só abre com login, então estas fixtures **não** foram gravadas de uma conta
real: gravá-las seria rodar o leitor que fica desligado atrás da flag
`integrations.eduzz_affiliate_market` (ADR 0039). Cada arquivo é o valor que o script de leitura
do app (`market_script`, em `src/affiliate_market.rs`, com o seletor de
`src/eduzz_affiliate_market.rs`) devolveria numa página: o endereço em que a página parou, o
título, se há uma verificação contra robôs, os cartões de produto (cada um pelo link para a página
do produto na Vitrine, com o texto de cada linha) e as linhas da página inteira. O servidor de
depuração falso dos testes responde com esse valor no lugar de rodar o script.

O que veio da ajuda pública da Eduzz (`ajuda.eduzz.com`, artigo "Como funciona Vitrine de Produtos
Digitais?", de 18/08/2026) e dos termos de uso (`eduzz.com/termos`, sem data de versão), conferidos
em 07/11/2026 sem login:

- a Vitrine é uma aba do menu lateral do MyEduzz, com as abas **Mais Vendidos** (os mais vendidos
  por afiliados nos últimos 30 dias), **Novidades** e **Produto Físico**, uma busca e **Filtros**;
  a ajuda não lista categorias nem mostra endereço de aba ou de página;
- para se afiliar, o afiliado clica em **Afiliar-se agora** na página do produto: com a afiliação
  automática ativa ele já vira afiliado; nos outros, o pedido vai para o produtor aprovar;
- os termos não citam robôs; a parte **Penalidades** deixa a Eduzz suspender o acesso ou bloquear
  a conta por qualquer conduta contra os termos, a seu critério; **Direitos Autorais** proíbe
  copiar ou guardar o conteúdo da plataforma sem autorização; **Rescisão** deixa a Eduzz reter o
  saldo.

O que veio do código público do MyEduzz (o JavaScript que `orbita.eduzz.com` entrega a qualquer
um, lido em 07/11/2026 sem login, sem chamar a API dele):

- a Vitrine fica em `https://my2.eduzz.com/showcase` e a página de um produto em
  `{vitrine}/product/details/{id}`;
- o cartão de produto que o MyEduzz desenha mostra o nome, o produtor (sem rótulo), "RPC P" e
  "RPC A" (receita por clique do produtor e dos afiliados), "Comissão:" com o valor por venda e,
  quando há assinatura, o valor recorrente com "rec.", e a nota em estrelas (sem rótulo). Esse
  cartão abre o produto por clique, sem link no HTML.

Inventado até o regression pass da fase 3 (#116):

- que cada cartão da Vitrine tem um link com `/product/details/{id}` (sem ele, o leitor não acha
  cartão nenhum e diz que a Vitrine não mostrou produtos);
- os rótulos da página do produto: "Produtor: …", "Preço", "Comissão por venda", "Avaliação" com
  "4,6 (23 avaliações)", "Garantia" e "7 dias", "Afiliação automática ativa" e "Afiliação com
  aprovação do produtor";
- o login em `accounts.eduzz.com/…/login`, a verificação contra robôs e a página de "403
  Forbidden".

Não viram sinal ainda: o RPC dos afiliados, a nota e o produtor do cartão (sem rótulo, só pela
posição) e a ordem de Mais Vendidos (a Vitrine não mostra o número). Se o regression pass mostrar
que vale a pena, o RPC entra como sinal numa fatia própria.

| Arquivo | O que imita |
|---|---|
| `listing.json` | a Vitrine: produto com comissão recorrente e por venda (vale a por venda); produto com comissão só em % e afiliação com aprovação; produto só com comissão recorrente (vale ela); produto com comissão que não se lê (fica vazia); um link com id estranho (fica de fora); um cartão sem nome (fica de fora) |
| `details.json` | a página de um produto: título, produtor, preço, comissão em R$ e em %, avaliação com o número de avaliações, garantia, afiliação automática ativa e um produto relacionado |
| `drawing.json` | a página ainda desenhando ("Carregando..."), sem cartões |
| `login.json` | o login da Eduzz |
| `captcha.json` | a verificação contra robôs |
| `blocked.json` | "403 Forbidden" |

Para regravar: ligue a flag, entre na Eduzz pela janela de **Entrar na Eduzz** no Garimpo, abra a
Vitrine e a página de um produto, abra as ferramentas do desenvolvedor (F12), cole o script (o
resultado de `market_script` com o seletor da Eduzz) envolto em `copy(...)` no console e cole o
resultado aqui, trocando nomes e valores de verdade por inventados. Confira também se o cartão tem
link para o produto, se há endereço para as abas, para a segunda página e para cada categoria, e
os rótulos da comissão, da avaliação e da afiliação automática.
