# Shopee Afiliados como porta de ofertas no Catalog

A #12 traz a Shopee Affiliate Open API como Product Source: buscar itens por palavra, categoria ou loja (`productOfferV2`) e lojas por nome (`shopOfferV2`), com preço, vendas, comissão e loja, e guardar o que interessa como Supplier Offers que entram nas Opportunities. O acesso depende de aprovação da Shopee, então o app precisa funcionar sem ela. Decidimos seis coisas.

1. **Porta nova no Catalog, adapter no Integrations.** O Catalog define `OfferSource` (`search_offers`, `search_shops`, `source`) com os próprios tipos (`OfferSearch`, `OfferOrder`, `FoundOffer`, `FoundShop`, `Found`), e `mascate_integrations::ShopeeAffiliates` a implementa, como a `DemandSource` do ADR 0013. A aresta `mascate-integrations -> mascate-catalog` já existe; nenhuma aresta nova no teste de arquitetura.
2. **A busca só lê; guardar é escolha do owner.** O resultado da busca aparece na tela Ofertas com o que já está guardado (`Catalog::search_offers`, `SearchedOffer::up_to_date`). Só o que o owner manda guardar, um item ou a lista toda, vira Supplier Offer (`Catalog::keep_found_offers`). Guardar tudo o que a Shopee devolve encheria as Ofertas e faria cada Sync de demanda consultar o Mercado Livre por itens que ninguém escolheu.
3. **Idempotência pelo id da Shopee.** A migração 3 do Catalog acrescenta `source` e `source_id` aos Suppliers e `source_id` às Supplier Offers. A loja vira um Supplier achado pelo `shopId`; sem ele, um Supplier cadastrado à mão com o mesmo nome é a mesma loja e ganha o id; um nome já usado por outra loja leva o id entre parênteses; uma loja renomeada renomeia o Supplier quando o nome novo está livre. O item é achado pelo `itemId`: guardar de novo com o mesmo título, link e preço não muda nada; um preço novo entra no histórico de preço do item (mesmo `link_key`, mesmo que o link mude) e herda o Product dele. Um link do item colado à mão cai no mesmo histórico.
4. **Frete zero, preço mínimo.** A API não diz quanto custa entregar ao owner, então a oferta guardada entra com frete zero e a tela avisa; registrar o mesmo link à mão com o frete corrige. Um item com variações entra pelo menor preço (`priceMin`) e a tela mostra a faixa.
5. **Assinatura e retry no adapter, sem dependência nova.** Cada requisição é `POST /graphql` com `{"query": ...}` e `Authorization: SHA256 Credential={AppID}, Timestamp={s}, Signature=sha256(AppID + Timestamp + corpo + Secret)`, com o crate `sha2`, que já estava no `Cargo.lock`. Os argumentos vão escritos na consulta, como nos exemplos da Shopee, com o texto escapado como string JSON. Limite de requisições (código 10030 ou HTTP 429), erro de sistema (10000), HTTP 5xx e falha de rede são tentados de novo depois de 2, 4 e 8 segundos (`retry.rs`, com a pausa injetada nos testes); a assinatura é refeita a cada tentativa com o horário dela. Assinatura recusada (10020) conta como chaves não aceitas; sem acesso (10031 a 10035) e erro de negócio (11000 a 11999) são recusas com o texto da Shopee. A leitura do JSON com limite de tamanho passou para `answers.rs`, usada pelos dois adapters; o do Mercado Livre segue sem retry.
6. **Sem as duas chaves, a busca fica desligada sem erro.** A Connection da Shopee já nasce "Aguardando aprovação" (#5). Enquanto não estiver conectada, a seção Buscar na Shopee mostra o porquê e o botão fica desligado; o adapter responde `NotConnected` sem fazer requisição. O cadastro à mão continua igual.

## Considered Options

- **Estender a `DemandSource`:** uma porta só, mas ela descreve a demanda do Sales Channel (mais vendidos, concorrência, tarifa); o Mercado Livre teria de implementar busca de ofertas e a Shopee métodos de demanda que não tem.
- **Cola no app:** o app chamaria o adapter e gravaria no Catalog pelos comandos que já existem; a regra de idempotência por id da Shopee ficaria fora do módulo dono da Supplier Offer.
- **Cliente GraphQL ou crate de HMAC:** a API usa um hash simples, não HMAC, e duas consultas fixas; montar a consulta à mão evita dependência nova.
- **Guardar comissão e vendas na Supplier Offer:** servem para escolher na busca; a comissão é assunto do Affiliate na fase 2, e a Opportunity ranqueia pela margem no Sales Channel.

## Consequences

- A tela Ofertas ganha a seção Buscar na Shopee: produtos por palavra, categoria (número ou link) e loja, em quatro ordens, e lojas por nome com "Ver produtos"; cada item com loja, vendas, comissão, nota e preço, e o botão de guardar.
- O Catalog vai à migração 3.
- `MASCATE_SHOPEE_API_URL` aponta um build de desenvolvimento para um servidor falso.
- Sem circuit breaker por Connection ainda: a busca só roda no clique do owner.
- Ficam para o regression pass (#37): o endereço da API no Brasil, os campos reais de `productOfferV2` e `shopOfferV2`, se os argumentos aceitam texto escapado como JSON, `sortType: 3` nas lojas, a tolerância do `Timestamp` e o limite real de requisições.
