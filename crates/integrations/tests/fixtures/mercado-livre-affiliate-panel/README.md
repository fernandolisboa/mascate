# Fixtures do painel de afiliado do Mercado Livre

O Mercado Livre não tem API para afiliados, e o painel só abre com login, então estas fixtures não
foram gravadas de uma conta real. Cada arquivo é o valor que o script de leitura do app
(`TABLES_SCRIPT`, em `src/affiliate_panel.rs`) devolve numa página: o endereço em que a página
parou e as tabelas, com o texto de cada célula. O servidor de depuração falso dos testes responde
com esse valor no lugar de rodar o script.

O que foi conferido em 22/10/2026, sem login:

- `https://www.mercadolivre.com.br/afiliados` e os caminhos abaixo dele redirecionam para
  `https://www.mercadolivre.com/jms/mlb/lgz/login?...&go=<o endereço pedido>` (`login.json`).
- `https://afiliados.mercadolivre.com.br/` redireciona para `/gz/account-verification`.

O que foi inventado a partir de descrições públicas do painel (cliques, vendas, comissões
pendentes e confirmadas, aba "Relatórios"), até o regression pass da fase 3 (#116):

- o endereço do relatório, `/afiliados/relatorios`;
- os nomes das colunas (`Data da venda`, `ID da venda`, `Produto`, `Comissão`, `Status`,
  `Etiqueta`) e os status (`Pendente`, `Aprovada`, `Paga`, `Cancelada`, `Devolvida`);
- uma linha por item, com o mesmo ID nas linhas da mesma venda, e uma linha de total no fim;
- que o relatório mostra o período dele sem clique e numa página só.

Para regravar: ligue a flag, entre no painel pela janela do app, abra o relatório, abra as
ferramentas do desenvolvedor (F12), cole o conteúdo de `TABLES_SCRIPT` envolto em `copy(...)` no
console e cole o resultado em `report.json`, trocando nomes e valores de verdade por inventados.
