//! Slide thumbnails (left panel). Only rows scrolled into view are painted;
//! thumbnails are drawn with the same vector renderer at a small scale and
//! request small image levels from the shared texture cache.

use super::Editor;
use crate::document::{SLIDE_H, SLIDE_W};
use crate::renderer::{self, PaintOpts, Services, View};
use egui::{Color32, Sense, Stroke, StrokeKind, Ui, vec2};

pub fn slides_ui(ui: &mut Ui, ed: &mut Editor, sv: &mut Services) {
    let width = ui.available_width() - 34.0;
    let thumb = vec2(width.max(40.0), width.max(40.0) * SLIDE_H / SLIDE_W);
    let row_h = thumb.y + 12.0;
    let n = ed.doc.slides.len();
    let mut go_to = None;
    let mut reorder: Option<(usize, usize)> = None;
    let mut action: Option<&'static str> = None;

    // Keep the current slide visible after navigation (rows outside the
    // viewport are not laid out, so scroll by offset).
    let mem_id = egui::Id::unique("thumbs-scroll");
    let (shown, offset, height) =
        ui.memory(|m| m.data.get_temp::<(usize, f32, f32)>(mem_id))
            .unwrap_or((usize::MAX, 0.0, 0.0));
    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if shown != ed.current && height > 0.0 {
        let top = ed.current as f32 * row_h;
        if top < offset || top + row_h > offset + height {
            area = area.vertical_scroll_offset((top - (height - row_h) * 0.5).max(0.0));
        }
    }

    let out = area.show_rows(ui, row_h, n, |ui, range| {
        for i in range {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            let num_rect = egui::Rect::from_min_size(rect.min, vec2(26.0, thumb.y));
            painter.text(
                num_rect.right_top() + vec2(-4.0, 2.0),
                egui::Align2::RIGHT_TOP,
                format!("{}", i + 1),
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
            let t_rect = egui::Rect::from_min_size(rect.min + vec2(30.0, 4.0), thumb);
            let view = View {
                origin: t_rect.min,
                scale: thumb.x / SLIDE_W,
            };
            renderer::paint_slide(&painter, &view, &ed.doc.slides[i], sv, PaintOpts::default());
            let selected = i == ed.current;
            let stroke = if selected {
                Stroke::new(2.5, Color32::from_rgb(10, 132, 255))
            } else {
                Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color)
            };
            painter.rect_stroke(t_rect, 2.0, stroke, StrokeKind::Outside);

            if resp.clicked() {
                go_to = Some(i);
            }
            if resp.drag_started() {
                resp.dnd_set_drag_payload(i);
            }
            if let Some(from) = resp.dnd_release_payload::<usize>() {
                reorder = Some((*from, i));
            }
            if let Some(from) = resp.dnd_hover_payload::<usize>()
                && *from != i
            {
                let y = if *from < i {
                    t_rect.bottom() + 4.0
                } else {
                    t_rect.top() - 4.0
                };
                painter.hline(t_rect.x_range(), y, Stroke::new(2.0, Color32::from_rgb(10, 132, 255)));
            }
            resp.context_menu(|ui| {
                go_to = Some(i);
                if ui.button("New Slide").clicked() {
                    action = Some("new");
                    ui.close();
                }
                if ui.button("Duplicate Slide").clicked() {
                    action = Some("dup");
                    ui.close();
                }
                if ui.button("Delete Slide").clicked() {
                    action = Some("del");
                    ui.close();
                }
            });
        }
    });

    ui.memory_mut(|m| {
        m.data.insert_temp(
            mem_id,
            (ed.current, out.state.clamped_offset().y, out.inner_rect.height()),
        )
    });

    if let Some(i) = go_to {
        ed.go_to_slide(i);
    }
    if let Some((from, to)) = reorder {
        ed.move_slide(from, to);
    }
    match action {
        Some("new") => ed.new_slide(),
        Some("dup") => ed.duplicate_slide(),
        Some("del") => ed.delete_slide(),
        _ => {}
    }
}
