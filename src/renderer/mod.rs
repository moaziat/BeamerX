//! Paints slides into an egui `Painter`.
//!
//! The same code draws the editing canvas, the slide thumbnails and the
//! full-screen presentation, at different scales. Geometry is converted from
//! slide space (bp) to screen points only here, through [`View`].

pub mod text_layout;
pub mod textures;

use crate::assets::AssetStore;
use crate::document::{
    BeamerTheme, Color, DashStyle, ImageRef, LineEnd, Object, ObjectId, ObjectKind, Rect, Shape, ShapeKind, Slide,
    TextBox,
};
use crate::math::MathService;
use egui::{Color32, FontFamily, FontId, Painter, Pos2, Stroke, StrokeKind, Vec2, pos2, vec2};
use std::sync::Arc;
use std::time::Instant;
use textures::TextureCache;

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

const LM_REGULAR: &[u8] = include_bytes!("../../assets/fonts/lmsans10-regular.otf");
const LM_BOLD: &[u8] = include_bytes!("../../assets/fonts/lmsans10-bold.otf");
const LM_OBLIQUE: &[u8] = include_bytes!("../../assets/fonts/lmsans10-oblique.otf");
const LM_BOLD_OBLIQUE: &[u8] = include_bytes!("../../assets/fonts/lmsans10-boldoblique.otf");

const FAMILIES: [(&str, &[u8]); 4] = [
    ("lmsans-r", LM_REGULAR),
    ("lmsans-b", LM_BOLD),
    ("lmsans-i", LM_OBLIQUE),
    ("lmsans-bi", LM_BOLD_OBLIQUE),
];

/// Register Latin Modern Sans (the font TeX uses for exported text) as
/// named families, falling back to egui's default fonts for missing glyphs.
pub fn install_fonts(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    let fallback = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (name, bytes) in FAMILIES {
        defs.font_data
            .insert(name.to_owned(), Arc::new(egui::FontData::from_static(bytes)));
        let mut chain = vec![name.to_owned()];
        chain.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name(name.into()), chain);
    }
    crate::ui::add_system_ui_font(&mut defs);
    ctx.set_fonts(defs);
}

pub fn text_family(bold: bool, italic: bool) -> FontFamily {
    FontFamily::Name(
        match (bold, italic) {
            (false, false) => "lmsans-r",
            (true, false) => "lmsans-b",
            (false, true) => "lmsans-i",
            (true, true) => "lmsans-bi",
        }
        .into(),
    )
}

pub fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

// ---------------------------------------------------------------------------
// View transform
// ---------------------------------------------------------------------------

/// Maps slide space (bp, y down) to screen points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen position of the slide's top-left corner.
    pub origin: Pos2,
    /// Screen points per bp.
    pub scale: f32,
}

impl View {
    pub fn pt(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x * self.scale, y * self.scale)
    }
    pub fn rect(&self, r: &Rect) -> egui::Rect {
        egui::Rect::from_min_size(self.pt(r.x, r.y), vec2(r.w * self.scale, r.h * self.scale))
    }
    pub fn to_slide(&self, p: Pos2) -> (f32, f32) {
        ((p.x - self.origin.x) / self.scale, (p.y - self.origin.y) / self.scale)
    }
    pub fn slide_rect(&self) -> egui::Rect {
        egui::Rect::from_min_size(
            self.origin,
            vec2(
                crate::document::SLIDE_W * self.scale,
                crate::document::SLIDE_H * self.scale,
            ),
        )
    }
    /// Fit the slide into `area` with a margin, centred.
    pub fn fit(area: egui::Rect, margin: f32) -> View {
        let avail = (area.size() - Vec2::splat(2.0 * margin)).max(Vec2::splat(10.0));
        let scale = (avail.x / crate::document::SLIDE_W).min(avail.y / crate::document::SLIDE_H);
        let size = vec2(crate::document::SLIDE_W, crate::document::SLIDE_H) * scale;
        View {
            origin: area.center() - size * 0.5,
            scale,
        }
    }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

pub struct Services<'a> {
    pub assets: &'a AssetStore,
    pub textures: &'a mut TextureCache,
    pub math: &'a mut MathService,
    pub now: Instant,
    /// When set, painted text boxes report their content height (bp), so
    /// the editor can keep auto-sized boxes in sync with rendered math.
    pub text_heights: Option<Vec<(ObjectId, f32)>>,
}

#[derive(Clone, Copy, Default)]
pub struct PaintOpts {
    /// Object being edited in place (its text is drawn by the editor).
    pub skip: Option<ObjectId>,
    /// Show editor-only hints (placeholders, loading states).
    pub editor_hints: bool,
    /// Presentation chrome to preview around slide content.
    pub theme: BeamerTheme,
}

pub fn paint_slide(p: &Painter, view: &View, slide: &Slide, sv: &mut Services, opts: PaintOpts) {
    let area = view.slide_rect();
    p.rect_filled(area, 0.0, c32(slide.background));
    paint_theme_chrome(p, view, opts.theme);
    let clip = p.clip_rect().intersect(area);
    let p = p.with_clip_rect(clip);
    for o in &slide.objects {
        // Cull objects outside the visible clip. The margin covers strokes
        // and arrow heads; text may overflow its box, so it is never culled.
        let r = view.rect(&o.rect);
        if !clip.expand(16.0).intersects(r) && !matches!(o.kind, ObjectKind::Text(_)) {
            continue;
        }
        if opts.skip == Some(o.id) {
            continue;
        }
        paint_object(&p, view, o, sv, opts);
    }
}

/// Paint a lightweight structural preview of a stock Beamer theme. This uses
/// only a handful of vector primitives, so thumbnails and canvas rendering
/// remain allocation-free apart from Berlin's tiny navigation markers.
pub fn paint_theme_chrome(p: &Painter, view: &View, theme: BeamerTheme) {
    let r = view.slide_rect();
    let h = |bp: f32| bp * view.scale;
    let navy = Color32::from_rgb(35, 55, 96);
    let blue = Color32::from_rgb(48, 93, 150);
    let pale_blue = Color32::from_rgb(217, 226, 243);
    let red = Color32::from_rgb(154, 39, 45);
    match theme {
        BeamerTheme::Default => {}
        BeamerTheme::Madrid => {
            p.rect_filled(
                egui::Rect::from_min_max(r.min, pos2(r.right(), r.top() + h(10.0))),
                0.0,
                blue,
            );
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.bottom() - h(8.0)), r.max),
                0.0,
                navy,
            );
        }
        BeamerTheme::Boadilla => {
            p.rect_filled(
                egui::Rect::from_min_max(r.min, pos2(r.right(), r.top() + h(2.0))),
                0.0,
                blue,
            );
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.bottom() - h(9.0)), r.max),
                0.0,
                Color32::from_rgb(235, 238, 244),
            );
            p.rect_filled(
                egui::Rect::from_min_max(
                    pos2(r.left(), r.bottom() - h(9.0)),
                    pos2(r.right(), r.bottom() - h(7.8)),
                ),
                0.0,
                blue,
            );
        }
        BeamerTheme::CambridgeUs => {
            p.rect_filled(
                egui::Rect::from_min_max(r.min, pos2(r.right(), r.top() + h(11.0))),
                0.0,
                red,
            );
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.top() + h(11.0)), pos2(r.right(), r.top() + h(14.0))),
                0.0,
                Color32::from_rgb(225, 205, 190),
            );
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.bottom() - h(8.0)), r.max),
                0.0,
                Color32::from_rgb(92, 30, 35),
            );
        }
        BeamerTheme::Berlin => {
            p.rect_filled(
                egui::Rect::from_min_max(r.min, pos2(r.right(), r.top() + h(14.0))),
                0.0,
                navy,
            );
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.top() + h(14.0)), pos2(r.right(), r.top() + h(17.0))),
                0.0,
                pale_blue,
            );
            let cy = r.top() + h(11.0);
            for i in 0..5 {
                p.circle_filled(pos2(r.left() + h(8.0 + i as f32 * 5.0), cy), h(1.15), Color32::WHITE);
            }
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.bottom() - h(7.0)), r.max),
                0.0,
                navy,
            );
        }
        BeamerTheme::Copenhagen => {
            p.rect_filled(
                egui::Rect::from_min_max(r.min, pos2(r.right(), r.top() + h(12.0))),
                0.0,
                navy,
            );
            p.rect_filled(
                egui::Rect::from_min_max(pos2(r.left(), r.top() + h(12.0)), pos2(r.right(), r.top() + h(18.0))),
                0.0,
                blue,
            );
        }
    }
}

pub fn paint_object(p: &Painter, view: &View, o: &Object, sv: &mut Services, opts: PaintOpts) {
    match &o.kind {
        ObjectKind::Text(t) => paint_text(p, view, o, t, sv, opts.editor_hints),
        ObjectKind::Image(i) => paint_image(p, view, &o.rect, i, sv, opts.editor_hints),
        ObjectKind::Shape(s) => paint_shape(p, view, &o.rect, s),
    }
}

/// Lay out a text box at `scale` (screen points per bp). Returns galleys with
/// their offsets (relative to the box's top-left, in screen points) and the
/// content height in bp.
/// Lay out a text box (with math) at `scale` screen points per bp.
pub fn layout_text(
    p: &Painter,
    sv: &mut Services,
    t: &TextBox,
    width_bp: f32,
    scale: f32,
    caret: Option<usize>,
) -> text_layout::TextLayout {
    text_layout::layout(
        p,
        sv.math,
        sv.now,
        &text_layout::LayoutInput {
            style: t,
            width_bp,
            scale,
            caret,
        },
    )
}

pub fn paint_text(p: &Painter, view: &View, o: &Object, t: &TextBox, sv: &mut Services, hints: bool) {
    let r = &o.rect;
    let origin = view.pt(r.x, r.y);
    if t.text.is_empty() {
        if hints {
            // Layout placeholders ("Click to add title") or a plain hint.
            let size = t.font_size * t.para(0).style.size_factor() * view.scale;
            let hint = t.placeholder.as_deref().unwrap_or("Click to add text");
            let x = match t.align {
                crate::document::TextAlign::Left => origin.x,
                crate::document::TextAlign::Center => origin.x + r.w * view.scale * 0.5,
                crate::document::TextAlign::Right => origin.x + r.w * view.scale,
            };
            let align = match t.align {
                crate::document::TextAlign::Left => egui::Align2::LEFT_TOP,
                crate::document::TextAlign::Center => egui::Align2::CENTER_TOP,
                crate::document::TextAlign::Right => egui::Align2::RIGHT_TOP,
            };
            p.text(
                egui::pos2(x, origin.y + size * 0.05),
                align,
                hint,
                FontId::new(size * 0.85, text_family(t.para(0).style.bold(), false)),
                Color32::from_gray(150),
            );
            if t.placeholder.is_some() {
                dashed_rect(p, view.rect(r).expand(2.0), Color32::from_gray(170));
            }
        }
        return;
    }
    let lay = layout_text(p, sv, t, r.w, view.scale, None);
    if let Some(h) = sv.text_heights.as_mut() {
        h.push((o.id, lay.height / view.scale));
    }
    text_layout::paint(
        p,
        origin,
        &lay,
        t.color,
        sv.textures,
        view.scale * p.pixels_per_point(),
        view.scale,
    );
}

fn paint_image(p: &Painter, view: &View, r: &Rect, i: &ImageRef, sv: &mut Services, hints: bool) {
    let dest = view.rect(r);
    let Some(meta) = sv.assets.get(&i.asset) else {
        paint_missing(p, dest, "Missing image");
        return;
    };
    if let Some(err) = sv.textures.image_error(&i.asset) {
        let msg = format!("Cannot display image: {err}");
        paint_missing(p, dest, &msg);
        return;
    }
    let px = dest.width().max(dest.height()) * p.pixels_per_point();
    let path = sv.assets.path_of(meta);
    match sv.textures.image(meta, path, px) {
        Some((tex, _)) => {
            p.image(
                tex,
                dest,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            if hints {
                p.rect_filled(dest, 0.0, Color32::from_gray(225));
            }
        }
    }
}

fn paint_missing(p: &Painter, dest: egui::Rect, msg: &str) {
    let red = Color32::from_rgb(200, 40, 40);
    p.rect_stroke(dest, 0.0, Stroke::new(1.0, red), StrokeKind::Inside);
    p.line_segment([dest.left_top(), dest.right_bottom()], Stroke::new(1.0, red));
    p.line_segment([dest.left_bottom(), dest.right_top()], Stroke::new(1.0, red));
    if dest.width() > 60.0 {
        p.text(
            dest.center(),
            egui::Align2::CENTER_CENTER,
            msg,
            FontId::proportional(11.0),
            red,
        );
    }
}

/// Dashed outline (layout placeholders).
fn dashed_rect(p: &Painter, r: egui::Rect, c: Color32) {
    let stroke = Stroke::new(1.0, c);
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    for w in pts.windows(2) {
        p.extend(egui::Shape::dashed_line(&[w[0], w[1]], stroke, 5.0, 4.0));
    }
}

/// Stroke a polyline (screen points) with a dash pattern given in bp.
/// The pattern runs continuously around corners; zero-length "on" phases
/// draw round dots.
pub fn stroke_path(
    p: &Painter,
    pts: &[Pos2],
    closed: bool,
    stroke: Stroke,
    dash: DashStyle,
    width_bp: f32,
    scale: f32,
) {
    if pts.len() < 2 || stroke.width <= 0.0 {
        return;
    }
    let pattern = dash.pattern(width_bp);
    if pattern.is_empty() {
        let shape = if closed {
            egui::Shape::closed_line(pts.to_vec(), stroke)
        } else {
            egui::Shape::line(pts.to_vec(), stroke)
        };
        p.add(shape);
        return;
    }
    let phases: Vec<(f32, bool)> = pattern
        .iter()
        .flat_map(|(on, off)| [(on * scale, true), (off * scale, false)])
        .collect();
    let mut segs: Vec<(Pos2, Pos2)> = pts.windows(2).map(|w| (w[0], w[1])).collect();
    if closed && pts.len() > 2 {
        segs.push((pts[pts.len() - 1], pts[0]));
    }
    let flush = |cur: &mut Vec<Pos2>| {
        if cur.len() >= 2 {
            p.add(egui::Shape::line(std::mem::take(cur), stroke));
        }
        cur.clear();
    };
    // Start "before" the first phase so the boundary logic enters it.
    let (mut pi, mut left, mut on) = (phases.len() - 1, 0.0f32, false);
    let mut cur: Vec<Pos2> = Vec::new();
    for (a, b) in segs {
        let d = b - a;
        let len = d.length();
        if len <= 1e-6 {
            continue;
        }
        let dir = d / len;
        let mut t = 0.0f32;
        if on && left > 0.0 && cur.is_empty() {
            cur.push(a);
        }
        loop {
            if left <= 1e-6 {
                if on {
                    flush(&mut cur);
                }
                pi = (pi + 1) % phases.len();
                (left, on) = phases[pi];
                let at = a + dir * t;
                if on {
                    if left <= 1e-6 {
                        if dash.round_caps() {
                            p.circle_filled(at, stroke.width * 0.5, stroke.color);
                        }
                    } else {
                        cur.push(at);
                    }
                }
                continue;
            }
            let step = left.min(len - t);
            t += step;
            left -= step;
            if on {
                cur.push(a + dir * t);
            }
            if t >= len - 1e-6 {
                break;
            }
        }
    }
    flush(&mut cur);
}

fn ellipse_points(c: Pos2, rx: f32, ry: f32, n: usize) -> Vec<Pos2> {
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            pos2(c.x + rx * a.cos(), c.y + ry * a.sin())
        })
        .collect()
}

fn rounded_rect_points(d: egui::Rect, radius: f32) -> Vec<Pos2> {
    let r = radius.min(d.width() * 0.5).min(d.height() * 0.5);
    if r <= 0.01 {
        return vec![d.left_top(), d.right_top(), d.right_bottom(), d.left_bottom()];
    }
    let corners = [
        (pos2(d.right() - r, d.top() + r), -90.0f32),
        (pos2(d.right() - r, d.bottom() - r), 0.0),
        (pos2(d.left() + r, d.bottom() - r), 90.0),
        (pos2(d.left() + r, d.top() + r), 180.0),
    ];
    let mut pts = Vec::with_capacity(4 * 9);
    for (c, start) in corners {
        for k in 0..=8 {
            let a = (start + 90.0 * k as f32 / 8.0).to_radians();
            pts.push(pos2(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    pts
}

/// Outline paths (screen points) of a closed shape: (points, closed).
fn shape_outlines(kind: ShapeKind, d: egui::Rect, r: &Rect, scale: f32) -> Vec<(Vec<Pos2>, bool)> {
    match kind {
        ShapeKind::Rectangle => vec![(
            vec![d.left_top(), d.right_top(), d.right_bottom(), d.left_bottom()],
            true,
        )],
        ShapeKind::RoundedRectangle => {
            vec![(rounded_rect_points(d, (r.w.min(r.h) * 0.18).min(7.0) * scale), true)]
        }
        ShapeKind::Ellipse => vec![(ellipse_points(d.center(), d.width() * 0.5, d.height() * 0.5, 96), true)],
        ShapeKind::Triangle => vec![(vec![d.center_top(), d.right_bottom(), d.left_bottom()], true)],
        ShapeKind::Diamond => vec![(
            vec![d.center_top(), d.right_center(), d.center_bottom(), d.left_center()],
            true,
        )],
        ShapeKind::Cube => {
            let depth = (d.width() * 0.24).min(d.height() * 0.28);
            let ftl = pos2(d.left(), d.top() + depth);
            let ftr = pos2(d.right() - depth, d.top() + depth);
            let fbr = pos2(d.right() - depth, d.bottom());
            vec![
                (vec![ftl, ftr, fbr, d.left_bottom()], true),
                (vec![ftl, pos2(d.left() + depth, d.top()), d.right_top(), ftr], false),
                (vec![d.right_top(), pos2(d.right(), d.bottom() - depth), fbr], false),
            ]
        }
        ShapeKind::Cylinder => {
            let cap = (d.height() * 0.24).min(d.width() * 0.3);
            let top = pos2(d.center().x, d.top() + cap * 0.5);
            let bottom = pos2(d.center().x, d.bottom() - cap * 0.5);
            vec![
                (vec![pos2(d.left(), top.y), pos2(d.left(), bottom.y)], false),
                (vec![pos2(d.right(), top.y), pos2(d.right(), bottom.y)], false),
                (ellipse_points(top, d.width() * 0.5, cap * 0.5, 72), true),
                (ellipse_points(bottom, d.width() * 0.5, cap * 0.5, 72), true),
            ]
        }
        ShapeKind::Bevel => {
            let inset = (d.width().min(d.height()) * 0.2).max(1.0);
            let i = d.shrink(inset);
            let mut v = vec![
                (
                    vec![d.left_top(), d.right_top(), d.right_bottom(), d.left_bottom()],
                    true,
                ),
                (
                    vec![i.left_top(), i.right_top(), i.right_bottom(), i.left_bottom()],
                    true,
                ),
            ];
            for (a, b) in [
                (d.left_top(), i.left_top()),
                (d.right_top(), i.right_top()),
                (d.right_bottom(), i.right_bottom()),
                (d.left_bottom(), i.left_bottom()),
            ] {
                v.push((vec![a, b], false));
            }
            v
        }
        ShapeKind::Line | ShapeKind::Arrow => Vec::new(),
    }
}

/// Draw a line-end decoration at `at`, pointing along `dir` (outwards).
/// Returns how far (screen points) the shaft should stop short of `at`.
pub fn paint_tip(p: &Painter, tip: LineEnd, at: Pos2, dir: Vec2, width_bp: f32, scale: f32, color: Color32) -> f32 {
    let (len, wid) = tip.size(width_bp);
    let (l, w) = (len * scale, wid * scale);
    let n = vec2(-dir.y, dir.x);
    let back = at - dir * l;
    match tip {
        LineEnd::None => 0.0,
        LineEnd::Triangle => {
            p.add(egui::Shape::convex_polygon(
                vec![at, back + n * w * 0.5, back - n * w * 0.5],
                color,
                Stroke::NONE,
            ));
            l * 0.6
        }
        LineEnd::Open => {
            let st = Stroke::new(width_bp * scale, color);
            p.add(egui::Shape::line(vec![back + n * w * 0.5, at, back - n * w * 0.5], st));
            width_bp * scale * 0.5
        }
        LineEnd::Stealth => {
            let inner = at - dir * l * 0.65;
            p.add(egui::Shape::convex_polygon(
                vec![at, back + n * w * 0.5, inner],
                color,
                Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![at, inner, back - n * w * 0.5],
                color,
                Stroke::NONE,
            ));
            l * 0.5
        }
        LineEnd::Diamond => {
            let mid = at - dir * l * 0.5;
            p.add(egui::Shape::convex_polygon(
                vec![at, mid + n * w * 0.5, back, mid - n * w * 0.5],
                color,
                Stroke::NONE,
            ));
            l * 0.6
        }
        LineEnd::Circle => {
            p.circle_filled(at - dir * l * 0.5, l * 0.5, color);
            l * 0.5
        }
        LineEnd::Bar => {
            p.line_segment(
                [at + n * w * 0.5, at - n * w * 0.5],
                Stroke::new(width_bp * scale, color),
            );
            0.0
        }
    }
}

pub fn paint_shape(p: &Painter, view: &View, r: &Rect, s: &Shape) {
    let sw = s.stroke_width * view.scale;
    let stroke = if s.stroke_width > 0.0 {
        Stroke::new(sw, c32(s.stroke))
    } else {
        Stroke::NONE
    };
    let fill = s.fill.map(c32).unwrap_or(Color32::TRANSPARENT);
    let d = view.rect(r);
    // Fills (with the 3-D shading faces); outlines are stroked below.
    match s.kind {
        ShapeKind::Rectangle | ShapeKind::RoundedRectangle => {
            let radius = if s.kind == ShapeKind::RoundedRectangle {
                (r.w.min(r.h) * 0.18).min(7.0) * view.scale
            } else {
                0.0
            };
            if s.fill.is_some() {
                p.rect_filled(d, radius, fill);
            }
        }
        ShapeKind::Ellipse => {
            if s.fill.is_some() {
                p.add(egui::Shape::ellipse_filled(d.center(), d.size() * 0.5, fill));
            }
        }
        ShapeKind::Triangle | ShapeKind::Diamond => {
            if s.fill.is_some() {
                let pts = if s.kind == ShapeKind::Triangle {
                    vec![d.center_top(), d.right_bottom(), d.left_bottom()]
                } else {
                    vec![d.center_top(), d.right_center(), d.center_bottom(), d.left_center()]
                };
                p.add(egui::Shape::convex_polygon(pts, fill, Stroke::NONE));
            }
        }
        ShapeKind::Cube => {
            let depth = (d.width() * 0.24).min(d.height() * 0.28);
            let front = vec![
                pos2(d.left(), d.top() + depth),
                pos2(d.right() - depth, d.top() + depth),
                pos2(d.right() - depth, d.bottom()),
                d.left_bottom(),
            ];
            let top = vec![
                pos2(d.left(), d.top() + depth),
                pos2(d.left() + depth, d.top()),
                d.right_top(),
                pos2(d.right() - depth, d.top() + depth),
            ];
            let side = vec![
                pos2(d.right() - depth, d.top() + depth),
                d.right_top(),
                pos2(d.right(), d.bottom() - depth),
                pos2(d.right() - depth, d.bottom()),
            ];
            p.add(egui::Shape::convex_polygon(front, fill, Stroke::NONE));
            p.add(egui::Shape::convex_polygon(top, fill.gamma_multiply(1.2), Stroke::NONE));
            p.add(egui::Shape::convex_polygon(
                side,
                fill.gamma_multiply(0.72),
                Stroke::NONE,
            ));
        }
        ShapeKind::Cylinder => {
            let cap = (d.height() * 0.24).min(d.width() * 0.3);
            let radius = vec2(d.width() * 0.5, cap * 0.5);
            let top = pos2(d.center().x, d.top() + cap * 0.5);
            let bottom = pos2(d.center().x, d.bottom() - cap * 0.5);
            p.rect_filled(
                egui::Rect::from_min_max(pos2(d.left(), top.y), pos2(d.right(), bottom.y)),
                0.0,
                fill,
            );
            p.add(egui::Shape::ellipse_filled(bottom, radius, fill.gamma_multiply(0.78)));
            p.add(egui::Shape::ellipse_filled(top, radius, fill.gamma_multiply(1.18)));
        }
        ShapeKind::Bevel => {
            let inset = (d.width().min(d.height()) * 0.2).max(1.0);
            let inner = d.shrink(inset);
            let faces = [
                (
                    vec![d.left_top(), d.right_top(), inner.right_top(), inner.left_top()],
                    1.22,
                ),
                (
                    vec![d.right_top(), d.right_bottom(), inner.right_bottom(), inner.right_top()],
                    0.72,
                ),
                (
                    vec![
                        d.right_bottom(),
                        d.left_bottom(),
                        inner.left_bottom(),
                        inner.right_bottom(),
                    ],
                    0.62,
                ),
                (
                    vec![d.left_bottom(), d.left_top(), inner.left_top(), inner.left_bottom()],
                    0.92,
                ),
            ];
            for (pts, k) in faces {
                p.add(egui::Shape::convex_polygon(pts, fill.gamma_multiply(k), Stroke::NONE));
            }
            p.rect_filled(inner, 0.0, fill);
        }
        ShapeKind::Line | ShapeKind::Arrow => {
            if s.stroke_width <= 0.0 {
                return;
            }
            let (a, b) = s.endpoints(r);
            let (a, b) = (view.pt(a.0, a.1), view.pt(b.0, b.1));
            let dvec = b - a;
            let len = dvec.length();
            if len <= 0.01 {
                return;
            }
            let dir = dvec / len;
            let color = c32(s.stroke);
            // Tips are solid; the shaft stops inside filled tips.
            let cut_end = paint_tip(p, s.end_tip, b, dir, s.stroke_width, view.scale, color).min(len * 0.45);
            let cut_start = paint_tip(p, s.start_tip, a, -dir, s.stroke_width, view.scale, color).min(len * 0.45);
            stroke_path(
                p,
                &[a + dir * cut_start, b - dir * cut_end],
                false,
                stroke,
                s.dash,
                s.stroke_width,
                view.scale,
            );
            return;
        }
    }
    if s.stroke_width > 0.0 {
        for (pts, closed) in shape_outlines(s.kind, d, r, view.scale) {
            stroke_path(p, &pts, closed, stroke, s.dash, s.stroke_width, view.scale);
        }
    }
}
