# Performance measurements

Targets come from the project brief; they are engineering targets, not
guarantees. Reproduce with `scripts/measure.sh` (release build, real window,
`--bench` scenarios in `src/bench.rs`).

## Environment (run 1)

| | |
|---|---|
| Date | 2026-11-17 |
| Machine | Apple M4 (MacBook Air), 16 GiB, on battery |
| OS | macOS 27.2.1 |
| Display | Built-in Retina 2560 × 1664 (pixels per point 2.0) |
| Build | `--release`, `aarch64-apple-darwin`, rustc 1.97.1, commit `17a159d` |
| Renderer | eframe/egui 0.37, glow (OpenGL) |
| LaTeX | Tectonic 0.17.0 |
| Binary | 9.4 MiB |

These numbers were taken **before** math moved into text boxes (commit
`17a159d`). Text layout changed afterwards (word-level layout with inline
math), so the frame-time numbers in particular must be re-measured.

## Results

| Metric | Target | Measured | Status |
|---|---|---|---|
| Warm start to first frame | ≤ 3 s to responsive window | 192–215 ms first frame; 690–730 ms until background work idle (3 runs) | met |
| Memory, empty document | ≤ 250 MiB | 113 MiB RSS | met |
| Drag 10 objects on deck B (20 slides × 100 objects), 600 frames | p95 ≤ 25 ms | frame CPU p50 1.8 ms, p95 2.3 ms, max 3.3 ms | met (pre-rework) |
| 100 slide switches, deck C (50 slides, 100 × 4K images, 25 distinct) | no unbounded growth | RSS 204 → 258 MiB (50 switches) → 258 MiB (100); texture cache 80 MiB of 128 MiB budget | met |
| 100 import/remove cycles of 4K JPEGs | no unbounded growth | RSS 140 → 204 MiB (50) → 207 MiB (100); cycle p50 33 ms, p95 133 ms | met |
| 10 save/reopen cycles of deck B | no unbounded growth | RSS 165 → 171 MiB; save or open p50 34 ms, max 72 ms | met |
| Idle CPU, blank deck, 30 s after 5 s settling | ≤ 2 % of one core | 7.2 % in the run, with 1,911 frames painted | **not verified** — the window was used during the measurement (textures for typeset math appeared in a blank deck), so the run is invalid; must be repeated untouched |
| Export latency, deck B | — | not measured (run stopped) | pending |
| End-to-end workflow (`--bench smoke`) | pass | 19/19 steps pass on the current code | met |

## Method notes

* Frame CPU time is eframe's `frame.info().cpu_usage` (CPU time spent on
  the previous frame: UI build, tessellation and paint submission). GPU time
  is not included.
* RSS is sampled with `ps`; GPU memory used by textures is not part of RSS and
  is bounded separately by the texture cache budget (128 MiB of decoded
  pixels, including mipmaps).
* Idle CPU is measured externally from the process's cumulative CPU time
  (`ps -o time`) over a 30 s window.
