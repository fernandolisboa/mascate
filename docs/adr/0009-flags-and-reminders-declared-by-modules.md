# Flags e Reminders declarados pelos módulos, guardados pelo platform

Cada módulo declara em código as suas flags (ADR 0006) e os seus Reminders, com os textos que o Fernando lê (motivo, risco, aviso), numa lista pública (`FLAGS`, `REMINDERS`), do mesmo jeito que declara as suas migrações. O app junta as listas na partida; uma chave repetida entre módulos é erro. O crate `platform` guarda o estado: um histórico de quem ligou ou desligou cada flag e quando, e as dispensas de cada Reminder. O estado atual de uma flag é a última mudança; um Reminder dispensado volta depois do intervalo que ele mesmo declara.

Ligar uma flag tem dois passos na interface pública: pedir (`request_turn_on`, que devolve a flag com o risco para mostrar) e confirmar (`confirm`, que gera a única coisa que `turn_on` aceita). Sem confirmação não há como ligar. Desligar não pede confirmação, porque desligado é o lado seguro. Código protegido por flag chama `ensure_on` antes de agir.

## Considered Options

- **Reminders no módulo Finance & Reporting**, como o PRD listava: Finance só pode depender de `kernel` e `platform`, então Commerce (LGPD dos compradores) e Marketing (CONAR nos posts) não teriam como registrar os seus sem abrir uma nova dependência entre módulos.
- **Um registro central com todas as flags num arquivo só:** o motivo e o risco ficariam longe do código que a flag protege.

## Consequences

- Finance continua dono dos seus Reminders (por exemplo, o de volume de vendas da fase 1); só o mecanismo e a tabela ficam no `platform`.
- Uma flag ou Reminder removido numa versão futura deixa linhas antigas no banco, que são ignoradas.
