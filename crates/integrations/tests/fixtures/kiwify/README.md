# Respostas da API pública da Kiwify (da documentação)

Estes arquivos **não** foram gravados com uma conta real: foram escritos a partir da documentação
pública da API (`https://docs.kiwify.com.br/api-reference`, com a especificação OpenAPI em
`https://docs.kiwify.com.br/api-reference/openapi.json`, consultadas em 13/10/2026): "Informações
Gerais", "Gerar token OAuth", "Listar vendas", "Consultar venda" e "Listar saques". O regression
pass da fase 2 (#76) os regrava com a conta do Fernando.

| Arquivo | Recurso | Origem |
|---|---|---|
| `token.json` | `POST /v1/oauth/token` (formulário com `client_id` e `client_secret`) | Gerar token OAuth, exemplo `Success` |
| `token-text-expiry.json` | o mesmo, com `expires_in` em texto | o esquema `OAuthSuccessResponse` dá `expires_in` como texto; o exemplo, como número |
| `token-refused.json` | credenciais que a Kiwify não conhece (HTTP 400) | Gerar token OAuth, exemplo `InvalidClient` |
| `error-invalid-token.json` | `GET /v1/sales` com token vencido (HTTP 400 ou 401) | Listar vendas, resposta 400 |
| `error-bad-query.json` | consulta recusada (HTTP 400) | `error` e `message` como no esquema `OAuthErrorResponse`; código e texto inventados |
| `error-429.json` | limite de 100 requisições por minuto | Informações Gerais cita o 429; o corpo é inventado |
| `empty.json` | uma página sem vendas | Listar vendas, `PaginatedResponse` |
| `sales-every-status.json` | uma venda em cada um dos 11 status do filtro `status` | Listar vendas com `view_full_sale_details`, parâmetro `SaleStatus` |
| `sales-refunded.json` | a venda `paid` de `sales-every-status.json`, agora reembolsada | Listar vendas |
| `sales-shares.json` | marcação em `sck` e em `s2`, comissão em centavos como número e como texto, venda em dólar, venda do próprio produto (`sale_type: producer`), venda sem afiliado, venda em euro e marcação em branco | Listar vendas com `view_full_sale_details` |
| `sales-page-1.json`, `sales-page-2.json` | uma página cheia de 100 vendas e outra com 3 | Listar vendas, `page_size` e `page_number` |
| `sales-small-page-1.json`, `sales-small-page-2.json` | páginas de 10 quando o app pede 100 | inventado: a documentação não dá o máximo de `page_size` |
| `payouts.json` | saques pago, aprovado, em análise, recusado e um pago em junho | Listar saques; só `pending_review` aparece na documentação |

Como o app chama a API:

- O token sai de `POST https://public-api.kiwify.com/v1/oauth/token` com `client_id` e
  `client_secret` em formulário e vai como `Authorization: Bearer` em cada chamada, junto do
  cabeçalho `x-kiwify-account-id` com o Account ID da conta. O app o reusa até 5 minutos antes de
  `expires_in` (24 horas no exemplo); duas leituras ao mesmo tempo esperam o mesmo pedido.
- A Kiwify responde 400 tanto para token vencido quanto para consulta errada; o `error`
  `auth_error` (ou uma mensagem sobre token) renova o token uma vez e, se voltar, as chaves contam
  como não aceitas. Outro 400 ou 403 é recusa com a mensagem da Kiwify; 400, 401 ou 403 no pedido
  de token é chave não aceita.
- As vendas vêm de `GET /v1/sales` com `view_full_sale_details=true` (sem ele não vêm
  `affiliate_commission`, `tracking` nem `sale_type`), `page_size=100` e `page_number` a partir
  de 1. O app para numa página vazia, numa menor que o `page_size` que a Kiwify devolveu ou numa
  igual à anterior.
- O Sync é por data de mudança: `updated_at_start_date`/`updated_at_end_date` cobrem o que mudou
  desde o último Sync menos um dia de folga (relógio e fuso). Como `start_date`/`end_date` são
  obrigatórios e ficam a no máximo 90 dias um do outro, cada janela de mudanças é pedida junto
  com as vendas feitas desde 180 dias antes dela (estorno e chargeback chegam meses depois da
  compra), em janelas de 89 dias. Um Sync de hora em hora faz 3 consultas; o primeiro pede cinco
  anos de mudanças, em janelas de 89 dias também. As datas vão como `aaaa-mm-dd hh:mm:ss.mmm`
  em UTC, o formato da seção de perguntas de Informações Gerais.
- A chave natural é o `id` da venda. A comissão é `affiliate_commission.amount`, em centavos
  (número ou texto, nunca por float), na moeda `currency` da venda; sem moeda, real; outra moeda
  que o app não conhece fica fora. Venda sem `affiliate_commission` ou com `sale_type` de produtor
  ou coprodutor fica fora: a comissão ali é de outro afiliado.
- A data da venda é `created_at`; a mudança de status é `approved_date` para aprovada e
  `refunded_at` para estornada. A marcação é a primeira não vazia entre `tracking.sck`, `src`,
  `s1`, `s2` e `s3`. O produto e o Program são `product.id` e `product.name`. Saem do JSON bruto
  guardado: `customer`, `revenue_partners`, `card_last_digits`, `boleto_url` e o documento do
  afiliado.
- Status: `waiting_payment`, `pending`, `processing`, `authorized`, `refund_requested` e
  `pending_refund` ficam pendentes; `approved` e `paid`, aprovadas; `refunded` e `chargedback`,
  estornadas; `refused`, cancelada. Um status novo fica pendente até o app aprendê-lo.
- 429, 5xx e falha de rede esperam 5, 10, 20 e 40 segundos antes de desistir (a mesma espera da
  Hotmart e da Monetizze; o limite é de 100 requisições por minuto).
- Os saques vêm de `GET /v1/payouts`, todas as páginas a cada Sync (a lista não tem filtro de
  data). Entra como Payout o saque com status `paid`, `approved` ou `completed`, pela chave `id`,
  com o valor em centavos em reais e a data de pagamento em `updated_at` (ou `created_at`), quando
  essa data cai na janela do Sync. A Kiwify não diz quais vendas um saque paga: em Afiliação ›
  Repasses o owner clica em "Conferir comissões", o app propõe as aprovadas mais antigas da
  Kiwify até o valor e nada fica pago antes da confirmação.

Inventados ou completados (conferir no regression pass):

- Se uma conta só de afiliado consegue criar a API Key em Apps › API e enxerga em `/v1/sales` as
  vendas que indicou; o valor de `sale_type` nessas vendas e se `affiliate_commission.amount` é
  mesmo o que o afiliado recebe (e em que moeda, quando a venda não é em real).
- Se `updated_at_start_date`/`updated_at_end_date` também têm o limite de 90 dias, se
  `start_date`/`end_date` filtram pela data da venda, o fuso das datas e se `updated_at` muda num
  estorno ou chargeback.
- O máximo de `page_size`, o que `pagination.count` conta e se a página começa em 1.
- Os status de um saque além de `pending_review`, a ordem da lista e se `updated_at` é a data do
  pagamento.
- O corpo de um 429 e de um 403 por escopo faltando (vendas ou financeiro).
- Ids, nomes, valores, datas e e-mails.
