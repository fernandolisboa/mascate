# Arquivos de cada Product numa pasta por SKU, fora do banco

As fotos, notas de compra e prints de um Product ficam em `Documentos/Mascate/produtos/<SKU>/`, ao lado da pasta de Backups, e não no banco. A pasta é o registro: o app lista o que estiver nela, inclusive arquivos que o Fernando pôs lá pelo gerenciador de arquivos, e arrastar arquivos para a ficha do Product só copia para essa pasta. Renomear o SKU renomeia a pasta na mesma operação; se a pasta não puder mudar de nome (um arquivo aberto no Windows, por exemplo), o SKU também não muda.

O SKU vira nome de pasta, então só aceita letras, números e hífen, em maiúsculas, com até 32 caracteres, e nunca um nome que o Windows reserva (`CON`, `NUL`, `COM1`...). Maiúsculas fixas evitam dois SKUs que o Windows veria como a mesma pasta.

## Considered Options

- **Arquivos dentro do banco (BLOB):** entrariam no Backup, mas o banco cresceria com fotos e vídeos, cada Backup diário copiaria tudo de novo, e o Fernando não acharia os arquivos pelo Explorer.
- **Pasta com o id do Product em vez do SKU:** renomear seria trivial, mas a pasta perderia o sentido para quem navega nela; a issue #9 pede organização por SKU.
- **Guardar no banco o nome da pasta, separado do SKU:** sobrevive a uma pasta que não renomeia, mas deixa SKU e pasta divergirem sem que o Fernando perceba.

## Consequences

- O Backup (ADR 0007) cobre o banco, não os arquivos dos Products. Eles ficam em Documentos, que o OneDrive ou outra sincronização do Fernando pode levar para fora da máquina; um Backup que inclua essa pasta fica para quando houver necessidade.
- Uma pasta que já exista com o nome de um SKU novo e não pertença a nenhum Product bloqueia a renomeação, em vez de misturar arquivos.
