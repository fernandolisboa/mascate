# Respostas da Open API da Magalu

Respostas escritas a partir da documentação oficial em
https://developers.magalu.com (Produtos › Portfólio, Pedidos, Análise
Financeira e ID Magalu), lida em 16/10/2026, sem conta real: a loja da Magalu pede CNPJ. Os formatos seguem
os esquemas e os exemplos da documentação; os valores são inventados.

- `token.json`, `token-renewed.json`: `POST https://id.magalu.com/oauth/token`, código e renovação.
- `token-refused.json`: código ou refresh token recusado (`invalid_grant`).
- `me.json`: `GET /seller/v1/portfolios/me`, com o canal da loja.
- `skus-page.json`: `GET /seller/v1/portfolios/skus`, uma página.
- `sku-*.json`, `prices-*.json`, `stocks-*.json`: cada SKU, o preço e o estoque.
- `accepted.json`: o 202 de `PATCH .../prices/{sku}` e `PATCH .../stocks/{sku}`.
- `error-*.json`: `{slug, message}` de 401, 403 e 429; `detail` do 422.
- `orders-page.json`: `GET /seller/v1/orders`, com um pedido pago esperando a
  NF-e, um entregue, um cancelado antes do envio e um novo; `orders-empty.json`,
  a página vazia; `orders-partly-cancelled.json`, um pedido finalizado com uma
  entrega cancelada.
- `financial-analysis-delivered.json`: `GET /seller/v1/financial-analysis/orders`
  do pedido entregue, com venda, comissão, tarifas fixas, MDR, frete, a parte
  do frete paga pela Magalu e ICMS informativo; `financial-analysis-empty.json`,
  sem análise; `error-422-window.json`, o 422 `orders_window_required`.

A regression pass da fase 3 (#116) regrava estes arquivos com a loja real e
confere o que a documentação deixa em aberto: se `_offset` conta registros
(como diz o guia de paginação) ou páginas, se o refresh token muda a cada
renovação, quanto tempo a Magalu leva para aplicar um preço depois do 202, e
qual armazém recebe o estoque enviado sem `branch`. Dos pedidos: se o
`updated_at` do pedido muda quando uma entrega muda de status; se
`unit_price` já vem com o desconto (o app conta as linhas `DISCOUNT` e
`PROMOTION` da análise financeira como tarifa); se a comissão do item é da
linha inteira (o app divide pela quantidade); se `handling_time.limit_date`
vem como data ou data e hora; e se a análise financeira repete transações
iguais (o app numera a segunda).
