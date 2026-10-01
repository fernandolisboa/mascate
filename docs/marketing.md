# Marketing: o que move resultado e o que o app deve ter

> Pesquisa feita em 01/10/2026. Complementa [commerce-integrations.md](commerce-integrations.md) e [platform-integrations.md](platform-integrations.md). Marcações: **[C]** confirmado em fonte oficial; **[T]** terceiros; **[I]** inferido ou não verificado. Nada aqui é parecer jurídico ou fiscal; a regra do projeto continua: jurídico/fiscal vira lembrete ⚠️, proibido por termos nasce **desativado por flag**.

## TL;DR

- **Vendedor pequeno no ML: marketing é, antes de tudo, o próprio anúncio.** A busca do ML ordena por relevância da palavra-chave, qualidade do anúncio (fotos, ficha técnica, categoria), preço e reputação/experiência de compra [C]. Tudo isso é grátis e quase tudo é legível por API (`/item/{id}/performance`, `/trends`, perguntas, opiniões) [C]. É onde o app mais ajuda na fase 1.
- **Publicidade paga (Product Ads) não serve no início:** exige reputação amarela ou melhor e um mínimo de vendas [C, valores divergem]; é CPC com meta de ROAS [C]. A API documentada para terceiros é **só de leitura** (campanhas e métricas) [C]; criar/editar campanha fica no painel do ML [I]. O app só precisa ler o custo e somar na margem.
- **Promoções do vendedor existem por API** (desconto individual, campanha do vendedor até 14 dias, cupom até 31 dias), mas **exigem reputação verde** [C]. Entram na fase 1 como ação guardada por margem mínima, liberada quando a reputação permitir.
- **Afiliado solo no Brasil vive de grupos/canais de oferta (WhatsApp, Telegram) e vídeo curto** [T]. A única automação ao mesmo tempo grátis, oficial e permitida é **Telegram Bot API num canal próprio** [C]. WhatsApp oficial (Cloud API) não serve para grupos de oferta: grupos têm no máximo **8 participantes** e mensagem de marketing custa ~US$ 0,07 cada [C]; automação não oficial viola os termos [C] e fica atrás de flag.
- **TikTok por API não é viável para um app pessoal:** cliente não auditado só publica em modo privado, e a auditoria recusa "ferramenta para subir conteúdo nas contas que você gerencia" [C]. Instagram por API é viável (até 100 posts/24 h, conta profissional) [C], mas exige mídia em URL pública, o que um app desktop não tem de graça. Ambos ficam manuais (o app gera o post, você cola) até haver volume.
- **Proibições que importam:** Shopee proíbe anúncio pago em busca/shopping (Google/Bing Ads) e palavra-chave com a marca, mas permite impulsionar post nas redes do próprio afiliado [C]; Amazon proíbe lance em "amazon", ação offline, encurtador que esconda o redirecionamento e mensagem não solicitada, e exige data/hora junto do preço [C]; Hotmart deixa cada produtor definir regras de divulgação e pune spam com bloqueio [C]; Google Ads reprova página-ponte e promessa de resultado improvável [C].
- **Escopo proposto (seção 5):** fase 1 = qualidade do anúncio, perguntas, preço/promoção com margem, leitura de Ads; fase 2 = gerador de post de oferta com link rastreado por canal, Telegram automático, regras por programa, comissão por canal; fase 3 = Instagram/Pinterest por API, encurtador com cliques. Fora: blog/SEO, gestão de Google/Meta Ads, e-mail, TikTok por API, WhatsApp Cloud API.

## 1. Vendedor pequeno no Mercado Livre: alavancas

| Alavanca | O que move | API | Custo | Fonte |
|---|---|---|---|---|
| **Qualidade do anúncio** (Básica/Satisfatória/Profissional, 0–100) | Exposição na busca; mostra ações pendentes: GTIN, nº de fotos (mín. 3), título, ficha técnica obrigatória, prazo de disponibilidade | `GET /item/{id}/performance` com `buckets` → `variables` → `rules` e link para corrigir; substituiu `/health` | Grátis | [C] |
| **Título e palavras-chave** | A busca mostra primeiro os produtos que contêm os termos buscados | `GET /trends/MLB` (50 termos em alta, semanal, por categoria) | Grátis | [C] busca por palavra [T] |
| **Fotos** | Foto fora do padrão tira o anúncio dos primeiros resultados; fundo branco obrigatório em Tecnologia, Beleza, Saúde, Supermercado; sem logo, marca d'água, texto; 1200×1200 recomendado; até 10 por variação | Upload de imagens e diagnóstico de imagens | Grátis (seu tempo) | [C] |
| **Categoria e ficha técnica** | Anúncio na categoria errada perde visibilidade; dados completos alimentam filtros | Atributos/categorias, validador de publicações | Grátis | [C] |
| **Preço competitivo** | Preço e desconto colocam o item na seção Ofertas; há ajuste automático de preço com mín./máx. | Automatizações de preço; `/seller-promotions` | Margem | [C] |
| **Reputação e experiência de compra** | Vermelha = menor exposição; despachar < 24 h, evitar cancelamento e reclamação | Reputação do vendedor, experiência de compra | Operação | [C] |
| **Perguntas** | ML pede comunicação "clara e rápida"; responder na primeira hora ajuda na busca | `/questions/search?seller_id=…&api_version=4`, responder via `POST /answers` | Grátis | [C] orientação; efeito na busca [T] |
| **Opiniões (avaliações)** | Prova social; nota média por item | `GET /reviews/item/{id}` | Grátis | [C] |
| **Promoções do vendedor** | Desconto individual (`PRICE_DISCOUNT`), campanha do vendedor (`SELLER_CAMPAIGN`, ≤ 14 dias), cupom (`SELLER_COUPON_CAMPAIGN`, ≤ 31 dias, só MLB), além de convites do ML (DEAL, co-participação, relâmpago) | `POST/PUT/DELETE /seller-promotions/promotions` | Desconto concedido | [C] **requer reputação verde, item novo e ativo, exposição não gratuita** |
| **Product Ads** | Posição patrocinada na busca; CPC (paga por clique), custo varia por categoria e preço; meta de ROAS 1x–35x (ROAS substituiu ACOS como padrão em 2026) | `/advertising/...` documentado como **leitura** (anunciantes, campanhas, ad groups, métricas: cliques, custo, CPC, ROAS, vendas diretas/indiretas/orgânicas) e bonificações | CPC + orçamento diário | [C]; escrita por API [I] |
| **Programa Decola** | Benefícios para vendedor novo nos primeiros passos | Endpoint "Programa Decola" | Grátis | [C] existência; detalhes [I] |

Requisitos de Product Ads divergem entre páginas oficiais: "reputação amarela, cadastro > 7 dias, ≥ 20 vendas, sem fatura vencida" (Central de Vendedores) [C] contra "15 dias, 1 venda (empresa) / 10 (pessoa)" em outra página [T]. Validar na conta.

**Leitura para o app:** na fase 1 o vendedor é novo, sem reputação verde e sem acesso a Ads. O que dá resultado é anúncio completo, preço certo, despacho rápido e pergunta respondida rápido. Ads e promoções viram relevantes depois das primeiras vendas, e aí a pergunta útil é "isso paga?": ROAS de equilíbrio = 1 / margem de contribuição antes de Ads (margem de 25% ⇒ ROAS mínimo 4x) [I, aritmética].

## 2. Afiliado solo no Brasil: canais

| Canal | Funciona para solo? | Automação oficial | Limites/regras | Fonte |
|---|---|---|---|---|
| **Grupos/canais de oferta no WhatsApp** | Principal canal de afiliado Shopee/ML/Amazon [T] | Nenhuma útil: Cloud API só cria grupos de até 8 participantes, exige Official Business Account; marketing ~US$ 0,07188/msg no BR, cobrança em BRL a partir de 01/07/2026 | Termos proíbem bulk/auto-messaging e clientes não autorizados; promoção exige opt-in | [C] (preço US$ [T]) |
| **Canal/grupo no Telegram** | Segundo canal de ofertas [T] | **Bot API** grátis; bot admin posta em canal | ~1 msg/s por chat, 20 msg/min em grupo, ~30 msg/s em broadcast | [C] |
| **Instagram (Reels, stories, carrossel)** | Bom para nicho e prova social [T]; legenda não tem link clicável, usa bio/sticker [T] | Graph API Content Publishing: conta profissional, permissão `instagram_content_publish`, até 100 posts/24 h, mídia precisa estar em URL pública | Shopee permite impulsionar post da conta cadastrada | [C] |
| **TikTok (vídeo curto)** | Forte para produto físico barato [T] | Content Posting API: não auditado = só privado, até 5 usuários/24 h; auditoria **recusa ferramenta de uso próprio**; app não pode sobrepor link ou texto promocional | Divulgação comercial obrigatória no fluxo | [C] |
| **Pinterest** | Tráfego de cauda longa, lento [T] | API v5: Trial cria pin visível só para você, 300 escritas/dia; Standard exige vídeo-demo do app | Shopee permite impulsionar | [C] |
| **Blog/SEO** | Lento (meses), exige conteúdo original | n/a | Também é o que o Google Ads exige como página de destino | [I] |
| **Google Ads** | Possível para infoproduto se o produtor permitir; proibido para links Shopee | Ads API (fora do escopo) | Reprova página-ponte e promessa improvável (renda, emagrecimento) | [C] |

## 3. O que cada plataforma proíbe (afeta o app)

| Plataforma | Proibição relevante | Fonte |
|---|---|---|
| Shopee Afiliados | Anúncio pago em search/shopping (Google Ads, Bing Ads) para links de afiliado (4.3(i)); palavra-chave SEM com a marca Shopee, que deve ser negativada (4.3(c)); e-mail publicitário sem consentimento (4.3(a)); iframe, pop-up, cookie dropping (4.4); autocompra (3.6(c)); alterar o link de afiliado (2.2). **Permitido:** impulsionar post no Instagram, TikTok, Facebook, Pinterest, YouTube a partir da conta cadastrada (2.3(b)) | [C] |
| Amazon Associados BR | Lance em palavras com "amazon"/"Kindle"; qualquer promoção offline; e-mail/SMS/DM só se solicitados; encurtador que não deixe claro o redirecionamento; usar conteúdo para spam; preço exige data/hora ao lado e aviso de que pode mudar | [C] |
| Hotmart | Spam bloqueia a conta; não agir em nome da Hotmart; **regras de divulgação definidas por produto pelo produtor** (ex.: proibir Google Ads com o nome do produto) | [C] |
| Mercado Livre Afiliados | Spam em comentários, redirecionamento enganoso, autocompra | [T] |
| WhatsApp | Bulk messaging, auto-messaging, contas/clientes por meios automatizados não autorizados | [C] |
| TikTok | App não sobrepõe marca, link ou texto promocional; usuário escolhe privacidade sem padrão | [C] |
| Google Ads | Página-ponte/doorway; conteúdo copiado; promessas de resultado improvável | [C] |
| CONAR (guia de influenciadores, vigente desde 01/06/2026) | Publicidade identificada de forma clara e imediata (#publi, "parceria paga"); passa a regular modelos de afiliação | [T] |
| Mercado Livre (vendedor) | Dados de contato/links externos no anúncio e nas respostas | [I] |

## 4. APIs de postagem: resumo para decisão

| API | Serve ao Mascate? | Custo | Bloqueio principal |
|---|---|---|---|
| Telegram Bot API | **Sim**, desde já | Grátis | Nenhum; respeitar rate limit |
| Instagram Graph API | Sim, com trabalho | Grátis | Mídia em URL pública (precisa de hospedagem temporária); app Meta em modo dev só com a própria conta dispensa App Review [I] |
| Pinterest API v5 | Talvez | Grátis | Standard access para pin público |
| TikTok Content Posting | Não | Grátis | Auditoria recusa uso próprio; não auditado = privado |
| WhatsApp Cloud API | Não para ofertas | ~US$ 0,07/msg marketing | Grupos ≤ 8; opt-in por contato |
| WhatsApp não oficial (WhatsApp Web) | Tecnicamente sim | Grátis | **Viola os termos**; risco de banir o número |

## 5. Escopo proposto

Critério: só entra o que o dono, sem saber marketing, usa toda semana e que mexe em venda ou comissão; o resto fica fora até haver dado provando que falta.

### Fase 1: revenda no ML

| Item | Por quê |
|---|---|
| **Painel de qualidade do anúncio**: nível, score e ações pendentes de `/performance`, ordenado por vendas/visitas, com link de correção | É a alavanca grátis de maior efeito; o ML já diz o que falta, o app só prioriza |
| **Checklist antes de publicar**: atributos obrigatórios, GTIN, ≥ 3 fotos, título com termo de `/trends` e da categoria, via validador de publicações | Evita nascer "Básica"; é marketing embutido no fluxo de anúncio que já existe |
| **Caixa de perguntas** com alerta de não respondidas, tempo de resposta e respostas-modelo (envio sempre confirmado por você) | Tempo de resposta pesa em conversão e posição; é o atendimento de um vendedor solo |
| **Preço e promoção com guarda de margem**: simulador (preço, desconto, tarifa, frete, Ads) e criação de desconto/campanha/cupom via `/seller-promotions` só se a margem ficar acima do mínimo | Desconto sem conta é o erro clássico; a API existe e o app já calcula margem |
| **Ads só leitura**: importar custo e ROAS por item/campanha e somar na margem real; mostrar ROAS de equilíbrio | Responde "o anúncio pago se paga?" sem construir gestor de campanha |
| **Monitor de reputação e opiniões**: cor da reputação, atrasos, cancelamentos, nota por item, alerta de avaliação ≤ 3 estrelas | Reputação desbloqueia promoções e Ads; avaliação ruim aponta produto a cortar |

### Fase 2: afiliação

| Item | Por quê |
|---|---|
| **Gerador de post de oferta**: texto curto (produto, preço, desconto, link), imagem, identificação "#publi"/"link de afiliado", e para Amazon a data/hora do preço; botão copiar | Serve WhatsApp, Instagram e TikTok sem API e sem risco; padroniza o que funciona em grupo de oferta |
| **Link rastreado por canal**: `subId` da Shopee (até 5) e tags por canal; relatório de comissão por canal | Única forma de saber qual canal dá dinheiro; dados já vêm na conversão |
| **Publicação automática no canal do Telegram**: fila, agenda (horários de pico), rate limit | Única automação oficial, grátis e permitida |
| **Regras por programa/produto**: tráfego pago permitido?, busca paga?, marca proibida?, e-mail?; aviso ao montar post ou campanha | Shopee, Amazon e cada produtor Hotmart têm regras diferentes; perder a conta é perder o ativo |
| **Calendário de campanhas** (9.9, 10.10, 11.11, Black Friday, comissão extra) como lista editável | Datas concentram comissão extra; custo de construção quase zero |

### Fase 3: expansão

| Item | Por quê |
|---|---|
| Publicação no Instagram via Graph API (com hospedagem temporária de mídia) | Só compensa com volume diário; antes disso, copiar e colar resolve |
| Publicação no Pinterest via API v5 (Standard access) | Canal de cauda longa; depende de aprovação |
| Encurtador próprio com contagem de cliques (já previsto) | Cliques por canal; respeitando as flags da seção 6 |
| Marketing para Shopee/Magalu como canais de venda (equivalentes de qualidade e promoção) | Mesmo padrão da fase 1, quando esses canais entrarem |
| Escrita em Product Ads, se a API for liberada ao app [I] | Hoje a doc é só leitura |

### Fora (por enquanto)

| Item | Por quê |
|---|---|
| Blog/site e SEO | Meses até dar resultado; WordPress externo resolve se um dia fizer sentido |
| Gestão de Google Ads / Meta Ads | Exige conhecimento que o dono não tem; o painel nativo basta; o app só registra o custo |
| E-mail marketing e lista de contatos | Sem lista, alto risco de spam/LGPD, proibido sem consentimento em Shopee e Amazon |
| TikTok por API | Auditoria recusa uso próprio; sem auditoria o post fica privado |
| WhatsApp Cloud API | Grupos de até 8 e custo por mensagem; não atende grupo de ofertas |
| Geração de vídeo/IA, CRM, remarketing para compradores do ML | Fora do núcleo; mensagem pós-venda do ML é restrita por motivo [I] e envolve LGPD |

## 6. Feature flags (proibido = construído, desativado)

| Item | Motivo | Fase | Padrão |
|---|---|---|---|
| Envio para grupos via WhatsApp não oficial (WhatsApp Web) | Termos proíbem bulk/auto-messaging e clientes não autorizados [C] | 3 | **Desativado**, aviso de risco de ban do número |
| Encurtador/redirect próprio em links Amazon | Encurtador que oculte o redirecionamento é proibido [C] | 3 | **Desativado** para Amazon; demais liberados |
| Encurtador/redirect próprio em links Shopee | Proibido alterar o link de afiliado (2.2) [C]; se redirect simples conta como alteração [I] | 3 | **Desativado** até confirmar |
| Marcação "tráfego de busca pago" em programa Shopee | 4.3(i) [C] | 2 | **Desativado**; o app avisa ao registrar campanha |
| Palavra-chave com marca (amazon, Shopee, Hotmart) em campanha registrada | Amazon, Shopee, Hotmart proíbem [C] | 2 | **Bloqueio por flag** com aviso |
| Resposta automática de perguntas no ML sem confirmação | Não proibido; risco de resposta errada (flag de produto) | 1 | Desativado |
| Autocompra pelo próprio link | Fraude, não funcionalidade | — | Não construído |

Lembretes ⚠️ (não bloqueiam): identificação publicitária CONAR nos posts (ligada por padrão no gerador); "de/por" com preço inflado antes do desconto pode ser questionado pelo CDC/Procon [I]; fotos copiadas de vendedor da Shopee podem gerar denúncia de direito autoral no ML [I]; LGPD para qualquer lista de contatos.

## 7. Validar com conta real

1. Requisitos atuais de Product Ads e se há escrita por API para o app.
2. Se `/seller-promotions` aceita desconto individual sem reputação verde em conta nova.
3. Instagram: app em modo desenvolvimento publicando na própria conta sem App Review.
4. Shopee: se redirect próprio antes do link de afiliado conta como "alterar o link".
5. Termos do programa de afiliados do ML e do Magalu (não lidos na íntegra).

## Fontes

- ML qualidade: https://developers.mercadolivre.com.br/pt-br/qualidade-das-publicacoes
- ML Mercado Ads: https://developers.mercadolivre.com.br/pt-br/introducao-ao-mercado-ads · https://developers.mercadolivre.com.br/pt-br/product-ads-para-catalogo-e-user-products-leitura · https://developers.mercadolivre.com.br/pt-br/bonificacoes-para-product-ads · https://vendedores.mercadolivre.com.br/nota/anuncie-no-mercado-livre-e-atraia-mais-clientes · https://vendedores.mercadolivre.com.br/nota/como-criar-campanhas-publicitarias-no-product-ads
- ML promoções: https://developers.mercadolibre.com.ar/central-de-promociones · https://developers.mercadolivre.com.br/pt-br/campanhas-do-vendedor · https://developers.mercadolivre.com.br/pt-br/cupons-do-vendedor
- ML perguntas, opiniões, tendências: https://developers.mercadolivre.com.br/pt-br/gerenciamento-perguntas-respostas · https://developers.mercadolivre.com.br/pt-br/opinioes-sobre-um-produto · https://developers.mercadolivre.com.br/pt-br/tendencias
- ML posicionamento e fotos: https://vendedores.mercadolivre.com.br/nota/como-posicionar-seus-produtos-nos-resultados-de-busca-nuevo · https://vendedores.mercadolivre.com.br/nota/fotos-de-qualidade-o-segredo-para-se-destacar-e-vender-mais
- Shopee Afiliados: https://help.shopee.com.br/portal/10/article/124094 (termos) · https://help.shopee.com.br/portal/10/article/170923 (tráfego pago)
- Amazon: https://associados.amazon.com.br/help/operating/policies
- Hotmart: https://help.hotmart.com/pt-br/article/208278538 · https://help.hotmart.com/pt-BR/article/Como-configurar-o-meu-Programa-de-Afiliados/210874788
- Google Ads: https://support.google.com/adspolicy/answer/16427718 · https://support.google.com/adspolicy/answer/6020955
- Meta: https://developers.facebook.com/documentation/instagram-platform/content-publishing · https://developers.facebook.com/documentation/business-messaging/whatsapp/groups/ · https://developers.facebook.com/docs/whatsapp/pricing · https://www.whatsapp.com/legal/terms-of-service
- TikTok: https://developers.tiktok.com/doc/content-sharing-guidelines · https://developers.tiktok.com/doc/content-posting-api-reference-direct-post
- Telegram: https://core.telegram.org/bots/faq
- Pinterest: https://developers.pinterest.com/docs/key-concepts/access-tiers/ · https://developers.pinterest.com/docs/reference/rate-limits.md
- Terceiros: https://www.divulganinja.com.br/blog/como-vender-shopee-whatsapp · https://jaguarsheet.com/pt/blog/gestion-preguntas-mercado-livre · https://propmark.com.br/digital/conar-atualiza-guia-de-publicidade-com-influenciadores-e-inclui-regras-sobre-ia/ · https://support.chatarchitect.com/books/meta-whatsapp/page/pricing-on-the-whatsapp-business-platform-developer-documentation
