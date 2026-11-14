<!-- A pinned copy from swarm-core (crates/swarm-core/assets/docs/app-ui-substrate.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`: the gate's
     `sync-assets --check` fails on a copy that differs. -->
# The App UI Substrate

> **Last reviewed:** 2026-11-14 · **Last updated:** 2026-11-14 · **Change:** minor (the page is generated from this doc by `cargo xtask docs render`, never by hand; new: the theme as data, the 20 tokens and 4 ranges, egui's visuals, the contrast rules, type, density, icons, the dock, the window and the menus, each in the skeleton) · **Target:** M1

How {{product}} looks. This is the UI part of the architectural substrate: every app built on
Swarm Core draws with the same theme, type, density and chrome, so the swarm's tools look and work
alike. [The app architecture contract](app-architecture.md) says *why* a colour is a token and a
word is a catalog entry ([Colours from the theme, words from the
catalog](app-architecture.md#colours-from-the-theme-words-from-the-catalog)); this document says
*what* they are.

The colours here are the skeleton's own, from `templates/app/crates/engine/src/theme.rs`, and a
test holds this document to them (`the_ui_substrate_has_the_skeletons_colours`). **[The page](app-ui-substrate.html)** is this document as a
page, with the colours drawn (each range on both backgrounds, with its borders and text
backgrounds): open it in a browser. It is generated from this document and never edited by hand
([section 13](#13-changing-the-ui-substrate)).

Each section says where it stands:

- **In the skeleton:** the app template does it today.
- **To adopt:** specified here; the template doesn't yet. (Every
  section is in the skeleton now.)

## 1. Principles

- **One home for each value.** Colours live in the engine's theme module; metrics, faces and
  sizes, and icon codepoints each have one module of their own. A call site never writes a colour,
  a size, a codepoint or a word.
- **The theme is data.** The palette sits in the engine, below the toolkit, so the `ui.theme`
  command gives every surface (the window, the CLI, MCP, a test) the same tokens and ranges. The UI
  crate's theme module is the one place a toolkit colour is made.
- **A colour is named for what it means.** Drawing code asks for `error` or `selection`, never for
  a hue. An identifier's colour comes from a range, by a fixed hash, so it is the same on every run,
  surface and machine.
- **Readable by rule, checked by test.** Contrast is measured (WCAG 2) in both looks; a value that
  falls short is either nudged and noted, or kept and recorded with its ratio and the reason.
- **No network.** Fonts and the app icon are embedded, so the desktop and browser builds draw the
  same offline.
- **Words from the catalog.** Every word a person reads is an entry in the string catalog. `scan`
  holds both rules: `ui-colours` finds a colour made outside the theme module, `ui-strings` a string
  literal outside the catalog.

## 2. Looks, and how the theme is applied

**In the skeleton.** The theme has two looks, **dark** and **light** (`Look`); each token and each
range entry has a value in both.

1. The engine's `theme.rs` holds the palette as constants: `TOKENS` and `RANGES`.
2. `ui.theme` returns it as data (`ThemeData`): each token's name, meaning and `#RRGGBB` in each
   look, each range's entries with their text background and border in each look, and the corner
   radius.
3. The UI crate's `theme::apply(ctx)` runs once, when the window starts: it installs the faces,
   and gives **both** of egui's appearances their visuals, the text styles and the density
   (`ctx.style_mut_of`), so egui chooses between them every frame and following the system's look
   works live.
4. Drawing code asks `Theme::of(ui.visuals())` while it draws and never caches it: the look can
   change between frames.

An app may offer people a choice. One of the swarm's tools has Options › Theme: **System**, **Dark** (its
default) and **Light**, saved with the UI state.

## 3. Tokens

**In the skeleton.** Twenty tokens, in the order `ui.theme` gives them.

| Token | Meaning | Dark | Light |
|---|---|---|---|
| **Surfaces** | | | |
| `background` | What everything is drawn on: a panel's fill | `#353535` | `#EFEFEF` |
| `chrome` | The menu bar, the status bar and other chrome | `#353535` | `#EFEFEF` |
| `panel` | Floating windows, dialogs and popups | `#353535` | `#EFEFEF` |
| `base` | Views: lists, tables and text fields | `#2A2A2A` | `#FFFFFF` |
| `alt_base` | Every other row of a list drawn on `base` | `#303030` | `#F7F7F7` |
| `header` | Table header rows and a ruler | `#3F3F3F` | `#CACACA` |
| `frame` | Lines that frame views and panels (decorative: no minimum) | `#424242` | `#ABABAB` |
| `border` | Decorative lines: window frames and separators | `#424242` | `#ABABAB` |
| **Text** | | | |
| `text` | Body text | `#FCFCFC` | `#000000` |
| `strong` | Emphasised text, and text on a range colour made a text background | `#FFFFFF` | `#000000` |
| `muted` | Text that matters less: a command the UI can't run here, a hint | `#A6A6A6` | `#666666` |
| `selected_text` | Text on the selection | `#FCFCFC` | `#000000` |
| `link` | A link | `#4DA6FF` | `#1F6FA8` |
| **Controls and highlights** | | | |
| `control` | The fill of a widget at rest: a button, a field | `#454545` | `#F7F7F7` |
| `control_border` | The edge of a widget at rest: 3:1 on the background | `#858585` | `#7A7A7A` |
| `accent` | An accent that carries no text: what has focus, an outline | `#007ACC` | `#2B78AE` |
| `selection` | What is selected: its fill, with `selected_text` on it | `#0070BB` | `#C1DCEE` |
| **States** | | | |
| `success` | Something that worked | `#90EE90` | `#006400` |
| `warning` | Something to look at that hasn't failed | `#FF7F1C` | `#965C00` |
| `error` | A failure, or why a command can't run now | `#FF8A80` | `#B42318` |

A colour of the app's own (a brand, a status) is a token too, added to `TOKENS` with a value in
each look.

## 4. egui's visuals

**In the skeleton.** `Theme::visuals` starts from `Visuals::dark()` or `Visuals::light()` and
changes these:

| egui field | Set to | Why |
|---|---|---|
| `panel_fill` | `background` | the window's ground |
| `window_fill` | `panel` | windows, dialogs, popups |
| `extreme_bg_color`, `code_bg_color` | `base` | views: lists, tables, text fields |
| `faint_bg_color` | `alt_base` | striped rows |
| `window_stroke` | 1 px `border` | flat frames |
| `hyperlink_color` | `link` | |
| `selection.bg_fill` / `selection.stroke` | `selection` / 1 px `selected_text` | the theme's selection, not egui's |
| `warn_fg_color`, `error_fg_color` | `warning`, `error` | |
| `weak_text_color` | `muted` | egui's weak text is the muted token, not text at 60 % |
| `fg_stroke.color` of the non-interactive, inactive, hovered and open states | `text` | one text colour across states |
| `widgets.active.fg_stroke.color` | `strong` | egui's strong text is its active widgets' text |
| `widgets.noninteractive.bg_stroke` | 1 px `border` | separators and frames |
| `widgets.inactive.bg_fill`, `.weak_bg_fill` | `control` | egui draws a resting widget with no edge and a fill barely off the panel: these make a resting button or field visible |
| `widgets.inactive.bg_stroke`, `widgets.open.bg_stroke` | 1 px `control_border` | its edge, at least 3:1 |
| `override_text_color` | `None` | widgets keep their own state colours |

### Corner radius

**In the skeleton.** **2 px** in both looks, for `window_corner_radius`, `menu_corner_radius` and
every widget state's `corner_radius`: windows, menus and controls are near-square. It is the
engine theme's `CORNER_RADIUS`, which `ui.theme` gives as `corner_radius` and `Theme::visuals` sets.
The dock ([section 10](#10-the-dock)) is square.

## 5. Ranges: colours for identifiers

**In the skeleton.** A range is a set of colours for telling things apart. There are two ways to
choose from one:

- `at(n)` takes a number in order: entry `n % size`.
- `pick(id)` hashes an identifier with 64-bit FNV-1a over the UTF-8 it displays as, so `pick(7)`
  is `pick("7")`. The hash is fixed and documented, unlike std's `DefaultHasher`, so an id's colour
  never changes between Rust releases.

Every entry also has a **text background**, `behind_text`: its HSL lightness held at most 0.5 in
dark or at least 0.5 in light, then moved further until `strong` text on it reaches 4.5:1, its hue
and saturation kept. It has a **border** too, `darker`: each channel × 0.8, rounded down, which
outlines a filled bar or mark. `ui.theme` gives both with each entry, and the skeleton's swatch view
draws them.

| Range | Size | For | Source |
|---|---|---|---|
| `series` | 7, and a grey for none | Numbered things in order (an issue, a lane, a frame): `at(n)` is entry `n % 7`; `none()` is the grey for something with no number | ColorBrewer Dark2 in dark, Set2 in light (Cynthia A. Brewer, colorbrewer2.org, Apache-2.0), as published |
| `markers` | 9 | Up to 9 marks told apart at a glance, the same in both looks: a marker by number (`at`), or a few ids (`pick`) | ColorBrewer Set1 (the same source) with the orange `#FF7F1C` for its `#FF7F00`, then grey |
| `range16` | 16 | Up to 16 identifiers by a stable pick of their id: an object's type, a thread, a source | The first 16 of `range32` |
| `range32` | 32 | Up to 32 identifiers side by side (the rows of a timeline, the bars of a scrubber), told apart beside each other, not only by a legend | Glasbey et al. 2007's sets in colorcet 3.2.1 (Peter Kovesi, CC BY 4.0): `glasbey_light` in dark, and `glasbey_dark`'s nearest hue to each in light; 11 entries nudged to 3:1 |

<!-- page: ranges -->

### `series`

| Entry | Dark | Light |
|---|---|---|
| series 0 | `#1B9E77` | `#66C2A5` |
| series 1 | `#D95F02` | `#FC8D62` |
| series 2 | `#7570B3` | `#8DA0CB` |
| series 3 | `#E7298A` | `#E78AC3` |
| series 4 | `#66A61E` | `#A6D854` |
| series 5 | `#E6AB02` | `#FFD92F` |
| series 6 | `#A6761D` | `#E5C494` |
| series none | `#666666` | `#B3B3B3` |

### `markers`

The same in both looks.

| Entry | Dark | Light |
|---|---|---|
| markers 0 | `#E41A1C` | `#E41A1C` |
| markers 1 | `#377EB8` | `#377EB8` |
| markers 2 | `#4DAF4A` | `#4DAF4A` |
| markers 3 | `#984EA3` | `#984EA3` |
| markers 4 | `#FF7F1C` | `#FF7F1C` |
| markers 5 | `#FFFF33` | `#FFFF33` |
| markers 6 | `#A65628` | `#A65628` |
| markers 7 | `#F781BF` | `#F781BF` |
| markers 8 | `#999999` | `#999999` |

### `range32` (and `range16`, its first 16)

| Entry | Dark | Light |
|---|---|---|
| range32 0 | `#F12F17` | `#D70000` |
| range32 1 | `#1B9211` | `#028800` |
| range32 2 | `#C727FF` | `#BB00FE` |
| range32 3 | `#06ACC6` | `#0088B1` |
| range32 4 | `#98FF00` | `#6C9700` |
| range32 5 | `#FFA530` | `#CC7407` |
| range32 6 | `#FF8FC8` | `#C5668F` |
| range32 7 | `#00FECF` | `#009D6F` |
| range32 8 | `#B0A5FF` | `#5A4AA3` |
| range32 9 | `#94AD84` | `#546745` |
| range32 10 | `#A77412` | `#755A00` |
| range32 11 | `#E6299C` | `#C63189` |
| range32 12 | `#FEF590` | `#8F7B01` |
| range32 13 | `#C86F66` | `#9D5743` |
| range32 14 | `#9EE3FF` | `#086B84` |
| range32 15 | `#00C946` | `#006D2E` |
| range32 16 | `#A977AD` | `#A57AB7` |
| range32 17 | `#B8BB02` | `#596D00` |
| range32 18 | `#F4C0B1` | `#A2766A` |
| range32 19 | `#FF28FD` | `#FD00CD` |
| range32 20 | `#F3CEFF` | `#836F8C` |
| range32 21 | `#009F7D` | `#00816A` |
| range32 22 | `#FF6200` | `#EB4E00` |
| range32 23 | `#FF3465` | `#CB0044` |
| range32 24 | `#A0E492` | `#004B00` |
| range32 25 | `#8D9BB2` | `#536177` |
| range32 26 | `#829126` | `#4C5001` |
| range32 27 | `#78C7BB` | `#499692` |
| range32 28 | `#BC9258` | `#9F6E32` |
| range32 29 | `#E58FFF` | `#7F2B8E` |
| range32 30 | `#72B9FF` | `#688CBD` |
| range32 31 | `#C6A5C1` | `#A67E93` |

## 6. Contrast

**In the skeleton.** Measured by WCAG 2 relative luminance, in each look, by the theme's tests.

- **Text: at least 4.5:1** (`TEXT_CONTRAST`, WCAG 2 1.4.3). Every token read as text meets it on
  what it is drawn on, and `strong` text on any entry's text background meets it by construction.
- **Fills and edges: at least 3:1** (`FILL_CONTRAST`, WCAG 2 1.4.11): `control_border` against
  `background` and `panel`, and each range entry against `background`.
- **A shortfall is nudged or recorded, never silent.** `range16` and `range32` are nudged: an entry
  under 3:1 moves lighter in dark (darker in light) in CIELAB lightness, keeping its hue, and its
  comment gives the original and its ratio. The rest are kept, and the tests' `KEPT_SHORT` lists
  each with its measured ratio and why (21 entries):

| Kept | Look | On `background` | Why it is kept |
|---|---|---|---|
| `accent` | dark | 2.72:1 | The swarm's tools share one accent blue. It never carries meaning alone: the selection (`selected_text` on it 5.07:1) and `control_border` (3.32:1) do. |
| `series` 2, 3 and none | dark | 2.14–2.97:1 | The ColorBrewer fills as published, so a series is the same colour in every app. A fill is told apart by its border and its neighbours, and text on it goes on its text background, which reaches 4.5:1. |
| `series` 0–6 and none | light | 1.20–2.28:1 | As above. Set2 is a pastel set made to carry black text. |
| `markers` 0, 1, 3, 6 | dark | 2.31–2.83:1 | As `series`: the published fills, kept. |
| `markers` 2, 4, 5, 7, 8 | light | 1.07–2.48:1 | As `series`. |

The tests also hold how far apart a range's entries stay: at least 10 (CIEDE2000) between any two,
and 15 between neighbours.

## 7. Type

**In the skeleton.** Embedded faces, in one module (`crates/ui/src/fonts.rs`) with their sizes,
installed by `theme::apply`. `init --app --ui` writes the files to `assets/fonts/`, each with its
licence, and their upstream releases and hashes in that folder's `README.md`.

| Family | Face | For |
|---|---|---|
| `Proportional` | Roboto Regular | body text, buttons |
| `"light"` | Roboto Light | small and subtle text |
| `"medium"` | Roboto Medium | headings, section titles, a start page's title |
| `"bold"` | Roboto Bold | emphasis (egui has no synthetic bold) |
| `Monospace` | Cascadia Mono Regular | code, ids, hex |
| `"icons"` | Font Awesome Free 6.7.2 Solid, **on its own** | icons only: text is never drawn from it, nor an icon from another font |

egui's own fonts stay behind Roboto and Cascadia Mono as fallbacks, for the symbols and emoji they
lack.

### Text styles

A scale on a 12 pt body:

| Style | Size (pt) | Family | Scale |
|---|---|---|---|
| `Small` | 11 | light | × 0.9 |
| `Body`, `Button` | 12 | Proportional | × 1 |
| `Monospace` | 12 | Monospace | × 1 |
| `"h2"`: section titles | 18 | medium | × 1.5 |
| `Heading` | 21 | medium | × 1.75 |
| `"callout"`: a start page's title | 36 | medium | × 3 |

**Wait for the fonts to bind.** egui binds new font definitions at the start of the *next* pass,
and panics on a named family it doesn't know: nothing draws in Light, Medium, Bold or an icon until
`fonts::bound(ctx)` is true.

## 8. Density

**In the skeleton.** Logical points, scaled by the system's display scale, in
`crates/ui/src/density.rs`. `theme::apply` installs `density::spacing()` as egui's `Spacing` in
both looks; the menu bar and its menus take `menu_bar_style` and `menu_style`, and a two-column
form `form_spacing()`, rather than writing numbers.

| Metric | Value | egui's | Note |
|---|---|---|---|
| Row height: list, table, menu and tab rows; a button's, combo box's and single-line field's height (`ROW_HEIGHT`) | 18 | 18 | the row for 12 pt Roboto |
| Item spacing (`ITEM_SPACING`) | 6 × 3 | 8 × 3 | tighter across |
| Form column spacing (`COLUMN_SPACING`) | 12 | – | between a label column and its values |
| Button padding (`BUTTON_PADDING`) | 6 × 2 | 4 × 1 | 2 down gives an 18 pt button |
| Menu entry padding (`MENU_PADDING`), item spacing y = 0 | 8 × 2 | – | entries touch, so hover fills meet |
| Menu-bar title padding (`MENU_BAR_PADDING`) | 7 × 2 | – | titles' text 20 pt apart |
| Menu frame margin (`MENU_MARGIN`) | 1 × 3 | – | |

## 9. Icons

**In the skeleton.** In `crates/ui/src/icons.rs`.

- **Glyphs:** Font Awesome Free 6.7.2 Solid (the font SIL OFL 1.1, the icons CC BY 4.0, whose
  attribution the embedded font's own metadata carries; an app with an About credits it there too),
  at the body size. Codepoints are named constants in `icons`, each the icon's Font Awesome name,
  never literals; a test checks each is in the embedded font.
- **Which icon goes where is data:** a table (`BY_COMMAND`) maps a command id to its icon, so a new
  command gets its menu icon with no menu code, and one function (`for_pane`) maps each dock pane
  to its icon, for its tab ([section 10](#10-the-dock)). An app with nested menus maps them the same
  way.
- **Alignment:** menu icons sit in a fixed 16 pt column (Font Awesome's widest glyphs are 1.25 em);
  a command with no icon keeps the empty column, so labels line up.
- **Colour:** an icon takes its widget's text colour, so a disabled item's icon dims with it; a
  decorative icon takes a token.
- **Accessibility:** an icon is an atom egui leaves out of a widget's accessible name, so a widget
  is named by its text; an icon-only button gets a name and a tooltip of its own; a dock tab's title
  is text with the icon in it.

## 10. The dock

**In the skeleton.** The window under the menu bar is a dock (`crates/ui/src/dock.rs`): its panes
as [`egui_tiles`](https://github.com/rerun-io/egui_crates) tiles (rerun-io, MIT OR Apache-2.0,
made for egui 0.37), tabs and splits a person drags and resizes. The skeleton's panes are the
command list (`commands`) and the open command's view (`view`); the default layout puts the list
on the left, 300 per mille of the width, and the view beside it.

**The UI state is the layout's source of truth.** The layout is the UI state's `dock`, a small typed
tree (`DockNode`: tabs of panes by id with the one shown, or a split across or down into parts,
each with its share per mille) that `ui.set` sets and `ui.inspect` reads. Each frame the dock is
built from it when it changed since it was built (an agent's `ui.set`, a restored launch), drawn,
and read back: what a person dragged or resized goes to the state through `ui.set`. egui_tiles'
own saved state is never loaded. `ui.set` refuses a layout deeper than 6 or holding more than 32
items, a split with no parts or a share out of 1–1000, empty tabs, or a pane the dock doesn't
have; a saved file with such a layout is refused as malformed. Whatever layout it is given, the
dock shows each pane once: one left out is added, one named twice is shown once. Hiding the
command list (`show_list`) hides its tile.

Its look, from the tokens, through egui_tiles' `Behavior`:

| Part | Value |
|---|---|
| Gap between panes and around the dock (`GAP`) | 4, in `chrome` |
| Separator: idle / hovered / dragged | `chrome` / `accent` / `accent` |
| Tab bar height (`TAB_BAR_HEIGHT`, the row + 4) | 22 |
| Tab bar fill / line under it | `header` / `frame` |
| The shown tab | fill `background`, outline `frame`: it joins its pane |
| Other tabs | fill `header`, no outline, title `muted` |
| A tab's title | its pane's icon, then its name, one line of text |
| The focused pane's tab (the pane last clicked in) | a 2 px `accent` underline |
| A pane's body | fill `background`, 1 px `frame` stroke, inner margin 6 (`PANE_MARGIN`) |
| Dragging a pane | the dragged pane under `accent` at 40 %; where it would land in `accent` at 15 %, edged 1 px `accent` |
| Closing a tab | not offered: the skeleton's panes are fixed, and the list hides by `show_list` |

## 11. The window and the app icon

**In the skeleton.** In `apps/<app>/src/window.rs` and `place.rs`, and `crates/ui/src/icon.rs`.

- **Frame:** the system's own title bar and frame: no custom decorations, no transparent window.
- **Title:** the product name.
- **Size and place:** the app keeps them itself, not eframe (`persist_window: false`), in a file
  beside the UI state's (`--ui-state <file>`; the window's is `<file>` with `.window.json`). It
  saves only a normal window's rectangle, and maximized on its own: one closed minimized, maximized
  or full screen keeps the last normal one. At launch it reads the monitors before the window opens
  (the app runs eframe on its own event loop, `place::Launch`) and restores the rectangle only when
  at least half of it is on a connected monitor and it is over the minimum (640 × 400); otherwise
  the window opens at its default size (1280 × 800, no more than nine tenths of the monitor),
  centred on the primary monitor. The file is read through the backends' capped read and written
  through their atomic writer, as hostile input.
- **Renderer:** glow (OpenGL), the skeleton's default: small, and enough for its window. An app that
  needs more switches to wgpu in its window's `eframe` features; one of the swarm's tools does, asking the
  GPU for its own largest texture side so a window across monitors fits, with vsync on and an
  environment variable to turn it off when measuring frame times.
- **App icon:** a generic one drawn in code from the theme's palette, with no third-party artwork:
  a dark tile (`header` to `base`) holding a list of three commands, each a `text` bullet and a bar
  in the series' first colours. The window uses it at 256 px. An app replaces it with its own,
  drawn the same way. A browser favicon or a Windows `.ico` (embedded in the exe with its version
  info by a `build.rs`) is the app's to add: generated from the drawing code and checked in, with a
  test that fails when they drift from it.

## 12. Menus

**In the skeleton.** The menus are the commands' menu paths and shortcuts
([app-architecture.md › The minimum interfaces](app-architecture.md#the-minimum-interfaces)): the
UI maps the same data to wherever the platform puts menus, and no command changes for it.

- **Windows and Linux: the window's own bar.** egui's menu bar across the top of the window, a menu
  per first part of the paths, each item with its icon ([section 9](#9-icons)), its shortcut and,
  when it can't run now, greyed out with why ([section 8](#8-density) has its metrics).
- **macOS: the system menu bar.** The menus are at the top of the screen and the window draws none:
  the dock fills it under the title bar. `crates/ui/src/native_menu.rs` makes the bar as plain data
  from `Shell::menus()` (tested on every platform), and `apps/<app>/src/macos_menu.rs` hands it to
  AppKit with [`muda`](https://github.com/tauri-apps/muda) (tauri-apps, MIT OR Apache-2.0) on the
  window's own event loop. Automatic window tabbing is off, so AppKit adds no tab items. If `muda`
  can't make the bar, the window draws its own and says so on stderr.
- **The macOS bar, left to right:**

  | Menu | Holds |
  |---|---|
  | The application menu, titled by the system with the app bundle's name (an unbundled binary's, as `cargo run`'s, shows the executable's) | About \<product> (the system's panel, with the name and version), the app's own `App` items, Services, Hide \<product>, Hide Others, Show All, Quit \<product> (⌘Q) |
  | The app's menus | in the order the commands first name them, as the window's bar has them |
  | Window | the app's own `Window` items, then Minimize, Zoom and Bring All to Front; the system lists the app's windows there |
  | Help | last, when the app has one; the system adds its search field |

- **Where an app's own items go.** A command whose menu path starts with `App` (`App/Settings…`,
  with `Mod+Comma`: preferences) is in the application menu after About on macOS, and in a menu
  named `App` elsewhere. The app's own info (`Help/App info`, the skeleton's `app.info`) stays in
  Help on every platform: About is the system's panel, not a command.
- **Quit** closes the window as its close button does, so the UI state and the window's place are
  saved; the system's other items are the system's to run.
- **A click is a click.** A native item activates its command's row as a click in the window's bar
  does (`Shell::activate`), through the one dispatch, so consent and `unavailable` behave the same.
  Each item is greyed out while its command can't run now, set again each frame from the rows.
- **A shortcut fires once.** On macOS it is the item's key equivalent alone: AppKit takes it before
  the window sees the key, so egui never gets it too. A command's shortcut that a system item takes
  (⌘Q, ⌘H, ⌥⌘H, ⌘M) keeps its item without the key. `Mod` is Command; `Plus` is the `=` key.
- **Layers.** `muda` is a toolkit to `layers`: the UI layer and the apps built on it may link it,
  nothing below. The skeleton links it in the windowed app, for macOS only (no GTK elsewhere).

## 13. Changing the UI substrate

1. **A colour** is a token in the engine's `theme.rs`, named for what it means, with a value in
   each look, in `TOKENS`. Its contrast is measured where it is read: a shortfall is nudged and
   noted, or kept in `KEPT_SHORT` with its ratio and why. Change its row in this document with it:
   the colour test fails on a colour that differs.
2. **An identifier's colour** comes from a range, by `pick` or `at`, never a colour of its own.
3. **A metric** goes in `density`, a text size in `fonts`, an icon in `icons`, each with a doc
   comment saying where its value comes from.
4. **A new command** gets its icon in the command table, and **a new pane** its icon in the pane
   mapping.
5. **Every word** a person reads is in the string catalog.
6. **Check a UI change by eye:** render it offscreen and put the picture on the pull request.
7. **Edit this document, never [the page](app-ui-substrate.html).** In swarm-core,
   `cargo xtask docs render` makes the page from it: its text, a swatch beside each colour, each
   section's status as a badge, and the ranges drawn from the skeleton's `theme.rs` where
   `<!-- page: ranges -->` stands (each range's table of entries is folded there, as the drawing
   shows them). The test `the_pages_are_rendered_from_their_docs` fails when the
   committed page differs from a fresh render, naming the command and the first line that differs.
   It holds what `the_ui_substrate_page_and_doc_agree` and the page's half of
   `the_ui_substrate_has_the_skeletons_colours` held, and both are gone.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-14 | minor | New: the App UI Substrate. The theme as data, the 20 tokens and the 4 ranges in both looks, egui's visuals, the contrast rules and what `KEPT_SHORT` keeps, and the corner radius, type, density, icons, the dock (egui_tiles), the window and the app icon, and the menus (the system menu bar on macOS), each in the skeleton. A test holds every colour here to the skeleton's theme, and another holds the page beside it to each section's status (mornycorp/swarm-core#104). |
| 2026-11-14 | minor | [The page](app-ui-substrate.html) is generated from this doc by `cargo xtask docs render` (in swarm-core's own xtask; the markdown by `pulldown-cmark`): a swatch beside each colour, each status a badge, and the ranges drawn from the skeleton's `theme.rs` where `<!-- page: ranges -->` stands. `the_pages_are_rendered_from_their_docs` fails on a page that differs from a fresh render. A link to [the App Substrate](app-substrate.html), the page that explains the whole substrate (mornycorp/swarm-core#108). |
