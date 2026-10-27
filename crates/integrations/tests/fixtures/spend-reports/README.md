# Relatórios de gasto da Meta e do Google Ads (da documentação)

Estes arquivos **não** foram exportados de uma conta real: foram escritos em 27/10/2026 a partir da
ajuda pública em inglês das duas Platforms e de guias de importação que mostram as colunas exportadas
(Ads Manager: "Export table data", com a quebra por tempo "Day"; Google Ads: "Download a report",
com o segmento "Day"). O regression pass da fase 3 (#116) os troca por um relatório exportado da
conta do Fernando em português, em CSV e em XLSX, e confere os nomes das colunas em português, que
aqui são palpites.

| Arquivo | O que tem |
|---|---|
| `meta-daily.csv` | Gerenciador de Anúncios por campanha e por dia, em inglês: `Reporting starts` e `Reporting ends` iguais ao `Day` de cada linha, `Amount spent (BRL)`, uma campanha com `|` e outra com vírgula no nome, um dia sem gasto (célula vazia) |
| `meta-daily.xlsx` | o mesmo em XLSX, com a linha de resumo que vem antes das campanhas (sem campanha, período inteiro), datas como data e valores como número |
| `meta-daily-pt.csv` | como o Excel brasileiro salva: `;`, vírgula decimal, `dd/mm/aaaa`, Windows-1252, CRLF, cabeçalho em português e sem a coluna `Dia` (o dia é o início e o término, iguais) |
| `meta-ad-sets.csv` | por conjunto de anúncios: dois conjuntos da mesma campanha no mesmo dia somam, uma linha em dólar pela coluna `Currency`, um valor negativo, um ilegível e uma moeda que o app não conta |
| `meta-period.csv` | sem a quebra por dia: uma linha por campanha de 01/10 a 27/10, recusado inteiro |
| `google-daily.csv` | Google Ads com o segmento Dia: título e período acima do cabeçalho, `Currency code`, `Cost` com separador de milhar e as linhas `Total: …` no fim |
| `google-daily-excel.csv` | o mesmo como o "Excel .csv" do Google Ads: UTF-16 com BOM, separado por tabulação |
| `google-daily.xlsx` | o mesmo em XLSX |
| `google-period.csv` | sem o segmento Dia, recusado inteiro |
| `google-no-currency.csv` | sem a coluna da moeda, recusado inteiro |

O `.gitattributes` da raiz tira esta pasta da conversão de quebras de linha do Git, porque os testes
leem os bytes como as Platforms e o Excel os escrevem.

Como o app lê o relatório:

- CSV ou XLSX, pela mesma leitura do relatório da Amazon (`spreadsheet.rs`). O cabeçalho diz qual é:
  com `Reporting starts`, `Reporting ends`, nome da campanha e valor gasto é o da Meta; com
  `Campaign` e `Cost`, o do Google Ads.
- Uma linha é uma campanha num dia. O dia vem da coluna `Day`; na Meta, sem ela, do início e do
  término quando são o mesmo dia. Um relatório com período de mais de um dia numa linha, ou um do
  Google Ads sem a coluna `Day`, é recusado inteiro com o passo de exportar por dia.
- A moeda vem da coluna (`Currency`, `Currency code`) ou do nome da coluna do valor, como
  `Amount spent (BRL)`; sem nenhuma das duas o relatório é recusado. Só real e dólar entram.
- Linhas de total (`Total: …` no Google, sem campanha na Meta) ficam fora sem aviso; linhas da mesma
  campanha e do mesmo dia somam. Valor vazio é zero; negativo ou ilegível fica fora com a linha e o
  motivo.
