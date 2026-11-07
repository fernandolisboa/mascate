# Fixtures do marketplace de afiliados da Kiwify (escritas à mão)

O Marketplace da Kiwify só abre com login, então estas fixtures **não** foram gravadas de uma conta
real: gravá-las seria rodar o leitor que fica desligado atrás da flag
`integrations.kiwify_affiliate_market` (ADR 0039). Cada arquivo é o valor que o script de leitura
do app (`market_script`, em `src/affiliate_market.rs`, com o seletor de
`src/kiwify_affiliate_market.rs`) devolveria numa página: o endereço em que a página parou, o
título, se há uma verificação contra robôs, os cartões de produto (cada um pelo link para a página
do produto no marketplace, com o texto de cada linha) e as linhas da página inteira. O servidor de
depuração falso dos testes responde com esse valor no lugar de rodar o script.

O que veio da ajuda pública da Kiwify (`ajuda.kiwify.com.br`) e dos termos de uso
(`kiwify.com.br/termos-de-uso`, versão de 29/10/2026), conferidos em 07/11/2026 sem login:

- o Marketplace é uma aba do menu principal do painel; para se afiliar, o afiliado abre o produto e
  clica em **Solicitar afiliação**;
- o produtor escolhe se a afiliação é aprovada na hora (automática) ou por ele (manual);
- só entram no Marketplace produtos com mais de R$ 5 mil vendidos na vida toda e com descrição;
- a ordem da lista vem do faturamento recente, da taxa de reembolso e dos chargebacks; a ajuda não
  diz que o cartão mostra o faturamento;
- há busca e filtros por preço, comissão e categoria; a ajuda não lista as categorias;
- o painel em `dashboard.kiwify.com.br/marketplace` é um app que desenha a página no navegador (sem
  login, o endereço responde só "Loading...");
- os termos não citam robôs; os itens 14.1 e 15.1 permitem suspender ou bloquear a conta por conduta
  contra os termos ou por qualquer motivo, e o item 10.1.17 bloqueia também o saldo.

Inventado até o regression pass da fase 3 (#116):

- os endereços: a lista em `{marketplace}` (sem segunda página: o leitor lê só a primeira), a
  página do produto em `{marketplace}/{código}`, e que cada cartão tem um link com
  `/marketplace/{código}`;
- os rótulos e formatos: "Preço R$ 197,00", "Comissão de até R$ 98,50", "Comissão: 50%", "Sua
  comissão", "Garantia" e "7 dias", "Produtor: …", "Aprovação automática" e "Aprovação manual";
- o login em `dashboard.kiwify.com.br/login`, a verificação contra robôs e a página de "Too Many
  Requests".

| Arquivo | O que imita |
|---|---|
| `listing.json` | a lista: produto completo com aprovação automática; produto com preço e comissão na linha seguinte, comissão só em % e aprovação manual; produto com preço e comissão que não se leem (ficam vazios); um link com código estranho (fica de fora); um cartão sem nome (fica de fora) |
| `details.json` | a página de um produto: título, produtor, preço, comissão em R$ e em %, garantia, aprovação automática e um produto relacionado |
| `drawing.json` | a página ainda desenhando ("Loading..."), sem cartões |
| `login.json` | o login da Kiwify |
| `captcha.json` | a verificação contra robôs |
| `blocked.json` | "429 Too Many Requests" |

Para regravar: ligue a flag, entre na Kiwify pela janela de **Entrar na Kiwify** no Garimpo, abra o
Marketplace e a página de um produto, abra as ferramentas do desenvolvedor (F12), cole o script
(o resultado de `market_script` com o seletor da Kiwify) envolto em `copy(...)` no console e cole o
resultado aqui, trocando nomes e valores de verdade por inventados. Confira também se o cartão
mostra o faturamento ou a categoria, se há endereço para a segunda página e para cada categoria, e
os rótulos da comissão e da aprovação.
