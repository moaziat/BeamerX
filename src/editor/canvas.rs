//! The editing canvas: painting, hit-testing, selection, move/resize with
//! snapping, creation tools, zoom/pan and in-place text editing.
//!
//! Drags mutate only the dragged objects' rectangles for live preview and
//! record a single undoable command when the gesture ends.

use super::{Editor, Tool, Zoom, new_object_kind};
use crate::commands::{Command, MergeKey};
use crate::document::{
    MAX_FONT_SIZE, MIN_EXTENT, MIN_FONT_SIZE, ObjectId, ObjectKind, Rect, SLIDE_H, SLIDE_W, Shape, TextAlign, TextBox,
    typeset,
};
use crate::renderer::{self, PaintOpts, Services, View, c32, text_family};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CursorIcon, FontId, Key, Pos2, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

const HANDLE: f32 = 7.0;
const HIT_TOL: f32 = 5.0;
const SNAP_PX: f32 = 6.0;
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
    Pan {
        last: Pos2,
    },
}

#[derive(Default)]
pub struct CanvasState {
    gesture: Option<Gesture>,
    guides: Vec<(bool, f32)>,
    edit_buffer: String,
    edit_for: Option<ObjectId>,
    /// Frames since in-place editing started.
    edit_age: u32,
    /// Where to put the caret when in-place editing starts.
    caret: Caret,
    /// Last view used, for drop-position mapping and bench drivers.
    pub last_view: Option<View>,
    /// Pointer position over the slide (slide space), if hovering.
    pub hover_slide_pos: Option<(f32, f32)>,
}

impl CanvasState {
    pub fn is_gesturing(&self) -> bool {
        self.gesture.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Caret {
    #[default]
    End,
    /// Screen position of the click that started editing.
    At(Pos2),
}

/// Start in-place editing of a text object.
fn begin_text_edit(ed: &mut Editor, st: &mut CanvasState, id: ObjectId, caret: Caret) {
    ed.selection = vec![id];
    ed.editing_text = Some(id);
    st.edit_for = None;
    st.caret = caret;
}

/// Requests from the canvas to the application.
pub enum CanvasRequest {
    InsertImage,
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

/// Topmost object under slide-space point `p` (tolerance in bp).
pub fn hit_test(ed: &Editor, p: (f32, f32), tol: f32) -> Option<ObjectId> {
    ed.slide().objects.iter().rev().find_map(|o| {
        let hit = match &o.kind {
            ObjectKind::Shape(s) if s.kind.is_linear() => {
                let (a, b) = s.endpoints(&o.rect);
                dist_to_segment(p, a, b) <= tol + s.stroke_width * 0.5
            }
            _ => o.rect.expanded(tol).contains(p),
        };
        hit.then_some(o.id)
    })
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
            let r = view.rect(&o.rect);
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

/// New rect (and kind, for equations) for a box resize.
fn resize_box(rect: Rect, kind: &ObjectKind, h: (i8, i8), d: (f32, f32), aspect: bool) -> (Rect, ObjectKind) {
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
    let mut out = Rect::new(l, t, r - l, b - t);
    let mut kind = kind.clone();
    let corner = h.0 != 0 && h.1 != 0;
    if aspect && corner && rect.w > 0.0 && rect.h > 0.0 {
        let mut s = (out.w / rect.w).max(out.h / rect.h);
        if let ObjectKind::Equation(e) = &mut kind {
            let f = (e.font_size * s).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
            s = f / e.font_size;
            e.font_size = (f * 10.0).round() / 10.0;
        }
        let (w, hh) = (rect.w * s, rect.h * s);
        // Anchor at the opposite corner.
        let x = if h.0 < 0 { rect.right() - w } else { rect.x };
        let y = if h.1 < 0 { rect.bottom() - hh } else { rect.y };
        out = Rect::new(x, y, w, hh);
    }
    (out, kind)
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

/// Text layout job matching the renderer (for the in-place editor).
fn text_job(text: &str, t: &TextBox, scale: f32, wrap: f32) -> LayoutJob {
    let size = t.font_size * scale;
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    job.halign = match t.align {
        TextAlign::Left => egui::Align::LEFT,
        TextAlign::Center => egui::Align::Center,
        TextAlign::Right => egui::Align::RIGHT,
    };
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::new(size, text_family(t.bold, t.italic)),
            color: c32(t.color),
            line_height: Some(size * typeset::LINE_SPREAD),
            ..Default::default()
        },
    );
    job
}

/// Re-measure a text object's height after its content or width changed.
pub fn refit_text(p: &egui::Painter, kind: &ObjectKind, mut rect: Rect) -> Rect {
    if let ObjectKind::Text(t) = kind {
        rect.h = renderer::text_height_bp(p, t, rect.w).max(t.font_size * typeset::LINE_SPREAD);
    }
    rect
}

/// Keep equation rects equal to their typeset size (derived data; not an
/// undoable edit).
pub fn sync_equation_sizes(ed: &mut Editor, sv: &Services) {
    let i = ed.current;
    let updates: Vec<(ObjectId, (f32, f32))> = ed.doc.slides[i]
        .objects
        .iter()
        .filter_map(|o| match &o.kind {
            ObjectKind::Equation(e) => sv.math.peek(&e.source, e.font_size).and_then(|r| {
                let (w, h) = r.size_bp;
                ((o.rect.w - w).abs() > 0.01 || (o.rect.h - h).abs() > 0.01).then_some((o.id, (w, h)))
            }),
            _ => None,
        })
        .collect();
    for (id, (w, h)) in updates {
        if let Some(o) = ed.doc.slides[i].object_mut(id) {
            o.rect.w = w;
            o.rect.h = h;
        }
    }
}

pub fn canvas_ui(ui: &mut Ui, ed: &mut Editor, st: &mut CanvasState, sv: &mut Services) -> Option<CanvasRequest> {
    let mut request = None;
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let area = resp.rect;
    let dark = ui.visuals().dark_mode;
    painter.rect_filled(
        area,
        0.0,
        if dark {
            Color32::from_gray(38)
        } else {
            Color32::from_gray(222)
        },
    );

    // ---- view / zoom / pan ----------------------------------------------
    let fit = View::fit(area, 28.0);
    let mut view = match ed.zoom {
        Zoom::Fit => fit,
        Zoom::Manual { scale, center } => View {
            origin: area.center() - vec2(center.0, center.1) * scale,
            scale,
        },
    };
    if resp.hovered() {
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
    renderer::paint_slide(
        &painter,
        &view,
        ed.slide(),
        sv,
        PaintOpts {
            skip: editing,
            editor_hints: true,
        },
    );

    // ---- keyboard ----------------------------------------------------------
    let text_focus = ui.ctx().text_edit_focused();
    // A single selected text box captures typing: letters go into the box on
    // the slide instead of triggering tool shortcuts.
    let selected_text = match ed.selection.as_slice() {
        [id] if matches!(ed.object(*id).map(|o| &o.kind), Some(ObjectKind::Text(_))) => Some(*id),
        _ => None,
    };
    if !text_focus
        && editing.is_none()
        && st.gesture.is_none()
        && let Some(id) = selected_text
    {
        let typed: String = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect()
        });
        if !typed.is_empty() {
            // The in-place editor is created and focused later this frame,
            // so it receives these same keystrokes itself.
            begin_text_edit(ed, st, id, Caret::End);
        }
    }
    if !text_focus && ed.editing_text.is_none() && st.gesture.is_none() {
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
                Key::E,
                Key::R,
                Key::O,
                Key::L,
                Key::A,
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
                Key::Enter if ed.selection.len() == 1 => match ed.object(ed.selection[0]).map(|o| &o.kind) {
                    Some(ObjectKind::Text(_)) => begin_text_edit(ed, st, ed.selection[0], Caret::End),
                    Some(ObjectKind::Equation(_)) => ed.focus_source = true,
                    _ => {}
                },
                Key::V if plain => ed.set_tool(Tool::Select),
                Key::T if plain => ed.set_tool(Tool::Text),
                Key::E if plain => ed.set_tool(Tool::Equation),
                Key::R if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Rectangle)),
                Key::O if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Ellipse)),
                Key::L if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Line)),
                Key::A if plain => ed.set_tool(Tool::Shape(crate::document::ShapeKind::Arrow)),
                Key::I if plain => request = Some(CanvasRequest::InsertImage),
                _ => {}
            }
        }
    }

    // ---- pointer gestures -------------------------------------------------
    let handles = handles_for(ed, &view);
    let mods = ui.input(|i| i.modifiers);
    if let Some(p) = pointer.filter(|p| area.contains(*p))
        && st.gesture.is_none()
    {
        if ed.tool != Tool::Select {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        } else if let Some((h, _)) = handles.iter().find(|(_, hp)| hp.distance(p) <= HANDLE) {
            ui.ctx().set_cursor_icon(cursor_for(*h));
        } else if hit_test(ed, view.to_slide(p), tol).is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Move);
        }
    }

    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let sp = view.to_slide(p);
        let space = ui.input(|i| i.key_down(Key::Space));
        if editing.is_some() {
            finish_text_edit(ed, st);
        }
        st.gesture = if space || resp.dragged_by(egui::PointerButton::Middle) {
            Some(Gesture::Pan { last: p })
        } else if ed.tool != Tool::Select {
            Some(Gesture::Create {
                tool: ed.tool,
                start: sp,
                current: sp,
            })
        } else if let Some((h, _)) = handles.iter().find(|(_, hp)| hp.distance(p) <= HANDLE) {
            let o = ed.object(ed.selection[0]).cloned();
            o.map(|o| Gesture::Resize {
                id: o.id,
                handle: *h,
                start: sp,
                rect: o.rect,
                kind: o.kind,
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
                if !*moved && (dx.abs() + dy.abs()) * view.scale < 2.0 {
                    // Ignore jitter before the drag really starts.
                } else {
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
                let painter2 = painter.clone();
                if let Some(o) = ed.object_mut_for_preview(id) {
                    match (handle, &mut o.kind) {
                        (Handle::End(end), ObjectKind::Shape(s)) => {
                            let ObjectKind::Shape(s0) = &kind0 else { unreachable!() };
                            let (mut a, mut b) = s0.endpoints(&rect0);
                            let target = if end { &mut b } else { &mut a };
                            *target = (target.0 + d.0, target.1 + d.1);
                            if mods.shift {
                                // Snap to horizontal / vertical / 45°.
                                let other = if end { a } else { b };
                                let mut t = if end { b } else { a };
                                let (vx, vy) = (t.0 - other.0, t.1 - other.1);
                                let ang = vy.atan2(vx);
                                let q = (ang / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
                                let len = (vx * vx + vy * vy).sqrt();
                                t = (other.0 + len * q.cos(), other.1 + len * q.sin());
                                if end { b = t } else { a = t }
                            }
                            o.rect = s.set_endpoints(a, b);
                        }
                        (Handle::Box(hx, hy), _) => {
                            let aspect = kind0.keeps_aspect() ^ mods.shift;
                            let (r, k) = resize_box(rect0, &kind0, (hx, hy), d, aspect);
                            o.kind = k;
                            o.rect = refit_text(&painter2, &o.kind, r);
                        }
                        _ => {}
                    }
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
                create_object(
                    ed,
                    &painter,
                    tool,
                    start,
                    current,
                    (current.0 - start.0).abs().max((current.1 - start.1).abs()) * view.scale > 4.0,
                );
            }
            _ => {}
        }
    }

    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let sp = view.to_slide(p);
            if editing.is_some() {
                finish_text_edit(ed, st);
            }
            if ed.tool != Tool::Select {
                create_object(ed, &painter, ed.tool, sp, sp, false);
            } else {
                match hit_test(ed, sp, tol) {
                    Some(id) if mods.shift => {
                        if let Some(i) = ed.selection.iter().position(|x| *x == id) {
                            ed.selection.remove(i);
                        } else {
                            ed.selection.push(id);
                        }
                    }
                    Some(id) if ed.selection == [id] && editing.is_none() && is_text(ed, id) => {
                        begin_text_edit(ed, st, id, Caret::At(p));
                    }
                    Some(id) => ed.selection = vec![id],
                    None => ed.selection.clear(),
                }
                ed.history.seal();
            }
        }
    }

    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
        && let Some(id) = hit_test(ed, view.to_slide(p), tol)
    {
        ed.selection = vec![id];
        match ed.object(id).map(|o| &o.kind) {
            Some(ObjectKind::Text(_)) => begin_text_edit(ed, st, id, Caret::At(p)),
            Some(ObjectKind::Equation(_)) => ed.focus_source = true,
            _ => {}
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
    if editing.is_none() {
        for (_, hp) in handles_for(ed, &view) {
            let r = egui::Rect::from_center_size(hp, Vec2::splat(HANDLE));
            ov.rect_filled(r, 1.5, Color32::WHITE);
            ov.rect_stroke(r, 1.5, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        }
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
        _ => {}
    }

    // ---- in-place text editing ----------------------------------------------
    if let Some(id) = ed.editing_text {
        inline_text_editor(ui, ed, st, &view, id);
    } else {
        st.edit_for = None;
    }
    request
}

fn create_object(ed: &mut Editor, p: &egui::Painter, tool: Tool, start: (f32, f32), end: (f32, f32), dragged: bool) {
    let Some(mut kind) = new_object_kind(tool) else {
        return;
    };
    let rect = match (&mut kind, dragged) {
        (ObjectKind::Shape(s), true) if s.kind.is_linear() => s.set_endpoints(start, end),
        (ObjectKind::Shape(s), false) if s.kind.is_linear() => s.set_endpoints(start, (start.0 + 80.0, start.1)),
        (ObjectKind::Shape(_), true) => Rect::from_points(start, end),
        (ObjectKind::Shape(Shape { kind: k, .. }), false) => {
            let (w, h) = if *k == crate::document::ShapeKind::Ellipse {
                (60.0, 60.0)
            } else {
                (90.0, 55.0)
            };
            Rect::new(start.0 - w * 0.5, start.1 - h * 0.5, w, h)
        }
        (ObjectKind::Text(t), d) => {
            let w = if d { (end.0 - start.0).abs().max(20.0) } else { 220.0 };
            Rect::new(
                start.0.min(end.0),
                start.1.min(end.1),
                w,
                t.font_size * typeset::LINE_SPREAD,
            )
        }
        (ObjectKind::Equation(e), _) => Rect::new(start.0, start.1, e.font_size * 3.0, e.font_size * 1.2),
        _ => return,
    };
    let rect = refit_text(p, &kind, rect);
    let is_text = matches!(kind, ObjectKind::Text(_));
    let is_eq = matches!(kind, ObjectKind::Equation(_));
    let id = ed.insert_object(kind, rect);
    ed.set_tool(Tool::Select);
    if is_text {
        ed.editing_text = Some(id);
    } else if is_eq {
        ed.focus_source = true;
    }
}

fn is_text(ed: &Editor, id: ObjectId) -> bool {
    matches!(ed.object(id).map(|o| &o.kind), Some(ObjectKind::Text(_)))
}

fn finish_text_edit(ed: &mut Editor, st: &mut CanvasState) {
    if let Some(id) = ed.editing_text.take() {
        ed.history.seal();
        // Remove boxes left empty (like Keynote/PowerPoint).
        if let Some(ObjectKind::Text(t)) = ed.object(id).map(|o| &o.kind)
            && t.text.trim().is_empty()
        {
            let keep = std::mem::take(&mut ed.selection);
            ed.selection = vec![id];
            ed.delete_selection();
            ed.selection = keep.into_iter().filter(|x| *x != id).collect();
        }
    }
    st.edit_for = None;
}

fn inline_text_editor(ui: &mut Ui, ed: &mut Editor, st: &mut CanvasState, view: &View, id: ObjectId) {
    let Some(obj) = ed.object(id).cloned() else {
        ed.editing_text = None;
        return;
    };
    let ObjectKind::Text(t) = &obj.kind else {
        ed.editing_text = None;
        return;
    };
    let te_id = egui::Id::unique(("inline-text", id.0));
    let first_frame = st.edit_for != Some(id);
    if first_frame {
        st.edit_for = Some(id);
        st.edit_age = 0;
        st.edit_buffer = t.text.clone();
        // Start with the caret at the end (a stale state from an earlier
        // edit of this box must not decide where typing goes).
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), te_id).unwrap_or_default();
        let end = egui::text::CCursor::new(st.edit_buffer.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ui.ctx(), te_id);
    } else {
        st.edit_age = st.edit_age.saturating_add(1);
        if st.edit_buffer != t.text {
            // Undo/redo changed the text while editing.
            st.edit_buffer = t.text.clone();
        }
    }
    // Editing usually starts on the frame of a mouse release, which the new
    // TextEdit would see as a click elsewhere and drop focus. Keep claiming
    // focus for the first few frames.
    const SETTLE_FRAMES: u32 = 3;
    if st.edit_age < SETTLE_FRAMES {
        ui.memory_mut(|m| m.request_focus(te_id));
    }

    let scale = view.scale;
    let size = t.font_size * scale;
    let indent = if t.bullets { size * typeset::BULLET_INDENT } else { 0.0 };
    let width = (obj.rect.w * scale - indent).max(4.0);
    // Offset so the first baseline matches the rendered text exactly.
    let probe = ui.painter().layout_job(text_job("Hg", t, scale, f32::INFINITY));
    let b0 = probe
        .rows
        .first()
        .and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y))
        .unwrap_or(size);
    let top = view.pt(obj.rect.x, obj.rect.y) + vec2(indent, size * typeset::FIRST_BASELINE - b0);
    let height = (obj.rect.h * scale).max(size * typeset::LINE_SPREAD) + size;
    let rect = egui::Rect::from_min_size(top, vec2(width, height));

    let tb = t.clone();
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        ui.painter().layout_job(text_job(buf.as_str(), &tb, scale, wrap))
    };
    // Make the editing state obvious on the slide: an accent outline around
    // the box and a caret that contrasts with the slide background (egui's
    // default caret is light in dark mode and invisible on a white slide).
    let bg = ed.slide().background;
    let light_bg = 0.299 * f32::from(bg.r) + 0.587 * f32::from(bg.g) + 0.114 * f32::from(bg.b) > 140.0;
    let caret_color = if light_bg {
        Color32::from_gray(15)
    } else {
        Color32::WHITE
    };
    let mut outline = view.rect(&obj.rect);
    outline.max.y = outline.max.y.max(outline.min.y + size * typeset::LINE_SPREAD);
    ui.painter()
        .rect_stroke(outline.expand(3.0), 2.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);

    let mut buf = std::mem::take(&mut st.edit_buffer);
    let out = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            let v = ui.visuals_mut();
            v.text_cursor.stroke = Stroke::new(2.0, caret_color);
            v.selection.bg_fill = Color32::from_rgba_unmultiplied(10, 132, 255, 80);
            egui::TextEdit::multiline(&mut buf)
                .id(te_id)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .desired_width(width)
                .desired_rows(1)
                .hint_text("Type here")
                .layouter(&mut layouter)
                .show(ui)
        })
        .inner;
    if first_frame && let Caret::At(p) = st.caret {
        // Put the caret where the user clicked.
        let c = out.galley.cursor_from_pos(p - out.galley_pos);
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), te_id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
        state.store(ui.ctx(), te_id);
    }
    // Bullets beside each paragraph's first row.
    if t.bullets {
        let mut para_start = true;
        for row in &out.galley.rows {
            if para_start && !row.row.glyphs.is_empty() {
                let y = out.galley_pos.y + row.pos.y + row.row.glyphs[0].pos.y;
                ui.painter().text(
                    pos2(top.x - indent, y),
                    egui::Align2::LEFT_BOTTOM,
                    "•",
                    FontId::new(size, text_family(t.bold, t.italic)),
                    c32(t.color),
                );
            }
            para_start = row.ends_with_newline;
        }
    }
    if buf != t.text {
        let mut kind = obj.kind.clone();
        if let ObjectKind::Text(tt) = &mut kind {
            tt.text = buf.clone();
        }
        let rect = refit_text(ui.painter(), &kind, obj.rect);
        ed.set_object(id, kind, rect, Some(MergeKey::Object(id, 0)));
    }
    st.edit_buffer = buf;
    let esc = ui.input(|i| i.key_pressed(Key::Escape));
    let lost = st.edit_age >= SETTLE_FRAMES && !ui.memory(|m| m.has_focus(te_id));
    if esc || lost {
        finish_text_edit(ed, st);
        if esc {
            ed.selection = vec![id];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Equation;

    #[test]
    fn resize_keeps_aspect_and_scales_equation_font() {
        let k = ObjectKind::Equation(Equation {
            font_size: 20.0,
            ..Default::default()
        });
        let (r, k2) = resize_box(Rect::new(0.0, 0.0, 100.0, 20.0), &k, (1, 1), (100.0, 0.0), true);
        assert!((r.w - 200.0).abs() < 0.5 && (r.h - 40.0).abs() < 0.5, "{r:?}");
        match k2 {
            ObjectKind::Equation(e) => assert_eq!(e.font_size, 40.0),
            _ => unreachable!(),
        }
        // Dragging the top-left corner anchors the bottom-right corner.
        let (r, _) = resize_box(
            Rect::new(10.0, 10.0, 100.0, 50.0),
            &ObjectKind::Image(crate::document::ImageRef {
                asset: crate::document::AssetId("a".repeat(64)),
            }),
            (-1, -1),
            (-50.0, -5.0),
            true,
        );
        assert!((r.right() - 110.0).abs() < 1e-3 && (r.bottom() - 60.0).abs() < 1e-3);
        assert!((r.w / r.h - 2.0).abs() < 1e-3);
    }

    #[test]
    fn resize_never_inverts() {
        let k = ObjectKind::Shape(Shape::new(crate::document::ShapeKind::Rectangle));
        let (r, _) = resize_box(Rect::new(0.0, 0.0, 10.0, 10.0), &k, (1, 0), (-50.0, 0.0), false);
        assert_eq!(r.w, MIN_EXTENT);
    }

    #[test]
    fn segment_distance() {
        assert_eq!(dist_to_segment((5.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 3.0);
        assert_eq!(dist_to_segment((-4.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 5.0);
    }
}
