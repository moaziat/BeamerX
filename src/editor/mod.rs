//! Editor state and operations.
//!
//! Holds the single source of truth for the open document, its history,
//! the current slide and the selection. Every document change goes through
//! [`Editor::exec`] (or [`Editor::record`] for live-previewed gestures), so
//! undo/redo and dirty tracking are always consistent.

pub mod canvas;
pub mod inspector;
pub mod layouts;
pub mod slides_panel;
pub mod text_edit;

use crate::commands::{Command, History, MergeKey};
use crate::document::{
    Document, Object, ObjectId, ObjectKind, Rect, SLIDE_H, SLIDE_W, Section, Shape, ShapeKind, Slide, SlideId, TextBox,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Text,
    Shape(ShapeKind),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// Fit the slide to the canvas.
    Fit,
    /// Fixed scale (screen points per bp) centred on a slide-space point.
    Manual { scale: f32, center: (f32, f32) },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZOrder {
    Front,
    Forward,
    Backward,
    Back,
}

pub struct Editor {
    pub doc: Document,
    pub history: History,
    /// Index of the slide shown on the canvas.
    pub current: usize,
    /// Selected objects on the current slide.
    pub selection: Vec<ObjectId>,
    pub tool: Tool,
    pub zoom: Zoom,
    /// Text object being edited in place on the canvas.
    pub editing_text: Option<ObjectId>,
    pub clipboard: Vec<Object>,
    paste_count: u32,
    pub path: Option<PathBuf>,
    pub saved_state: u64,
}

impl Editor {
    pub fn new(doc: Document, path: Option<PathBuf>) -> Self {
        let history = History::default();
        let saved_state = history.state();
        Self {
            doc,
            history,
            current: 0,
            selection: Vec::new(),
            tool: Tool::Select,
            zoom: Zoom::Fit,
            editing_text: None,
            clipboard: Vec::new(),
            paste_count: 0,
            path,
            saved_state,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history.state() != self.saved_state
    }

    pub fn mark_saved(&mut self, state: u64) {
        self.saved_state = state;
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "Untitled".to_string())
    }

    pub fn slide(&self) -> &Slide {
        &self.doc.slides[self.current.min(self.doc.slides.len() - 1)]
    }

    pub fn slide_id(&self) -> SlideId {
        self.slide().id
    }

    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.slide().object(id)
    }

    /// Direct mutable access for live gesture previews. The caller must
    /// record a command (see [`Editor::record`]) when the gesture ends.
    pub fn object_mut_for_preview(&mut self, id: ObjectId) -> Option<&mut Object> {
        let i = self.current.min(self.doc.slides.len() - 1);
        self.doc.slides[i].object_mut(id)
    }

    pub fn exec(&mut self, cmd: Command) {
        self.exec_merging(cmd, None);
    }

    pub fn exec_merging(&mut self, cmd: Command, key: Option<MergeKey>) {
        if let Err(e) = self.history.execute_merging(&mut self.doc, cmd, key) {
            tracing::error!("{e}");
        }
        self.fix_up();
    }

    /// Record a command whose effect is already applied (gesture end).
    pub fn record(&mut self, cmd: Command) {
        self.history.record_applied(cmd, None);
        self.fix_up();
    }

    pub fn undo(&mut self) {
        self.editing_text = None;
        match self.history.undo(&mut self.doc) {
            Ok(Some(cmd)) => {
                if let Some(s) = cmd.affected_slide().and_then(|s| self.doc.slide_index(s)) {
                    self.current = s;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!("undo failed: {e}"),
        }
        self.fix_up();
    }

    pub fn redo(&mut self) {
        self.editing_text = None;
        match self.history.redo(&mut self.doc) {
            Ok(Some(cmd)) => {
                if let Some(s) = cmd.affected_slide().and_then(|s| self.doc.slide_index(s)) {
                    self.current = s;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!("redo failed: {e}"),
        }
        self.fix_up();
    }

    /// Keep derived state valid after any document change.
    fn fix_up(&mut self) {
        if self.current >= self.doc.slides.len() {
            self.current = self.doc.slides.len().saturating_sub(1);
        }
        let slide = &self.doc.slides[self.current];
        self.selection.retain(|id| slide.index_of(*id).is_some());
        if let Some(id) = self.editing_text
            && slide.index_of(id).is_none()
        {
            self.editing_text = None;
        }
    }

    pub fn go_to_slide(&mut self, index: usize) {
        let index = index.min(self.doc.slides.len() - 1);
        if index != self.current {
            self.current = index;
            self.selection.clear();
            self.editing_text = None;
            self.history.seal();
        }
    }

    // ---- objects ---------------------------------------------------------

    pub fn insert_object(&mut self, kind: ObjectKind, rect: Rect) -> ObjectId {
        let id = self.doc.alloc_object_id();
        let index = self.slide().objects.len();
        let obj = Object {
            id,
            rect: rect.sanitized(),
            kind,
        };
        self.exec(Command::InsertObjects {
            slide: self.slide_id(),
            items: vec![(index, obj)],
        });
        self.selection = vec![id];
        id
    }

    /// Default placement for objects inserted from menus: centred, with a
    /// small cascade so repeated inserts do not stack exactly.
    pub fn default_rect(&self, w: f32, h: f32) -> Rect {
        let n = (self.slide().objects.len() % 6) as f32 * 12.0;
        Rect::new((SLIDE_W - w) * 0.5 + n, (SLIDE_H - h) * 0.5 + n, w, h)
    }

    pub fn set_object(&mut self, id: ObjectId, kind: ObjectKind, rect: Rect, merge: Option<MergeKey>) {
        let Some(o) = self.object(id) else { return };
        if o.kind == kind && o.rect == rect {
            return;
        }
        let before = (o.kind.clone(), o.rect);
        self.exec_merging(
            Command::SetObject {
                slide: self.slide_id(),
                id,
                before,
                after: (kind, rect),
            },
            merge,
        );
    }

    /// Style of the first selected text box (for the ribbon's controls).
    pub fn selected_text_style(&self) -> Option<TextBox> {
        let s = self.slide();
        self.selection
            .iter()
            .filter_map(|id| s.object(*id))
            .find_map(|o| match &o.kind {
                ObjectKind::Text(t) => Some(t.clone()),
                _ => None,
            })
    }

    /// Change every selected text box with `f` as one undo step.
    pub fn update_selected_text(&mut self, field: u8, f: impl Fn(&mut TextBox)) {
        self.update_selected(field, |k| {
            if let ObjectKind::Text(t) = k {
                f(t);
            }
        });
    }

    /// Change every selected shape with `f` as one undo step.
    pub fn update_selected_shapes(&mut self, field: u8, f: impl Fn(&mut Shape)) {
        self.update_selected(field, |k| {
            if let ObjectKind::Shape(sh) = k {
                f(sh);
            }
        });
    }

    fn update_selected(&mut self, field: u8, f: impl Fn(&mut ObjectKind)) {
        let slide = self.slide_id();
        let s = self.slide();
        let cmds: Vec<Command> = self
            .selection
            .iter()
            .filter_map(|id| s.object(*id))
            .filter_map(|o| {
                let mut k = o.kind.clone();
                f(&mut k);
                (k != o.kind).then(|| Command::SetObject {
                    slide,
                    id: o.id,
                    before: (o.kind.clone(), o.rect),
                    after: (k, o.rect),
                })
            })
            .collect();
        let key = self.selection.first().map(|id| MergeKey::Object(*id, 100 + field));
        match cmds.len() {
            0 => {}
            1 => self.exec_merging(cmds.into_iter().next().expect("one"), key),
            _ => self.exec(Command::Batch(cmds)),
        }
    }

    /// Use the current slide's background on every slide.
    pub fn apply_background_to_all(&mut self) {
        let bg = self.slide().background;
        let cmds: Vec<Command> = self
            .doc
            .slides
            .iter()
            .filter(|s| s.background != bg)
            .map(|s| Command::SetBackground {
                slide: s.id,
                before: s.background,
                after: bg,
            })
            .collect();
        if !cmds.is_empty() {
            self.exec(Command::Batch(cmds));
        }
    }

    fn selected_items(&self) -> Vec<(usize, Object)> {
        let s = self.slide();
        let mut v: Vec<(usize, Object)> = s
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| self.selection.contains(&o.id))
            .map(|(i, o)| (i, o.clone()))
            .collect();
        v.sort_by_key(|(i, _)| *i);
        v
    }

    pub fn delete_selection(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        self.editing_text = None;
        self.exec(Command::RemoveObjects {
            slide: self.slide_id(),
            items,
        });
        self.selection.clear();
    }

    pub fn copy_selection(&mut self) -> bool {
        let items = self.selected_items();
        if items.is_empty() {
            return false;
        }
        self.clipboard = items.into_iter().map(|(_, o)| o).collect();
        self.paste_count = 0;
        true
    }

    pub fn cut_selection(&mut self) -> bool {
        if self.copy_selection() {
            self.delete_selection();
            true
        } else {
            false
        }
    }

    /// Insert copies of `objects` with new ids, offset by `offset` bp.
    fn insert_copies(&mut self, objects: &[Object], offset: f32) {
        if objects.is_empty() {
            return;
        }
        let start = self.slide().objects.len();
        let mut items = Vec::with_capacity(objects.len());
        let mut ids = Vec::with_capacity(objects.len());
        for (n, o) in objects.iter().enumerate() {
            let id = self.doc.alloc_object_id();
            ids.push(id);
            items.push((
                start + n,
                Object {
                    id,
                    rect: o.rect.translated(offset, offset),
                    kind: o.kind.clone(),
                },
            ));
        }
        self.exec(Command::InsertObjects {
            slide: self.slide_id(),
            items,
        });
        self.selection = ids;
    }

    pub fn paste(&mut self) -> bool {
        if self.clipboard.is_empty() {
            return false;
        }
        self.paste_count += 1;
        let objs = self.clipboard.clone();
        // Pasting onto another slide keeps positions; on the same slide offset.
        let same_slide = objs.iter().any(|o| self.slide().index_of(o.id).is_some());
        let off = if same_slide {
            10.0 * self.paste_count as f32
        } else {
            10.0 * (self.paste_count - 1) as f32
        };
        self.insert_copies(&objs, off);
        true
    }

    pub fn duplicate_selection(&mut self) {
        let objs: Vec<Object> = self.selected_items().into_iter().map(|(_, o)| o).collect();
        self.insert_copies(&objs, 10.0);
    }

    pub fn select_all(&mut self) {
        self.selection = self.slide().objects.iter().map(|o| o.id).collect();
    }

    pub fn selection_bounds(&self) -> Option<Rect> {
        let s = self.slide();
        self.selection
            .iter()
            .filter_map(|id| s.object(*id))
            .map(|o| o.rect)
            .reduce(|a, b| a.union(&b))
    }

    pub fn nudge(&mut self, dx: f32, dy: f32) {
        let s = self.slide();
        let changes: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| s.object(*id))
            .map(|o| (o.id, o.rect, o.rect.translated(dx, dy)))
            .collect();
        if changes.is_empty() {
            return;
        }
        let key = MergeKey::Object(changes[0].0, 200);
        self.exec_merging(
            Command::SetRects {
                slide: self.slide_id(),
                changes,
            },
            Some(key),
        );
    }

    /// Align selected objects to each other, or a single object to the slide.
    pub fn align(&mut self, how: Align) {
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let target = if self.selection.len() == 1 {
            Rect::new(0.0, 0.0, SLIDE_W, SLIDE_H)
        } else {
            bounds
        };
        let s = self.slide();
        let changes: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| s.object(*id))
            .map(|o| {
                let r = o.rect;
                let n = match how {
                    Align::Left => Rect { x: target.x, ..r },
                    Align::HCenter => Rect {
                        x: target.x + (target.w - r.w) * 0.5,
                        ..r
                    },
                    Align::Right => Rect {
                        x: target.right() - r.w,
                        ..r
                    },
                    Align::Top => Rect { y: target.y, ..r },
                    Align::VCenter => Rect {
                        y: target.y + (target.h - r.h) * 0.5,
                        ..r
                    },
                    Align::Bottom => Rect {
                        y: target.bottom() - r.h,
                        ..r
                    },
                };
                (o.id, r, n)
            })
            .filter(|(_, a, b)| a != b)
            .collect();
        if !changes.is_empty() {
            self.exec(Command::SetRects {
                slide: self.slide_id(),
                changes,
            });
        }
    }

    pub fn z_order(&mut self, how: ZOrder) {
        let cur: Vec<ObjectId> = self.slide().objects.iter().map(|o| o.id).collect();
        let sel = |id: &ObjectId| self.selection.contains(id);
        let mut want = cur.clone();
        match how {
            ZOrder::Front => {
                want = cur
                    .iter()
                    .filter(|i| !sel(i))
                    .chain(cur.iter().filter(|i| sel(i)))
                    .copied()
                    .collect();
            }
            ZOrder::Back => {
                want = cur
                    .iter()
                    .filter(|i| sel(i))
                    .chain(cur.iter().filter(|i| !sel(i)))
                    .copied()
                    .collect();
            }
            ZOrder::Forward => {
                for i in (0..want.len().saturating_sub(1)).rev() {
                    if sel(&want[i]) && !sel(&want[i + 1]) {
                        want.swap(i, i + 1);
                    }
                }
            }
            ZOrder::Backward => {
                for i in 1..want.len() {
                    if sel(&want[i]) && !sel(&want[i - 1]) {
                        want.swap(i, i - 1);
                    }
                }
            }
        }
        let cmds = moves_to_reorder(&cur, &want, self.slide_id());
        match cmds.len() {
            0 => {}
            1 => self.exec(cmds.into_iter().next().expect("one command")),
            _ => self.exec(Command::Batch(cmds)),
        }
    }

    // ---- slides ----------------------------------------------------------

    pub fn new_slide(&mut self) {
        let slide = Slide::new(self.doc.alloc_slide_id());
        let index = self.current + 1;
        self.exec(Command::InsertSlide { index, slide });
        self.go_to_slide(index);
    }

    /// Insert a slide with a layout after the current one.
    pub fn new_slide_with(&mut self, layout: layouts::Layout) {
        let slide = layouts::new_slide(&mut self.doc, layout);
        let index = self.current + 1;
        self.exec(Command::InsertSlide { index, slide });
        self.go_to_slide(index);
    }

    pub fn duplicate_slide(&mut self) {
        let src = self.slide().clone();
        let slide = self.doc.duplicate_slide(&src);
        let index = self.current + 1;
        self.exec(Command::InsertSlide { index, slide });
        self.go_to_slide(index);
    }

    pub fn delete_slide(&mut self) {
        if self.doc.slides.len() <= 1 {
            // Keep at least one slide: clear it instead.
            let items: Vec<(usize, Object)> = self.slide().objects.iter().cloned().enumerate().collect();
            if !items.is_empty() {
                self.exec(Command::RemoveObjects {
                    slide: self.slide_id(),
                    items,
                });
            }
            return;
        }
        let index = self.current;
        let slide = self.slide().clone();
        let before_sections = self.doc.sections.clone();
        let mut after_sections = before_sections.clone();
        let replacement = self.doc.slides.get(index + 1).map(|next| next.id);
        let replacement_has_section =
            replacement.is_some_and(|id| after_sections.iter().any(|section| section.start == id));
        after_sections.retain_mut(|section| {
            if section.start != slide.id {
                return true;
            }
            if let Some(next) = replacement.filter(|_| !replacement_has_section) {
                section.start = next;
                true
            } else {
                false
            }
        });
        self.selection.clear();
        self.editing_text = None;
        let remove = Command::RemoveSlide { index, slide };
        if before_sections == after_sections {
            self.exec(remove);
        } else {
            self.exec(Command::Batch(vec![
                Command::SetSections {
                    before: before_sections,
                    after: after_sections,
                    focus: replacement,
                },
                remove,
            ]));
        }
        self.current = index.min(self.doc.slides.len() - 1);
    }

    pub fn move_slide(&mut self, from: usize, to: usize) {
        if from == to || from >= self.doc.slides.len() || to >= self.doc.slides.len() {
            return;
        }
        self.exec(Command::MoveSlide { from, to });
        self.current = to;
    }

    /// Add a named section immediately before `index`.
    pub fn add_section_before(&mut self, index: usize) -> Option<SlideId> {
        let start = self.doc.slides.get(index)?.id;
        if self.doc.section_before(start).is_some() {
            return None;
        }
        let before = self.doc.sections.clone();
        let mut after = before.clone();
        let mut number = after.len() + 1;
        let name = loop {
            let candidate = format!("Section {number}");
            if after.iter().all(|section| section.name != candidate) {
                break candidate;
            }
            number += 1;
        };
        after.push(Section { name, start });
        after.sort_by_key(|section| self.doc.slide_index(section.start).unwrap_or(usize::MAX));
        self.exec(Command::SetSections {
            before,
            after,
            focus: Some(start),
        });
        Some(start)
    }

    pub fn rename_section(&mut self, start: SlideId, name: &str) {
        let name: String = name.trim().chars().take(200).collect();
        if name.is_empty() {
            return;
        }
        let before = self.doc.sections.clone();
        let mut after = before.clone();
        let Some(section) = after.iter_mut().find(|section| section.start == start) else {
            return;
        };
        if section.name == name {
            return;
        }
        section.name = name;
        self.exec_merging(
            Command::SetSections {
                before,
                after,
                focus: Some(start),
            },
            Some(MergeKey::Sections),
        );
    }

    pub fn remove_section(&mut self, start: SlideId) {
        let before = self.doc.sections.clone();
        let mut after = before.clone();
        after.retain(|section| section.start != start);
        if before != after {
            self.exec(Command::SetSections {
                before,
                after,
                focus: Some(start),
            });
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        if tool != Tool::Select {
            self.editing_text = None;
        }
    }
}

/// Sequence of single-object moves that turns order `cur` into `want`.
fn moves_to_reorder(cur: &[ObjectId], want: &[ObjectId], slide: SlideId) -> Vec<Command> {
    let mut cur = cur.to_vec();
    let mut cmds = Vec::new();
    for (t, id) in want.iter().enumerate() {
        let Some(p) = cur.iter().position(|x| x == id) else {
            continue;
        };
        if p != t {
            let v = cur.remove(p);
            cur.insert(t, v);
            cmds.push(Command::MoveObject { slide, from: p, to: t });
        }
    }
    cmds
}

/// Default object for a creation tool at `rect`.
pub fn new_object_kind(tool: Tool) -> Option<ObjectKind> {
    Some(match tool {
        Tool::Select => return None,
        Tool::Text => ObjectKind::Text(Default::default()),
        Tool::Shape(k) => ObjectKind::Shape(Shape::new(k)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Color, TextBox};

    fn editor_with(n: usize) -> Editor {
        let mut e = Editor::new(Document::new(), None);
        for i in 0..n {
            e.insert_object(
                ObjectKind::Text(TextBox {
                    text: format!("t{i}"),
                    ..Default::default()
                }),
                Rect::new(10.0 * i as f32, 10.0, 50.0, 20.0),
            );
        }
        e.selection.clear();
        e
    }

    fn order(e: &Editor) -> Vec<u64> {
        e.slide().objects.iter().map(|o| o.id.0).collect()
    }

    #[test]
    fn insert_select_delete_undo() {
        let mut e = editor_with(3);
        assert!(e.is_dirty());
        let ids = order(&e);
        e.selection = vec![ObjectId(ids[0]), ObjectId(ids[2])];
        e.delete_selection();
        assert_eq!(order(&e), vec![ids[1]]);
        assert!(e.selection.is_empty());
        e.undo();
        assert_eq!(order(&e), ids);
    }

    #[test]
    fn copy_paste_creates_new_ids_and_offsets() {
        let mut e = editor_with(2);
        let ids = order(&e);
        e.selection = vec![ObjectId(ids[1])];
        assert!(e.copy_selection());
        assert!(e.paste());
        let new = order(&e);
        assert_eq!(new.len(), 3);
        assert!(!ids.contains(&new[2]));
        assert_eq!(e.selection, vec![ObjectId(new[2])]);
        let r = e.slide().objects[2].rect;
        assert_eq!((r.x, r.y), (20.0, 20.0));
        // Paste onto another slide keeps the original position.
        e.new_slide();
        e.paste();
        let r = e.slide().objects[0].rect;
        assert_eq!((r.x, r.y), (10.0 + 10.0, 10.0 + 10.0));
    }

    #[test]
    fn align_and_nudge() {
        let mut e = editor_with(3);
        e.select_all();
        e.align(Align::Left);
        assert!(e.slide().objects.iter().all(|o| o.rect.x == 0.0));
        e.nudge(5.0, 0.0);
        e.nudge(5.0, 0.0);
        assert!(e.slide().objects.iter().all(|o| o.rect.x == 10.0));
        // Consecutive nudges are one undo step.
        e.undo();
        assert!(e.slide().objects.iter().all(|o| o.rect.x == 0.0));
    }

    #[test]
    fn z_order_single_and_multi() {
        let mut e = editor_with(4);
        let ids = order(&e);
        e.selection = vec![ObjectId(ids[0])];
        e.z_order(ZOrder::Front);
        assert_eq!(order(&e), vec![ids[1], ids[2], ids[3], ids[0]]);
        e.z_order(ZOrder::Backward);
        assert_eq!(order(&e), vec![ids[1], ids[2], ids[0], ids[3]]);
        e.selection = vec![ObjectId(ids[2]), ObjectId(ids[3])];
        e.z_order(ZOrder::Back);
        let o = order(&e);
        assert_eq!(&o[..2], &[ids[2], ids[3]]);
        e.undo();
        assert_eq!(order(&e), vec![ids[1], ids[2], ids[0], ids[3]]);
    }

    #[test]
    fn slides_add_duplicate_move_delete() {
        let mut e = editor_with(2);
        e.duplicate_slide();
        assert_eq!(e.doc.slides.len(), 2);
        assert_eq!(e.current, 1);
        assert_eq!(e.slide().objects.len(), 2);
        assert_ne!(e.doc.slides[0].objects[0].id, e.doc.slides[1].objects[0].id);
        e.new_slide();
        e.move_slide(2, 0);
        assert_eq!(e.current, 0);
        assert!(e.slide().objects.is_empty());
        e.delete_slide();
        assert_eq!(e.doc.slides.len(), 2);
        while e.history.can_undo() {
            e.undo();
        }
        assert_eq!(e.doc.slides.len(), 1);
        assert!(e.doc.slides[0].objects.is_empty());
        assert!(!e.is_dirty());
    }

    #[test]
    fn sections_rename_and_follow_slide_deletion() {
        let mut e = Editor::new(Document::new(), None);
        e.new_slide();
        e.new_slide();
        let start = e.doc.slides[1].id;
        let replacement = e.doc.slides[2].id;
        assert_eq!(e.add_section_before(1), Some(start));
        e.rename_section(start, "Results");
        assert_eq!(
            e.doc.section_before(start).map(|section| section.name.as_str()),
            Some("Results")
        );

        e.go_to_slide(1);
        e.delete_slide();
        assert!(e.doc.section_before(start).is_none());
        assert_eq!(
            e.doc.section_before(replacement).map(|section| section.name.as_str()),
            Some("Results")
        );

        e.undo();
        assert_eq!(
            e.doc.section_before(start).map(|section| section.name.as_str()),
            Some("Results")
        );
        assert!(e.doc.section_before(replacement).is_none());
        e.redo();
        assert!(e.doc.section_before(start).is_none());
        assert_eq!(
            e.doc.section_before(replacement).map(|section| section.name.as_str()),
            Some("Results")
        );
    }

    #[test]
    fn formatting_applies_to_all_selected_text_as_one_step() {
        let mut e = editor_with(3);
        e.select_all();
        e.update_selected_text(1, |t| t.bold = true);
        assert!(
            e.slide()
                .objects
                .iter()
                .all(|o| matches!(&o.kind, ObjectKind::Text(t) if t.bold))
        );
        assert_eq!(e.selected_text_style().map(|t| t.bold), Some(true));
        e.undo();
        assert!(
            e.slide()
                .objects
                .iter()
                .all(|o| matches!(&o.kind, ObjectKind::Text(t) if !t.bold))
        );
        e.new_slide();
        e.go_to_slide(0);
        let before = e.slide().background;
        let sid = e.slide_id();
        e.exec(Command::SetBackground {
            slide: sid,
            before,
            after: Color::BLACK,
        });
        e.apply_background_to_all();
        assert!(e.doc.slides.iter().all(|s| s.background == Color::BLACK));
    }

    #[test]
    fn last_slide_is_cleared_not_removed() {
        let mut e = editor_with(2);
        e.delete_slide();
        assert_eq!(e.doc.slides.len(), 1);
        assert!(e.slide().objects.is_empty());
    }

    #[test]
    fn property_edits_merge() {
        let mut e = editor_with(1);
        let id = e.slide().objects[0].id;
        let rect = e.slide().objects[0].rect;
        for c in [Color::rgb(1, 0, 0), Color::rgb(2, 0, 0), Color::rgb(3, 0, 0)] {
            let mut k = e.object(id).unwrap().kind.clone();
            if let ObjectKind::Text(t) = &mut k {
                t.color = c;
            }
            e.set_object(id, k, rect, Some(MergeKey::Object(id, 1)));
        }
        let n = e.history.len();
        e.undo();
        assert_eq!(e.history.len(), n - 1);
        match &e.object(id).unwrap().kind {
            ObjectKind::Text(t) => assert_eq!(t.color, Color::BLACK),
            _ => unreachable!(),
        }
    }
}
