# Um Listing por variação do anúncio

Um anúncio do Mercado Livre pode ter variações (cor, tamanho), cada uma com o próprio estoque e o próprio SKU do vendedor; o preço é o mesmo para todas. No `CONTEXT.md`, um Listing é o anúncio de **um** Product. Por isso o Sync de Listings (#15) guarda uma linha por variação, com o id do anúncio e o da variação, e cada uma se liga ao Product que vende; um anúncio sem variações é uma linha só. A chave natural é o par (id do anúncio, id da variação), o que deixa a reimportação idempotente.

## Considered Options

- **Anúncio com variações dentro:** um Listing por anúncio e uma tabela de variações, cada uma ligável a um Product. É mais fiel ao formato do Mercado Livre, mas estoque (#18), preço (#19) e pedidos (#20) teriam de tratar sempre os dois casos, anúncio simples e variação.
- **Ignorar variações:** um Listing por anúncio ligado a um só Product, com o estoque somado. Simples, mas errado para um anúncio que vende vários SKUs.

## Consequences

- Um pedido do Mercado Livre traz o item e a variação; a #20 acha o Listing, e por ele o Product, direto por esse par.
- A #18 atualiza o estoque de cada variação pelo seu Listing. A #19 sugere um preço por anúncio: as variações do mesmo anúncio têm o mesmo preço e mudam juntas.
- Telas que mostram o anúncio inteiro (qualidade, #23) agrupam os Listings pelo id do anúncio.
- Uma variação que some do anúncio, ou um anúncio apagado, fica como encerrado; o vínculo com o Product continua, para o histórico.
