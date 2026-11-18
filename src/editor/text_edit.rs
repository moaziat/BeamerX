//! In-place text editing on the slide (PowerPoint-style).
//!
//! The text box is edited where it sits, in its real fonts and sizes, with
//! LaTeX math rendered live: a `$…$` pair renders as soon as it is closed,
//! and the formula under the caret shows its source so it can be edited.
//! Character formats (bold, italic, colour) apply to the selection, the word
//! at the caret, or what is typed next; paragraph formats (style, list,
//! outline level) apply to the lines touched by the selection. Every change
//! goes through the undoable command history (typing merges into one step).

use super::Editor;
use crate::commands::MergeKey;
use crate::document::{CharFmt, ListKind, MAX_LEVEL, ObjectId, ObjectKind, ParaFmt, Rect, Resolved, TextBox, typeset};
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
    /// Format for the next typed chars (set by Bold etc. with no selection).
    pub typing: Option<CharFmt>,
    start: Option<Caret>,
    preferred_x: Option<f32>,
    /// Time and position of the last press (multi-click detection).
    last_press: Option<(f64, Pos2)>,
    settle: u32,
    blink: Instant,
}

impl RichEdit {
    pub fn new(id: ObjectId, start: Caret) -> Self {
        Self {
            id,
            caret: 0,
            anchor: 0,
            typing: None,
            start: Some(start),
            preferred_x: None,
            last_press: None,
            settle: 0,
            blink: Instant::now(),
        }
    }

    pub fn selection(&self) -> (usize, usize) {
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

/// Word strictly around the caret (caret inside, not at its edges).
fn word_inside(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let n = chars.len();
    (i > 0 && i < n && is_word(chars[i - 1]) && is_word(chars[i])).then(|| word_at(chars, i))
}

/// Insert `ins` at the caret, replacing the selection.
pub fn insert(tb: &mut TextBox, st: &mut RichEdit, ins: &str) {
    let (a, b) = st.selection();
    tb.replace(a, b, ins, st.typing);
    st.caret = a + ins.chars().count();
    st.anchor = st.caret;
}

/// Lines touched by the selection.
fn selected_lines(tb: &TextBox, st: &RichEdit) -> (usize, usize) {
    let (a, b) = st.selection();
    (tb.line_of(a), tb.line_of(b))
}

/// Change the outline level of the selected lines by `delta`.
fn change_level(tb: &mut TextBox, st: &RichEdit, delta: i8) {
    let (l0, l1) = selected_lines(tb, st);
    tb.format_lines(l0, l1, |p| {
        p.level = (i16::from(p.level) + i16::from(delta)).clamp(0, i16::from(MAX_LEVEL)) as u8;
    });
}

/// Toggle a character attribute: on the selection, else the word at the
/// caret, else for the next typed chars. `get`/`set` select the attribute.
fn toggle_char(tb: &mut TextBox, st: &mut RichEdit, get: fn(&Resolved) -> bool, set: fn(&mut CharFmt, bool)) {
    let chars: Vec<char> = tb.text.chars().collect();
    let (a, b) = if st.has_selection() {
        st.selection()
    } else {
        word_inside(&chars, st.caret).unwrap_or((st.caret, st.caret))
    };
    if a < b {
        let on = !get(&tb.resolved_at(a));
        tb.format_range(a, b, |f| set(f, on));
    } else {
        let cur = typing_resolved(tb, st);
        let mut f = st.typing.unwrap_or_else(|| char_fmt_before(tb, st.caret));
        set(&mut f, !get(&cur));
        st.typing = Some(f);
    }
}

fn char_fmt_before(tb: &TextBox, i: usize) -> CharFmt {
    let fm = tb.char_formats();
    if i > 0 {
        fm.get(i - 1).copied().unwrap_or_default()
    } else {
        fm.first().copied().unwrap_or_default()
    }
}

/// Appearance the next typed char would have.
fn typing_resolved(tb: &TextBox, st: &RichEdit) -> Resolved {
    let f = st.typing.unwrap_or_else(|| char_fmt_before(tb, st.caret));
    tb.resolve(f, tb.para(tb.line_of(st.caret)))
}

/// Apply editing events to `tb`. Returns (changed, finished).
fn apply_events(
    tb: &mut TextBox,
    st: &mut RichEdit,
    events: &[Event],
    lay: &text_layout::TextLayout,
    ctx: &egui::Context,
) -> (bool, bool) {
    let mut changed = false;
    let mut finished = false;
    for ev in events {
        let chars: Vec<char> = tb.text.chars().collect();
        let n = chars.len();
        st.caret = st.caret.min(n);
        st.anchor = st.anchor.min(n);
        match ev {
            Event::Text(t) | Event::Ime(egui::ImeEvent::Commit(t)) => {
                let t: String = t.chars().filter(|c| !c.is_control() || *c == '\n').collect();
                if !t.is_empty() {
                    insert(tb, st, &t);
                    changed = true;
                }
            }
            Event::Paste(t) => {
                let t = t.replace("\r\n", "\n").replace('\r', "\n");
                insert(tb, st, &t);
                changed = true;
            }
            Event::Copy | Event::Cut => {
                let (a, b) = st.selection();
                if a < b {
                    ctx.copy_text(chars[a..b].iter().collect());
                    if matches!(ev, Event::Cut) {
                        insert(tb, st, "");
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
                let moving = matches!(
                    key,
                    Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End
                );
                if moving {
                    st.typing = None;
                }
                let mv = |st: &mut RichEdit, to: usize| {
                    st.caret = to.min(n);
                    if !m.shift {
                        st.anchor = st.caret;
                    }
                };
                match key {
                    Key::Escape => finished = true,
                    Key::Enter => {
                        insert(tb, st, "\n");
                        changed = true;
                    }
                    Key::Tab => {
                        // Outline hierarchy: Tab demotes, Shift-Tab promotes.
                        change_level(tb, st, if m.shift { -1 } else { 1 });
                        changed = true;
                    }
                    Key::B if m.command => {
                        toggle_char(tb, st, |r| r.bold, |f, v| f.bold = Some(v));
                        changed = true;
                    }
                    Key::I if m.command => {
                        toggle_char(tb, st, |r| r.italic, |f, v| f.italic = Some(v));
                        changed = true;
                    }
                    Key::Backspace => {
                        let line = tb.line_of(st.caret);
                        let line_start = st.caret == 0 || chars.get(st.caret - 1) == Some(&'\n');
                        let pf = tb.para(line);
                        if !st.has_selection() && line_start && (pf.level > 0 || pf.list != ListKind::None) {
                            // At the start of a list item: promote, then end the list.
                            tb.format_lines(line, line, |p| {
                                if p.level > 0 {
                                    p.level -= 1;
                                } else {
                                    p.list = ListKind::None;
                                }
                            });
                        } else if st.has_selection() {
                            insert(tb, st, "");
                        } else if st.caret > 0 {
                            let from = if m.alt {
                                word_left(&chars, st.caret)
                            } else if m.command {
                                lay.line_bounds(st.caret).0.min(st.caret - 1)
                            } else {
                                st.caret - 1
                            };
                            st.anchor = from;
                            insert(tb, st, "");
                        }
                        changed = true;
                    }
                    Key::Delete => {
                        if st.has_selection() {
                            insert(tb, st, "");
                        } else if st.caret < n {
                            let to = if m.alt {
                                word_right(&chars, st.caret)
                            } else {
                                st.caret + 1
                            };
                            st.anchor = to;
                            insert(tb, st, "");
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
    let n = t.char_len();
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
        let (pressed, now) = ui.input(|i| (i.pointer.primary_pressed(), i.time));
        // The 2nd/3rd press of a double/triple click must not collapse the
        // selection that the click's release will make.
        let multi = pressed && st.last_press.is_some_and(|(t, q)| now - t < 0.3 && q.distance(p) < 6.0);
        if pressed {
            st.last_press = Some((now, p));
        }
        if pressed && resp.hovered() && !multi {
            ui.memory_mut(|m| m.request_focus(eid));
            st.caret = lay.hit(p - origin);
            if !shift {
                st.anchor = st.caret;
            }
            st.preferred_x = None;
            st.typing = None;
            st.blink = Instant::now();
        } else if resp.dragged() {
            st.caret = lay.hit(p - origin);
        }
        let chars: Vec<char> = t.text.chars().collect();
        if resp.triple_clicked() {
            // Triple-click selects the paragraph (line).
            let i = lay.hit(p - origin);
            let start = chars[..i.min(chars.len())]
                .iter()
                .rposition(|c| *c == '\n')
                .map_or(0, |k| k + 1);
            let end = chars[i.min(chars.len())..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |k| i + k);
            st.anchor = start;
            st.caret = end;
        } else if resp.double_clicked() {
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
        let mut tb = t.clone();
        let (changed, finished) = apply_events(&mut tb, st, &events, &lay, ui.ctx());
        if changed || !events.is_empty() {
            st.blink = Instant::now();
        }
        if changed && &tb != t {
            ed.set_object(id, ObjectKind::Text(tb), obj.rect, Some(MergeKey::Object(id, 0)));
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
    lay = renderer::layout_text(&painter, sv, t, obj.rect.w, scale, Some(st.caret));
    let h_bp = (lay.height / scale).max(t.max_size() * typeset::LINE_SPREAD);
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
        let hint = t.placeholder.as_deref().unwrap_or("Type text — use $…$ for math");
        let hs = t.font_size * t.para(0).style.size_factor() * scale * 0.9;
        painter.text(
            origin,
            egui::Align2::LEFT_TOP,
            hint,
            egui::FontId::new(hs, renderer::text_family(false, true)),
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

// ---- formatting API used by the ribbon ---------------------------------------

/// The text box being edited and its caret state, if any.
fn editing<'a>(ed: &Editor, rich: &'a mut Option<RichEdit>) -> Option<(ObjectId, &'a mut RichEdit)> {
    let id = ed.editing_text?;
    let st = rich.as_mut().filter(|st| st.id == id)?;
    Some((id, st))
}

fn commit(ed: &mut Editor, id: ObjectId, tb: TextBox, field: u8) {
    let Some(obj) = ed.object(id).cloned() else { return };
    if obj.kind != ObjectKind::Text(tb.clone()) {
        ed.set_object(
            id,
            ObjectKind::Text(tb),
            obj.rect,
            Some(MergeKey::Object(id, 50 + field)),
        );
    }
}

/// Character appearance shown in the ribbon: at the caret while editing,
/// else the first selected text box's start.
pub fn current_char(ed: &Editor, rich: &Option<RichEdit>) -> Option<Resolved> {
    if let (Some(id), Some(st)) = (ed.editing_text, rich.as_ref())
        && st.id == id
        && let Some(ObjectKind::Text(t)) = ed.object(id).map(|o| &o.kind)
    {
        let (a, _) = st.selection();
        return Some(if st.has_selection() {
            t.resolved_at(a)
        } else {
            typing_resolved(t, st)
        });
    }
    ed.selected_text_style().map(|t| t.resolved_at(0))
}

/// Paragraph format shown in the ribbon.
pub fn current_para(ed: &Editor, rich: &Option<RichEdit>) -> Option<ParaFmt> {
    if let (Some(id), Some(st)) = (ed.editing_text, rich.as_ref())
        && st.id == id
        && let Some(ObjectKind::Text(t)) = ed.object(id).map(|o| &o.kind)
    {
        return Some(t.para(t.line_of(st.caret)));
    }
    ed.selected_text_style().map(|t| t.para(0))
}

/// Toggle bold or italic (`which`: 0 = bold, 1 = italic) for the current
/// text target. While editing: selection / word / typing format. Otherwise:
/// all text of the selected boxes.
pub fn toggle_style(ed: &mut Editor, rich: &mut Option<RichEdit>, which: u8) {
    let get: fn(&Resolved) -> bool = if which == 0 { |r| r.bold } else { |r| r.italic };
    let set: fn(&mut CharFmt, bool) = if which == 0 {
        |f, v| f.bold = Some(v)
    } else {
        |f, v| f.italic = Some(v)
    };
    if let Some((id, st)) = editing(ed, rich) {
        let Some(ObjectKind::Text(t)) = ed.object(id).map(|o| o.kind.clone()) else {
            return;
        };
        let mut tb = t;
        toggle_char(&mut tb, st, get, set);
        commit(ed, id, tb, which);
        return;
    }
    let on = !ed
        .selected_text_style()
        .map(|t| get(&t.resolved_at(0)))
        .unwrap_or(false);
    ed.update_selected_text(which, |t| {
        if t.text.is_empty() {
            if which == 0 { t.bold = on } else { t.italic = on }
        } else {
            let n = t.char_len();
            t.format_range(0, n, |f| set(f, on));
        }
    });
}

/// Set the text colour of the current target (see [`toggle_style`]).
pub fn set_color(ed: &mut Editor, rich: &mut Option<RichEdit>, c: crate::document::Color) {
    if let Some((id, st)) = editing(ed, rich) {
        let Some(ObjectKind::Text(mut tb)) = ed.object(id).map(|o| o.kind.clone()) else {
            return;
        };
        let chars: Vec<char> = tb.text.chars().collect();
        let (a, b) = if st.has_selection() {
            st.selection()
        } else {
            word_inside(&chars, st.caret).unwrap_or((st.caret, st.caret))
        };
        if a < b {
            tb.format_range(a, b, |f| f.color = Some(c));
            commit(ed, id, tb, 2);
        } else {
            let mut f = st.typing.unwrap_or_else(|| char_fmt_before(&tb, st.caret));
            f.color = Some(c);
            st.typing = Some(f);
        }
        return;
    }
    ed.update_selected_text(3, |t| {
        t.color = c;
        let n = t.char_len();
        t.format_range(0, n, |f| f.color = None);
    });
}

/// Change the paragraph format of the current target: lines touched by the
/// selection while editing, else every line of the selected boxes.
pub fn update_paras(ed: &mut Editor, rich: &mut Option<RichEdit>, f: impl Fn(&mut ParaFmt)) {
    if let Some((id, st)) = editing(ed, rich) {
        let Some(ObjectKind::Text(mut tb)) = ed.object(id).map(|o| o.kind.clone()) else {
            return;
        };
        let (l0, l1) = selected_lines(&tb, st);
        tb.format_lines(l0, l1, &f);
        commit(ed, id, tb, 3);
        return;
    }
    ed.update_selected_text(6, |t| {
        let n = t.line_count();
        t.format_lines(0, n - 1, &f);
    });
}

/// Toggle a list kind on the current paragraphs.
pub fn toggle_list(ed: &mut Editor, rich: &mut Option<RichEdit>, kind: ListKind) {
    let on = current_para(ed, rich).is_none_or(|p| p.list != kind);
    update_paras(ed, rich, |p| p.list = if on { kind } else { ListKind::None });
}

/// Increase (+1) or decrease (−1) the outline level of the current paragraphs.
pub fn shift_level(ed: &mut Editor, rich: &mut Option<RichEdit>, delta: i8) {
    update_paras(ed, rich, |p| {
        p.level = (i16::from(p.level) + i16::from(delta)).clamp(0, i16::from(MAX_LEVEL)) as u8;
    });
}

/// Insert text at the caret of the box being edited (e.g. ribbon "Math").
pub fn insert_at_caret(ed: &mut Editor, rich: &mut Option<RichEdit>, s: &str, caret_back: usize) -> bool {
    let Some((id, st)) = editing(ed, rich) else {
        return false;
    };
    let Some(ObjectKind::Text(mut tb)) = ed.object(id).map(|o| o.kind.clone()) else {
        return false;
    };
    insert(&mut tb, st, s);
    st.caret -= caret_back.min(st.caret);
    st.anchor = st.caret;
    commit(ed, id, tb, 0);
    true
}

/// Give keyboard focus back to the box being edited (after a ribbon click).
pub fn refocus(ctx: &egui::Context, ed: &Editor) {
    if let Some(id) = ed.editing_text {
        ctx.memory_mut(|m| m.request_focus(widget_id(id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::ParaStyle;

    fn st_at(caret: usize, anchor: usize) -> RichEdit {
        let mut st = RichEdit::new(ObjectId(1), Caret::End);
        st.caret = caret;
        st.anchor = anchor;
        st
    }

    #[test]
    fn insert_replaces_selection_and_moves_caret() {
        let mut t = TextBox {
            text: "héllo world".into(),
            ..Default::default()
        };
        let mut st = st_at(5, 1);
        insert(&mut t, &mut st, "ey");
        assert_eq!(t.text, "hey world");
        assert_eq!((st.caret, st.anchor), (3, 3));
    }

    #[test]
    fn bold_applies_to_selection_word_or_typing() {
        let mut t = TextBox {
            text: "one two three".into(),
            ..Default::default()
        };
        // Selection.
        let mut st = st_at(7, 4);
        toggle_char(&mut t, &mut st, |r| r.bold, |f, v| f.bold = Some(v));
        assert!(t.resolved_at(4).bold && !t.resolved_at(3).bold && !t.resolved_at(8).bold);
        // Caret inside a word: the whole word.
        let mut st = st_at(10, 10);
        toggle_char(&mut t, &mut st, |r| r.italic, |f, v| f.italic = Some(v));
        assert!(t.resolved_at(8).italic && t.resolved_at(12).italic && !t.resolved_at(7).italic);
        // Caret at the end: applies to what is typed next.
        let n = t.char_len();
        let mut st = st_at(n, n);
        toggle_char(&mut t, &mut st, |r| r.bold, |f, v| f.bold = Some(v));
        insert(&mut t, &mut st, " four");
        assert!(t.resolved_at(n + 2).bold);
        assert!(!t.resolved_at(0).bold);
    }

    #[test]
    fn tab_and_backspace_manage_outline_levels() {
        let ctx = egui::Context::default();
        let lay = text_layout::TextLayout {
            lines: vec![],
            height: 0.0,
            len: 0,
            pending: false,
            errors: vec![],
        };
        let mut t = TextBox {
            text: "a\nb".into(),
            ..Default::default()
        };
        t.format_lines(0, 1, |p| p.list = ListKind::Bullet);
        let key = |k: Key, shift: bool| Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
        };
        let mut st = st_at(2, 2);
        apply_events(
            &mut t,
            &mut st,
            &[key(Key::Tab, false), key(Key::Tab, false)],
            &lay,
            &ctx,
        );
        assert_eq!(t.para(1).level, 2);
        apply_events(&mut t, &mut st, &[key(Key::Tab, true)], &lay, &ctx);
        assert_eq!(t.para(1).level, 1);
        // Backspace at the start of the item promotes, then removes the bullet,
        // and only then joins the lines.
        apply_events(&mut t, &mut st, &[key(Key::Backspace, false)], &lay, &ctx);
        assert_eq!((t.para(1).level, t.para(1).list), (0, ListKind::Bullet));
        apply_events(&mut t, &mut st, &[key(Key::Backspace, false)], &lay, &ctx);
        assert_eq!(t.para(1).list, ListKind::None);
        assert_eq!(t.text, "a\nb");
        apply_events(&mut t, &mut st, &[key(Key::Backspace, false)], &lay, &ctx);
        assert_eq!(t.text, "ab");
        assert_eq!(t.para(0).list, ListKind::Bullet);
        let _ = ParaStyle::Title;
    }

    #[test]
    fn word_navigation() {
        let c: Vec<char> = "one two, three".chars().collect();
        assert_eq!(word_right(&c, 0), 3);
        assert_eq!(word_right(&c, 3), 7);
        assert_eq!(word_left(&c, 14), 9);
        assert_eq!(word_left(&c, 9), 4);
        assert_eq!(word_at(&c, 5), (4, 7));
        assert_eq!(word_inside(&c, 5), Some((4, 7)));
        assert_eq!(word_inside(&c, 4), None);
    }
}
