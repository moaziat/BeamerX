# BeamerX

A lightweight, native visual editor for scientific presentations, written in
Rust for **Linux and macOS**, with a PowerPoint-style layout. Type text
directly on 16:9 slides with **LaTeX math written like LaTeX** (`$f(x)$`
inline, `$$…$$` centred) rendered live by the real TeX engine; add images and
shapes; save editable projects; present full-screen; export a self-contained
**LaTeX Beamer** folder and a **PDF**.

Status: **MVP 0.1** (see [scope](#scope-of-010) and
[known limitations](#known-limitations)). Measured performance is in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

## Requirements

| | |
|---|---|
| OS | Linux with glibc 2.35+ on x86_64 or ARM64 (X11/XWayland), or macOS 11+ on Apple Silicon. |
| Rust | 1.85 or newer (`rustup` stable) |
| LaTeX | **[Tectonic](https://tectonic-typesetting.github.io/)** (recommended: `brew install tectonic`) or a TeX Live/MacTeX `xelatex`/`lualatex`/`pdflatex`. Needed for math previews and PDF export; everything else works without it. Tectonic downloads the LaTeX packages it needs on first use (network required once). |

BeamerX looks for the engine on `PATH` and in `/opt/homebrew/bin`,
`/usr/local/bin`, `/Library/TeX/texbin` and `~/.cargo/bin` (apps started from
Finder do not inherit your shell `PATH`). Set `BEAMERX_LATEX=/path/to/engine`
to choose one explicitly.

## Install

Install the latest checksum-verified release into `~/.local/bin` without
Rust or a compiler:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/moaziat/BeamerX/main/install.sh | sh
```

The installer detects Linux/macOS and x86_64/ARM64, downloads the matching
GitHub Release asset, and verifies it against the published SHA-256 checksum
before installing. Set `BEAMERX_INSTALL_DIR` to choose another destination or
`BEAMERX_VERSION=v0.1.0` to install a specific release.

Linux desktops need OpenGL, X11 (or XWayland), and an XDG desktop portal for
native file dialogs. These are already present on normal Ubuntu, Fedora,
Debian, Arch, GNOME, and KDE desktop installations. LaTeX remains optional.

## Build and run from source

```sh
cargo run --release                 # start the editor
cargo run --release -- deck.beamerx # open a project
scripts/bundle-macos.sh             # build dist/BeamerX.app (shipping build)
scripts/package-release.sh x86_64-unknown-linux-gnu
```

Build profiles: `release` is tuned for quick rebuilds (parallel codegen,
incremental, no LTO; a rebuild after an edit takes a few seconds on an M4).
`dist` adds thin LTO and a single codegen unit for the shipped app bundle and
performance measurements; it takes about 1.5 min to compile.

## Using BeamerX

The window follows PowerPoint: a ribbon (**File**, Home, Insert, Design,
Arrange, Slide Show, View) at the top, slide thumbnails on the left, the slide
in the middle, zoom at the bottom right. *Arrange › Format Pane* opens an
optional pane with exact position, size, colours and picture details.

* **Text** — double-click empty space on the slide, or *Text Box* (Home or
  Insert, or `T`) and click, then type. Click any text box to put the caret
  there; select a box and just start typing to add to it. Double-click selects
  a word, triple-click a paragraph. `Esc` or clicking elsewhere finishes; empty
  boxes are removed (layout placeholders stay).
* **Formatting** — **bold** (`⌘B`), *italic* (`⌘I`) and colour apply to the
  selected characters, the word at the caret, or what you type next (*Home ›
  Font*). Font size and alignment apply to the box.
* **Titles and hierarchy** — *Home › Paragraph › Style*: Title, Subtitle,
  Heading 1, Heading 2, Body (per paragraph). Bullets and numbering per
  paragraph with five outline levels: `Tab` / `⇧Tab` (or Indent / Outdent)
  demote and promote; `⌫` at the start of an item promotes it, then ends the
  list. Numbers restart per level automatically.
* **Slide layouts** — *New Slide* adds Title and Content; *Layout* offers Title
  Slide, Title and Content, Section Header, Two Content, Title Only and Blank.
  Placeholders ("Click to add title") disappear from the export until filled;
  slide titles become the PDF outline (bookmarks).
* **Math** — write LaTeX math inside any text box exactly as in LaTeX:
  `The energy $E = mc^2$ is…` renders inline as soon as the closing `$` is
  typed; `$$\int_0^1 f(x)\,dx$$` is a centred display formula; `\$` is a
  literal dollar sign. When the caret is inside a formula its source is shown
  for editing; clicking a rendered formula opens it. Errors appear in red under
  the box. Formulas are typeset by the same LaTeX engine as the export (about
  0.5 s after you stop typing the first time, instant once cached).
  *Home › Math* inserts `$ $` with the caret inside. Macros go in
  *Design › Title & LaTeX…*.
* **Move / resize** — drag any object to move it (for the text box being
  edited, drag its blue outline). Handles resize; text boxes resize in width and
  grow in height with their content. Snapping to the slide centre/edges and other
  objects (hold `⌘` to disable, `⇧` to constrain); arrow keys nudge (`⇧` = 10 pt).
* **Images** — drag PNG/JPEG files from Finder onto the slide, or *Insert ›
  Picture…*. A selected image shows a three-dot menu for fit/aspect sizing,
  exact Size & Position controls, and front/back ordering.
* **Shapes** — rectangle, rounded rectangle, oval, triangle, diamond, cube,
  cylinder, bevel, block arrow, line and arrow from the *Home › Drawing* or *Insert › Shapes* gallery; fill and
  outline stay available in *Home › Drawing*.
* **Edit** — shift-click / marquee for multi-selection; align and order in
  *Arrange*; `⌘C`/`⌘X`/`⌘V`/`⌘D`; `⌫` deletes; undo/redo `⌘Z`/`⇧⌘Z`.
* **Slides** — *Home › New Slide*, or thumbnails: click to select, drag to
  reorder, right-click for new/duplicate/delete.
* **Sections** — add a named divider before the current slide from the Slides
  pane or a slide's context menu. Click a section header to collapse/expand;
  right-click it to rename or remove it. Sections are included in Beamer export.
* **Dashes and line ends** — *Home › Drawing* or the Format pane: dash type
  (solid, round dot, square dot, dash, dash dot, long dash, long dash dot) for
  any outline; begin/end types for lines and arrows (arrow, open arrow,
  stealth, diamond, oval, bar). Exported as TikZ dash patterns and
  `arrows.meta` tips of the same size.
* **Colours** — every colour button opens a picker with theme colours (tints
  and shades), standard colours, suggestions for the current colour (lighter,
  darker, complementary, analogous, triadic), recent colours, and *More
  Colors…* with a colour wheel, brightness bar and hex input.
* **Theme** — *View › Theme*: Light (default), Dark or System; remembered
  between launches.
* **View** — pinch or `⌘`-scroll to zoom around the pointer, scroll to pan,
  space-drag to pan, zoom slider bottom right, `⌘0` fit.
* **Present** — *Present* (top right), *Slide Show › From Beginning*, or
  `⌘↩`. Arrows / space / click to advance, `Esc` to leave.
* **Save** `⌘S`; unsaved work is auto-saved to a recovery file 30 s after the
  last change and offered back after a crash.
* **Export** — *File › Export Beamer* (`⌘E`) writes a folder with `main.tex`
  and `images/`; *File › Export PDF* (`⇧⌘E`) compiles it in the background
  (cancellable).

## Project format (`.beamerx`)

A ZIP container: `mimetype`, a versioned `project.json` (document + asset
manifest) and the **original image bytes** under `assets/<sha256>.<ext>`
(stored, not recompressed, de-duplicated by content hash). No base64 in JSON.
Saves are atomic (temp file → fsync → rename); a failed save never damages the
previous file. Format 7 adds the block-arrow shape; format 6 adds line ends; format 5 adds a persistent stock Beamer theme; format 4 adds named slide sections; format 3 stores character
formats as spans and paragraph formats (style, list, level) per line. Older files are migrated on open:
format-1 equation objects become text boxes containing `$\displaystyle …$`,
and format-2 box-wide bullets become per-paragraph bullets. Loading validates structure, bounds sizes, never uses archive
entry names as paths, and verifies every image against its hash. Format
changes go through `persistence::migrate` with frozen fixtures in
`tests/fixtures`.

## Export fidelity

All geometry is stored in slide-space PostScript points (`bp`) on a 16 cm × 9 cm
page, exactly `\documentclass[aspectratio=169]{beamer}`, and every object is
placed absolutely with TikZ, so positions and sizes match the editor exactly.

* **Math** is typeset by the same LaTeX engine, preamble, packages
  (`amsmath`, `amssymb`, `fix-cm`) and font size in the editor and in the
  export: what you see is what TeX produces. Inline formulas sit on the text
  baseline using the depth TeX measures, and are never split across lines in
  either place.
* **Text** uses Latin Modern Sans in both the editor (bundled font) and
  Beamer's default text font. Every line is its own TeX paragraph with its own
  size, outline indent (`\leftskip`) and hanging list label, following the same
  baseline convention as the editor (`\strut`, 1.2 × size line spacing, TeX's
  `\lineskip` rule for tall math). Bold/italic/colour runs become
  `\bfseries`/`\itshape`/`\color` groups. Text boxes are exported with TikZ's
  `flush` alignments, which fill lines greedily and never hyphenate, like the
  editor; small width differences from kerning can still move a word.
* Math is serif (`\usefonttheme[onlymath]{serif}`), as in the preview.
* Colours with transparency are exported with TikZ opacity.
* The Design ribbon includes stock Beamer themes (Default, Madrid, Boadilla,
  CambridgeUS, Berlin and Copenhagen); the editor previews their structural
  chrome and export emits the matching `\usetheme{…}` command.
* Shape drawing includes flat primitives plus native vector Cube, Cylinder
  and Bevel shapes; no bitmap assets or extra runtime dependencies are used.
* The exported `main.tex` compiles with Tectonic in one command, or with
  XeLaTeX/LuaLaTeX in two passes (`latexmk -xelatex`). pdfLaTeX works for
  documents without non-Latin text, best effort.

## Known limitations

* Font size is per text box (scaled per paragraph by its style); there is no
  per-word size, underline or font family choice yet.
* Math previews ignore colour commands inside the source (`\color`); the text
  colour applies to the whole formula (export honours inline colours).
* Plain text outside `$…$` is literal: LaTeX commands there are printed, not
  executed.
* User preamble lines that only make sense in Beamer (`\usetheme`,
  `\setbeamer…`, …) are skipped when typesetting previews.
* Equation preview rasterization uses macOS CoreGraphics; other platforms show
  the source instead of the typeset formula.
* No rotation, cropping, grouping, animations, speaker notes, Beamer import,
  PowerPoint, collaboration or cloud sync (out of scope for 0.1).
* Opening a `.beamerx` by double-clicking it in Finder is not wired up yet;
  use *File › Open*, drag the file onto the window, or pass it as an argument.

## Architecture

```
src/
  document.rs     typed, versioned model (slide space in bp); no GUI types
  commands.rs     reversible commands, bounded undo history, dirty tracking
  editor/         editor state; canvas (hit-test, drag/resize/snap); on-slide
                  text editor with live math; format pane; slide thumbnails
  renderer/       paints slides for canvas, thumbnails and presentation; text
                  layout with inline math; bounded LRU texture cache
  assets.rs       content-addressed image store, import guards, decoding
  rich.rs         `$…$` / `$$…$$` parsing shared by editor layout and export
  math.rs         debounced, batched, disk-cached LaTeX math previews
  latex.rs        external engine discovery; cancellable, time-limited runs
  export.rs       Beamer generation; atomic folder and PDF export
  persistence.rs  .beamerx packages; atomic save; validated load; migrations
  platform/       macOS menu bar (muda) and CoreGraphics PDF rasterization
                  (the only `unsafe` code, documented in the module)
  jobs.rs         bounded worker pools (decode, LaTeX, I/O)
  perf.rs         frame statistics; bench.rs / bench_smoke.rs benchmarks;
                  bench_demo.rs scripted, self-recorded product demo
```

Rendering is event-driven (no continuous repaint); image decoding, LaTeX,
saving, loading and export run on worker threads.

**"Written in Rust" vs "pure Rust".** The application and all its logic are
Rust, using `egui`/`eframe` with the OpenGL (`glow`) renderer, `winit`, `muda`
and `rfd` for native windows, menus and dialogs. Image decoding (`image`,
`zune-jpeg`, `png`) and ZIP/zlib (`zip`, `zlib-rs`) are pure Rust. The app
links macOS system frameworks (AppKit, OpenGL, CoreGraphics). LaTeX is **not**
bundled: it is a separately installed program (Tectonic is itself a Rust
program embedding the XeTeX engine, written in C/C++) invoked as a subprocess
with shell escape disabled.

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all                       # LaTeX integration tests need tectonic
cargo run --example showcase -- out/   # sample deck + Beamer + PDF + PNGs
cargo run --release -- --bench smoke   # end-to-end UI workflow (23 steps)
scripts/measure.sh                     # performance measurements
```

**Demo video.** `scripts/make-demo-video.sh DECK.beamerx OUT.mp4 [ORIGINAL…]`
opens one window for a few minutes in which BeamerX rebuilds `DECK` from a
blank slide through its own UI: it double-clicks and types every text box and
equation, drops, moves and resizes pictures, draws shapes, bolds runs,
presents, then saves the Beamer source (*File › Export Beamer*) and scrolls
through its `main.tex`. Coloured step titles and a visible cursor are drawn on
top. The app records its own frames (not the screen), and AVFoundation encodes
them (`scripts/frames-to-mp4.swift`, no ffmpeg needed). Optional ORIGINAL
images or PDF pages (`deck.pdf#73`) come first; the saved Beamer folder is
kept next to the video. `cargo run --example template_73_74 -- deck.pptx out/` builds the
reference deck used for slides 73–74 of the winter-school template (`.beamerx`,
Beamer folder and PDF).

Logging: `BEAMERX_LOG=beamerx=debug cargo run`.

## Scope of 0.1

Slides (add/remove/duplicate/reorder), text with inline/display LaTeX math, PNG/JPEG images,
lines/arrows/rectangles/ellipses; move/resize, multi-selection, alignment,
z-order, undo/redo, zoom/pan; `.beamerx` save/open with packaged assets;
autosave recovery; full-screen presentation; Beamer folder and PDF export.

## License

MIT OR Apache-2.0. Bundled Latin Modern Sans fonts: GUST Font License (see
[`assets/fonts/NOTICE.md`](assets/fonts/NOTICE.md)).
