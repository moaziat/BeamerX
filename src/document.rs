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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    /// Unicode text with LaTeX math: `$…$` inline, `$$…$$` display (see
    /// [`crate::rich`]). Line breaks are hard breaks; long lines wrap at the
    /// box width. Plain text is exported with LaTeX specials escaped; math is
    /// passed to LaTeX as written.
    pub text: String,
    /// Font size in bp.
    pub font_size: f32,
    pub color: Color,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub align: TextAlign,
    /// Render each non-empty line as a bullet point.
    #[serde(default)]
    pub bullets: bool,
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
            bullets: false,
        }
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
    Ellipse,
    Line,
    Arrow,
}

impl ShapeKind {
    pub fn is_linear(self) -> bool {
        matches!(self, ShapeKind::Line | ShapeKind::Arrow)
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
}

impl Shape {
    pub fn new(kind: ShapeKind) -> Self {
        let fill = match kind {
            ShapeKind::Rectangle | ShapeKind::Ellipse => Some(Color::LIGHT_FILL),
            _ => None,
        };
        Self {
            kind,
            stroke: Color::ACCENT,
            stroke_width: 1.5,
            fill,
            diagonal: Diagonal::Down,
            reversed: false,
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
                ShapeKind::Ellipse => "Ellipse",
                ShapeKind::Line => "Line",
                ShapeKind::Arrow => "Arrow",
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
                ObjectKind::Text(t) => t.text.len(),
                ObjectKind::Image(i) => i.asset.0.len(),
                ObjectKind::Shape(_) => 0,
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

/// Document-level metadata and settings that are not tied to one slide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct DocMeta {
    pub title: String,
    #[serde(default)]
    pub author: String,
    /// Extra LaTeX preamble (e.g. `\newcommand` macros). Used both for
    /// equation previews and for the exported Beamer file.
    #[serde(default)]
    pub preamble: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub meta: DocMeta,
    pub slides: Vec<Slide>,
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
                    ObjectKind::Text(t) => t.font_size = sanitize_font(t.font_size),
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
                    }
                }
            }
        }
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
