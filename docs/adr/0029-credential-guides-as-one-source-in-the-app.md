# Guias de credenciais com fonte única no app

A #65 põe dentro do app o passo a passo de cada Connection que pede chave colada: em Configurações › Conexões, cada cartão abre "Como conseguir as chaves", uma tela só daquela Connection com passos numerados, o link para a página exata da plataforma em cada passo (aberto no navegador do sistema) e, no último passo, os mesmos campos e o mesmo Salvar do cartão. O mesmo passo a passo existe para quem lê fora do app em `docs/credentials.html`. Decidimos três coisas.

1. **Os passos são dados em Rust, no app.** `credential_guides::guide(connection)` devolve os passos (texto e, quando há, o link com rótulo e endereço) e o que falta depois de salvar. O `match` cobre toda `Connection`: uma Connection nova não compila sem guia. Os passos ficam no app e não no `integrations` porque são texto de tela para o Fernando, como o nome e o propósito de cada Connection, que já moram em `connections.rs`; o `integrations` continua só com as credenciais e o estado.
2. **A seção da fase 1 de `docs/credentials.html` é gerada desses dados.** Os cartões ficam entre os marcadores `phase-1-guides:start` e `phase-1-guides:end`; o resto da página (regras, variáveis de desenvolvimento, fases 2 e 3, ainda sem Connection) segue escrito à mão. Um teste compara a página commitada com a gerada e falha com a instrução de rodar `cargo test -p mascate -- --ignored write_credentials_page`, o mesmo padrão do banco de exemplo das versões (ADR 0010). A comparação ignora CRLF, porque o Windows faz checkout com ele.
3. **Os links são das páginas oficiais.** Um teste exige `https` no domínio de cada plataforma (`developers.mercadolivre.com.br`, `affiliate.shopee.com.br`, `platform.claude.com`). Os passos seguem a documentação atual de cada uma; rótulos de painel que só a conta real confirma vão para o regression pass (#37).

## Considered Options

- **Arquivo JSON versionado, embutido no app:** dá para editar sem Rust, mas um erro de digitação só aparece em teste, a página HTML ainda precisaria ser gerada, e a leitura na carga é um caminho de falha a mais.
- **Duas cópias, app e HTML à mão, com um teste que só confere links e nomes de variável:** menos código, mas o texto dos passos diverge em silêncio quando uma plataforma muda o painel.

A pergunta foi ao Fernando num card; ele escolheu os dados no código.

## Consequences

- Mudar um passo é editar `crates/app/src/credential_guides.rs` e regravar a página com o teste ignorado; o CI recusa a página velha.
- Quando a fase 2 trouxer uma Connection nova (Hotmart, Kiwify, ...), ela ganha guia no mesmo `match` e o seu cartão sai da parte escrita à mão da página.
- A URL de redirecionamento do Mercado Livre ainda não é a definitiva: o passo manda cadastrar um endereço `https` provisório até o login de vendedor (#10) definir o endereço que o botão Conectar vai usar.
