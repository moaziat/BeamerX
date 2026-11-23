//! Scripted product demo (`--bench demo --out DIR REFERENCE.beamerx`),
//! recorded by the app itself.
//!
//! The demo opens a reference deck, starts a blank presentation and rebuilds
//! every slide through the real UI, the way a person would: double-click on
//! empty space and type each text box (LaTeX between `$…$` renders as soon as
//! the dollar closes), drag boxes into place, drop each picture and move and
//! resize it with the mouse, draw shapes with the shape tool, select bold
//! runs and click **B**. Pointer and keyboard input is injected into egui
//! like in the smoke test; buttons are found through
//! [`crate::ui::targets`]. Font sizes, box widths and colours that have no
//! one-click control are applied by the script when a box is created.
//! Finally it presents the slides and exports a PDF, and compares the result
//! with the reference.
//!
//! A cursor and captions are painted on top, and the window's own frames are
//! captured (the screen is never recorded): each frame is read back after
//! painting, JPEG-encoded on worker threads and listed with its duration in
//! `DIR/frames.txt` (`file<TAB>seconds`). `scripts/make-demo-video.sh` turns
//! the frames into an MP4.

use crate::app::BeamerApp;
use crate::document::{Document, ObjectId, ObjectKind, Rect, SLIDE_H, SLIDE_W, ShapeKind, TextBox, typeset};
use egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Capture rate of the recording.
const FPS: f64 = 30.0;
const WORKERS: usize = 6;
const JPEG_QUALITY: u8 = 90;
/// Typing speed (characters per second).
const CPS_TEXT: f32 = 60.0;
const CPS_MATH: f32 = 25.0;
/// Pace of the whole demo: every pause and pointer move takes this fraction
/// of its scripted duration.
const TEMPO: f32 = 0.6;
/// The opening title card (seconds).
const INTRO: f32 = 3.2 * TEMPO;

/// State shared by the acts while the demo runs.
#[derive(Default)]
struct Shared {
    out: PathBuf,
    /// The object being built.
    current: Option<ObjectId>,
    /// Where the next text box is created (slide bp).
    spot: (f32, f32),
    /// Objects on the slide before the current one was created.
    count: usize,
    /// A source file shown over the window (title, text, shown since, seconds).
    code: Option<(String, String, Instant, f32)>,
}

type Find = Rc<dyn Fn(&BeamerApp, &Shared) -> Option<(f32, f32)>>;
type Action = Rc<dyn Fn(&mut BeamerApp, &mut Shared)>;
type Check = Rc<dyn Fn(&BeamerApp, &Shared) -> bool>;

/// Where the pointer should go.
#[derive(Clone)]
enum Target {
    /// A control recorded with [`crate::ui::targets::mark`].
    Named(String),
    /// A point on the slide computed when the move starts and while it runs.
    Find(Find),
    /// A point of the window, as fractions of its width and height.
    Screen(f32, f32),
}

#[derive(Clone)]
enum Act {
    /// A step title: (tag, text). The tag picks the colour.
    Caption(&'static str, String),
    NoCaption,
    Wait(f32),
    /// Glide the pointer to a target over the given seconds.
    Move(Target, f32),
    Click,
    DoubleClick,
    Press,
    Release,
    /// Type text at a natural speed.
    Type(String, f32),
    Keys(Key, Modifiers),
    Do(Action),
    /// Until the condition holds (at most 20 s).
    Until(Check),
    /// Skip the next `n` acts when the condition holds.
    SkipIf(Check, usize),
    /// Until background work (LaTeX, import, export) is done.
    Idle,
}

fn named(s: &str) -> Target {
    Target::Named(s.to_owned())
}

fn at(x: f32, y: f32) -> Target {
    Target::Find(Rc::new(move |_, _| Some((x, y))))
}

fn find(f: impl Fn(&BeamerApp, &Shared) -> Option<(f32, f32)> + 'static) -> Target {
    Target::Find(Rc::new(f))
}

fn act(f: impl Fn(&mut BeamerApp, &mut Shared) + 'static) -> Act {
    Act::Do(Rc::new(f))
}

fn until(f: impl Fn(&BeamerApp, &Shared) -> bool + 'static) -> Act {
    Act::Until(Rc::new(f))
}

fn step(tag: &'static str, s: &str) -> Act {
    Act::Caption(tag, s.to_owned())
}

/// Colour of a step title by its tag.
fn tag_color(tag: &str) -> Color32 {
    match tag {
        "TEXT" => Color32::from_rgb(37, 99, 235),
        "EQUATION" => Color32::from_rgb(124, 58, 237),
        "PICTURE" => Color32::from_rgb(5, 150, 105),
        "SHAPE" => Color32::from_rgb(234, 88, 12),
        "FORMAT" => Color32::from_rgb(219, 39, 119),
        "MOVE" => Color32::from_rgb(8, 145, 178),
        "SLIDE" => Color32::from_rgb(79, 70, 229),
        "PRESENT" => Color32::from_rgb(220, 38, 38),
        "SAVE" => Color32::from_rgb(196, 62, 28),
        "LATEX" => Color32::from_rgb(15, 118, 110),
        "COLOR" => Color32::from_rgb(217, 119, 6),
        _ => Color32::from_rgb(55, 65, 81),
    }
}

/// Current rect of the object being built.
fn current_rect(app: &BeamerApp, sh: &Shared) -> Option<Rect> {
    sh.current.and_then(|id| app.ed.object(id)).map(|o| o.rect)
}

/// Is a box of size `w`×`h` at `(x, y)` clear of every object?
fn is_clear(app: &BeamerApp, x: f32, y: f32, w: f32, h: f32) -> bool {
    // A small margin keeps the double-click off neighbouring boxes.
    let probe = Rect::new(x - 3.0, y - 3.0, w + 6.0, h + 6.0);
    x + w <= SLIDE_W && y + h <= SLIDE_H && app.ed.slide().objects.iter().all(|o| !o.rect.intersects(&probe))
}

/// A clear spot for a new box (top-left), searching from the bottom left.
fn clear_spot(app: &BeamerApp, w: f32, h: f32) -> (f32, f32) {
    let w = w.min(SLIDE_W * 0.45);
    let mut y = SLIDE_H - h - 6.0;
    while y > 30.0 {
        let mut x = 12.0;
        while x + w < SLIDE_W {
            if is_clear(app, x, y, w, h) {
                return (x, y);
            }
            x += 10.0;
        }
        y -= 8.0;
    }
    (12.0, SLIDE_H - h - 6.0)
}

/// The colour picker's swatch target for `c`, if the picker offers it as a
/// one-click colour (black, the default, needs no step).
fn picker_swatch(c: crate::document::Color) -> Option<String> {
    use crate::ui::color_picker::{STANDARD, THEME};
    let offered = THEME.iter().chain(STANDARD.iter()).any(|x| *x == c);
    (offered && c != crate::document::Color::rgb(0, 0, 0)).then(|| format!("swatch:{}", c.to_hex().to_uppercase()))
}

/// Maximal runs of characters that the reference marks bold explicitly.
fn bold_runs(t: &TextBox) -> Vec<(usize, usize)> {
    let fm = t.char_formats();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < fm.len() {
        if fm[i].bold == Some(true) {
            let a = i;
            while i < fm.len() && fm[i].bold == Some(true) {
                i += 1;
            }
            runs.push((a, i));
        } else {
            i += 1;
        }
    }
    runs
}

/// Acts that rebuild one text box at `r`.
fn text_acts(r: Rect, t: &TextBox, first_math: &mut bool) -> Vec<Act> {
    let mut v = Vec::new();
    let math = crate::rich::has_math(&t.text);
    v.push(if math {
        step("EQUATION", "Type LaTeX between $…$: it renders as soon as the $ closes")
    } else {
        step("TEXT", "Double-click empty space, then type right on the slide")
    });
    let fs0 = TextBox::default().font_size;
    // Double-click so that the box's top-left lands on the original spot,
    // or on a clear spot when something is in the way (then drag it).
    let click_dy = fs0 * typeset::FIRST_BASELINE * 0.7;
    let est_h = t.max_size() * typeset::LINE_SPREAD * 1.2;
    let (rx, ry, rw) = (r.x, r.y, r.w);
    // Let pending LaTeX finish first: rendered formulas make their boxes
    // taller, and the spot must be clear of the final boxes.
    v.push(until(|app, _| app.is_idle()));
    v.push(act(move |app, sh| {
        sh.count = app.ed.slide().objects.len();
        sh.spot = if is_clear(app, rx, ry, rw.min(120.0), est_h) {
            (rx, ry)
        } else {
            clear_spot(app, rw, est_h)
        };
        app.ed.set_tool(crate::editor::Tool::Select);
    }));
    v.push(Act::Move(
        find(move |_, sh| Some((sh.spot.0 + 2.0, sh.spot.1 + click_dy))),
        0.55,
    ));
    v.push(Act::DoubleClick);
    v.push(until(|app, _| app.ed.editing_text.is_some()));
    // The double-click must have created a new box; if it opened an existing
    // one instead, leave it untouched and start a new box on a clear spot.
    v.push(act(move |app, sh| {
        let objects = &app.ed.slide().objects;
        let fresh = objects.len() == sh.count + 1 && objects.last().map(|o| o.id) == app.ed.editing_text;
        if fresh {
            return;
        }
        crate::editor::canvas::finish_text_edit(&mut app.ed, &mut app.canvas);
        sh.spot = clear_spot(app, rw, est_h);
        let t = TextBox::default();
        let rect = Rect::new(sh.spot.0, sh.spot.1, rw, t.font_size * typeset::LINE_SPREAD);
        let id = app.ed.insert_object(ObjectKind::Text(t), rect);
        crate::editor::canvas::begin_text_edit(&mut app.ed, &mut app.canvas, id, crate::editor::text_edit::Caret::End);
        app.ed.history.seal();
    }));
    v.push(until(|app, _| app.ed.editing_text.is_some()));
    // A colour the picker offers is applied on camera after typing;
    // others (like the title's) are part of the box style.
    let swatch = picker_swatch(t.color);
    let preset_color = swatch.is_none();
    // Style of the new box (size, colour, alignment, width, paragraph style).
    let style = t.clone();
    v.push(act(move |app, sh| {
        let Some(id) = app.ed.editing_text else { return };
        sh.current = Some(id);
        let s = style.clone();
        app.ed.update_selected_text(90, |x| {
            x.font_size = s.font_size;
            if preset_color {
                x.color = s.color;
            }
            x.align = s.align;
            x.bold = s.bold;
            x.italic = s.italic;
        });
        let para = s.para(0);
        crate::editor::text_edit::update_paras(&mut app.ed, &mut app.canvas.rich, |p| *p = para);
        if let Some(o) = app.ed.object(id).cloned() {
            let rect = Rect::new(sh.spot.0, sh.spot.1, rw, o.rect.h);
            app.ed.set_object(id, o.kind, rect, None);
        }
    }));
    v.push(Act::Type(t.text.clone(), if math { CPS_MATH } else { CPS_TEXT }));
    if math && *first_math {
        // Show the first equation rendering; later ones render while the
        // demo moves on.
        v.push(Act::Idle);
        *first_math = false;
    }
    v.push(Act::Wait(0.35));
    if let Some(swatch) = swatch {
        v.push(step(
            "COLOR",
            if math {
                "Colour the equation: select it, then pick a colour"
            } else {
                "Colour the text: select it, then pick a colour"
            },
        ));
        let n = t.char_len();
        v.push(act(move |app, _| {
            if let Some(r) = app.canvas.rich.as_mut() {
                r.anchor = 0;
                r.caret = n;
            }
        }));
        v.push(Act::Wait(0.5));
        v.push(Act::Move(named("Font color"), 0.7));
        v.push(Act::Click);
        v.push(Act::Wait(0.9));
        v.push(Act::Move(Target::Named(swatch), 0.7));
        v.push(Act::Wait(0.2));
        v.push(Act::Click);
        v.push(Act::Wait(0.6));
        // Deselect to show the coloured result.
        v.push(act(move |app, _| {
            if let Some(r) = app.canvas.rich.as_mut() {
                r.anchor = n;
                r.caret = n;
            }
        }));
        v.push(Act::Wait(0.8));
    }
    let runs = bold_runs(t);
    if !runs.is_empty() {
        v.push(step("FORMAT", "Select characters and click B to make them bold"));
        for (a, b) in runs {
            v.push(act(move |app, _| {
                if let Some(r) = app.canvas.rich.as_mut() {
                    r.anchor = a;
                    r.caret = b;
                }
            }));
            v.push(Act::Wait(0.5));
            v.push(Act::Move(named("B"), 0.6));
            v.push(Act::Click);
            v.push(Act::Wait(0.5));
        }
    }
    v.push(Act::Keys(Key::Escape, Modifiers::NONE));
    v.push(Act::Wait(0.25));
    // Drag it into place when it was typed elsewhere.
    v.push(Act::SkipIf(
        Rc::new(move |app, sh| current_rect(app, sh).is_none_or(|c| (c.x - rx).abs() + (c.y - ry).abs() < 2.0)),
        5,
    ));
    v.push(step("MOVE", "Drag the text box into place"));
    v.push(Act::Move(
        find(|app, sh| current_rect(app, sh).map(|c| (c.x + 6.0, c.y + c.h * 0.5))),
        0.45,
    ));
    v.push(Act::Press);
    v.push(Act::Move(
        find(move |app, sh| current_rect(app, sh).map(|c| (rx + 6.0, ry + c.h * 0.5))),
        0.9,
    ));
    v.push(Act::Release);
    // Exact final geometry and formats.
    let reference = t.clone();
    v.push(act(move |app, sh| {
        let Some(id) = sh.current else { return };
        if let Some(o) = app.ed.object(id).cloned() {
            let rect = Rect::new(rx, ry, rw, o.rect.h);
            app.ed.set_object(id, ObjectKind::Text(reference.clone()), rect, None);
        }
        app.ed.selection.clear();
    }));
    v.push(Act::Wait(0.3));
    v
}

/// Acts that drop a picture, drag it into place and resize it to `r`.
fn image_acts(r: Rect, file: PathBuf) -> Vec<Act> {
    let mut v = vec![step("PICTURE", "Drop a picture on the slide, then move and resize it")];
    v.push(act(move |app, sh| {
        sh.current = None;
        app.import_paths(vec![file.clone()], Some((SLIDE_W * 0.42, SLIDE_H * 0.55)));
    }));
    v.push(Act::Idle);
    v.push(act(|app, sh| {
        sh.current = app
            .ed
            .slide()
            .objects
            .iter()
            .rev()
            .find(|o| matches!(o.kind, ObjectKind::Image(_)))
            .map(|o| o.id);
        if let Some(id) = sh.current {
            app.ed.selection = vec![id];
        }
    }));
    v.push(Act::Wait(0.8));
    // Move: grab the middle, drop it where its top-left lands on the target.
    v.push(Act::Move(
        find(|app, sh| current_rect(app, sh).map(|c| (c.x + c.w * 0.5, c.y + c.h * 0.5))),
        0.6,
    ));
    v.push(Act::Press);
    v.push(Act::Move(
        find(move |app, sh| current_rect(app, sh).map(|c| (r.x + c.w * 0.5, r.y + c.h * 0.5))),
        1.1,
    ));
    v.push(Act::Release);
    v.push(Act::Wait(0.5));
    // Resize from the bottom-right corner (a little too big, then right).
    v.push(Act::Move(
        find(|app, sh| current_rect(app, sh).map(|c| (c.right(), c.bottom()))),
        0.6,
    ));
    v.push(Act::Press);
    v.push(Act::Move(at(r.x + r.w * 0.82, r.y + r.h * 0.82), 0.7));
    v.push(Act::Move(at(r.right(), r.bottom()), 0.7));
    v.push(Act::Release);
    v.push(act(move |app, sh| {
        if let Some(o) = sh.current.and_then(|id| app.ed.object(id)).cloned() {
            app.ed.set_object(o.id, o.kind, r, None);
        }
    }));
    v.push(Act::Wait(0.6));
    v
}

fn gallery_label(k: ShapeKind) -> &'static str {
    match k {
        ShapeKind::Rectangle => "Rectangle",
        ShapeKind::RoundedRectangle => "Rounded rectangle",
        ShapeKind::Ellipse => "Oval",
        ShapeKind::Triangle => "Triangle",
        ShapeKind::Diamond => "Diamond",
        ShapeKind::Cube => "Cube",
        ShapeKind::Cylinder => "Cylinder",
        ShapeKind::Bevel => "Bevel",
        ShapeKind::BlockArrow => "Block arrow",
        ShapeKind::Line => "Line",
        ShapeKind::Arrow => "Arrow",
        ShapeKind::Freehand => "Drawing",
    }
}

/// Acts that draw shape `s` over `r` with the shape tool.
fn shape_acts(r: Rect, s: &crate::document::Shape) -> Vec<Act> {
    let mut v = vec![step(
        "SHAPE",
        "Pick a shape in the gallery and drag it out on the slide",
    )];
    v.push(Act::Move(named("Shapes"), 0.7));
    v.push(Act::Click);
    v.push(Act::Wait(0.5));
    v.push(Act::Move(named(gallery_label(s.kind)), 0.5));
    v.push(Act::Click);
    v.push(Act::Wait(0.2));
    let n = Rc::new(std::cell::Cell::new(0usize));
    let n1 = Rc::clone(&n);
    v.push(act(move |app, _| n1.set(app.ed.slide().objects.len())));
    let (a, b) = if s.kind.is_linear() {
        s.endpoints(&r)
    } else {
        ((r.x, r.y), (r.right(), r.bottom()))
    };
    v.push(Act::Move(at(a.0, a.1), 0.5));
    v.push(Act::Press);
    v.push(Act::Move(at(b.0, b.1), 0.8));
    v.push(Act::Release);
    let n2 = Rc::clone(&n);
    v.push(until(move |app, _| app.ed.slide().objects.len() > n2.get()));
    let shape = s.clone();
    v.push(act(move |app, sh| {
        sh.current = app.ed.slide().objects.last().map(|o| o.id);
        if let Some(id) = sh.current {
            app.ed.set_object(id, ObjectKind::Shape(shape.clone()), r, None);
        }
    }));
    v.push(Act::Wait(0.5));
    v.push(act(|app, _| app.ed.selection.clear()));
    v
}

/// The whole demo for a reference deck whose pictures were copied to
/// `images` (asset id → file).
fn storyboard(reference: &Document, images: &HashMap<String, PathBuf>, stem: &str) -> Vec<Act> {
    let stem = stem.to_owned();
    let mut v = vec![Act::Wait(3.2)];
    let mut first_math = true;
    for (i, slide) in reference.slides.iter().enumerate() {
        if i > 0 {
            v.push(step("SLIDE", "Add a blank slide"));
            v.push(Act::Move(named("Layout"), 0.8));
            v.push(Act::Click);
            v.push(Act::Wait(0.5));
            v.push(Act::Move(named("layout:Blank"), 0.5));
            v.push(Act::Click);
            v.push(Act::Wait(0.8));
        }
        for o in &slide.objects {
            match &o.kind {
                ObjectKind::Text(t) => v.extend(text_acts(o.rect, t, &mut first_math)),
                ObjectKind::Image(img) => {
                    if let Some(f) = images.get(&img.asset.0) {
                        v.extend(image_acts(o.rect, f.clone()));
                    }
                }
                ObjectKind::Shape(s) => v.extend(shape_acts(o.rect, s)),
            }
        }
        v.push(step(
            "SLIDE",
            &format!("Slide {} of {} rebuilt", i + 1, reference.slides.len()),
        ));
        v.push(act(|_, _| crate::ui::targets::clear()));
        v.push(Act::Wait(2.0));
    }
    v.push(step("PRESENT", "Present the slides"));
    v.push(Act::Move(named("slide:0"), 0.9));
    v.push(Act::Click);
    v.push(Act::Wait(0.5));
    v.push(Act::Move(named("Present"), 0.9));
    v.push(Act::Click);
    v.push(Act::NoCaption);
    for _ in 1..reference.slides.len() {
        v.push(Act::Wait(3.5));
        v.push(Act::Keys(Key::ArrowRight, Modifiers::NONE));
    }
    v.push(Act::Wait(3.5));
    v.push(Act::Keys(Key::Escape, Modifiers::NONE));
    v.push(Act::Wait(0.8));
    // Save the Beamer source through the File menu, then show it.
    v.push(step("SAVE", "Save the Beamer source: File › Export Beamer (.tex)"));
    v.extend(file_menu("file:Export Beamer (.tex)…"));
    v.push(Act::Idle);
    v.push(Act::Wait(1.2));
    v.push(step(
        "LATEX",
        "main.tex: every object placed with TikZ, every equation kept as LaTeX",
    ));
    let folder = format!("{stem}-beamer");
    v.push(act(move |_, sh| {
        let path = sh.out.join(&folder).join("main.tex");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| format!("% {}: {e}", path.display()));
        sh.code = Some((format!("{folder}/main.tex"), text, Instant::now(), CODE_SECONDS * TEMPO));
    }));
    v.push(Act::Move(Target::Screen(0.72, 0.5), 0.9));
    v.push(Act::Wait(CODE_SECONDS - 0.9));
    v.push(act(|_, sh| sh.code = None));
    v.push(Act::Wait(0.8));
    v.push(Act::NoCaption);
    v.push(Act::Wait(0.4));
    // Each step title is shown the first time only; a repeat clears the
    // caption instead of showing the same notice again.
    let mut seen = std::collections::HashSet::new();
    for a in &mut v {
        if let Act::Caption(tag, text) = a
            && !seen.insert((*tag, text.clone()))
        {
            *a = Act::NoCaption;
        }
    }
    v
}

/// How long the generated source stays on screen (it scrolls through).
const CODE_SECONDS: f32 = 11.5;

/// Open the File menu and click one of its items.
fn file_menu(item: &str) -> Vec<Act> {
    vec![
        Act::Move(named("File"), 0.9),
        Act::Click,
        Act::Wait(0.6),
        Act::Move(named(item), 0.6),
        Act::Wait(0.3),
        Act::Click,
    ]
}

/// Differences between the rebuilt document and the reference.
fn compare(rebuilt: &Document, reference: &Document) -> Vec<String> {
    let mut out = Vec::new();
    if rebuilt.slides.len() != reference.slides.len() {
        out.push(format!(
            "{} slides instead of {}",
            rebuilt.slides.len(),
            reference.slides.len()
        ));
    }
    for (i, (a, b)) in rebuilt.slides.iter().zip(&reference.slides).enumerate() {
        if a.objects.len() != b.objects.len() {
            out.push(format!(
                "slide {}: {} objects instead of {}",
                i + 1,
                a.objects.len(),
                b.objects.len()
            ));
            continue;
        }
        for (j, (x, y)) in a.objects.iter().zip(&b.objects).enumerate() {
            let same_kind = match (&x.kind, &y.kind) {
                (ObjectKind::Text(p), ObjectKind::Text(q)) => p.text == q.text && p.char_formats() == q.char_formats(),
                (ObjectKind::Image(_), ObjectKind::Image(_)) => true,
                (p, q) => p == q,
            };
            let close = (x.rect.x - y.rect.x).abs() < 0.5
                && (x.rect.y - y.rect.y).abs() < 0.5
                && (x.rect.w - y.rect.w).abs() < 0.5;
            if !same_kind || !close {
                out.push(format!("slide {} object {}: differs from the reference", i + 1, j + 1));
            }
        }
    }
    out
}

/// One captured frame on its way to the encoder.
struct Shot {
    index: usize,
    image: Arc<egui::ColorImage>,
}

struct Recorder {
    dir: PathBuf,
    /// The only sender: screenshot callbacks borrow it, so dropping it in
    /// `finish` lets the workers drain and exit even if a callback for the
    /// last frame never runs.
    tx: Arc<Mutex<Option<SyncSender<Shot>>>>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// (file, capture time) per frame, in order.
    frames: Arc<Mutex<Vec<(usize, Instant)>>>,
    next: usize,
    last: Option<Instant>,
    dropped: usize,
}

impl Recorder {
    fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Shot>(12);
        let rx = Arc::new(Mutex::new(rx));
        let workers = (0..WORKERS)
            .map(|_| {
                let rx: Arc<Mutex<Receiver<Shot>>> = Arc::clone(&rx);
                let dir = dir.clone();
                std::thread::spawn(move || {
                    loop {
                        let Ok(shot) = rx.lock().map_err(|_| ()).and_then(|r| r.recv().map_err(|_| ())) else {
                            return;
                        };
                        let [w, h] = shot.image.size;
                        let rgb: Vec<u8> = shot.image.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b()]).collect();
                        let path = dir.join(format!("frame-{:05}.jpg", shot.index));
                        let res = std::fs::File::create(&path)
                            .map_err(image::ImageError::IoError)
                            .and_then(|f| {
                                let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                                    std::io::BufWriter::new(f),
                                    JPEG_QUALITY,
                                );
                                enc.encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
                            });
                        if let Err(e) = res {
                            tracing::error!("demo frame {}: {e}", shot.index);
                        }
                    }
                })
            })
            .collect();
        Self {
            dir,
            tx: Arc::new(Mutex::new(Some(tx))),
            workers,
            frames: Arc::new(Mutex::new(Vec::new())),
            next: 0,
            last: None,
            dropped: 0,
        }
    }

    /// Capture this frame if one is due.
    fn capture(&mut self, ctx: &egui::Context, now: Instant) {
        if self
            .last
            .is_some_and(|t| now.duration_since(t).as_secs_f64() < 1.0 / FPS - 0.004)
        {
            return;
        }
        self.last = Some(now);
        let index = self.next;
        self.next += 1;
        let frames = Arc::clone(&self.frames);
        let tx = Arc::clone(&self.tx);
        ctx.request_screenshot(move |image| {
            let Ok(guard) = tx.lock() else { return };
            let Some(tx) = guard.as_ref() else { return };
            match tx.try_send(Shot { index, image }) {
                Ok(()) => {
                    if let Ok(mut f) = frames.lock() {
                        f.push((index, now));
                    }
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
            }
        });
    }

    /// Wait for the encoders and write the frame list. Returns
    /// (frames written, frames dropped because the encoders fell behind).
    fn finish(&mut self, end: Instant) -> (usize, usize) {
        if let Ok(mut tx) = self.tx.lock() {
            *tx = None;
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        let mut frames = self.frames.lock().map(|f| f.clone()).unwrap_or_default();
        frames.sort_by_key(|f| f.0);
        self.dropped = self.next.saturating_sub(frames.len());
        let mut list = String::new();
        for (k, (index, t)) in frames.iter().enumerate() {
            let until = frames.get(k + 1).map_or(end, |n| n.1);
            let secs = until.duration_since(*t).as_secs_f64().max(0.002);
            list.push_str(&format!("frame-{index:05}.jpg\t{secs:.4}\n"));
        }
        let _ = std::fs::write(self.dir.join("frames.txt"), list);
        (frames.len(), self.dropped)
    }
}

pub struct Demo {
    acts: Vec<Act>,
    step: usize,
    /// When the current act started.
    act_start: Option<Instant>,
    /// Frames spent in the current act.
    sub: usize,
    /// Characters of the current `Type` act already sent.
    typed: usize,
    move_from: Option<Pos2>,
    pointer: Pos2,
    pressed: bool,
    caption: Option<(&'static str, String, Instant)>,
    clicks: Vec<(Pos2, Instant)>,
    started: Option<Instant>,
    reference: Option<Document>,
    shared: Shared,
    /// The window's content rect (for [`Target::Screen`]).
    screen: egui::Rect,
    recorder: Option<Recorder>,
    problems: Vec<String>,
}

pub struct Report {
    pub frames: usize,
    pub dropped: usize,
    pub seconds: f64,
    pub problems: Vec<String>,
}

impl Demo {
    pub fn new() -> Self {
        Self {
            acts: Vec::new(),
            step: 0,
            act_start: None,
            sub: 0,
            typed: 0,
            move_from: None,
            pointer: pos2(900.0, 500.0),
            pressed: false,
            caption: None,
            clicks: Vec::new(),
            started: None,
            reference: None,
            shared: Shared::default(),
            screen: egui::Rect::from_min_size(Pos2::ZERO, vec2(1360.0, 840.0)),
            recorder: None,
            problems: Vec::new(),
        }
    }

    /// Take the opened deck as the reference, copy its pictures out and
    /// start over with one blank slide.
    fn setup(&mut self, app: &mut BeamerApp, out: &Path) {
        let reference = app.ed.doc.clone();
        let stem = app
            .ed
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "presentation".to_owned(), |s| s.to_string_lossy().into_owned());
        let mut images = HashMap::new();
        let dir = out.join("pictures");
        let _ = std::fs::create_dir_all(&dir);
        for id in reference.referenced_assets() {
            if let Some(meta) = app.assets.get(&id) {
                let name = if meta.original_name.is_empty() {
                    format!("{}.{}", id.0, meta.format.ext())
                } else {
                    meta.original_name.clone()
                };
                let dst = dir.join(name);
                if std::fs::copy(app.assets.path_of(meta), &dst).is_ok() {
                    images.insert(id.0.clone(), dst);
                }
            }
        }
        app.new_document();
        app.ed.doc.slides.truncate(1);
        app.ed.doc.slides[0].objects.clear();
        app.ed.doc.meta = reference.meta.clone();
        app.ed.selection.clear();
        app.ed.go_to_slide(0);
        app.present_windowed = true;
        crate::ui::targets::enable();
        app.export_base = Some(out.join(&stem));
        self.acts = storyboard(&reference, &images, &stem);
        self.reference = Some(reference);
        self.shared.out = out.to_path_buf();
    }

    fn resolve(&self, app: &BeamerApp, t: &Target) -> Option<Pos2> {
        let view = app.canvas.last_view?;
        match t {
            Target::Named(name) => crate::ui::targets::get(name).map(|r| r.center()),
            Target::Find(f) => f(app, &self.shared).map(|(x, y)| view.pt(x, y)),
            Target::Screen(fx, fy) => {
                let r = self.screen;
                Some(pos2(r.left() + r.width() * fx, r.top() + r.height() * fy))
            }
        }
    }

    fn button(&self, pressed: bool) -> Event {
        Event::PointerButton {
            pos: self.pointer,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn next(&mut self) {
        self.step += 1;
        self.sub = 0;
        self.typed = 0;
        self.act_start = None;
        self.move_from = None;
    }

    /// Drive one frame. Returns the report when the demo is over.
    pub fn tick(&mut self, app: &mut BeamerApp, ctx: &egui::Context, out: &Path) -> Option<Report> {
        let now = Instant::now();
        self.screen = ctx.content_rect();
        ctx.request_repaint();
        if self.reference.is_none() {
            // Wait until the reference deck is open and fully rendered.
            if app.ed.path.is_none() || !app.is_idle() || app.canvas.last_view.is_none() {
                return None;
            }
            self.setup(app, out);
            self.recorder = Some(Recorder::new(out.to_path_buf()));
        }
        let started = *self.started.get_or_insert(now);

        if self.step < self.acts.len() {
            let act = self.acts[self.step].clone();
            let t0 = *self.act_start.get_or_insert(now);
            let dt = now.duration_since(t0).as_secs_f32();
            if self.run(&act, app, dt) {
                self.next();
            } else {
                self.sub += 1;
                if dt > 20.0 {
                    self.problems.push(format!("step {} timed out", self.step));
                    tracing::error!("demo step {} timed out", self.step);
                    self.next();
                }
            }
        }

        let presenting = app.presenting_index().is_some();
        self.paint_overlay(ctx, now, now.duration_since(started).as_secs_f32(), presenting);
        if let Some(r) = &mut self.recorder {
            r.capture(ctx, now);
        }
        if self.step >= self.acts.len() {
            if let Some(reference) = &self.reference {
                self.problems.extend(compare(&app.ed.doc, reference));
            }
            let (frames, dropped) = self.recorder.as_mut().map_or((0, 0), |r| r.finish(now));
            return Some(Report {
                frames,
                dropped,
                seconds: now.duration_since(started).as_secs_f64(),
                problems: std::mem::take(&mut self.problems),
            });
        }
        None
    }

    /// Execute (part of) one act. Returns true when it is complete.
    fn run(&mut self, act: &Act, app: &mut BeamerApp, dt: f32) -> bool {
        match act {
            Act::Caption(tag, c) => {
                if self.caption.as_ref().is_none_or(|(_, old, _)| old != c) {
                    self.caption = Some((tag, c.clone(), Instant::now()));
                }
                true
            }
            Act::NoCaption => {
                self.caption = None;
                true
            }
            Act::Wait(s) => dt >= *s * TEMPO,
            Act::Move(target, secs) => {
                let Some(to) = self.resolve(app, target) else {
                    if dt > 3.0 {
                        let what = match target {
                            Target::Named(n) => n.replace('\n', " "),
                            Target::Find(_) | Target::Screen(..) => "point".into(),
                        };
                        self.problems.push(format!("target {what:?} not found"));
                        return true;
                    }
                    return false;
                };
                let from = *self.move_from.get_or_insert(self.pointer);
                let t = (dt / (secs * TEMPO).max(0.01)).clamp(0.0, 1.0);
                let e = t * t * (3.0 - 2.0 * t);
                self.pointer = from + (to - from) * e;
                let p = self.pointer;
                app.inject_events().push(Event::PointerMoved(p));
                t >= 1.0
            }
            Act::Click | Act::DoubleClick => {
                let presses = if matches!(act, Act::Click) { 1 } else { 2 };
                if self.sub < presses * 2 {
                    let down = self.sub.is_multiple_of(2);
                    let ev = self.button(down);
                    app.inject_events().push(ev);
                    if down {
                        self.clicks.push((self.pointer, Instant::now()));
                    }
                    false
                } else {
                    // Controls are re-marked as they are drawn next frame.
                    crate::ui::targets::clear();
                    true
                }
            }
            Act::Press => {
                let ev = self.button(true);
                app.inject_events().push(ev);
                self.pressed = true;
                self.clicks.push((self.pointer, Instant::now()));
                true
            }
            Act::Release => {
                let ev = self.button(false);
                app.inject_events().push(ev);
                self.pressed = false;
                true
            }
            Act::Type(text, cps) => {
                let chars: Vec<char> = text.chars().collect();
                let due = ((dt * cps) as usize + 1).min(chars.len());
                if due > self.typed {
                    let s: String = chars[self.typed..due].iter().collect();
                    app.inject_events().push(Event::Text(s));
                    self.typed = due;
                }
                due >= chars.len()
            }
            Act::Keys(key, modifiers) => {
                for pressed in [true, false] {
                    app.inject_events().push(Event::Key {
                        key: *key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: *modifiers,
                    });
                }
                true
            }
            Act::Do(f) => {
                f(app, &mut self.shared);
                true
            }
            Act::Until(f) => f(app, &self.shared),
            Act::SkipIf(f, n) => {
                if f(app, &self.shared) {
                    self.step += n;
                }
                true
            }
            Act::Idle => dt > 0.3 && app.is_idle(),
        }
    }

    fn paint_overlay(&self, ctx: &egui::Context, now: Instant, t: f32, presenting: bool) {
        let layer = egui::LayerId::new(egui::Order::Debug, egui::Id::unique("bx-demo-overlay"));
        let p = ctx.layer_painter(layer);
        let screen = ctx.content_rect();

        let mut wheel = false;
        if let Some((title, text, since, secs)) = &self.shared.code {
            wheel = paint_code(&p, screen, title, text, now.duration_since(*since).as_secs_f32(), *secs);
        }

        // Title card at the start.
        if t < INTRO {
            let fade = INTRO * 0.75;
            let a = if t < fade {
                1.0
            } else {
                1.0 - (t - fade) / (INTRO - fade)
            };
            p.rect_filled(
                screen,
                0.0,
                Color32::from_rgba_unmultiplied(22, 22, 26, (235.0 * a) as u8),
            );
            let c = Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * a) as u8);
            p.text(
                screen.center() - vec2(0.0, 30.0),
                egui::Align2::CENTER_CENTER,
                "BeamerX",
                egui::FontId::proportional(64.0),
                c,
            );
            p.text(
                screen.center() + vec2(0.0, 30.0),
                egui::Align2::CENTER_CENTER,
                "A native macOS editor for scientific Beamer presentations",
                egui::FontId::proportional(22.0),
                Color32::from_rgba_unmultiplied(220, 220, 225, (255.0 * a) as u8),
            );
            p.text(
                screen.center() + vec2(0.0, 66.0),
                egui::Align2::CENTER_CENTER,
                "Visual editing · live LaTeX · Beamer and PDF export",
                egui::FontId::proportional(16.0),
                Color32::from_rgba_unmultiplied(196, 62, 28, (255.0 * a) as u8),
            );
        }

        // Caption pill.
        if let Some((tag, text, since)) = &self.caption {
            paint_step(&p, screen, tag, text, now.duration_since(*since).as_secs_f32());
        }

        // Click ripples and the cursor (not over a running presentation).
        if presenting {
            return;
        }
        for (at, when) in &self.clicks {
            let age = now.duration_since(*when).as_secs_f32();
            if age < 0.45 {
                let k = age / 0.45;
                p.circle_stroke(
                    *at,
                    8.0 + 18.0 * k,
                    egui::Stroke::new(
                        3.0 * (1.0 - k) + 0.5,
                        Color32::from_rgba_unmultiplied(196, 62, 28, (220.0 * (1.0 - k)) as u8),
                    ),
                );
            }
        }

        // Cursor (macOS-style arrow), hidden under the title card.
        if t >= INTRO * 0.75 {
            let at = self.pointer;
            let s = 1.15;
            let q = |x: f32, y: f32| at + vec2(x * s, y * s);
            let outline = vec![
                q(0.0, 0.0),
                q(0.0, 17.0),
                q(4.2, 13.2),
                q(7.0, 19.6),
                q(9.8, 18.4),
                q(7.1, 12.2),
                q(12.6, 12.2),
            ];
            p.add(egui::Shape::closed_line(
                outline,
                egui::Stroke::new(2.6, Color32::WHITE),
            ));
            let black = Color32::from_rgb(12, 12, 12);
            p.add(egui::Shape::convex_polygon(
                vec![q(0.0, 0.0), q(0.0, 17.0), q(4.2, 13.2), q(7.1, 12.2), q(12.6, 12.2)],
                black,
                egui::Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![q(4.2, 13.2), q(7.0, 19.6), q(9.8, 18.4), q(7.1, 12.2)],
                black,
                egui::Stroke::NONE,
            ));
            if self.pressed {
                p.circle_filled(at, 5.0, Color32::from_rgba_unmultiplied(196, 62, 28, 120));
            }
            if wheel {
                paint_wheel_hint(&p, at, t);
            }
        }
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

/// A dark editor panel showing `text` (LaTeX, lightly highlighted) with line
/// numbers. After `HOLD` seconds at the top it scrolls rapidly to the end in
/// mouse-wheel steps (`SCROLL` seconds), then rests there. Returns true while
/// scrolling.
fn paint_code(p: &egui::Painter, screen: egui::Rect, title: &str, text: &str, t: f32, secs: f32) -> bool {
    use egui::text::{LayoutJob, TextFormat};
    const HOLD: f32 = 1.0;
    const SCROLL: f32 = 3.0;
    const NOTCH: f32 = 0.12;
    let panel = egui::Rect::from_min_max(
        pos2(screen.left() + 60.0, screen.top() + 54.0),
        pos2(screen.right() - 60.0, screen.bottom() - 120.0),
    );
    p.rect_filled(panel.expand(1.0), 12.0, Color32::from_black_alpha(90));
    p.rect_filled(panel, 10.0, Color32::from_rgb(30, 31, 36));
    // Title bar.
    let bar = egui::Rect::from_min_size(panel.min, vec2(panel.width(), 30.0));
    p.rect_filled(
        bar,
        egui::CornerRadius {
            nw: 10,
            ne: 10,
            sw: 0,
            se: 0,
        },
        Color32::from_rgb(44, 45, 52),
    );
    for (i, c) in [(255, 95, 86), (255, 189, 46), (39, 201, 63)].into_iter().enumerate() {
        p.circle_filled(
            pos2(bar.left() + 18.0 + i as f32 * 18.0, bar.center().y),
            5.5,
            Color32::from_rgb(c.0, c.1, c.2),
        );
    }
    p.text(
        bar.center(),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(13.5),
        Color32::from_rgb(210, 210, 216),
    );

    let font = egui::FontId::monospace(12.5);
    let plain = Color32::from_rgb(214, 216, 222);
    let comment = Color32::from_rgb(120, 140, 120);
    let command = Color32::from_rgb(255, 140, 105);
    let math = Color32::from_rgb(110, 200, 220);
    let gutter = 46.0;
    let view = egui::Rect::from_min_max(
        pos2(panel.left() + 8.0, bar.bottom() + 8.0),
        pos2(panel.right() - 16.0, panel.bottom() - 8.0),
    );
    let text_left = view.left() + gutter;
    // One wrapped galley per source line, numbered in the gutter.
    let lines: Vec<std::sync::Arc<egui::Galley>> = text
        .lines()
        .map(|line| {
            let mut job = LayoutJob::default();
            job.wrap.max_width = view.right() - text_left;
            let chars: Vec<char> = line.chars().collect();
            let (mut i, mut in_math) = (0, false);
            let mut run = String::new();
            let mut run_color = plain;
            let flush = |job: &mut LayoutJob, run: &mut String, color: Color32| {
                if !run.is_empty() {
                    job.append(run, 0.0, TextFormat::simple(font.clone(), color));
                    run.clear();
                }
            };
            while i < chars.len() {
                let c = chars[i];
                let (piece, color): (String, Color32) = if c == '%' && (i == 0 || chars[i - 1] != '\\') {
                    let rest: String = chars[i..].iter().collect();
                    i = chars.len();
                    (rest, comment)
                } else if c == '\\' {
                    let mut j = i + 1;
                    while j < chars.len() && chars[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    if j == i + 1 && j < chars.len() {
                        j += 1;
                    }
                    let s: String = chars[i..j].iter().collect();
                    i = j;
                    (s, if in_math { math } else { command })
                } else {
                    if c == '$' {
                        in_math = !in_math;
                    }
                    i += 1;
                    (c.to_string(), if in_math || c == '$' { math } else { plain })
                };
                if color != run_color {
                    flush(&mut job, &mut run, run_color);
                    run_color = color;
                }
                run.push_str(&piece);
            }
            flush(&mut job, &mut run, run_color);
            if job.sections.is_empty() {
                job.append(" ", 0.0, TextFormat::simple(font.clone(), plain));
            }
            p.layout_job(job)
        })
        .collect();
    let total: f32 = lines.iter().map(|g| g.size().y).sum();
    let overflow = (total - view.height()).max(0.0);

    // Wheel-like scrolling: one eased notch every NOTCH seconds.
    let scroll_time = SCROLL.min(secs - HOLD - 1.0).max(0.5);
    let notches = (scroll_time / NOTCH).floor().max(1.0);
    let step = overflow / notches;
    let s = (t - HOLD).max(0.0) / NOTCH;
    let done = s.floor();
    let frac = (s - done).min(1.0);
    let ease = 1.0 - (1.0 - (frac / 0.6).min(1.0)).powi(3);
    let offset = ((done + ease) * step).min(overflow);
    let scrolling = overflow > 0.0 && t > HOLD && offset < overflow;

    let clip = p.with_clip_rect(view);
    let mut y = view.top() - offset;
    for (n, g) in lines.into_iter().enumerate() {
        let h = g.size().y;
        if y + h >= view.top() && y <= view.bottom() {
            clip.text(
                pos2(view.left() + gutter - 12.0, y),
                egui::Align2::RIGHT_TOP,
                (n + 1).to_string(),
                font.clone(),
                Color32::from_rgb(98, 102, 112),
            );
            clip.galley(pos2(text_left, y), g, plain);
        }
        y += h;
    }
    p.vline(
        view.left() + gutter - 4.0,
        view.y_range(),
        egui::Stroke::new(1.0, Color32::from_rgb(52, 54, 62)),
    );
    if overflow > 0.0 {
        let track = egui::Rect::from_min_max(
            pos2(panel.right() - 11.0, view.top()),
            pos2(panel.right() - 4.0, view.bottom()),
        );
        p.rect_filled(track, 3.0, Color32::from_rgb(44, 45, 52));
        let h = (track.height() * view.height() / total).max(24.0);
        let y = track.top() + (track.height() - h) * (offset / overflow);
        p.rect_filled(
            egui::Rect::from_min_size(pos2(track.left(), y), vec2(track.width(), h)),
            3.0,
            if scrolling {
                Color32::from_rgb(170, 172, 182)
            } else {
                Color32::from_rgb(120, 122, 132)
            },
        );
    }
    scrolling
}

/// A small mouse with a turning wheel and a down arrow, next to the cursor.
fn paint_wheel_hint(p: &egui::Painter, at: Pos2, t: f32) {
    let body = egui::Rect::from_center_size(at + vec2(30.0, 26.0), vec2(18.0, 28.0));
    p.rect_filled(body, 9.0, Color32::from_rgba_unmultiplied(255, 255, 255, 235));
    p.rect_stroke(
        body,
        9.0,
        egui::Stroke::new(1.5, Color32::from_rgb(40, 40, 46)),
        egui::StrokeKind::Inside,
    );
    let wy = body.top() + 6.0 + (t * 6.0).fract() * 6.0;
    p.line_segment(
        [pos2(body.center().x, wy), pos2(body.center().x, wy + 4.0)],
        egui::Stroke::new(2.5, Color32::from_rgb(196, 62, 28)),
    );
    let ax = body.right() + 10.0;
    let c = Color32::from_rgb(255, 255, 255);
    p.line_segment(
        [pos2(ax, body.top() + 4.0), pos2(ax, body.bottom() - 4.0)],
        egui::Stroke::new(2.5, c),
    );
    p.add(egui::Shape::convex_polygon(
        vec![
            pos2(ax - 5.0, body.bottom() - 8.0),
            pos2(ax + 5.0, body.bottom() - 8.0),
            pos2(ax, body.bottom() - 1.0),
        ],
        c,
        egui::Stroke::NONE,
    ));
}

/// A step title: a coloured badge with the tag on the left, sliding up
/// and fading in when it changes.
fn paint_step(p: &egui::Painter, screen: egui::Rect, tag: &str, text: &str, age: f32) {
    let a = (age / 0.3).min(1.0);
    let lift = (1.0 - a) * 14.0;
    let color = tag_color(tag);
    let alpha = |c: Color32, k: f32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (255.0 * k * a) as u8);
    let tag_galley = p.layout_no_wrap(tag.to_owned(), egui::FontId::proportional(14.0), Color32::WHITE);
    let text_galley = p.layout_no_wrap(text.to_owned(), egui::FontId::proportional(21.0), Color32::WHITE);
    let h = text_galley.size().y + 22.0;
    let tag_w = tag_galley.size().x + 26.0;
    let w = tag_w + text_galley.size().x + 40.0;
    let rect = egui::Rect::from_center_size(pos2(screen.center().x, screen.bottom() - 78.0 + lift), vec2(w, h));
    // Shadow, body, tag segment and a light rim.
    p.rect_filled(
        rect.translate(vec2(0.0, 4.0)).expand(2.0),
        16.0,
        Color32::from_black_alpha((70.0 * a) as u8),
    );
    p.rect_filled(rect, 14.0, alpha(color, 0.96));
    let tag_rect = egui::Rect::from_min_size(rect.min, vec2(tag_w, h));
    let darker = Color32::from_rgb(
        (color.r() as f32 * 0.62) as u8,
        (color.g() as f32 * 0.62) as u8,
        (color.b() as f32 * 0.62) as u8,
    );
    p.rect_filled(
        tag_rect,
        egui::CornerRadius {
            nw: 14,
            sw: 14,
            ne: 0,
            se: 0,
        },
        alpha(darker, 1.0),
    );
    p.rect_stroke(
        rect,
        14.0,
        egui::Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 255, 255, (90.0 * a) as u8)),
        egui::StrokeKind::Inside,
    );
    p.galley(
        tag_rect.center() - tag_galley.size() * 0.5,
        tag_galley,
        Color32::from_rgba_unmultiplied(255, 255, 255, (235.0 * a) as u8),
    );
    p.galley(
        pos2(tag_rect.right() + 20.0, rect.center().y - text_galley.size().y * 0.5),
        text_galley,
        Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * a) as u8),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_runs_follow_explicit_formats() {
        let mut t = TextBox {
            text: "a v//B b".into(),
            ..Default::default()
        };
        t.format_range(2, 3, |f| f.bold = Some(true));
        t.format_range(5, 6, |f| f.bold = Some(true));
        assert_eq!(bold_runs(&t), vec![(2, 3), (5, 6)]);
    }

    #[test]
    fn storyboard_rebuilds_every_object_and_ends_quietly() {
        let mut doc = Document::new();
        let s = &mut doc.slides[0];
        s.objects.clear();
        let id = doc.alloc_object_id();
        doc.slides[0].objects.push(crate::document::Object {
            id,
            rect: Rect::new(10.0, 10.0, 100.0, 20.0),
            kind: ObjectKind::Text(TextBox {
                text: "x $y$".into(),
                ..Default::default()
            }),
        });
        let acts = storyboard(&doc, &HashMap::new(), "deck");
        let presses = acts.iter().filter(|a| matches!(a, Act::Press)).count();
        let releases = acts.iter().filter(|a| matches!(a, Act::Release)).count();
        assert_eq!(presses, releases);
        assert!(acts.iter().any(|a| matches!(a, Act::Type(t, _) if t == "x $y$")));
        assert!(matches!(acts[acts.len() - 2], Act::NoCaption));
        // Every step title appears once.
        let titles: Vec<_> = acts
            .iter()
            .filter_map(|a| match a {
                Act::Caption(tag, text) => Some((*tag, text.clone())),
                _ => None,
            })
            .collect();
        let unique: std::collections::HashSet<_> = titles.iter().cloned().collect();
        assert_eq!(titles.len(), unique.len());
        assert!(compare(&doc, &doc).is_empty());
    }
}
