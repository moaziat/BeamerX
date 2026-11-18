//! Reproducible in-app benchmarks (`beamerx --bench <scenario>`).
//!
//! Scenarios drive the real application (real window, renderer, workers)
//! and print one JSON object with the measurements:
//!
//! * `idle`     – deck A (blank). Reports startup time and frames painted
//!   while idle; CPU% is measured externally by `scripts/measure.sh`.
//! * `drag`     – deck B (20 slides × 100 objects). Injects pointer events
//!   to drag 10 objects for 600 frames; reports frame CPU p50/p95/max.
//! * `switch`   – deck C (50 slides, 100 4K images). 100 slide switches;
//!   reports RSS before/after and texture-cache peak.
//! * `images`   – 100 import/remove cycles of 4K images; RSS growth.
//! * `saveload` – 10 save/reopen cycles of deck B; RSS growth and timings.

use crate::app::BeamerApp;
use crate::assets::{AssetMeta, AssetStore};
use crate::document::*;
use egui::{Event, Modifiers, PointerButton, Pos2};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Idle,
    Drag,
    Switch,
    Images,
    SaveLoad,
    /// Wait until the opened document is fully rendered, save a PNG of the
    /// window to `--out`, and quit (visual regression checks).
    Screenshot,
    /// End-to-end workflow through the real UI (injected input + menu
    /// actions): create, edit, drag, undo, import, save, reopen, export,
    /// present. Reports pass/fail per step.
    Smoke,
}

impl std::str::FromStr for Scenario {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "idle" => Scenario::Idle,
            "drag" => Scenario::Drag,
            "switch" => Scenario::Switch,
            "images" => Scenario::Images,
            "saveload" => Scenario::SaveLoad,
            "screenshot" => Scenario::Screenshot,
            "smoke" => Scenario::Smoke,
            _ => {
                return Err(format!(
                    "unknown scenario {s:?} (idle|drag|switch|images|saveload|screenshot)"
                ));
            }
        })
    }
}

enum Phase {
    Setup,
    Warmup { until_idle_since: Option<Instant> },
    Run { step: u64 },
    Done,
}

pub struct Bench {
    pub scenario: Scenario,
    pub process_start: Instant,
    pub idle_seconds: u64,
    out: Option<PathBuf>,
    phase: Phase,
    first_frame: Option<Duration>,
    t_phase: Instant,
    frames_in_phase: u64,
    results: serde_json::Map<String, Value>,
    work_dir: PathBuf,
    drag_ids: Vec<ObjectId>,
    drag_origin: Option<Pos2>,
    rss: Vec<u64>,
    files: Vec<PathBuf>,
    wait_frames: u64,
    cycle_start: Option<Instant>,
    cycle_ms: Vec<f64>,
    done_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    smoke: crate::bench_smoke::Smoke,
}

impl Bench {
    pub fn new(scenario: Scenario, process_start: Instant, out: Option<PathBuf>, idle_seconds: u64) -> Self {
        let work_dir = std::env::temp_dir().join(format!("BeamerX/bench-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&work_dir);
        Self {
            scenario,
            process_start,
            idle_seconds,
            out,
            phase: Phase::Setup,
            first_frame: None,
            t_phase: Instant::now(),
            frames_in_phase: 0,
            results: serde_json::Map::new(),
            work_dir,
            drag_ids: Vec::new(),
            drag_origin: None,
            rss: Vec::new(),
            files: Vec::new(),
            wait_frames: 0,
            cycle_start: None,
            cycle_ms: Vec::new(),
            done_flag: None,
            smoke: Default::default(),
        }
    }

    fn put(&mut self, k: &str, v: Value) {
        self.results.insert(k.into(), v);
    }

    fn finish(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        self.put("scenario", json!(format!("{:?}", self.scenario).to_lowercase()));
        self.put("os", json!(std::env::consts::OS));
        self.put("arch", json!(std::env::consts::ARCH));
        self.put("pixels_per_point", json!(ctx.pixels_per_point()));
        self.put(
            "texture_peak_mib",
            json!(app.textures.stats.peak_bytes as f64 / 1048576.0),
        );
        self.put("texture_evictions", json!(app.textures.stats.evictions));
        self.put("image_decodes", json!(app.textures.stats.decodes));
        let line = Value::Object(self.results.clone()).to_string();
        println!("BENCH_RESULT {line}");
        if let Some(p) = &self.out {
            let _ = std::fs::write(p, format!("{line}\n"));
        }
        let _ = std::fs::remove_dir_all(&self.work_dir);
        self.phase = Phase::Done;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn rss() -> u64 {
        crate::platform::resident_bytes().unwrap_or(0)
    }

    /// Drive one frame. Returns false when the benchmark is finished.
    pub fn tick(&mut self, app: &mut BeamerApp, ctx: &egui::Context) -> bool {
        if self.first_frame.is_none() {
            self.first_frame = Some(self.process_start.elapsed());
            let ms = self.first_frame.map(|d| d.as_secs_f64() * 1000.0);
            self.put("first_frame_ms", json!(ms));
        }
        self.frames_in_phase += 1;
        if matches!(self.phase, Phase::Done) {
            return false;
        }
        match self.scenario {
            Scenario::Idle => self.tick_idle(app, ctx),
            Scenario::Drag => self.tick_drag(app, ctx),
            Scenario::Switch => self.tick_switch(app, ctx),
            Scenario::Images => self.tick_images(app, ctx),
            Scenario::SaveLoad => self.tick_saveload(app, ctx),
            Scenario::Screenshot => self.tick_screenshot(app, ctx),
            Scenario::Smoke => self.tick_smoke(app, ctx),
        }
        !matches!(self.phase, Phase::Done)
    }

    fn enter(&mut self, p: Phase) {
        self.phase = p;
        self.t_phase = Instant::now();
        self.frames_in_phase = 0;
    }

    /// Wait until background work has been quiet for 500 ms.
    fn warmed_up(&mut self, app: &BeamerApp, ctx: &egui::Context) -> bool {
        ctx.request_repaint_after(Duration::from_millis(50));
        if let Phase::Warmup { until_idle_since } = &mut self.phase {
            if app.is_idle() {
                let s = *until_idle_since.get_or_insert(Instant::now());
                return s.elapsed() > Duration::from_millis(500);
            }
            *until_idle_since = None;
        }
        false
    }

    // ---- idle ---------------------------------------------------------------

    fn tick_idle(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        match self.phase {
            Phase::Setup => {
                self.enter(Phase::Warmup { until_idle_since: None });
            }
            Phase::Warmup { .. } => {
                if self.warmed_up(app, ctx) {
                    self.put("ready_ms", json!(self.process_start.elapsed().as_secs_f64() * 1000.0));
                    self.put("rss_mib_start", json!(Self::rss() as f64 / 1048576.0));
                    println!("BENCH_READY {}", std::process::id());
                    self.enter(Phase::Run { step: 0 });
                    // Single deadline to end the idle window: no polling.
                    ctx.request_repaint_after(Duration::from_secs(self.idle_seconds));
                }
            }
            Phase::Run { .. } => {
                if self.t_phase.elapsed() >= Duration::from_secs(self.idle_seconds) {
                    // frames_in_phase counts this final wake-up too.
                    self.put("idle_seconds", json!(self.idle_seconds));
                    self.put(
                        "frames_painted_while_idle",
                        json!(self.frames_in_phase.saturating_sub(1)),
                    );
                    self.put("rss_mib_end", json!(Self::rss() as f64 / 1048576.0));
                    self.finish(app, ctx);
                }
            }
            Phase::Done => {}
        }
    }

    // ---- drag ---------------------------------------------------------------

    fn tick_drag(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        const FRAMES: u64 = 600;
        match self.phase {
            Phase::Setup => {
                let doc = deck_b(&mut app.assets);
                app.ed = crate::editor::Editor::new(doc, None);
                app.ed.go_to_slide(0);
                self.put(
                    "deck",
                    json!({"slides": app.ed.doc.slides.len(), "objects": app.ed.doc.object_count()}),
                );
                self.enter(Phase::Warmup { until_idle_since: None });
            }
            Phase::Warmup { .. } => {
                let view = app.canvas.last_view;
                if let Some(view) = view.filter(|_| self.warmed_up(app, ctx)) {
                    self.put("warmup_ms", json!(self.t_phase.elapsed().as_secs_f64() * 1000.0));
                    // Drag the 10 top-most objects, grabbing the top-most one.
                    let objs = &app.ed.slide().objects;
                    self.drag_ids = objs.iter().rev().take(10).map(|o| o.id).collect();
                    let grab = objs.last().map(|o| o.rect.center()).unwrap_or((100.0, 100.0));
                    let p = view.pt(grab.0, grab.1);
                    app.ed.selection = self.drag_ids.clone();
                    self.drag_origin = Some(p);
                    app.perf.clear();
                    self.enter(Phase::Run { step: 0 });
                }
            }
            Phase::Run { step } => {
                ctx.request_repaint();
                let o = self.drag_origin.expect("set in warmup");
                let ev = &mut app_inject(app);
                match step {
                    0 => ev.push(Event::PointerMoved(o)),
                    1 => ev.push(Event::PointerButton {
                        pos: o,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    }),
                    s if s < FRAMES + 2 => {
                        let t = (s - 2) as f32 / 60.0;
                        let p = o + egui::vec2(80.0 * (t * 1.3).sin(), 40.0 * (t * 2.1).sin());
                        ev.push(Event::PointerMoved(p));
                    }
                    s if s == FRAMES + 2 => {
                        let p = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(o);
                        ev.push(Event::PointerButton {
                            pos: p,
                            button: PointerButton::Primary,
                            pressed: false,
                            modifiers: Modifiers::NONE,
                        });
                    }
                    _ => {
                        let moved = self.drag_ids.iter().filter_map(|id| app.ed.object(*id)).count();
                        self.put("dragged_objects", json!(moved));
                        self.put("undo_entries_after_drag", json!(app.ed.history.len()));
                        self.put("frames", json!(app.perf.len()));
                        self.put("frame_cpu_ms_p50", json!(app.perf.percentile(50.0)));
                        self.put("frame_cpu_ms_p95", json!(app.perf.percentile(95.0)));
                        self.put("frame_cpu_ms_max", json!(app.perf.max()));
                        self.put("wall_s", json!(self.t_phase.elapsed().as_secs_f64()));
                        self.put("rss_mib", json!(Self::rss() as f64 / 1048576.0));
                        self.finish(app, ctx);
                        return;
                    }
                }
                self.phase = Phase::Run { step: step + 1 };
            }
            Phase::Done => {}
        }
    }

    // ---- slide switching ----------------------------------------------------

    fn tick_switch(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        const SWITCHES: u64 = 100;
        match self.phase {
            Phase::Setup => {
                let t = Instant::now();
                let doc = deck_c(&mut app.assets, &self.work_dir);
                self.put("deck_generation_s", json!(t.elapsed().as_secs_f64()));
                self.put("deck", json!({"slides": doc.slides.len(), "objects": doc.object_count(), "distinct_4k_images": app.assets.len()}));
                app.ed = crate::editor::Editor::new(doc, None);
                self.enter(Phase::Warmup { until_idle_since: None });
            }
            Phase::Warmup { .. } => {
                if self.warmed_up(app, ctx) {
                    self.rss.push(Self::rss());
                    self.enter(Phase::Run { step: 0 });
                }
            }
            Phase::Run { step } => {
                ctx.request_repaint();
                // Advance once the current slide's images are loaded (or after 2 s).
                self.wait_frames += 1;
                let ready = app.textures.pending() == 0 && self.wait_frames > 2;
                if ready || self.wait_frames > 120 {
                    self.wait_frames = 0;
                    if step == SWITCHES / 2 {
                        self.rss.push(Self::rss());
                    }
                    if step >= SWITCHES {
                        self.rss.push(Self::rss());
                        let mib: Vec<f64> = self.rss.iter().map(|b| *b as f64 / 1048576.0).collect();
                        self.put("rss_mib_after_warmup", json!(mib[0]));
                        self.put("rss_mib_after_50_switches", json!(mib[1]));
                        self.put("rss_mib_after_100_switches", json!(mib[2]));
                        self.put("switch_wall_s", json!(self.t_phase.elapsed().as_secs_f64()));
                        self.put("texture_cache_mib_end", json!(app.textures.bytes() as f64 / 1048576.0));
                        self.finish(app, ctx);
                        return;
                    }
                    let n = app.ed.doc.slides.len() as u64;
                    app.ed.go_to_slide(((step * 7) % n) as usize);
                    self.phase = Phase::Run { step: step + 1 };
                }
            }
            Phase::Done => {}
        }
    }

    // ---- image import/remove ------------------------------------------------

    fn tick_images(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        const CYCLES: u64 = 100;
        match self.phase {
            Phase::Setup => {
                for i in 0..10 {
                    let p = self.work_dir.join(format!("photo-{i}.jpg"));
                    std::fs::write(&p, jpeg_4k(i as u32)).expect("write test image");
                    self.files.push(p);
                }
                self.enter(Phase::Warmup { until_idle_since: None });
            }
            Phase::Warmup { .. } => {
                if self.warmed_up(app, ctx) {
                    self.rss.push(Self::rss());
                    self.enter(Phase::Run { step: 0 });
                }
            }
            Phase::Run { step } => {
                ctx.request_repaint();
                // Even steps import, odd steps wait for display then delete.
                let cycle = step / 2;
                if cycle >= CYCLES {
                    self.rss.push(Self::rss());
                    let mib: Vec<f64> = self.rss.iter().map(|b| *b as f64 / 1048576.0).collect();
                    self.put("cycles", json!(CYCLES));
                    self.put("rss_mib_before", json!(mib[0]));
                    self.put("rss_mib_after_50", json!(mib.get(1)));
                    self.put("rss_mib_after", json!(mib.last()));
                    self.put(
                        "cycle_ms_p50",
                        json!(crate::perf::percentile(self.cycle_ms.iter().map(|v| *v as f32), 50.0)),
                    );
                    self.put(
                        "cycle_ms_p95",
                        json!(crate::perf::percentile(self.cycle_ms.iter().map(|v| *v as f32), 95.0)),
                    );
                    self.put("objects_left", json!(app.ed.slide().objects.len()));
                    self.finish(app, ctx);
                    return;
                }
                if step % 2 == 0 {
                    let f = self.files[(cycle % self.files.len() as u64) as usize].clone();
                    self.cycle_start = Some(Instant::now());
                    app.import_paths(vec![f], None);
                    self.phase = Phase::Run { step: step + 1 };
                } else if app.is_idle() && !app.ed.slide().objects.is_empty() {
                    self.wait_frames += 1;
                    if self.wait_frames > 2 {
                        self.wait_frames = 0;
                        if let Some(t) = self.cycle_start.take() {
                            self.cycle_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                        app.ed.select_all();
                        app.ed.delete_selection();
                        if cycle == CYCLES / 2 {
                            self.rss.push(Self::rss());
                        }
                        self.phase = Phase::Run { step: step + 1 };
                    }
                }
            }
            Phase::Done => {}
        }
    }

    // ---- smoke ----------------------------------------------------------------

    fn tick_smoke(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        ctx.request_repaint();
        if let Some(report) = self.smoke.tick(app, ctx, &self.work_dir) {
            self.put("steps", report.steps);
            self.put("passed", json!(report.passed));
            self.put("failed", json!(report.failed));
            self.finish(app, ctx);
        }
    }

    // ---- screenshot ---------------------------------------------------------

    fn tick_screenshot(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        match self.phase {
            Phase::Setup => {
                // Optional: BEAMERX_SHOT_SLIDE=<n> shows slide n (1-based).
                if let Some(n) = std::env::var("BEAMERX_SHOT_SLIDE")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                {
                    app.ed.go_to_slide(n.saturating_sub(1));
                }
                self.enter(Phase::Warmup { until_idle_since: None })
            }
            Phase::Warmup { .. } => {
                if self.warmed_up(app, ctx) {
                    let out = self
                        .out
                        .clone()
                        .unwrap_or_else(|| PathBuf::from("beamerx-screenshot.png"));
                    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let flag = std::sync::Arc::clone(&done);
                    let ctx2 = ctx.clone();
                    ctx.request_screenshot(move |img| {
                        let [w, h] = img.size;
                        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                        if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                            let _ = buf.save(&out);
                        }
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        ctx2.request_repaint();
                    });
                    self.done_flag = Some(done);
                    self.enter(Phase::Run { step: 0 });
                }
            }
            Phase::Run { .. } => {
                ctx.request_repaint_after(Duration::from_millis(30));
                if self
                    .done_flag
                    .as_ref()
                    .is_some_and(|f| f.load(std::sync::atomic::Ordering::SeqCst))
                {
                    self.out = None;
                    self.finish(app, ctx);
                }
            }
            Phase::Done => {}
        }
    }

    // ---- save / reopen ------------------------------------------------------

    fn tick_saveload(&mut self, app: &mut BeamerApp, ctx: &egui::Context) {
        const CYCLES: u64 = 10;
        match self.phase {
            Phase::Setup => {
                let doc = deck_b(&mut app.assets);
                app.ed = crate::editor::Editor::new(doc, None);
                self.enter(Phase::Warmup { until_idle_since: None });
            }
            Phase::Warmup { .. } => {
                if self.warmed_up(app, ctx) {
                    self.rss.push(Self::rss());
                    self.enter(Phase::Run { step: 0 });
                }
            }
            Phase::Run { step } => {
                ctx.request_repaint_after(Duration::from_millis(16));
                let path = self.work_dir.join("cycle.beamerx");
                if !app.is_idle() {
                    return;
                }
                if step >= CYCLES * 2 {
                    self.rss.push(Self::rss());
                    let mib: Vec<f64> = self.rss.iter().map(|b| *b as f64 / 1048576.0).collect();
                    self.put("cycles", json!(CYCLES));
                    self.put("rss_mib_before", json!(mib[0]));
                    self.put("rss_mib_after", json!(mib.last()));
                    self.put(
                        "file_mib",
                        json!(
                            std::fs::metadata(&path)
                                .map(|m| m.len() as f64 / 1048576.0)
                                .unwrap_or(0.0)
                        ),
                    );
                    self.put(
                        "save_or_open_ms_p50",
                        json!(crate::perf::percentile(self.cycle_ms.iter().map(|v| *v as f32), 50.0)),
                    );
                    self.put(
                        "save_or_open_ms_max",
                        json!(self.cycle_ms.iter().copied().fold(0.0, f64::max)),
                    );
                    self.put("objects", json!(app.ed.doc.object_count()));
                    self.finish(app, ctx);
                    return;
                }
                if let Some(t) = self.cycle_start.take() {
                    self.cycle_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                self.cycle_start = Some(Instant::now());
                if step % 2 == 0 {
                    app.start_save(path, false);
                } else {
                    app.open_path(Some(path), false);
                }
                self.phase = Phase::Run { step: step + 1 };
            }
            Phase::Done => {}
        }
    }
}

fn app_inject(app: &mut BeamerApp) -> &mut Vec<Event> {
    app.inject_events()
}

// ---------------------------------------------------------------------------
// Representative decks
// ---------------------------------------------------------------------------

const EQUATIONS: [&str; 10] = [
    r"E = mc^2",
    r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
    r"\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}",
    r"\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
    r"\hat{H}\psi = i\hbar \frac{\partial \psi}{\partial t}",
    r"f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & x < 0 \end{cases}",
    r"\mathbf{A} = \begin{bmatrix} a & b \\ c & d \end{bmatrix}",
    r"P(A \mid B) = \frac{P(B \mid A)\,P(A)}{P(B)}",
    r"\lim_{h \to 0} \frac{f(x+h) - f(x)}{h}",
    r"\oint_{\partial \Sigma} \mathbf{B} \cdot d\boldsymbol{\ell} = \mu_0 I",
];

fn png_pattern(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let v = (x.wrapping_mul(7) ^ y.wrapping_mul(13)).wrapping_add(seed * 41);
        image::Rgb([(v & 255) as u8, ((x + seed) & 255) as u8, ((y * 2) & 255) as u8])
    });
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encode png");
    out
}

/// A 3840×2160 JPEG with photo-like detail (~1–3 MB).
pub fn jpeg_4k(seed: u32) -> Vec<u8> {
    let (w, h) = (3840u32, 2160u32);
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let noise = (s & 31) as i32 - 16;
        let r = ((x * 255 / w) as i32 + noise).clamp(0, 255) as u8;
        let g = ((y * 255 / h) as i32 + noise + (seed as i32 * 30)).clamp(0, 255) as u8;
        let b = ((((x / 64) + (y / 64) + seed) % 2) as i32 * 120 + 60 + noise).clamp(0, 255) as u8;
        image::Rgb([r, g, b])
    });
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
    img.write_with_encoder(enc).expect("encode jpeg");
    out
}

fn add_asset(assets: &mut AssetStore, bytes: &[u8], name: &str) -> AssetMeta {
    let meta = AssetStore::ingest_bytes(assets.dir(), bytes, name).expect("ingest bench asset");
    assets.insert(meta.clone());
    meta
}

/// Deck B: 20 slides × 100 editable objects (title, 39 text boxes, 20
/// text boxes with inline math, 30 shapes, 10 images).
pub fn deck_b(assets: &mut AssetStore) -> Document {
    let images: Vec<AssetMeta> = (0..10)
        .map(|i| add_asset(assets, &png_pattern(256, 192, i), &format!("plot-{i}.png")))
        .collect();
    let mut doc = Document::new();
    doc.meta.title = "Deck B".into();
    doc.slides.clear();
    for s in 0..20 {
        let sid = doc.alloc_slide_id();
        let mut objects = Vec::with_capacity(100);
        let mut push = |doc: &mut Document, rect: Rect, kind: ObjectKind| {
            objects.push(Object {
                id: doc.alloc_object_id(),
                rect,
                kind,
            });
        };
        push(
            &mut doc,
            Rect::new(20.0, 12.0, 400.0, 30.0),
            ObjectKind::Text(TextBox {
                text: format!("Slide {} — results", s + 1),
                font_size: 24.0,
                bold: true,
                ..Default::default()
            }),
        );
        for i in 0..39 {
            let (c, r) = (i % 3, i / 3);
            push(
                &mut doc,
                Rect::new(20.0 + c as f32 * 140.0, 50.0 + r as f32 * 15.0, 130.0, 12.0),
                ObjectKind::Text(TextBox {
                    text: format!("Observation {i}: value {}", i * 7 + s),
                    font_size: 9.0,
                    ..Default::default()
                }),
            );
        }
        for i in 0..20 {
            // Text with inline LaTeX math.
            let src = EQUATIONS[(i + s) % EQUATIONS.len()];
            let (c, r) = (i % 4, i / 4);
            push(
                &mut doc,
                Rect::new(10.0 + c as f32 * 110.0, 140.0 + r as f32 * 22.0, 105.0, 14.0),
                ObjectKind::Text(TextBox {
                    text: format!("Eq. {i}: ${src}$"),
                    font_size: 9.0 + (i % 3) as f32,
                    ..Default::default()
                }),
            );
        }
        for i in 0..30 {
            let kinds = [
                ShapeKind::Rectangle,
                ShapeKind::Ellipse,
                ShapeKind::Line,
                ShapeKind::Arrow,
            ];
            let mut sh = Shape::new(kinds[i % 4]);
            let x = 300.0 + (i % 6) as f32 * 25.0;
            let y = 20.0 + (i / 6) as f32 * 25.0;
            let rect = if sh.kind.is_linear() {
                sh.set_endpoints((x, y), (x + 20.0, y + 15.0))
            } else {
                Rect::new(x, y, 20.0, 15.0)
            };
            push(&mut doc, rect, ObjectKind::Shape(sh));
        }
        for (i, m) in images.iter().enumerate() {
            push(
                &mut doc,
                Rect::new(10.0 + i as f32 * 44.0, 215.0, 40.0, 30.0),
                ObjectKind::Image(ImageRef { asset: m.id.clone() }),
            );
        }
        doc.slides.push(Slide {
            id: sid,
            objects,
            background: Color::WHITE,
        });
    }
    doc
}

/// Deck C: 50 slides, each with 2 full-resolution 4K photos (25 distinct
/// images) and a caption.
pub fn deck_c(assets: &mut AssetStore, _work: &Path) -> Document {
    let images: Vec<AssetMeta> = (0..25)
        .map(|i| add_asset(assets, &jpeg_4k(i), &format!("photo-{i}.jpg")))
        .collect();
    let mut doc = Document::new();
    doc.meta.title = "Deck C".into();
    doc.slides.clear();
    for s in 0..50usize {
        let sid = doc.alloc_slide_id();
        let objects = vec![
            Object {
                id: doc.alloc_object_id(),
                rect: Rect::new(10.0, 40.0, 214.0, 120.4),
                kind: ObjectKind::Image(ImageRef {
                    asset: images[(2 * s) % 25].id.clone(),
                }),
            },
            Object {
                id: doc.alloc_object_id(),
                rect: Rect::new(230.0, 40.0, 214.0, 120.4),
                kind: ObjectKind::Image(ImageRef {
                    asset: images[(2 * s + 1) % 25].id.clone(),
                }),
            },
            Object {
                id: doc.alloc_object_id(),
                rect: Rect::new(10.0, 10.0, 400.0, 24.0),
                kind: ObjectKind::Text(TextBox {
                    text: format!("Field photos {}", s + 1),
                    ..Default::default()
                }),
            },
        ];
        doc.slides.push(Slide {
            id: sid,
            objects,
            background: Color::WHITE,
        });
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_b_shape() {
        let mut assets = AssetStore::new_session().unwrap();
        let d = deck_b(&mut assets);
        assert_eq!(d.slides.len(), 20);
        assert!(d.slides.iter().all(|s| s.objects.len() == 100));
        let mut d2 = d.clone();
        d2.validate_and_repair().unwrap();
        assert_eq!(d, d2);
    }

    #[test]
    fn scenario_parsing() {
        assert_eq!("drag".parse::<Scenario>(), Ok(Scenario::Drag));
        assert!("nope".parse::<Scenario>().is_err());
    }
}
