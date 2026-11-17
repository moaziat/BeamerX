//! Contextual property inspector (right panel).

use super::canvas::refit_text;
use super::{Align, Editor, ZOrder};
use crate::assets::AssetStore;
use crate::commands::{Command, MergeKey};
use crate::document::{Color, MAX_FONT_SIZE, MIN_FONT_SIZE, ObjectKind, Rect, ShapeKind, TextAlign};
use crate::math::{MathService, MathStatus};
use egui::{Color32, DragValue, RichText, Ui};
use std::time::Instant;

fn color_edit(ui: &mut Ui, c: &mut Color) -> bool {
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
    ui.add_space(6.0);
    ui.label(
        RichText::new(title)
            .small()
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(2.0);
}

/// Snippets for the equation palette: (label, LaTeX inserted).
const SNIPPETS: &[(&str, &str)] = &[
    ("a/b", r"\frac{a}{b}"),
    ("sqrt", r"\sqrt{x}"),
    ("x^n", r"x^{n}"),
    ("x_i", r"x_{i}"),
    ("sum", r"\sum_{i=1}^{n}"),
    ("int", r"\int_{a}^{b}"),
    ("∂", r"\partial"),
    ("∞", r"\infty"),
    ("α", r"\alpha"),
    ("β", r"\beta"),
    ("λ", r"\lambda"),
    ("π", r"\pi"),
    ("σ", r"\sigma"),
    ("θ", r"\theta"),
    ("to", r"\to"),
    ("≤", r"\leq"),
    ("≈", r"\approx"),
    ("( )", r"\left( x \right)"),
    ("[ ]", r"\begin{bmatrix} a & b \\ c & d \end{bmatrix}"),
    (
        "cases",
        r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
    ),
    ("align", r"\begin{aligned} a &= b + c \\ &= d \end{aligned}"),
];

pub fn inspector_ui(ui: &mut Ui, ed: &mut Editor, math: &mut MathService, assets: &AssetStore, now: Instant) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        match ed.selection.len() {
            0 => slide_and_document(ui, ed, math),
            1 => single(ui, ed, math, assets, now),
            n => {
                ui.heading(format!("{n} objects"));
                arrange(ui, ed);
            }
        }
    });
}

fn slide_and_document(ui: &mut Ui, ed: &mut Editor, math: &mut MathService) {
    ui.heading(format!("Slide {}", ed.current + 1));
    section(ui, "SLIDE");
    ui.horizontal(|ui| {
        ui.label("Background");
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
    });

    section(ui, "DOCUMENT");
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
    ui.label("LaTeX preamble (macros and packages, used for equations and export):");
    ui.add(
        egui::TextEdit::multiline(&mut meta.preamble)
            .code_editor()
            .desired_rows(5)
            .desired_width(f32::INFINITY)
            .hint_text(r"\newcommand{\R}{\mathbb{R}}"),
    );
    if meta != ed.doc.meta {
        let before = ed.doc.meta.clone();
        ed.exec_merging(Command::SetMeta { before, after: meta }, Some(MergeKey::Meta(0)));
    }

    section(ui, "LATEX ENGINE");
    match math.toolchain() {
        Some(tc) => {
            ui.label(RichText::new(tc.describe()).small());
        }
        None => {
            ui.colored_label(Color32::from_rgb(200, 60, 40), crate::math::NO_ENGINE);
        }
    }
    if ui.small_button("Retry failed equations").clicked() {
        math.retry_failed();
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Tip: type directly on the slide — click a selected text box (or just start typing) to edit it in place. Drop PNG/JPEG files from Finder onto the slide. Enter edits the selected equation.").small().weak());
}

fn geometry(ui: &mut Ui, rect: &mut Rect, w_enabled: bool, h_enabled: bool) {
    egui::Grid::new("geom")
        .num_columns(4)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("X");
            ui.add(DragValue::new(&mut rect.x).speed(0.5).max_decimals(1).suffix(" bp"));
            ui.label("Y");
            ui.add(DragValue::new(&mut rect.y).speed(0.5).max_decimals(1).suffix(" bp"));
            ui.end_row();
            ui.label("W");
            ui.add_enabled(
                w_enabled,
                DragValue::new(&mut rect.w)
                    .speed(0.5)
                    .range(1.0..=5000.0)
                    .max_decimals(1)
                    .suffix(" bp"),
            );
            ui.label("H");
            ui.add_enabled(
                h_enabled,
                DragValue::new(&mut rect.h)
                    .speed(0.5)
                    .range(1.0..=5000.0)
                    .max_decimals(1)
                    .suffix(" bp"),
            );
            ui.end_row();
        });
}

fn font_size(ui: &mut Ui, size: &mut f32) {
    ui.horizontal(|ui| {
        ui.label("Size");
        ui.add(
            DragValue::new(size)
                .speed(0.2)
                .range(MIN_FONT_SIZE..=MAX_FONT_SIZE)
                .max_decimals(1)
                .suffix(" bp"),
        );
        for s in [14.0, 20.0, 28.0, 40.0] {
            if ui.small_button(format!("{s}")).clicked() {
                *size = s;
            }
        }
    });
}

fn single(ui: &mut Ui, ed: &mut Editor, math: &mut MathService, assets: &AssetStore, now: Instant) {
    let id = ed.selection[0];
    let Some(obj) = ed.object(id).cloned() else {
        return;
    };
    ui.heading(obj.kind.label());
    let mut kind = obj.kind.clone();
    let mut rect = obj.rect;
    let mut focus_text = false;

    match &mut kind {
        ObjectKind::Text(t) => {
            section(ui, "TEXT");
            let te_id = egui::Id::unique(("insp-text", id.0));
            ui.add(
                egui::TextEdit::multiline(&mut t.text)
                    .id(te_id)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            font_size(ui, &mut t.font_size);
            ui.horizontal(|ui| {
                ui.label("Color");
                color_edit(ui, &mut t.color);
                ui.toggle_value(&mut t.bold, RichText::new("B").strong());
                ui.toggle_value(&mut t.italic, RichText::new("I").italics());
                ui.toggle_value(&mut t.bullets, "• List");
            });
            ui.horizontal(|ui| {
                ui.label("Align");
                ui.selectable_value(&mut t.align, TextAlign::Left, "Left");
                ui.selectable_value(&mut t.align, TextAlign::Center, "Center");
                ui.selectable_value(&mut t.align, TextAlign::Right, "Right");
            });
            focus_text = ui.memory(|m| m.has_focus(te_id));
        }
        ObjectKind::Equation(e) => {
            section(ui, "LATEX SOURCE");
            let te_id = egui::Id::unique(("insp-eq", id.0));
            let out = egui::TextEdit::multiline(&mut e.source)
                .id(te_id)
                .code_editor()
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .show(ui);
            if ed.focus_source {
                ed.focus_source = false;
                out.response.request_focus();
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                for (label, snippet) in SNIPPETS {
                    if ui.small_button(*label).on_hover_text(*snippet).clicked() {
                        if !e.source.is_empty() && !e.source.ends_with(' ') {
                            e.source.push(' ');
                        }
                        e.source.push_str(snippet);
                    }
                }
            });
            font_size(ui, &mut e.font_size);
            ui.horizontal(|ui| {
                ui.label("Color");
                color_edit(ui, &mut e.color);
            });
            section(ui, "PREVIEW");
            match math.request(&e.source, e.font_size, now) {
                MathStatus::Ready(r) => {
                    ui.label(
                        RichText::new(format!("Typeset by LaTeX ({:.1} × {:.1} bp)", r.size_bp.0, r.size_bp.1)).small(),
                    );
                }
                MathStatus::Pending => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Typesetting…").small());
                    });
                }
                MathStatus::Failed(msg) => {
                    ui.colored_label(Color32::from_rgb(200, 50, 40), "LaTeX error:");
                    ui.label(RichText::new(msg.as_ref()).monospace().small());
                }
            }
            focus_text = out.response.has_focus();
        }
        ObjectKind::Image(i) => {
            section(ui, "IMAGE");
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
                    if ui.button("Restore aspect ratio").clicked() && m.width > 0 {
                        rect.h = rect.w * m.height as f32 / m.width as f32;
                    }
                }
                None => {
                    ui.colored_label(Color32::RED, "Image data missing");
                }
            }
        }
        ObjectKind::Shape(s) => {
            section(ui, "SHAPE");
            ui.horizontal(|ui| {
                ui.label("Stroke");
                color_edit(ui, &mut s.stroke);
                ui.add(
                    DragValue::new(&mut s.stroke_width)
                        .speed(0.1)
                        .range(0.0..=30.0)
                        .max_decimals(2)
                        .suffix(" bp"),
                );
            });
            if !s.kind.is_linear() {
                ui.horizontal(|ui| {
                    let mut has_fill = s.fill.is_some();
                    if ui.checkbox(&mut has_fill, "Fill").changed() {
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

    section(ui, "GEOMETRY");
    let (w_ok, h_ok) = match &kind {
        ObjectKind::Equation(_) => (false, false),
        ObjectKind::Text(_) => (true, false),
        _ => (true, true),
    };
    geometry(ui, &mut rect, w_ok, h_ok);
    // Only user edits produce commands; merely showing the inspector must
    // never modify the document.
    if kind != obj.kind || rect != obj.rect {
        let rect = refit_text(ui.painter(), &kind, rect.sanitized());
        // Typing merges into one undo step; other edits merge per field burst.
        let field = if focus_text { 0 } else { 1 };
        ed.set_object(id, kind, rect, Some(MergeKey::Object(id, field)));
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
    section(ui, "ARRANGE");
    ui.horizontal_wrapped(|ui| {
        for (label, how) in [
            ("To Front", ZOrder::Front),
            ("Forward", ZOrder::Forward),
            ("Backward", ZOrder::Backward),
            ("To Back", ZOrder::Back),
        ] {
            if ui.small_button(label).clicked() {
                ed.z_order(how);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.small_button("Duplicate").clicked() {
            ed.duplicate_selection();
        }
        if ui.small_button("Delete").clicked() {
            ed.delete_selection();
        }
    });
}
