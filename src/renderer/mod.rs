//! Paints slides into an egui `Painter`.
//!
//! The same code draws the editing canvas, the slide thumbnails and the
//! full-screen presentation, at different scales. Geometry is converted from
//! slide space (bp) to screen points only here, through [`View`].

pub mod text_layout;
pub mod textures;

use crate::assets::AssetStore;
use crate::document::{
    BeamerTheme, Color, ImageRef, Object, ObjectId, ObjectKind, Rect, Shape, ShapeKind, Slide, TextBox,
};
use crate::export::arrow_head;
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

pub fn paint_shape(p: &Painter, view: &View, r: &Rect, s: &Shape) {
    let sw = s.stroke_width * view.scale;
    let stroke = if s.stroke_width > 0.0 {
        Stroke::new(sw, c32(s.stroke))
    } else {
        Stroke::NONE
    };
    match s.kind {
        ShapeKind::Rectangle | ShapeKind::RoundedRectangle => {
            let dest = view.rect(r);
            let radius = if s.kind == ShapeKind::RoundedRectangle {
                (r.w.min(r.h) * 0.18).min(7.0) * view.scale
            } else {
                0.0
            };
            if let Some(f) = s.fill {
                p.rect_filled(dest, radius, c32(f));
            }
            if s.stroke_width > 0.0 {
                p.rect_stroke(dest, radius, stroke, StrokeKind::Middle);
            }
        }
        ShapeKind::Ellipse => {
            let dest = view.rect(r);
            let radius = dest.size() * 0.5;
            if let Some(f) = s.fill {
                p.add(egui::Shape::ellipse_filled(dest.center(), radius, c32(f)));
            }
            if s.stroke_width > 0.0 {
                p.add(egui::Shape::ellipse_stroke(dest.center(), radius, stroke));
            }
        }
        ShapeKind::Triangle | ShapeKind::Diamond => {
            let dest = view.rect(r);
            let points = if s.kind == ShapeKind::Triangle {
                vec![dest.center_top(), dest.right_bottom(), dest.left_bottom()]
            } else {
                vec![
                    dest.center_top(),
                    dest.right_center(),
                    dest.center_bottom(),
                    dest.left_center(),
                ]
            };
            p.add(egui::Shape::convex_polygon(
                points,
                s.fill.map(c32).unwrap_or(Color32::TRANSPARENT),
                stroke,
            ));
        }
        ShapeKind::Cube => {
            let d = view.rect(r);
            let depth = (d.width() * 0.24).min(d.height() * 0.28);
            let fill = s.fill.map(c32).unwrap_or(Color32::TRANSPARENT);
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
            p.add(egui::Shape::convex_polygon(front, fill, stroke));
            p.add(egui::Shape::convex_polygon(top, fill.gamma_multiply(1.2), stroke));
            p.add(egui::Shape::convex_polygon(side, fill.gamma_multiply(0.72), stroke));
        }
        ShapeKind::Cylinder => {
            let d = view.rect(r);
            let cap = (d.height() * 0.24).min(d.width() * 0.3);
            let radius = vec2(d.width() * 0.5, cap * 0.5);
            let top = pos2(d.center().x, d.top() + cap * 0.5);
            let bottom = pos2(d.center().x, d.bottom() - cap * 0.5);
            let fill = s.fill.map(c32).unwrap_or(Color32::TRANSPARENT);
            p.rect_filled(
                egui::Rect::from_min_max(pos2(d.left(), top.y), pos2(d.right(), bottom.y)),
                0.0,
                fill,
            );
            p.add(egui::Shape::ellipse_filled(bottom, radius, fill.gamma_multiply(0.78)));
            p.add(egui::Shape::ellipse_filled(top, radius, fill.gamma_multiply(1.18)));
            if s.stroke_width > 0.0 {
                p.line_segment([pos2(d.left(), top.y), pos2(d.left(), bottom.y)], stroke);
                p.line_segment([pos2(d.right(), top.y), pos2(d.right(), bottom.y)], stroke);
                p.add(egui::Shape::ellipse_stroke(top, radius, stroke));
                p.add(egui::Shape::ellipse_stroke(bottom, radius, stroke));
            }
        }
        ShapeKind::Bevel => {
            let d = view.rect(r);
            let inset = (d.width().min(d.height()) * 0.2).max(1.0);
            let inner = d.shrink(inset);
            let fill = s.fill.map(c32).unwrap_or(Color32::TRANSPARENT);
            let top = vec![d.left_top(), d.right_top(), inner.right_top(), inner.left_top()];
            let right = vec![d.right_top(), d.right_bottom(), inner.right_bottom(), inner.right_top()];
            let bottom = vec![
                d.right_bottom(),
                d.left_bottom(),
                inner.left_bottom(),
                inner.right_bottom(),
            ];
            let left = vec![d.left_bottom(), d.left_top(), inner.left_top(), inner.left_bottom()];
            p.add(egui::Shape::convex_polygon(
                top,
                fill.gamma_multiply(1.22),
                Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                right,
                fill.gamma_multiply(0.72),
                Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                bottom,
                fill.gamma_multiply(0.62),
                Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                left,
                fill.gamma_multiply(0.92),
                Stroke::NONE,
            ));
            p.rect_filled(inner, 0.0, fill);
            if s.stroke_width > 0.0 {
                p.rect_stroke(d, 0.0, stroke, StrokeKind::Middle);
                p.rect_stroke(inner, 0.0, stroke, StrokeKind::Middle);
                for (a, b) in [
                    (d.left_top(), inner.left_top()),
                    (d.right_top(), inner.right_top()),
                    (d.right_bottom(), inner.right_bottom()),
                    (d.left_bottom(), inner.left_bottom()),
                ] {
                    p.line_segment([a, b], stroke);
                }
            }
        }
        ShapeKind::Line | ShapeKind::Arrow => {
            if s.stroke_width <= 0.0 {
                return;
            }
            let (a, b) = s.endpoints(r);
            let (a, b) = (view.pt(a.0, a.1), view.pt(b.0, b.1));
            let d = b - a;
            let len = d.length();
            if s.kind == ShapeKind::Arrow && len > 0.01 {
                let (hl, hw) = arrow_head(s.stroke_width);
                let (hl, hw) = (hl * view.scale, hw * view.scale);
                let dir = d / len;
                let n = vec2(-dir.y, dir.x);
                let base = b - dir * hl.min(len);
                // Stop the shaft inside the head so the joint is hidden.
                let shaft_end = b - dir * (hl.min(len) * 0.6);
                p.line_segment([a, shaft_end], stroke);
                p.add(egui::Shape::convex_polygon(
                    vec![b, base + n * hw * 0.5, base - n * hw * 0.5],
                    c32(s.stroke),
                    Stroke::NONE,
                ));
            } else {
                p.line_segment([a, b], stroke);
            }
        }
    }
}
