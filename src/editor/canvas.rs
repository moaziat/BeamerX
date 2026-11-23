//! The editing canvas: painting, hit-testing, selection, move/resize with
//! snapping, creation tools, zoom/pan and in-place text editing.
//!
//! Interaction follows PowerPoint: a click on a text box puts the caret
//! there; dragging a box (or the outline of the box being edited) moves it;
//! handles resize. Drags mutate only the dragged objects for live preview
//! and record a single undoable command when the gesture ends.

use super::text_edit::{self, Caret, Outcome, RichEdit};
use super::{Editor, Tool, Zoom, new_object_kind};
use crate::commands::Command;
use crate::document::{
    Color, MAX_FREEHAND_POINTS, MIN_EXTENT, ObjectId, ObjectKind, Rect, SLIDE_H, SLIDE_W, Shape, ShapeKind, typeset,
};
use crate::renderer::{self, PaintOpts, Services, View};
use egui::{Color32, CursorIcon, Key, Pos2, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};

const HANDLE: f32 = 7.0;
const HIT_TOL: f32 = 5.0;
const SNAP_PX: f32 = 6.0;
/// Width of the band around an edited text box that drags the box.
const EDIT_BAND: f32 = 9.0;
const ACCENT: Color32 = Color32::from_rgb(10, 132, 255);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    /// Box handle with x/y direction in {-1, 0, 1}.
    Box(i8, i8),
    /// Line endpoint: false = start, true = end.
    End(bool),
}

#[derive(Clone, Debug)]
enum Gesture {
    Move {
        start: (f32, f32),
        originals: Vec<(ObjectId, Rect)>,
        moved: bool,
    },
    Resize {
        id: ObjectId,
        handle: Handle,
        start: (f32, f32),
        rect: Rect,
        kind: ObjectKind,
    },
    Marquee {
        start: (f32, f32),
        current: (f32, f32),
        base: Vec<ObjectId>,
    },
    Create {
        tool: Tool,
        start: (f32, f32),
        current: (f32, f32),
    },
    Draw {
        points: Vec<(f32, f32)>,
    },
    Pan {
        last: Pos2,
    },
}

pub struct CanvasState {
    gesture: Option<Gesture>,
    guides: Vec<(bool, f32)>,
    /// Caret/selection of the text box being edited.
    pub rich: Option<RichEdit>,
    /// Where the caret goes when the next edit starts.
    edit_start: Caret,
    /// Last view used, for drop-position mapping and bench drivers.
    pub last_view: Option<View>,
    /// Pointer position over the slide (slide space), if hovering.
    pub hover_slide_pos: Option<(f32, f32)>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            gesture: None,
            guides: Vec::new(),
            rich: None,
            edit_start: Caret::End,
            last_view: None,
            hover_slide_pos: None,
        }
    }
}

impl CanvasState {
    pub fn is_gesturing(&self) -> bool {
        self.gesture.is_some()
    }
}

/// Start in-place editing of a text object.
pub fn begin_text_edit(ed: &mut Editor, st: &mut CanvasState, id: ObjectId, caret: Caret) {
    ed.selection = vec![id];
    ed.editing_text = Some(id);
    ed.tool = Tool::Select;
    st.rich = None;
    st.edit_start = caret;
}

/// Leave text editing; boxes left empty are removed (as in PowerPoint).
pub fn finish_text_edit(ed: &mut Editor, st: &mut CanvasState) {
    if let Some(id) = ed.editing_text.take() {
        ed.history.seal();
        if let Some(ObjectKind::Text(t)) = ed.object(id).map(|o| &o.kind)
            && t.text.trim().is_empty()
            && t.placeholder.is_none()
        {
            let keep = std::mem::take(&mut ed.selection);
            ed.selection = vec![id];
            ed.delete_selection();
            ed.selection = keep.into_iter().filter(|x| *x != id).collect();
        }
    }
    st.rich = None;
}

/// Delete an empty text box that currently owns the caret. Layout
/// placeholders start in text-edit mode when clicked, so without this bridge
/// Delete/Backspace would be routed to an empty string and become a no-op
/// instead of behaving like object deletion.
fn delete_empty_edited_box(ed: &mut Editor, st: &mut CanvasState) -> bool {
    let Some(id) = ed.editing_text else {
        return false;
    };
    let empty = matches!(ed.object(id).map(|o| &o.kind), Some(ObjectKind::Text(t)) if t.text.is_empty());
    if !empty {
        return false;
    }
    ed.history.seal();
    st.rich = None;
    ed.selection = vec![id];
    ed.delete_selection();
    true
}

/// Requests from the canvas to the application.
pub enum CanvasRequest {
    InsertImage,
    ShowFormatPane,
}

fn fitted_image_rect(width: u32, height: u32) -> Option<Rect> {
    if width == 0 || height == 0 {
        return None;
    }
    let margin = 12.0;
    let available = (SLIDE_W - margin * 2.0, SLIDE_H - margin * 2.0);
    let scale = (available.0 / width as f32).min(available.1 / height as f32);
    let (w, h) = (width as f32 * scale, height as f32 * scale);
    Some(Rect::new((SLIDE_W - w) * 0.5, (SLIDE_H - h) * 0.5, w, h))
}

fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Ramer-Douglas-Peucker simplification without recursion. It preserves the
/// visible shape while bounding save size and paint/export work.
fn simplify_freehand(points: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut spans = vec![(0usize, points.len() - 1)];
    while let Some((start, end)) = spans.pop() {
        let mut farthest = None;
        let mut max_distance = tolerance;
        for i in start + 1..end {
            let distance = dist_to_segment(points[i], points[start], points[end]);
            if distance > max_distance {
                max_distance = distance;
                farthest = Some(i);
            }
        }
        if let Some(i) = farthest {
            keep[i] = true;
            spans.push((start, i));
            spans.push((i, end));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(&point, keep)| keep.then_some(point))
        .collect()
}

fn insert_freehand(ed: &mut Editor, points: &[(f32, f32)], view_scale: f32) -> Option<ObjectId> {
    let points = simplify_freehand(points, (0.75 / view_scale).clamp(0.08, 0.75));
    if points.len() < 2 {
        return None;
    }
    let mut shape = Shape::new(ShapeKind::Freehand);
    shape.stroke = Color::BLACK;
    shape.stroke_width = 2.0;
    let rect = shape.set_freehand_points(&points)?;
    let id = ed.insert_object(ObjectKind::Shape(shape), rect);
    // The pen remains active for consecutive strokes, without resize handles
    // obscuring the line that was just drawn.
    ed.selection.clear();
    Some(id)
}

/// Topmost object under slide-space point `p` (tolerance in bp).
pub fn hit_test(ed: &Editor, p: (f32, f32), tol: f32) -> Option<ObjectId> {
    ed.slide().objects.iter().rev().find_map(|o| {
        let hit = match &o.kind {
            ObjectKind::Shape(s) if s.kind.is_linear() => {
                let (a, b) = s.endpoints(&o.rect);
                dist_to_segment(p, a, b) <= tol + s.stroke_width * 0.5
            }
            ObjectKind::Shape(s) if s.kind == ShapeKind::Freehand => s.points.windows(2).any(|pair| {
                let at = |(x, y): (f32, f32)| (o.rect.x + x * o.rect.w, o.rect.y + y * o.rect.h);
                dist_to_segment(p, at(pair[0]), at(pair[1])) <= tol + s.stroke_width * 0.5
            }),
            _ => o.rect.expanded(tol).contains(p),
        };
        hit.then_some(o.id)
    })
}

fn is_text(ed: &Editor, id: ObjectId) -> bool {
    matches!(ed.object(id).map(|o| &o.kind), Some(ObjectKind::Text(_)))
}

fn handles_for(ed: &Editor, view: &View) -> Vec<(Handle, Pos2)> {
    if ed.selection.len() != 1 {
        return Vec::new();
    }
    let Some(o) = ed.object(ed.selection[0]) else {
        return Vec::new();
    };
    match &o.kind {
        ObjectKind::Shape(s) if s.kind.is_linear() => {
            let (a, b) = s.endpoints(&o.rect);
            vec![
                (Handle::End(false), view.pt(a.0, a.1)),
                (Handle::End(true), view.pt(b.0, b.1)),
            ]
        }
        ObjectKind::Text(_) => {
            // Text height follows its content; only the width is resizable.
            let r = view.rect(&o.rect).expand(3.0);
            vec![
                (Handle::Box(-1, 0), r.left_center()),
                (Handle::Box(1, 0), r.right_center()),
            ]
        }
        _ => {
            let r = view.rect(&o.rect);
            let mut v = vec![
                (Handle::Box(-1, -1), r.left_top()),
                (Handle::Box(1, -1), r.right_top()),
                (Handle::Box(1, 1), r.right_bottom()),
                (Handle::Box(-1, 1), r.left_bottom()),
            ];
            if !o.kind.keeps_aspect() {
                v.extend([
                    (Handle::Box(0, -1), r.center_top()),
                    (Handle::Box(1, 0), r.right_center()),
                    (Handle::Box(0, 1), r.center_bottom()),
                    (Handle::Box(-1, 0), r.left_center()),
                ]);
            }
            v
        }
    }
}

fn cursor_for(h: Handle) -> CursorIcon {
    match h {
        Handle::End(_) => CursorIcon::Crosshair,
        Handle::Box(-1, -1) | Handle::Box(1, 1) => CursorIcon::ResizeNwSe,
        Handle::Box(1, -1) | Handle::Box(-1, 1) => CursorIcon::ResizeNeSw,
        Handle::Box(0, _) => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeHorizontal,
    }
}

/// New rect for a box resize. Corner drags keep the aspect ratio when
/// `aspect` is set.
fn resize_box(rect: Rect, h: (i8, i8), d: (f32, f32), aspect: bool) -> Rect {
    let (mut l, mut t, mut r, mut b) = (rect.x, rect.y, rect.right(), rect.bottom());
    match h.0 {
        -1 => l = (l + d.0).min(r - MIN_EXTENT),
        1 => r = (r + d.0).max(l + MIN_EXTENT),
        _ => {}
    }
    match h.1 {
        -1 => t = (t + d.1).min(b - MIN_EXTENT),
        1 => b = (b + d.1).max(t + MIN_EXTENT),
        _ => {}
    }
    let out = Rect::new(l, t, r - l, b - t);
    if aspect && h.0 != 0 && h.1 != 0 && rect.w > 0.0 && rect.h > 0.0 {
        let s = (out.w / rect.w).max(out.h / rect.h);
        let (w, hh) = (rect.w * s, rect.h * s);
        // Anchor at the opposite corner.
        let x = if h.0 < 0 { rect.right() - w } else { rect.x };
        let y = if h.1 < 0 { rect.bottom() - hh } else { rect.y };
        return Rect::new(x, y, w, hh);
    }
    out
}

/// Snap the moving bounds to slide edges/centre and other objects.
fn snap(ed: &Editor, moving: &[ObjectId], bounds: Rect, tol: f32) -> (f32, f32, Vec<(bool, f32)>) {
    let mut xs = vec![0.0, SLIDE_W * 0.5, SLIDE_W];
    let mut ys = vec![0.0, SLIDE_H * 0.5, SLIDE_H];
    for o in ed.slide().objects.iter().filter(|o| !moving.contains(&o.id)) {
        xs.extend([o.rect.x, o.rect.center().0, o.rect.right()]);
        ys.extend([o.rect.y, o.rect.center().1, o.rect.bottom()]);
    }
    let best = |edges: [f32; 3], lines: &[f32]| -> Option<(f32, f32)> {
        let mut best: Option<(f32, f32)> = None;
        for e in edges {
            for &l in lines {
                let d = l - e;
                if d.abs() <= tol && best.is_none_or(|(bd, _)| d.abs() < bd.abs()) {
                    best = Some((d, l));
                }
            }
        }
        best
    };
    let mut guides = Vec::new();
    let (cx, cy) = bounds.center();
    let dx = best([bounds.x, cx, bounds.right()], &xs).map_or(0.0, |(d, l)| {
        guides.push((true, l));
        d
    });
    let dy = best([bounds.y, cy, bounds.bottom()], &ys).map_or(0.0, |(d, l)| {
        guides.push((false, l));
        d
    });
    (dx, dy, guides)
}

/// Re-measure a text object's height after its content or width changed.
pub fn refit_text(p: &egui::Painter, sv: &mut Services, kind: &ObjectKind, mut rect: Rect) -> Rect {
    if let ObjectKind::Text(t) = kind {
        let lay = renderer::layout_text(p, sv, t, rect.w, 2.0, None);
        rect.h = (lay.height / 2.0).max(t.max_size() * typeset::LINE_SPREAD);
    }
    rect
}

/// Keep text box heights equal to their laid-out content (derived data that
/// changes when math finishes typesetting; not an undoable edit).
fn apply_text_heights(ed: &mut Editor, heights: &[(ObjectId, f32)], skip: Option<ObjectId>) {
    let i = ed.current;
    for &(id, h) in heights {
        if Some(id) == skip {
            continue;
        }
        if let Some(o) = ed.doc.slides[i].object_mut(id)
            && let ObjectKind::Text(t) = &o.kind
        {
            let h = h.max(t.max_size() * typeset::LINE_SPREAD);
            if (o.rect.h - h).abs() > 0.5 {
                o.rect.h = h;
            }
        }
    }
}

pub fn canvas_ui(ui: &mut Ui, ed: &mut Editor, st: &mut CanvasState, sv: &mut Services) -> Option<CanvasRequest> {
    let mut request = None;
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let area = resp.rect;
    painter.rect_filled(area, 0.0, crate::ui::palette_of(ui).workspace);

    // ---- view / zoom / pan ----------------------------------------------
    let fit = View::fit(area, 28.0);
    let mut view = match ed.zoom {
        Zoom::Fit => fit,
        Zoom::Manual { scale, center } => View {
            origin: area.center() - vec2(center.0, center.1) * scale,
            scale,
        },
    };
    if resp.hovered() || ui.rect_contains_pointer(area) {
        let (zoom, scroll, pointer) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        if (zoom - 1.0).abs() > 1e-4 {
            let p = pointer.unwrap_or(area.center());
            let sp = view.to_slide(p);
            let scale = (view.scale * zoom).clamp(fit.scale * 0.25, 16.0);
            let origin = p - vec2(sp.0, sp.1) * scale;
            let c = (area.center() - origin) / scale;
            ed.zoom = Zoom::Manual {
                scale,
                center: (c.x, c.y),
            };
        } else if scroll != Vec2::ZERO {
            let (scale, center) = match ed.zoom {
                Zoom::Fit => (fit.scale, (SLIDE_W * 0.5, SLIDE_H * 0.5)),
                Zoom::Manual { scale, center } => (scale, center),
            };
            ed.zoom = Zoom::Manual {
                scale,
                center: (center.0 - scroll.x / scale, center.1 - scroll.y / scale),
            };
        }
        if let Zoom::Manual { scale, center } = ed.zoom {
            view = View {
                origin: area.center() - vec2(center.0, center.1) * scale,
                scale,
            };
        }
    }
    st.last_view = Some(view);
    let pointer = ui.input(|i| i.pointer.hover_pos());
    st.hover_slide_pos = pointer.filter(|p| area.contains(*p)).map(|p| view.to_slide(p));
    let tol = HIT_TOL / view.scale;

    // ---- paint -----------------------------------------------------------
    let slide_rect = view.slide_rect();
    painter.rect_filled(
        slide_rect.translate(vec2(0.0, 2.0)).expand(1.0),
        2.0,
        Color32::from_black_alpha(40),
    );
    let editing = ed.editing_text;
    sv.text_heights = Some(Vec::new());
    renderer::paint_slide(
        &painter,
        &view,
        ed.slide(),
        sv,
        PaintOpts {
            skip: editing,
            editor_hints: true,
            theme: ed.doc.meta.theme,
        },
    );
    let heights = sv.text_heights.take().unwrap_or_default();
    if st.gesture.is_none() {
        apply_text_heights(ed, &heights, editing);
    }

    // A compact contextual menu for the selected image. Existing corner
    // handles remain the direct-manipulation resize path.
    let mut image_menu_active = false;
    if st.gesture.is_none()
        && ed.editing_text.is_none()
        && let [id] = ed.selection.as_slice()
        && let Some(o) = ed.object(*id).cloned()
        && let ObjectKind::Image(image) = &o.kind
    {
        let image_id = o.id;
        let meta = sv.assets.get(&image.asset).cloned();
        let screen = view.rect(&o.rect);
        let size = vec2(30.0, 24.0);
        let pos = egui::pos2(
            (screen.right() - size.x - 4.0).clamp(area.left() + 4.0, area.right() - size.x - 4.0),
            (screen.top() + 4.0).clamp(area.top() + 4.0, area.bottom() - size.y - 4.0),
        );
        let menu_rect = egui::Rect::from_min_size(pos, size);
        let button = ui
            .put(
                menu_rect,
                egui::Button::new(egui::RichText::new("•••").strong())
                    .min_size(size)
                    .corner_radius(5.0),
            )
            .on_hover_text("Image options");
        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Image options"));
        let popup_id = egui::Popup::default_response_id(&button);
        image_menu_active = button.contains_pointer() || egui::Popup::is_id_open(ui.ctx(), popup_id);
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(190.0);
            crate::ui::heading(ui, "IMAGE");
            if ui
                .add_enabled(meta.is_some(), egui::Button::new("Reset Aspect Ratio"))
                .clicked()
            {
                if let Some(meta) = &meta
                    && meta.width > 0
                {
                    let mut rect = o.rect;
                    rect.h = rect.w * meta.height as f32 / meta.width as f32;
                    ed.set_object(image_id, o.kind.clone(), rect.sanitized(), None);
                }
                ui.close();
            }
            if ui
                .add_enabled(meta.is_some(), egui::Button::new("Fit to Slide"))
                .clicked()
            {
                if let Some(meta) = &meta
                    && let Some(rect) = fitted_image_rect(meta.width, meta.height)
                {
                    ed.set_object(image_id, o.kind.clone(), rect, None);
                }
                ui.close();
            }
            if ui.button("Size & Position…").clicked() {
                request = Some(CanvasRequest::ShowFormatPane);
                ui.close();
            }
            ui.separator();
            if ui.button("Bring to Front").clicked() {
                ed.z_order(super::ZOrder::Front);
                ui.close();
            }
            if ui.button("Bring Forward").clicked() {
                ed.z_order(super::ZOrder::Forward);
                ui.close();
            }
            if ui.button("Send Backward").clicked() {
                ed.z_order(super::ZOrder::Backward);
                ui.close();
            }
            if ui.button("Send to Back").clicked() {
                ed.z_order(super::ZOrder::Back);
                ui.close();
            }
        });
    }

    // ---- keyboard ----------------------------------------------------------
    let deleted_empty_text = st.gesture.is_none()
        && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
        && delete_empty_edited_box(ed, st);
    let text_focus = ui.ctx().text_edit_focused();
    // A single selected text box captures typing: letters go into the box on
    // the slide instead of triggering tool shortcuts (as in PowerPoint).
    let selected_text = match ed.selection.as_slice() {
        [id] if is_text(ed, *id) => Some(*id),
        _ => None,
    };
    if !text_focus
        && editing.is_none()
        && st.gesture.is_none()
        && let Some(id) = selected_text
    {
        let typed = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_))));
        if typed {
            // The editor is created and focused later this frame and
            // receives these same keystrokes itself.
            begin_text_edit(ed, st, id, Caret::End);
        }
    }
    if !deleted_empty_text && !text_focus && ed.editing_text.is_none() && st.gesture.is_none() {
        let (mods, keys) = ui.input(|i| {
            let keys: Vec<Key> = [
                Key::Delete,
                Key::Backspace,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::ArrowUp,
                Key::ArrowDown,
                Key::Escape,
                Key::Enter,
                Key::V,
                Key::T,
                Key::R,
                Key::O,
                Key::L,
                Key::A,
                Key::P,
                Key::I,
            ]
            .into_iter()
            .filter(|k| i.key_pressed(*k))
            .collect();
            (i.modifiers, keys)
        });
        let step = if mods.shift { 10.0 } else { 1.0 };
        for k in keys {
            let plain = !mods.command && !mods.alt && !mods.ctrl && selected_text.is_none();
            match k {
                Key::Delete | Key::Backspace => ed.delete_selection(),
                Key::ArrowLeft => ed.nudge(-step, 0.0),
                Key::ArrowRight => ed.nudge(step, 0.0),
                Key::ArrowUp => ed.nudge(0.0, -step),
                Key::ArrowDown => ed.nudge(0.0, step),
                Key::Escape => {
                    if ed.tool != Tool::Select {
                        ed.set_tool(Tool::Select);
                    } else {
                        ed.selection.clear();
                    }
                }
                Key::Enter => {
                    if let Some(id) = selected_text {
                        begin_text_edit(ed, st, id, Caret::End);
                    }
                }
                Key::V if plain => ed.set_tool(Tool::Select),
                Key::T if plain => ed.set_tool(Tool::Text),
                Key::R if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Rectangle)),
                Key::O if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Ellipse)),
                Key::L if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Line)),
                Key::A if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Arrow)),
                Key::P if plain => ed.set_tool(Tool::Pen),
                Key::I if plain => request = Some(CanvasRequest::InsertImage),
                _ => {}
            }
        }
    }

    // ---- pointer gestures -------------------------------------------------
    let handles = handles_for(ed, &view);
    let mods = ui.input(|i| i.modifiers);
    let editing = ed.editing_text;
    // Screen rect of the box being edited, including its drag band.
    let edit_band = editing
        .and_then(|id| ed.object(id))
        .map(|o| view.rect(&o.rect).expand(EDIT_BAND));
    if let Some(p) = pointer.filter(|p| area.contains(*p))
        && st.gesture.is_none()
    {
        if ed.tool != Tool::Select {
            ui.ctx().set_cursor_icon(if ed.tool == Tool::Text {
                CursorIcon::Text
            } else {
                CursorIcon::Crosshair
            });
        } else if let Some((h, _)) = handles.iter().find(|(_, hp)| hp.distance(p) <= HANDLE) {
            ui.ctx().set_cursor_icon(cursor_for(*h));
        } else if edit_band.is_some_and(|b| b.contains(p)) {
            ui.ctx().set_cursor_icon(CursorIcon::Move);
        } else if let Some(id) = hit_test(ed, view.to_slide(p), tol) {
            // Text boxes take a caret on click; everything moves on drag.
            ui.ctx().set_cursor_icon(if is_text(ed, id) {
                CursorIcon::Text
            } else {
                CursorIcon::Move
            });
        }
    }

    if !image_menu_active
        && resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let sp = view.to_slide(p);
        let space = ui.input(|i| i.key_down(Key::Space));
        let handle = handles.iter().find(|(_, hp)| hp.distance(p) <= HANDLE).map(|(h, _)| *h);
        let on_edit_band = edit_band.is_some_and(|b| b.contains(p));
        if editing.is_some() && handle.is_none() && !on_edit_band {
            finish_text_edit(ed, st);
        }
        st.gesture = if space || resp.dragged_by(egui::PointerButton::Middle) {
            Some(Gesture::Pan { last: p })
        } else if ed.tool == Tool::Pen {
            slide_rect.contains(p).then(|| Gesture::Draw { points: vec![sp] })
        } else if ed.tool != Tool::Select {
            Some(Gesture::Create {
                tool: ed.tool,
                start: sp,
                current: sp,
            })
        } else if let Some(h) = handle {
            let o = ed.object(ed.selection[0]).cloned();
            o.map(|o| Gesture::Resize {
                id: o.id,
                handle: h,
                start: sp,
                rect: o.rect,
                kind: o.kind,
            })
        } else if let (true, Some(id)) = (on_edit_band, editing) {
            // Dragging the outline of the box being edited moves it.
            let r = ed.object(id).map(|o| o.rect).unwrap_or_default();
            Some(Gesture::Move {
                start: sp,
                originals: vec![(id, r)],
                moved: false,
            })
        } else if let Some(id) = hit_test(ed, sp, tol) {
            if !ed.selection.contains(&id) {
                if mods.shift {
                    ed.selection.push(id);
                } else {
                    ed.selection = vec![id];
                }
            }
            let s = ed.slide();
            let originals = ed
                .selection
                .iter()
                .filter_map(|id| s.object(*id))
                .map(|o| (o.id, o.rect))
                .collect();
            Some(Gesture::Move {
                start: sp,
                originals,
                moved: false,
            })
        } else {
            let base = if mods.shift { ed.selection.clone() } else { Vec::new() };
            ed.selection = base.clone();
            Some(Gesture::Marquee {
                start: sp,
                current: sp,
                base,
            })
        };
    }

    if resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let sp = view.to_slide(p);
        st.guides.clear();
        match &mut st.gesture {
            Some(Gesture::Pan { last }) => {
                let d = p - *last;
                *last = p;
                let (scale, center) = match ed.zoom {
                    Zoom::Fit => (view.scale, view.to_slide(area.center())),
                    Zoom::Manual { scale, center } => (scale, center),
                };
                ed.zoom = Zoom::Manual {
                    scale,
                    center: (center.0 - d.x / scale, center.1 - d.y / scale),
                };
            }
            Some(Gesture::Create { current, .. }) => *current = sp,
            Some(Gesture::Draw { points }) => {
                let point = (sp.0.clamp(0.0, SLIDE_W), sp.1.clamp(0.0, SLIDE_H));
                let far_enough = points.last().is_none_or(|last| {
                    let dx = point.0 - last.0;
                    let dy = point.1 - last.1;
                    (dx * dx + dy * dy).sqrt() * view.scale >= 1.25
                });
                if far_enough && points.len() < MAX_FREEHAND_POINTS {
                    points.push(point);
                }
            }
            Some(Gesture::Marquee { start, current, base }) => {
                *current = sp;
                let r = Rect::from_points(*start, *current);
                let mut sel = base.clone();
                for o in &ed.slide().objects {
                    if r.intersects(&o.rect) && !sel.contains(&o.id) {
                        sel.push(o.id);
                    }
                }
                ed.selection = sel;
            }
            Some(Gesture::Move {
                start,
                originals,
                moved,
            }) => {
                let (mut dx, mut dy) = (sp.0 - start.0, sp.1 - start.1);
                if *moved || (dx.abs() + dy.abs()) * view.scale >= 2.0 {
                    *moved = true;
                    if mods.shift {
                        // Constrain to the dominant axis.
                        if dx.abs() > dy.abs() { dy = 0.0 } else { dx = 0.0 }
                    }
                    let ids: Vec<ObjectId> = originals.iter().map(|(id, _)| *id).collect();
                    if !mods.command
                        && let Some(b) = originals.iter().map(|(_, r)| *r).reduce(|a, b| a.union(&b))
                    {
                        let (sx, sy, guides) = snap(ed, &ids, b.translated(dx, dy), SNAP_PX / view.scale);
                        dx += sx;
                        dy += sy;
                        st.guides = guides;
                    }
                    for (id, r) in originals.iter() {
                        if let Some(o) = ed.object_mut_for_preview(*id) {
                            o.rect = r.translated(dx, dy);
                        }
                    }
                }
            }
            Some(Gesture::Resize {
                id,
                handle,
                start,
                rect,
                kind,
            }) => {
                let d = (sp.0 - start.0, sp.1 - start.1);
                let (id, handle, rect0, kind0) = (*id, *handle, *rect, kind.clone());
                let new_rect = match (handle, &kind0) {
                    (Handle::End(end), ObjectKind::Shape(s0)) => {
                        let (mut a, mut b) = s0.endpoints(&rect0);
                        let target = if end { &mut b } else { &mut a };
                        *target = (target.0 + d.0, target.1 + d.1);
                        if mods.shift {
                            // Snap to horizontal / vertical / 45°.
                            let other = if end { a } else { b };
                            let t = if end { b } else { a };
                            let (vx, vy) = (t.0 - other.0, t.1 - other.1);
                            let q = (vy.atan2(vx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
                            let len = (vx * vx + vy * vy).sqrt();
                            let t = (other.0 + len * q.cos(), other.1 + len * q.sin());
                            if end { b = t } else { a = t }
                        }
                        let mut s = s0.clone();
                        let r = s.set_endpoints(a, b);
                        Some((r, ObjectKind::Shape(s)))
                    }
                    (Handle::Box(hx, hy), _) => {
                        let aspect = kind0.keeps_aspect() ^ mods.shift;
                        let r = resize_box(rect0, (hx, hy), d, aspect);
                        Some((refit_text(&painter, sv, &kind0, r), kind0.clone()))
                    }
                    _ => None,
                };
                if let Some((r, k)) = new_rect
                    && let Some(o) = ed.object_mut_for_preview(id)
                {
                    o.rect = r;
                    o.kind = k;
                }
            }
            None => {}
        }
    }

    if resp.drag_stopped() {
        st.guides.clear();
        match st.gesture.take() {
            Some(Gesture::Move {
                originals, moved: true, ..
            }) => {
                let s = ed.slide();
                let changes: Vec<_> = originals
                    .iter()
                    .filter_map(|(id, r0)| s.object(*id).map(|o| (*id, *r0, o.rect)))
                    .filter(|(_, a, b)| a != b)
                    .collect();
                if !changes.is_empty() {
                    ed.record(Command::SetRects {
                        slide: ed.slide_id(),
                        changes,
                    });
                }
            }
            Some(Gesture::Resize { id, rect, kind, .. }) => {
                if let Some(o) = ed.object(id).cloned()
                    && (o.rect != rect || o.kind != kind)
                {
                    ed.record(Command::SetObject {
                        slide: ed.slide_id(),
                        id,
                        before: (kind, rect),
                        after: (o.kind, o.rect),
                    });
                }
            }
            Some(Gesture::Create { tool, start, current }) => {
                let dragged = (current.0 - start.0).abs().max((current.1 - start.1).abs()) * view.scale > 4.0;
                create_object(ed, st, &painter, sv, tool, start, current, dragged);
            }
            Some(Gesture::Draw { points }) => {
                insert_freehand(ed, &points, view.scale);
            }
            _ => {}
        }
    }

    // Double-click on empty slide space creates a text box there.
    let mut created_by_double_click = false;
    // (A quick click just before can make egui count a triple-click.)
    if !image_menu_active
        && (resp.double_clicked() || resp.triple_clicked())
        && ed.tool == Tool::Select
        && let Some(p) = resp.interact_pointer_pos()
        && edit_band.is_none_or(|b| !b.contains(p))
    {
        let sp = view.to_slide(p);
        if hit_test(ed, sp, tol).is_none() && view.slide_rect().contains(p) {
            if editing.is_some() {
                finish_text_edit(ed, st);
            }
            let t = crate::document::TextBox::default();
            // Put the first baseline roughly where the user clicked.
            let top = sp.1 - t.font_size * typeset::FIRST_BASELINE * 0.7;
            let w = (SLIDE_W - sp.0 - 8.0).clamp(60.0, 260.0);
            let rect = Rect::new(sp.0, top, w, t.font_size * typeset::LINE_SPREAD);
            let id = ed.insert_object(ObjectKind::Text(t), rect);
            begin_text_edit(ed, st, id, Caret::End);
            ed.history.seal();
            created_by_double_click = true;
        }
    }

    if !image_menu_active
        && resp.clicked()
        && !created_by_double_click
        && let Some(p) = resp.interact_pointer_pos()
    {
        let sp = view.to_slide(p);
        let on_edit_band = edit_band.is_some_and(|b| b.contains(p));
        if !on_edit_band {
            if editing.is_some() {
                finish_text_edit(ed, st);
            }
            let hit = hit_test(ed, sp, tol);
            if ed.tool == Tool::Text && hit.is_some_and(|id| is_text(ed, id)) {
                // With the text tool, clicking existing text edits it.
                begin_text_edit(ed, st, hit.expect("checked"), Caret::At(p));
            } else if ed.tool != Tool::Select {
                create_object(ed, st, &painter, sv, ed.tool, sp, sp, false);
            } else {
                match hit {
                    Some(id) if mods.shift => {
                        if let Some(i) = ed.selection.iter().position(|x| *x == id) {
                            ed.selection.remove(i);
                        } else {
                            ed.selection.push(id);
                        }
                    }
                    // One click on text puts the caret there (PowerPoint).
                    Some(id) if is_text(ed, id) => begin_text_edit(ed, st, id, Caret::At(p)),
                    Some(id) => ed.selection = vec![id],
                    None => ed.selection.clear(),
                }
            }
            ed.history.seal();
        }
    }

    // Context menu.
    resp.context_menu(|ui| {
        if let Some(p) = ui.ctx().input(|i| i.pointer.press_origin())
            && ed.selection.is_empty()
            && let Some(id) = hit_test(ed, view.to_slide(p), tol)
        {
            ed.selection = vec![id];
        }
        let has_sel = !ed.selection.is_empty();
        if ui.add_enabled(has_sel, egui::Button::new("Cut")).clicked() {
            ed.cut_selection();
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Copy")).clicked() {
            ed.copy_selection();
            ui.close();
        }
        if ui
            .add_enabled(!ed.clipboard.is_empty(), egui::Button::new("Paste"))
            .clicked()
        {
            ed.paste();
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Duplicate")).clicked() {
            ed.duplicate_selection();
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Delete")).clicked() {
            ed.delete_selection();
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(has_sel, egui::Button::new("Bring to Front")).clicked() {
            ed.z_order(super::ZOrder::Front);
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Bring Forward")).clicked() {
            ed.z_order(super::ZOrder::Forward);
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Send Backward")).clicked() {
            ed.z_order(super::ZOrder::Backward);
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Send to Back")).clicked() {
            ed.z_order(super::ZOrder::Back);
            ui.close();
        }
    });

    // ---- overlays ----------------------------------------------------------
    let ov = painter.clone();
    for (vertical, v) in &st.guides {
        let (a, b) = if *vertical {
            (view.pt(*v, -2000.0), view.pt(*v, 2000.0))
        } else {
            (view.pt(-2000.0, *v), view.pt(2000.0, *v))
        };
        ov.line_segment([a, b], Stroke::new(1.0, Color32::from_rgb(255, 45, 85)));
    }
    for id in &ed.selection {
        if Some(*id) == ed.editing_text {
            continue; // the editor draws its own outline
        }
        if let Some(o) = ed.object(*id) {
            match &o.kind {
                ObjectKind::Shape(s) if s.kind.is_linear() => {
                    let (a, b) = s.endpoints(&o.rect);
                    ov.line_segment([view.pt(a.0, a.1), view.pt(b.0, b.1)], Stroke::new(1.0, ACCENT));
                }
                _ => {
                    ov.rect_stroke(
                        view.rect(&o.rect).expand(1.0),
                        0.0,
                        Stroke::new(1.0, ACCENT),
                        StrokeKind::Outside,
                    );
                }
            }
        }
    }
    for (_, hp) in handles_for(ed, &view) {
        let r = egui::Rect::from_center_size(hp, Vec2::splat(HANDLE));
        ov.rect_filled(r, 1.5, Color32::WHITE);
        ov.rect_stroke(r, 1.5, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
    }
    match &st.gesture {
        Some(Gesture::Marquee { start, current, .. }) => {
            let r = view.rect(&Rect::from_points(*start, *current));
            ov.rect_filled(r, 0.0, ACCENT.gamma_multiply(0.12));
            ov.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        }
        Some(Gesture::Create { tool, start, current }) => {
            if let Tool::Shape(k) = tool
                && k.is_linear()
            {
                ov.line_segment(
                    [view.pt(start.0, start.1), view.pt(current.0, current.1)],
                    Stroke::new(1.5, ACCENT),
                );
            } else {
                let r = view.rect(&Rect::from_points(*start, *current));
                ov.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
            }
        }
        Some(Gesture::Draw { points }) if points.len() >= 2 => {
            ov.add(egui::Shape::line(
                points.iter().map(|&(x, y)| view.pt(x, y)).collect(),
                Stroke::new(2.0 * view.scale, Color32::from_gray(25)),
            ));
        }
        _ => {}
    }

    // ---- in-place text editing ----------------------------------------------
    if let Some(id) = ed.editing_text {
        let start = st.edit_start;
        match text_edit::editor_ui(ui, ed, &mut st.rich, &view, sv, id, start) {
            Outcome::Continue => {}
            Outcome::Finished => {
                finish_text_edit(ed, st);
                if ed.object(id).is_some() {
                    ed.selection = vec![id];
                }
            }
        }
    } else {
        st.rich = None;
    }
    request
}

#[allow(clippy::too_many_arguments)]
fn create_object(
    ed: &mut Editor,
    st: &mut CanvasState,
    p: &egui::Painter,
    sv: &mut Services,
    tool: Tool,
    start: (f32, f32),
    end: (f32, f32),
    dragged: bool,
) {
    let Some(mut kind) = new_object_kind(tool) else { return };
    let rect = match (&mut kind, dragged) {
        (ObjectKind::Shape(s), true) if s.kind.is_linear() => s.set_endpoints(start, end),
        (ObjectKind::Shape(s), false) if s.kind.is_linear() => s.set_endpoints(start, (start.0 + 80.0, start.1)),
        (ObjectKind::Shape(_), true) => Rect::from_points(start, end),
        (ObjectKind::Shape(Shape { kind: k, .. }), false) => {
            let (w, h) = k.default_size();
            Rect::new(start.0 - w * 0.5, start.1 - h * 0.5, w, h)
        }
        (ObjectKind::Text(t), d) => {
            let w = if d { (end.0 - start.0).abs().max(20.0) } else { 240.0 };
            Rect::new(
                start.0.min(end.0),
                start.1.min(end.1),
                w,
                t.font_size * typeset::LINE_SPREAD,
            )
        }
        _ => return,
    };
    let rect = refit_text(p, sv, &kind, rect);
    let is_text = matches!(kind, ObjectKind::Text(_));
    let id = ed.insert_object(kind, rect);
    ed.set_tool(Tool::Select);
    if is_text {
        begin_text_edit(ed, st, id, Caret::End);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_aspect_and_anchors_opposite_corner() {
        let r = resize_box(Rect::new(10.0, 10.0, 100.0, 50.0), (-1, -1), (-50.0, -5.0), true);
        assert!((r.right() - 110.0).abs() < 1e-3 && (r.bottom() - 60.0).abs() < 1e-3);
        assert!((r.w / r.h - 2.0).abs() < 1e-3);
        let r = resize_box(Rect::new(0.0, 0.0, 100.0, 20.0), (1, 1), (100.0, 0.0), true);
        assert!((r.w - 200.0).abs() < 0.5 && (r.h - 40.0).abs() < 0.5, "{r:?}");
    }

    #[test]
    fn resize_never_inverts() {
        let r = resize_box(Rect::new(0.0, 0.0, 10.0, 10.0), (1, 0), (-50.0, 0.0), false);
        assert_eq!(r.w, MIN_EXTENT);
    }

    #[test]
    fn freehand_simplification_preserves_ends_and_corners() {
        let straight: Vec<_> = (0..100).map(|x| (x as f32, 0.0)).collect();
        assert_eq!(simplify_freehand(&straight, 0.1), [(0.0, 0.0), (99.0, 0.0)]);
        let corner = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        assert_eq!(simplify_freehand(&corner, 0.1), corner);
    }

    #[test]
    fn freehand_hit_test_follows_stroke_not_bounding_box() {
        let mut ed = Editor::new(crate::document::Document::new(), None);
        let mut shape = Shape::new(ShapeKind::Freehand);
        let rect = shape
            .set_freehand_points(&[(10.0, 10.0), (110.0, 10.0), (110.0, 110.0)])
            .unwrap();
        let id = ed.insert_object(ObjectKind::Shape(shape), rect);
        assert_eq!(hit_test(&ed, (30.0, 11.0), 2.0), Some(id));
        assert_eq!(hit_test(&ed, (55.0, 55.0), 2.0), None);
    }

    #[test]
    fn inserting_freehand_is_undoable_and_keeps_pen_active() {
        let mut ed = Editor::new(crate::document::Document::new(), None);
        ed.set_tool(Tool::Pen);
        let id = insert_freehand(&mut ed, &[(10.0, 10.0), (20.0, 20.0), (30.0, 10.0)], 2.0).unwrap();
        assert_eq!(ed.tool, Tool::Pen);
        assert!(ed.selection.is_empty());
        assert!(matches!(ed.object(id).map(|o| &o.kind), Some(ObjectKind::Shape(s)) if s.kind == ShapeKind::Freehand));
        ed.undo();
        assert!(ed.object(id).is_none());
        ed.redo();
        assert!(ed.object(id).is_some());
    }

    #[test]
    fn fitted_image_stays_inside_slide_and_keeps_aspect() {
        let wide = fitted_image_rect(1920, 1080).unwrap();
        assert!((wide.w / wide.h - 16.0 / 9.0).abs() < 1e-3);
        assert!(wide.x >= 12.0 && wide.y >= 12.0);
        assert!(wide.right() <= SLIDE_W - 12.0 + 1e-3);
        assert!(wide.bottom() <= SLIDE_H - 12.0 + 1e-3);
        assert!(fitted_image_rect(0, 10).is_none());
    }

    #[test]
    fn segment_distance() {
        assert_eq!(dist_to_segment((5.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 3.0);
        assert_eq!(dist_to_segment((-4.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 5.0);
    }

    #[test]
    fn delete_key_removes_empty_layout_placeholder_and_is_undoable() {
        let mut ed = Editor::new(crate::editor::layouts::new_presentation(), None);
        let id = ed.doc.slides[0].objects[0].id;
        let mut st = CanvasState::default();
        begin_text_edit(&mut ed, &mut st, id, Caret::End);

        assert!(delete_empty_edited_box(&mut ed, &mut st));
        assert!(ed.object(id).is_none());
        assert!(ed.editing_text.is_none());

        ed.undo();
        assert!(ed.object(id).is_some());
    }

    #[test]
    fn delete_key_stays_with_text_editor_when_box_has_content() {
        let mut doc = crate::editor::layouts::new_presentation();
        let id = doc.slides[0].objects[0].id;
        let Some(ObjectKind::Text(t)) = doc.slides[0].object_mut(id).map(|o| &mut o.kind) else {
            unreachable!()
        };
        t.text = "Title".into();
        let mut ed = Editor::new(doc, None);
        let mut st = CanvasState::default();
        begin_text_edit(&mut ed, &mut st, id, Caret::End);

        assert!(!delete_empty_edited_box(&mut ed, &mut st));
        assert!(ed.object(id).is_some());
        assert_eq!(ed.editing_text, Some(id));
    }
}
