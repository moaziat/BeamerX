//! Format pane (optional right panel, like PowerPoint's "Format Shape"),
//! the presentation properties window, and shared formatting widgets used by
//! the ribbon.

use super::{Align, Editor, ZOrder};
use crate::assets::AssetStore;
use crate::commands::{Command, MergeKey};
use crate::document::{Color, MAX_FONT_SIZE, MIN_FONT_SIZE, ObjectKind, Rect, ShapeKind, TextAlign};
use crate::math::MathService;
use egui::{Color32, DragValue, RichText, Ui};

pub fn color_edit(ui: &mut Ui, c: &mut Color) -> bool {
    let mut c32 = Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a);
    let changed =
        egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::OnlyBlend).changed();
    if changed {
        let [r, g, b, a] = c32.to_srgba_unmultiplied();
        *c = Color::rgba(r, g, b, a);
    }
    changed
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(
        RichText::new(title)
            .small()
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(2.0);
}

pub fn font_size_edit(ui: &mut Ui, size: &mut f32) -> bool {
    ui.add(
        DragValue::new(size)
            .speed(0.2)
            .range(MIN_FONT_SIZE..=MAX_FONT_SIZE)
            .max_decimals(1)
            .suffix(" pt"),
    )
    .on_hover_text("Font size")
    .changed()
}

pub fn format_pane_ui(ui: &mut Ui, ed: &mut Editor, assets: &AssetStore) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        match ed.selection.len() {
            0 => slide_format(ui, ed),
            1 => single(ui, ed, assets),
            n => {
                ui.heading(format!("{n} objects"));
                arrange(ui, ed);
            }
        }
    });
}

fn slide_format(ui: &mut Ui, ed: &mut Editor) {
    ui.heading(format!("Slide {}", ed.current + 1));
    section(ui, "BACKGROUND");
    ui.horizontal(|ui| {
        ui.label("Fill");
        let mut bg = ed.slide().background;
        if color_edit(ui, &mut bg) {
            let before = ed.slide().background;
            let sid = ed.slide_id();
            ed.exec_merging(
                Command::SetBackground {
                    slide: sid,
                    before,
                    after: bg,
                },
                Some(MergeKey::Background(sid)),
            );
        }
        if ui.button("Apply to All").clicked() {
            ed.apply_background_to_all();
        }
    });
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Click a text box on the slide to type. Write math like LaTeX: $f(x)$ inline, $$\\int f$$ centred.",
        )
        .small()
        .weak(),
    );
}

fn geometry(ui: &mut Ui, rect: &mut Rect, h_enabled: bool) {
    egui::Grid::new("geom")
        .num_columns(4)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("X");
            ui.add(DragValue::new(&mut rect.x).speed(0.5).max_decimals(1).suffix(" pt"));
            ui.label("Y");
            ui.add(DragValue::new(&mut rect.y).speed(0.5).max_decimals(1).suffix(" pt"));
            ui.end_row();
            ui.label("W");
            ui.add(
                DragValue::new(&mut rect.w)
                    .speed(0.5)
                    .range(1.0..=5000.0)
                    .max_decimals(1)
                    .suffix(" pt"),
            );
            ui.label("H");
            ui.add_enabled(
                h_enabled,
                DragValue::new(&mut rect.h)
                    .speed(0.5)
                    .range(1.0..=5000.0)
                    .max_decimals(1)
                    .suffix(" pt"),
            );
            ui.end_row();
        });
}

fn single(ui: &mut Ui, ed: &mut Editor, assets: &AssetStore) {
    let id = ed.selection[0];
    let Some(obj) = ed.object(id).cloned() else { return };
    ui.heading(obj.kind.label());
    let mut kind = obj.kind.clone();
    let mut rect = obj.rect;

    match &mut kind {
        ObjectKind::Text(t) => {
            section(ui, "FONT");
            ui.horizontal(|ui| {
                font_size_edit(ui, &mut t.font_size);
                color_edit(ui, &mut t.color);
                ui.toggle_value(&mut t.bold, RichText::new("B").strong());
                ui.toggle_value(&mut t.italic, RichText::new("I").italics());
            });
            section(ui, "PARAGRAPH");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut t.align, TextAlign::Left, "Left");
                ui.selectable_value(&mut t.align, TextAlign::Center, "Center");
                ui.selectable_value(&mut t.align, TextAlign::Right, "Right");
                let mut list = t.para(0).list != crate::document::ListKind::None;
                if ui.toggle_value(&mut list, "• Bullets").changed() {
                    let n = t.line_count();
                    let kind = if list {
                        crate::document::ListKind::Bullet
                    } else {
                        crate::document::ListKind::None
                    };
                    t.format_lines(0, n - 1, |p| p.list = kind);
                }
            });
        }
        ObjectKind::Image(i) => {
            section(ui, "PICTURE");
            match assets.get(&i.asset) {
                Some(m) => {
                    ui.label(if m.original_name.is_empty() {
                        "(unnamed)".to_string()
                    } else {
                        m.original_name.clone()
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} × {} px · {} · {:.1} MiB",
                            m.width,
                            m.height,
                            m.format.media_type(),
                            m.bytes as f64 / 1048576.0
                        ))
                        .small(),
                    );
                    if ui.button("Reset aspect ratio").clicked() && m.width > 0 {
                        rect.h = rect.w * m.height as f32 / m.width as f32;
                    }
                }
                None => {
                    ui.colored_label(Color32::RED, "Image data missing");
                }
            }
        }
        ObjectKind::Shape(s) => {
            section(ui, "LINE");
            ui.horizontal(|ui| {
                color_edit(ui, &mut s.stroke);
                ui.add(
                    DragValue::new(&mut s.stroke_width)
                        .speed(0.1)
                        .range(0.0..=30.0)
                        .max_decimals(2)
                        .suffix(" pt"),
                );
            });
            if !s.kind.is_linear() {
                section(ui, "FILL");
                ui.horizontal(|ui| {
                    let mut has_fill = s.fill.is_some();
                    if ui.checkbox(&mut has_fill, "Solid fill").changed() {
                        s.fill = has_fill.then_some(Color::LIGHT_FILL);
                    }
                    if let Some(f) = &mut s.fill {
                        color_edit(ui, f);
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    if ui.selectable_label(s.kind == ShapeKind::Line, "Line").clicked() {
                        s.kind = ShapeKind::Line;
                    }
                    if ui.selectable_label(s.kind == ShapeKind::Arrow, "Arrow").clicked() {
                        s.kind = ShapeKind::Arrow;
                    }
                    if ui.button("Reverse direction").clicked() {
                        s.reversed = !s.reversed;
                    }
                });
            }
        }
    }

    section(ui, "SIZE & POSITION");
    let h_ok = !matches!(kind, ObjectKind::Text(_));
    geometry(ui, &mut rect, h_ok);
    // Only user edits produce commands; showing the pane never edits.
    if kind != obj.kind || rect != obj.rect {
        ed.set_object(id, kind, rect.sanitized(), Some(MergeKey::Object(id, 1)));
    }
    arrange(ui, ed);
}

fn arrange(ui: &mut Ui, ed: &mut Editor) {
    section(
        ui,
        if ed.selection.len() > 1 {
            "ALIGN SELECTION"
        } else {
            "ALIGN TO SLIDE"
        },
    );
    ui.horizontal_wrapped(|ui| {
        for (label, how) in [
            ("Left", Align::Left),
            ("Center", Align::HCenter),
            ("Right", Align::Right),
            ("Top", Align::Top),
            ("Middle", Align::VCenter),
            ("Bottom", Align::Bottom),
        ] {
            if ui.small_button(label).clicked() {
                ed.align(how);
            }
        }
    });
    section(ui, "ORDER");
    ui.horizontal_wrapped(|ui| {
        for (label, how) in [
            ("Bring to Front", ZOrder::Front),
            ("Forward", ZOrder::Forward),
            ("Backward", ZOrder::Backward),
            ("Send to Back", ZOrder::Back),
        ] {
            if ui.small_button(label).clicked() {
                ed.z_order(how);
            }
        }
    });
}

/// Presentation title/author and the LaTeX preamble used for math and
/// export (Design ▸ Presentation & LaTeX).
pub fn properties_window(ctx: &egui::Context, open: &mut bool, ed: &mut Editor, math: &mut MathService) {
    egui::Window::new("Presentation & LaTeX")
        .open(open)
        .resizable(true)
        .default_width(460.0)
        .show(ctx, |ui| {
            let mut meta = ed.doc.meta.clone();
            egui::Grid::new("docmeta")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Title");
                    ui.add(egui::TextEdit::singleline(&mut meta.title).desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("Author");
                    ui.add(egui::TextEdit::singleline(&mut meta.author).desired_width(f32::INFINITY));
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label("LaTeX preamble — macros and packages used by all math and by the export:");
            ui.add(
                egui::TextEdit::multiline(&mut meta.preamble)
                    .code_editor()
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .hint_text(r"\newcommand{\R}{\mathbb{R}}"),
            );
            if meta != ed.doc.meta {
                let before = ed.doc.meta.clone();
                ed.exec_merging(Command::SetMeta { before, after: meta }, Some(MergeKey::Meta(0)));
            }
            ui.add_space(6.0);
            match math.toolchain() {
                Some(tc) => {
                    ui.label(RichText::new(format!("LaTeX engine: {}", tc.describe())).small());
                }
                None => {
                    ui.colored_label(Color32::from_rgb(200, 60, 40), crate::math::NO_ENGINE);
                }
            }
            if ui.small_button("Retry math that failed").clicked() {
                math.retry_failed();
            }
        });
}
