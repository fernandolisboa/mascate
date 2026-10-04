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
Candidato a Product que cruza uma Supplier Offer com a demanda num Sales Channel, com Estimated Margin e pontuação para ranqueamento.
_Avoid_: sugestão, lead, achado

## Venda

**Sales Channel**:
Platform onde o Fernando vende como vendedor (Mercado Livre na fase 1).
_Avoid_: canal (sozinho), marketplace, loja

**Listing**:
Anúncio de um Product num Sales Channel. Nasce como rascunho no app e passa a publicado quando sobe para o canal.
_Avoid_: anúncio de afiliado, publicação, item do ML

**Order**:
Venda recebida num Sales Channel, com um ou mais itens e seu envio.
_Avoid_: venda (sozinho), pedido de compra

**Fulfillment Mode**:
De onde sai o item de um Order: `own_stock` (estoque local) ou `dropship` (o Supplier envia).
_Avoid_: modalidade, tipo de venda

**Purchase Order**:
Compra que o Fernando faz a um Supplier para repor estoque ou atender um Order em dropship.
_Avoid_: pedido (sozinho), compra ao fornecedor

**Fee**:
Valor descontado de uma venda: tarifa do canal, frete, imposto, custo de publicidade.
_Avoid_: taxa, custo (sozinho)

**Price Suggestion**:
Preço calculado pelo app para um Listing a partir da margem alvo; só vale depois que o Fernando aprova.
_Avoid_: reprecificação, preço automático

**Estimated Margin**:
Margem prevista de uma Opportunity ou Listing antes de vender.
_Avoid_: lucro, margem (sozinho)

**Realized Margin**:
Margem de um Order já vendido: preço menos Fees menos custo médio do estoque.
_Avoid_: lucro, margem (sozinho)

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

## Afiliação

**Program**:
Programa de afiliado de uma Platform (ou de um produto específico, como na Hotmart), com suas regras de divulgação.
_Avoid_: parceria, afiliação (sozinho)

**Affiliate Link**:
Link de afiliado de um Program para um produto, com marcação do Promotion Channel onde foi divulgado.
_Avoid_: link rastreado, URL, tracking link

**Conversion**:
Venda atribuída a um Affiliate Link, informada pela Platform.
_Avoid_: venda (sozinho), pedido

**Commission**:
Valor que o Fernando recebe por uma Conversion, com status derivado: pendente, aprovada, paga ou estornada.
_Avoid_: ganho, receita

**Payout**:
Repasse de dinheiro de um Program para o Fernando, cobrindo uma ou mais Commissions.
_Avoid_: saque, pagamento (sozinho)

## Marketing

**Promotion Channel**:
Lugar onde o Fernando divulga produtos ou links (grupo de WhatsApp, canal do Telegram, Instagram). Não confundir com Sales Channel.
_Avoid_: canal (sozinho), rede

**Offer Post**:
Texto e imagem gerados pelo app para divulgar um produto num Promotion Channel.
_Avoid_: post, copy (sozinho), anúncio

**Listing Copy**:
Título e descrição de um Listing gerados como rascunho para o Fernando editar.
_Avoid_: copy (sozinho), descrição

**Promotion**:
Desconto, campanha ou cupom do vendedor num Sales Channel.
_Avoid_: campanha (sozinho), oferta

**Sales Event**:
Data comercial do calendário (9.9, 11.11, Black Friday) usada para planejar Promotions e Offer Posts.
_Avoid_: campanha, evento (sozinho)

## Regras do app

**Restricted Feature**:
Funcionalidade construída e testada, mas desativada por padrão porque a lei ou os termos de uma Platform a proíbem ou não foram verificados.
_Avoid_: feature bloqueada, funcionalidade ilegal

**Reminder**:
Aviso fiscal ou jurídico que aparece no app sem bloquear nada.
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
Importação de outro banco do Mascate combinando registros linha a linha, em vez de substituir o banco atual.
_Avoid_: restore, sync

## Interface

**Interface Theme**:
Conjunto de cores, cantos e fonte da interface do app (Papel, Grafite, Ouro Negro…). O Fernando escolhe um tema fixo ou segue o claro/escuro do sistema com um par de temas. Só muda a aparência, nunca a posição das coisas.
_Avoid_: tema (sozinho, quando houver dúvida com tema de anúncio), skin, modo escuro

**Layout**:
Arranjo que posiciona as partes de cada tela (navegação, título, ações, conteúdo): Workspace (barra lateral) ou Studio (abas no topo e barra de status). Nunca muda o que as partes fazem.
_Avoid_: tela, visual, template
