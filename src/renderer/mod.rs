//! Paints slides into an egui `Painter`.
//!
//! The same code draws the editing canvas, the slide thumbnails and the
//! full-screen presentation, at different scales. Geometry is converted from
//! slide space (bp) to screen points only here, through [`View`].

pub mod textures;

use crate::assets::AssetStore;
use crate::document::{
    Color, Equation, ImageRef, Object, ObjectId, ObjectKind, Rect, Shape, ShapeKind, Slide, TextAlign, TextBox, typeset,
};
use crate::export::arrow_head;
use crate::math::{MathService, MathStatus};
use egui::text::{LayoutJob, TextFormat};
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
}

#[derive(Clone, Copy, Default)]
pub struct PaintOpts {
    /// Object being edited in place (its text is drawn by the editor).
    pub skip: Option<ObjectId>,
    /// Show editor-only hints (placeholders, loading states).
    pub editor_hints: bool,
}

pub fn paint_slide(p: &Painter, view: &View, slide: &Slide, sv: &mut Services, opts: PaintOpts) {
    let area = view.slide_rect();
    p.rect_filled(area, 0.0, c32(slide.background));
    let clip = p.clip_rect().intersect(area);
    let p = p.with_clip_rect(clip);
    for o in &slide.objects {
        let r = view.rect(&o.rect).expand(4.0 + o.rect.w.max(o.rect.h) * 0.0);
        // Cull objects far outside the visible clip (keep lines' stroke margin).
        if !clip.expand(16.0).intersects(r) && !matches!(o.kind, ObjectKind::Text(_)) {
            continue;
        }
        if opts.skip == Some(o.id) {
            continue;
        }
        paint_object(&p, view, o, sv, opts);
    }
}

pub fn paint_object(p: &Painter, view: &View, o: &Object, sv: &mut Services, opts: PaintOpts) {
    match &o.kind {
        ObjectKind::Text(t) => paint_text(p, view, &o.rect, t, opts.editor_hints),
        ObjectKind::Equation(e) => paint_equation(p, view, &o.rect, e, sv, opts.editor_hints),
        ObjectKind::Image(i) => paint_image(p, view, &o.rect, i, sv, opts.editor_hints),
        ObjectKind::Shape(s) => paint_shape(p, view, &o.rect, s),
    }
}

/// Lay out a text box at `scale` (screen points per bp). Returns galleys with
/// their offsets (relative to the box's top-left, in screen points) and the
/// content height in bp.
pub fn layout_text(
    fonts_painter: &Painter,
    t: &TextBox,
    width_bp: f32,
    scale: f32,
) -> (Vec<(Arc<egui::Galley>, Vec2)>, f32) {
    let size = t.font_size * scale;
    let line = size * typeset::LINE_SPREAD;
    let family = text_family(t.bold, t.italic);
    let fmt = TextFormat {
        font_id: FontId::new(size, family.clone()),
        color: c32(t.color),
        line_height: Some(line),
        ..Default::default()
    };
    let width = (width_bp * scale).max(1.0);
    let halign = match t.align {
        TextAlign::Left => egui::Align::LEFT,
        TextAlign::Center => egui::Align::Center,
        TextAlign::Right => egui::Align::RIGHT,
    };
    let x_for = |w: f32| match t.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => w * 0.5,
        TextAlign::Right => w,
    };
    let first_baseline = size * typeset::FIRST_BASELINE;
    let mut out = Vec::new();
    let mut y = 0.0f32;
    let paragraphs: Vec<&str> = if t.bullets {
        t.text.split('\n').collect()
    } else {
        vec![t.text.as_str()]
    };
    let indent = if t.bullets { size * typeset::BULLET_INDENT } else { 0.0 };
    for para in paragraphs {
        let mut job = LayoutJob::default();
        job.wrap.max_width = (width - indent).max(1.0);
        job.halign = halign;
        job.append(para, 0.0, fmt.clone());
        let g = fonts_painter.layout_job(job);
        // Align the first baseline with TeX's \strut convention.
        let b0 = g
            .rows
            .first()
            .and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y));
        let b0 = b0.unwrap_or(line * 0.75);
        let dy = y + first_baseline - b0;
        if t.bullets && !para.trim().is_empty() {
            let mut bj = LayoutJob::default();
            bj.append("•", 0.0, fmt.clone());
            let bg = fonts_painter.layout_job(bj);
            let bb = bg
                .rows
                .first()
                .and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y))
                .unwrap_or(b0);
            out.push((bg, vec2(0.0, y + first_baseline - bb)));
        }
        let rows = g.rows.len().max(1) as f32;
        out.push((g, vec2(indent + x_for(width - indent), dy)));
        y += rows * line;
    }
    (out, y / scale)
}

/// Content height (bp) of a text box: number of laid-out lines × line spread.
pub fn text_height_bp(p: &Painter, t: &TextBox, width_bp: f32) -> f32 {
    // Lay out at a fixed, moderately large scale to make wrapping
    // decisions zoom-independent.
    layout_text(p, t, width_bp, 2.0).1
}

pub fn paint_text(p: &Painter, view: &View, r: &Rect, t: &TextBox, hints: bool) {
    let origin = view.pt(r.x, r.y);
    if t.text.is_empty() {
        if hints {
            let size = t.font_size * view.scale;
            p.text(
                origin + vec2(0.0, size * 0.1),
                egui::Align2::LEFT_TOP,
                "Double-click to type",
                FontId::new(size, text_family(false, true)),
                Color32::from_gray(150),
            );
        }
        return;
    }
    let (galleys, _) = layout_text(p, t, r.w, view.scale);
    for (g, off) in galleys {
        p.galley(origin + off, g, c32(t.color));
    }
}

fn paint_equation(p: &Painter, view: &View, r: &Rect, e: &Equation, sv: &mut Services, hints: bool) {
    let ppp = p.pixels_per_point();
    let status = sv.math.request(&e.source, e.font_size, sv.now);
    match status {
        MathStatus::Ready(render) => {
            let size = vec2(render.size_bp.0, render.size_bp.1) * view.scale;
            let dest = egui::Rect::from_min_size(view.pt(r.x, r.y), size);
            if let Some(tex) = sv.textures.math(&render, view.scale * ppp) {
                p.image(
                    tex,
                    dest,
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    c32(e.color),
                );
            } else if hints {
                p.rect_filled(dest, 2.0, Color32::from_gray(235));
            }
        }
        MathStatus::Pending => {
            let dest = view.rect(r);
            if hints {
                p.rect_filled(dest, 2.0, Color32::from_rgba_unmultiplied(31, 95, 168, 18));
                let size = (e.font_size * view.scale * 0.6).max(6.0);
                p.text(
                    dest.left_center(),
                    egui::Align2::LEFT_CENTER,
                    truncate(&e.source, 60),
                    FontId::monospace(size),
                    Color32::from_gray(110),
                );
            }
        }
        MathStatus::Failed(_) => {
            let dest = view.rect(r).expand(1.0);
            if hints {
                p.rect_stroke(
                    dest,
                    2.0,
                    Stroke::new(1.5, Color32::from_rgb(200, 40, 40)),
                    StrokeKind::Outside,
                );
                let size = (e.font_size * view.scale * 0.6).max(6.0);
                p.text(
                    dest.left_center() + vec2(2.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("LaTeX error: {}", truncate(&e.source, 50)),
                    FontId::monospace(size),
                    Color32::from_rgb(200, 40, 40),
                );
            }
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > n {
        one_line.chars().take(n).chain(std::iter::once('…')).collect()
    } else {
        one_line
    }
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

pub fn paint_shape(p: &Painter, view: &View, r: &Rect, s: &Shape) {
    let sw = s.stroke_width * view.scale;
    let stroke = if s.stroke_width > 0.0 {
        Stroke::new(sw, c32(s.stroke))
    } else {
        Stroke::NONE
    };
    match s.kind {
        ShapeKind::Rectangle => {
            let dest = view.rect(r);
            if let Some(f) = s.fill {
                p.rect_filled(dest, 0.0, c32(f));
            }
            if s.stroke_width > 0.0 {
                p.rect_stroke(dest, 0.0, stroke, StrokeKind::Middle);
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
