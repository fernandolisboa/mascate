# Commerce movimenta o estoque pela interface do Inventory, na mesma transação

Receber uma Purchase Order grava o recebimento (Commerce) e os Stock Movements de entrada (Inventory). Os dois precisam entrar juntos ou não entrar: um recebimento sem entrada de estoque, ou o contrário, deixaria o saldo errado sem que o Fernando percebesse. Por isso o crate `mascate-commerce` passa a depender de `mascate-inventory`: o Commerce abre a transação, grava o que é dele e chama `Inventory::record_entries` nessa mesma transação. O Inventory continua sem saber o que é uma Purchase Order além do id que o movimento guarda como motivo, e nunca depende do Commerce. A baixa de estoque por venda (#42) e as devoluções (#58) seguem o mesmo caminho.

## Considered Options

- **Evento interno com fila no banco:** o Commerce gravaria um evento "Purchase Order recebida" na mesma transação e o Inventory o consumiria sem duplicar. Desacopla mais, mas exige fila, despachante e controle do que já foi processado para um único consumidor.
- **A interface orquestra:** a tela chamaria o Commerce e depois o Inventory. Sem transação comum, uma falha no meio deixa o recebimento sem estoque, e a regra do recebimento ficaria na camada de interface, que não tem regra de negócio.

## Consequences

- A lista de dependências permitidas do teste de arquitetura ganha `mascate-commerce -> mascate-inventory`; o caminho inverso continua proibido.
- O Inventory expõe a escrita só como `record_entries` sobre a conexão de quem chama; saldo e Average Cost continuam derivados do ledger (ADR 0005).
- Se um segundo módulo precisar reagir ao mesmo fato, a decisão volta para evento interno.
