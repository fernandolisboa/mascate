# Respostas da Bot API do Telegram (da documentação)

Estes arquivos **não** foram gravados com um bot real: os testes não falam com o Telegram. Seguem a
documentação oficial da Bot API 10.3 (24/08/2026), consultada em 14/10/2026: "Making requests"
(envelope `ok`/`result`/`description`/`error_code`/`parameters`), `ResponseParameters`
(`retry_after`), `getMe`, `getChat` (`ChatFullInfo`), `getChatMember` (`ChatMemberAdministrator`,
`ChatMemberOwner`, `ChatMemberLeft`), `sendMessage`, `sendPhoto` e "Sending files". O regression
pass da fase 2 (#76) confere com um bot real num canal de teste.

| Arquivo | Recurso | Origem |
|---|---|---|
| `get-me.json` | `getMe`: o `User` do bot | getMe; User |
| `get-chat-public.json` | `getChat` de um canal público, com `username` | getChat; ChatFullInfo |
| `get-chat-private.json` | `getChat` de um canal privado, sem `username`, id `-100…` | getChat; ChatFullInfo |
| `get-chat-group.json` | `getChat` de um supergrupo, que não é canal | getChat; ChatFullInfo |
| `chat-member-administrator.json` | o bot administrador com `can_post_messages` | getChatMember; ChatMemberAdministrator |
| `chat-member-administrator-without-posting.json` | o bot administrador sem `can_post_messages` | ChatMemberAdministrator |
| `chat-member-left.json` | o bot fora do canal (`status` `left`) | ChatMemberLeft |
| `send-message.json` | `sendMessage` aceito num canal público: a `Message` com `message_id` e `chat.username` | sendMessage; Message |
| `send-message-private.json` | o mesmo num canal privado, sem `username` | sendMessage; Message |
| `send-photo.json` | `sendPhoto` aceito, com os tamanhos da foto e a legenda | sendPhoto; PhotoSize |
| `error-400-chat-not-found.json` | 400 de um chat que o bot não acha | Making requests |
| `error-400-wrong-photo-url.json` | 400 de uma URL de foto que o Telegram não baixa | Sending files |
| `error-401.json` | 401 de um token que o Telegram não conhece | Making requests |
| `error-403-not-a-member.json` | 403 de um bot que não está no canal | Making requests |
| `error-404.json` | 404 de um token que nem tem a forma de token | Making requests |
| `error-429.json`, `error-429-long.json` | 429 com `parameters.retry_after` de 3 e de 120 segundos | ResponseParameters |
| `error-500.json` | 500 no envelope | Making requests |
| `error-502.html` | 502 do proxy na frente da API, sem envelope | (comportamento comum de proxy; conferir) |

Como o app chama a API:

- `POST {API}/bot<token>/<método>` com corpo JSON; o token sai do cofre (`TELEGRAM_BOT_TOKEN`) e
  nunca aparece em mensagem de erro.
- "Conferir canal": `getMe` (id do bot), `getChat` (título e tipo) e, num canal, `getChatMember`
  com o id do bot: pode postar quem é `creator` ou `administrator` com `can_post_messages`.
- Texto: `sendMessage` com `chat_id` e `text`, sem `parse_mode` (nada para escapar) e com a prévia
  do link no padrão (ligada). Foto por URL: `sendPhoto` com `photo` e `caption` em JSON. Foto da
  pasta do Product: `sendPhoto` em `multipart/form-data` com `chat_id`, `caption` e o arquivo em
  `photo` (nome e `Content-Type` `image/jpeg` ou `image/png`, até 10 MB).
- `chat_id` vai como texto quando é `@nome` e como número quando é o id `-100…`.
- O link do post é `https://t.me/<username>/<message_id>` num canal público e
  `https://t.me/c/<id sem -100>/<message_id>` num privado.
- Erros pelo `error_code` (ou o status HTTP sem envelope), nunca pelo texto: 429 espera o
  `retry_after` (até 60 s, até 3 vezes) e repete, porque o Telegram garante que não publicou; 401 e
  404 são token não aceito; 400, 403 e os outros 4xx são recusa com a `description`; 5xx, resposta
  sem envelope, tempo esgotado e conexão caída depois do envio deixam o post "não confirmado", sem
  segunda chamada. Falha de conexão antes do envio (DNS, conexão recusada, TLS) não publicou nada.

Inventados ou completados (conferir no regression pass):

- Ids de bot, chat e mensagem, `date`, `file_id`, títulos e os campos de presente e reação.
- O texto exato de `description` (o app não depende dele) e se o 502 vem mesmo sem envelope.
- Se `getChatMember` de um bot recém-adicionado ao canal já traz `can_post_messages`.
