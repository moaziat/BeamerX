//! In-place text editing on the slide (PowerPoint-style).
//!
//! The text box is edited where it sits, in its real font and size, with
//! LaTeX math rendered live: a `$…$` pair renders as soon as it is closed,
//! and the formula under the caret shows its source so it can be edited.
//! Text changes go through the undoable command history (consecutive typing
//! merges into one undo step).

use super::Editor;
use crate::commands::MergeKey;
use crate::document::{ObjectId, ObjectKind, Rect, typeset};
use crate::renderer::{self, Services, View, text_layout};
use egui::{Color32, Event, EventFilter, Key, Modifiers, Pos2, Sense, Stroke, StrokeKind, Ui, vec2};
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(10, 132, 255);
const BLINK: Duration = Duration::from_millis(530);
/// Frames during which focus is (re)claimed after editing starts: editing
/// usually begins on a mouse release that would otherwise clear focus.
const SETTLE_FRAMES: u32 = 3;

/// Where to put the caret when editing starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Caret {
    End,
    /// At this caret index.
    Index(usize),
    /// At the char under this screen position.
    At(Pos2),
}

pub struct RichEdit {
    pub id: ObjectId,
    pub caret: usize,
    pub anchor: usize,
    start: Option<Caret>,
    preferred_x: Option<f32>,
    settle: u32,
    blink: Instant,
}

impl RichEdit {
    pub fn new(id: ObjectId, start: Caret) -> Self {
        Self {
            id,
            caret: 0,
            anchor: 0,
            start: Some(start),
            preferred_x: None,
            settle: 0,
            blink: Instant::now(),
        }
    }

    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }
}

pub fn widget_id(id: ObjectId) -> egui::Id {
    egui::Id::unique(("rich-edit", id.0))
}

/// Result of one frame of editing.
pub enum Outcome {
    Continue,
    /// Editing finished (Escape); the box stays selected.
    Finished,
}

// ---- text operations on char indices -------------------------------------

fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

fn replace_range(s: &mut String, a: usize, b: usize, with: &str) {
    let (ba, bb) = (char_to_byte(s, a), char_to_byte(s, b));
    s.replace_range(ba..bb, with);
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_left(chars: &[char], mut i: usize) -> usize {
    while i > 0 && !is_word(chars[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word(chars[i - 1]) {
        i -= 1;
    }
    i
}

fn word_right(chars: &[char], mut i: usize) -> usize {
    let n = chars.len();
    while i < n && !is_word(chars[i]) {
        i += 1;
    }
    while i < n && is_word(chars[i]) {
        i += 1;
    }
    i
}

fn word_at(chars: &[char], i: usize) -> (usize, usize) {
    let n = chars.len();
    let i = i.min(n);
    if i < n && is_word(chars[i]) || i > 0 && is_word(chars[i - 1]) {
        let mut a = i;
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        let mut b = i;
        while b < n && is_word(chars[b]) {
            b += 1;
        }
        (a, b)
    } else {
        (i, (i + 1).min(n))
    }
}

/// Insert `text` at the caret, replacing the selection.
pub fn insert(text: &mut String, st: &mut RichEdit, ins: &str) {
    let (a, b) = st.selection();
    replace_range(text, a, b, ins);
    st.caret = a + ins.chars().count();
    st.anchor = st.caret;
}

/// Apply editing events to `text`. Returns (changed, finished).
fn apply_events(
    text: &mut String,
    st: &mut RichEdit,
    events: &[Event],
    lay: &text_layout::TextLayout,
    ctx: &egui::Context,
) -> (bool, bool) {
    let mut changed = false;
    let mut finished = false;
    for ev in events {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        st.caret = st.caret.min(n);
        st.anchor = st.anchor.min(n);
        match ev {
            Event::Text(t) | Event::Ime(egui::ImeEvent::Commit(t)) => {
                let t: String = t.chars().filter(|c| !c.is_control() || *c == '\n').collect();
                if !t.is_empty() {
                    insert(text, st, &t);
                    changed = true;
                }
            }
            Event::Paste(t) => {
                let t = t.replace("\r\n", "\n").replace('\r', "\n");
                insert(text, st, &t);
                changed = true;
            }
            Event::Copy | Event::Cut => {
                let (a, b) = st.selection();
                if a < b {
                    ctx.copy_text(chars[a..b].iter().collect());
                    if matches!(ev, Event::Cut) {
                        insert(text, st, "");
                        changed = true;
                    }
                }
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let m: Modifiers = *modifiers;
                let mv = |st: &mut RichEdit, to: usize| {
                    st.caret = to.min(n);
                    if !m.shift {
                        st.anchor = st.caret;
                    }
                };
                match key {
                    Key::Escape => finished = true,
                    Key::Enter => {
                        insert(text, st, "\n");
                        changed = true;
                    }
                    Key::Backspace => {
                        if st.has_selection() {
                            insert(text, st, "");
                        } else if st.caret > 0 {
                            let from = if m.alt {
                                word_left(&chars, st.caret)
                            } else if m.command {
                                lay.line_bounds(st.caret).0.min(st.caret - 1)
                            } else {
                                st.caret - 1
                            };
                            st.anchor = from;
                            insert(text, st, "");
                        }
                        changed = true;
                    }
                    Key::Delete => {
                        if st.has_selection() {
                            insert(text, st, "");
                        } else if st.caret < n {
                            let to = if m.alt {
                                word_right(&chars, st.caret)
                            } else {
                                st.caret + 1
                            };
                            st.anchor = to;
                            insert(text, st, "");
                        }
                        changed = true;
                    }
                    Key::ArrowLeft => {
                        st.preferred_x = None;
                        if st.has_selection() && !m.shift {
                            let (a, _) = st.selection();
                            mv(st, a);
                        } else if m.command {
                            mv(st, lay.line_bounds(st.caret).0);
                        } else if m.alt {
                            mv(st, word_left(&chars, st.caret));
                        } else {
                            mv(st, st.caret.saturating_sub(1));
                        }
                    }
                    Key::ArrowRight => {
                        st.preferred_x = None;
                        if st.has_selection() && !m.shift {
                            let (_, b) = st.selection();
                            mv(st, b);
                        } else if m.command {
                            mv(st, lay.line_bounds(st.caret).1);
                        } else if m.alt {
                            mv(st, word_right(&chars, st.caret));
                        } else {
                            mv(st, st.caret + 1);
                        }
                    }
                    Key::ArrowUp | Key::ArrowDown => {
                        let (li, x) = lay.stop_of(st.caret);
                        let x = *st.preferred_x.get_or_insert(x);
                        let target = if *key == Key::ArrowUp {
                            if li == 0 { None } else { Some(li - 1) }
                        } else if li + 1 < lay.lines.len() {
                            Some(li + 1)
                        } else {
                            None
                        };
                        let to = match target {
                            Some(t) => lay.hit_on_line(t, x),
                            None if *key == Key::ArrowUp => 0,
                            None => n,
                        };
                        let keep = st.preferred_x;
                        mv(st, to);
                        st.preferred_x = keep;
                    }
                    Key::Home => mv(st, lay.line_bounds(st.caret).0),
                    Key::End => mv(st, lay.line_bounds(st.caret).1),
                    Key::A if m.command => {
                        st.anchor = 0;
                        st.caret = n;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (changed, finished)
}

/// Run one frame of in-place editing for text object `id`.
pub fn editor_ui(
    ui: &mut Ui,
    ed: &mut Editor,
    rich: &mut Option<RichEdit>,
    view: &View,
    sv: &mut Services,
    id: ObjectId,
    start: Caret,
) -> Outcome {
    let Some(obj) = ed.object(id).cloned() else {
        return Outcome::Finished;
    };
    let ObjectKind::Text(t) = &obj.kind else {
        return Outcome::Finished;
    };
    if rich.as_ref().is_none_or(|r| r.id != id) {
        *rich = Some(RichEdit::new(id, start));
    }
    let st = rich.as_mut().expect("set above");
    let eid = widget_id(id);
    let painter = ui.painter().clone();
    let scale = view.scale;
    let size = t.font_size * scale;
    let origin = view.pt(obj.rect.x, obj.rect.y);
    let n = t.text.chars().count();
    st.caret = st.caret.min(n);
    st.anchor = st.anchor.min(n);

    // Initial caret placement (needs a layout to map positions to chars).
    if let Some(start) = st.start.take() {
        let idx = match start {
            Caret::End => n,
            Caret::Index(i) => i.min(n),
            Caret::At(p) => {
                let lay = renderer::layout_text(&painter, sv, t, obj.rect.w, scale, Some(n));
                lay.hit(p - origin)
            }
        };
        st.caret = idx;
        st.anchor = idx;
    }

    let mut lay = renderer::layout_text(&painter, sv, t, obj.rect.w, scale, Some(st.caret));
    let box_h = lay.height.max(size * typeset::LINE_SPREAD);
    let box_rect = egui::Rect::from_min_size(origin, vec2(obj.rect.w * scale, box_h));

    // Pointer: the text area (minus the side handles) belongs to the editor;
    // the outline band around it stays with the canvas, which moves the box.
    let resp = ui.interact(box_rect.shrink2(vec2(5.0, 0.0)), eid, Sense::click_and_drag());
    if st.settle < SETTLE_FRAMES {
        st.settle += 1;
        ui.memory_mut(|m| m.request_focus(eid));
    }
    let shift = ui.input(|i| i.modifiers.shift);
    if let Some(p) = resp.interact_pointer_pos() {
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        if pressed && resp.hovered() {
            ui.memory_mut(|m| m.request_focus(eid));
            st.caret = lay.hit(p - origin);
            if !shift {
                st.anchor = st.caret;
            }
            st.preferred_x = None;
            st.blink = Instant::now();
        } else if resp.dragged() {
            st.caret = lay.hit(p - origin);
        }
        if resp.double_clicked() {
            let chars: Vec<char> = t.text.chars().collect();
            let (a, b) = word_at(&chars, lay.hit(p - origin));
            st.anchor = a;
            st.caret = b;
        }
    }
    resp.clone().on_hover_cursor(egui::CursorIcon::Text);

    // Keyboard.
    let focused = ui.memory(|m| m.has_focus(eid));
    let mut outcome = Outcome::Continue;
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                eid,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let events = ui.input(|i| i.events.clone());
        let mut text = t.text.clone();
        let (changed, finished) = apply_events(&mut text, st, &events, &lay, ui.ctx());
        if changed || !events.is_empty() {
            st.blink = Instant::now();
        }
        if changed && text != t.text {
            let mut kind = obj.kind.clone();
            if let ObjectKind::Text(tt) = &mut kind {
                tt.text = text;
            }
            ed.set_object(id, kind, obj.rect, Some(MergeKey::Object(id, 0)));
        }
        if finished {
            outcome = Outcome::Finished;
        }
    }

    // Re-layout after edits and keep the box height in sync (derived data).
    let Some(obj) = ed.object(id).cloned() else {
        return Outcome::Finished;
    };
    let ObjectKind::Text(t) = &obj.kind else {
        return Outcome::Finished;
    };
    if focused {
        lay = renderer::layout_text(&painter, sv, t, obj.rect.w, scale, Some(st.caret));
    }
    let h_bp = (lay.height / scale).max(t.font_size * typeset::LINE_SPREAD);
    if (obj.rect.h - h_bp).abs() > 0.25
        && let Some(o) = ed.object_mut_for_preview(id)
    {
        o.rect = Rect { h: h_bp, ..o.rect };
    }

    // ---- paint ---------------------------------------------------------------
    let bg = ed.slide().background;
    let light_bg = 0.299 * f32::from(bg.r) + 0.587 * f32::from(bg.g) + 0.114 * f32::from(bg.b) > 140.0;
    let box_h = lay.height.max(size * typeset::LINE_SPREAD);
    let outline = egui::Rect::from_min_size(origin, vec2(obj.rect.w * scale, box_h)).expand(3.0);
    painter.rect_stroke(outline, 2.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
    let (a, b) = st.selection();
    if a < b {
        for r in lay.selection_rects(a, b) {
            painter.rect_filled(
                r.translate(origin.to_vec2()),
                0.0,
                Color32::from_rgba_unmultiplied(10, 132, 255, 70),
            );
        }
    }
    if t.text.is_empty() {
        painter.text(
            origin,
            egui::Align2::LEFT_TOP,
            "Type text — use $…$ for math",
            egui::FontId::new(size * 0.9, renderer::text_family(false, true)),
            Color32::from_gray(160),
        );
    }
    text_layout::paint(
        &painter,
        origin,
        &lay,
        t.color,
        sv.textures,
        scale * painter.pixels_per_point(),
        scale,
    );
    let caret_rect = lay.caret_rect(st.caret, size).translate(origin.to_vec2());
    if focused {
        let elapsed = st.blink.elapsed();
        let phase = elapsed.as_millis() / BLINK.as_millis();
        if phase % 2 == 0 {
            let c = if light_bg {
                Color32::from_gray(15)
            } else {
                Color32::WHITE
            };
            painter.rect_filled(
                egui::Rect::from_min_size(caret_rect.min, vec2(1.5, caret_rect.height())),
                0.0,
                c,
            );
        }
        let next = BLINK.as_millis() - elapsed.as_millis() % BLINK.as_millis();
        ui.ctx().request_repaint_after(Duration::from_millis(next as u64));
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: Default::default(),
                rect: box_rect,
                cursor_rect: caret_rect,
                should_interrupt_composition: false,
            })
        });
    }
    // Show LaTeX errors of formulas in this box right under it.
    if let Some((src, err)) = lay.errors.first() {
        let first = err.lines().next().unwrap_or("LaTeX error");
        painter.text(
            outline.left_bottom() + vec2(0.0, 4.0),
            egui::Align2::LEFT_TOP,
            format!("LaTeX error in ${src}$: {first}"),
            egui::FontId::proportional(11.0),
            Color32::from_rgb(200, 40, 40),
        );
    }
    outcome
}

/// Insert text at the caret of the box being edited (e.g. ribbon "Math").
pub fn insert_at_caret(ed: &mut Editor, rich: &mut Option<RichEdit>, s: &str, caret_back: usize) -> bool {
    let (Some(id), Some(st)) = (ed.editing_text, rich.as_mut()) else {
        return false;
    };
    if st.id != id {
        return false;
    }
    let Some(obj) = ed.object(id).cloned() else {
        return false;
    };
    let ObjectKind::Text(t) = &obj.kind else { return false };
    let mut text = t.text.clone();
    insert(&mut text, st, s);
    st.caret -= caret_back.min(st.caret);
    st.anchor = st.caret;
    let mut kind = obj.kind.clone();
    if let ObjectKind::Text(tt) = &mut kind {
        tt.text = text;
    }
    ed.set_object(id, kind, obj.rect, Some(MergeKey::Object(id, 0)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_replaces_selection_and_moves_caret() {
        let mut s = String::from("héllo world");
        let mut st = RichEdit::new(ObjectId(1), Caret::End);
        st.anchor = 1;
        st.caret = 5;
        insert(&mut s, &mut st, "ey");
        assert_eq!(s, "hey world");
        assert_eq!((st.caret, st.anchor), (3, 3));
    }

    #[test]
    fn word_navigation() {
        let c: Vec<char> = "one two, three".chars().collect();
        assert_eq!(word_right(&c, 0), 3);
        assert_eq!(word_right(&c, 3), 7);
        assert_eq!(word_left(&c, 14), 9);
        assert_eq!(word_left(&c, 9), 4);
        assert_eq!(word_at(&c, 5), (4, 7));
    }
}
