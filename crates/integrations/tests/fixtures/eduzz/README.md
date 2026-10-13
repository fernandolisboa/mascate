# Respostas da API da Eduzz (da documentação)

Estes arquivos **não** foram gravados com uma conta real: foram escritos a partir da documentação do
Developer Hub da Eduzz (`https://developers.eduzz.com`, com a especificação em
`https://developers.eduzz.com/api/openapi`, consultadas em 13/10/2026): "Token Pessoal",
"Criando um Aplicativo", "Padrões de resposta", "Extrato financeiro" (`GET
/myeduzz/v2/financial/statement`), "Listar vendas" (`GET /myeduzz/v1/sales`) e "Listar
transferências" (`GET /myeduzz/v1/financial/transfers`). O regression pass da fase 2 (#76) os
regrava com a conta do Fernando.

| Arquivo | Recurso | Origem |
|---|---|---|
| `statement-page-1.json`, `statement-page-2.json` | o extrato de um dia em duas páginas: comissões (`affiliante_sale`) das vendas 1001, 1003, 1006 (em euro) e 1010, reembolso da 1004, chargeback da 1005 e linhas que o app não lê (`sale`, `fee`, `bonus`) | Extrato financeiro, resposta e enum `type` |
| `statement-sale-1004.json`, `-1005.json`, `-1007.json` | o extrato de uma venda só (`saleId`), com a comissão creditada antes da janela | Extrato financeiro, filtro `saleId` |
| `statement-every-status.json` | uma comissão para cada venda de `sales-every-status.json` | Extrato financeiro |
| `statement-amounts.json` | valor em texto, venda com `saleId` numérico, comissão creditada em duas linhas e uma em dólar sem descrição | Extrato financeiro; o texto e o id numérico são inventados |
| `statement-empty.json`, `sales-empty.json` | uma página sem itens | Padrões de resposta, lista paginada |
| `sales.json` | vendas que mudaram: com produto e `tracker2`, aguardando reembolso, uma do próprio produto sem afiliado, uma paga ainda sem comissão no extrato e uma que mudou em agosto | Listar vendas, exemplo de resposta |
| `sales-every-status.json` | uma venda em cada um dos 12 status do filtro `status` | Listar vendas, enum `status` |
| `transfers.json` | transferências `confirmed` (uma com data sem hora, uma de junho e uma com `id` numérico), `pending` e `canceled` | Listar transferências, exemplo de resposta |
| `error-401.json`, `error-403.json`, `error-429.json` | token não aceito, escopo faltando e limite de requisições | Padrões de resposta, erro genérico; mensagens e códigos inventados |

Como o app chama a API:

- Base `https://api.eduzz.com`. O token pessoal (Meus Aplicativos › Copiar access token no
  console) vai como `Authorization: bearer <token>` em cada chamada; colado com `bearer` na frente,
  o app tira. Ele não expira: 401 quer dizer token trocado ou apagado e a Connection conta como não
  aceita. 400, 403 e 422 são recusa com a `message` da Eduzz (403 é o escopo faltando). Os escopos
  são `myeduzz_financial_read` (extrato e transferências) e `myeduzz_sales_read` (vendas).
- As listas vêm com `page` a partir de 1 e `itemsPerPage=100`; o app para quando `page` chega a
  `pages`, numa página vazia ou numa igual à anterior. Datas vão como `aaaa-mm-ddThh:mm:ss.mmmZ` em
  UTC, o formato do exemplo de chargebacks.
- O Sync é por data do que aconteceu, com um dia de folga (relógio e fuso). O extrato da janela
  diz o que o owner ganhou: cada `affiliante_sale` (a grafia é da Eduzz) é a comissão de uma
  venda (`saleId`); `refund`, `chargeback_blocked` e `chargerback_success` (também a grafia da
  Eduzz; `chargeback_success` também vale) estornam a venda que nomeiam. Linhas sem `saleId` e
  dos outros tipos ficam de fora.
- As vendas vêm de `GET /myeduzz/v1/sales` com `referenceDate=updatedAt`, desde 90 dias antes da
  janela (o tempo em que a comissão ainda pode ser creditada depois da última mudança da venda).
  Quando a lista mostra a venda, ela dá o Program e o produto (`product.id` e `product.name`), a
  data da venda (`createdAt`), o status e a marcação (o primeiro não vazio entre `utm.content`,
  onde os links do app levam a marcação, `tracker`, `tracker2` e `tracker3`). A documentação não
  diz qual parâmetro de link preenche os `tracker*` (conferido em 13/10/2026); o regression pass
  (#76) confere se o `utm_content` do link volta em `utm.content`.
- Uma venda com afiliado que mudou na janela, ou um estorno na janela, sem a comissão no extrato
  da janela, é procurada no extrato pelo `saleId`, nos cinco anos que o app lê. Sem
  `affiliante_sale` em lugar nenhum, a venda fica de fora: ainda não há comissão creditada (ou a
  venda é do próprio produto). Venda sem `affiliate` não é procurada.
- A chave natural é o `saleId`. A comissão é a soma das linhas `affiliante_sale` da venda (número
  ou texto, nunca por float), na moeda delas; moeda que o app não conhece fica fora. Só no
  extrato, o Program é "Eduzz", o produto é a descrição da linha e a data é a do crédito.
- Status: uma linha de estorno deixa a comissão estornada, com a data da linha. Sem estorno e com
  a venda na lista: `paid` aprovada (desde `paidAt`), `refunded` estornada (desde
  `refund.refundedAt`), `canceled`, `expired` e `duplicated` canceladas, o resto pendente
  (`open`, `processing`, `waitingRefund`, `recovering`, `waitingPayment`, `scheduled`,
  `negotiated` e qualquer status novo). Só no extrato: aprovada desde o crédito.
- O JSON bruto guardado é `{"sale": ..., "statement": [...]}`, sem `buyer`, `recipient`,
  `payment`, `affiliates`, `partners` nem o motivo e o nome de quem pediu o reembolso.
- 429, 5xx e falha de rede esperam 5, 10, 20 e 40 segundos antes de desistir (a mesma espera da
  Hotmart, da Monetizze e da Kiwify; a lista de vendas aceita 30 requisições por minuto).
- As transferências vêm de `GET /myeduzz/v1/financial/transfers`, todas as páginas a cada Sync (a
  lista não tem filtro de data). Entra como Payout a `confirmed`, pela chave `id`, com o valor e a
  moeda de `value` e a data em `completedAt` (ou `requestedAt`, ou `createdAt`), quando essa data
  cai na janela; data sem hora é meia-noite em Brasília. A conta bancária não é guardada. A Eduzz
  não diz quais vendas uma transferência paga: em Afiliação › Repasses o owner clica em "Conferir
  comissões" e confirma as que o app propõe.

Inventados ou completados (conferir no regression pass):

- Se `GET /myeduzz/v1/sales` lista as vendas em que o owner é o afiliado (ou só as dos produtos
  dele), e o que vem em `affiliate` nelas.
- Os tipos reais do extrato numa conta de afiliado: se a comissão vem mesmo como
  `affiliante_sale`, se `creditDate` é o dia da venda ou o da liberação, se o valor é positivo, o
  que vem em `description`, como aparecem reembolso e chargeback (valor negativo? um
  `chargeback_blocked` desfeito quando o chargeback é vencido?) e se parcelas ou assinaturas geram
  mais de uma linha por venda.
- Se 90 dias cobrem a distância entre a última mudança da venda e o crédito da comissão.
- Se o extrato e as vendas aceitam um intervalo de cinco anos numa consulta só, o formato de data
  aceito e o fuso.
- O máximo de `itemsPerPage` e se `pages` vem sempre.
- O limite de requisições do extrato e das transferências e o corpo de um 401, 403 e 429.
- Os formatos de data das transferências (o exemplo traz só a data) e quando `completedAt` é
  preenchido.
- Se gerar outro token pessoal invalida o anterior.
- Ids, nomes, valores, datas e e-mails.
