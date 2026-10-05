# Listing Copy no Marketing, escrita por uma porta

A #17 põe no rascunho de Listing um botão que escreve título e descrição pela API da Anthropic, com os dados do Product, das Supplier Offers, os atributos da categoria e os termos em alta da categoria no Mercado Livre. O texto entra no rascunho para o Fernando editar, respeita o limite de título da categoria e nunca traz contato nem link para fora do canal. O PRD põe a Listing Copy no Marketing. Decidimos cinco coisas.

1. **O Marketing é dono da Listing Copy e não lê o Commerce nem o Catalog.** `ListingCopy::write(canal, redator, CopyBrief)` recebe do app o nome do Product, os títulos das Supplier Offers, a categoria, a condição e os atributos como estão digitados, como na qualidade e nas perguntas (ADR 0021, 0022). O Marketing monta o pedido (as regras fixas em `COPY_INSTRUCTIONS` e os dados entre `<dados>` e `</dados>`, sem `<` nem `>` vindos dos dados) e confere o que voltou. A única tabela nova é `marketing_copy_settings` (migração 5 do Marketing), com o modelo escolhido; o texto gerado não é guardado à parte, porque ele só existe como rascunho.
2. **Duas portas, implementadas pelos adapters.** `CopyChannel` dá o limite de título e de descrição da categoria (`GET /categories/{id}`, `settings.max_title_length` e `max_description_length`, com 60 e 50.000 quando faltam) e os termos em alta (`GET /trends/MLB/{id}`, o `keyword` de cada um; 404 é categoria sem tendências); o `MercadoLivre` a implementa. `CopyWriter` escreve; `mascate_integrations::Anthropic` a implementa com `POST /v1/messages`, a chave da Connection Anthropic no cofre e saída estruturada (`output_config.format` com o esquema `{title, description}`). Nenhuma aresta nova no teste de arquitetura: `integrations -> marketing` já existe.
3. **Conferir depois de escrever, uma segunda chance, nunca texto proibido no rascunho.** O texto é arrumado (título numa linha, descrição sem espaço sobrando) e conferido por `check_copy`: título e descrição não vazios, dentro dos limites, e sem contato pela mesma regra das respostas (`contact_in`: telefone, e-mail, link que não seja `https` do Mercado Livre, domínio, `@perfil`, rede social). Com algum problema, o pedido vai mais uma vez dizendo o que estava errado. Se o título ainda passa do limite, ele é cortado na última palavra que cabe e a tela avisa; qualquer outro problema falha sem tocar no rascunho. Os termos em alta que o título carrega (todas as palavras, sem acento e caixa) aparecem na tela.
4. **Cliente HTTP que já existe, sem SDK.** Não há SDK oficial da Anthropic para Rust; o adapter usa o `ureq` do app (ADR 0010), com `answers::read_json` e o `retry.rs` dos outros adapters: 429, 529, outros 5xx e falha de rede são tentados de novo depois de 2, 4 e 8 segundos; 401 é chave não aceita; os outros 4xx são recusas com o texto da Anthropic (um modelo que não existe responde 404). `stop_reason` `refusal` é recusa e `max_tokens` é falha. Como a resposta sem streaming só começa quando o texto está pronto, o `mascate-platform` ganhou `slow_http_agent`, com até 5 minutos para a resposta começar.
5. **Modelo configurável, padrão `claude-sonnet-5-5`.** A issue pede o mais recente da família Claude adequado ao custo: o Sonnet 5.5 escreve bem um texto curto pela metade do preço do Opus 5.5. O Fernando troca em Configurações › Títulos e descrições com IA por qualquer nome de modelo (letras, números e `-._:@`, até 100 caracteres). O pedido não manda `thinking` nem `effort`, para qualquer modelo atual aceitar o mesmo corpo; também não usa o `fallbacks` do lado do servidor, que nem todo modelo aceita: uma recusa do classificador de segurança aparece como recusa e o Fernando escreve à mão ou gera de novo.

Sem a chave da Anthropic, o botão fica desligado com a orientação de colar a chave em Configurações › Conexões; também pede o Mercado Livre conectado e a categoria escolhida, porque o limite e os termos vêm dela. O texto entra nos campos do editor e só é gravado quando o Fernando salva; publicar continua sendo só no clique dele (ADR 0016).

## Considered Options

- **Crate próprio de IA:** isola o assunto, mas cria arestas novas no teste de arquitetura para uma tela só, e a Listing Copy é Marketing no PRD.
- **No Commerce, junto do rascunho:** nenhuma aresta nova, mas contraria a divisão de módulos e engorda o Commerce, que já tem Listings, Orders, Fees e preços.
- **SDK de terceiros para Rust:** dependência nova não oficial para uma chamada só; o corpo é pequeno e o retry já existe.
- **Cortar o título sem pedir de novo:** mais barato, mas um título cortado perde o fim, onde costumam estar cor e modelo; a segunda chance só custa quando a primeira falha.

A pergunta foi ao Fernando num card; ele escolheu a porta no Marketing.

## Consequences

- O editor do rascunho ganha "Gerar título e descrição com IA" (e "Gerar de novo com IA"), com o limite de título da categoria no contador e os termos em alta que entraram no título.
- Configurações ganha a seção Títulos e descrições com IA, com o modelo.
- Cada geração custa uma chamada à API da Anthropic, duas quando a primeira volta fora das regras, além de duas leituras no Mercado Livre.
- `MASCATE_ANTHROPIC_API_URL` aponta um build de desenvolvimento para um servidor falso.
- Fica para o regression pass (#37): uma geração real com a chave (tempo de resposta, se `claude-sonnet-5-5` aceita o corpo sem `effort`, qualidade do texto), se `/trends` pede token e como responde uma categoria sem tendências, e se os limites de título reais do MLB batem com `settings`.
