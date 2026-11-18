//! Slide thumbnails and persistent section dividers in the left panel.
//! Thumbnail rectangles are laid out for scrolling, but only visible slides
//! are painted with the shared renderer.

use super::Editor;
use crate::document::{SLIDE_H, SLIDE_W, Section, SlideId};
use crate::renderer::{self, PaintOpts, Services, View};
use egui::{Color32, Sense, Stroke, StrokeKind, Ui, vec2};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct SectionEdit {
    start: SlideId,
    text: String,
    fresh: bool,
}

enum SectionAction {
    Add(usize),
    Rename(SlideId, String),
    Remove(SlideId),
}

const COLLAPSED_ID: &str = "slide-sections-collapsed";
const EDIT_ID: &str = "slide-section-edit";

pub fn slides_ui(ui: &mut Ui, ed: &mut Editor, sv: &mut Services) {
    let width = ui.available_width() - 34.0;
    let thumb = vec2(width.max(40.0), width.max(40.0) * SLIDE_H / SLIDE_W);
    let row_h = thumb.y + 12.0;
    let n = ed.doc.slides.len();
    let mut go_to = None;
    let mut reorder: Option<(usize, usize)> = None;
    let mut slide_action: Option<&'static str> = None;
    let mut section_action = None;

    let mut starts: Vec<(usize, Section)> = ed
        .doc
        .sections
        .iter()
        .filter_map(|section| ed.doc.slide_index(section.start).map(|index| (index, section.clone())))
        .collect();
    starts.sort_by_key(|(index, _)| *index);
    let sections: HashMap<usize, (Section, usize)> = starts
        .iter()
        .enumerate()
        .map(|(position, (index, section))| {
            let end = starts.get(position + 1).map_or(n, |(next, _)| *next);
            (*index, (section.clone(), end.saturating_sub(*index)))
        })
        .collect();

    let collapsed_id = ui.make_persistent_id(COLLAPSED_ID);
    let edit_id = ui.make_persistent_id(EDIT_ID);
    let shown_id = ui.make_persistent_id("thumbs-shown");
    let mut collapsed = ui
        .memory(|memory| memory.data.get_temp::<HashSet<SlideId>>(collapsed_id))
        .unwrap_or_default();
    let mut editing = ui.memory(|memory| memory.data.get_temp::<SectionEdit>(edit_id));
    let shown = ui.memory(|memory| memory.data.get_temp::<usize>(shown_id));

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut section_hidden = false;
        for i in 0..n {
            if let Some((section, count)) = sections.get(&i) {
                section_hidden = collapsed.contains(&section.start);
                let contains_current = i <= ed.current && ed.current < i + count;
                let pal = crate::ui::palette_of(ui);
                let fill = if section_hidden && contains_current {
                    pal.on
                } else {
                    pal.hover
                };
                let is_editing = editing.as_ref().is_some_and(|edit| edit.start == section.start);
                let frame = egui::Frame::new()
                    .fill(fill)
                    .corner_radius(5.0)
                    .inner_margin(egui::Margin::symmetric(7, 3));
                let inner = frame.show(ui, |ui| {
                    ui.set_min_height(24.0);
                    ui.horizontal(|ui| {
                        ui.label(if section_hidden { "▸" } else { "▾" });
                        if is_editing {
                            let edit = editing.as_mut().expect("checked above");
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut edit.text)
                                    .desired_width((ui.available_width() - 54.0).max(60.0)),
                            );
                            if edit.fresh {
                                response.request_focus();
                                edit.fresh = false;
                            }
                            let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
                            let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                            if escape {
                                editing = None;
                            } else if enter || response.lost_focus() {
                                section_action = Some(SectionAction::Rename(section.start, edit.text.clone()));
                                editing = None;
                            }
                        } else {
                            ui.label(egui::RichText::new(&section.name).strong());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(if *count == 1 {
                                    "1 slide".to_string()
                                } else {
                                    format!("{count} slides")
                                })
                                .small()
                                .weak(),
                            );
                        });
                    });
                });
                let header = if is_editing {
                    inner.response
                } else {
                    inner.response.interact(Sense::click())
                };
                if !is_editing && header.clicked() {
                    if section_hidden {
                        collapsed.remove(&section.start);
                        section_hidden = false;
                    } else {
                        collapsed.insert(section.start);
                        section_hidden = true;
                    }
                }
                if !is_editing {
                    header.context_menu(|ui| {
                        if ui.button("Rename Section…").clicked() {
                            editing = Some(SectionEdit {
                                start: section.start,
                                text: section.name.clone(),
                                fresh: true,
                            });
                            ui.close();
                        }
                        if ui.button("Remove Section").clicked() {
                            section_action = Some(SectionAction::Remove(section.start));
                            ui.close();
                        }
                    });
                }
                if contains_current && shown != Some(ed.current) {
                    header.scroll_to_me(Some(egui::Align::Center));
                }
                ui.add_space(3.0);
            }
            if section_hidden {
                continue;
            }

            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            let num_rect = egui::Rect::from_min_size(rect.min, vec2(26.0, thumb.y));
            let t_rect = egui::Rect::from_min_size(rect.min + vec2(30.0, 4.0), thumb);
            if ui.is_rect_visible(rect) {
                painter.text(
                    num_rect.right_top() + vec2(-4.0, 2.0),
                    egui::Align2::RIGHT_TOP,
                    format!("{}", i + 1),
                    egui::FontId::proportional(11.0),
                    ui.visuals().weak_text_color(),
                );
                let view = View {
                    origin: t_rect.min,
                    scale: thumb.x / SLIDE_W,
                };
                renderer::paint_slide(&painter, &view, &ed.doc.slides[i], sv, PaintOpts::default());
                let stroke = if i == ed.current {
                    Stroke::new(2.5, Color32::from_rgb(10, 132, 255))
                } else {
                    Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color)
                };
                painter.rect_stroke(t_rect, 2.0, stroke, StrokeKind::Outside);
            }

            if i == ed.current && shown != Some(ed.current) {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
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
                    slide_action = Some("new");
                    ui.close();
                }
                if ui.button("Duplicate Slide").clicked() {
                    slide_action = Some("dup");
                    ui.close();
                }
                if ui.button("Delete Slide").clicked() {
                    slide_action = Some("del");
                    ui.close();
                }
                ui.separator();
                if let Some((section, _)) = sections.get(&i) {
                    if ui.button("Rename Section…").clicked() {
                        editing = Some(SectionEdit {
                            start: section.start,
                            text: section.name.clone(),
                            fresh: true,
                        });
                        ui.close();
                    }
                    if ui.button("Remove Section").clicked() {
                        section_action = Some(SectionAction::Remove(section.start));
                        ui.close();
                    }
                } else if ui.button("Add Section Before").clicked() {
                    section_action = Some(SectionAction::Add(i));
                    ui.close();
                }
            });
        }
    });

    if let Some(i) = go_to {
        ed.go_to_slide(i);
    }
    if let Some((from, to)) = reorder {
        ed.move_slide(from, to);
    }
    match slide_action {
        Some("new") => ed.new_slide(),
        Some("dup") => ed.duplicate_slide(),
        Some("del") => ed.delete_slide(),
        _ => {}
    }
    match section_action {
        Some(SectionAction::Add(index)) => {
            if let Some(start) = ed.add_section_before(index)
                && let Some(section) = ed.doc.section_before(start)
            {
                editing = Some(SectionEdit {
                    start,
                    text: section.name.clone(),
                    fresh: true,
                });
            }
        }
        Some(SectionAction::Rename(start, name)) => ed.rename_section(start, &name),
        Some(SectionAction::Remove(start)) => {
            collapsed.remove(&start);
            ed.remove_section(start);
        }
        None => {}
    }

    ui.memory_mut(|memory| {
        memory.data.insert_temp(collapsed_id, collapsed);
        memory.data.insert_temp(shown_id, ed.current);
        if let Some(edit) = editing {
            memory.data.insert_temp(edit_id, edit);
        } else {
            memory.data.remove::<SectionEdit>(edit_id);
        }
    });
}
