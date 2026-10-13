# Respostas da API 2.1 da Monetizze (da documentação)

Estes arquivos **não** foram gravados com uma conta real: foram escritos a partir da documentação
pública da API 2.1 (`https://api.monetizze.com.br/2.1/apidoc/`, gerada em 23/02/2023 e consultada
em 13/10/2026): `GET /token`, `GET /transactions` e `GET /myproducts`, além da página de ajuda
"API Monetizze" (Menu › Ferramentas › API) e de `docs/platform-integrations.md`. O regression pass
da fase 2 (#76) os regrava com a conta do Fernando.

| Arquivo | Recurso | Origem |
|---|---|---|
| `token.json` | `GET /token` com o cabeçalho `X_CONSUMER_KEY` | Token, exemplo de sucesso |
| `token-refused.json` | o mesmo, com uma chave que a Monetizze não aceita (HTTP 403) | Token, "Credenciais de API inválidas" |
| `error-invalid-token.json` | `GET /transactions` com um token vencido (HTTP 403) | Transactions, "Token inválido" |
| `error-invalid-key.json` | `GET /transactions` com chave inválida (HTTP 403) | Transactions, "Chave inválida" |
| `error-access-denied.json` | chave sem permissão para o método (HTTP 403) | MyProducts, "001-Acesso negado!" |
| `error-429.json` | limite de requisições | inventado: a documentação não publica limite |
| `error-in-answer.json` | HTTP 200 com o campo `error` preenchido | Transactions, campo `error` (mensagem inventada) |
| `empty.json` | uma página sem vendas | Transactions |
| `status-1.json` … `status-6.json` | uma venda em cada status (1 Aguardando pagamento, 2 Finalizada, 3 Cancelada, 4 Devolvida, 5 Bloqueada, 6 Completa), na resposta do filtro do próprio status | Transactions, filtro `status` e campo `venda.status` |
| `shares.json` | seis vendas finalizadas: o afiliado dono entre outros afiliados (por e-mail, como Co-Afiliado), as partes ao lado de `venda` sem e-mail e valor como número JSON, uma em dólar, uma em euro, uma com dois outros afiliados e uma só com produtor | Transactions, `comissoes[]` nos dois lugares em que a documentação os mostra |
| `page-1.json`, `page-2.json` | uma página cheia de 100 vendas e outra com 3; a segunda sem a lista em volta | Transactions, parâmetro `page` |
| `returned.json` | a venda finalizada de `status-2.json`, agora devolvida | Transactions |

Como o app chama a API:

- O token sai de `GET https://api.monetizze.com.br/2.1/token` com a chave da conta no cabeçalho
  `X_CONSUMER_KEY` e vai no cabeçalho `TOKEN` de cada chamada. Ele vence 15 minutos depois do último
  uso; o app o reusa enquanto foi usado há menos de 14 minutos e pede outro depois disso (na prática,
  um por Sync de hora em hora). Duas leituras ao mesmo tempo esperam o mesmo pedido de token.
- A Monetizze responde 403 tanto para token vencido quanto para chave inválida; só a mensagem
  (`Error`) diz qual. Uma mensagem sobre token ou chave renova o token uma vez; se voltar, a chave
  conta como não aceita e a Conexão pede para conferir as chaves. Outro 403 (como "Acesso negado")
  é uma recusa com as palavras da Monetizze. Um 403 no pedido de token é chave não aceita.
- As vendas vêm de `GET /transactions` com `date_min` e `date_max` (data de cadastro da venda, em
  `aaaa-mm-dd hh:mm:ss` no horário de Brasília, UTC−3, sem horário de verão desde 2019),
  `status[]` com um código por vez e `page` a partir de 1. A página tem 100 vendas; o app para numa
  página com menos de 100, vazia ou igual à anterior. Pedir status por status dá 6 requisições por
  Sync, mais as páginas, e o status da Commission sai do código pedido, não do texto mostrado.
- A documentação mostra a resposta dentro de uma lista (`[ { "status": 200, "dados": [...] } ]`);
  o app aceita com ou sem a lista. Um `error` não vazio numa resposta 200 é uma recusa.
- A lista de campos põe `produto` e `comissoes` dentro de `venda`; o exemplo, ao lado. O app lê os
  dois lugares, preferindo `venda`.
- A chave natural é `venda.codigo`. A comissão é a linha de `comissoes[]` cujo `email` é o e-mail da
  conta informado na Conexão (sem diferença de maiúsculas); se nenhuma linha tiver esse e-mail, vale
  a única linha com `tipo_comissao` "Afiliado" ou "Afiliado Premium"; uma venda com outros afiliados
  e nenhum com o e-mail do dono fica fora. `valor` vem como texto decimal ou número e nunca passa
  por float. A moeda é `venda.moeda`; sem ela, real. Outra moeda que o app não conhece fica fora.
- A data da venda é `venda.dataInicio`; a mudança de status, `venda.dataFinalizada`, só para
  Finalizada e Completa. A marcação é `venda.src`. O produto e o Program são `produto.codigo` e
  `produto.nome`. O comprador (`comprador`) é tirado do JSON bruto guardado.
- Status: 1 Aguardando pagamento e 5 Bloqueada ficam pendentes; 2 Finalizada e 6 Completa,
  aprovadas; 4 Devolvida, estornada; 3 Cancelada, cancelada.
- 429, 5xx e falha de rede esperam 5, 10, 20 e 40 segundos antes de desistir (a mesma espera da
  Hotmart; a Monetizze não publica limite).
- O Sync relê os últimos 35 dias a cada vez: a garantia mais longa (30 dias) mais os dias de um
  boleto. O primeiro Sync pede cinco anos.
- A Monetizze não informa repasses por esta API: o owner registra o Payout em Afiliação.

Diferença em relação à issue #82: a issue previa achar a linha do dono pelo `refAfiliado` vindo de
`/myproducts`, mas a documentação atual de `/myproducts` não traz `refAfiliado` nem nada que
identifique o afiliado. Por isso a Conexão pede também o e-mail da conta (não é segredo e aparece
sem máscara) e `/myproducts` não é chamado.

Inventados ou completados (conferir no regression pass):

- Se a chave de uma conta só de afiliado enxerga as vendas que ele indicou em `/transactions`.
- Qual linha de `comissoes[]` é a do afiliado: se ela traz `email` e `refAfiliado`, e o texto exato
  de `tipo_comissao` para afiliado.
- Onde `produto` e `comissoes` chegam de fato (dentro de `venda` ou ao lado) e se a resposta vem
  dentro de uma lista.
- O formato aceito em `date_min`/`date_max` (a documentação escreve `hh-nn-ss`), o fuso das datas,
  se o filtro de status é `status[]=2` ou `status=2`, se `page` começa em 1 e o que `pages` conta.
- Se `venda.moeda` vem sempre e se a data de PHP vazia (`0000-00-00 00:00:00`) aparece.
- O corpo de um 429 e se ele existe; se o token vence mesmo 15 minutos depois do último uso.
- Ids, nomes, valores, datas, e-mails e o token.
