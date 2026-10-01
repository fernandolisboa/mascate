# PRD: Mascate

> Escrito em 01/10/2026 a partir do grilling com o Fernando. Vocabulário em [CONTEXT.md](../CONTEXT.md); decisões estruturais em [docs/adr/](adr/). Pesquisa de base em [platform-integrations.md](platform-integrations.md), [commerce-integrations.md](commerce-integrations.md) e [marketing.md](marketing.md). Credenciais em [credentials.html](credentials.html).

## Problem Statement

O Fernando quer ganhar dinheiro vendendo online sozinho, com um espaço desocupado que pode virar estoque. A ideia é garimpar produtos baratos na Shopee e revendê-los no Mercado Livre, e depois somar comissões de afiliado em várias plataformas. Hoje não há nada que junte isso: achar um produto bom exige cruzar preço da Shopee com demanda do ML na mão; calcular a margem real depende de tarifas, frete e custo de estoque que ficam espalhados; anúncios, pedidos e estoque vivem em painéis diferentes; e cada plataforma de afiliado tem um painel e um jeito próprio de mostrar comissão. Ele não entende de marketing e não quer ler muito código, mas quer um app local, dele, que organize a operação inteira.

## Solution

Um app desktop (Windows primeiro, Linux para testes) que roda localmente, sem servidor, e cobre a operação em três fases, cada uma utilizável sozinha:

1. **Venda com estoque local e descoberta.** O app encontra Opportunities cruzando Supplier Offers da Shopee com a demanda no Mercado Livre, mostra a Estimated Margin, ajuda a comprar (Purchase Order registrada à mão), controla o estoque do espaço, cria rascunhos de Listing com Listing Copy gerada por IA, sugere preço, sincroniza Orders e mostra a Realized Margin de cada venda. Marketing aqui é o próprio anúncio: qualidade, perguntas, Promotions com guarda de margem e leitura do custo de Ads.
2. **Afiliação.** Commissions de Hotmart, Shopee, Kiwify, Monetizze e Eduzz por API, e de Amazon, ML e Magalu por arquivo ou lançamento manual, num painel único com status, estornos e Payouts. Divulgação com Offer Posts, Affiliate Links por Promotion Channel e postagem automática num canal do Telegram.
3. **Expansão.** Dropshipping, novos Sales Channels (Shopee, Magalu, Amazon), Restricted Features (crawlers, automação de painel), Instagram e Pinterest por API, encurtador com cliques, Merge Import e sincronização entre máquinas.

Tudo que a lei ou os termos proíbem é construído e desligado por flag (ADR 0006); questões fiscais e jurídicas viram Reminders e nunca bloqueiam.

## User Stories

### Fundação do app

1. Como Fernando, quero instalar o app no Windows 11 com um instalador, para começar a usar sem configurar nada técnico.
2. Como Fernando, quero que o app abra com um painel inicial mostrando o que precisa da minha atenção hoje, para não ter de procurar em cada tela.
3. Como Fernando, quero que fechar a janela minimize o app para a bandeja e mantenha o Sync rodando, para não perder pedidos por ter fechado a janela.
4. Como Fernando, quero uma notificação do sistema quando entrar um Order novo, para despachar rápido.
5. Como Fernando, quero escolher o intervalo do Sync (padrão 5 minutos), para equilibrar rapidez e uso de rede.
6. Como Fernando, quero cadastrar Connections numa tela de configurações, colando chaves ou clicando em Conectar, para não editar arquivos.
7. Como Fernando, quero que segredos fiquem no cofre do Windows e nunca no banco ou no Backup, para que um Backup vazado não exponha minhas contas.
8. Como Fernando, quero ver o estado de cada Connection (conectada, expirada, com erro, aguardando aprovação), para saber o que precisa de ação.
9. Como Fernando, quero que o app funcione com a Shopee Affiliate API ainda não aprovada, com a Connection marcada como pendente, para não travar a fase 1.
10. Como Fernando, quero um Backup automático diário numa pasta que eu escolher, para não perder dados se o computador der problema.
11. Como Fernando, quero configurar quantos Backups guardar (padrão 14), para controlar o espaço em disco.
12. Como Fernando, quero um botão "Backup agora", para fazer uma cópia antes de algo arriscado.
13. Como Fernando, quero exportar o banco num arquivo e restaurá-lo noutra máquina, para mudar de computador.
14. Como Fernando, quero ser avisado de versão nova e atualizar com um clique, para ficar em dia sem baixar instalador.
15. Como Fernando, quero uma opção de atualização silenciosa (desligada por padrão) aplicada ao reiniciar, para não pensar em atualização.
16. Como Fernando, quero que versões com migração de risco nunca se instalem sozinhas e me peçam confirmação, para não arriscar o banco sem saber.
17. Como Fernando, quero Backup automático antes de toda migração e restauração automática se ela falhar, para nunca perder dados numa atualização.
18. Como Fernando, quero ver Reminders fiscais e jurídicos (CNPJ, NF-e, LGPD) num lugar discreto, para lembrar sem ser bloqueado.
19. Como Fernando, quero uma tela que liste as Restricted Features, o motivo de cada uma estar desligada e o risco de ligar, para decidir com informação.
20. Como Fernando, quero confirmar explicitamente antes de ligar uma Restricted Feature, para não ativar algo arriscado por engano.

### Catálogo e descoberta (fase 1)

21. Como Fernando, quero cadastrar uma Supplier Offer colando o link e o preço de um produto da Shopee, para usar a descoberta antes de a API ser aprovada.
22. Como Fernando, quero que, com a API aprovada, o app busque Supplier Offers da Shopee por palavra-chave e categoria, com preço e vendas, para garimpar em volume.
23. Como Fernando, quero ver os mais vendidos do ML por categoria, para saber o que tem demanda.
24. Como Fernando, quero que o app cruze Supplier Offers com a demanda no ML e me mostre Opportunities ranqueadas, para focar no que tem mais chance.
25. Como Fernando, quero ver a Estimated Margin de cada Opportunity já descontando tarifa do ML, frete e imposto configurado, para não me enganar com preço bruto.
26. Como Fernando, quero filtrar Opportunities por margem mínima, categoria e faixa de preço, para ajustar a busca ao meu capital.
27. Como Fernando, quero ver quantos concorrentes vendem o mesmo produto no catálogo do ML e por qual preço, para estimar se consigo competir.
28. Como Fernando, quero transformar uma Opportunity em Product com um SKU próprio, para começar a operar o item.
29. Como Fernando, quero ligar várias Supplier Offers ao mesmo Product, para comparar fornecedores.
30. Como Fernando, quero ver o histórico de preço das Supplier Offers de um Product, para comprar na hora certa.
31. Como Fernando, quero descartar uma Opportunity com um motivo, para que ela não volte a aparecer.
32. Como Fernando, quero que o app guarde fotos, notas de compra e prints de cada Product numa pasta organizada por SKU, para achar tudo rápido.
33. Como Fernando, quero arrastar arquivos para a ficha do Product e o app copiar para a pasta certa, para não organizar à mão.

### Compra e estoque (fase 1)

34. Como Fernando, quero registrar uma Purchase Order com Supplier, itens, quantidades, preço, frete e data, para controlar o que comprei.
35. Como Fernando, quero acompanhar o status de cada Purchase Order (comprada, enviada, recebida, cancelada), para saber o que está a caminho.
36. Como Fernando, quero dar entrada no estoque ao receber uma Purchase Order, incluindo recebimento parcial, para o saldo refletir o físico.
37. Como Fernando, quero que o Average Cost do Product seja recalculado a cada entrada, incluindo o frete rateado, para a margem usar o custo real.
38. Como Fernando, quero registrar ajustes de estoque (perda, avaria, contagem) com motivo, para manter o saldo certo.
39. Como Fernando, quero ver o saldo de cada Product por Stock Location e o valor total parado em estoque, para saber quanto capital está imobilizado.
40. Como Fernando, quero ver o histórico de Stock Movements de um Product, para entender de onde veio cada diferença.
41. Como Fernando, quero alerta de estoque baixo com ponto de reposição por Product, para não zerar e pausar o anúncio.
42. Como Fernando, quero que uma venda dê baixa automática no estoque, para não atualizar à mão.

### Anúncios e preço (fase 1)

43. Como Fernando, quero conectar minha conta de vendedor do ML, para o app ler e escrever anúncios e pedidos.
44. Como Fernando, quero importar meus anúncios existentes do ML e ligá-los aos Products, para começar a partir do que já tenho.
45. Como Fernando, quero criar um rascunho de Listing a partir de um Product, com categoria, atributos e fotos sugeridos, para refinar em vez de começar do zero.
46. Como Fernando, quero que o app gere Listing Copy (título e descrição) com IA no rascunho, para ter uma primeira versão boa sem saber escrever anúncio.
47. Como Fernando, quero que a Listing Copy use termos em alta da categoria, para aparecer mais na busca.
48. Como Fernando, quero um checklist antes de publicar (atributos obrigatórios, GTIN, mínimo de 3 fotos, título), para o anúncio não nascer com qualidade básica.
49. Como Fernando, quero publicar o Listing no ML a partir do rascunho, para não redigitar no painel.
50. Como Fernando, quero que o estoque do Listing no ML acompanhe o saldo do app, para não vender o que não tenho.
51. Como Fernando, quero uma Price Suggestion por Listing a partir da margem alvo, para precificar sem planilha.
52. Como Fernando, quero aprovar ou ajustar a Price Suggestion antes de ela ir para o ML, para manter o controle.
53. Como Fernando, quero um simulador de preço (preço, desconto, tarifa, frete, Ads, imposto) mostrando a margem resultante, para testar cenários.
54. Como Fernando, quero pausar e reativar Listings pelo app, para reagir a falta de estoque ou de margem.

### Pedidos e envio (fase 1)

55. Como Fernando, quero ver os Orders do ML sincronizados com itens, comprador e prazo de despacho, para organizar o dia.
56. Como Fernando, quero baixar a etiqueta de envio do Order pelo app, para despachar por coleta ou agência.
57. Como Fernando, quero ver Orders atrasados ou perto do prazo destacados, para proteger minha reputação.
58. Como Fernando, quero que cancelamentos e devoluções atualizem o Order e devolvam o item ao estoque quando aplicável, para o saldo ficar certo.
59. Como Fernando, quero ver as Fees de cada Order (tarifa, frete, imposto, Ads) vindas do ML, para saber para onde foi o dinheiro.
60. Como Fernando, quero ver a Realized Margin de cada Order, para saber se o produto vale a pena.
61. Como Fernando, quero que o app reconcilie Orders perdidos no Sync seguinte, para não ter buracos quando o computador ficou desligado.
62. Como Fernando, quero guardar só os dados do comprador necessários para envio e suporte, com retenção configurável, para reduzir exposição de dados pessoais.

### Marketing no ML (fase 1)

63. Como Fernando, quero um painel de qualidade dos meus Listings com o que falta em cada um, ordenado por vendas e visitas, para saber onde mexer primeiro.
64. Como Fernando, quero ver as perguntas sem resposta com o tempo de espera, para responder rápido.
65. Como Fernando, quero respostas-modelo para perguntas comuns, enviadas só depois que eu confirmar, para ganhar tempo sem risco.
66. Como Fernando, quero criar descontos, campanhas e cupons do vendedor pelo app, bloqueados abaixo de uma margem mínima, para não dar desconto que dá prejuízo.
67. Como Fernando, quero saber quando minha reputação libera Promotions e Product Ads, para usar essas ferramentas assim que possível.
68. Como Fernando, quero importar o custo e o ROAS do Product Ads por item e somar na margem real, para saber se o anúncio pago se paga.
69. Como Fernando, quero ver o ROAS de equilíbrio de cada Product, para decidir quanto posso gastar em anúncio.
70. Como Fernando, quero monitorar a cor da minha reputação, atrasos, cancelamentos e notas por item, com alerta para avaliação de 3 estrelas ou menos, para agir antes de piorar.

### Painel e finanças (fase 1, ampliado na fase 2)

71. Como Fernando, quero um painel com receita, margem, capital em estoque e Orders por período, para ver como o negócio está.
72. Como Fernando, quero ver margem por Product e por categoria, para cortar o que não dá dinheiro.
73. Como Fernando, quero configurar a alíquota de imposto (padrão 0%) usada nos cálculos, para simular quando tiver CNPJ.
74. Como Fernando, quero um Reminder quando o volume de vendas passar de um limite configurável, para lembrar da formalização.
75. Como Fernando, quero ver valores pendentes e liberados do Mercado Pago, para saber quando o dinheiro cai.

### Afiliação (fase 2)

76. Como Fernando, quero conectar Hotmart, Shopee, Kiwify, Monetizze e Eduzz, para ver Commissions sem abrir cada painel.
77. Como Fernando, quero importar relatórios de ganhos da Amazon e, se existirem, do ML e Magalu, para incluir plataformas sem API.
78. Como Fernando, quero lançar Commissions à mão quando não houver API nem arquivo, para o painel ficar completo.
79. Como Fernando, quero que reimportar o mesmo arquivo nunca duplique Conversions, para confiar nos números.
80. Como Fernando, quero ver cada Commission com status pendente, aprovada, paga ou estornada, para saber o que vou receber de fato.
81. Como Fernando, quero ver estornos destacados e descontados do total, para não contar com dinheiro que voltou.
82. Como Fernando, quero ver Payouts e quais Commissions cada um cobre, para conferir os repasses.
83. Como Fernando, quero um painel único de comissões por Platform, Program, produto e Promotion Channel, para ver o que rende mais.
84. Como Fernando, quero cadastrar Programs com suas regras de divulgação (tráfego pago, palavras de marca, e-mail), para não perder a conta por violar regra.
85. Como Fernando, quero criar Affiliate Links marcados por Promotion Channel, para saber qual canal dá comissão.
86. Como Fernando, quero gerar um Offer Post (texto, imagem, link, identificação de publicidade) e copiar com um clique, para divulgar no WhatsApp ou Instagram.
87. Como Fernando, quero que Offer Posts da Amazon incluam data e hora do preço, para cumprir a regra do programa.
88. Como Fernando, quero agendar Offer Posts para um canal meu no Telegram, publicados automaticamente nos horários que eu escolher, para divulgar sem ficar no celular.
89. Como Fernando, quero ser avisado quando um post ou link violar uma regra do Program, para corrigir antes de publicar.
90. Como Fernando, quero um calendário editável de Sales Events (9.9, 11.11, Black Friday), para planejar Promotions e Offer Posts.
91. Como Fernando, quero que o mesmo Product apareça ligado ao lado de venda e ao lado de afiliado, para comparar se vale mais revender ou divulgar.

### Expansão (fase 3)

92. Como Fernando, quero vender em dropshipping com o Fulfillment Mode `dropship` e o prazo de disponibilidade do ML, desligado por padrão, para testar sem estoque quando eu decidir.
93. Como Fernando, quero registrar a Purchase Order ao Supplier a partir de um Order em dropship, para não esquecer de comprar.
94. Como Fernando, quero conectar fornecedores com API de compra (AliExpress, fornecedores nacionais), para automatizar o dropshipping.
95. Como Fernando, quero vender também na Shopee, Magalu e Amazon com o mesmo estoque, para alcançar mais compradores.
96. Como Fernando, quero que o estoque seja compartilhado entre Sales Channels sem vender duas vezes o mesmo item, para não cancelar pedidos.
97. Como Fernando, quero reprecificação automática dentro de limites mínimo e máximo, desligada por padrão, para reagir à concorrência sem estar no app.
98. Como Fernando, quero crawlers de descoberta prontos e desligados como Restricted Features, para ligar se os termos mudarem ou se eu decidir.
99. Como Fernando, quero publicar Offer Posts no Instagram e Pinterest pela API, para divulgar em volume quando fizer sentido.
100. Como Fernando, quero um encurtador próprio com contagem de cliques, desligado para links Amazon e Shopee, para medir cliques onde é permitido.
101. Como Fernando, quero importar outro banco do Mascate com Merge Import e ver o relatório de conflitos, para juntar dados de duas máquinas.
102. Como Fernando, quero escolher nas configurações um modo de sincronização entre máquinas (réplica gerenciada, local-first com relay, backend próprio), para usar em mais de um computador e, no futuro, consultar pelo celular.

## Implementation Decisions

### Arquitetura

- App desktop nativo em Rust com interface em GPUI e `gpui-component` (ADR 0001). Versões fixadas.
- Monólito modular num workspace Cargo, um crate por módulo, cada um com interface pública explícita; nenhum módulo lê tabelas de outro; comunicação por chamada à interface pública ou evento interno (ADR 0002).
- Módulos: **Catalog & Discovery** (Product, Supplier, Supplier Offer, Product Source, Opportunity), **Commerce** (Listing, Order, Purchase Order, Fee, Price Suggestion, Promotion), **Inventory** (Stock Location, Stock Movement, Average Cost), **Affiliate** (Program, Affiliate Link, Conversion, Commission, Payout), **Marketing** (Offer Post, Listing Copy, Promotion Channel, Sales Event, qualidade de anúncio), **Integrations** (Connections, adapters por Platform, Syncs, importadores de arquivo), **Finance & Reporting** (margens, painel, Reminders). Mais um crate de plataforma do app (backup, atualização, migrações, flags, cofre de segredos) e o crate de interface.
- A interface é uma camada fina sobre o núcleo: telas chamam a interface pública dos módulos e não têm regra de negócio, para que outra interface (macOS, celular, web) reaproveite o núcleo.

### Dados

- Banco local SQLite-compatível da família Turso, sem servidor (ADR 0003). Cada módulo tem seu próprio prefixo/conjunto de tabelas.
- Todo registro tem ID UUIDv7, `created_at`, `updated_at` e exclusão lógica (ADR 0004).
- Dinheiro é sempre decimal com moeda explícita; nunca float. Arredondamento só na apresentação e em valores enviados às Platforms.
- Estoque como ledger imutável de Stock Movements; saldo derivado; Average Cost por média ponderada móvel, com frete da Purchase Order rateado por valor (ADR 0005).
- `fulfillment_mode` (`own_stock` | `dropship`) existe no Order desde a fase 1.
- Commission tem status derivado no adapter a partir do status da transação de cada Platform (pendente, aprovada, paga, estornada) com as datas de cada transição.
- Idempotência por chave natural de cada Platform (por exemplo, `transaction` na Hotmart, `conversionId` + item na Shopee, id do pedido no ML); reprocessar ou reimportar nunca duplica.
- Dados pessoais de compradores: só o necessário para envio e suporte, com retenção configurável.

### Integrações

- Cada Platform é um adapter atrás de uma interface por papel: fonte de ofertas e demanda (Product Source), canal de venda (Sales Channel), programa de afiliado e destino de postagem. O resto do app só conhece essas interfaces.
- Sync por polling, sem webhooks (ADR 0003). ML: pedidos recentes mais `/missed_feeds`; reconciliação periódica cobre o tempo com o app fechado.
- ML: OAuth com refresh token de uso único; refresh serializado por Connection, com gravação atômica do novo token no cofre antes de usar.
- Shopee Affiliate: assinatura SHA256 com AppID e Secret; Connection fica "aguardando aprovação" enquanto as credenciais não existirem.
- Cadastro manual de Supplier Offer é uma Product Source de primeira classe, não um paliativo.
- Rate limit, retry com backoff e circuit breaker por Connection.
- Listing Copy via API da Anthropic, sempre como rascunho editável.
- Segredos no cofre do sistema; variáveis de ambiente com os nomes de `docs/credentials.html` só em desenvolvimento.

### Regras de negócio

- Estimated Margin e Realized Margin = preço − Fees (tarifa do canal, frete, imposto configurado, custo de Ads atribuído) − Average Cost. Imposto padrão 0%.
- Price Suggestion parte da margem alvo do Product; nunca é aplicada sem aprovação na fase 1.
- Promotions só são criadas se a margem resultante ficar acima da margem mínima configurada.
- ROAS de equilíbrio = 1 / margem de contribuição antes de Ads.
- Respostas a perguntas do ML sempre confirmadas pelo Fernando; resposta automática é flag desligada.

### Flags e Restricted Features

- Um registro central de flags com motivo, fase e risco; Restricted Features nascem desligadas e exigem confirmação para ligar (ADR 0006). Lista inicial em `commerce-integrations.md` seção 6 e `marketing.md` seção 6.

### Atualização e backup

- Distribuição por GitHub Releases com instalador Windows e pacote Linux; verificação de versão nova ao abrir o app.
- Cada versão declara se tem migração de risco; atualização silenciosa nunca aplica essas (ADR 0007).
- Backup = cópia consistente do banco (não cópia do arquivo aberto), diário, para pasta escolhida, com N configurável (padrão 14), e antes de toda migração.

## Testing Decisions

Bons testes verificam comportamento externo pela interface pública do módulo, com entradas e saídas do domínio, e sobrevivem a refatorações internas. Nada de testar funções privadas ou a forma das tabelas.

Seams, do mais alto para o mais baixo:

- **Interface pública de cada módulo** (o seam principal). Testes de integração chamam o crate do módulo com um banco temporário real (arquivo SQLite em diretório temporário), relógio e gerador de IDs injetados. Cobrem os fluxos das user stories: receber Purchase Order → Stock Movements → Average Cost; Order sincronizado → baixa de estoque → Realized Margin; Conversion importada duas vezes → uma só Commission.
- **Adapters de Platform contra HTTP gravado.** Cada adapter é testado contra um servidor HTTP falso que serve respostas JSON reais gravadas (fixtures por Platform), incluindo erros 401, 403, 429 e refresh de token concorrente. Os mesmos fixtures documentam o contrato que validamos com conta real.
- **Cálculos puros** (dinheiro, margem, Average Cost, Price Suggestion, ROAS, derivação de status de Commission) com testes por exemplo e testes de propriedade (por exemplo: saldo nunca negativo sem ajuste explícito; soma de rateio de frete igual ao frete).
- **Migrações:** para cada versão publicada, um banco de exemplo daquela versão é migrado até a atual e verificado; teste de falha simulada confirma restauração do Backup.
- **Merge Import (fase 3):** dois bancos com alterações cruzadas mesclados, verificando vencedor por `updated_at` e relatório de conflitos.
- **Interface:** só fluxos críticos com o contexto de teste do GPUI (criar rascunho de Listing, aprovar Price Suggestion, ligar Restricted Feature com confirmação). A interface não tem regra de negócio, então a maior parte da cobertura fica nos módulos.
- **Arquitetura:** as fronteiras entre módulos são garantidas pelas dependências do workspace Cargo; um teste falha se um crate de módulo depender de outro fora da lista permitida.

Prior art: o repositório ainda não tem código; esses padrões serão estabelecidos na primeira fatia (esqueleto do app) e copiados pelas seguintes.

## Out of Scope

- Servidor, webhooks e acesso pelo celular até a fase 3 escolher um modo de sincronização com backend.
- macOS e iOS; o núcleo fica pronto para isso, mas nenhuma interface é feita nas três fases.
- Multiusuário e multi-tenant.
- Emissão de NF-e, Mercado Envios Flex e Full (exigem NF-e) enquanto não houver CNPJ; ficam como Reminder.
- Compra automática na Shopee ou no ML como comprador (não existe API); a Purchase Order é registrada à mão.
- Gestão de campanhas de Google Ads, Meta Ads e Product Ads (só leitura de custo), blog/SEO, e-mail marketing, TikTok por API, WhatsApp Cloud API, geração de vídeo, CRM e remarketing.
- Autocompra pelo próprio link de afiliado (não é construída nem atrás de flag).

## Further Notes

- **Fase 1 em fatias finas, fases 2 e 3 como épicos**, refinados ao chegar nelas.
- **Validar com conta real antes de cada adapter** os itens marcados [I] e [T] nos docs de pesquisa, em especial: refresh token do ML, `/highlights` e `/products/search` com token, requisitos de reputação para Promotions e Product Ads, formato da Affiliate API da Shopee.
- **Pedir acesso à Shopee Affiliate Open API já**, porque a aprovação pode levar semanas; o app funciona com cadastro manual até lá.
- Reputação de vendedor novo limita Promotions (exige verde) e Product Ads (exige amarela); o app mostra quando cada um libera.
- Labels das issues no GitHub têm cada uma uma cor diferente, a pedido do Fernando.
