//! Dash-style and line-end pickers with drawn previews (the same drawing
//! code as the slide), like PowerPoint's "Dashes" and "Arrows" galleries.

use super::palette_of;
use crate::document::{DashStyle, LineEnd};
use crate::renderer::{paint_tip, stroke_path};
use egui::{Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

fn preview_dash(ui: &Ui, r: Rect, dash: DashStyle) {
    let c = palette_of(ui).text;
    let y = r.center().y;
    stroke_path(
        ui.painter(),
        &[pos2(r.left() + 4.0, y), pos2(r.right() - 4.0, y)],
        false,
        Stroke::new(2.0, c),
        dash,
        2.0,
        1.0,
    );
}

fn preview_tip(ui: &Ui, r: Rect, tip: LineEnd, at_start: bool) {
    let c = palette_of(ui).text;
    let y = r.center().y;
    let (a, b) = (pos2(r.left() + 6.0, y), pos2(r.right() - 6.0, y));
    let (tip_at, dir) = if at_start {
        (a, vec2(-1.0, 0.0))
    } else {
        (b, vec2(1.0, 0.0))
    };
    let cut = paint_tip(ui.painter(), tip, tip_at, dir, 1.5, 1.0, c);
    let (a, b) = if at_start {
        (a + vec2(cut, 0.0), b)
    } else {
        (a, b - vec2(cut, 0.0))
    };
    ui.painter().line_segment([a, b], Stroke::new(1.5, c));
}

/// A drop-down button showing a preview; `paint` draws a preview in a rect.
fn preview_button(ui: &mut Ui, tooltip: &str, paint: impl Fn(&Ui, Rect)) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(58.0, 22.0), Sense::click());
    if ui.is_rect_visible(r) {
        let pal = palette_of(ui);
        let bg = if resp.hovered() { pal.hover } else { pal.card };
        ui.painter().rect_filled(r, 4.0, bg);
        ui.painter()
            .rect_stroke(r, 4.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        paint(ui, Rect::from_min_max(r.min, pos2(r.right() - 12.0, r.bottom())));
        let (x, y) = (r.right() - 7.0, r.center().y);
        let fg = if ui.is_enabled() { pal.text } else { pal.weak };
        ui.painter()
            .line_segment([pos2(x - 3.0, y - 1.5), pos2(x, y + 1.5)], Stroke::new(1.2, fg));
        ui.painter()
            .line_segment([pos2(x, y + 1.5), pos2(x + 3.0, y - 1.5)], Stroke::new(1.2, fg));
    }
    resp.on_hover_text(tooltip)
}

fn option_row(ui: &mut Ui, selected: bool, label: &str, paint: impl Fn(&Ui, Rect)) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(190.0, 24.0), Sense::click());
    if ui.is_rect_visible(r) {
        let pal = palette_of(ui);
        if selected {
            ui.painter().rect_filled(r, 4.0, pal.on);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, pal.hover);
        }
        paint(ui, Rect::from_min_size(r.min, vec2(80.0, r.height())));
        ui.painter().text(
            pos2(r.left() + 90.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            pal.text,
        );
    }
    resp.clicked()
}

/// Dash style picker. Returns the new style when changed.
pub fn dash_menu(ui: &mut Ui, current: DashStyle) -> Option<DashStyle> {
    let resp = preview_button(ui, "Dash type", |ui, r| preview_dash(ui, r, current));
    let mut out = None;
    egui::Popup::menu(&resp).show(|ui| {
        super::heading(ui, "DASHES");
        for d in DashStyle::ALL {
            if option_row(ui, d == current, d.label(), |ui, r| preview_dash(ui, r, d)) {
                out = Some(d);
                ui.close();
            }
        }
    });
    out.filter(|d| *d != current)
}

/// Line-end picker for the start (`at_start`) or end of a line.
pub fn tip_menu(ui: &mut Ui, current: LineEnd, at_start: bool) -> Option<LineEnd> {
    let tip = if at_start { "Begin arrow type" } else { "End arrow type" };
    let resp = preview_button(ui, tip, |ui, r| preview_tip(ui, r, current, at_start));
    let mut out = None;
    egui::Popup::menu(&resp).show(|ui| {
        super::heading(ui, if at_start { "BEGIN" } else { "END" });
        for t in LineEnd::ALL {
            if option_row(ui, t == current, t.label(), |ui, r| preview_tip(ui, r, t, at_start)) {
                out = Some(t);
                ui.close();
            }
        }
    });
    out.filter(|t| *t != current)
}
