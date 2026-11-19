//! LaTeX math preview service for `$…$` / `$$…$$` inside text boxes.
//!
//! Math is typeset by the real LaTeX engine (the same one used for export)
//! so the editor shows exactly what the exported Beamer will contain. To
//! keep the editor responsive:
//!
//! * requests are **debounced** (a formula must stay unchanged for
//!   [`DEBOUNCE`] before it is compiled, so typing does not launch TeX on
//!   every keystroke);
//! * pending formulas are compiled in **batches** (one TeX run, one page per
//!   formula) on a bounded pool of worker threads;
//! * results are cached in memory and **on disk** (keyed by source, size,
//!   style, preamble and renderer version), so reopening a deck is instant;
//! * stale requests are never dispatched, and results nobody wants any more
//!   are simply dropped.
//!
//! Each result carries the formula's width, height and **depth** (measured
//! by TeX) so inline math sits on the text baseline. The compiled PDF is kept
//! (small, vector); rasterization to the zoom-dependent pixel size happens
//! in the texture cache.

use crate::export::{MATH_PACKAGES, PRE_CLASS, normalize_math};
use crate::jobs::WorkerPool;
use crate::latex::{CompileError, CompileOptions, Toolchain};
use crate::platform::pdf_raster;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Bump when the preview template changes, invalidating disk caches.
pub const RENDER_VERSION: u32 = 2;
pub const DEBOUNCE: Duration = Duration::from_millis(140);
const BATCH_MAX: usize = 24;
const FORGET_UNWANTED_AFTER: Duration = Duration::from_secs(3);
const MAX_ENTRIES: usize = 4096;
/// TeX points per PostScript point.
const PT_PER_BP: f32 = 72.27 / 72.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MathKey(pub [u8; 16]);

impl MathKey {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

pub fn math_key(source: &str, font_size: f32, display: bool, preamble: &str) -> MathKey {
    let mut h = Sha256::new();
    h.update(RENDER_VERSION.to_le_bytes());
    h.update(((font_size * 100.0).round() as u32).to_le_bytes());
    h.update([u8::from(display)]);
    h.update((preamble.len() as u64).to_le_bytes());
    h.update(preamble.as_bytes());
    h.update(source.as_bytes());
    let d = h.finalize();
    let mut k = [0u8; 16];
    k.copy_from_slice(&d[..16]);
    MathKey(k)
}

/// A typeset formula: one page of a (possibly shared) PDF.
#[derive(Debug)]
pub struct MathRender {
    pub key: MathKey,
    pub pdf: Arc<Vec<u8>>,
    pub page: usize,
    /// Size of the box in bp: (width, height + depth).
    pub size_bp: (f32, f32),
    /// Depth below the baseline in bp.
    pub depth_bp: f32,
}

impl MathRender {
    pub fn ascent_bp(&self) -> f32 {
        (self.size_bp.1 - self.depth_bp).max(0.0)
    }
}

pub enum MathStatus {
    Ready(Arc<MathRender>),
    Pending,
    Failed(Arc<str>),
}

#[derive(Clone, Debug)]
struct Req {
    key: MathKey,
    src: String,
    size: f32,
    display: bool,
}

enum State {
    Waiting { since: Instant, req: Req },
    Queued,
    Ready(Arc<MathRender>),
    Failed(Arc<str>),
}

struct Entry {
    state: State,
    last_wanted: Instant,
}

type BatchResult = Vec<(MathKey, Result<Arc<MathRender>, String>)>;
/// Parsed batch PDFs (bytes + page sizes) by cache file name.
type PdfMemo = HashMap<String, (Arc<Vec<u8>>, Vec<(f32, f32)>)>;

pub struct MathService {
    entries: HashMap<MathKey, Entry>,
    preamble: String,
    toolchain: Option<Toolchain>,
    cache_dir: PathBuf,
    pool: WorkerPool,
    tx: Sender<BatchResult>,
    rx: Receiver<BatchResult>,
    waker: Arc<dyn Fn() + Send + Sync>,
    in_flight: usize,
    compiled_total: usize,
}

pub const NO_ENGINE: &str =
    "No LaTeX engine found. Install Tectonic or a TeX Live/MacTeX distribution, then restart BeamerX.";

impl MathService {
    pub fn new(toolchain: Option<Toolchain>, cache_dir: PathBuf, waker: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        let pool = WorkerPool::new("latex", 2);
        let dir = cache_dir.clone();
        pool.spawn(move || prune_cache(&dir, 256 << 20));
        Self {
            entries: HashMap::new(),
            preamble: String::new(),
            toolchain,
            cache_dir,
            pool,
            tx,
            rx,
            waker,
            in_flight: 0,
            compiled_total: 0,
        }
    }

    pub fn toolchain(&self) -> Option<&Toolchain> {
        self.toolchain.as_ref()
    }

    /// Set the document preamble used for previews (changes all keys).
    pub fn set_preamble(&mut self, preamble: &str) {
        if self.preamble != preamble {
            self.preamble = preamble.to_string();
        }
    }

    pub fn key_for(&self, source: &str, font_size: f32, display: bool) -> MathKey {
        math_key(source, font_size, display, &self.preamble)
    }

    /// Ask for a formula. Cheap; call every frame for visible math.
    pub fn request(&mut self, source: &str, font_size: f32, display: bool, now: Instant) -> MathStatus {
        let key = self.key_for(source, font_size, display);
        let e = self.entries.entry(key).or_insert_with(|| Entry {
            state: State::Waiting {
                since: now,
                req: Req {
                    key,
                    src: source.to_string(),
                    size: font_size,
                    display,
                },
            },
            last_wanted: now,
        });
        e.last_wanted = now;
        match &e.state {
            State::Ready(r) => MathStatus::Ready(Arc::clone(r)),
            State::Failed(m) => MathStatus::Failed(Arc::clone(m)),
            _ => MathStatus::Pending,
        }
    }

    /// Peek without registering interest.
    pub fn peek(&self, source: &str, font_size: f32, display: bool) -> Option<Arc<MathRender>> {
        match self
            .entries
            .get(&self.key_for(source, font_size, display))
            .map(|e| &e.state)
        {
            Some(State::Ready(r)) => Some(Arc::clone(r)),
            _ => None,
        }
    }

    /// Drop all failures so they are retried (e.g. after a network error).
    pub fn retry_failed(&mut self) {
        self.entries.retain(|_, e| !matches!(e.state, State::Failed(_)));
    }

    pub fn busy(&self) -> usize {
        self.in_flight
    }

    pub fn compiled_total(&self) -> usize {
        self.compiled_total
    }

    /// Collect finished work and dispatch due requests. Returns `true` if
    /// any result arrived and the time until the next debounce deadline.
    pub fn pump(&mut self, now: Instant) -> (bool, Option<Duration>) {
        let mut changed = false;
        while let Ok(batch) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            for (key, res) in batch {
                changed = true;
                self.compiled_total += 1;
                if let Some(e) = self.entries.get_mut(&key) {
                    e.state = match res {
                        Ok(r) => State::Ready(r),
                        Err(m) => State::Failed(m.into()),
                    };
                }
            }
        }

        let mut due: Vec<Req> = Vec::new();
        let mut next: Option<Duration> = None;
        self.entries.retain(|_, e| {
            if let State::Waiting { since, req } = &e.state {
                if now.duration_since(e.last_wanted) > FORGET_UNWANTED_AFTER {
                    return false;
                }
                let age = now.duration_since(*since);
                if age >= DEBOUNCE {
                    // Only compile what is still being asked for.
                    if now.duration_since(e.last_wanted) < Duration::from_millis(1000) {
                        due.push(req.clone());
                    }
                } else {
                    let left = DEBOUNCE - age;
                    next = Some(next.map_or(left, |n: Duration| n.min(left)));
                }
            }
            true
        });

        if !due.is_empty() {
            for r in &due {
                if let Some(e) = self.entries.get_mut(&r.key) {
                    e.state = State::Queued;
                }
            }
            match &self.toolchain {
                None => {
                    for r in due {
                        if let Some(e) = self.entries.get_mut(&r.key) {
                            e.state = State::Failed(NO_ENGINE.into());
                        }
                    }
                    changed = true;
                }
                Some(tc) => {
                    for chunk in due.chunks(BATCH_MAX) {
                        let items = chunk.to_vec();
                        let tc = tc.clone();
                        let preamble = self.preamble.clone();
                        let dir = self.cache_dir.clone();
                        let tx = self.tx.clone();
                        let waker = Arc::clone(&self.waker);
                        self.in_flight += 1;
                        self.pool.spawn(move || {
                            let out = compile_batch(&items, &preamble, &tc, &dir);
                            let _ = tx.send(out);
                            waker();
                        });
                    }
                }
            }
        }

        if self.entries.len() > MAX_ENTRIES {
            let mut by_age: Vec<(Instant, MathKey)> = self
                .entries
                .iter()
                .filter(|(_, e)| matches!(e.state, State::Ready(_) | State::Failed(_)))
                .map(|(k, e)| (e.last_wanted, *k))
                .collect();
            by_age.sort_by_key(|(t, _)| *t);
            let drop_n = self.entries.len() - MAX_ENTRIES / 2;
            for (_, k) in by_age.into_iter().take(drop_n) {
                self.entries.remove(&k);
            }
        }
        (changed, next)
    }
}

/// Lines of the user preamble that only make sense in Beamer (themes,
/// templates) are skipped for previews, which use the `article` class.
pub fn preview_preamble(preamble: &str) -> String {
    const BEAMER_ONLY: [&str; 12] = [
        "\\usetheme",
        "\\usecolortheme",
        "\\usefonttheme",
        "\\useinnertheme",
        "\\useoutertheme",
        "\\setbeamer",
        "\\addtobeamertemplate",
        "\\defbeamertemplate",
        "\\AtBeginSection",
        "\\AtBeginSubsection",
        "\\logo",
        "\\titlegraphic",
    ];
    preamble
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !BEAMER_ONLY.iter().any(|p| t.starts_with(p))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The math as it appears in the exported text: `$…$`, display style for
/// `$$…$$` blocks.
pub fn math_mode(src: &str, display: bool) -> String {
    if display {
        format!("$\\displaystyle {}$", normalize_math(src))
    } else {
        format!("${}$", normalize_math(src))
    }
}

fn batch_tex(items: &[Req], preamble: &str) -> String {
    let mut t = String::new();
    t.push_str(PRE_CLASS);
    t.push_str("\\documentclass{article}\n");
    t.push_str(MATH_PACKAGES);
    let pre = preview_preamble(preamble);
    if !pre.trim().is_empty() {
        t.push_str(&pre);
        t.push('\n');
    }
    t.push_str("\\usepackage[active,tightpage]{preview}\n\\setlength\\PreviewBorder{0pt}\n\\pagestyle{empty}\n");
    t.push_str("\\begin{document}\n");
    for r in items {
        // Measure the box so the editor knows the baseline (depth).
        t.push_str(&format!(
            "\\begin{{preview}}\\fontsize{{{s}bp}}{{{l}bp}}\\selectfont\\sffamily\\setbox0=\\hbox{{{m}}}\\typeout{{BXDIM \\the\\ht0 \\the\\dp0 \\the\\wd0}}\\box0\\end{{preview}}\n",
            s = r.size,
            l = r.size * crate::document::typeset::LINE_SPREAD,
            m = math_mode(&r.src, r.display),
        ));
    }
    t.push_str("\\end{document}\n");
    t
}

/// Depths (bp) of each measured box, from `BXDIM <ht>pt <dp>pt <wd>pt`.
fn parse_depths(log: &str) -> Vec<f32> {
    log.lines()
        .filter_map(|l| l.trim().strip_prefix("BXDIM "))
        .map(|rest| {
            let parts: Vec<f32> = rest.split("pt").filter_map(|p| p.trim().parse::<f32>().ok()).collect();
            parts.get(1).copied().unwrap_or(0.0) / PT_PER_BP
        })
        .collect()
}

fn idx_path(dir: &Path, key: &MathKey) -> PathBuf {
    dir.join(format!("{}.idx", key.hex()))
}

fn from_disk(dir: &Path, key: &MathKey, pdfs: &mut PdfMemo) -> Option<Arc<MathRender>> {
    let idx = fs::read_to_string(idx_path(dir, key)).ok()?;
    let mut parts = idx.split_whitespace();
    let name = parts.next()?.to_string();
    let page: usize = parts.next()?.parse().ok()?;
    let depth_bp: f32 = parts.next()?.parse().ok()?;
    if !name.starts_with("b-") || !name.ends_with(".pdf") || name.contains('/') {
        return None;
    }
    if !pdfs.contains_key(&name) {
        let bytes = fs::read(dir.join(&name)).ok()?;
        let sizes = pdf_raster::page_sizes(&bytes).ok()?;
        pdfs.insert(name.clone(), (Arc::new(bytes), sizes));
    }
    let (pdf, sizes) = pdfs.get(&name)?;
    let size_bp = *sizes.get(page)?;
    Some(Arc::new(MathRender {
        key: *key,
        pdf: Arc::clone(pdf),
        page,
        size_bp,
        depth_bp,
    }))
}

/// Typeset formulas synchronously without the disk cache (tests, tools).
/// Returns each formula's (width, height+depth, depth) in bp, or the error.
pub fn typeset_blocking(
    items: &[(&str, f32, bool)],
    preamble: &str,
    tc: &Toolchain,
) -> Vec<Result<(f32, f32, f32), String>> {
    let reqs: Vec<Req> = items
        .iter()
        .map(|(s, f, d)| Req {
            key: math_key(s, *f, *d, preamble),
            src: s.to_string(),
            size: *f,
            display: *d,
        })
        .collect();
    let dir = std::env::temp_dir().join(format!("BeamerX/typeset-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let out = compile_uncached(&reqs, preamble, tc, &dir);
    let _ = fs::remove_dir_all(&dir);
    reqs.iter()
        .map(|r| match out.iter().find(|(k, _)| *k == r.key) {
            Some((_, Ok(m))) => Ok((m.size_bp.0, m.size_bp.1, m.depth_bp)),
            Some((_, Err(e))) => Err(e.clone()),
            None => Err("missing result".into()),
        })
        .collect()
}

/// Compile a batch (runs on a worker thread).
fn compile_batch(items: &[Req], preamble: &str, tc: &Toolchain, cache_dir: &Path) -> BatchResult {
    let _span = tracing::debug_span!("math_batch", n = items.len()).entered();
    let started = Instant::now();
    let math_dir = cache_dir.join("math");
    let _ = fs::create_dir_all(&math_dir);
    let mut out = Vec::with_capacity(items.len());
    let mut pdfs = HashMap::new();
    let mut todo = Vec::new();
    for it in items {
        match from_disk(&math_dir, &it.key, &mut pdfs) {
            Some(r) => out.push((it.key, Ok(r))),
            None => todo.push(it.clone()),
        }
    }
    if !todo.is_empty() {
        out.extend(compile_uncached(&todo, preamble, tc, &math_dir));
    }
    tracing::debug!(
        ms = started.elapsed().as_millis() as u64,
        n = items.len(),
        "math batch done"
    );
    out
}

fn compile_uncached(items: &[Req], preamble: &str, tc: &Toolchain, math_dir: &Path) -> BatchResult {
    // Reject structurally broken input early with a clear message.
    let mut ok_items = Vec::new();
    let mut out = Vec::new();
    for it in items {
        match crate::export::math_problem(&it.src) {
            Some(why) => out.push((it.key, Err(why.to_string()))),
            None => ok_items.push(it.clone()),
        }
    }
    if ok_items.is_empty() {
        return out;
    }
    match compile_pages(&ok_items, preamble, tc) {
        Ok((pdf, sizes, depths)) if sizes.len() == ok_items.len() => {
            let name = format!("b-{}.pdf", ok_items[0].key.hex());
            let pdf = Arc::new(pdf);
            let depth = |i: usize| depths.get(i).copied().unwrap_or(0.0);
            if crate::assets::write_atomic(&math_dir.join(&name), &pdf).is_ok() {
                for (page, it) in ok_items.iter().enumerate() {
                    let _ = crate::assets::write_atomic(
                        &idx_path(math_dir, &it.key),
                        format!("{name} {page} {}\n", depth(page)).as_bytes(),
                    );
                }
            }
            for (page, it) in ok_items.iter().enumerate() {
                let r = MathRender {
                    key: it.key,
                    pdf: Arc::clone(&pdf),
                    page,
                    size_bp: sizes[page],
                    depth_bp: depth(page),
                };
                out.push((it.key, Ok(Arc::new(r))));
            }
        }
        Ok(_) | Err(_) if ok_items.len() > 1 => {
            // Isolate the failing formula(s).
            for it in &ok_items {
                out.extend(compile_uncached(std::slice::from_ref(it), preamble, tc, math_dir));
            }
        }
        Ok(_) => out.push((
            ok_items[0].key,
            Err("unexpected LaTeX output (page count mismatch)".into()),
        )),
        Err(e) => out.push((ok_items[0].key, Err(e))),
    }
    out
}

/// A compiled PDF, its page sizes and the depth of each formula (bp).
type Compiled = (Vec<u8>, Vec<(f32, f32)>, Vec<f32>);

fn compile_pages(items: &[Req], preamble: &str, tc: &Toolchain) -> Result<Compiled, String> {
    let work = std::env::temp_dir().join(format!("BeamerX/math-{}-{}", std::process::id(), items[0].key.hex()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let res = (|| {
        fs::write(work.join("eq.tex"), batch_tex(items, preamble)).map_err(|e| e.to_string())?;
        let never = AtomicBool::new(false);
        let pdf_path = tc
            .compile(&work, "eq.tex", &CompileOptions::preview(), &never)
            .map_err(|e| match e.downcast_ref::<CompileError>() {
                Some(ce) => clean_error(&ce.log_excerpt),
                None => format!("{e:#}"),
            })?;
        let pdf = fs::read(pdf_path).map_err(|e| e.to_string())?;
        let sizes = pdf_raster::page_sizes(&pdf).map_err(|e| format!("{e:#}"))?;
        let depths = parse_depths(&fs::read_to_string(work.join("eq.log")).unwrap_or_default());
        Ok((pdf, sizes, depths))
    })();
    let _ = fs::remove_dir_all(&work);
    res
}

/// Make TeX errors readable: drop file/line prefixes that refer to the
/// temporary preview document.
fn clean_error(s: &str) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|l| {
            let l = l.trim();
            match l.split_once(": ") {
                Some((pre, rest)) if pre.contains("eq.tex") => rest.to_string(),
                _ => l.to_string(),
            }
        })
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        "LaTeX error".into()
    } else {
        lines.join("\n")
    }
}

/// Keep the on-disk preview cache below `max_bytes` (oldest files go first).
fn prune_cache(cache_dir: &Path, max_bytes: u64) {
    let dir = cache_dir.join("math");
    let Ok(rd) = fs::read_dir(&dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max_bytes {
        return;
    }
    files.sort();
    let mut excess = total - max_bytes / 2;
    for (_, len, p) in files {
        if excess == 0 {
            break;
        }
        let _ = fs::remove_file(p);
        excess = excess.saturating_sub(len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_depend_on_all_inputs() {
        let a = math_key("x", 20.0, false, "");
        assert_eq!(a, math_key("x", 20.0, false, ""));
        assert_ne!(a, math_key("y", 20.0, false, ""));
        assert_ne!(a, math_key("x", 21.0, false, ""));
        assert_ne!(a, math_key("x", 20.0, true, ""));
        assert_ne!(a, math_key("x", 20.0, false, "\\newcommand{\\R}{x}"));
        // Length-prefixing prevents ambiguity between preamble and source.
        assert_ne!(math_key("bc", 20.0, false, "a"), math_key("c", 20.0, false, "ab"));
    }

    #[test]
    fn preview_preamble_drops_beamer_only_lines() {
        let p = "\\usetheme{Madrid}\n\\newcommand{\\R}{\\mathbb{R}}\n  \\setbeamercolor{x}{fg=red}\n\\usepackage{bm}";
        assert_eq!(preview_preamble(p), "\\newcommand{\\R}{\\mathbb{R}}\n\\usepackage{bm}");
    }

    #[test]
    fn depths_parse_from_log() {
        let log = "junk\nBXDIM 15.03816pt5.00195pt37.22072pt\nmore\nBXDIM 10.0pt 0.0pt 5.0pt\n";
        let d = parse_depths(log);
        assert_eq!(d.len(), 2);
        assert!((d[0] - 5.00195 / PT_PER_BP).abs() < 1e-4);
        assert_eq!(d[1], 0.0);
    }

    #[test]
    fn debounce_then_dispatch_without_engine() {
        let dir = std::env::temp_dir().join("BeamerX/test-math-cache");
        let mut m = MathService::new(None, dir, Arc::new(|| {}));
        let t0 = Instant::now();
        assert!(matches!(m.request("x^2", 20.0, false, t0), MathStatus::Pending));
        let (_, next) = m.pump(t0);
        assert!(next.is_some(), "should wait for debounce");
        let t1 = t0 + DEBOUNCE + Duration::from_millis(1);
        m.request("x^2", 20.0, false, t1);
        m.pump(t1);
        match m.request("x^2", 20.0, false, t1) {
            MathStatus::Failed(msg) => assert!(msg.contains("No LaTeX engine")),
            _ => panic!("expected failure without engine"),
        }
    }

    #[test]
    fn abandoned_keystrokes_are_never_compiled() {
        let dir = std::env::temp_dir().join("BeamerX/test-math-cache");
        let mut m = MathService::new(None, dir, Arc::new(|| {}));
        let t0 = Instant::now();
        m.request("x", 20.0, false, t0);
        // User keeps typing; "x" is no longer requested.
        let t1 = t0 + Duration::from_secs(5);
        m.request("x+1", 20.0, false, t1);
        m.pump(t1);
        assert!(m.peek("x", 20.0, false).is_none());
        assert!(!m.entries.contains_key(&m.key_for("x", 20.0, false)));
    }
}
