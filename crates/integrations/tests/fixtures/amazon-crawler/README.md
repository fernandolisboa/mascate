# Páginas de busca da Amazon (escritas à mão)

Estes arquivos **não** foram gravados do site: gravá-los seria rodar o crawler da Amazon, que as
Condições de Uso proíbem (mineração de dados, robôs e ferramentas de extração) e que fica
desligado atrás da flag `integrations.amazon_crawler` (ADR 0035). Foram escritos à mão, em
16/10/2026, a partir da estrutura pública conhecida da busca (`www.amazon.com.br/s?k={termo}`):
os blocos de resultado `data-asin` com `data-component-type="s-search-result"`. A busca não traz
JSON-LD nem estado para máquinas, então o crawler recorta só esses blocos, sem parser de HTML.
Quem ligar a flag regrava estes arquivos pelo regression pass da fase 3 (#116) e corrige o leitor
onde a página de verdade for diferente.

| Arquivo | O que imita |
|---|---|
| `robots.txt` | um robots.txt que deixa ler `/s?k=` e fecha o carrinho, o login e uma combinação de filtros; não é a cópia do robots.txt da Amazon |
| `search.html` | a primeira página de `fone bluetooth`: um separador com `data-asin=""`; um resultado patrocinado com marca num `h2` e título noutro, foto, nota, "Mais de 1 mil compras no mês passado", preço e preço riscado (`a-text-price`); um resultado com aspas, `&amp;` e `&#8211;` no título, preço com milhar e "Mais de 500 compras"; um resultado sem preço ("Ver opções de compra") e com foto fora da Amazon (entra sem preço e sem foto); o patrocinado repetido como resultado normal (sai uma vez); um bloco com ASIN inválido (fica de fora); um resultado com preço em `US$` (entra sem preço); um cartão de carrossel com `data-asin` que não é resultado (fica de fora); e o "Próximo" como link |
| `search-page-2.html` | a página seguinte, com "1 mil+ comprados no mês passado" e o "Próximo" desligado (`s-pagination-disabled`, sem link) |
| `robot-check.html` | a verificação de robô da Amazon (formulário para `/errors/validateCaptcha` e o aviso com `api-services-support@amazon.com`), servida pelos testes com 503 e com 200 |

Como o crawler lê a busca:

- O endereço é `{site}/s?k={palavras+com+mais}`, as palavras sem acento e em minúsculas; a ordem
  por menor preço acrescenta `&s=price-asc-rank`, a por mais vendidos `&s=exact-aware-popularity-rank`
  e a página N, `&page=N`. Sem categoria, sem loja e sem a ordem por comissão.
- Cada bloco é lido do seu começo até o começo do próximo, no máximo 64 KiB. O id é o ASIN do bloco
  (10 letras maiúsculas ou dígitos), o link é `https://www.amazon.com.br/dp/{ASIN}`, o título é o
  `h2` de texto mais longo, o preço é o `a-offscreen` do primeiro `a-price` que não é
  `a-text-price` (só em `R$`), a foto é o `src` de `img.s-image` (só em `media-amazon.com`), a nota
  é o `a-icon-alt` e as vendas são as compras do mês passado. O fornecedor é sempre "Amazon": a busca
  não mostra o vendedor.
- Bloco sem preço entra sem preço e não pode ser guardado nas ofertas.
- Há próxima página quando o `s-pagination-next` é um link e não está desligado.

Inventados ou completados (conferir no regression pass):

- As classes (`s-image`, `a-price`, `a-text-price`, `a-offscreen`, `a-icon-alt`,
  `s-pagination-next`, `s-pagination-disabled`) e o `data-component-type` dos resultados.
- Os parâmetros `s=price-asc-rank`, `s=exact-aware-popularity-rank` e `page`, e se o robots.txt de
  verdade deixa ler a busca com eles.
- O texto das compras do mês e da nota, e o status (503 ou 200) da verificação de robô.
- ASINs, títulos, preços, vendas e fotos.
