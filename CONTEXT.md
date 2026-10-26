# Mascate

App desktop de um vendedor solo para garimpar produtos, revender com estoque próprio em marketplaces e acompanhar comissões de afiliado, tudo num só lugar. Os termos abaixo são os nomes usados no código; as definições estão em português.

## Plataformas e conexões

**Platform**:
Empresa externa com a qual o app conversa: Mercado Livre, Shopee, Hotmart, Amazon etc. A mesma Platform pode ter vários papéis (fonte de ofertas, canal de venda, programa de afiliado).
_Avoid_: marketplace (quando se fala do papel genérico), integração, site

**Connection**:
Autorização do Fernando numa conta de uma Platform (tokens, chaves), usada para ler ou escrever dados nela.
_Avoid_: conta, credencial, integração

**Sync**:
Rodada de leitura e conciliação dos dados de uma Connection com o banco local.
_Avoid_: importação (reservado para arquivos), atualização

## Catálogo e descoberta

**Product**:
Item canônico que o Fernando vende ou divulga, identificado por um SKU próprio, independente de onde é comprado ou anunciado.
_Avoid_: item, anúncio, oferta

**Supplier**:
Quem vende para o Fernando (uma loja na Shopee, um atacadista, um fornecedor de dropshipping).
_Avoid_: vendedor, loja, fonte

**Supplier Offer**:
Preço e condições de um Supplier para um Product (ou candidato a Product) numa data, vindos de uma Product Source.
_Avoid_: oferta (sozinho), cotação, anúncio do fornecedor

**Product Source**:
Origem das Supplier Offers e dos sinais de demanda: API oficial, cadastro manual, arquivo ou crawler.
_Avoid_: integração, scraper

**Opportunity**:
Candidato a Product que cruza uma Supplier Offer com a demanda num Sales Channel, com Estimated Margin e pontuação para ranqueamento. Uma Opportunity descartada leva sempre um motivo e nunca volta numa nova Sync.
_Avoid_: sugestão, lead, achado

**Best Seller**:
Um dos mais vendidos de uma categoria que o Fernando segue num Sales Channel, com a posição no ranking. É o sinal de demanda das Opportunities.
_Avoid_: destaque, top, campeão de vendas

## Venda

**Sales Channel**:
Platform onde o Fernando vende como vendedor: Mercado Livre desde a fase 1; Amazon, Shopee e Magalu na fase 3. Uma conta por Platform, numa ordem que o Fernando define e que decide quem recebe primeiro as unidades da Stock Share.
_Avoid_: canal (sozinho), marketplace, loja

**Listing**:
Anúncio de um Product num Sales Channel. Nasce como rascunho no app e passa a publicado quando sobe para o canal; os que já existem no canal chegam por Sync. Um anúncio com variações vira um Listing por variação, porque cada uma vende um Product.
_Avoid_: anúncio de afiliado, publicação, item do ML

**Listing Draft**:
Listing ainda não publicado, montado a partir de um Product: título, descrição, categoria e atributos do Sales Channel, fotos da pasta do Product, Listing Type, preço e estoque. Na Shopee pode ter variações: outros Products vendidos como opções do mesmo anúncio (como cores), cada um com a sua opção e o seu estoque, e cada uma vira um Listing do seu Product ao publicar. Só sobe para o canal quando o Fernando clica em publicar.
_Avoid_: pré-anúncio, rascunho de produto

**Catalog Offer**:
Listing Draft que oferece um produto que o catálogo do Sales Channel já tem, achado pelo GTIN (o código de barras da embalagem), em vez de uma página própria. A página, as fotos e a descrição são do canal; o Fernando escolhe o produto quando o GTIN traz mais de um e confirma condição, preço e estoque. É como se publica na Amazon (o produto é o ASIN); criar um ASIN novo fica fora.
_Avoid_: anúncio de catálogo (no Mercado Livre é outra coisa), cadastro de produto

**Stock Mirror**:
Regra pela qual o estoque de cada Listing publicado e ligado a um Product segue o saldo dele no app. Depois de cada Stock Movement e de cada Sync, o app envia ao Sales Channel a Stock Share que mudou desde o último envio, as que diminuem antes das que aumentam; o que não foi fica na fila de reenvio, com o motivo. Um Product que nunca entrou no estoque ainda não é espelhado.
_Avoid_: sincronização de estoque (Sync é leitura), estoque automático

**Stock Share**:
Parte do saldo de um Product que um Listing mostra no Sales Channel. O saldo é dividido uma unidade por vez, em rodízio, entre os Listings à venda (ou pausados pelo canal por falta de estoque), na ordem dos Sales Channels e, dentro de um canal, do Listing mais antigo para o mais novo. A soma das partes é sempre o saldo, então a mesma unidade nunca está à venda em dois lugares; com 1 unidade só o primeiro Listing mostra.
_Avoid_: reserva, cota, estoque do canal

**Checklist**:
O que um Listing Draft ainda pede antes de publicar. Os itens que bloqueiam (título, categoria, preço, atributos obrigatórios, mínimo de 3 fotos, nome e opções das variações, erros do validador do canal, inclusive uma recusa do canal ao publicar) impedem a publicação; os avisos (GTIN e atributos recomendados, descrição vazia, estoque zero, avisos do validador) não. Uma Catalog Offer pede o produto do catálogo no lugar da categoria e não pede fotos nem descrição.
_Avoid_: pendências, validação (sozinho)

**Order**:
Venda recebida num Sales Channel, com um ou mais itens e seu envio. Chega por Sync; cada item vendido do estoque próprio depois do primeiro Sync de Orders dá baixa uma vez, por um Stock Movement de saída ligado ao Order. Dos compradores o app guarda só o necessário para envio e suporte, pelo prazo que o Fernando escolhe.
_Avoid_: venda (sozinho), pedido de compra

**Return**:
Volta de unidades de um Order ao Fernando: pela devolução que o comprador abre no Sales Channel, por um envio não entregue ou por um cancelamento depois do envio. As unidades só voltam ao estoque quando o Fernando confirma que chegaram, e ao confirmar ele diz se voltaram em condição de venda; um Order cancelado antes do envio devolve as unidades sozinho, no Sync. Cada unidade volta uma vez, ao custo com que saiu.
_Avoid_: estorno (é dinheiro), troca

**Dispatch Deadline**:
Hora até a qual o Fernando precisa despachar um Order, informada pelo Sales Channel. O Order fica atrasado depois dela e com o prazo perto dentro do aviso escolhido nas configurações.
_Avoid_: prazo de entrega (é do comprador), SLA

**Fulfillment Mode**:
De onde sai o item de um Order: `own_stock` (estoque local) ou `dropship` (o Supplier envia).
_Avoid_: modalidade, tipo de venda

**Purchase Order**:
Compra que o Fernando faz a um Supplier para repor estoque ou atender um Order em dropship.
_Avoid_: pedido (sozinho), compra ao fornecedor

**Fee**:
Valor descontado de uma venda: tarifa do canal, frete, imposto, custo de publicidade.
_Avoid_: taxa, custo (sozinho)

**Listing Type**:
Exposição de um Listing no Sales Channel, que define a tarifa de venda: Clássico ou Premium no Mercado Livre.
_Avoid_: tipo de anúncio (no código), plano

**Target Margin**:
Margem que o Fernando quer em cada venda de um Product, em percentual do preço de venda. Cada Product pode ter a sua; sem ela, vale a margem alvo padrão.
_Avoid_: markup, margem mínima (reservada para Promotions)

**Price Suggestion**:
Menor preço, em centavos, em que um Listing chega à Target Margin depois das Fees e do Average Cost; as variações do mesmo anúncio dividem o preço. Só vai ao Sales Channel quando o Fernando aprova.
_Avoid_: reprecificação, preço automático

**Automatic Repricing**:
Preço de um Listing ajustado sem confirmação a cada mudança, entre um mínimo e um máximo que o Fernando confirma, seguindo o preço que o Sales Channel diz ganhar a disputa (a automação de preços do Mercado Livre, a oferta em destaque da Amazon). O mínimo nunca fica abaixo do preço que mantém a Minimum Margin. Desligada por flag até o Fernando ligar; cada mudança fica no histórico de preço do Listing.
_Avoid_: preço dinâmico, robô de preço

**Estimated Margin**:
Margem prevista de uma Opportunity ou Listing antes de vender.
_Avoid_: lucro, margem (sozinho)

**Realized Margin**:
Margem de um Order já vendido: o que o comprador pagou e não recebeu de volta, menos as Fees, o imposto, a parte do custo de Product Ads que cabe ao Order e o custo com que as unidades saíram do estoque (as que voltaram à prateleira saem do custo). Enquanto o Sales Channel não fatura o Order, é provisória: as Fees são as que o Order informa.
_Avoid_: lucro, margem (sozinho)

**Money Release**:
Momento em que o dinheiro de um pagamento de Order fica livre para saque no Mercado Pago. Antes disso o valor está a liberar, com ou sem data; o valor é o que o vendedor recebe do pagamento.
_Avoid_: Release (sozinho, que é a versão do app), repasse, saque

**Sales Volume**:
Receita das vendas dos últimos 12 meses, comparada a um limite que o Fernando configura (padrão: o teto anual do MEI). Passar do limite levanta um Reminder de formalização e não bloqueia nada.
_Avoid_: faturamento (no código), teto

## Estoque

**Stock Location**:
Lugar físico onde o estoque fica (o espaço do Fernando é o primeiro).
_Avoid_: depósito, armazém

**Stock Movement**:
Registro imutável de entrada, saída ou ajuste de um Product numa Stock Location. O saldo é sempre derivado dos movimentos.
_Avoid_: saldo, lançamento

**Receipt**:
Chegada de toda ou de parte de uma Purchase Order ao espaço do Fernando. Cada Receipt gera os Stock Movements de entrada, com o preço e a parte do frete que as unidades carregam.
_Avoid_: recebimento parcial (sozinho), baixa, entrada (quando se fala da chegada)

**Average Cost**:
Custo unitário de um Product em estoque, recalculado como média ponderada móvel a cada entrada.
_Avoid_: preço de custo, custo FIFO

**Stock Adjustment**:
Stock Movement que o Fernando registra à mão para corrigir o saldo, sempre com motivo: perda, avaria ou contagem. Na contagem ele informa o saldo físico e o ajuste é a diferença. As unidades saem (ou voltam) pelo Average Cost, então um ajuste nunca o altera, e o saldo nunca fica negativo.
_Avoid_: baixa manual, acerto, correção (sozinho)

**Reorder Point**:
Saldo de um Product a partir do qual é hora de comprar de novo. Com o saldo nele ou abaixo, o Product aparece como estoque baixo na tela Hoje; quando um movimento o leva até ali, o app notifica.
_Avoid_: estoque mínimo, ponto de pedido

## Afiliação

**Program**:
Programa de afiliado de uma Platform (ou de um produto específico, como na Hotmart), com suas regras de divulgação: cada uma permitida, proibida ou não verificada (que avisa como proibida).
_Avoid_: parceria, afiliação (sozinho)

**Affiliate Link**:
Link de afiliado de um Program para um produto, com marcação do Promotion Channel onde foi divulgado. A marcação é única por Platform e volta na Conversion, o que liga cada Commission ao seu canal.
_Avoid_: link rastreado, URL, tracking link

**Short Link**:
Link curto que redireciona para um Affiliate Link, criado por um encurtador externo, que conta os cliques por dia. Nunca existe para Platform cujos termos proíbem encurtar (Amazon, Shopee, Mercado Livre) sem a Restricted Feature ligada.
_Avoid_: link encurtado, URL curta, encurtador (para o link)

**Conversion**:
Venda atribuída a um Affiliate Link, informada pela Platform.
_Avoid_: venda (sozinho), pedido

**Commission**:
Valor que o Fernando recebe por uma Conversion, com status derivado do status da venda na Platform: pendente, aprovada, paga, estornada (o dinheiro voltou ao comprador) ou cancelada (a venda não se concretizou e nunca houve comissão, como um boleto vencido).
_Avoid_: ganho, receita, estorno (quando a venda só não se concretizou)

**Payout**:
Repasse de dinheiro de um Program para o Fernando, cobrindo uma ou mais Commissions.
_Avoid_: saque, pagamento (sozinho)

## Marketing

**Promotion Channel**:
Lugar onde o Fernando divulga produtos ou links (grupo de WhatsApp, canal do Telegram, Instagram). Não confundir com Sales Channel.
_Avoid_: canal (sozinho), rede

**Offer Post**:
Texto e imagem gerados pelo app para divulgar um produto num Promotion Channel, com identificação de publicidade. Só é publicado (no Telegram, no Pinterest ou no Instagram) quando o Fernando confirma, na hora ou agendado, e sai no máximo uma vez.
_Avoid_: post, copy (sozinho), anúncio

**Pitch**:
Frase curta que vende o produto num Offer Post, na variável `{chamada}` do modelo, escrita pela IA ou à mão. Nunca traz preço, desconto, link ou contato: isso vem dos fatos do post. Na tela, "chamada".
_Avoid_: copy (sozinho), gancho, slogan

**Listing Copy**:
Título e descrição de um Listing gerados como rascunho para o Fernando editar.
_Avoid_: copy (sozinho), descrição

**Listing Quality**:
O que o Sales Channel diz de um Listing publicado: as ações que faltam, cada uma com o link para corrigir no canal, e, no Mercado Livre, a nota de 0 a 100 e o nível (Básica, Satisfatória, Profissional). Na Amazon são os problemas do anúncio; na Shopee, os anúncios com problema que podem render pontos de penalidade; a Magalu não informa. As que derrubam a nota ou põem o anúncio em risco são problemas; as outras, sugestões. O painel põe primeiro os Listings com algo pendente que mais venderam e receberam visitas nos últimos 30 dias (só o Mercado Livre informa visitas). Variações do mesmo anúncio dividem a mesma nota. Ao lado fica o Shop Performance.
_Avoid_: saúde do anúncio, health, score (sozinho), Checklist (é do rascunho)

**Shop Performance**:
Avaliação que a Shopee dá à loja (Ruim, Precisa melhorar, Bom, Excelente), com cada métrica de envio, anúncios e atendimento contra a meta do canal. O app mostra as fora da meta primeiro; a correção é feita no canal.
_Avoid_: saúde da conta, account health, Reputation (é a cor do Mercado Livre)

**Question**:
Pergunta de um comprador num Listing publicado, lida do Sales Channel com o status (sem resposta, respondida, encerrada sem resposta, em análise, removida) e o tempo de espera. Na caixa, as sem resposta ficam em cima, a que espera há mais tempo primeiro. Cada pergunta é respondida uma vez, só depois que o Fernando confere e confirma o texto final, que o app guarda com ela; contato e link para fora do canal nunca saem numa resposta.
_Avoid_: mensagem, dúvida, pós-venda (é mensagem de Order)

**Reply Template**:
Resposta pronta do Fernando para uma Question comum, com as variáveis `{produto}` (nome do Product do Listing) e `{prazo}` (o prazo de despacho em dias úteis). Preenche a resposta, que o Fernando ainda edita e confirma antes de enviar.
_Avoid_: resposta automática, macro, template (no código, `ReplyTemplate`)

**Reputation**:
Cor que o Sales Channel dá ao Fernando como vendedor (vermelha, laranja, amarela, verde-clara, verde), medida por reclamações, cancelamentos e despachos com atraso sobre as vendas do período (60 dias, ou 365 com poucas vendas). O app mostra cada métrica contra o limite da verde, em pontos e em vendas que ainda cabem. Enquanto o canal protege um vendedor novo, a cor fica protegida e o app mostra as métricas reais. A Reputation libera Promotions (verde) e Product Ads (amarela e um mínimo de vendas), e o app avisa uma vez quando cada um fica disponível.
_Avoid_: termômetro, nível (sozinho), qualificação

**Review**:
Avaliação de 1 a 5 estrelas que um comprador deixa num Listing, com título e texto. A nota média é somada por Product, juntando os Listings dele; uma Review de 3 estrelas ou menos é baixa e fica em Hoje até o Fernando marcar que viu.
_Avoid_: opinião (é o nome da API), comentário, nota (sozinho)

**Promotion**:
Preço menor por alguns dias num Sales Channel. No Mercado Livre: o desconto de um Listing (até 14 dias), a campanha do vendedor com os Listings que ele põe nela (até 14 dias) e o cupom do vendedor tirado no checkout de compras acima de um mínimo (até 31 dias), que exigem a Reputation que libera Promotions. Na Shopee: o desconto da loja, uma campanha do vendedor de menos de 180 dias que começa ao menos uma hora depois de criada e depois só encurta. Em qualquer canal, a campanha do canal em que o Listing entrou pelo próprio canal é só mostrada; Amazon e Magalu não têm Promotion do vendedor por API. Os dias contam no horário de Brasília, o primeiro e o último incluídos. Passa pela Margin Guard; só vai ao canal quando o Fernando confirma.
_Avoid_: campanha (sozinho), oferta

**Minimum Margin**:
Menor margem, em percentual do preço de venda, que uma Promotion pode deixar numa venda; 10% até o Fernando mudar em Configurações. É diferente da Target Margin: a alvo é onde o preço quer chegar, a mínima é o piso que nenhuma Promotion fura.
_Avoid_: margem alvo, margem de segurança

**Margin Guard**:
Conferência que bloqueia uma Promotion quando a margem de uma unidade, no pior caso, fica abaixo da Minimum Margin, mostrando a conta e o menor preço que a mantém. O pior caso é o menor preço do Listing enquanto as Promotions dele correm, depois do cupom que tira mais: um comprador leva só esse Listing, nas menos unidades que a compra mínima pede, e o desconto inteiro cai nelas. Conta a variação de menor margem. Não há como forçar.
_Avoid_: trava (no código), validação de preço

**Product Ads**:
Anúncio pago do Mercado Livre que destaca um Listing nas buscas, pago por clique. O app lê o custo, as vendas atribuídas, os cliques e as impressões de cada anúncio por dia, uma vez por dia, e muda Ad Campaigns e anúncios só quando o Fernando confirma; o custo do Shopee Ads entra do mesmo jeito, só lido. O custo de um Listing num mês entra na Realized Margin dos Orders dele nesse mês, dividido pelo valor de cada venda; o de um mês sem venda fica à parte no resumo. Exige a Reputation que libera Product Ads; sem ela, a seção fica escondida.
_Avoid_: Ads (sozinho, no código), anúncio patrocinado, publicidade

**Ad Campaign**:
Campanha do Product Ads que agrupa anúncios com um orçamento diário, uma estratégia (rentabilidade, crescimento ou visibilidade) e um Target ROAS. O app cria, pausa, ativa e muda uma Ad Campaign só depois da confirmação do Fernando.
_Avoid_: campanha (sozinho, é também a Promotion do vendedor), campaign

**Ad Group**:
Unidade que o Mercado Livre anuncia numa Ad Campaign: um Listing, uma família de variações de User Products ou um produto de catálogo. O app lê as métricas do dia por Ad Group e divide uma família ou um produto de catálogo nos Listings dele, então o custo e as vendas continuam por Listing. O app põe, move, pausa e tira um Ad Group de uma campanha só depois da confirmação do Fernando; uma família ou um produto de catálogo vai inteiro, e a confirmação lista os Listings dele. Um Product que gastou pelo menos R$ 20,00 no período com ROAS abaixo do Break-even ROAS ganha a sugestão "Pausar anúncio"; o app nunca pausa sozinho.
_Avoid_: grupo de anúncios (no código), agrupador

**Target ROAS**:
ROAS que uma Ad Campaign pede ao Sales Channel, de 1x a 35x: alvo alto vende menos com mais lucro por venda, alvo baixo vende mais. O app sugere o maior Break-even ROAS dos Products da campanha; abaixo dele, pede uma segunda confirmação. Na tela, "ROAS alvo".
_Avoid_: ROAS de equilíbrio (é outra coisa), ACOS

**Maximum Daily Budget**:
Maior orçamento diário que o app aceita enviar a uma Ad Campaign; R$ 50,00 até o Fernando mudar em Configurações. Protege de um valor digitado errado; o Sales Channel ainda pode gastar até o dobro num dia usando a sobra de dias anteriores.
_Avoid_: limite de gasto, teto (no código)

**ROAS**:
Vendas atribuídas pelo Sales Channel a um anúncio de Product Ads, diretas e indiretas, para cada real investido nele: 4,25x é R$ 4,25 vendidos por R$ 1 de custo.
_Avoid_: retorno, ROI (é outra conta)

**Break-even ROAS**:
Menor ROAS em que o anúncio de um Product não dá prejuízo: 1 ÷ a margem antes de Product Ads, ao preço de hoje, na variação de menor margem. Sem margem antes de Ads, não existe e qualquer ROAS dá prejuízo. Na tela, "ROAS de equilíbrio".
_Avoid_: ROAS mínimo, ROAS alvo

**Sales Event**:
Data comercial do calendário (9.9, 11.11, Black Friday) usada para planejar Promotions e Offer Posts.
_Avoid_: campanha, evento (sozinho)

## Regras do app

**Restricted Feature**:
Funcionalidade construída e testada, mas desativada por padrão porque a lei ou os termos de uma Platform a proíbem ou não foram verificados.
_Avoid_: feature bloqueada, funcionalidade ilegal

**Reminder**:
Aviso fiscal ou jurídico que aparece no app sem bloquear nada. Uns aparecem sempre; outros só enquanto a condição deles vale, como o de Sales Volume.
_Avoid_: bloqueio, alerta legal

**Backup**:
Cópia consistente do banco local guardada numa pasta escolhida pelo Fernando.
_Avoid_: export (quando o objetivo é guarda)

**Release**:
Versão do app publicada nas GitHub Releases, com changelog, instaladores e um manifesto que diz se ela traz Risky Migration.
_Avoid_: build, pacote (quando se fala da versão)

**Risky Migration**:
Migração do banco que reescreve ou apaga dados que já existem, em vez de só acrescentar. Uma Release com Risky Migration nunca se instala sozinha.
_Avoid_: migração destrutiva, migração grande

**Merge Import**:
Importação de outro banco do Mascate, da mesma versão ou anterior, combinando registros linha a linha, em vez de substituir o banco atual. Vence a alteração mais recente de cada registro.
_Avoid_: restore, sync

**Merge Conflict**:
Registro que existe nos dois bancos de um Merge Import com conteúdo diferente. Vence o de alteração mais recente, e o relatório mostra os dois lados.
_Avoid_: divergência, duplicata

## Interface

**Interface Theme**:
Conjunto de cores, cantos e fonte da interface do app (Papel, Grafite, Ouro Negro…). O Fernando escolhe um tema fixo ou segue o claro/escuro do sistema com um par de temas. Só muda a aparência, nunca a posição das coisas.
_Avoid_: tema (sozinho, quando houver dúvida com tema de anúncio), skin, modo escuro

**Layout**:
Arranjo que posiciona as partes de cada tela (navegação, título, ações, conteúdo): Workspace (barra lateral) ou Studio (abas no topo e barra de status). Nunca muda o que as partes fazem.
_Avoid_: tela, visual, template
