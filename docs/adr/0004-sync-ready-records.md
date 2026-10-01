# Registros prontos para merge e sincronização

Todo registro nasce com ID UUIDv7, `created_at`, `updated_at` e exclusão lógica (`deleted_at`), em vez de ID sequencial e exclusão física. Isso custa pouco agora e é o que torna possível o Merge Import (combinar dois bancos linha a linha, vence a alteração mais recente, com relatório de conflitos) e as opções de sincronização entre máquinas que o Fernando quer configuráveis, nesta ordem: réplica gerenciada da Turso, depois local-first com CRDT e relay, depois backend próprio (para consulta pelo celular).

## Consequences

- Dinheiro é sempre decimal com moeda explícita, nunca float, para que merge e soma deem o mesmo resultado em qualquer máquina.
- Stock Movements são imutáveis e só se acrescentam, o que os torna triviais de mesclar.
- Fase 1 entrega só Backup, export e restore; Merge Import e sincronização são épicos da fase 3.
