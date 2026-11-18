//! BeamerX — a lightweight native editor for scientific Beamer presentations.
//!
//! Module boundaries:
//! * [`document`]    – typed, versioned document model (no GUI types)
//! * [`commands`]    – reversible operations and bounded undo history
//! * [`editor`]      – editor state, canvas interaction, inspector, slide list
//! * [`renderer`]    – painting slides; bounded texture cache
//! * [`assets`]      – content-addressed image store, import guards, decoding
//! * [`rich`]        – `$…$` / `$$…$$` math inside text (shared by editor and export)
//! * [`math`]        – debounced, batched, cached LaTeX math previews
//! * [`latex`]       – external LaTeX toolchain discovery and invocation
//! * [`export`]      – Beamer `.tex` generation, folder and PDF export
//! * [`persistence`] – `.beamerx` package save/load with atomic writes
//! * [`platform`]    – macOS integration (menu bar, PDF rasterization)
//! * [`perf`] / [`bench`] – frame statistics and reproducible benchmarks
//! * [`jobs`]        – bounded worker pools

pub mod app;
pub mod assets;
pub mod bench;
mod bench_smoke;
pub mod commands;
pub mod document;
pub mod editor;
pub mod export;
pub mod jobs;
pub mod latex;
pub mod math;
pub mod perf;
pub mod persistence;
pub mod platform;
pub mod renderer;
pub mod rich;
pub mod ui;
