# Fixtures do painel do Parceiro Magalu

A Magalu não tem API para afiliados, e o painel só abre com login, então estas fixtures não foram
gravadas de uma conta real. Cada arquivo é o valor que o script de leitura do app
(`TABLES_SCRIPT`, em `src/affiliate_panel.rs`) devolve numa página: o endereço em que a página
parou e as tabelas, com o texto de cada célula. O servidor de depuração falso dos testes responde
com esse valor no lugar de rodar o script.

O que foi conferido em 22/10/2026, sem login:

- `https://www.parceiromagalu.com.br/painel` e os caminhos abaixo dele redirecionam (301) para o
  mesmo caminho em `https://www.magazinevoce.com.br`.
- `https://www.magazinevoce.com.br/painel` (e a página inicial) redireciona para
  `/az-request-verify?url=<o endereço pedido>`, uma página "Captcha Magalu" com um desafio Altcha
  que pede um clique (`verification.json`). O app trata essa página como login: quem responde é
  o Fernando, pela janela de Entrar no painel.

O que foi inventado a partir de descrições públicas do painel (aba "Vendas"; vendas realizadas e
aprovadas, comissões pendentes e liberadas), até o regression pass da fase 3 (#116):

- o endereço das vendas, `/painel/vendas`, e o do login, `/login` (`login.json`);
- os nomes das colunas (`Data do pedido`, `Pedido`, `Produto`, `Comissão`, `Status`) e os status
  (`Aguardando aprovação`, `Aprovada`, `Liberada`, `Paga`, `Cancelada`, `Devolvida`);
- uma linha por item, com o mesmo número nas linhas do mesmo pedido, e uma linha de total no fim;
- que a página mostra o período dela sem clique e numa página só.

Para regravar: ligue a flag, entre no painel pela janela do app, abra Vendas, abra as ferramentas
do desenvolvedor (F12), cole o conteúdo de `TABLES_SCRIPT` envolto em `copy(...)` no console e
cole o resultado em `report.json`, trocando nomes e valores de verdade por inventados.
