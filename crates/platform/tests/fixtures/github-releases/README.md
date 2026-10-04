# Releases do GitHub (fixtures)

Respostas montadas a partir da documentação oficial da API de releases do GitHub
(<https://docs.github.com/en/rest/releases/releases#get-the-latest-release>, versão
`2022-11-28` da API), sem gravar de uma release real: o Mascate ainda não publicou
nenhuma. O regression pass (#37) troca estes arquivos por respostas gravadas da
primeira release publicada.

- `release.json`: resposta de `GET /repos/{owner}/{repo}/releases/latest` e de
  `GET /repos/{owner}/{repo}/releases/tags/{tag}`, com os arquivos que o workflow
  de release publica.
- `mascate-update.json`: o manifesto que o workflow de release publica em cada
  release (formato do próprio Mascate, ADR 0010).

Os testes trocam os marcadores `{server}`, `{version}`, `{windows_size}`,
`{windows_sha256}`, `{appimage_size}` e `{appimage_sha256}` antes de servir.
