# Unidades voltam uma vez: o cancelamento antes do envio sozinho, a devolução com a palavra do Fernando

A #21 traz a etiqueta de envio, o destaque dos Orders atrasados e o caminho de volta das unidades que a #20 tirou do estoque. Decidimos cinco coisas.

1. **A volta reverte a saída.** O Inventory ganha `record_return` e dois motivos, `SaleCancelled` e `SaleReturned`, ambos com o id do Order. As unidades voltam ao mesmo Stock Location e ao custo com que saíram (a parte proporcional do movimento de saída), não ao Average Cost do dia: o ledger fica como se a venda não tivesse acontecido, e uma compra mais cara que chegou no meio-tempo continua mexendo o Average Cost só pelo que ela trouxe. O Inventory recusa devolver mais unidades do que a saída levou; quantas já voltaram é controle de quem chama.
2. **Cancelado antes do envio volta sozinho.** Um Order cancelado cujo envio não saiu das mãos do Fernando (sem envio, pendente, pronto para despachar ou cancelado) devolve as unidades no próprio Order Sync, na mesma transação que grava o Order (ADR 0012). Cada item guarda quantas unidades voltaram (`restocked`) e quantas chegaram sem condição de venda (`unsellable`); um Order lido de novo nunca devolve duas vezes. Depois do Sync o app já chama `StockMirror::send` (ADR 0017), e o anúncio recebe as unidades de volta.
3. **O que já saiu espera a confirmação.** Unidades que já foram enviadas só voltam ao estoque quando o Fernando diz que chegaram: todas, quando o envio não foi entregue ou o Order foi cancelado depois de enviado; as de cada devolução não cancelada, pela quantidade que o canal informa por item. Ao confirmar ele escolhe "voltou ao estoque" (as unidades entram por `SaleReturned`) ou "chegou sem condição de venda" (a devolução fecha e o estoque não muda; o custo continua na venda, como a #22 vai ler). Só itens que deram baixa têm o que devolver: um Order anterior ao primeiro Sync nunca saiu do ledger.
4. **Devoluções vêm das reclamações do Order.** O adapter lê as reclamações que o Order lista em `mediations` e, de cada uma, `/post-purchase/v2/claims/{id}/returns` (404: reclamação sem devolução), guardando o id da devolução, o status (a caminho, entregue, cancelada) e as unidades de cada item. Elas ficam em `commerce_order_returns` (migração 7 do Commerce), por Order, devolução e item, e entram na comparação que decide se o Order mudou.
5. **Etiqueta só em PDF, por uma porta irmã; prazo pelo `dispatch_by`.** O Commerce define `ShippingLabels` (só `label_pdf`), separada de `ChannelOrders` pelo mesmo motivo do ADR 0018; o adapter pede `/shipment_labels?response_type=pdf`. O Commerce só entrega a etiqueta de um Order pronto para despachar e recusa uma resposta que não seja PDF; o app a salva em `Documentos/Mascate/Etiquetas` e a abre no visualizador do sistema, que imprime. Um Order pronto para despachar é atrasado depois do `dispatch_by` e fica com o prazo perto dentro do aviso escolhido em Configurações › Pedidos (24 horas por padrão, de 1 a 72); a tela Hoje lista os dois e as devoluções a confirmar.

## Considered Options

- **Voltar ao Average Cost atual:** é o que a contagem faz com unidades achadas, mas numa devolução o custo daquelas unidades é conhecido; voltar pelo custo do dia criaria ou apagaria valor no estoque sem nada ter sido comprado.
- **Devolver ao estoque ao ver a devolução entregue no canal:** dispensaria um clique, mas o canal diz que o pacote chegou, não em que estado; uma peça avariada voltaria ao anúncio pelo Stock Mirror antes de alguém abrir a caixa.
- **Só "volta ao estoque", e a avaria por ajuste depois:** reaproveita o ajuste de Avaria, mas no meio-tempo o anúncio reabre a unidade avariada; a segunda saída na confirmação fecha a devolução sem esse risco.
- **ZPL para impressora térmica:** a issue pede PDF; o ZPL fica para quando houver impressora térmica, no mesmo endpoint.

## Consequences

- A lista de dependências não muda: o Commerce já dependia do Inventory (ADR 0012) e o adapter já implementava portas do Commerce (ADR 0013).
- Reembolsos e estornos de tarifa ficam para a #22, que lê o status do Order e as devoluções guardadas.
- Se o `last_updated` do Order não mudar quando uma reclamação ou devolução muda, a busca por data não traria a devolução: o regression pass (#37) confere, e a saída seria uma busca de reclamações por vendedor no mesmo Sync.
