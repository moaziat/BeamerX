//! Small vector icons drawn with the egui painter (no icon font needed) and
//! flat ribbon buttons that use them.

use super::palette_of;
use egui::epaint::PathStroke;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Paste,
    Cut,
    Copy,
    NewSlide,
    Section,
    Layout,
    Duplicate,
    Delete,
    TextBox,
    Picture,
    Shapes,
    Rectangle,
    RoundedRectangle,
    Oval,
    Triangle,
    Diamond,
    Cube,
    BlockArrow,
    Cylinder,
    Bevel,
    Line,
    Arrow,
    Math,
    Present,
    Undo,
    Redo,
    Bullets,
    Numbering,
    Outdent,
    Indent,
    AlignLeft,
    AlignCenter,
    AlignRight,
    AlignTop,
    AlignMiddle,
    AlignBottom,
    Front,
    Back,
    ZoomIn,
    ZoomOut,
    Fit,
    FontColor,
    Fill,
    Outline,
    Properties,
    Pane,
    Background,
    Close,
}

/// Paint `icon` inside `r` (square-ish) with colour `c`.
pub fn paint(p: &Painter, r: Rect, icon: Icon, c: Color32) {
    let s = r.width().min(r.height());
    let r = Rect::from_center_size(r.center(), Vec2::splat(s));
    let w = (s / 14.0).max(1.2);
    let st = Stroke::new(w, c);
    let at = |x: f32, y: f32| pos2(r.left() + x * s, r.top() + y * s);
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let line = |a: Pos2, b: Pos2| p.line_segment([a, b], st);
    let text = |t: &str, size: f32, pos: Pos2| p.text(pos, Align2::CENTER_CENTER, t, FontId::proportional(size), c);
    let lines = |x0: f32, x1: f32, ys: &[f32]| {
        for y in ys {
            line(at(x0, *y), at(x1, *y));
        }
    };
    match icon {
        Icon::Paste => {
            p.rect_stroke(rect(0.2, 0.18, 0.8, 0.92), 2.0, st, egui::StrokeKind::Middle);
            p.rect_filled(rect(0.35, 0.08, 0.65, 0.26), 1.5, c);
            lines(0.32, 0.68, &[0.48, 0.62, 0.76]);
        }
        Icon::Cut => {
            p.circle_stroke(at(0.3, 0.75), s * 0.13, st);
            p.circle_stroke(at(0.7, 0.75), s * 0.13, st);
            line(at(0.38, 0.65), at(0.68, 0.12));
            line(at(0.62, 0.65), at(0.32, 0.12));
        }
        Icon::Copy => {
            p.rect_stroke(rect(0.18, 0.12, 0.62, 0.66), 2.0, st, egui::StrokeKind::Middle);
            p.rect_stroke(rect(0.38, 0.34, 0.82, 0.88), 2.0, st, egui::StrokeKind::Middle);
        }
        Icon::NewSlide => {
            p.rect_stroke(rect(0.08, 0.22, 0.78, 0.74), 2.0, st, egui::StrokeKind::Middle);
            line(at(0.78, 0.66), at(0.78, 0.96));
            line(at(0.63, 0.81), at(0.93, 0.81));
        }
        Icon::Section => {
            p.rect_stroke(rect(0.12, 0.08, 0.88, 0.38), 1.5, st, egui::StrokeKind::Middle);
            p.rect_stroke(rect(0.12, 0.62, 0.88, 0.92), 1.5, st, egui::StrokeKind::Middle);
            line(at(0.12, 0.5), at(0.62, 0.5));
            p.circle_filled(at(0.82, 0.5), s * 0.08, c);
        }
        Icon::Layout => {
            p.rect_stroke(rect(0.1, 0.2, 0.9, 0.8), 2.0, st, egui::StrokeKind::Middle);
            p.rect_filled(rect(0.18, 0.28, 0.82, 0.38), 1.0, c);
            p.rect_stroke(rect(0.18, 0.46, 0.46, 0.72), 1.0, st, egui::StrokeKind::Middle);
            p.rect_stroke(rect(0.54, 0.46, 0.82, 0.72), 1.0, st, egui::StrokeKind::Middle);
        }
        Icon::Duplicate => {
            p.rect_stroke(rect(0.08, 0.16, 0.66, 0.56), 2.0, st, egui::StrokeKind::Middle);
            p.rect_filled(rect(0.3, 0.42, 0.92, 0.84), 2.0, c.gamma_multiply(0.25));
            p.rect_stroke(rect(0.3, 0.42, 0.92, 0.84), 2.0, st, egui::StrokeKind::Middle);
        }
        Icon::Delete => {
            p.rect_stroke(rect(0.26, 0.3, 0.74, 0.9), 2.0, st, egui::StrokeKind::Middle);
            line(at(0.16, 0.24), at(0.84, 0.24));
            line(at(0.4, 0.14), at(0.6, 0.14));
            lines(0.5, 0.5, &[]);
            line(at(0.42, 0.42), at(0.42, 0.78));
            line(at(0.58, 0.42), at(0.58, 0.78));
        }
        Icon::TextBox => {
            p.rect_stroke(
                rect(0.08, 0.14, 0.92, 0.86),
                2.0,
                Stroke::new(w * 0.8, c.gamma_multiply(0.6)),
                egui::StrokeKind::Middle,
            );
            text("A", s * 0.62, at(0.5, 0.52));
        }
        Icon::Picture => {
            p.rect_stroke(rect(0.08, 0.16, 0.92, 0.84), 2.0, st, egui::StrokeKind::Middle);
            p.add(Shape::convex_polygon(
                vec![at(0.16, 0.76), at(0.4, 0.44), at(0.6, 0.76)],
                c,
                Stroke::NONE,
            ));
            p.add(Shape::convex_polygon(
                vec![at(0.48, 0.76), at(0.66, 0.54), at(0.84, 0.76)],
                c.gamma_multiply(0.6),
                Stroke::NONE,
            ));
            p.circle_filled(at(0.7, 0.34), s * 0.07, c);
        }
        Icon::Shapes => {
            p.rect_stroke(rect(0.08, 0.12, 0.56, 0.5), 1.5, st, egui::StrokeKind::Middle);
            p.circle_stroke(at(0.68, 0.68), s * 0.22, st);
            p.add(Shape::convex_polygon(
                vec![at(0.48, 0.22), at(0.82, 0.22), at(0.65, 0.02)],
                c.gamma_multiply(0.4),
                st,
            ));
        }
        Icon::Rectangle => {
            p.rect_stroke(rect(0.12, 0.24, 0.88, 0.76), 1.5, st, egui::StrokeKind::Middle);
        }
        Icon::RoundedRectangle => {
            p.rect_stroke(rect(0.1, 0.22, 0.9, 0.78), s * 0.14, st, egui::StrokeKind::Middle);
        }
        Icon::Oval => {
            p.add(Shape::ellipse_stroke(r.center(), vec2(s * 0.38, s * 0.27), st));
        }
        Icon::Triangle => {
            p.add(Shape::convex_polygon(
                vec![at(0.5, 0.1), at(0.9, 0.84), at(0.1, 0.84)],
                Color32::TRANSPARENT,
                st,
            ));
        }
        Icon::Diamond => {
            p.add(Shape::convex_polygon(
                vec![at(0.5, 0.08), at(0.92, 0.5), at(0.5, 0.92), at(0.08, 0.5)],
                Color32::TRANSPARENT,
                st,
            ));
        }
        Icon::BlockArrow => {
            let pts = crate::document::block_arrow_points(0.08, 0.22, 0.84, 0.56);
            let pts: Vec<Pos2> = pts.iter().map(|(x, y)| at(*x, *y)).collect();
            p.add(Shape::closed_line(pts, st));
        }
        Icon::Cube => {
            p.add(Shape::convex_polygon(
                vec![at(0.12, 0.32), at(0.42, 0.12), at(0.88, 0.3), at(0.58, 0.5)],
                c.gamma_multiply(0.12),
                st,
            ));
            p.add(Shape::convex_polygon(
                vec![at(0.12, 0.32), at(0.58, 0.5), at(0.58, 0.9), at(0.12, 0.7)],
                Color32::TRANSPARENT,
                st,
            ));
            p.add(Shape::convex_polygon(
                vec![at(0.58, 0.5), at(0.88, 0.3), at(0.88, 0.7), at(0.58, 0.9)],
                c.gamma_multiply(0.2),
                st,
            ));
        }
        Icon::Cylinder => {
            p.rect_stroke(rect(0.18, 0.25, 0.82, 0.75), 0.0, st, egui::StrokeKind::Middle);
            p.add(Shape::ellipse_filled(
                at(0.5, 0.25),
                vec2(s * 0.32, s * 0.14),
                c.gamma_multiply(0.1),
            ));
            p.add(Shape::ellipse_stroke(at(0.5, 0.25), vec2(s * 0.32, s * 0.14), st));
            p.add(Shape::ellipse_stroke(at(0.5, 0.75), vec2(s * 0.32, s * 0.14), st));
        }
        Icon::Bevel => {
            p.rect_stroke(rect(0.1, 0.18, 0.9, 0.82), 0.0, st, egui::StrokeKind::Middle);
            p.rect_stroke(rect(0.27, 0.34, 0.73, 0.66), 0.0, st, egui::StrokeKind::Middle);
            for (a, b) in [
                (at(0.1, 0.18), at(0.27, 0.34)),
                (at(0.9, 0.18), at(0.73, 0.34)),
                (at(0.9, 0.82), at(0.73, 0.66)),
                (at(0.1, 0.82), at(0.27, 0.66)),
            ] {
                line(a, b);
            }
        }
        Icon::Line => {
            line(at(0.14, 0.84), at(0.86, 0.16));
        }
        Icon::Arrow => {
            line(at(0.14, 0.84), at(0.72, 0.28));
            p.add(Shape::convex_polygon(
                vec![at(0.88, 0.12), at(0.56, 0.22), at(0.78, 0.44)],
                c,
                Stroke::NONE,
            ));
        }
        Icon::Math => {
            text("Σ", s * 0.72, at(0.38, 0.5));
            text("x", s * 0.42, at(0.76, 0.6));
        }
        Icon::Present => {
            p.add(Shape::convex_polygon(
                vec![at(0.28, 0.16), at(0.84, 0.5), at(0.28, 0.84)],
                c,
                Stroke::NONE,
            ));
        }
        Icon::Undo | Icon::Redo => {
            let flip = matches!(icon, Icon::Redo);
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = std::f32::consts::PI * (0.15 + 0.85 * i as f32 / 12.0);
                    at(fx(0.5 + 0.3 * t.cos()), 0.55 - 0.3 * t.sin())
                })
                .collect();
            p.add(Shape::line(pts, PathStroke::new(w, c)));
            let tip = at(fx(0.2), 0.62);
            p.add(Shape::convex_polygon(
                vec![tip, at(fx(0.06), 0.42), at(fx(0.36), 0.42)],
                c,
                Stroke::NONE,
            ));
        }
        Icon::Bullets | Icon::Numbering => {
            for (k, y) in [0.25f32, 0.5, 0.75].iter().enumerate() {
                if icon == Icon::Bullets {
                    p.circle_filled(at(0.16, *y), s * 0.06, c);
                } else {
                    text(&format!("{}", k + 1), s * 0.3, at(0.16, *y));
                }
                line(at(0.34, *y), at(0.9, *y));
            }
        }
        Icon::Outdent | Icon::Indent => {
            lines(0.1, 0.9, &[0.15, 0.85]);
            lines(0.5, 0.9, &[0.38, 0.62]);
            let (a, b) = if icon == Icon::Indent {
                (0.12, 0.36)
            } else {
                (0.36, 0.12)
            };
            line(at(a, 0.5), at(b, 0.5));
            p.add(Shape::convex_polygon(
                if icon == Icon::Indent {
                    vec![at(0.4, 0.5), at(0.28, 0.38), at(0.28, 0.62)]
                } else {
                    vec![at(0.08, 0.5), at(0.2, 0.38), at(0.2, 0.62)]
                },
                c,
                Stroke::NONE,
            ));
        }
        Icon::AlignLeft | Icon::AlignCenter | Icon::AlignRight => {
            for (y, len) in [(0.2f32, 0.8f32), (0.4, 0.5), (0.6, 0.8), (0.8, 0.5)] {
                let x0 = match icon {
                    Icon::AlignLeft => 0.1,
                    Icon::AlignCenter => 0.5 - len / 2.0,
                    _ => 0.9 - len,
                };
                line(at(x0, y), at(x0 + len, y));
            }
        }
        Icon::AlignTop | Icon::AlignMiddle | Icon::AlignBottom => {
            let y = match icon {
                Icon::AlignTop => 0.16,
                Icon::AlignMiddle => 0.5,
                _ => 0.84,
            };
            line(at(0.08, y), at(0.92, y));
            let (y0, y1) = match icon {
                Icon::AlignTop => (y + 0.12, y + 0.48),
                Icon::AlignMiddle => (y - 0.18, y + 0.18),
                _ => (y - 0.48, y - 0.12),
            };
            p.rect_stroke(rect(0.18, y0, 0.4, y1), 1.0, st, egui::StrokeKind::Middle);
            p.rect_stroke(rect(0.6, y0, 0.82, y1), 1.0, st, egui::StrokeKind::Middle);
        }
        Icon::Front | Icon::Back => {
            let (back, front) = (rect(0.1, 0.1, 0.62, 0.62), rect(0.38, 0.38, 0.9, 0.9));
            if icon == Icon::Front {
                p.rect_stroke(back, 1.5, st, egui::StrokeKind::Middle);
                p.rect_filled(front, 1.5, c);
            } else {
                p.rect_filled(back, 1.5, c);
                p.rect_filled(front, 1.5, Color32::TRANSPARENT);
                p.rect_stroke(front, 1.5, st, egui::StrokeKind::Middle);
            }
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            p.circle_stroke(at(0.42, 0.42), s * 0.27, st);
            line(at(0.62, 0.62), at(0.88, 0.88));
            line(at(0.3, 0.42), at(0.54, 0.42));
            if icon == Icon::ZoomIn {
                line(at(0.42, 0.3), at(0.42, 0.54));
            }
        }
        Icon::Fit => {
            for (x, y, dx, dy) in [
                (0.1, 0.1, 1.0, 1.0),
                (0.9, 0.1, -1.0, 1.0),
                (0.1, 0.9, 1.0, -1.0),
                (0.9, 0.9, -1.0, -1.0),
            ] {
                line(at(x, y), at(x + 0.25 * dx, y));
                line(at(x, y), at(x, y + 0.25 * dy));
            }
        }
        Icon::FontColor => {
            text("A", s * 0.7, at(0.5, 0.42));
        }
        Icon::Fill => {
            p.add(Shape::convex_polygon(
                vec![at(0.2, 0.42), at(0.5, 0.12), at(0.8, 0.42), at(0.5, 0.72)],
                c.gamma_multiply(0.35),
                st,
            ));
        }
        Icon::Outline => {
            line(at(0.2, 0.72), at(0.72, 0.2));
            line(at(0.72, 0.2), at(0.84, 0.32));
            line(at(0.84, 0.32), at(0.32, 0.84));
            line(at(0.32, 0.84), at(0.16, 0.88));
            line(at(0.16, 0.88), at(0.2, 0.72));
        }
        Icon::Properties => {
            p.rect_stroke(rect(0.14, 0.1, 0.86, 0.9), 2.0, st, egui::StrokeKind::Middle);
            lines(0.28, 0.72, &[0.3, 0.5, 0.7]);
        }
        Icon::Pane => {
            p.rect_stroke(rect(0.08, 0.16, 0.92, 0.84), 2.0, st, egui::StrokeKind::Middle);
            p.rect_filled(rect(0.62, 0.16, 0.92, 0.84), 1.0, c.gamma_multiply(0.5));
        }
        Icon::Background => {
            p.rect_filled(rect(0.08, 0.16, 0.92, 0.84), 2.0, c.gamma_multiply(0.3));
            p.rect_stroke(rect(0.08, 0.16, 0.92, 0.84), 2.0, st, egui::StrokeKind::Middle);
        }
        Icon::Close => {
            line(at(0.22, 0.22), at(0.78, 0.78));
            line(at(0.78, 0.22), at(0.22, 0.78));
        }
    }
}

fn flat_bg(ui: &Ui, resp: &Response, selected: bool) -> Option<Color32> {
    let pal = palette_of(ui);
    if !ui.is_enabled() {
        return selected.then_some(pal.on.gamma_multiply(0.5));
    }
    if resp.is_pointer_button_down_on() {
        Some(pal.pressed)
    } else if selected {
        Some(pal.on)
    } else if resp.hovered() {
        Some(pal.hover)
    } else {
        None
    }
}

fn fg(ui: &Ui) -> Color32 {
    let pal = palette_of(ui);
    if ui.is_enabled() {
        pal.text
    } else {
        pal.weak.gamma_multiply(0.6)
    }
}

fn accessible_name(icon: Icon) -> &'static str {
    match icon {
        Icon::Paste => "Paste",
        Icon::Cut => "Cut",
        Icon::Copy => "Copy",
        Icon::NewSlide => "New slide",
        Icon::Section => "Section",
        Icon::Layout => "Layout",
        Icon::Duplicate => "Duplicate",
        Icon::Delete => "Delete",
        Icon::TextBox => "Text box",
        Icon::Picture => "Picture",
        Icon::Shapes => "Shapes",
        Icon::Rectangle => "Rectangle",
        Icon::RoundedRectangle => "Rounded rectangle",
        Icon::Oval => "Oval",
        Icon::Triangle => "Triangle",
        Icon::Diamond => "Diamond",
        Icon::Cube => "Cube",
        Icon::BlockArrow => "Block arrow",
        Icon::Cylinder => "Cylinder",
        Icon::Bevel => "Bevel",
        Icon::Line => "Line",
        Icon::Arrow => "Arrow",
        Icon::Math => "Math",
        Icon::Present => "Present",
        Icon::Undo => "Undo",
        Icon::Redo => "Redo",
        Icon::Bullets => "Bullets",
        Icon::Numbering => "Numbering",
        Icon::Outdent => "Outdent",
        Icon::Indent => "Indent",
        Icon::AlignLeft => "Align left",
        Icon::AlignCenter => "Align center",
        Icon::AlignRight => "Align right",
        Icon::AlignTop => "Align top",
        Icon::AlignMiddle => "Align middle",
        Icon::AlignBottom => "Align bottom",
        Icon::Front => "Bring forward",
        Icon::Back => "Send backward",
        Icon::ZoomIn => "Zoom in",
        Icon::ZoomOut => "Zoom out",
        Icon::Fit => "Fit",
        Icon::FontColor => "Font color",
        Icon::Fill => "Fill",
        Icon::Outline => "Outline",
        Icon::Properties => "Properties",
        Icon::Pane => "Pane",
        Icon::Background => "Background",
        Icon::Close => "Close",
    }
}

fn button_info(resp: &Response, ui: &Ui, label: &str, selected: Option<bool>) {
    resp.widget_info(|| match selected {
        Some(selected) => egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label),
        None => egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label),
    });
}

/// Large ribbon button: icon above a (possibly two-line) label.
pub fn big(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    big_impl(ui, icon, label, None, None)
}

/// Large button that shows a selected state.
pub fn big_toggle(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
    big_impl(ui, icon, label, None, Some(selected))
}

/// Large button with an optional colour bar under the icon.
pub fn big_with(ui: &mut Ui, icon: Icon, label: &str, bar: Option<Color32>) -> Response {
    big_impl(ui, icon, label, bar, None)
}

fn big_impl(ui: &mut Ui, icon: Icon, label: &str, bar: Option<Color32>, selected: Option<bool>) -> Response {
    let font = FontId::proportional(11.5);
    let width = label
        .lines()
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(36.0f32, f32::max)
        + 12.0;
    let (r, resp) = ui.allocate_exact_size(vec2(width, 58.0), Sense::click());
    if ui.is_rect_visible(r) {
        if let Some(bg) = flat_bg(ui, &resp, selected.unwrap_or(false)) {
            ui.painter().rect_filled(r, 5.0, bg);
        }
        let c = fg(ui);
        let ir = Rect::from_center_size(pos2(r.center().x, r.top() + 15.0), Vec2::splat(22.0));
        paint(ui.painter(), ir, icon, c);
        if let Some(b) = bar {
            ui.painter().rect_filled(
                Rect::from_center_size(pos2(ir.center().x, ir.bottom() + 2.0), vec2(20.0, 3.5)),
                1.0,
                b,
            );
        }
        let mut y = r.top() + 33.0;
        for l in label.lines() {
            ui.painter()
                .text(pos2(r.center().x, y), Align2::CENTER_TOP, l, font.clone(), c);
            y += 12.5;
        }
    }
    button_info(&resp, ui, label, selected);
    resp
}

/// Small ribbon button: icon followed by a label.
pub fn small(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    small_impl(ui, icon, label, None, None)
}

/// Compact high-emphasis action, used for the primary Present button.
pub fn small_accent(ui: &mut Ui, icon: Icon, label: &str, accent: Color32) -> Response {
    small_impl(ui, icon, label, None, Some(accent))
}

/// Small button that shows a selected state.
pub fn small_toggle(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
    small_impl(ui, icon, label, Some(selected), None)
}

fn small_impl(ui: &mut Ui, icon: Icon, label: &str, selected: Option<bool>, accent: Option<Color32>) -> Response {
    let font = FontId::proportional(12.0);
    let tw = if label.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
            + 6.0
    };
    let (r, resp) = ui.allocate_exact_size(vec2(28.0 + tw, 26.0), Sense::click());
    if ui.is_rect_visible(r) {
        let bg = accent.map(|c| {
            if resp.is_pointer_button_down_on() {
                c.gamma_multiply(0.82)
            } else if resp.hovered() {
                c.gamma_multiply(1.08)
            } else {
                c
            }
        });
        if let Some(bg) = bg.or_else(|| flat_bg(ui, &resp, selected.unwrap_or(false))) {
            ui.painter().rect_filled(r, 4.0, bg);
        }
        let c = accent.map_or_else(|| fg(ui), |_| Color32::WHITE);
        paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), Vec2::splat(15.0)),
            icon,
            c,
        );
        if !label.is_empty() {
            ui.painter()
                .text(pos2(r.left() + 27.0, r.center().y), Align2::LEFT_CENTER, label, font, c);
        }
    }
    let accessible = if label.is_empty() { accessible_name(icon) } else { label };
    button_info(&resp, ui, accessible, selected);
    resp
}

/// Text-only flat toggle (B, I, style names) in the ribbon's look.
pub fn text_toggle(ui: &mut Ui, text: egui::RichText, selected: bool) -> Response {
    text_impl(ui, text, Some(selected))
}

/// Text-only flat button in the ribbon's look.
pub fn text(ui: &mut Ui, text: egui::RichText) -> Response {
    text_impl(ui, text, None)
}

fn text_impl(ui: &mut Ui, text: egui::RichText, selected: Option<bool>) -> Response {
    let accessible = text.text().to_owned();
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let size = vec2(galley.size().x + 14.0, 26.0);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(r) {
        if let Some(bg) = flat_bg(ui, &resp, selected.unwrap_or(false)) {
            ui.painter().rect_filled(r, 4.0, bg);
        }
        let pos = r.center() - galley.size() * 0.5;
        ui.painter().galley(pos, galley, fg(ui));
    }
    button_info(&resp, ui, &accessible, selected);
    resp
}
