# Estoque como ledger com custo médio ponderado móvel

O saldo de estoque nunca é gravado como número editável: é derivado dos Stock Movements (entrada, saída, ajuste), que são imutáveis. O custo de cada Product é o Average Cost, recalculado como média ponderada móvel a cada entrada, e é ele que entra na Realized Margin. Escolhido em vez de FIFO por ser o método usual no Brasil e bem mais simples para um vendedor solo; o ledger permite recalcular tudo se o método mudar.
