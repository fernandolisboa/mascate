# Adapters implementam as portas que os módulos definem

A #11 é a primeira fatia em que um adapter de Platform alimenta outro módulo: a demanda do Mercado Livre vira Opportunities no Catalog. O módulo que usa o dado define a interface do papel de que precisa (aqui, `DemandSource` no `mascate-catalog`, com os tipos do próprio Catalog), e o adapter da Platform no `mascate-integrations` a implementa. O Catalog orquestra o Sync de demanda (quais categorias, quais ofertas comparar, o que guardar e o que descartar) e nunca sabe qual Platform respondeu. O `mascate-integrations` passa a depender de `mascate-catalog`; as próximas portas seguem o mesmo caminho (Sales Channel no Commerce, programas de afiliado no Affiliate), cada uma registrada na lista do teste de arquitetura.

## Considered Options

- **Cola no app:** o Catalog definiria a porta e o app embrulharia o cliente do Mercado Livre para implementá-la, sem dependência nova entre módulos. Cada papel de cada Platform ganharia código de tradução no app, que deve ser uma camada fina.
- **Sync no Integrations:** o adapter orquestraria o Sync e gravaria pelos comandos públicos de cada módulo. A regra de cruzamento ficaria fora do módulo dono da Opportunity.

## Consequences

- Dependência permitida nova no teste de arquitetura: `mascate-integrations -> mascate-catalog`. Nenhum módulo depende do `mascate-integrations`.
- Testes do Catalog usam uma porta falsa em memória; os do adapter usam o servidor HTTP falso com respostas gravadas (fixtures da documentação até o regression pass, #37).
- A alíquota de imposto fica no Finance (`Taxes`), dono das margens e do painel; quem lista Opportunities passa a alíquota ao Catalog. A #19 reaproveita a mesma configuração.
- O que dois módulos precisam dizer sobre uma Platform no mesmo idioma fica no kernel: `PlatformError` (como uma chamada falha) e `ListingType`, usados pela porta de demanda do Catalog e pela porta de Sales Channel do Commerce (#15). A tradução para os ids de cada Platform fica no adapter.
