//! Layout of text boxes containing LaTeX math (`$…$`, `$$…$$`).
//!
//! Text is shaped by egui word by word (Latin Modern Sans); closed math is
//! typeset by LaTeX and placed as an atom on the baseline using the depth
//! measured by TeX. Lines wrap greedily at spaces, like the exported
//! `\raggedright` TikZ node, and follow TeX's line-spacing rule
//! (`\baselineskip` = 1.2 × size, `\lineskip` = 1 bp when tall math would
//! collide).
//!
//! While editing, the math run containing the caret is shown as LaTeX source
//! so it can be edited; everything else stays rendered ("live preview").
//! All coordinates are screen points relative to the box's top-left corner.

use crate::document::{Color, ListKind, TextAlign, TextBox, typeset};
use crate::math::{MathRender, MathService, MathStatus};
use crate::renderer::{c32, text_family};
use crate::rich::{self, ParaKind, RunKind};
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Color32, FontId, Galley, Painter, Pos2, Rect, Vec2, pos2, vec2};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

/// How math that is not (yet) rendered is drawn.
const SOURCE_COLOR: Color32 = Color32::from_rgb(31, 95, 168);
const ERROR_COLOR: Color32 = Color32::from_rgb(200, 40, 40);

#[derive(Clone)]
pub enum Piece {
    Glyphs { galley: Arc<Galley>, baseline: f32 },
    Math(Arc<MathRender>),
}

#[derive(Clone)]
pub struct Placed {
    pub piece: Piece,
    pub x: f32,
    pub w: f32,
    pub chars: Range<usize>,
    /// Ascent/descent in screen points.
    pub ascent: f32,
    pub descent: f32,
    /// For rendered math: caret index used when the user clicks into it
    /// (just before the closing delimiter), which reveals its source.
    pub enter_at: Option<usize>,
    /// Tint for math (its text colour).
    pub tint: Color32,
}

#[derive(Clone)]
pub struct Line {
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Font size of the line's paragraph (screen points).
    pub size: f32,
    pub items: Vec<Placed>,
    /// List label (bullet or number): galley, its baseline offset and x.
    pub bullet: Option<(Arc<Galley>, f32, f32)>,
    /// Char stops on this line: (index, x), sorted by x.
    pub stops: Vec<(usize, f32)>,
}

impl Line {
    pub fn top(&self) -> f32 {
        self.baseline - self.ascent
    }
    pub fn bottom(&self) -> f32 {
        self.baseline + self.descent
    }
}

pub struct TextLayout {
    pub lines: Vec<Line>,
    /// Content height in screen points.
    pub height: f32,
    /// Number of chars in the source.
    pub len: usize,
    /// Some math is still being typeset.
    pub pending: bool,
    /// LaTeX errors of math runs in this box.
    pub errors: Vec<(String, Arc<str>)>,
}

pub struct LayoutInput<'a> {
    pub style: &'a TextBox,
    pub width_bp: f32,
    pub scale: f32,
    /// Caret while editing: the math run containing it is shown as source,
    /// and escapes are shown raw so caret positions map 1:1 to chars.
    pub caret: Option<usize>,
}

enum Atom {
    /// A word; `raw` words (math source) use `color` instead of char formats.
    Word {
        range: Range<usize>,
        text: String,
        raw: Option<Color32>,
    },
    Space {
        range: Range<usize>,
    },
    Math {
        range: Range<usize>,
        render: Arc<MathRender>,
        enter_at: usize,
    },
}

/// Per-paragraph text measurement context.
struct Measure<'p> {
    painter: &'p Painter,
    size: f32,
}

impl Measure<'_> {
    fn job(&self, sections: &[(String, bool, bool, Color32)]) -> Arc<Galley> {
        let mut job = LayoutJob::default();
        for (text, bold, italic, color) in sections {
            job.append(
                text,
                0.0,
                TextFormat {
                    font_id: FontId::new(self.size, text_family(*bold, *italic)),
                    color: *color,
                    ..Default::default()
                },
            );
        }
        self.painter.layout_job(job)
    }

    fn galley(&self, text: &str, bold: bool, italic: bool, color: Color32) -> Arc<Galley> {
        self.job(&[(text.to_string(), bold, italic, color)])
    }

    fn baseline_of(g: &Galley) -> f32 {
        g.rows
            .first()
            .and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y))
            .unwrap_or(0.0)
    }

    fn space_width(&self) -> f32 {
        let a = self.galley("x x", false, false, Color32::BLACK).size().x;
        let b = self.galley("xx", false, false, Color32::BLACK).size().x;
        (a - b).max(self.size * 0.2)
    }
}

/// Lay out a text box.
pub fn layout(painter: &Painter, math: &mut MathService, now: Instant, inp: &LayoutInput) -> TextLayout {
    let t = inp.style;
    let s = inp.scale;
    let chars: Vec<char> = t.text.chars().collect();
    let fmts = t.char_formats();
    let pfs = t.paras_normalized();
    let labels = crate::document::list_labels(&pfs);
    let paras = rich::paragraphs(&chars);
    let editing = inp.caret.is_some();
    let inside = |r: &Range<usize>| inp.caret.is_some_and(|c| c > r.start && c < r.end);
    let unit = t.font_size * typeset::BULLET_INDENT * s;

    let mut pending = false;
    let mut errors = Vec::new();
    let mut lines: Vec<Line> = Vec::new();

    // Tokenize plain chars (text or raw math source) into words and spaces.
    let tokenize = |atoms: &mut Vec<Atom>, r: Range<usize>, raw: Option<Color32>| {
        let mut i = r.start;
        while i < r.end {
            if chars[i].is_whitespace() {
                atoms.push(Atom::Space { range: i..i + 1 });
                i += 1;
            } else {
                let start = i;
                while i < r.end && !chars[i].is_whitespace() {
                    i += 1;
                }
                let text = if raw.is_some() || editing {
                    chars[start..i].iter().collect()
                } else {
                    rich::unescape_text(&chars[start..i])
                };
                atoms.push(Atom::Word {
                    range: start..i,
                    text,
                    raw,
                });
            }
        }
    };

    for para in &paras {
        let pf = pfs.get(para.line).copied().unwrap_or_default();
        let base_size = t.font_size * pf.style.size_factor();
        let size = base_size * s;
        let m = Measure { painter, size };
        let resolved = |i: usize| t.resolve(fmts.get(i).copied().unwrap_or_default(), pf);
        let list = pf.list != ListKind::None;
        let indent = f32::from(pf.level) * unit + if list { unit } else { 0.0 };
        let limit = (inp.width_bp * s - indent).max(1.0);
        let (text_asc, text_desc) = (
            size * typeset::FIRST_BASELINE,
            size * (typeset::LINE_SPREAD - typeset::FIRST_BASELINE),
        );
        let space_w = m.space_width();

        let mut atoms: Vec<Atom> = Vec::new();
        let mut display_atom = false;
        // Rendered math, or the colour to show its source in.
        let mut query =
            |src: &Range<usize>, display: bool, math: &mut MathService| -> Result<Arc<MathRender>, Color32> {
                let src_s = rich::slice(&chars, src);
                match math.request(&src_s, base_size, display, now) {
                    MathStatus::Ready(r) => Ok(r),
                    MathStatus::Pending => {
                        pending = true;
                        Err(SOURCE_COLOR)
                    }
                    MathStatus::Failed(e) => {
                        errors.push((src_s, e));
                        Err(ERROR_COLOR)
                    }
                }
            };
        match &para.kind {
            ParaKind::Display { src } => {
                let r = if inside(&para.range) {
                    Err(SOURCE_COLOR)
                } else {
                    query(src, true, math)
                };
                match r {
                    Ok(render) => {
                        atoms.push(Atom::Math {
                            range: para.range.clone(),
                            render,
                            enter_at: src.end,
                        });
                        display_atom = true;
                    }
                    Err(c) => tokenize(&mut atoms, para.range.clone(), Some(c)),
                }
            }
            ParaKind::Normal(runs) => {
                for run in runs {
                    match &run.kind {
                        RunKind::Text => tokenize(&mut atoms, run.range.clone(), None),
                        RunKind::Math { src } => {
                            let r = if inside(&run.range) {
                                Err(SOURCE_COLOR)
                            } else {
                                query(src, false, math)
                            };
                            match r {
                                Ok(render) => atoms.push(Atom::Math {
                                    range: run.range.clone(),
                                    render,
                                    enter_at: src.end,
                                }),
                                Err(c) => tokenize(&mut atoms, run.range.clone(), Some(c)),
                            }
                        }
                    }
                }
            }
        }

        // ---- break into lines --------------------------------------------
        let mut cur = new_line(size);
        let mut x = 0.0f32;
        let mut has_content = false;
        let mut para_lines: Vec<Line> = Vec::new();
        let mut i = 0;
        while i < atoms.len() {
            if let Atom::Space { range } = &atoms[i] {
                cur.stops.push((range.start, x));
                x += space_w;
                cur.stops.push((range.end, x));
                i += 1;
                continue;
            }
            // A chunk of non-space atoms cannot be broken.
            let mut j = i;
            let mut cw = 0.0;
            let mut built: Vec<Placed> = Vec::new();
            while j < atoms.len() && !matches!(atoms[j], Atom::Space { .. }) {
                let p = place(&atoms[j], &m, s, &resolved, editing);
                cw += p.w;
                built.push(p);
                j += 1;
            }
            if has_content && x + cw > limit {
                para_lines.push(std::mem::replace(&mut cur, new_line(size)));
                x = 0.0;
            }
            for mut p in built {
                p.x = x;
                add_stops(&mut cur, &p, x);
                x += p.w;
                cur.items.push(p);
            }
            has_content = true;
            i = j;
        }
        if cur.stops.is_empty() {
            cur.stops.push((para.range.start, 0.0));
        }
        para_lines.push(cur);

        // ---- horizontal alignment ----------------------------------------
        let label = labels.get(para.line).filter(|l| para.starts_line && !l.is_empty());
        for (k, line) in para_lines.iter_mut().enumerate() {
            let content_w = line.items.last().map(|p| p.x + p.w).unwrap_or(0.0);
            let offset = if display_atom {
                ((limit - content_w) * 0.5).max(0.0)
            } else {
                match t.align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => ((limit - content_w) * 0.5).max(0.0),
                    TextAlign::Right => (limit - content_w).max(0.0),
                }
            } + indent;
            for p in &mut line.items {
                p.x += offset;
            }
            for st in &mut line.stops {
                st.1 += offset;
            }
            if k == 0
                && let Some(l) = label
            {
                // The label hangs in the indent, left-aligned in one unit.
                let r = resolved(para.range.start);
                let g = m.galley(l, r.bold, false, c32(r.color));
                let b = Measure::baseline_of(&g);
                line.bullet = Some((g, b, offset - unit));
            }
            line.ascent = line.items.iter().map(|p| p.ascent).fold(text_asc, f32::max);
            line.descent = line.items.iter().map(|p| p.descent).fold(text_desc, f32::max);
        }
        lines.extend(para_lines);
    }

    // ---- vertical placement (TeX's baselineskip / lineskip rule) ----------
    // Each line uses its own paragraph's \baselineskip (1.2 × its size).
    let lineskip = s; // 1 bp
    let mut prev_desc: Option<f32> = None;
    let mut baseline = 0.0;
    for line in &mut lines {
        let skip = line.size * typeset::LINE_SPREAD;
        baseline = match prev_desc {
            None => line.ascent,
            Some(d) => {
                if d + line.ascent > skip + 1e-3 {
                    baseline + d + line.ascent + lineskip
                } else {
                    baseline + skip
                }
            }
        };
        line.baseline = baseline;
        prev_desc = Some(line.descent);
        line.stops.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    }
    let fallback = t.font_size * s * typeset::LINE_SPREAD;
    let height = lines.last().map(|l| l.bottom()).unwrap_or(fallback);
    TextLayout {
        lines,
        height,
        len: chars.len(),
        pending,
        errors,
    }
}

fn new_line(size: f32) -> Line {
    Line {
        baseline: 0.0,
        ascent: 0.0,
        descent: 0.0,
        size,
        items: Vec::new(),
        bullet: None,
        stops: Vec::new(),
    }
}

fn place(
    a: &Atom,
    m: &Measure,
    scale: f32,
    resolved: &dyn Fn(usize) -> crate::document::Resolved,
    editing: bool,
) -> Placed {
    match a {
        Atom::Word { range, text, raw } => {
            // Split the word into sections where the char format changes.
            let wchars: Vec<char> = text.chars().collect();
            let mut sections: Vec<(String, bool, bool, Color32)> = Vec::new();
            // Escapes are hidden when not editing, so map sections by source
            // index only when lengths agree.
            let aligned = wchars.len() == range.len() || editing;
            for (k, c) in wchars.iter().enumerate() {
                let r = resolved(if aligned { range.start + k } else { range.start });
                let fmt = match raw {
                    Some(color) => (false, false, *color),
                    None => (r.bold, r.italic, c32(r.color)),
                };
                match sections.last_mut() {
                    Some(last) if (last.1, last.2, last.3) == fmt => last.0.push(*c),
                    _ => sections.push((c.to_string(), fmt.0, fmt.1, fmt.2)),
                }
            }
            let galley = m.job(&sections);
            let baseline = Measure::baseline_of(&galley);
            Placed {
                w: galley.size().x,
                x: 0.0,
                chars: range.clone(),
                ascent: 0.0,
                descent: 0.0,
                enter_at: None,
                tint: Color32::WHITE,
                piece: Piece::Glyphs { galley, baseline },
            }
        }
        Atom::Math {
            range,
            render,
            enter_at,
        } => Placed {
            w: render.size_bp.0 * scale,
            x: 0.0,
            chars: range.clone(),
            ascent: render.ascent_bp() * scale,
            descent: render.depth_bp * scale,
            enter_at: Some(*enter_at),
            tint: c32(resolved(range.start).color),
            piece: Piece::Math(Arc::clone(render)),
        },
        Atom::Space { .. } => unreachable!("spaces are not placed"),
    }
}

fn add_stops(line: &mut Line, p: &Placed, x: f32) {
    match &p.piece {
        Piece::Glyphs { galley, .. } => {
            for k in 0..=(p.chars.end - p.chars.start) {
                let dx = galley.pos_from_cursor(CCursor::new(k)).min.x;
                line.stops.push((p.chars.start + k, x + dx));
            }
        }
        Piece::Math(_) => {
            line.stops.push((p.chars.start, x));
            line.stops.push((p.chars.end, x + p.w));
        }
    }
}

impl TextLayout {
    /// Screen-space caret rectangle (relative to the box) for char `idx`.
    pub fn caret_rect(&self, idx: usize, size: f32) -> Rect {
        let (li, x) = self.stop_of(idx);
        let line = &self.lines[li];
        let size = if line.size > 0.0 { line.size } else { size };
        let asc = size * typeset::FIRST_BASELINE * 0.95;
        let desc = size * 0.25;
        Rect::from_min_max(pos2(x, line.baseline - asc), pos2(x + 1.0, line.baseline + desc))
    }

    /// Line index and x of char index `idx` (nearest known stop).
    pub fn stop_of(&self, idx: usize) -> (usize, f32) {
        let mut best: Option<(usize, f32, usize)> = None;
        for (li, l) in self.lines.iter().enumerate() {
            for &(i, x) in &l.stops {
                let d = i.abs_diff(idx);
                if best.is_none_or(|b| d < b.2) {
                    best = Some((li, x, d));
                }
                if d == 0 {
                    return (li, x);
                }
            }
        }
        best.map(|b| (b.0, b.1)).unwrap_or((0, 0.0))
    }

    /// Char index nearest to a point (relative to the box). Clicking on
    /// rendered math enters it (caret just before its closing `$`).
    pub fn hit(&self, p: Vec2) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let li = self
            .lines
            .iter()
            .position(|l| p.y <= l.bottom())
            .unwrap_or(self.lines.len() - 1);
        self.hit_on_line(li, p.x)
    }

    pub fn hit_on_line(&self, li: usize, x: f32) -> usize {
        let line = &self.lines[li.min(self.lines.len() - 1)];
        for it in &line.items {
            if let Some(enter) = it.enter_at
                && x > it.x + 2.0
                && x < it.x + it.w - 2.0
            {
                // Enter the formula (reveals its source for editing).
                return enter;
            }
        }
        line.stops
            .iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map(|s| s.0)
            .unwrap_or(0)
    }

    /// Range of the line containing `idx` (first, last char index).
    pub fn line_bounds(&self, idx: usize) -> (usize, usize) {
        let (li, _) = self.stop_of(idx);
        let l = &self.lines[li];
        let lo = l.stops.iter().map(|s| s.0).min().unwrap_or(idx);
        let hi = l.stops.iter().map(|s| s.0).max().unwrap_or(idx);
        (lo, hi)
    }

    /// Highlight rectangles for the char range `a..b` (relative to the box).
    pub fn selection_rects(&self, a: usize, b: usize) -> Vec<Rect> {
        let (a, b) = (a.min(b), a.max(b));
        let mut out = Vec::new();
        for l in &self.lines {
            let xs: Vec<f32> = l.stops.iter().filter(|s| s.0 >= a && s.0 <= b).map(|s| s.1).collect();
            if xs.len() >= 2 {
                let x0 = xs.iter().copied().fold(f32::INFINITY, f32::min);
                let x1 = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                out.push(Rect::from_min_max(pos2(x0, l.top()), pos2(x1, l.bottom())));
            }
        }
        out
    }
}

/// Paint a laid-out text box at `origin` (screen position of its top-left).
pub fn paint(
    p: &Painter,
    origin: Pos2,
    lay: &TextLayout,
    color: Color,
    textures: &mut super::textures::TextureCache,
    px_per_bp: f32,
    scale: f32,
) {
    let tint = c32(color);
    for line in &lay.lines {
        if let Some((g, b, x)) = &line.bullet {
            p.galley(origin + vec2(*x, line.baseline - b), Arc::clone(g), tint);
        }
        for it in &line.items {
            match &it.piece {
                Piece::Glyphs { galley, baseline } => {
                    p.galley(origin + vec2(it.x, line.baseline - baseline), Arc::clone(galley), tint);
                }
                Piece::Math(r) => {
                    let rect = Rect::from_min_size(
                        origin + vec2(it.x, line.baseline - it.ascent),
                        vec2(it.w, r.size_bp.1 * scale),
                    );
                    if let Some(tex) = textures.math(r, px_per_bp) {
                        p.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), it.tint);
                    }
                }
            }
        }
    }
}
