# Categorias do Mascate: categorias do Fernando que juntam categorias do Sales Channel, com palavras-chave que filtram

Depois da #235, o Fernando quis acompanhar "Café" em Oportunidades, e não só para café: para qualquer assunto. "Café" não é uma categoria de primeiro nível no Mercado Livre; café em grãos fica em Alimentos e Bebidas, cafeteiras e moedores em Eletrodomésticos. O "Escolher categorias" só listava as de cima (`/sites/MLB/categories`), e a documentação dos mais vendidos (`/highlights/MLB/category/{id}`, consultada em 11/11/2026) diz que uma categoria sem ranking costuma ser uma que não é folha: seguir só as de cima perde a maior parte dos rankings. O Fernando disse sim, em 08/11/2026, a categorias próprias que juntam categorias do Mercado Livre e palavras-chave opcionais. Decidimos quatro coisas.

1. **A Mascate Category é do Catalog.** Uma Mascate Category tem nome (único, comparado como o Fernando lê: sem acento e sem caixa), uma ou mais categorias do Sales Channel, de qualquer nível, e palavras-chave opcionais. A tabela nova `catalog_mascate_categories` (Catalog v4) guarda nome e palavras; `catalog_demand_categories` passa a ser o vínculo, com a coluna `mascate_category_id` e a chave (categoria sua, categoria do canal), então a mesma categoria do Mercado Livre pode estar em duas categorias suas. Os mais vendidos continuam guardados por categoria do canal, como antes.

2. **As palavras-chave filtram.** Sem palavras, entram todos os mais vendidos das categorias do canal; com elas, só os que têm uma delas no título, como palavras inteiras, sem acento e sem caixa ("moedor manual" pega "Moedor Manual de Café", não "Moedor elétrico, manual de uso"). A lista continua sendo de mais vendidos, com posição. Recomendado num card ao Fernando em 11/11/2026; a outra opção ficou nas consideradas.

3. **A porta `DemandSource` desce na árvore.** Ganha `category(id)` (a categoria, o caminho desde o topo e as subcategorias, por `/categories/{id}`) e `find_categories(palavras)` (o preditor de categorias do Mercado Livre, `/sites/MLB/domain_discovery/search`, cada sugestão uma vez e com o caminho). A tela navega pela árvore e busca por palavra; `root_category` passa a usar `category`. O Catalog continua sem saber que é o Mercado Livre: só o vínculo com o Mercado Livre existe hoje, e outro Sales Channel entra como outra `DemandSource`.

4. **O Sync lê cada categoria do canal uma vez e junta tudo.** Uma categoria do Mercado Livre em duas categorias suas é lida uma vez por Sync. A lista dos mais vendidos mostra cada produto uma vez, com a posição em cada categoria do Mercado Livre e a melhor posição em cada categoria sua; os chips da tela são as categorias suas. Os pontos de mais vendido na pontuação das Opportunities vêm da mesma lista, então uma palavra-chave que tira um produto da lista também tira os pontos dele.

A migração transforma cada categoria seguida numa categoria sua de mesmo nome que a contém, usando o id da linha seguida como id da nova (nenhum id é inventado em SQL). Ela só acrescenta tabela, coluna e linhas e troca um índice: não é `risky`.

## Considered Options

- **Palavras-chave que trazem mais produtos pela busca:** cada palavra buscaria no catálogo do Mercado Livre e somaria o que achasse, mas por relevância, sem posição de venda, misturando busca com ranking.
- **Só subcategorias no "Escolher categorias", sem categorias suas:** resolve o Café em grãos, mas não junta cafeteiras e moedores de outro ramo num chip só.
- **Coluna `channel` no vínculo:** deixaria pronto para outro Sales Channel, mas nenhum outro tem `DemandSource` hoje (YAGNI); a coluna entra com o segundo.
- **Palavras-chave numa tabela própria:** uma linha por palavra; nada consulta palavra por palavra, então uma coluna com uma por linha basta.

## Consequences

- Fatia #237 sob o PRD #3. Migração Catalog v4. Nenhuma dependência, crate ou aresta nova.
- Merge Import conhece `catalog_mascate_categories` (chave: o nome sem caixa) e o vínculo pela categoria sua.
- Glossário: Mascate Category entra no `CONTEXT.md`, e Best Seller passa a falar dela; o PRD ganha a história 119.
- As fixtures da árvore e do preditor seguem a documentação; o regression pass da fase 3 (#116) confere com a conta do Fernando quais níveis têm ranking e o que o preditor sugere.
