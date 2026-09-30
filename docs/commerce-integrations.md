# Venda própria, dropshipping, crawling e MVP revisado

> Pesquisa feita em 30/09/2026. Complementa [platform-integrations.md](platform-integrations.md), que cobre só afiliação. Mesmas marcações: **[C]** confirmado em fonte oficial; **[T]** terceiros; **[I]** inferido ou não verificado. Nada aqui é parecer jurídico ou fiscal.

## TL;DR

- **O escopo mudou de "afiliação" para "comércio multicanal".** O app passa a ter três modos de operação: afiliado (comissão), revenda com estoque próprio e dropshipping. Isso muda o modelo de dados: entram produto canônico, oferta de fornecedor, anúncio por canal, pedido, compra ao fornecedor, estoque (ledger) e tarifas.
- **APIs de vendedor são muito melhores que as de afiliado.** Mercado Livre, Shopee, Amazon (SP-API) e Magalu têm API oficial completa para anúncios, pedidos, envios e financeiro.
- **Shopee → Mercado Livre em dropshipping puro não fecha na prática.** O Mercado Envios exige a etiqueta do ML no pacote, e o vendedor da Shopee não vai colar a sua etiqueta. O caminho viável é **comprar na Shopee, receber no seu espaço e despachar pelo ML**, ou seja, estoque local (mesmo que curto). O espaço desocupado resolve exatamente isso.
- **Não existe API de compra como comprador** em Shopee, ML ou Amazon. A compra no fornecedor será manual (o app registra e acompanha). Compra automatizada só com AliExpress Dropshipping API ou fornecedores nacionais com API (Dropify e similares).
- **Crawling é tecnicamente possível, mas é o pior caminho** para os seus alvos: a Shopee proíbe robôs nos termos (cláusula 3.1 e 6.2(k)) e assina as requisições do site com headers anti-bot; a busca pública do ML via API está bloqueada desde o fim de 2025. A via sancionada para achar produtos bons na Shopee é a **própria Affiliate Open API** (`productOfferV2`, com vendas, preço e comissão).
- **Fiscal é um bloqueio real para revenda.** Vender com regularidade no ML exige CNPJ e NF-e de saída; comprar como pessoa física na Shopee não gera nota de entrada adequada para revenda. Vale conversar com um contador antes de operar em volume [I].

## 1. Crawling: dá para fazer?

**Resposta curta:** dá, mas não recomendo como base do produto. Recomendo como adapter opcional, isolado e desligável, só para dados públicos de produto.

| Alvo | Barreira técnica | Termos | Risco principal |
|---|---|---|---|
| Shopee (site) | API web exige headers assinados de curta duração (`x-sap-ri`, `x-sap-sec`, `af-ac-enc-dat`), fingerprinting e login [T] | Proíbe robôs para "monitorar ou copiar conteúdo" (3.1) e acesso por bots à conta (6.2(k)) [C] | Bloqueio e quebra frequente; burlar assinatura agrava (6.2(n)) |
| Mercado Livre (site) | Anti-bot não verificado [I]; busca via API retorna 403 mesmo com token [T] | Termos não puderam ser lidos (robots.txt); historicamente proíbem interferir no sistema [I] | Contratual e bloqueio |
| Amazon (site) | Bloqueio agressivo de tráfego automatizado [T] | Condições de Uso proíbem mineração de dados e robôs [I] | Perda da conta de Associados/vendedor |
| Painel logado (qualquer plataforma) | Cookies, captcha, 2FA | Hotmart 3.5(m) e Shopee 6.2(k) proíbem expressamente [C] | **Suspensão da sua própria conta**, e o app guardando credenciais de sessão vira passivo de segurança |

Situação jurídica no Brasil [T/I]: não há lei específica. O risco vem de violação contratual (termos), concorrência desleal (Lei 9.279/96, art. 195), proteção de base de dados (Lei 9.610/98) e LGPD quando há dados pessoais (nome de vendedor PF, avaliações). O precedente mais citado é Catho x Curriculum (indenização de ~R$ 21,8 mi por cópia automatizada de currículos burlando controle de acesso). Dados de produto (preço, título, vendas) têm risco baixo de LGPD; o risco dominante é bloqueio e banimento.

**Alternativas legítimas para "achar bons produtos":**

| Fonte | O que dá | Custo |
|---|---|---|
| Shopee Affiliate Open API (`productOfferV2`, `shopOfferV2`) | Busca por palavra-chave/categoria, ordenação por vendas e comissão, preço, vendas, loja | Grátis, exige aprovação de acesso [T] |
| ML `GET /highlights/MLB/category/{id}` | Top 20 mais vendidos por categoria, com token [C] | Grátis |
| ML catálogo (`/products/search`, `/products/{id}`) | Produto canônico, concorrentes no catálogo [C] | Grátis |
| Metrify | Vendas e faturamento estimados de concorrentes no ML; tem API [C] | R$ 59,90–89,90/mês [C] |
| Real Trends | Análise de mercado e concorrência no ML [C] | R$ 69–445/mês [C] |
| JoomPulse | Analytics de Shopee BR (vendas mensais, faturamento, preço) [C] | Sob consulta |
| Google Trends API | Tendência de demanda | Alfa com acesso restrito [T] |

**Recomendação:** Shopee Affiliate API + ML highlights/catálogo no MVP (grátis e oficiais). Se faltar sinal de demanda no ML, assinar Metrify, que tem API, antes de escrever qualquer crawler.

## 2. APIs de vendedor

| | Mercado Livre | Shopee Open Platform | Amazon SP-API (BR) | Magalu |
|---|---|---|---|---|
| **Auth** | OAuth2 authorization code; access 6 h; **refresh token de uso único**, 6 meses [C] | `partner_id`/`partner_key`, HMAC-SHA256 por request; autorização da loja 7–365 dias; access 4 h [T] | LWA OAuth, refresh token; app privado exige conta Professional [C] | OAuth2 via ID Magalu; access 2 h [C] |
| **Registro/aprovação** | App livre; certificação opcional (Developer Partner Program) [C] | App passa por aprovação; tipo Individual Seller disponível [T] | Registro de developer + questionário de segurança [C] | Client via CLI `idm`; alguns scopes pendentes de aprovação [C] |
| **Anúncios, preço, estoque** | `/items`, catálogo, variações; estoque 0 pausa o anúncio [C] | Product module [T] | Listings Items [C] | SKUs, preço, estoque [C] |
| **Pedidos e envio** | `/orders`, `/shipments`, etiqueta PDF/ZPL; Flex, Full [C] | Order + Logistics (etiqueta, tracking) [T] | Orders, FBA (BR com restrições) [C] | Orders, deliveries, NF-e, Magalu Entregas [C] |
| **Financeiro** | Billing (comissão, frete, publicação por pedido) [C]; liberação via Mercado Pago [I] | Escrow, payout, income report [T] | Reports, Finances [I] | Análise Financeira [C] |
| **Webhooks** | Sim; retry por 1 h; recuperação via `/missed_feeds` [C] | Push com HMAC; `get_lost_push_message` [I] | SQS/EventBridge (ex.: `ORDER_CHANGE`) [C] | Sim; HMAC-SHA256 com timestamp; payload só com referência [C] |
| **Rate limit** | Não publicado; por client e endpoint; 429 [C] | ~10 req/s por loja [T] | Token bucket por operação; `searchOrders` 0,0056 req/s [C] | Não publicado [C] |
| **Dropshipping** | Permitido via "prazo de disponibilidade" (`MANUFACTURING_TIME`, até 45 dias) [T]; não funciona com Flex/Full; atraso e cancelamento pesam na reputação (verde: ≤ 1,5% cancelamento, ≤ 10% atraso) [C] | Pré-venda com DTS (faixa diverge: 3–15 ou 7–30 dias) [T] | Permitido só se você é o vendedor identificado; **proibido comprar de outro varejista online e mandar direto** [C] | Sem política explícita; exige NF-e e SLA [C] |

Pontos que afetam implementação:
- **ML refresh token de uso único:** precisa de lock por conta e gravação atômica do novo token; dois refreshes concorrentes derrubam a integração.
- **Webhooks do ML e Magalu só trazem a referência:** o padrão é responder 200 na hora, enfileirar e buscar o recurso depois, com reconciliação periódica por polling.

## 3. Modelos de operação e o que o app precisa

| Modelo | Fluxo | Integrações | Risco principal |
|---|---|---|---|
| **Afiliado** | Divulga link, recebe comissão | APIs de afiliado (ver doc anterior) | Baixo |
| **Revenda com estoque local** | Compra (Shopee, atacado, fornecedor) → recebe no espaço → anuncia no ML → despacha com etiqueta ML (ou Flex) | ML seller API; compra registrada manualmente | Capital parado em estoque; fiscal |
| **Dropshipping nacional** | Anuncia → vende → compra no fornecedor → fornecedor envia | ML seller API + fornecedor com API (Dropify etc.) | Reputação (atraso/cancelamento); ruptura do fornecedor |
| **"Dropshipping" Shopee → ML** | Anuncia no ML → vende → compra na Shopee → recebe no espaço → reenvia | ML seller API; Shopee só como fonte de dados | Prazo (dois fretes), margem espremida; vira na prática revenda com estoque sob demanda |

O caso Shopee → ML se encaixa melhor como **revenda com estoque local**, comprando em lote o que a análise indicar, do que como dropshipping por pedido: o prazo de dois fretes pune a reputação no ML e anula a vantagem do Flex.

**Implicação no modelo de dados** (decisão sua antes de codar, como pedido no briefing):
- Núcleo comum: `Product` (canônico, SKU próprio), `SupplierOffer` (oferta de origem com preço e data), `ChannelListing` (anúncio por canal), `Order`/`OrderItem`, `PurchaseOrder` (compra ao fornecedor), `StockMovement` (ledger de entradas, saídas, ajustes; saldo é derivado), `Fee` (comissão do marketplace, frete, imposto).
- Afiliação continua como está: `Program`, `AffiliateLink`, `Conversion`, `Commission`, `Payout`.
- Margem por venda = preço − tarifas do canal − frete − custo de aquisição (custo médio do estoque) − impostos. Tudo em decimal com moeda.

## 4. MVP revisado

Você achou Hotmart + Shopee + Amazon pouco. Com o novo escopo, proponho organizar o MVP por **fatias verticais** em vez de por número de plataformas.

### Opção A (recomendada): arbitragem Shopee → ML com estoque local, mais afiliação

| Fatia | Plataformas | Entrega |
|---|---|---|
| **1. Descoberta** | Shopee Affiliate API (catálogo) + ML highlights/catálogo | Lista de produtos da Shopee com preço e vendas, cruzada com o que vende no ML, e margem estimada |
| **2. Venda e estoque** | Mercado Livre (seller API) + estoque local | Anúncios, pedidos, envio com etiqueta, tarifas, ledger de estoque, compras registradas manualmente, margem real por venda |
| **3. Afiliação** | Hotmart, Shopee e Kiwify por API/webhook; Amazon, ML e Magalu por import de arquivo | Vendas, comissões e status consolidados |

- **Prós:** atende a ideia que você trouxe (Shopee → ML) de ponta a ponta com integrações oficiais; o espaço físico entra no fluxo; Shopee rende duas coisas com uma credencial (catálogo e comissão).
- **Contras:** é bem maior que o MVP anterior; depende da aprovação da Shopee Open API; a parte fiscal precisa estar resolvida para vender em volume.

### Opção B: venda primeiro, afiliação depois

- Fatias 1 e 2 da Opção A; afiliação entra na fase seguinte.
- **Prós:** foco no que gera receita nova; menos adapters; valida o núcleo de comércio (o mais complexo) primeiro.
- **Contras:** o dashboard de afiliação, que era o objetivo original do briefing, fica para depois.

### Opção C: afiliação ampla primeiro (Hotmart, Kiwify, Monetizze, Shopee, import para o resto), venda depois

- **Prós:** integrações mais simples (só leitura); resultado rápido.
- **Contras:** adia a ideia Shopee → ML e o uso do espaço físico.

**Recomendação:** Opção A, entregue na ordem 2 → 1 → 3. A fatia de venda e estoque é a que define o núcleo do modelo de dados; descoberta e afiliação se apoiam nela. Magalu e Amazon como vendedor ficam fora do MVP: a Amazon proíbe expressamente comprar em outro varejista para enviar direto, e ambos exigem conta profissional e SLA mais rígido.

## 5. Decisões e pré-requisitos

1. **Escopo do MVP:** A, B ou C.
2. **CNPJ e NF-e:** vocês já têm CNPJ apto a vender no ML e emitir NF-e? Sem isso, a fatia 2 não opera em volume. Conversar com contador sobre regime e nota de entrada das compras na Shopee [I].
3. **Pedir acesso à Shopee Affiliate Open API** já, porque a aprovação tem relatos de semanas.
4. **Crawling:** confirmar que fica fora do MVP. Se um dia entrar, como adapter isolado, só com dados públicos de produto, em baixo volume e sem burlar barreiras.

## Fontes principais

- ML: https://developers.mercadolibre.com.ar/en_us/authentication-and-authorization · https://developers.mercadolibre.com.ar/en_us/products-receive-notifications · https://developers.mercadolibre.com.ar/en_us/sellers-reputation · https://developers.mercadolibre.com.ar/en_us/billing-reports · https://developers.mercadolibre.com.ar/en_us/best-sellers-in-mercado-libre · https://developers.mercadolibre.com.mx/en_us/rate-limit-429-error
- Shopee: https://help.shopee.com.br/portal/4/article/77113 (termos) · https://open.shopee.com/developer-guide/20 · https://docs.celigo.com/hc/en-us/articles/18977971017883-Available-Shopee-APIs
- Amazon: https://developer-docs.amazon/sp-api/docs/sp-api-registration-overview · https://developer-docs.amazon/sp-api/docs/usage-plans-and-rate-limits · https://venda.amazon.com.br/sellerblog/dropshipping
- Magalu: https://developers.magalu.com/docs/apis/ · https://developers.magalu.com/docs/development-guide/webhooks/index.html · https://universo.magalu.com/blog/artigo/acordo-de-nivel
- Hotmart (termos): https://hotmart.com/pt-br/legal/termos-de-uso
- Dados de mercado: https://metrify.com.br/ · https://www.real-trends.com/br/precos · https://joompulse.com/shopee
- Fornecedores: https://dropify.com.br/api · https://dev.to/zuplo/a-developers-guide-to-the-aliexpress-api-4f45
