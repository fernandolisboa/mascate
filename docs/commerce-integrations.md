# Venda própria, dropshipping, crawling e plano por fases

> Pesquisa feita em 30/09/2026. Complementa [platform-integrations.md](platform-integrations.md), que cobre só afiliação. Mesmas marcações: **[C]** confirmado em fonte oficial; **[T]** terceiros; **[I]** inferido ou não verificado. Nada aqui é parecer jurídico ou fiscal.

## TL;DR

- **O escopo mudou de "afiliação" para "comércio multicanal".** O app passa a ter três modos de operação: afiliado (comissão), revenda com estoque próprio e dropshipping. Isso muda o modelo de dados: entram produto canônico, oferta de fornecedor, anúncio por canal, pedido, compra ao fornecedor, estoque (ledger) e tarifas.
- **APIs de vendedor são muito melhores que as de afiliado.** Mercado Livre, Shopee, Amazon (SP-API) e Magalu têm API oficial para anúncios, pedidos, envios e financeiro (Shopee e parte do Magalu com detalhes só de terceiros [T]).
- **Shopee → Mercado Livre em dropshipping puro não fecha na prática.** O Mercado Envios exige a etiqueta do ML no pacote, e o vendedor da Shopee não vai colar a sua etiqueta. O caminho viável é **comprar na Shopee, receber no seu espaço e despachar pelo ML**, ou seja, estoque local (mesmo que curto). O espaço desocupado resolve exatamente isso.
- **Não existe API de compra como comprador** em Shopee, ML ou Amazon. A compra no fornecedor será manual (o app registra e acompanha). Compra automatizada só com AliExpress Dropshipping API ou fornecedores nacionais com API (Dropify e similares).
- **Crawling entra como fonte complementar**, não como base: o site da Shopee assina as requisições com headers anti-bot que mudam com frequência, e a busca do ML via API tem relatos de 403 desde o fim de 2025 [T]. A fonte principal para achar produtos bons na Shopee é a **própria Affiliate Open API** (`productOfferV2`, com vendas, preço e comissão), oficial e gratuita, sujeita a aprovação.
- **Regra jurídica do projeto:** questões fiscais e jurídicas não bloqueiam o planejamento nem as issues; viram lembretes ⚠️. O que for proibido (por lei ou pelos termos da plataforma) é construído, mas fica **desativado por feature flag**, ativável quando houver permissão, mudança de termos ou decisão sua (seção 6).

## 1. Crawling: dá para fazer?

**Resposta curta:** dá, e entra como mais uma fonte de dados (adapter de crawler) com flag por plataforma. Critério único: o adapter é construído e só nasce **ativo** quando os termos foram lidos e não proíbem; proibido (Shopee [C], Amazon [I]) ou termos não verificados = **desativado** por padrão.

| Alvo | Barreira técnica | Termos | Risco principal |
|---|---|---|---|
| Shopee (site) | API web exige headers assinados de curta duração (`x-sap-ri`, `x-sap-sec`, `af-ac-enc-dat`), fingerprinting e login [T] | Proíbe robôs para "monitorar ou copiar conteúdo" (3.1) e acesso por bots à conta (6.2(k)) [C] | Bloqueio e quebra frequente; burlar assinatura agrava (6.2(n)) |
| Mercado Livre (site) | Anti-bot não verificado [I]; busca via API retorna 403 mesmo com token [T] | Termos não lidos (acesso bloqueado na pesquisa); historicamente proíbem interferir no sistema [I] | Bloqueio (⚠️ termos) |
| Amazon (site) | Bloqueio agressivo de tráfego automatizado [T] | Condições de Uso proíbem mineração de dados e robôs [I] | Perda da conta de Associados/vendedor |
| Painel logado (qualquer plataforma) | Cookies, captcha, 2FA | Hotmart 3.5(m) e Shopee 6.2(k) proíbem expressamente [C] | **Suspensão da sua própria conta**, e o app guardando credenciais de sessão vira passivo de segurança |

Situação jurídica no Brasil [T/I]: não há lei específica. O risco vem de violação contratual (termos), concorrência desleal (Lei 9.279/96, art. 195), proteção de base de dados (Lei 9.610/98) e LGPD quando há dados pessoais (nome de vendedor PF, avaliações). O precedente mais citado é o caso Catho x Curriculum (TJSP), em que a cópia automatizada de currículos burlando controle de acesso foi tratada como concorrência desleal, com indenização milionária [T, valor a conferir]. Dados de produto (preço, título, vendas) têm risco baixo de LGPD; o risco dominante é bloqueio e banimento.

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

**Recomendação técnica:** APIs oficiais primeiro (Shopee Affiliate API, ML highlights/catálogo), porque são estáveis e baratas de manter. Crawler de páginas públicas entra como fonte complementar atrás da mesma interface de `ProductSource`, com rate limit baixo, cache, circuit breaker e flag por plataforma (desligado onde os termos proíbem). Automação de painel logado segue a mesma regra (construída, desativada), com aviso explícito ao ativar: o risco é perder a conta de vendedor/afiliado, que é o ativo do negócio. Enquanto a Shopee API não for aprovada, JoomPulse (Shopee) e Metrify (ML) são o plano B pago.

## 2. APIs de vendedor

| | Mercado Livre | Shopee Open Platform | Amazon SP-API (BR) | Magalu |
|---|---|---|---|---|
| **Auth** | OAuth2 authorization code; access 6 h; **refresh token de uso único**, 6 meses [C] | `partner_id`/`partner_key`, HMAC-SHA256 por request; autorização da loja 7–365 dias; access 4 h [T] | LWA OAuth, refresh token; app privado exige conta Professional [C] | OAuth2 via ID Magalu; access 2 h [C] |
| **Registro/aprovação** | App livre; certificação opcional (Developer Partner Program) [C] | App passa por aprovação; tipo Individual Seller disponível [T] | Registro de developer + questionário de segurança [C] | Client via CLI `idm`; alguns scopes pendentes de aprovação [C] |
| **Anúncios, preço, estoque** | `/items`, catálogo, variações; estoque 0 pausa o anúncio [C] | Product module [T] | Listings Items [C] | SKUs, preço, estoque [C] |
| **Pedidos e envio** | `/orders`, `/shipments`, etiqueta PDF/ZPL; Flex, Full [C] | Order + Logistics (etiqueta, tracking) [T] | Orders, FBA (BR com restrições) [C] | Orders, deliveries, NF-e, Magalu Entregas [C] |
| **Financeiro** | Billing (comissão, frete, publicação por pedido) [C]; liberação via Mercado Pago [I] | Escrow, payout, income report [T] | Reports, Finances [C] | Análise Financeira [C] |
| **Webhooks** | Sim; retry por 1 h; recuperação via `/missed_feeds` [C] | Push com HMAC; `get_lost_push_message` [I] | SQS/EventBridge (ex.: `ORDER_CHANGE`) [C] | Sim; HMAC-SHA256 com timestamp; payload só com referência [C] |
| **Rate limit** | Não publicado; por client e endpoint; 429 [C] | ~10 req/s por loja [T] | Token bucket por operação; `searchOrders` 0,0056 req/s [C] | Não publicado [C] |
| **Dropshipping** | Permitido via "prazo de disponibilidade" (`MANUFACTURING_TIME`, até 45 dias) [T]; não funciona com Flex/Full; atraso e cancelamento pesam na reputação (verde no MLB: ≤ 2% reclamações, ≤ 1,5% cancelamentos, ≤ 10% despacho atrasado) [C, revalidar] | Pré-venda com DTS (faixa diverge: 3–15 ou 7–30 dias) [T] | Permitido só se você é o vendedor identificado; **proibido comprar de outro varejista online e mandar direto** [C] | Sem política explícita; exige NF-e e SLA [C] |

Pontos que afetam implementação:
- **ML refresh token de uso único:** precisa de lock por conta e gravação atômica do novo token; dois refreshes concorrentes derrubam a integração.
- **Webhooks do ML e Magalu só trazem a referência:** o padrão é responder 200 na hora, enfileirar e buscar o recurso depois, com reconciliação periódica por polling.

## 3. Modelos de operação e o que o app precisa

| Modelo | Fluxo | Integrações | Risco principal |
|---|---|---|---|
| **Afiliado** | Divulga link, recebe comissão | APIs de afiliado ([platform-integrations.md](platform-integrations.md)) | Baixo |
| **Revenda com estoque local** | Compra (Shopee, atacado, fornecedor) → recebe no espaço → anuncia no ML → despacha com etiqueta ML (ou Flex) | ML seller API; compra registrada manualmente | Capital parado em estoque (⚠️ fiscal, seção 6) |
| **Dropshipping nacional** | Anuncia → vende → compra no fornecedor → fornecedor envia | ML seller API + fornecedor com API (Dropify etc.); o fornecedor precisa imprimir a etiqueta ML do pedido [I] | Reputação (atraso/cancelamento); ruptura do fornecedor |
| **"Dropshipping" Shopee → ML** | Anuncia no ML → vende → compra na Shopee → recebe no espaço → reenvia | ML seller API; Shopee só como fonte de dados | Prazo (dois fretes), margem espremida; vira na prática revenda com estoque sob demanda |

O caso Shopee → ML se encaixa melhor como **revenda com estoque local**, comprando em lote o que a análise indicar, do que como dropshipping por pedido: o prazo de dois fretes pune a reputação no ML e anula a vantagem do Flex.

**Implicação no modelo de dados** (a ser detalhada nas specs):
- Núcleo comum: `Product` (canônico, SKU próprio), `SupplierOffer` (oferta de origem com preço e data), `ChannelListing` (anúncio por canal), `Order`/`OrderItem`, `PurchaseOrder` (compra ao fornecedor), `StockMovement` (ledger de entradas, saídas, ajustes; saldo é derivado), `Fee` (comissão do marketplace, frete, imposto).
- Afiliação continua como está: `Program`, `AffiliateLink`, `Conversion`, `Commission`, `Payout`.
- Margem por venda = preço − tarifas do canal − frete − custo de aquisição (custo médio do estoque) − impostos. Tudo em decimal com moeda.

## 4. Plano por fases

Tudo entra no planejamento desde o início (PRD, specs e issues das três fases); a execução segue a ordem abaixo. Cada fase é uma fatia vertical utilizável sozinha.

| Fase | Foco | Plataformas e fontes | Entrega |
|---|---|---|---|
| **1. Venda com estoque local + descoberta** | Revenda Shopee → ML | Mercado Livre (seller API, envio por coleta/agência Mercado Envios); Shopee Affiliate API e ML highlights/catálogo como fontes de descoberta; estoque local | Produtos da Shopee com preço e vendas cruzados com o que vende no ML, margem estimada; anúncios, pedidos, envio com etiqueta, tarifas; ledger de estoque do seu espaço; compras ao fornecedor registradas manualmente; margem real por venda |
| **2. Afiliação** | Dashboard de comissões | Hotmart, Shopee, Kiwify, Monetizze (API/webhook); Eduzz (a validar); Amazon por import de arquivo; ML e Magalu por import (se houver export) ou lançamento manual | Vendas, comissões com status derivado, estornos e repasses consolidados; links e campanhas |
| **3. Expansão e dropshipping** | Novos modos e canais | Dropshipping (flag), fornecedores com API (Dropify, AliExpress), Shopee e Magalu como canais de venda, Amazon SP-API, crawlers de descoberta e automação de painel (flag), encurtador próprio para cliques | Venda sem estoque com compra no fornecedor; multicanal; fontes extras de descoberta |

Por que nessa ordem:
- A fase 1 define o núcleo do modelo de dados (produto canônico, pedido, estoque, dinheiro); afiliação e expansão se apoiam nele.
- A fase 2 é só leitura e reconciliação, mais simples, e reaproveita o produto canônico e as credenciais da Shopee da fase 1.
- A fase 3 concentra o que tem mais risco operacional (dropshipping pune reputação) e os itens que nascem desativados por flag.

Pontos de atenção:
- A fase 1 depende da aprovação da Shopee Affiliate Open API para a descoberta. Até lá: ML highlights/catálogo para o lado ML; para o lado Shopee, JoomPulse (pago) ou cadastro manual de ofertas.
- Sem NF-e, Flex e Full ficam indisponíveis; a fase 1 assume envio por coleta/agência do Mercado Envios [I, confirmar exigência de NF no Flex].
- Os módulos de todas as fases existem no modelo desde a fase 1 onde forem núcleo (ex.: `fulfillment_mode = own_stock | dropship` já nasce no pedido), para não exigir migração estrutural depois.

## 5. Arquitetura: um app ou vários?

Afiliação e venda própria compartilham muito: catálogo de produtos, descoberta de oportunidades, credenciais de plataforma (a mesma credencial da Shopee serve à descoberta e à afiliação), dinheiro com moeda e o dashboard consolidado. Mas têm ciclos de vida diferentes: afiliação é só leitura e reconciliação; venda própria tem escrita (anúncios, estoque, pedidos) e estado físico.

| Opção | Prós | Contras |
|---|---|---|
| **A. Apps separados** (afiliação e comércio) | Isolamento total; deploy e falha independentes | Duplica catálogo, descoberta, credenciais, auth e dashboard; o painel consolidado vira integração entre apps; custo operacional dobrado para um usuário só |
| **B. Um app sem fronteiras internas** | Mais rápido no início | Afiliação e estoque se acoplam pelo banco; mudar um quebra o outro; difícil extrair depois |
| **C. Monólito modular** (recomendado) | Um deploy, um banco, um login; módulos com fronteira explícita; dá para extrair um módulo depois se precisar | Exige disciplina nas fronteiras (resolvida com testes de arquitetura) |

**Recomendação: C.** Módulos propostos:

- **Catalog & Discovery:** produto canônico, ofertas por fonte (API ou crawler), ranking de oportunidades. Usado pelos dois lados.
- **Affiliate:** programas, links, conversões, comissões, repasses.
- **Commerce:** anúncios por canal, pedidos, compras ao fornecedor, envios. Suporta modos `own_stock` e `dropship` na mesma estrutura (o modo muda de onde sai o item, não o modelo).
- **Inventory:** ledger de movimentos e locais de estoque (o espaço físico é um `StockLocation`). Separado de Commerce porque o estoque é compartilhado entre canais.
- **Integrations:** adapters por plataforma (API, webhook, import, crawler) e credenciais. Cada adapter publica eventos normalizados para os módulos acima.
- **Finance & Reporting:** consolida receita, comissões, tarifas e margem; dashboard e alertas.

Regras: cada módulo com schema próprio no banco; comunicação só por interface pública ou evento interno; nenhum módulo lê tabela de outro. Isso mantém a porta aberta para separar em apps no futuro sem pagar o custo agora.

## 6. Regra jurídica e itens com flag

Princípio: nada jurídico bloqueia planejamento ou issues, e o app não faz nada proibido. Onde a funcionalidade é proibida por lei ou pelos termos de uma plataforma, ela é **construída e testada, mas nasce desativada** por feature flag, com o motivo registrado. Quando houver permissão, mudança de termos ou decisão sua, basta ativar.

| Item | Situação | Tratamento |
|---|---|---|
| Crawler Shopee | Termos proíbem robôs (3.1, 6.2(k)) [C] | Construído, **desativado** |
| Crawler Amazon | Condições de Uso proíbem mineração de dados [I] | Construído, **desativado** |
| Crawler Mercado Livre | Termos não lidos [I] | Construído, desativado até ler os termos |
| Crawler de vitrines de infoproduto (Hotmart, Kiwify, Eduzz, Monetizze) | Hotmart proíbe robôs (3.5(m)) [C]; demais sem cláusula encontrada ou não lidos [I] | Construído, **desativado** (Hotmart); demais desativados até ler os termos |
| Crawler Magalu | Sem cláusula encontrada no termo de compra [C]; demais termos não lidos | Construído, desativado até ler os termos |
| Automação de painel logado (qualquer plataforma) | Proibida nos termos da Hotmart e Shopee [C]; risco de perder a conta | Construído na fase 3 (baixa prioridade), **desativado**, com aviso de risco ao ativar |
| Amazon como canal de venda com compra em outro varejista | Proibido pela política de dropshipping da Amazon [C] | Flag `dropship` do canal Amazon construída e **desativada**; venda com estoque próprio liberada |
| Dropshipping no ML e Shopee | Permitido com prazo de disponibilidade/pré-venda [T] (flag de produto, não jurídica) | Construído na fase 3, desativado por padrão até você decidir usar |
| Venda sem CNPJ/NF-e | ⚠️ Lembrete fiscal: pode gerar questionamento e algumas modalidades (Full, Flex) exigem NF | Não bloqueia; lembrete no painel quando o volume de vendas passar de um limite configurável |
| LGPD (dados de compradores) | ⚠️ Lembrete: nome, endereço e telefone são dados pessoais | Guardar só o necessário para despacho e suporte, com retenção configurável |

## 7. Decisões tomadas e próximos passos

Decididas por você em 30/09/2026:
1. **Plano por fases** 1 → 2 → 3, com as três no planejamento desde o início.
2. **Monólito modular** (seção 5).
3. **Crawling** como fonte complementar, com flag por plataforma, desativado onde os termos proíbem.
4. **Regra jurídica** da seção 6.

Próximos passos:
- Especificação no fluxo spec-driven das skills do Matt Pocock: grilling do que faltar, PRD, specs e issues por fase.
- Pedir acesso à Shopee Affiliate Open API já, porque a aprovação tem relatos de semanas.
- Validar com conta real os itens marcados [I]/[T] antes de implementar cada adapter.

## Fontes principais

- ML: https://developers.mercadolibre.com.ar/en_us/authentication-and-authorization · https://developers.mercadolibre.com.ar/en_us/products-receive-notifications · https://developers.mercadolibre.com.ar/en_us/sellers-reputation · https://developers.mercadolibre.com.ar/en_us/billing-reports · https://developers.mercadolibre.com.ar/en_us/best-sellers-in-mercado-libre · https://developers.mercadolibre.com.mx/en_us/rate-limit-429-error
- Shopee: https://help.shopee.com.br/portal/4/article/77113 (termos) · https://open.shopee.com/developer-guide/20 · https://docs.celigo.com/hc/en-us/articles/18977971017883-Available-Shopee-APIs
- Amazon: https://developer-docs.amazon/sp-api/docs/sp-api-registration-overview · https://developer-docs.amazon/sp-api/docs/usage-plans-and-rate-limits · https://venda.amazon.com.br/sellerblog/dropshipping
- Magalu: https://developers.magalu.com/docs/apis/ · https://developers.magalu.com/docs/development-guide/webhooks/index.html · https://universo.magalu.com/blog/artigo/acordo-de-nivel
- Hotmart (termos): https://hotmart.com/pt-br/legal/termos-de-uso
- Dados de mercado: https://metrify.com.br/ · https://www.real-trends.com/br/precos · https://joompulse.com/shopee
- Fornecedores: https://dropify.com.br/api · https://dev.to/zuplo/a-developers-guide-to-the-aliexpress-api-4f45
