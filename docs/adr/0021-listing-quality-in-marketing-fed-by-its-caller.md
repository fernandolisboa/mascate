# A qualidade do anúncio fica no Marketing, que recebe os anúncios de quem chama

A #23 traz o painel de qualidade dos Listings: o nível e a pontuação que o Mercado Livre dá a cada anúncio, as ações que ele diz faltar (fotos, ficha técnica, GTIN, título) com o link para corrigir, e a ordem em que vale mexer, por vendas e visitas. O PRD põe a qualidade de anúncio no módulo Marketing; os Listings e os Orders moram no Commerce. Decidimos três coisas.

1. **O Marketing é dono da Listing Quality e não lê o Commerce.** Ele guarda, por id do anúncio no canal, a pontuação, o nível, as visitas dos últimos 30 dias e as ações pendentes (`marketing_listing_quality` e `marketing_quality_actions`, primeira migração do Marketing). Quem chama passa os anúncios abertos e as unidades vendidas de cada um: `ListingQuality::sync(fonte, ids)` lê o canal e `ListingQuality::panel(anúncios)` devolve o painel ordenado. É o mesmo desenho do Commerce recebendo `CatalogProduct` e do Catalog recebendo a alíquota (ADR 0013, 0015). O Commerce ganha só a consulta `Orders::units_sold_since`, unidades vendidas por anúncio (as variações juntas), sem Orders cancelados.
2. **Uma porta própria, implementada pelo adapter.** O Marketing define `QualitySource` (qualidade e visitas de um anúncio), e o `MercadoLivre` a implementa com `GET /item/{id}/performance`, que substituiu `/health`, e `GET /items/{id}/visits/time_window?last=30&unit=day`. Só as regras `PENDING` viram ação; `mode` `WARNING` é problema que derruba a nota, `OPPORTUNITY` é sugestão; um 404 quer dizer que o Mercado Livre ainda não calculou a qualidade. O texto e o link vêm do próprio canal (em português no MLB), e o app só abre links `https` do Mercado Livre. O `mascate-integrations` passa a depender de `mascate-marketing`, extensão da lista do teste de arquitetura prevista no ADR 0013.
3. **A ordem é a de onde mexer primeiro.** Primeiro os anúncios com algo pendente, dos que mais venderam nos últimos 30 dias, depois os mais visitados, depois a menor nota; em seguida os que ainda não têm nota; por último os que não têm nada pendente. Dentro de um anúncio, os problemas vêm antes das sugestões. A qualidade é lida no Sync de Anúncios, depois dos anúncios e do estoque, e no botão da tela Qualidade; ler de novo com as mesmas respostas não muda nada, e uma ação que o canal deixou de informar sai (soft delete). Se a leitura de algum anúncio falha, nada é gravado e o painel fica com o último Sync.

O painel só aponta e leva ao Mercado Livre: nenhuma correção sai do app sem a mão do Fernando, como nas outras fatias da fase 1.

## Considered Options

- **Marketing lê o Commerce:** o Marketing dependeria do Commerce e leria Listings e Orders sozinho. Menos código no app, mas cria a aresta `mascate-marketing -> mascate-commerce`, que as próximas fatias de marketing tenderiam a usar.
- **Tudo no Commerce:** nenhuma aresta nova e a qualidade perto do Checklist da #16, mas contraria a divisão de módulos do PRD, e o Commerce já concentra Listings, Orders, Fees e preços.

A pergunta foi ao Fernando num card; o app seguiu a recomendação.

## Consequences

- O app ganha a tela Qualidade, entre Anúncios e Pedidos, e o Sync de Anúncios passa a dizer quantas ações ficaram pendentes.
- Uma chamada de qualidade e uma de visitas por anúncio a cada Sync: com os poucos anúncios da fase 1, não há fila nem espera entre elas. Se o volume crescer, a leitura pode passar a pular anúncios lidos há pouco, como o faturamento faz na #22.
- Fica para o regression pass (#37): os textos e os valores reais de `level`, se a janela de visitas sem `ending` termina no dia de hoje, e se o 401 "Caller must be the seller of the item" aparece para anúncios do próprio vendedor.
