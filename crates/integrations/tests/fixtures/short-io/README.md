# Respostas do Short.io (da documentação)

Estes arquivos **não** foram gravados com uma conta real: os testes não usam chave nem domínio de
verdade. Seguem a referência da API do Short.io (`POST https://api.short.io/links`, o registro de
link e os códigos de erro do `createLink` no SDK oficial `@short.io/client-node`; e
`GET https://statistics.short.io/statistics/link/{linkId}`, "Link statistics", com o esquema
`LinkStatistics`) e o guia de chave de API, consultados em 22/10/2026. O regression pass da fase 3 (#116) confere com uma conta e um
domínio `*.short.gy` reais e grava estes arquivos de novo.

| Arquivo | Recurso | Origem |
|---|---|---|
| `link.json` | `POST /links` respondido com o link: `idString`, `path`, `shortURL`, `secureShortURL`, `originalURL`, `duplicate` | API reference: Create a new link; Link record |
| `link-off-domain.json` | o mesmo, com o link curto em outro domínio (o app recusa) | inventado |
| `error-400.json` | pedido inválido: `{code, message, success, statusCode}` | Create a new link (400) |
| `error-401.json` | chave não aceita | Create a new link (401) |
| `error-403.json` | chave sem acesso ao domínio (a chave secreta é restrita a um time ou domínio) | Create a new link (403); Creating an API key |
| `error-409.json` | o caminho já existe com outro `originalURL` | Create a new link (409) |
| `error-429.json` | limite de pedidos (50 por segundo nesta rota) | SDK, tratamento de erros |
| `link-statistics.json` | cliques de um link por dia: `clickStatistics.datasets[0].data` com `{x, y}`, `x` o instante em que o dia começa | Link statistics (`LinkStatistics`, `ClickStatistics`, `DatasetItem`) |
| `link-statistics-by-date.json` | o mesmo com `x` só a data e um `y` em texto | inventado, para o app aceitar as duas formas |
| `link-statistics-empty.json` | um período sem pontos no gráfico | inventado |
| `error-404.json` | link que o Short.io não tem mais | inventado (a referência só documenta o 200) |

Como o app chama a API:

- `POST {API}/links` com `Authorization: <chave secreta>`, `Content-Type: application/json` e o
  corpo `{domain, originalURL, path, allowDuplicates: false}`. O caminho sai do id do Affiliate
  Link (8 letras minúsculas e dígitos); com o mesmo caminho e o mesmo `originalURL`, o Short.io
  devolve o link que já existe, então pedir de novo é seguro.
- O domínio colado pode vir com `https://` e barra no fim; o app manda só o nome.
- 429, 5xx e falha de rede são tentados de novo depois de 1, 2 e 4 segundos. 401 é chave não
  aceita (a Connection fica expirada). 400, 402, 403, 404, 409 e os outros 4xx são recusas com o
  texto de `message` (ou `error`).
- O app guarda `secureShortURL` só se for https no domínio do Fernando, e `idString` para ler os
  cliques (#159).
- Cliques (#159): `GET {STATISTICS}/statistics/link/{idString}?period=custom&startDate=AAAA-MM-DD
  &endDate=AAAA-MM-DD&tz=America/Sao_Paulo&clicksChartInterval=day&skipTops=true`, com a mesma
  chave. A referência diz que `endDate` conta os cliques "antes" dela, então o app manda o dia
  seguinte ao último pedido e descarta qualquer dia fora da janela. `x` pode vir como data ou
  como o instante em que o dia começa; o dia é o de Brasília. 404 é link que o Short.io não tem
  mais (o app pula esse link e guarda os cliques que já tinha); 401, 429 e 5xx como acima.

Inventados ou completados (conferir no regression pass):

- Ids, datas, `DomainId`, `OwnerId` e o texto exato das mensagens de erro.
- O corpo exato do 401, do 403 e do 429, que a referência não mostra.
- Se o Short.io aceita o caminho de 8 caracteres num domínio `*.short.gy` do plano grátis.
- Se `endDate` inclui ou não o próprio dia, a forma exata de `x` com `tz` e se os dias sem clique
  vêm com `y: 0`; o corpo do 404 e o limite de pedidos da API de estatísticas.
