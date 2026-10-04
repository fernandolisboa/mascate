# Margem alvo e Price Suggestion ficam no Commerce

A #19 traz a margem alvo de cada Product (com um padrão para todos), a Price Suggestion de cada Listing e o simulador de preço. O PRD põe a Price Suggestion no Commerce; a margem alvo fica com ela, numa tabela própria do Commerce ligada ao Product pelo id (`commerce_target_margins`), mais o padrão numa linha de configuração (`commerce_pricing_settings`, 20% até o Fernando mudar). É o mesmo desenho do Reorder Point no Inventory (#14). O custo de cada unidade é o Average Cost do Inventory, que o Commerce já usa (ADR 0012); a alíquota de imposto (Finance) e o frete estimado (configurações de descoberta do Catalog) chegam de quem chama, como a alíquota chega ao Catalog nas Opportunities (ADR 0013).

A sugestão é o menor preço, em centavos, em que cada variação aberta do anúncio chega à margem alvo do Product que vende, depois da tarifa do Sales Channel nesse preço, do frete a partir do preço de frete grátis, do imposto e do Average Cost. As variações dividem o preço (ADR 0014), então vale a mais alta. A tarifa muda com o preço (o Mercado Livre cobra uma parte fixa abaixo de R$ 79), por isso a porta pergunta a tarifa no preço de hoje e de novo em cada preço calculado, até a tarifa e o frete usados serem os que valem ali.

O Mercado Livre só recebe um preço quando o Fernando clica para enviar: a porta `SalesChannel` ganha `set_price`, chamada apenas por `Listings::change_price`, que nenhuma rotina automática usa.

## Considered Options

- **Margem alvo no Catalog:** coluna no Product e padrão nas configurações do Catalog; o Commerce receberia o valor de quem chama. Põe uma regra de preço no módulo de descoberta.
- **Margem alvo no Finance:** ao lado da alíquota; o Commerce receberia a margem de quem chama. O Finance é dono das margens realizadas, não do preço que se pede.

## Consequences

- Migração nova no Commerce (versão 3), sem risco: só cria tabelas.
- A porta `SalesChannel` ganha `sale_fee` (parte percentual e parte fixa da tarifa) e `set_price`; o adapter do Mercado Livre lê as variações logo antes de mudar o preço, porque uma variação deixada fora do pedido é apagada.
- A regra do frete grátis a partir de R$ 79 passa para o kernel (`shipping_paid_by_seller`), usada pelas Opportunities e pela Price Suggestion.
- A alíquota de imposto sai da tela de Oportunidades e vai para Configurações › Preços e margens, junto da margem alvo padrão.
