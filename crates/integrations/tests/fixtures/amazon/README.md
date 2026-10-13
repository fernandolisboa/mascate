# Relatório de ganhos do Amazon Associados (da documentação)

Estes arquivos **não** foram exportados de uma conta real: foram escritos a partir da documentação
pública do Associates Central ("How to read the Earnings Report",
`https://affiliate-program.amazon.com/help/node/topic/GCDWWRFEMW3Q2TE2`, consultada em 13/10/2026),
da lista de colunas da issue #83 e de um relatório "Fee-Earnings" do programa americano publicado
como dado de teste de um importador de código aberto (linha de título antes do cabeçalho, colunas de
dinheiro com a moeda entre parênteses, devolução como linha própria com `Items Shipped` 0,
`Returns` 1 e valores negativos). O regression pass da fase 2 (#76) os troca por um relatório
exportado da conta do Fernando, em CSV e em XLSX.

| Arquivo | O que tem |
|---|---|
| `earnings-2026-09.csv` | setembro como o programa americano escreve: título, `,`, ponto decimal, UTF-8 sem BOM. Oito vendas (uma com aspas e vírgula no nome, uma com quebra de linha no nome e 2 unidades, três do mesmo livro no mesmo dia, duas idênticas, uma comissão sem casas decimais) e a devolução do fone |
| `earnings-2026-09.xlsx` | o mesmo setembro em XLSX: título em A1, cabeçalho na linha 2, datas como número de série com formato de data, valores e quantidades como número, textos compartilhados |
| `earnings-2026-09-br.csv` | o mesmo setembro como o Excel brasileiro salva: `;`, vírgula decimal, `dd/mm/aaaa hh:mm:ss`, CRLF, Windows-1252 (com um travessão, que não existe no Latin-1 puro) e o cabeçalho em português que o app aceita até conhecer o real |
| `earnings-2026-09-tab.csv` | o mesmo setembro separado por tabulação, com BOM UTF-8 e CRLF |
| `earnings-2026-10.csv` | de 16/09 a 15/10, cobrindo parte de setembro: três linhas iguais às de setembro, uma venda nova, a devolução de 1 das 2 panelas, a devolução de uma cafeteira que nenhum relatório trouxe, uma data ilegível e uma linha sem itens |
| `orders-2026-09.csv` | o relatório de pedidos (Orders), que não é o de ganhos e é recusado inteiro |

O `.gitattributes` da raiz tira esta pasta da conversão de quebras de linha do Git, porque os testes leem os bytes
como a Amazon e o Excel os escrevem (o CI do Windows trocaria as quebras de linha).

Como o app lê o relatório:

- O arquivo pode ser CSV (crate `csv`) ou XLSX (crate `calamine`, primeira planilha). Um CSV em
  UTF-8, com ou sem BOM, é lido como UTF-8; qualquer outro, como Windows-1252 (o Latin-1 do Excel
  no Windows). O separador (`;`, `,` ou tabulação) é o que faz uma das 10 primeiras linhas ser o
  cabeçalho do relatório; o que vem antes, como a linha de título, é ignorado.
- As colunas são achadas pelo nome, sem diferença de maiúsculas, acentos, espaços e da moeda entre
  parênteses: `Ad Fees($)`, `Ad Fees (R$)` e `ad fees` são a mesma. Sem Name, ASIN, Seller, Tracking
  ID, Date Shipped, Price, Items Shipped, Returns e Ad Fees o arquivo não é o relatório de ganhos e
  nada é gravado. Category, Revenue e Device Type Group são guardadas quando vêm.
- Valores em texto aceitam `1.234,56` e `1234.56` (a mesma regra do que o owner digita); no XLSX,
  números viram decimal pelo menor texto que volta ao mesmo número, então `8.97` nunca vira
  `8.9700000000000006`. A moeda é sempre o real: o programa é o da amazon.com.br.
- Datas em texto aceitam `aaaa-mm-dd hh:mm:ss` e `dd/mm/aaaa hh:mm:ss`, com ou sem hora; no XLSX, a
  célula de data ou o número de série do Excel. A data é a do horário de Brasília (UTC−3).
- Uma linha com itens enviados é uma Commission aprovada, com o valor de Ad Fees, a data de envio
  como data da venda e o Tracking ID como marcação. O Program é um só, "Amazon Associados".
- A chave natural é a lista JSON `["shipped" ou "returned", Tracking ID, ASIN, Seller, Price, Date
  Shipped, posição]`, em que a posição conta as linhas iguais nesses campos, ordenadas pelas demais
  colunas: a ordem das linhas no arquivo nunca muda uma chave, e reimportar o mesmo arquivo, nos
  dois formatos, ou outro que cubra os mesmos dias, não duplica.
- Uma devolução (sem itens enviados, com Returns) não vira Commission própria: ela estorna a venda
  mais antiga do mesmo item (Tracking ID, ASIN, Seller e Price) enviada até a data dela que ainda
  tenha unidades, deste arquivo ou de um importado antes. Todas as unidades devolvidas: a Commission
  fica estornada, com o valor original. Parte delas: continua aprovada, menos o Ad Fees da
  devolução, e a plataforma mostra "Devolvido em parte (1 de 2)". A devolução fica anotada no JSON
  bruto da venda, então reimportar não estorna de novo. Sem venda para estornar, a linha é pulada
  com o motivo, e entra quando o relatório do mês do envio for importado e este de novo.
- A importação mostra antes o que vai adicionar, atualizar e manter, e as linhas puladas com a linha
  e o motivo; tudo é gravado numa transação só.

Inventados ou completados (conferir no regression pass):

- Os nomes das colunas no relatório da amazon.com.br (o app aceita os em inglês e um palpite em
  português: Categoria, Nome, Vendedor, ID de rastreamento, Data de envio, Preço, Itens enviados,
  Devoluções, Receita, Taxas de publicidade, Grupo de tipo de dispositivo), a moeda entre
  parênteses e o texto da linha de título.
- Se as devoluções vêm mesmo como linhas próprias com valores negativos e a data da devolução, ou
  como Returns na mesma linha da venda; se o Price da devolução é o da venda.
- O formato das datas, se têm hora e em que fuso; o formato dos números no CSV brasileiro e a
  codificação (UTF-8 ou Windows-1252).
- Se o XLSX tem título acima do cabeçalho, se as datas vêm como data ou como texto e se há linha de
  total no fim.
- Nomes, ASINs, valores, datas e tracking ids.
