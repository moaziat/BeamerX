//! Typed, versioned document model.
//!
//! This module is independent of the GUI and of the export backends. All
//! geometry is stored in **slide space**: PostScript points (`bp`, 1/72 inch)
//! with the origin at the top-left corner of the slide and `y` growing
//! downwards. A slide is 16 cm × 9 cm, exactly the paper size used by
//! `\documentclass[aspectratio=169]{beamer}`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Slide width in bp (160 mm).
pub const SLIDE_W: f32 = 160.0 / 25.4 * 72.0;
/// Slide height in bp (90 mm).
pub const SLIDE_H: f32 = 90.0 / 25.4 * 72.0;

/// Typesetting conventions shared by the editor renderer and the exporter so
/// that both place text identically.
pub mod typeset {
    /// Baseline-to-baseline distance as a multiple of the font size
    /// (`\fontsize{S}{1.2S}`, LaTeX's standard ratio).
    pub const LINE_SPREAD: f32 = 1.2;
    /// Distance from the top of a text box to its first baseline, as a
    /// multiple of the font size. Equals LaTeX's `\strut` height
    /// (0.7 × baselineskip); the exporter starts every text box with `\strut`.
    pub const FIRST_BASELINE: f32 = 0.7 * LINE_SPREAD;
    /// Hanging indent of bullet lists, as a multiple of the font size.
    pub const BULLET_INDENT: f32 = 1.0;
}

/// Smallest permitted object extent (bp) for boxes that the user resizes.
pub const MIN_EXTENT: f32 = 2.0;
pub const MIN_FONT_SIZE: f32 = 4.0;
pub const MAX_FONT_SIZE: f32 = 144.0;
/// Coordinates are clamped to this range to keep files sane.
pub const COORD_LIMIT: f32 = 10_000.0;
/// Hard cap for one freehand stroke. Pointer sampling and simplification keep
/// normal strokes far below this; the cap protects loading and rendering from
/// hostile or accidentally enormous project files.
pub const MAX_FREEHAND_POINTS: usize = 8_192;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlideId(pub u64);

/// Content hash (lower-case hex SHA-256) of an imported asset.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub String);

impl AssetId {
    /// An asset id is exactly 64 lower-case hex digits. Anything else is
    /// rejected so that ids can be used safely to build file names.
    pub fn is_valid(&self) -> bool {
        self.0.len() == 64 && self.0.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Geometry & colour
// ---------------------------------------------------------------------------

/// Axis-aligned rectangle in slide space (bp). `w` and `h` are never negative.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Rectangle spanning two arbitrary corner points.
    pub fn from_points(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            w: (a.0 - b.0).abs(),
            h: (a.1 - b.1).abs(),
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect {
            x,
            y,
            w: self.right().max(o.right()) - x,
            h: self.bottom().max(o.bottom()) - y,
        }
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x <= o.right() && o.x <= self.right() && self.y <= o.bottom() && o.y <= self.bottom()
    }

    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 <= self.right() && p.1 >= self.y && p.1 <= self.bottom()
    }

    pub fn expanded(&self, m: f32) -> Rect {
        Rect {
            x: self.x - m,
            y: self.y - m,
            w: self.w + 2.0 * m,
            h: self.h + 2.0 * m,
        }
    }

    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }

    /// Clamp into the sane coordinate range and make extents non-negative.
    pub fn sanitized(&self) -> Rect {
        let c = |v: f32| {
            if v.is_finite() {
                v.clamp(-COORD_LIMIT, COORD_LIMIT)
            } else {
                0.0
            }
        };
        Rect {
            x: c(self.x),
            y: c(self.y),
            w: c(self.w).max(0.0),
            h: c(self.h).max(0.0),
        }
    }
}

/// sRGB colour with straight (non-premultiplied) alpha. Serialised as
/// `"#rrggbb"` or `"#rrggbbaa"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const ACCENT: Color = Color::rgb(0x1f, 0x5f, 0xa8);
    pub const LIGHT_FILL: Color = Color::rgb(0xd6, 0xe4, 0xf5);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    pub fn parse_hex(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#')?;
        if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        let a = if h.len() == 8 { byte(6)? } else { 255 };
        Some(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a,
        })
    }
}

impl From<Color> for String {
    fn from(c: Color) -> String {
        c.to_hex()
    }
}

impl TryFrom<String> for Color {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Color::parse_hex(&s).ok_or_else(|| format!("invalid colour {s:?}"))
    }
}

// ---------------------------------------------------------------------------
// Objects
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Character formatting override. `None` inherits from the paragraph style
/// and the box defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CharFmt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

impl CharFmt {
    pub fn is_default(&self) -> bool {
        *self == CharFmt::default()
    }
}

/// A run of characters `[start, end)` (char indices) with a format override.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    #[serde(flatten)]
    pub fmt: CharFmt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParaStyle {
    #[default]
    Body,
    Title,
    Subtitle,
    Heading1,
    Heading2,
}

impl ParaStyle {
    pub const ALL: [ParaStyle; 5] = [
        ParaStyle::Title,
        ParaStyle::Subtitle,
        ParaStyle::Heading1,
        ParaStyle::Heading2,
        ParaStyle::Body,
    ];

    /// Font size relative to the box's base size.
    pub fn size_factor(self) -> f32 {
        match self {
            ParaStyle::Body => 1.0,
            ParaStyle::Title => 1.8,
            ParaStyle::Subtitle => 1.25,
            ParaStyle::Heading1 => 1.45,
            ParaStyle::Heading2 => 1.2,
        }
    }

    pub fn bold(self) -> bool {
        matches!(self, ParaStyle::Title | ParaStyle::Heading1 | ParaStyle::Heading2)
    }

    pub fn italic(self) -> bool {
        matches!(self, ParaStyle::Subtitle)
    }

    pub fn label(self) -> &'static str {
        match self {
            ParaStyle::Body => "Body",
            ParaStyle::Title => "Title",
            ParaStyle::Subtitle => "Subtitle",
            ParaStyle::Heading1 => "Heading 1",
            ParaStyle::Heading2 => "Heading 2",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    #[default]
    None,
    Bullet,
    Number,
}

/// Deepest list/indent level (0-based).
pub const MAX_LEVEL: u8 = 4;

/// Paragraph format for one line of a text box (lines are separated by
/// `\n` in the text).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParaFmt {
    #[serde(default)]
    pub style: ParaStyle,
    #[serde(default)]
    pub list: ListKind,
    /// Outline level: indentation and bullet shape.
    #[serde(default)]
    pub level: u8,
}

impl ParaFmt {
    pub fn is_default(&self) -> bool {
        *self == ParaFmt::default()
    }
}

/// Fully resolved character appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub bold: bool,
    pub italic: bool,
    pub color: Color,
}

/// Bullet glyph for an outline level.
pub fn bullet_for_level(level: u8) -> char {
    ['•', '–', '◦', '–', '•'][usize::from(level.min(MAX_LEVEL))]
}

/// List labels for each line (`""` for lines that are not list items).
/// Numbering restarts after any line that is not a numbered item at the
/// same or a deeper level.
pub fn list_labels(paras: &[ParaFmt]) -> Vec<String> {
    let mut counters = [0u32; MAX_LEVEL as usize + 1];
    paras
        .iter()
        .map(|p| {
            let l = usize::from(p.level.min(MAX_LEVEL));
            // Deeper levels restart below any item.
            for c in counters.iter_mut().skip(l + 1) {
                *c = 0;
            }
            match p.list {
                ListKind::None => {
                    counters.iter_mut().for_each(|c| *c = 0);
                    String::new()
                }
                ListKind::Bullet => {
                    counters[l] = 0;
                    bullet_for_level(p.level).to_string()
                }
                ListKind::Number => {
                    counters[l] += 1;
                    format!("{}.", counters[l])
                }
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    /// Unicode text with LaTeX math: `$…$` inline, `$$…$$` display (see
    /// [`crate::rich`]). Line breaks (`\n`) separate paragraphs; long lines
    /// wrap at the box width. Plain text is exported with LaTeX specials
    /// escaped; math is passed to LaTeX as written.
    pub text: String,
    /// Base font size in bp (paragraph styles scale it).
    pub font_size: f32,
    pub color: Color,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub align: TextAlign,
    /// Character formatting overrides (sorted, non-overlapping).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
    /// Paragraph formats by line; missing entries are the default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paras: Vec<ParaFmt>,
    /// Hint shown while the box is empty (layout placeholders). Placeholder
    /// boxes stay when empty and are not exported until they have text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl Default for TextBox {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_size: 20.0,
            color: Color::BLACK,
            bold: false,
            italic: false,
            align: TextAlign::Left,
            spans: Vec::new(),
            paras: Vec::new(),
            placeholder: None,
        }
    }
}

impl TextBox {
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn line_count(&self) -> usize {
        self.text.chars().filter(|c| *c == '\n').count() + 1
    }

    /// Line index of char index `i`.
    pub fn line_of(&self, i: usize) -> usize {
        self.text.chars().take(i).filter(|c| *c == '\n').count()
    }

    pub fn para(&self, line: usize) -> ParaFmt {
        self.paras.get(line).copied().unwrap_or_default()
    }

    /// Paragraph formats, one per line.
    pub fn paras_normalized(&self) -> Vec<ParaFmt> {
        let n = self.line_count();
        let mut v = self.paras.clone();
        v.resize(n, ParaFmt::default());
        v
    }

    pub fn set_paras(&mut self, mut v: Vec<ParaFmt>) {
        v.truncate(self.line_count());
        for p in &mut v {
            p.level = p.level.min(MAX_LEVEL);
        }
        while v.last().is_some_and(ParaFmt::is_default) {
            v.pop();
        }
        self.paras = v;
    }

    /// Format override of every char.
    pub fn char_formats(&self) -> Vec<CharFmt> {
        let n = self.char_len();
        let mut v = vec![CharFmt::default(); n];
        for sp in &self.spans {
            let (a, b) = (sp.start as usize, (sp.end as usize).min(n));
            for f in v.iter_mut().take(b).skip(a) {
                *f = sp.fmt;
            }
        }
        v
    }

    /// Store per-char formats as spans, dropping overrides that equal what
    /// the char would inherit anyway.
    pub fn set_char_formats(&mut self, fmts: &[CharFmt]) {
        let paras = self.paras_normalized();
        let mut line = 0usize;
        let mut spans: Vec<Span> = Vec::new();
        for (i, (c, f)) in self.text.chars().zip(fmts.iter()).enumerate() {
            let p = paras[line.min(paras.len() - 1)];
            let mut f = *f;
            if f.bold == Some(self.bold || p.style.bold()) {
                f.bold = None;
            }
            if f.italic == Some(self.italic || p.style.italic()) {
                f.italic = None;
            }
            if f.color == Some(self.color) {
                f.color = None;
            }
            if c == '\n' {
                line += 1;
                f = CharFmt::default();
            }
            if f.is_default() {
                continue;
            }
            match spans.last_mut() {
                Some(last) if last.end as usize == i && last.fmt == f => last.end += 1,
                _ => spans.push(Span {
                    start: i as u32,
                    end: i as u32 + 1,
                    fmt: f,
                }),
            }
        }
        self.spans = spans;
    }

    /// Appearance of a char with override `cf` in a paragraph `p`.
    pub fn resolve(&self, cf: CharFmt, p: ParaFmt) -> Resolved {
        Resolved {
            bold: cf.bold.unwrap_or(self.bold || p.style.bold()),
            italic: cf.italic.unwrap_or(self.italic || p.style.italic()),
            color: cf.color.unwrap_or(self.color),
        }
    }

    /// Replace chars `a..b` with `ins`, keeping character and paragraph
    /// formats aligned. Inserted chars take `fmt`, or the format of the char
    /// before the insertion point. New lines inherit the paragraph format of
    /// the line they were split from.
    pub fn replace(&mut self, a: usize, b: usize, ins: &str, fmt: Option<CharFmt>) {
        let n = self.char_len();
        let a = a.min(n);
        let b = b.clamp(a, n);
        let mut paras = self.paras_normalized();
        let line_a = self.line_of(a);
        let removed_nl = self.text.chars().skip(a).take(b - a).filter(|c| *c == '\n').count();
        let added_nl = ins.chars().filter(|c| *c == '\n').count();
        let keep = paras.get(line_a).copied().unwrap_or_default();
        let end = (line_a + 1 + removed_nl).min(paras.len());
        paras.drain((line_a + 1).min(end)..end);
        for _ in 0..added_nl {
            paras.insert(line_a + 1, keep);
        }

        let mut fm = self.char_formats();
        let fill = fmt.unwrap_or_else(|| {
            if a > 0 {
                fm[a - 1]
            } else {
                fm.get(b).copied().unwrap_or_default()
            }
        });
        let ins_n = ins.chars().count();
        fm.splice(a..b, std::iter::repeat_n(fill, ins_n));

        let ba = self
            .text
            .char_indices()
            .nth(a)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len());
        let bb = self
            .text
            .char_indices()
            .nth(b)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len());
        self.text.replace_range(ba..bb, ins);
        self.set_paras(paras);
        self.set_char_formats(&fm);
    }

    /// Apply `f` to the char formats of `a..b`.
    pub fn format_range(&mut self, a: usize, b: usize, f: impl Fn(&mut CharFmt)) {
        let mut fm = self.char_formats();
        let b = b.min(fm.len());
        for x in fm.iter_mut().take(b).skip(a.min(b)) {
            f(x);
        }
        self.set_char_formats(&fm);
    }

    /// Apply `f` to the paragraph formats of lines `l0..=l1`.
    pub fn format_lines(&mut self, l0: usize, l1: usize, f: impl Fn(&mut ParaFmt)) {
        let mut paras = self.paras_normalized();
        let l1 = l1.min(paras.len() - 1);
        for p in paras.iter_mut().take(l1 + 1).skip(l0.min(l1)) {
            f(p);
        }
        // Paragraph defaults may change what the char overrides mean.
        let fm = self.char_formats();
        self.set_paras(paras);
        self.set_char_formats(&fm);
    }

    /// Effective appearance at char `i` (for toolbar state).
    pub fn resolved_at(&self, i: usize) -> Resolved {
        let fm = self.char_formats();
        let cf = fm.get(i).copied().unwrap_or_default();
        self.resolve(cf, self.para(self.line_of(i)))
    }

    /// Largest font size used by any paragraph (bp).
    pub fn max_size(&self) -> f32 {
        self.paras_normalized()
            .iter()
            .map(|p| self.font_size * p.style.size_factor())
            .fold(self.font_size, f32::max)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub asset: AssetId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Rectangle,
    RoundedRectangle,
    Ellipse,
    Triangle,
    Diamond,
    Cube,
    Cylinder,
    Bevel,
    /// PowerPoint's "Right Arrow" block shape (outline + fill).
    BlockArrow,
    Line,
    Arrow,
    /// An open, normalized vector path produced by the pen tool.
    Freehand,
}

/// Outline of a right-pointing block arrow inside `(x, y, w, h)`:
/// shaft half as tall as the box, head as long as the box is tall (at most
/// half its width). Shared by the editor and the TikZ export.
pub fn block_arrow_points(x: f32, y: f32, w: f32, h: f32) -> [(f32, f32); 7] {
    let head = h.min(w * 0.5);
    let (cy, sh) = (y + h * 0.5, h * 0.25);
    let (r, neck) = (x + w, x + w - head);
    [
        (x, cy - sh),
        (neck, cy - sh),
        (neck, y),
        (r, cy),
        (neck, y + h),
        (neck, cy + sh),
        (x, cy + sh),
    ]
}

impl ShapeKind {
    pub fn is_linear(self) -> bool {
        matches!(self, ShapeKind::Line | ShapeKind::Arrow)
    }

    pub fn supports_fill(self) -> bool {
        !matches!(self, ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Freehand)
    }

    /// Comfortable click-to-insert dimensions in slide-space points.
    pub fn default_size(self) -> (f32, f32) {
        match self {
            ShapeKind::Ellipse => (60.0, 60.0),
            ShapeKind::Cube => (80.0, 70.0),
            ShapeKind::Cylinder => (68.0, 76.0),
            ShapeKind::BlockArrow => (90.0, 30.0),
            _ => (90.0, 55.0),
        }
    }
}

/// For lines and arrows: which diagonal of the bounding rect the segment
/// follows. `Down` runs top-left → bottom-right, `Up` runs bottom-left →
/// top-right. `reversed` swaps the start and end points (arrow direction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Diagonal {
    #[default]
    Down,
    Up,
}

/// Outline dash style (PowerPoint's "Dash type").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashStyle {
    #[default]
    Solid,
    RoundDot,
    SquareDot,
    Dash,
    DashDot,
    LongDash,
    LongDashDot,
}

impl DashStyle {
    pub const ALL: [DashStyle; 7] = [
        DashStyle::Solid,
        DashStyle::RoundDot,
        DashStyle::SquareDot,
        DashStyle::Dash,
        DashStyle::DashDot,
        DashStyle::LongDash,
        DashStyle::LongDashDot,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DashStyle::Solid => "Solid",
            DashStyle::RoundDot => "Round dot",
            DashStyle::SquareDot => "Square dot",
            DashStyle::Dash => "Dash",
            DashStyle::DashDot => "Dash dot",
            DashStyle::LongDash => "Long dash",
            DashStyle::LongDashDot => "Long dash dot",
        }
    }

    /// Dash pattern as (on, off) pairs in bp for a line of width `w`
    /// (empty = solid). Shared by the editor and the TikZ export.
    pub fn pattern(self, w: f32) -> Vec<(f32, f32)> {
        let u = w.max(0.75);
        match self {
            DashStyle::Solid => vec![],
            // Zero-length dashes with round caps draw dots.
            DashStyle::RoundDot => vec![(0.0, 2.0 * u)],
            DashStyle::SquareDot => vec![(u, u)],
            DashStyle::Dash => vec![(4.0 * u, 3.0 * u)],
            DashStyle::DashDot => vec![(4.0 * u, 3.0 * u), (u, 3.0 * u)],
            DashStyle::LongDash => vec![(8.0 * u, 3.0 * u)],
            DashStyle::LongDashDot => vec![(8.0 * u, 3.0 * u), (u, 3.0 * u)],
        }
    }

    pub fn round_caps(self) -> bool {
        self == DashStyle::RoundDot
    }
}

/// Decoration at the start or end of a line (PowerPoint's arrow types).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnd {
    #[default]
    None,
    /// Filled triangle.
    Triangle,
    /// Open "V" arrow.
    Open,
    /// Filled arrow with a notched back.
    Stealth,
    Diamond,
    /// Filled dot.
    Circle,
    /// Perpendicular bar.
    Bar,
}

impl LineEnd {
    pub const ALL: [LineEnd; 7] = [
        LineEnd::None,
        LineEnd::Triangle,
        LineEnd::Open,
        LineEnd::Stealth,
        LineEnd::Diamond,
        LineEnd::Circle,
        LineEnd::Bar,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LineEnd::None => "None",
            LineEnd::Triangle => "Arrow",
            LineEnd::Open => "Open arrow",
            LineEnd::Stealth => "Stealth arrow",
            LineEnd::Diamond => "Diamond",
            LineEnd::Circle => "Oval",
            LineEnd::Bar => "Bar",
        }
    }

    /// (length along the line, width across it) in bp for stroke width `w`.
    pub fn size(self, w: f32) -> (f32, f32) {
        let len = 3.0 + 3.0 * w;
        match self {
            LineEnd::None => (0.0, 0.0),
            LineEnd::Circle => (len * 0.7, len * 0.7),
            LineEnd::Bar => (0.0, len),
            LineEnd::Diamond => (len, len * 0.7),
            _ => (len, len * 0.8),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    pub stroke: Color,
    /// Stroke width in bp; 0 means no outline.
    pub stroke_width: f32,
    pub fill: Option<Color>,
    #[serde(default)]
    pub diagonal: Diagonal,
    #[serde(default)]
    pub reversed: bool,
    /// Outline dash style.
    #[serde(default)]
    pub dash: DashStyle,
    /// Line decorations at the start and end (lines and arrows).
    #[serde(default)]
    pub start_tip: LineEnd,
    #[serde(default)]
    pub end_tip: LineEnd,
    /// Points normalized into the object's rectangle. Used only by
    /// [`ShapeKind::Freehand`], and omitted for every ordinary shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<(f32, f32)>,
}

impl Shape {
    pub fn new(kind: ShapeKind) -> Self {
        let fill = match kind {
            ShapeKind::Rectangle
            | ShapeKind::RoundedRectangle
            | ShapeKind::Ellipse
            | ShapeKind::Triangle
            | ShapeKind::Diamond
            | ShapeKind::Cube
            | ShapeKind::Cylinder
            | ShapeKind::Bevel
            | ShapeKind::BlockArrow => Some(Color::LIGHT_FILL),
            _ => None,
        };
        Self {
            kind,
            stroke: Color::ACCENT,
            stroke_width: 1.5,
            fill,
            diagonal: Diagonal::Down,
            reversed: false,
            dash: DashStyle::Solid,
            start_tip: LineEnd::None,
            end_tip: if kind == ShapeKind::Arrow {
                LineEnd::Triangle
            } else {
                LineEnd::None
            },
            points: Vec::new(),
        }
    }

    /// Start and end point (slide space) for linear shapes.
    pub fn endpoints(&self, r: &Rect) -> ((f32, f32), (f32, f32)) {
        let (a, b) = match self.diagonal {
            Diagonal::Down => ((r.x, r.y), (r.right(), r.bottom())),
            Diagonal::Up => ((r.x, r.bottom()), (r.right(), r.y)),
        };
        if self.reversed { (b, a) } else { (a, b) }
    }

    /// Set the bounding rect / diagonal / direction from two endpoints.
    pub fn set_endpoints(&mut self, start: (f32, f32), end: (f32, f32)) -> Rect {
        let down = (end.0 - start.0) * (end.1 - start.1) >= 0.0;
        if down {
            // Canonical start is the top-left corner. With a zero delta on one
            // axis (horizontal/vertical line) only the other axis decides.
            self.diagonal = Diagonal::Down;
            self.reversed = start.0 > end.0 || start.1 > end.1;
        } else {
            // Canonical start is the bottom-left corner; neither delta is 0.
            self.diagonal = Diagonal::Up;
            self.reversed = start.0 > end.0;
        }
        Rect::from_points(start, end)
    }

    /// Store an absolute slide-space freehand path as normalized points and
    /// return its resizeable bounding rectangle.
    pub fn set_freehand_points(&mut self, points: &[(f32, f32)]) -> Option<Rect> {
        if self.kind != ShapeKind::Freehand || points.len() < 2 {
            return None;
        }
        let clean: Vec<_> = points
            .iter()
            .copied()
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .take(MAX_FREEHAND_POINTS)
            .collect();
        if clean.len() < 2 {
            return None;
        }
        let (mut min_x, mut min_y) = clean[0];
        let (mut max_x, mut max_y) = clean[0];
        for &(x, y) in &clean[1..] {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        let raw_w = max_x - min_x;
        let raw_h = max_y - min_y;
        let w = raw_w.max(MIN_EXTENT);
        let h = raw_h.max(MIN_EXTENT);
        let x = min_x - (w - raw_w) * 0.5;
        let y = min_y - (h - raw_h) * 0.5;
        self.points = clean.into_iter().map(|(px, py)| ((px - x) / w, (py - y) / h)).collect();
        Some(Rect::new(x, y, w, h))
    }

    pub fn freehand_points(&self, rect: &Rect) -> Vec<(f32, f32)> {
        self.points
            .iter()
            .map(|&(x, y)| (rect.x + x * rect.w, rect.y + y * rect.h))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ObjectKind {
    Text(TextBox),
    Image(ImageRef),
    Shape(Shape),
}

impl ObjectKind {
    pub fn label(&self) -> &'static str {
        match self {
            ObjectKind::Text(_) => "Text",
            ObjectKind::Image(_) => "Image",
            ObjectKind::Shape(s) => match s.kind {
                ShapeKind::Rectangle => "Rectangle",
                ShapeKind::RoundedRectangle => "Rounded Rectangle",
                ShapeKind::Ellipse => "Ellipse",
                ShapeKind::Triangle => "Triangle",
                ShapeKind::Diamond => "Diamond",
                ShapeKind::Cube => "Cube",
                ShapeKind::Cylinder => "Cylinder",
                ShapeKind::Bevel => "Bevel",
                ShapeKind::BlockArrow => "Block Arrow",
                ShapeKind::Line => "Line",
                ShapeKind::Arrow => "Arrow",
                ShapeKind::Freehand => "Drawing",
            },
        }
    }

    /// Objects whose aspect ratio is preserved when resized from a corner.
    pub fn keeps_aspect(&self) -> bool {
        matches!(self, ObjectKind::Image(_))
    }

    /// Rough heap footprint, used for bounding undo history.
    pub fn approx_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                ObjectKind::Text(t) => t.text.len() + t.spans.len() * 16 + t.paras.len() * 4,
                ObjectKind::Image(i) => i.asset.0.len(),
                ObjectKind::Shape(s) => s.points.len() * std::mem::size_of::<(f32, f32)>(),
            }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub rect: Rect,
    #[serde(flatten)]
    pub kind: ObjectKind,
}

impl Object {
    pub fn approx_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.kind.approx_bytes()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slide {
    pub id: SlideId,
    /// Paint order: index 0 is at the back.
    pub objects: Vec<Object>,
    #[serde(default = "default_background")]
    pub background: Color,
}

fn default_background() -> Color {
    Color::WHITE
}

impl Slide {
    pub fn new(id: SlideId) -> Self {
        Self {
            id,
            objects: Vec::new(),
            background: Color::WHITE,
        }
    }

    pub fn index_of(&self, id: ObjectId) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
    }

    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn approx_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.objects.iter().map(Object::approx_bytes).sum::<usize>()
    }
}

/// Stock presentation themes shipped with Beamer. The editor previews their
/// structural chrome; exported TeX uses the corresponding `\usetheme` name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeamerTheme {
    #[default]
    Default,
    Madrid,
    Boadilla,
    CambridgeUs,
    Berlin,
    Copenhagen,
}

impl BeamerTheme {
    pub const ALL: [Self; 6] = [
        Self::Default,
        Self::Madrid,
        Self::Boadilla,
        Self::CambridgeUs,
        Self::Berlin,
        Self::Copenhagen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Madrid => "Madrid",
            Self::Boadilla => "Boadilla",
            Self::CambridgeUs => "CambridgeUS",
            Self::Berlin => "Berlin",
            Self::Copenhagen => "Copenhagen",
        }
    }

    pub fn latex_name(self) -> Option<&'static str> {
        (self != Self::Default).then(|| self.label())
    }
}

/// Document-level metadata and settings that are not tied to one slide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct DocMeta {
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub theme: BeamerTheme,
    /// Extra LaTeX preamble (e.g. `\newcommand` macros). Used both for
    /// equation previews and for the exported Beamer file.
    #[serde(default)]
    pub preamble: String,
}

/// A named divider anchored immediately before a slide. Anchoring by slide
/// id keeps the divider attached when slides are reordered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub start: SlideId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub meta: DocMeta,
    pub slides: Vec<Slide>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    /// Next free id for slides and objects (shared counter).
    pub next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// A new document with one empty slide.
    pub fn new() -> Self {
        let mut d = Self {
            meta: DocMeta {
                title: "Untitled presentation".into(),
                ..Default::default()
            },
            slides: Vec::new(),
            sections: Vec::new(),
            next_id: 1,
        };
        let id = d.alloc_slide_id();
        d.slides.push(Slide::new(id));
        d
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn alloc_object_id(&mut self) -> ObjectId {
        ObjectId(self.alloc_id())
    }
    pub fn alloc_slide_id(&mut self) -> SlideId {
        SlideId(self.alloc_id())
    }

    pub fn slide_index(&self, id: SlideId) -> Option<usize> {
        self.slides.iter().position(|s| s.id == id)
    }
    pub fn slide(&self, id: SlideId) -> Option<&Slide> {
        self.slides.iter().find(|s| s.id == id)
    }
    pub fn slide_mut(&mut self, id: SlideId) -> Option<&mut Slide> {
        self.slides.iter_mut().find(|s| s.id == id)
    }

    pub fn section_before(&self, slide: SlideId) -> Option<&Section> {
        self.sections.iter().find(|s| s.start == slide)
    }

    pub fn object_count(&self) -> usize {
        self.slides.iter().map(|s| s.objects.len()).sum()
    }

    /// All asset ids referenced by any slide, sorted and de-duplicated.
    pub fn referenced_assets(&self) -> Vec<AssetId> {
        let mut v: Vec<AssetId> = self
            .slides
            .iter()
            .flat_map(|s| s.objects.iter())
            .filter_map(|o| match &o.kind {
                ObjectKind::Image(i) => Some(i.asset.clone()),
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Deep copy of a slide with fresh ids for the slide and its objects.
    pub fn duplicate_slide(&mut self, src: &Slide) -> Slide {
        let id = self.alloc_slide_id();
        let objects = src
            .objects
            .iter()
            .map(|o| Object {
                id: self.alloc_object_id(),
                ..o.clone()
            })
            .collect();
        Slide {
            id,
            objects,
            background: src.background,
        }
    }

    /// Structural validation of untrusted (loaded) documents. Fixes what can
    /// be fixed safely (non-finite numbers, out-of-range sizes) and rejects
    /// what cannot (duplicate ids, invalid asset ids).
    pub fn validate_and_repair(&mut self) -> Result<(), String> {
        use std::collections::HashSet;
        if self.slides.is_empty() {
            let id = self.alloc_slide_id();
            self.slides.push(Slide::new(id));
        }
        let mut seen = HashSet::new();
        let mut max_id = 0u64;
        for s in &mut self.slides {
            if !seen.insert(s.id.0) {
                return Err(format!("duplicate id {}", s.id.0));
            }
            max_id = max_id.max(s.id.0);
            for o in &mut s.objects {
                if !seen.insert(o.id.0) {
                    return Err(format!("duplicate id {}", o.id.0));
                }
                max_id = max_id.max(o.id.0);
                o.rect = o.rect.sanitized();
                match &mut o.kind {
                    ObjectKind::Text(t) => {
                        t.font_size = sanitize_font(t.font_size);
                        // Re-derive spans/paras so they are sorted, in range and minimal.
                        let fm = t.char_formats();
                        let paras = t.paras_normalized();
                        t.set_paras(paras);
                        t.set_char_formats(&fm);
                    }
                    ObjectKind::Image(i) => {
                        if !i.asset.is_valid() {
                            return Err(format!("invalid asset id {:?}", i.asset.0));
                        }
                    }
                    ObjectKind::Shape(sh) => {
                        sh.stroke_width = if sh.stroke_width.is_finite() {
                            sh.stroke_width.clamp(0.0, 50.0)
                        } else {
                            1.0
                        };
                        if sh.kind == ShapeKind::Freehand {
                            sh.points.retain(|(x, y)| x.is_finite() && y.is_finite());
                            sh.points.truncate(MAX_FREEHAND_POINTS);
                            for (x, y) in &mut sh.points {
                                *x = x.clamp(0.0, 1.0);
                                *y = y.clamp(0.0, 1.0);
                            }
                        } else {
                            sh.points.clear();
                        }
                    }
                }
            }
        }
        let slide_order: std::collections::HashMap<SlideId, usize> = self
            .slides
            .iter()
            .enumerate()
            .map(|(index, slide)| (slide.id, index))
            .collect();
        let mut section_starts = HashSet::new();
        self.sections.retain_mut(|section| {
            if !slide_order.contains_key(&section.start) || !section_starts.insert(section.start) {
                return false;
            }
            section.name = section.name.trim().chars().take(200).collect();
            if section.name.is_empty() {
                section.name = "Untitled Section".into();
            }
            true
        });
        self.sections.sort_by_key(|section| slide_order[&section.start]);
        if self.next_id <= max_id {
            self.next_id = max_id + 1;
        }
        Ok(())
    }
}

pub fn sanitize_font(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        20.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slide_is_beamer_169() {
        // 16 cm x 9 cm in bp.
        assert!((SLIDE_W - 453.5433).abs() < 1e-3);
        assert!((SLIDE_H - 255.1181).abs() < 1e-3);
    }

    #[test]
    fn color_hex_roundtrip() {
        for c in [Color::BLACK, Color::rgba(1, 2, 3, 4), Color::rgb(255, 128, 0)] {
            assert_eq!(Color::parse_hex(&c.to_hex()), Some(c));
        }
        assert_eq!(Color::parse_hex("#12345"), None);
        assert_eq!(Color::parse_hex("123456"), None);
        assert_eq!(Color::parse_hex("#gg0000"), None);
    }

    #[test]
    fn asset_id_validation() {
        assert!(AssetId("a".repeat(64)).is_valid());
        assert!(!AssetId("A".repeat(64)).is_valid());
        assert!(!AssetId("../../etc/passwd".into()).is_valid());
        assert!(!AssetId("a".repeat(63)).is_valid());
    }

    #[test]
    fn line_endpoints_roundtrip() {
        let mut s = Shape::new(ShapeKind::Arrow);
        for (a, b) in [
            ((0.0, 0.0), (10.0, 5.0)),
            ((10.0, 5.0), (0.0, 0.0)),
            ((0.0, 5.0), (10.0, 0.0)),
            ((10.0, 0.0), (0.0, 5.0)),
        ] {
            let r = s.set_endpoints(a, b);
            assert_eq!(s.endpoints(&r), (a, b), "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn freehand_points_normalize_and_follow_resizes() {
        let source = [(10.0, 20.0), (30.0, 25.0), (50.0, 10.0)];
        let mut shape = Shape::new(ShapeKind::Freehand);
        let rect = shape.set_freehand_points(&source).unwrap();
        let restored = shape.freehand_points(&rect);
        for (actual, expected) in restored.iter().zip(source) {
            assert!((actual.0 - expected.0).abs() < 1e-4);
            assert!((actual.1 - expected.1).abs() < 1e-4);
        }
        let resized = Rect::new(rect.x, rect.y, rect.w * 2.0, rect.h * 3.0);
        let scaled = shape.freehand_points(&resized);
        assert!((scaled[2].0 - (rect.x + (source[2].0 - rect.x) * 2.0)).abs() < 1e-3);
        assert!((scaled[2].1 - (rect.y + (source[2].1 - rect.y) * 3.0)).abs() < 1e-3);
    }

    #[test]
    fn validation_repairs_section_names_and_anchors() {
        let mut doc = Document::new();
        let first = doc.slides[0].id;
        doc.sections = vec![
            Section {
                name: "  Results  ".into(),
                start: first,
            },
            Section {
                name: "duplicate".into(),
                start: first,
            },
            Section {
                name: "missing".into(),
                start: SlideId(999),
            },
        ];
        doc.validate_and_repair().unwrap();
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].name, "Results");
    }

    fn tb(text: &str) -> TextBox {
        TextBox {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn spans_follow_edits() {
        let mut t = tb("hello world");
        t.format_range(6, 11, |f| f.bold = Some(true));
        assert_eq!(
            t.spans,
            vec![Span {
                start: 6,
                end: 11,
                fmt: CharFmt {
                    bold: Some(true),
                    ..Default::default()
                }
            }]
        );
        // Typing inside the bold word continues bold; before it, plain.
        t.replace(8, 8, "XY", None);
        assert_eq!(t.text, "hello woXYrld");
        assert_eq!((t.spans[0].start, t.spans[0].end), (6, 13));
        t.replace(0, 0, "Oh ", None);
        assert_eq!((t.spans[0].start, t.spans[0].end), (9, 16));
        // Deleting across the boundary shrinks the span.
        t.replace(5, 11, "", None);
        assert_eq!(t.text, "Oh heXYrld");
        assert_eq!((t.spans[0].start, t.spans[0].end), (5, 10));
        // Bold equal to the inherited value is not stored.
        t.bold = true;
        t.format_range(0, t.char_len(), |f| f.bold = Some(true));
        assert!(t.spans.is_empty());
    }

    #[test]
    fn paragraph_formats_follow_newlines() {
        let mut t = tb("a\nb");
        t.format_lines(0, 0, |p| p.style = ParaStyle::Title);
        t.format_lines(1, 1, |p| p.list = ListKind::Bullet);
        // Enter at the end of the title starts another title paragraph.
        t.replace(1, 1, "\n", None);
        assert_eq!(t.text, "a\n\nb");
        assert_eq!(t.para(1).style, ParaStyle::Title);
        assert_eq!(t.para(2).list, ListKind::Bullet);
        // Joining lines keeps the first line's format.
        t.replace(1, 3, "", None);
        assert_eq!(t.text, "ab");
        assert_eq!(
            t.paras_normalized(),
            vec![ParaFmt {
                style: ParaStyle::Title,
                ..Default::default()
            }]
        );
    }

    #[test]
    fn titles_are_bold_by_style_and_overridable() {
        let mut t = tb("Title");
        t.format_lines(0, 0, |p| p.style = ParaStyle::Title);
        assert!(t.resolved_at(0).bold);
        t.format_range(0, 2, |f| f.bold = Some(false));
        assert!(!t.resolved_at(0).bold && t.resolved_at(3).bold);
        assert!((t.max_size() - 36.0).abs() < 1e-3);
    }

    #[test]
    fn list_labels_number_and_nest() {
        let p = |list, level| ParaFmt {
            list,
            level,
            ..Default::default()
        };
        let labels = list_labels(&[
            p(ListKind::Number, 0),
            p(ListKind::Number, 1),
            p(ListKind::Number, 1),
            p(ListKind::Number, 0),
            p(ListKind::Bullet, 1),
            p(ListKind::None, 0),
            p(ListKind::Number, 0),
        ]);
        assert_eq!(labels, ["1.", "1.", "2.", "2.", "–", "", "1."]);
    }

    #[test]
    fn validate_rejects_duplicate_ids_and_repairs_numbers() {
        let mut d = Document::new();
        let sid = d.slides[0].id;
        d.slides[0].objects.push(Object {
            id: ObjectId(sid.0),
            rect: Rect::new(f32::NAN, 0.0, 1.0, 1.0),
            kind: ObjectKind::Text(TextBox::default()),
        });
        assert!(d.validate_and_repair().is_err());

        let mut d = Document::new();
        d.slides[0].objects.push(Object {
            id: ObjectId(99),
            rect: Rect::new(f32::NAN, 1e9, -5.0, 1.0),
            kind: ObjectKind::Text(TextBox {
                font_size: f32::INFINITY,
                ..Default::default()
            }),
        });
        d.validate_and_repair().unwrap();
        let o = &d.slides[0].objects[0];
        assert_eq!(o.rect, Rect::new(0.0, COORD_LIMIT, 0.0, 1.0));
        assert!(d.next_id > 99);
    }
}
