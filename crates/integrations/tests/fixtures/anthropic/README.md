# Respostas da API da Anthropic (da documentação)

Estes arquivos **não** foram gravados com uma chave real: os testes não fazem chamadas pagas. Seguem
a documentação da Messages API (`POST /v1/messages`, versão `2023-06-01`), as saídas estruturadas
(`output_config.format` com `json_schema`) e a referência de erros, consultadas em 05/10/2026. O
regression pass da fase 1 (#37) confere com uma chave real.

| Arquivo | Recurso | Origem |
|---|---|---|
| `message-copy.json` | `POST /v1/messages` respondido: um bloco `thinking` vazio (o padrão `display: "omitted"`) e o bloco `text` com o JSON do esquema (`title`, `description`), `stop_reason` `end_turn` | Messages API; Structured outputs |
| `message-refusal.json` | o mesmo, recusado pelo classificador de segurança: HTTP 200, `stop_reason` `refusal`, `stop_details` | Refusals and fallback |
| `message-max-tokens.json` | a resposta cortada em `max_tokens`, com o JSON pela metade | Messages API |
| `error-400.json` | `invalid_request_error` | Errors |
| `error-401.json` | `authentication_error` (chave errada ou revogada) | Errors |
| `error-404-model.json` | `not_found_error` de um modelo que não existe ou a organização não usa | Errors |
| `error-429.json` | `rate_limit_error` | Errors |
| `error-529.json` | `overloaded_error` | Errors |

Como o app chama a API:

- `POST {API}/v1/messages` com os cabeçalhos `x-api-key`, `anthropic-version: 2023-06-01` e
  `Content-Type: application/json`, sem streaming, `max_tokens` 16000.
- O corpo leva `model` (o das Configurações, padrão `claude-sonnet-5-5`), `system` com as regras do
  anúncio, uma mensagem `user` com os dados do produto e `output_config.format` com o esquema
  `{title, description}` (`additionalProperties: false`). `thinking` e `effort` ficam no padrão do
  modelo, para qualquer modelo atual aceitar o mesmo corpo.
- 429, 529 e outros 5xx, e falha de rede, são tentados de novo depois de 2, 4 e 8 segundos. 401 é
  chave não aceita; 400, 402, 403, 404 e os outros 4xx são recusas com o texto de `error.message`.
  `stop_reason` `refusal` é recusa; `max_tokens` é falha.

Inventados ou completados (conferir no regression pass):

- Ids, `request_id`, assinatura do bloco `thinking`, contagem de tokens e o texto do anúncio.
- Se `claude-sonnet-5-5` aceita `output_config.format` sem `effort` nem `thinking` e quanto tempo
  leva uma resposta com o pensamento adaptativo no padrão.
