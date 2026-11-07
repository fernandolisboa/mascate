# Fixtures da Vitrine de afiliação da Monetizze (escritas à mão)

A Vitrine de afiliação da Monetizze (a Loja do painel antigo) só abre com login, então estas
fixtures **não** foram gravadas de uma conta real: gravá-las seria rodar o leitor que fica
desligado atrás da flag `integrations.monetizze_affiliate_market` (ADR 0039). Cada arquivo é o
valor que o script de leitura do app (`market_script`, em `src/affiliate_market.rs`, com o seletor
de `src/monetizze_affiliate_market.rs`) devolveria numa página: o endereço em que a página parou,
o título, se há uma verificação contra robôs, os cartões de produto (cada um pelo link para a
página do produto na Vitrine, com o texto de cada linha) e as linhas da página inteira. O servidor
de depuração falso dos testes responde com esse valor no lugar de rodar o script.

O que veio da ajuda pública da Monetizze (`help.monetizze.com.br`, livro Afiliação, páginas
"Vitrine de afiliação (Loja)", "Como se tornar um Afiliado na Monetizze" e "Modelos de atribuição
de comissão", e livro Programa de Afiliação) e dos termos de uso (`monetizze.com.br/termos-uso`,
atualizados em 13/01/2026), conferidos em 07/11/2026 sem login:

- a Vitrine é "Vitrine de afiliação" no menu lateral do painel novo e "Loja" no menu de cima do
  antigo, com mais de 35 mil produtos, uma busca e a "Busca avançada" por tipo do produto,
  comissionamento, categoria e preço;
- as seções **Hall da Fama** (pela pontuação BAR, que soma vendas, afiliados, o desempenho deles,
  reembolsos e chargebacks), **Em Destaque** (mais de R$ 250 mil vendidos), **Oportunidades**
  (de R$ 50 mil a R$ 250 mil) e **Mais Recentes**; a ajuda não mostra endereço de seção, de página
  nem de busca;
- a captura da página de um produto na ajuda mostra: estrelas com "100 Bars", "Você ganha até
  R$ … por indicação", "Página de Venda", "Categoria", "Formato: Produto Físico", "Produtor",
  "Email Suporte", "Tipo de atribuição", "Comissão dos afiliados: 60,00% do valor líquido da
  venda", "Preço do Produto: Adesão R$ … • Recorrência R$ …", "Sua Comissão: Adesão … •
  Recorrência até R$ …", "Televendas" e os botões "Denunciar esse produto" e "Promover este
  produto";
- o produtor escolhe "Todos podem promover meu produto" (afiliação automática) ou "Somente
  afiliados aprovados por mim podem promover meu produto"; a ajuda chama a segunda de afiliação
  restrita ou fechada;
- os termos não citam robôs; o item 13.5 deixa a Monetizze rescindir por qualquer descumprimento,
  o 13.8 cita a tentativa de violar as medidas de segurança da plataforma como causa de bloqueio,
  o 14.1 veda usar o sistema, a interface e os softwares sem autorização, e os 13.4 e 13.9 deixam
  bloquear e reter o saldo.

Inventado até o regression pass da fase 3 (#116):

- o endereço da Vitrine, `https://app.monetizze.com.br/loja`, e que cada cartão tem um link
  para a página do produto debaixo dele, terminando no número do produto (aqui
  `/loja/produto/{número}`); sem esse link, o leitor não acha cartão nenhum e diz que a Vitrine não
  mostrou produtos;
- que o cartão mostra os BARs, a categoria, o formato, "Você ganha" ou "Comissão" e a afiliação
  ("Afiliação aberta", "Afiliação mediante aprovação do produtor");
- o login em `app.monetizze.com.br/login`, a verificação contra robôs e a página de "403
  Forbidden".

Não viram sinal ainda: a faixa de faturamento (Em Destaque ou Oportunidades) e a posição no Hall da
Fama, porque a ajuda só os mostra como seções sem endereço; os BARs, que ordenam o Hall da Fama,
entram como sinal e pontuam as vendas. O e-mail de suporte, a página de venda e o tipo de
atribuição não são guardados.

| Arquivo | O que imita |
|---|---|
| `listing.json` | a Vitrine: produto físico com BARs, comissão em R$ e afiliação aberta; produto digital com comissão só em % e afiliação com aprovação, com o link levando `?origem=`; produto com "Bars" no nome e a comissão na linha de baixo (o nome não vira BARs); produto com "Bars" sem número, formato e comissão que não se leem (ficam vazios); um link de categoria (fica de fora); um link com número estranho (fica de fora); um cartão sem nome (fica de fora) |
| `details.json` | a página de um produto, como a captura da ajuda: BARs, "Você ganha", categoria, formato, produtor, comissão em %, preço de adesão e de recorrência (vale o de adesão), "Sua Comissão" e afiliação aberta |
| `drawing.json` | a página ainda desenhando ("Carregando..."), sem cartões |
| `login.json` | o login da Monetizze |
| `captcha.json` | a verificação contra robôs |
| `blocked.json` | "403 Forbidden" |

Para regravar: ligue a flag, entre na Monetizze pela janela de **Entrar na Monetizze** no Garimpo,
abra a Vitrine de afiliação e a página de um produto, abra as ferramentas do desenvolvedor (F12),
cole o script (o resultado de `market_script` com o seletor da Monetizze) envolto em `copy(...)` no
console e cole o resultado aqui, trocando nomes e valores de verdade por inventados. Confira também
o endereço da Vitrine e o link de cada cartão, se há endereço para as seções, para a segunda página
e para a busca avançada, e os rótulos dos BARs, da comissão e da afiliação aberta ou fechada.
