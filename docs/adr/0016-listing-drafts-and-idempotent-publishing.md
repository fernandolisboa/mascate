# Rascunho é um Listing, e publicar é idempotente

A #16 cria anúncios a partir de um Product: o app monta um Listing Draft, mostra o Checklist, passa pelo validador do Mercado Livre e publica só no clique do Fernando. Decidimos três coisas.

1. **O rascunho é o próprio Listing.** Ele é uma linha em `commerce_listings` com status `draft` e sem id do Mercado Livre, já ligada ao Product. Os campos que só um rascunho tem (descrição, categoria, condição, garantia, atributos, fotos) ficam em tabelas próprias do Commerce: `commerce_listing_drafts`, `commerce_draft_attributes` e `commerce_draft_pictures`. Ao publicar, a mesma linha ganha o id do anúncio e o status do canal; o vínculo com o Product não muda, e o histórico do rascunho fica.
2. **Uma porta própria para publicar.** O Commerce define `ListingPublisher` (preditor de categoria, atributos da categoria, envio de foto, validador, criar o anúncio, enviar a descrição, buscar anúncio pelo SKU do vendedor), separada de `SalesChannel`, que lê anúncios e muda preço. O adapter `MercadoLivre` implementa as duas, como manda o ADR 0013; nenhum crate ganha dependência nova.
3. **Publicar de novo nunca duplica o anúncio.** Antes do `POST /items` o rascunho grava a hora da tentativa. Se a resposta se perde (timeout, 5xx), a marca fica, e a tentativa seguinte procura o anúncio pelo SKU do vendedor antes de criar outro. Adota um anúncio que o app ainda não conhece, ou um que um Sync trouxe depois do início da tentativa, sem vínculo ou já ligado ao mesmo Product; esse Listing sincronizado é apagado (soft delete) para o rascunho ficar como o único. Um Listing que o app já tinha antes da tentativa nunca é adotado. Uma recusa clara do canal (400, 403, 429) limpa a marca, porque nada foi criado. As fotos enviadas guardam o id do canal na hora e não sobem duas vezes. A descrição vai depois do anúncio; se falhar, o rascunho fica com a descrição pendente e a próxima publicação manda só ela.

## Considered Options

- **Rascunho numa tabela separada de Listings**, copiado para `commerce_listings` ao publicar. Deixa a tabela de Listings só com anúncios do canal, mas a cópia troca o id e cria dois registros do mesmo anúncio para Orders (#20) e estoque (#18) reconciliarem.
- **Estender `SalesChannel` com os métodos de publicação.** Uma porta só, mas o Sync e o Pricing passariam a depender de métodos que não usam, e os fakes de teste de cada um cresceriam sem motivo.
- **Idempotência só pela marca local**, sem buscar pelo SKU. Mais simples, mas uma resposta perdida deixaria o rascunho travado ou criaria um anúncio duplicado no Mercado Livre.

## Consequences

- `Listings::drafts()` lista os rascunhos e os publicados com a descrição pendente; a lista de Listings do Sync segue mostrando só os que têm id do canal.
- O SKU do vendedor vai no anúncio como atributo `SELLER_SKU`; é por ele que a busca acha uma tentativa perdida. Dois Listings do mesmo Product não se confundem, porque só é adotado um anúncio que nenhum outro Listing do app já tem.
- O GTIN é um atributo do rascunho, não um campo do Product: o Checklist só avisa quando ele falta.
- A #17 preenche o título e a descrição do rascunho; a #18 passa a mandar o estoque dos Listings publicados.
