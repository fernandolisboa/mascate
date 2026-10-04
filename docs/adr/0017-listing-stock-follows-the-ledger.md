# O estoque do Listing segue o ledger, sem fila de eventos

A #18 faz o estoque de cada Listing publicado acompanhar o saldo do Product no app: toda mudança de saldo vai ao Mercado Livre, e uma falha de envio não pode perder a atualização. Decidimos que o que falta enviar é **derivado**, não enfileirado.

1. **Cada Listing guarda o último estoque enviado** (`stock_sent`, com a hora) e a última falha (o tipo do `PlatformError`, o detalhe e a hora), em colunas novas de `commerce_listings` (migração 5 do Commerce). O alvo de cada Listing é o saldo do Product em todas as Stock Locations, lido do ledger na hora (ADR 0005). Um Listing está na fila quando esse saldo difere do último enviado ou, antes do primeiro envio, do estoque que o canal informou no Sync.
2. **Quem envia é o `StockMirror` do Commerce**, que lê o saldo pelo Inventory (ADR 0012) e escreve pela porta `SalesChannel`, que ganhou `set_stock`, `pause` e `activate` (ADR 0013). Um anúncio com variações recebe um único `PUT` com o id de todas as variações, lidas do canal logo antes, e o estoque só nas que mudaram: o Mercado Livre apaga uma variação omitida. Depois do envio, os anúncios enviados são lidos de novo, porque o Mercado Livre pausa sozinho um anúncio que fica sem estoque e o reativa quando o estoque volta (exceto se foi o vendedor quem pausou).
3. **O app envia depois de cada Stock Movement e de cada Sync**, e o Fernando pode mandar a fila na hora pela tela Anúncios. Uma resposta que se perde só deixa o Listing na fila; reenviar o mesmo saldo é igual ao anterior, então nada é escrito. Uma chamada que corre junto com um movimento novo refaz a rodada (até três) com o saldo atualizado.
4. **Só segue o app** o Listing ligado a um Product, ativo ou pausado, cujo Product já teve algum movimento de estoque. Um Product que nunca entrou no estoque não zera anúncios importados de um estoque que o app ainda não conhece; ele passa a seguir no primeiro recebimento.
5. **A base é o último enviado, não o que o canal tem agora.** Uma venda no Mercado Livre baixa o estoque lá antes de a #20 baixar o do app; se a base fosse o estoque do canal, cada Sync devolveria ao anúncio a unidade vendida. Com o último enviado como base, o app só escreve quando o saldo dele muda. Trocar o Product do Listing apaga a base, que era de outro Product.

## Considered Options

- **Fila de eventos no banco:** cada Stock Movement gravaria um pedido de envio na mesma transação, e um despachante os consumiria. Exige ordenar e compactar pedidos do mesmo Listing, e todo caminho que grava movimento (recebimento, ajuste, venda, devolução) teria de enfileirar; o Inventory passaria a saber de Listings, ou o Commerce teria de envolver toda escrita de estoque.
- **Só envio imediato:** mandar na hora e mostrar o erro. Uma falha só voltaria a ser enviada na próxima mudança de saldo, o que não atende a issue.
- **Base no estoque do canal:** mais simples de explicar ("o anúncio tem o que o app tem"), mas antes da baixa por venda (#20) cada envio desfaria as vendas do canal e abriria espaço para vender o que não existe.

## Consequences

- A #20 grava a baixa por venda no ledger e chama `StockMirror::send` depois de cada Sync de Orders; o estoque enviado já sai certo, sem código novo no espelhamento. A ordem importa: Orders primeiro, estoque depois.
- Dois Listings do mesmo Product recebem o saldo inteiro cada um; dividir o estoque entre canais é da fase 3 (história 96).
- Pausar e reativar são ações do Fernando na tela, sobre o anúncio inteiro (todas as variações). Reativar um anúncio sem estoque no canal é recusado antes de chamar o Mercado Livre, porque ele continuaria pausado.
