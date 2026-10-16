# Respostas da Shopee Open Platform v2 (vendedor)

Escritas a partir da documentação oficial da Shopee Open Platform v2 (open.shopee.com, guia do desenvolvedor e referência das APIs `auth`, `product`, `order`, `returns` e `payment`), lida em 16/10/2026, sem conta real: o app de vendedor na Shopee pede CNPJ. A regression pass da fase 3 (#116) grava as respostas de verdade e troca estes arquivos.

- `token-get.json`: `POST /api/v2/auth/token/get`, a troca do código do login pelo primeiro token.
- `access-token-get.json` e `access-token-get-again.json`: `POST /api/v2/auth/access_token/get`, cada renovação com um refresh token novo (o anterior não vale mais).
- `refresh-refused.json`: a renovação com um refresh token já usado ou vencido.
- `code-refused.json`: a troca de um código vencido ou já usado.
- `item-list-page-1.json` e `item-list-page-2.json`: `GET /api/v2/product/get_item_list` com `item_status` NORMAL e UNLIST.
- `item-base-info.json`: `GET /api/v2/product/get_item_base_info` dos três itens: um sem variação, um com variações (modelos) e um deslistado.
- `model-list-camiseta.json`: `GET /api/v2/product/get_model_list` do item com variações.
- `model-list-empty.json`: o mesmo para um item sem variações.
- `categories.json`: `GET /api/v2/product/get_category` em pt-br (recorte).
- `update-ok.json`, `update-refused.json`: respostas de `update_price`, `update_stock` e `unlist_item`, com e sem `failure_list`.
- `order-list-window-1-page-1.json`, `order-list-window-1-page-2.json` e `order-list-window-2.json`: `GET /api/v2/order/get_order_list` por `update_time` nas duas janelas de 15 dias do primeiro Sync, a primeira em duas páginas por `cursor`; o pedido cancelado fica na divisa e vem nas duas.
- `order-list-statuses.json`, `order-list-empty.json`: um pedido em cada um dos outros status, e nenhum.
- `order-detail.json`: `GET /api/v2/order/get_order_detail` dos cinco pedidos do primeiro Sync: concluído, cancelado antes da coleta, pronto para enviar com desconto do vendedor, esperando a NF-e (`INVOICE_PENDING`, comprador mascarado) e com devolução (`TO_RETURN`).
- `order-detail-statuses.json`, `order-detail-escrow.json`, `order-detail-return.json`: o mesmo para os status, para as tarifas e repasses, e para o pedido com devolução sozinho.
- `return-list.json`, `return-list-accepted.json`, `return-list-refund-only.json`, `return-list-empty.json`: `GET /api/v2/returns/get_return_list` por `update_time`: a devolução de uma caneca a caminho, a mesma aceita com o reembolso, um reembolso sem devolver o item (`needs_logistics` falso) e nenhuma.
- `escrow-completed.json`, `escrow-ready.json`: `GET /api/v2/payment/get_escrow_detail` de um pedido concluído (comissão, taxa de serviço e parte do frete com o vendedor) e de um ainda aberto, cujo escrow a Shopee muda até concluir.
- `error-auth.json`, `error-rate-limit.json`, `error-limit.json`: o envelope de erro (`error_auth` token inválido, `error_rate_limit` por segundo, `error_limit` cota diária).

Os ids seguem o exemplo da documentação (partner 2001887, loja 14701711); os itens são inventados.
