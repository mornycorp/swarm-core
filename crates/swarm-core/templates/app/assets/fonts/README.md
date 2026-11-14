# Fonts

The UI's faces, embedded in the window by `crates/ui/src/fonts.rs` (`include_bytes!`), so every
build draws the same with no network (the App UI Substrate › Type). Each comes unmodified from its
upstream release and keeps its licence beside it. Swarm Core's app skeleton wrote them
(`cargo xtask swarm-core init --app --ui`); they are this app's now.

| File | Face | Version | Licence | Upstream |
|---|---|---|---|---|
| `Roboto-Regular.ttf` | Roboto Regular (400): body text, buttons | 2.138 | Apache-2.0 (`Roboto-LICENSE.txt`) | `roboto-unhinted.zip` |
| `Roboto-Light.ttf` | Roboto Light (300): small and subtle text | 2.138 | Apache-2.0 | `roboto-unhinted.zip` |
| `Roboto-Medium.ttf` | Roboto Medium (500): headings | 2.138 | Apache-2.0 | `roboto-unhinted.zip` |
| `Roboto-Bold.ttf` | Roboto Bold (700): emphasis | 2.138 | Apache-2.0 | `roboto-unhinted.zip` |
| `CascadiaMono-Regular.ttf` | Cascadia Mono Regular: monospace | 2407.24 | SIL OFL 1.1 (`CascadiaMono-OFL.txt`) | `CascadiaCode-2407.24.zip`, `ttf/static/` |
| `fa-solid-900.ttf` | Font Awesome 6 Free Solid (900): icons | 6.7.2 | font SIL OFL 1.1, icons CC BY 4.0 (`FontAwesome-LICENSE.txt`) | `fontawesome-free-6.7.2.tgz`, `package/webfonts/` |

## Where they come from

- **Roboto 2.138**: <https://github.com/googlefonts/roboto-2/releases/download/v2.138/roboto-unhinted.zip>.
  The last Apache-2.0 release; Roboto 3 moved to the OFL. Unhinted, because egui rasterizes
  without hinting and the files are smaller. `Roboto-LICENSE.txt` is the archive's `LICENSE`.
- **Cascadia Mono 2407.24**: <https://github.com/microsoft/cascadia-code/releases/download/v2407.24/CascadiaCode-2407.24.zip>,
  the static Regular face (no ligatures, no Powerline or Nerd Font glyphs). The archive carries no
  licence file, so `CascadiaMono-OFL.txt` is the repository's `LICENSE` at the same tag.
- **Font Awesome Free 6.7.2**: the npm package `@fortawesome/fontawesome-free@6.7.2`,
  <https://registry.npmjs.org/@fortawesome/fontawesome-free/-/fontawesome-free-6.7.2.tgz>.
  `fa-solid-900.ttf` is `package/webfonts/fa-solid-900.ttf`, and `FontAwesome-LICENSE.txt` is
  `package/LICENSE.txt`: the font is SIL OFL 1.1 (Reserved Font Name "Font Awesome"), and the icons
  are CC BY 4.0, which asks for credit in the app (an About). Only the solid face is embedded.

SHA-256 of the files as checked in:

| File | SHA-256 |
|---|---|
| `Roboto-Regular.ttf` | `f3edb8058e523f5612bfd99d0745e661568ad85e1b6217bc62f786fabae624c6` |
| `Roboto-Light.ttf` | `a08729d794801eaa158c53b1558f4cc351c3b1791eb3d22faf7058b2b7df9c8c` |
| `Roboto-Medium.ttf` | `b398bb9c791ddb08f1063d9c874f98e8aadb99132043b2d272a9276cf90c465a` |
| `Roboto-Bold.ttf` | `2ca2b3bfc2c2d43fa8f2b7227982d735fd537ecf113a4c56cc2d292bcc3106c8` |
| `CascadiaMono-Regular.ttf` | `06520d032ec274fa5040b22c6f4a1d829081b24ba40b2da56dae89bf10c7b481` |
| `fa-solid-900.ttf` | `d2f0593540b0e33ba6de255a54f272d466e31144806956bea8cfdbf7edffc9bd` |

## Updating

Take new files from the upstream release, update the tables here, and keep the licence files
current. A new Font Awesome must keep every codepoint the app names.
