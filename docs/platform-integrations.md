# Integrações oficiais para afiliados: levantamento

> Pesquisa feita em 30/09/2026 na documentação pública de cada plataforma. Marcações: **[C]** confirmado em fonte oficial; **[T]** fonte de terceiros (SDK, blog, Reclame Aqui); **[I]** inferido ou não verificado. Todo item [I]/[T] deve ser validado com conta real antes de virar código.

## TL;DR

- **Só 3 das 8 plataformas expõem dados do afiliado por API oficial:** Hotmart, Monetizze e Shopee. Eduzz e Kiwify têm APIs boas, mas desenhadas para o produtor; o que um token de afiliado enxerga ainda precisa ser testado.
- **Amazon** tem API, mas apenas de **catálogo** (Creators API, que substituiu a PA-API 5). Ganhos, pedidos e cliques só saem por relatório exportado.
- **Mercado Livre e Magalu não têm API para afiliados.** O painel é a única fonte; scraping ou automação com cookie de sessão é frágil e arrisca violar os termos.
- **Nenhuma plataforma expõe marketplace/catálogo de afiliação de infoprodutos por API** (Hotmart, Kiwify, Eduzz, Monetizze). O objetivo "descoberta de oportunidades" para produtos digitais não é atendível por integração oficial hoje; só Shopee e Amazon têm catálogo consultável.
- **Cliques** praticamente não existem via API em lugar nenhum. Vão depender de encurtador próprio ou import de relatório.
- **Nenhuma plataforma tem "status de comissão" nativo** (pendente/aprovada/paga/estornada). O status precisa ser derivado do status da transação + regras de garantia/liberação de cada uma. Isso é responsabilidade do adapter.
- **Consequência arquitetural:** o import de arquivo (CSV/XLSX) não é um "plano B", é um modo de ingestão de primeira classe. 5 das 8 plataformas dependem dele para receita.

## Matriz resumida

| Plataforma | API para afiliado | Auth | Rate limit | Vendas/comissões | Estornos | Cliques | Catálogo/descoberta | Links | Pagamentos | Webhook p/ afiliado | Import manual |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Hotmart** | Sim [C] (`commission_as=AFFILIATE`) | OAuth2 client credentials | 500 req/min [C] | Sim [C] | Via status da transação [C] | Não | Não (só painel) | Não | Não encontrado | Parcial [I] | CSV/XLS [C] |
| **Kiwify** | A validar [I] (API do produtor) | OAuth2 (API Key → Bearer) | 100 req/min [C] | A validar | Via status [C] | Não | Não | Não | `/finance` a validar | **Sim** [C] | CSV/XLS [C] |
| **Eduzz** | A validar [I] (API do produtor) | OAuth2 authorization code; app precisa de aprovação | 30 req/min em `/sales` [C] | Extrato inclui venda de afiliado [C]; `/sales` a validar | Chargebacks endpoint [C] | Não | Não | Não | Transfers [C] | Provavelmente não [I] | Não documentado |
| **Monetizze** | **Sim** [C] | Token via chave do painel (fluxo de troca não documentado [I]) | Não documentado | Sim, `/transactions` com `comissoes[]` [C] | Status 4 "Devolvida" [C] | Não | Não (só produtos já afiliados) | Não | Não documentado | **Sim**, postback tipo 4 [C] | Não documentado |
| **Shopee** | **Sim** [T] (GraphQL, acesso sob aprovação) | AppID + SHA256(AppId+ts+payload+secret) | Não publicado (erro 10030) | Sim, `conversionReport` e `validatedReport` [T] | `orderStatus`/`fraudStatus` [T] | Não (só `clickTime` por conversão) | **Sim**, `productOfferV2`/`shopOfferV2` [T] | **Sim**, `generateShortLink` [T] | Não | Não (polling) | CSV [I] |
| **Amazon BR** | Só catálogo [C] (Creators API) | OAuth2 client credentials (LWA) | 1 TPS / 8.640 TPD iniciais, escala com receita, teto 10 TPS [C] | Não (só relatório) | Relatório | Relatório | **Sim** [C] (exige 10 vendas/30 dias) | Tag na URL [C] | Relatório | Não | xlsx/CSV/XML [C] |
| **Mercado Livre** | **Não** [T] | n/a | n/a | Só painel | Só painel | Só painel | Busca pública bloqueada/instável em 2026 [T] | Só painel | Só painel (Mercado Pago) | Não | Não confirmado |
| **Magalu** | **Não** [C/I] | n/a | n/a | Só painel | Só painel | Só painel (15 dias) | Não | Só painel | Só painel | Não | Não documentado |

## Detalhe por plataforma

### Hotmart

- **API:** REST (Hotmart Developers). `GET /payments/api/v1/sales/history`, `/sales/commissions` e `/sales/summary` aceitam `commission_as=PRODUCER|COPRODUCER|AFFILIATE` [C]. [sales history](https://developers.hotmart.com/docs/pt-BR/v1/sales/sales-history/), [commissions](https://developers.hotmart.com/docs/pt-BR/v1/sales/sales-commissions/)
- **Auth:** OAuth2 client credentials; credencial criada em Ferramentas > Credenciais, exibida uma única vez; token com `expires_in` [C]. [app-auth](https://developers.hotmart.com/docs/pt-BR/start/app-auth/)
- **Rate limit:** 500 chamadas/min, 429 com headers `RateLimit-*` [C]. [rate-limit](https://developers.hotmart.com/docs/pt-BR/start/rate-limit/)
- **Status de transação:** APPROVED, COMPLETE, CANCELLED, REFUNDED, PARTIALLY_REFUNDED, CHARGEBACK, BLOCKED, WAITING_PAYMENT e outros. **Sem filtro, a API retorna só APPROVED e COMPLETE**; o sync precisa pedir os demais explicitamente para capturar estornos [C].
- **Webhook 2.0:** PURCHASE_APPROVED/COMPLETE/CANCELED/REFUNDED/CHARGEBACK/etc., payload com `commissions[]` e `affiliates[]`, validação por header `X-HOTMART-HOTTOK` [C]. Se o afiliado cadastra webhook próprio para todos os eventos não está claro [I]. [purchase webhook](https://developers.hotmart.com/docs/pt-BR/2.0.0/webhook/purchase-webhook/)
- **Fora da API:** Mercado de Afiliação e "temperatura", HotLinks, cliques (Hotmart Analytics), saldo e saques.
- **Termos:** proíbem robôs/scripts/spiders sobre a plataforma (3.5(m)) [C]. Scraping do marketplace está fora. [termos](https://hotmart.com/pt-br/legal/termos-de-uso)
- **Validar:** conta só de afiliado consegue gerar credencial? Webhook de afiliado recebe quais eventos?

### Kiwify

- **API:** REST em `https://public-api.kiwify.com/v1` com OpenAPI [C]. Produtos, vendas (janela máx. 90 dias por consulta, `start_date`/`end_date` obrigatórios), `/stats`, `/finance` (saldos, saques), `/affiliates`, `/webhooks` [C]. [docs](https://docs.kiwify.com.br/)
- **Escopo:** desenhada para o produtor; `/affiliates` lista os afiliados *do produtor*. Não está documentado se `/sales` de uma conta afiliada devolve as vendas onde ela é afiliada [I].
- **Auth:** API Key com escopos → `POST /oauth/token` → Bearer + header `x-kiwify-account-id` [C]. Validade do token inconsistente na doc (96 h vs. 24 h); usar `expires_in`.
- **Rate limit:** 100 req/min por usuário [C].
- **Webhook:** **afiliado pode usar** ("Sim! Você pode utilizar webhooks como afiliado") [C]. Triggers: compra_aprovada, compra_reembolsada, chargeback, compra_recusada, pix/boleto gerado, assinaturas [C]. Assinatura: historicamente `?signature=` HMAC-SHA1 com o token [I, validar]. [webhooks](https://ajuda.kiwify.com.br/pt-br/article/como-funcionam-os-webhooks-2ydtgl/)
- **Export:** Vendas > Exportar em CSV/XLS, com `deposit_status` (disponibilidade do valor) [C].
- **Fora da API:** marketplace/ranking, links de afiliado, cliques.

### Eduzz

- **API:** Developer Hub, REST em `https://api.eduzz.com` com OpenAPI público e MCP server [C]. MyEduzz: `/sales`, `/sales/chargebacks`, `/v2/financial/statement`, `/financial/transfers`, `/financial/affiliate/fiscal-documents` [C]. [endpoints](https://developers.eduzz.com/api/endpoints)
- **Escopo:** descrições falam do produtor. O extrato tem tipo `affiliante_sale` (sic), o que sugere que o próprio extrato do afiliado traz as comissões [C, leitura parcial]. `/sales` para afiliado: a testar [I].
- **Auth:** OAuth2 authorization code via app no Console; **app multiusuário em produção precisa de aprovação da Eduzz** [C]. Personal token existe, mas a doc diz que é só para testes [C]. Para uso próprio (1–2 contas) o personal token resolve, ciente do aviso.
- **Rate limit:** 30 req/min em `/sales` [C].
- **Webhook:** HMAC-SHA256 no header `x-signature` [C], mas "enviará eventos apenas dos produtos pertencentes a esta conta" [C], ou seja, provavelmente inútil para afiliado.
- **Fora da API:** Vitrine, afiliações, links, cliques.

### Monetizze

- **API 2.1:** a doc diz explicitamente que serve a "Produtores e afiliados" [C]. `GET /transactions` (filtros de data, status 1–6, 100/página, retorna `comissoes[]` com `refAfiliado`, `valor`, `porcentagem`) e `GET /myproducts?retorno_produtor=0` (produtos onde sou afiliado ativo) [C]. [apidoc](https://api.monetizze.com.br/2.1/apidoc/)
- **Auth:** chave gerada em Ferramentas > API, trocada por token temporário; o endpoint de troca (`/2.1/token` com `X_CONSUMER_KEY`) não aparece na doc atual [I].
- **Rate limit:** não documentado.
- **Postback/Webhook:** afiliado recebe [C]; tipos 4 = Afiliado, 5 = Afiliado Premium etc. Form-encoded; validação comparando `chave_unica`, **sem HMAC** [C]. Existe doc nova "Webhook (antigo Postback)" em `apidoc.monetizze.com.br` que não pôde ser lida (403) e pode ter mudado o formato [I].
- **Fora da API:** vitrine, cliques, links, extrato/saques (citado na intro da doc, sem endpoint).
- **Risco:** API sem evolução desde 2023, doc parcialmente inacessível.

### Shopee Afiliados

- **API:** Shopee Affiliate Open API, GraphQL em `POST https://open-api.affiliate.shopee.com.br/graphql` [T]. Documentação só dentro do painel logado; detalhes vêm de SDKs públicos.
- **Acesso:** solicitado no painel (Open API); aprovação manual, com relatos de atraso em 2026 [C via Reclame Aqui].
- **Auth:** `Authorization: SHA256 Credential={AppId}, Timestamp={ts}, Signature={sha256(AppId+ts+payload+secret)}`, tolerância ~5 min [T].
- **Operações** [T]:
  - `productOfferV2`, `shopOfferV2`: catálogo com comissão, preço, vendas, ordenação por maior comissão.
  - `generateShortLink`: link curto com até 5 `subIds` (útil para rastrear canal/campanha).
  - `conversionReport`: conversões com `orderStatus` (UNPAID/PENDING/COMPLETED/CANCELLED), `fraudStatus`, `totalCommission`, `netCommission`. **Janela de ~3 meses**; paginação por `scrollId` que expira em 30 s.
  - `validatedReport`: conversões validadas para pagamento. É o mais próximo de "comissão aprovada".
- **Rate limit:** não publicado; erro 10030 [T].
- **Fora da API:** cliques agregados, pagamentos, histórico > 3 meses. Sem webhooks.

### Amazon Associados BR

- **API:** **Creators API** substituiu a PA-API 5 (que já responde 403) [C]. Só catálogo: `SearchItems`, `GetItems`, `GetVariations`, `GetBrowseNodes`, preço via `OffersV2`; retorna URL já com a tag [C]. [intro](https://associados.amazon.com.br/creatorsapi/docs/en-us/introduction), [deprecação PA-API](https://affiliate-program.amazon.com/creatorsapi/docs/en-us/paapiv5-deprecation)
- **Elegibilidade:** **10 vendas qualificadas nos últimos 30 dias**; perde acesso após 30 dias sem venda [C].
- **Auth:** OAuth2 client credentials (LWA, scope `creatorsapi::default`), token de 1 h [C].
- **Rate limit:** 1 TPS / 8.640 TPD nos primeiros 30 dias; depois escala com a receita, teto 10 TPS [C]. [api-rates](https://affiliate-program.amazon.com/creatorsapi/docs/en-us/concepts/api-rates)
- **Termos que afetam o modelo de dados** [C]: não armazenar imagens; demais dados de catálogo por no máximo 24 h; preço exige timestamp ou refresh horário; proíbe usar o conteúdo para treinar LLM. [políticas](https://associados.amazon.com.br/help/operating/policies)
- **Ganhos, pedidos, cliques, pagamentos:** só relatórios no Associates Central, exportáveis em **xlsx, CSV e XML** [C].

### Mercado Livre

- **Sem API de afiliados** [T, várias fontes convergentes]. Ferramentas de terceiros geram link usando cookie de sessão do painel (endpoints internos) [T]: frágil e provavelmente contra os termos.
- A API geral (OAuth2) é de vendedor. A busca pública `/sites/MLB/search` passou a exigir token e há muitos relatos de 403 mesmo autenticado em 2026 [T].
- Comissões, cliques e pagamentos: só painel; exportação não confirmada.

### Magalu (Influenciador Magalu, ex-Parceiro Magalu / Magazine Você)

- **Sem API de afiliados** [C/I]. `developers.magalu.com` cobre só sellers.
- Painel mostra vendas, comissões (aparecem perto da data de pagamento), acessos dos últimos 15 dias. Pagamento dias 4 e 19, mínimo R$ 50, retenção de INSS 11% e IR [C, central de ajuda].
- Exportação CSV não documentada; possivelmente só digitação manual.

## Implicações para o design

1. **Três modos de ingestão, um contrato.** Cada adapter declara capacidades (`webhook`, `polling`, `fileImport`, `catalog`, `linkGeneration`) em vez de assumir que toda plataforma tem API. O núcleo trata os três modos da mesma forma: eventos brutos → normalização → upsert idempotente.
2. **Chave natural por plataforma para idempotência.** Hotmart: `transaction`; Kiwify: `order_id`; Monetizze: `venda.codigo` + `chave_unica`; Shopee: `conversionId` + item; Amazon (arquivo): combinação de colunas do relatório (ASIN + data + tracking id + ...), a definir ao ver um arquivo real. Reimportar o mesmo arquivo ou reprocessar o mesmo webhook não pode duplicar.
3. **Status de comissão derivado.** Máquina de estados própria (pendente → aprovada → paga, com estornada a partir de qualquer estado), alimentada pelo status bruto da plataforma + regras de prazo de garantia. Guardar o status bruto e o histórico de transições.
4. **Guardar o dado bruto.** Shopee só consulta ~3 meses, Kiwify 90 dias por chamada, Magalu mostra acessos de 15 dias. O app vira o sistema de registro; o payload original deve ficar persistido para reprocessamento.
5. **Cache de catálogo com TTL por plataforma.** Amazon exige ≤ 24 h e proíbe armazenar imagens. O modelo de "produto descoberto" precisa suportar expiração.
6. **Cliques via encurtador próprio.** Como nenhuma plataforma entrega cliques por API (Shopee só `clickTime` por conversão), um redirecionador próprio com UTM/subId é a única forma de ter cliques por canal de forma uniforme. Entra na fase 3, mas o modelo de `link` deve prever isso desde o início.
7. **Webhooks exigem endpoint público.** Hotmart, Kiwify e Monetizze entregam melhor via webhook. Isso influencia hosting (precisa de URL HTTPS pública e fila para retry), mas não força stack. Alternativa na fase 2: começar só com polling e adicionar webhooks depois.

## Proposta original de MVP (só afiliação)

> **Substituída.** O escopo passou a incluir venda própria (estoque local e dropshipping). O plano vigente está em [commerce-integrations.md](commerce-integrations.md#4-plano-por-fases). A análise abaixo continua válida para a fatia de afiliação.

Critério: maximizar dado real ingerido com o menor número de adapters, e exercitar cada modo de ingestão uma vez para validar o núcleo agnóstico antes de escalar.

### Opção A (recomendada): Hotmart + Shopee + Amazon via import

| Plataforma | Por quê | Modo |
|---|---|---|
| **Hotmart** | Maior player de infoproduto; API confirmada para afiliado, rate limit folgado, webhook com hottok | Polling (`sales/history` + `commissions`), webhook depois |
| **Shopee** | Única plataforma de físico com API real de conversões, catálogo e geração de link; cobre os objetivos 1 e 2 | Polling GraphQL |
| **Amazon** | Maior marketplace; ganhos só por arquivo, então valida o pipeline de import que servirá depois para ML, Magalu e lacunas das outras | Import de CSV/XLSX; Creators API (catálogo) só se houver elegibilidade |

- **Prós:** exercita API REST, API GraphQL com assinatura e import de arquivo; cobre digital e físico; cobre descoberta (Shopee) e gestão (as três).
- **Contras:** Shopee depende de aprovação de acesso (atrasos relatados); Amazon Creators API exige 10 vendas/30 dias, então o catálogo Amazon pode ficar de fora no início.

### Opção B: Hotmart + Monetizze + Kiwify

- **Prós:** as três entregam dados de afiliado por API e/ou webhook; foco em infoproduto, domínio mais homogêneo; mais rápido de ter dashboard financeiro.
- **Contras:** não valida import de arquivo nem produto físico; nenhuma entrega catálogo, então o objetivo de descoberta fica sem dado; Kiwify ainda precisa de teste com conta de afiliado.

### Opção C: priorizar onde já existe receita

- Se a operação atual está concentrada em 2–3 plataformas específicas, começar por elas vale mais que o critério técnico. Plataformas sem API (ML, Magalu) entram só com import/digitação.

**Recomendação:** Opção A, desde que Hotmart, Shopee e Amazon façam parte da operação real. Se a receita estiver toda em infoproduto, Opção B.

## Validações antes de codar

Todas com conta real de afiliado, baratas e que podem mudar a escolha:

1. **Hotmart:** conta de afiliado gera credencial em Ferramentas > Credenciais? `sales/history?commission_as=AFFILIATE` retorna as vendas esperadas?
2. **Shopee:** solicitar acesso à Open API já (aprovação pode levar semanas). Confirmar rate limit e schema na doc do painel.
3. **Amazon:** exportar um relatório de ganhos e um de pedidos (CSV e XLSX) para desenhar o parser e a chave natural.
4. **Kiwify / Eduzz** (se Opção B): testar `/sales` e `/finance` com token de conta afiliada.
5. **Monetizze** (se Opção B): confirmar fluxo de token e formato atual do webhook.

## Fontes principais

- Hotmart: https://developers.hotmart.com/docs/pt-BR/ · https://hotmart.com/pt-br/legal/termos-de-uso · https://suportehotmart.zendesk.com/hc/pt-br/articles/360001491352
- Kiwify: https://docs.kiwify.com.br/ · https://ajuda.kiwify.com.br/pt-br/article/como-funcionam-os-webhooks-2ydtgl/ · https://ajuda.kiwify.com.br/pt-br/article/como-exportar-suas-vendas-1e27zef/
- Eduzz: https://developers.eduzz.com/ · https://developers.eduzz.com/api/openapi · https://developers.eduzz.com/docs/webhook/security
- Monetizze: https://api.monetizze.com.br/2.1/apidoc/ · https://help.monetizze.com.br/books/integracoes/page/postback · https://github.com/Monetizze/ExemploPOSTCallback
- Shopee: https://affiliate.shopee.com.br/open_api (login) · https://www.nuget.org/packages/Shopee.Affiliate · https://github.com/liamtran96/shopee-aff
- Amazon: https://associados.amazon.com.br/creatorsapi/docs/en-us/introduction · https://affiliate-program.amazon.com/creatorsapi/docs/en-us/concepts/api-rates · https://associados.amazon.com.br/help/operating/policies
- Mercado Livre: https://developers.mercadolivre.com.br/itens-e-buscas · https://www.reclameaqui.com.br/mercado-livre/programa-de-afiliados-do-mercado-livre-nao-tem-uma-api_-lfESpIamuDGm2ro/
- Magalu: https://developers.magalu.com/ · https://divulgador-magalu.zendesk.com/hc/pt-br
