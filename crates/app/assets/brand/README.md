# Mascate logo

Chosen by the owner on 2026-10-13 (issue #69): a price tag lying flat, with the hole and the M cut out, and the name in Fraunces lowercase.

| File | What it is | Used by |
| --- | --- | --- |
| `mark.svg` | The tag alone, cropped to its outline, one color | Title bar, tinted with the theme's accent |
| `wordmark.svg` | "mascate" as outlines, one color | Title bar, tinted with the theme's text |
| `app-icon.svg` | The tag on the app's dark ground (`#17181A`), rounded square | Tray, `../icon.png`, `../mascate.ico` |
| `lockup-light.svg`, `lockup-dark.svg` | Tag and name side by side, for light and dark backgrounds | Documents and pages about the app |

Colors: amber `#F2A33A` (the accent of the interface themes), ink `#17181A`, light `#ECEAE4`.

The wordmark is Fraunces (SIL Open Font License 1.1) at weight 600, optical size 144, SOFT 100, WONK 1, converted to paths: the app ships the outlines, not the font.

After changing `app-icon.svg`, regenerate the packaged icons; a test fails while they differ:

```sh
cargo test -p mascate -- --ignored write_app_icons
```
