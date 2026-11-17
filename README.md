# BeamerX

A lightweight, native visual editor for scientific presentations, written in
Rust for **macOS on Apple Silicon**. Place text, LaTeX equations, images and
shapes on 16:9 slides; save editable projects; present full-screen; export a
self-contained **LaTeX Beamer** folder and a **PDF**.

Status: **MVP 0.1** (see [scope](#scope-of-010) and
[known limitations](#known-limitations)). Measured performance is in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

## Requirements

| | |
|---|---|
| OS | macOS 11+ on Apple Silicon (`aarch64-apple-darwin`). Other platforms compile but are not supported releases. |
| Rust | 1.85 or newer (`rustup` stable) |
| LaTeX | **[Tectonic](https://tectonic-typesetting.github.io/)** (recommended: `brew install tectonic`) or a TeX Live/MacTeX `xelatex`/`lualatex`/`pdflatex`. Needed for equation previews and PDF export; everything else works without it. Tectonic downloads the LaTeX packages it needs on first use (network required once). |

BeamerX looks for the engine on `PATH` and in `/opt/homebrew/bin`,
`/usr/local/bin`, `/Library/TeX/texbin` and `~/.cargo/bin` (apps started from
Finder do not inherit your shell `PATH`). Set `BEAMERX_LATEX=/path/to/engine`
to choose one explicitly.

## Build and run

```sh
cargo run --release                 # start the editor
cargo run --release -- deck.beamerx # open a project
scripts/bundle-macos.sh             # build dist/BeamerX.app
```

## Using BeamerX

* **Insert** with the toolbar, the *Insert* menu, or single-key tools on the
  canvas: `V` select, `T` text, `E` equation, `R` rectangle, `O` ellipse, `L`
  line, `A` arrow, `I` image. Click to place at a default size or drag to size.
* **Images**: drag PNG/JPEG files from Finder onto the slide, or *Insert ›
  Image…* (multi-select supported).
* **Text**: double-click (or `Enter`) to edit in place. Bold, italic, size,
  colour, alignment and bullet lists are in the inspector.
* **Equations**: type LaTeX math in the inspector (`Enter` or double-click
  focuses it). The preview is typeset by the real LaTeX engine, ~0.2 s after you
  stop typing. Drag a corner to scale (changes the font size).
  Document-wide macros go in *LaTeX preamble* (shown when nothing is selected).
* **Edit**: shift-click / marquee for multi-selection; drag to move (snaps to
  slide centre/edges and other objects, hold `⌘` to disable, `⇧` to constrain);
  arrow keys nudge (`⇧` = 10 bp); align and z-order in the inspector or
  *Arrange* menu; `⌘C`/`⌘X`/`⌘V`/`⌘D`; `⌫` deletes; unlimited-ish undo
  (`⌘Z`/`⇧⌘Z`, bounded by memory).
* **Slides**: thumbnails on the left: click to select, drag to reorder,
  right-click for new/duplicate/delete; `⇧⌘N` new slide.
* **View**: pinch or `⌘`-scroll to zoom around the pointer, scroll to pan,
  space-drag to pan, `⌘0` fit, `⌘1` actual size.
* **Present**: `⌘↩` or *Present*. Arrows / space / click to advance, `Esc` to
  leave.
* **Save** `⌘S`; unsaved work is auto-saved to a recovery file 30 s after the
  last change and offered back after a crash.
* **Export**: *Export Beamer* (`⌘E`) writes a folder with `main.tex` and
  `images/`; *Export PDF* (`⇧⌘E`) compiles it in the background (cancellable).

## Project format (`.beamerx`)

A ZIP container: `mimetype`, a versioned `project.json` (document + asset
manifest) and the **original image bytes** under `assets/<sha256>.<ext>`
(stored, not recompressed, de-duplicated by content hash). No base64 in JSON.
Saves are atomic (temp file → fsync → rename); a failed save never damages the
previous file. Loading validates structure, bounds sizes, never uses archive
entry names as paths, and verifies every image against its hash. Format
changes go through `persistence::migrate` with frozen fixtures in
`tests/fixtures`.

## Export fidelity

All geometry is stored in slide-space PostScript points (`bp`) on a 16 cm × 9 cm
page, exactly `\documentclass[aspectratio=169]{beamer}`, and every object is
placed absolutely with TikZ, so positions and sizes match the editor exactly.

* **Equations** are typeset by the same LaTeX engine, preamble, packages
  (`amsmath`, `amssymb`, `fix-cm`) and font size in the editor and in the
  export: what you see is what TeX produces.
* **Text** uses Latin Modern Sans in both the editor (bundled font) and
  Beamer's default text font. Line positions follow the same baseline
  convention (`\strut`, 1.2 × size line spacing). Line breaks can occasionally
  differ: TeX uses optimal-fit breaking and kerning tables; the editor uses
  greedy wrapping.
* Math is serif (`\usefonttheme[onlymath]{serif}`), as in the preview.
* Colours with transparency are exported with TikZ opacity.
* The exported `main.tex` compiles with Tectonic in one command, or with
  XeLaTeX/LuaLaTeX in two passes (`latexmk -xelatex`). pdfLaTeX works for
  documents without non-Latin text, best effort.

## Known limitations

* Text formatting is per text box (no mixed bold/italic inside one box); text
  boxes do not support inline `$…$` math — use equation objects.
* Equation previews ignore colour commands inside the source (`\color`); the
  object colour applies to the whole equation (export honours inline colours).
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
  editor/         editor state; canvas (hit-test, drag/resize/snap, inline
                  text); inspector; slide thumbnails
  renderer/       paints slides for canvas, thumbnails and presentation;
                  bounded LRU texture cache with downsampled image levels
  assets.rs       content-addressed image store, import guards, decoding
  math.rs         debounced, batched, disk-cached LaTeX equation previews
  latex.rs        external engine discovery; cancellable, time-limited runs
  export.rs       Beamer generation; atomic folder and PDF export
  persistence.rs  .beamerx packages; atomic save; validated load; migrations
  platform/       macOS menu bar (muda) and CoreGraphics PDF rasterization
                  (the only `unsafe` code, documented in the module)
  jobs.rs         bounded worker pools (decode, LaTeX, I/O)
  perf.rs         frame statistics; bench.rs / bench_smoke.rs benchmarks
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
cargo run --release -- --bench smoke   # end-to-end UI workflow (16 steps)
scripts/measure.sh                     # performance measurements
```

Logging: `BEAMERX_LOG=beamerx=debug cargo run`.

## Scope of 0.1

Slides (add/remove/duplicate/reorder), text, equations, PNG/JPEG images,
lines/arrows/rectangles/ellipses; move/resize, multi-selection, alignment,
z-order, undo/redo, zoom/pan; `.beamerx` save/open with packaged assets;
autosave recovery; full-screen presentation; Beamer folder and PDF export.

## License

MIT OR Apache-2.0. Bundled Latin Modern Sans fonts: GUST Font License (see
[`assets/fonts/NOTICE.md`](assets/fonts/NOTICE.md)).
