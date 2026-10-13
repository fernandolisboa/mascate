# Respostas da API de vendas da Hotmart (da documentação)

Estes arquivos **não** foram gravados com uma conta real: foram escritos a partir da documentação
pública da Hotmart (Hotmart Developers), consultada em 13/10/2026: autenticação (`start/app-auth`),
paginação (`start/pagination`), limite de requisições (`start/rate-limit`), histórico de vendas
(`v1/sales/sales-history`) e comissões das vendas (`v1/sales/sales-commissions`), além de
`docs/platform-integrations.md`. O regression pass da fase 2 (#76) os regrava com a conta do Fernando.

| Arquivo | Recurso | Origem |
|---|---|---|
| `token.json` | `POST /security/oauth/token?grant_type=client_credentials&client_id=…&client_secret=…` com `Authorization: Basic …` | Autenticação, exemplo de retorno |
| `token-refused.json` | o mesmo, com credenciais que a Hotmart não aceita (HTTP 401) | Autenticação (inventado: a documentação não mostra o corpo do erro) |
| `history-every-status.json` | `GET /payments/api/v1/sales/history`: uma venda em cada um dos 17 status documentados e uma num status que ainda não existe | Histórico de vendas |
| `commissions-every-status.json` | `GET /payments/api/v1/sales/commissions`: a comissão de cada venda acima, com a parte do produtor e a do afiliado | Comissões das vendas |
| `history-page-1.json`, `history-page-2.json` | duas páginas do histórico ligadas por `page_info.next_page_token`: venda aprovada com `sck` e `src`, venda completa em dólar, boleto aguardando pagamento com `src` e venda em euro | Histórico de vendas e paginação |
| `commissions-pages.json` | as comissões dessas quatro vendas | Comissões das vendas |
| `history-refunded.json`, `commissions-refunded.json` | a venda aprovada de antes, agora reembolsada | Histórico e comissões |
| `empty.json` | uma página sem vendas | Paginação |
| `error-401.json` | token que a Hotmart não aceita mais | Códigos de resposta (corpo inventado) |
| `error-429.json` | limite de requisições | Limite de requisições (corpo inventado) |

Como o app chama a API:

- O token sai de `https://api-sec-vlc.hotmart.com/security/oauth/token` com `client_id` e
  `client_secret` na query e o token Basic da credencial no cabeçalho; o app aceita o Basic colado
  com ou sem a palavra `Basic` na frente. O token de acesso dura `expires_in` segundos (cerca de dois
  dias) e é renovado cinco minutos antes; duas leituras ao mesmo tempo esperam o mesmo pedido de
  token. Um 401 numa leitura renova o token uma vez; um segundo 401, ou um 400/401/403 no pedido de
  token, conta como chaves não aceitas.
- As vendas vêm de `https://developers.hotmart.com/payments/api/v1/sales/history` e
  `/sales/commissions`, com `commission_as=AFFILIATE`, `start_date` e `end_date` em milissegundos UTC
  e `max_results=100`, página a página por `page_token`. Sem `transaction_status` a Hotmart só
  devolve APPROVED e COMPLETE, então o app pede cada um dos 17 status nos dois endpoints (34
  requisições por Sync, mais as páginas).
- Histórico e comissões se juntam pela `transaction`, que é a chave natural. A comissão é a entrada
  de `commissions[]` com `source` AFFILIATE (`commission.value` na moeda `commission.currency_value`).
  A data da venda é `order_date`; a mudança de status é `approved_date` para APPROVED e
  `warranty_expire_date` para COMPLETE. A marcação é `tracking.source_sck` ou, sem ela,
  `tracking.source` (o `src`). O comprador é tirado do JSON bruto guardado.
- Status: WAITING_PAYMENT, PRINTED_BILLET, STARTED, PROCESSING_TRANSACTION, UNDER_ANALISYS,
  PRE_ORDER, OVERDUE e BLOCKED ficam pendentes; APPROVED e COMPLETE, aprovadas; REFUNDED,
  PARTIALLY_REFUNDED, CHARGEBACK e PROTESTED, estornadas; CANCELLED, EXPIRED e NO_FUNDS, canceladas.
  Um status desconhecido fica pendente com o nome bruto guardado.
- 429, 5xx e falha de rede esperam 5, 10, 20 e 40 segundos antes de desistir (o limite é de 500
  chamadas por minuto).
- O Sync relê os últimos 35 dias a cada vez: a garantia mais longa da Hotmart (7, 15, 21 ou 30 dias)
  mais os dias de um boleto. O primeiro Sync pede cinco anos.
- No teste de status, as 18 vendas vêm todas na resposta de WAITING_PAYMENT para caber em um
  arquivo; o app lê o status de cada venda, não o do filtro.

Inventados ou completados (conferir no regression pass):

- Os corpos de erro (`error`, `error_description`) e o corpo do 401 no pedido de token.
- Se uma conta só de afiliado consegue criar credenciais em Ferramentas › Credenciais Developers.
- Os campos reais de `/sales/commissions` para o afiliado (se `source` vem AFFILIATE e se pode haver
  mais de uma entrada AFFILIATE), o tamanho máximo de página e se `transaction_status` aceita vários
  status de uma vez (cortaria o Sync de 34 para 2 requisições).
- Se `start_date`/`end_date` filtram pela data do pedido e se aceitam uma janela de cinco anos.
- Se as recorrências de uma assinatura têm `transaction` próprio.
- Moedas em que as comissões chegam: o app só guarda real e dólar; outra moeda fica fora do Sync.
- Ids, nomes, valores, datas e o token.
