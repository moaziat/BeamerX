//! Reversible document operations and bounded undo/redo history.
//!
//! Commands mutate only the affected objects. A drag gesture is recorded as a
//! single [`Command::SetRects`] holding before/after rectangles, never as a
//! sequence of document snapshots.

use crate::document::{Color, DocMeta, Document, Object, ObjectId, ObjectKind, Rect, Slide, SlideId};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Insert objects; each `(index, object)` is inserted in ascending index order.
    InsertObjects {
        slide: SlideId,
        items: Vec<(usize, Object)>,
    },
    /// Remove objects; `items` lists `(index, object)` in ascending index order
    /// as they were before removal.
    RemoveObjects {
        slide: SlideId,
        items: Vec<(usize, Object)>,
    },
    /// Geometry change for several objects: `(id, before, after)`.
    SetRects {
        slide: SlideId,
        changes: Vec<(ObjectId, Rect, Rect)>,
    },
    /// Property change of one object (and optionally its rect).
    SetObject {
        slide: SlideId,
        id: ObjectId,
        before: (ObjectKind, Rect),
        after: (ObjectKind, Rect),
    },
    /// Change paint order of one object.
    MoveObject {
        slide: SlideId,
        from: usize,
        to: usize,
    },
    InsertSlide {
        index: usize,
        slide: Slide,
    },
    RemoveSlide {
        index: usize,
        slide: Slide,
    },
    MoveSlide {
        from: usize,
        to: usize,
    },
    SetBackground {
        slide: SlideId,
        before: Color,
        after: Color,
    },
    SetMeta {
        before: DocMeta,
        after: DocMeta,
    },
    Batch(Vec<Command>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ApplyError(pub String);

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot apply command: {}", self.0)
    }
}
impl std::error::Error for ApplyError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ApplyError> {
    Err(ApplyError(msg.into()))
}

fn slide_mut(doc: &mut Document, id: SlideId) -> Result<&mut Slide, ApplyError> {
    match doc.slide_mut(id) {
        Some(s) => Ok(s),
        None => err(format!("slide {} not found", id.0)),
    }
}

impl Command {
    /// Apply to the document. On error the document is left unchanged for
    /// non-batch commands; batches are rolled back.
    pub fn apply(&self, doc: &mut Document) -> Result<(), ApplyError> {
        match self {
            Command::InsertObjects { slide, items } => {
                let s = slide_mut(doc, *slide)?;
                if let Some((_, o)) = items.iter().find(|(_, o)| s.index_of(o.id).is_some()) {
                    return err(format!("object {} already exists", o.id.0));
                }
                for (i, o) in items {
                    let i = (*i).min(s.objects.len());
                    s.objects.insert(i, o.clone());
                }
                Ok(())
            }
            Command::RemoveObjects { slide, items } => {
                let s = slide_mut(doc, *slide)?;
                // Verify first so that failure leaves the slide untouched.
                if let Some((_, o)) = items.iter().find(|(_, o)| s.index_of(o.id).is_none()) {
                    return err(format!("object {} not found", o.id.0));
                }
                for (_, o) in items.iter().rev() {
                    if let Some(idx) = s.index_of(o.id) {
                        s.objects.remove(idx);
                    }
                }
                Ok(())
            }
            Command::SetRects { slide, changes } => {
                let s = slide_mut(doc, *slide)?;
                if changes.iter().any(|(id, _, _)| s.index_of(*id).is_none()) {
                    return err("object not found");
                }
                for (id, _, after) in changes {
                    if let Some(o) = s.object_mut(*id) {
                        o.rect = *after;
                    }
                }
                Ok(())
            }
            Command::SetObject { slide, id, after, .. } => {
                let s = slide_mut(doc, *slide)?;
                let Some(o) = s.object_mut(*id) else {
                    return err("object not found");
                };
                o.kind = after.0.clone();
                o.rect = after.1;
                Ok(())
            }
            Command::MoveObject { slide, from, to } => {
                let s = slide_mut(doc, *slide)?;
                if *from >= s.objects.len() || *to >= s.objects.len() {
                    return err("z-order index out of range");
                }
                let o = s.objects.remove(*from);
                s.objects.insert(*to, o);
                Ok(())
            }
            Command::InsertSlide { index, slide } => {
                if *index > doc.slides.len() {
                    return err("slide index out of range");
                }
                doc.slides.insert(*index, slide.clone());
                Ok(())
            }
            Command::RemoveSlide { index, slide } => {
                if doc.slides.get(*index).map(|s| s.id) != Some(slide.id) {
                    return err("slide mismatch");
                }
                doc.slides.remove(*index);
                Ok(())
            }
            Command::MoveSlide { from, to } => {
                if *from >= doc.slides.len() || *to >= doc.slides.len() {
                    return err("slide index out of range");
                }
                let s = doc.slides.remove(*from);
                doc.slides.insert(*to, s);
                Ok(())
            }
            Command::SetBackground { slide, after, .. } => {
                slide_mut(doc, *slide)?.background = *after;
                Ok(())
            }
            Command::SetMeta { after, .. } => {
                doc.meta = after.clone();
                Ok(())
            }
            Command::Batch(cmds) => {
                for (n, c) in cmds.iter().enumerate() {
                    if let Err(e) = c.apply(doc) {
                        // Roll back what was applied so far.
                        for done in cmds[..n].iter().rev() {
                            let _ = done.inverse().apply(doc);
                        }
                        return Err(e);
                    }
                }
                Ok(())
            }
        }
    }

    pub fn inverse(&self) -> Command {
        match self {
            Command::InsertObjects { slide, items } => Command::RemoveObjects {
                slide: *slide,
                items: items.clone(),
            },
            Command::RemoveObjects { slide, items } => Command::InsertObjects {
                slide: *slide,
                items: items.clone(),
            },
            Command::SetRects { slide, changes } => Command::SetRects {
                slide: *slide,
                changes: changes.iter().map(|(id, b, a)| (*id, *a, *b)).collect(),
            },
            Command::SetObject {
                slide,
                id,
                before,
                after,
            } => Command::SetObject {
                slide: *slide,
                id: *id,
                before: after.clone(),
                after: before.clone(),
            },
            Command::MoveObject { slide, from, to } => Command::MoveObject {
                slide: *slide,
                from: *to,
                to: *from,
            },
            Command::InsertSlide { index, slide } => Command::RemoveSlide {
                index: *index,
                slide: slide.clone(),
            },
            Command::RemoveSlide { index, slide } => Command::InsertSlide {
                index: *index,
                slide: slide.clone(),
            },
            Command::MoveSlide { from, to } => Command::MoveSlide { from: *to, to: *from },
            Command::SetBackground { slide, before, after } => Command::SetBackground {
                slide: *slide,
                before: *after,
                after: *before,
            },
            Command::SetMeta { before, after } => Command::SetMeta {
                before: after.clone(),
                after: before.clone(),
            },
            Command::Batch(cmds) => Command::Batch(cmds.iter().rev().map(Command::inverse).collect()),
        }
    }

    /// The slide this command visibly affects, used to navigate on undo.
    pub fn affected_slide(&self) -> Option<SlideId> {
        match self {
            Command::InsertObjects { slide, .. }
            | Command::RemoveObjects { slide, .. }
            | Command::SetRects { slide, .. }
            | Command::SetObject { slide, .. }
            | Command::MoveObject { slide, .. }
            | Command::SetBackground { slide, .. } => Some(*slide),
            Command::InsertSlide { slide, .. } | Command::RemoveSlide { slide, .. } => Some(slide.id),
            Command::Batch(c) => c.iter().find_map(Command::affected_slide),
            Command::MoveSlide { .. } | Command::SetMeta { .. } => None,
        }
    }

    /// Approximate memory held by this command (for history bounding).
    pub fn approx_bytes(&self) -> usize {
        let base = std::mem::size_of::<Self>();
        base + match self {
            Command::InsertObjects { items, .. } | Command::RemoveObjects { items, .. } => {
                items.iter().map(|(_, o)| o.approx_bytes() + 8).sum()
            }
            Command::SetRects { changes, .. } => changes.len() * std::mem::size_of::<(ObjectId, Rect, Rect)>(),
            Command::SetObject { before, after, .. } => before.0.approx_bytes() + after.0.approx_bytes(),
            Command::InsertSlide { slide, .. } | Command::RemoveSlide { slide, .. } => slide.approx_bytes(),
            Command::SetMeta { before, after } => {
                before.title.len() + before.preamble.len() + after.title.len() + after.preamble.len()
            }
            Command::Batch(c) => c.iter().map(Command::approx_bytes).sum(),
            _ => 0,
        }
    }
}

/// Key identifying edits that may be merged into the previous history entry
/// (e.g. consecutive keystrokes in the same text field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKey {
    Object(ObjectId, u8),
    Meta(u8),
    Background(SlideId),
}

struct Entry {
    cmd: Command,
    bytes: usize,
    merge_key: Option<MergeKey>,
    last_merge: Instant,
    state_before: u64,
    state_after: u64,
}

/// Undo/redo history bounded by entry count and approximate memory.
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    merge_window: Duration,
    state: u64,
    next_state: u64,
}

impl Default for History {
    fn default() -> Self {
        Self::new(1000, 64 * 1024 * 1024)
    }
}

impl History {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            bytes: 0,
            max_entries,
            max_bytes,
            merge_window: Duration::from_millis(1500),
            state: 0,
            next_state: 1,
        }
    }

    /// Identifier of the current document state; equal ids mean equal
    /// content (used for the "unsaved changes" indicator).
    pub fn state(&self) -> u64 {
        self.state
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn len(&self) -> usize {
        self.undo.len()
    }
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
        self.state = self.next_state;
        self.next_state += 1;
    }

    /// Apply `cmd` and record it.
    pub fn execute(&mut self, doc: &mut Document, cmd: Command) -> Result<(), ApplyError> {
        self.execute_merging(doc, cmd, None)
    }

    /// Apply and record `cmd`; if `merge_key` matches the most recent entry
    /// within the merge window, fold into it instead of adding a new entry.
    pub fn execute_merging(
        &mut self,
        doc: &mut Document,
        cmd: Command,
        merge_key: Option<MergeKey>,
    ) -> Result<(), ApplyError> {
        cmd.apply(doc)?;
        self.record_applied(cmd, merge_key);
        Ok(())
    }

    /// Record a command whose effect is already present in the document
    /// (e.g. a drag that was previewed live).
    pub fn record_applied(&mut self, cmd: Command, merge_key: Option<MergeKey>) {
        self.drop_redo();
        let now = Instant::now();
        if let (Some(k), Some(last)) = (merge_key, self.undo.last_mut())
            && last.merge_key == Some(k)
            && now.duration_since(last.last_merge) < self.merge_window
            && last.state_after == self.state
            && let Some(merged) = merge(&last.cmd, &cmd)
        {
            self.bytes -= last.bytes;
            last.bytes = merged.approx_bytes();
            last.cmd = merged;
            last.last_merge = now;
            self.bytes += last.bytes;
            // Keep the same state id progression: new content, new id.
            last.state_after = self.next_state;
            self.state = self.next_state;
            self.next_state += 1;
            return;
        }
        let bytes = cmd.approx_bytes();
        let entry = Entry {
            cmd,
            bytes,
            merge_key,
            last_merge: now,
            state_before: self.state,
            state_after: self.next_state,
        };
        self.state = self.next_state;
        self.next_state += 1;
        self.bytes += bytes;
        self.undo.push(entry);
        self.enforce_bounds();
    }

    /// Break merging so the next edit starts a new undo step.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.merge_key = None;
        }
    }

    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<&Command>, ApplyError> {
        let Some(entry) = self.undo.pop() else {
            return Ok(None);
        };
        if let Err(e) = entry.cmd.inverse().apply(doc) {
            self.bytes -= entry.bytes;
            return Err(e);
        }
        self.state = entry.state_before;
        self.bytes -= entry.bytes;
        self.redo.push(entry);
        Ok(self.redo.last().map(|e| &e.cmd))
    }

    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<&Command>, ApplyError> {
        let Some(mut entry) = self.redo.pop() else {
            return Ok(None);
        };
        entry.cmd.apply(doc)?;
        self.state = entry.state_after;
        entry.merge_key = None;
        self.bytes += entry.bytes;
        self.undo.push(entry);
        Ok(self.undo.last().map(|e| &e.cmd))
    }

    fn drop_redo(&mut self) {
        self.redo.clear();
    }

    fn enforce_bounds(&mut self) {
        let mut drop_n = 0;
        while self.undo.len() - drop_n > 1
            && (self.undo.len() - drop_n > self.max_entries || self.bytes > self.max_bytes)
        {
            self.bytes -= self.undo[drop_n].bytes;
            drop_n += 1;
        }
        if drop_n > 0 {
            self.undo.drain(..drop_n);
        }
    }
}

/// Merge two consecutive edits of the same target into one command.
fn merge(prev: &Command, next: &Command) -> Option<Command> {
    match (prev, next) {
        (
            Command::SetObject { slide, id, before, .. },
            Command::SetObject {
                slide: s2,
                id: i2,
                after,
                ..
            },
        ) if slide == s2 && id == i2 => Some(Command::SetObject {
            slide: *slide,
            id: *id,
            before: before.clone(),
            after: after.clone(),
        }),
        (Command::SetMeta { before, .. }, Command::SetMeta { after, .. }) => Some(Command::SetMeta {
            before: before.clone(),
            after: after.clone(),
        }),
        (Command::SetBackground { slide, before, .. }, Command::SetBackground { slide: s2, after, .. })
            if slide == s2 =>
        {
            Some(Command::SetBackground {
                slide: *slide,
                before: *before,
                after: *after,
            })
        }
        (Command::SetRects { slide, changes: c1 }, Command::SetRects { slide: s2, changes: c2 })
            if slide == s2 && c1.len() == c2.len() && c1.iter().zip(c2).all(|(a, b)| a.0 == b.0) =>
        {
            Some(Command::SetRects {
                slide: *slide,
                changes: c1.iter().zip(c2).map(|(a, b)| (a.0, a.1, b.2)).collect(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ObjectKind, TextBox};

    fn text_obj(doc: &mut Document, s: &str) -> Object {
        Object {
            id: doc.alloc_object_id(),
            rect: Rect::new(10.0, 10.0, 100.0, 20.0),
            kind: ObjectKind::Text(TextBox {
                text: s.into(),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn insert_move_undo_redo_roundtrip() {
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let original = doc.clone();
        let mut h = History::default();
        let a = text_obj(&mut doc, "a");
        let b = text_obj(&mut doc, "b");
        let (ida, idb) = (a.id, b.id);
        let after_alloc = doc.clone();
        h.execute(
            &mut doc,
            Command::InsertObjects {
                slide: sid,
                items: vec![(0, a), (1, b)],
            },
        )
        .unwrap();
        h.execute(
            &mut doc,
            Command::SetRects {
                slide: sid,
                changes: vec![(
                    ida,
                    Rect::new(10.0, 10.0, 100.0, 20.0),
                    Rect::new(50.0, 60.0, 100.0, 20.0),
                )],
            },
        )
        .unwrap();
        h.execute(
            &mut doc,
            Command::MoveObject {
                slide: sid,
                from: 0,
                to: 1,
            },
        )
        .unwrap();
        assert_eq!(doc.slides[0].objects[0].id, idb);
        assert_eq!(doc.slides[0].objects[1].rect.x, 50.0);
        let edited = doc.clone();
        while h.can_undo() {
            h.undo(&mut doc).unwrap();
        }
        assert_eq!(doc.slides, after_alloc.slides);
        assert_eq!(doc.slides, original.slides);
        while h.can_redo() {
            h.redo(&mut doc).unwrap();
        }
        assert_eq!(doc, edited);
    }

    #[test]
    fn remove_noncontiguous_and_undo_restores_order() {
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let objs: Vec<Object> = (0..5).map(|i| text_obj(&mut doc, &i.to_string())).collect();
        doc.slides[0].objects = objs.clone();
        let before = doc.clone();
        let mut h = History::default();
        let items = vec![(1, objs[1].clone()), (3, objs[3].clone())];
        h.execute(&mut doc, Command::RemoveObjects { slide: sid, items })
            .unwrap();
        let ids: Vec<u64> = doc.slides[0].objects.iter().map(|o| o.id.0).collect();
        assert_eq!(ids, vec![objs[0].id.0, objs[2].id.0, objs[4].id.0]);
        h.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
    }

    #[test]
    fn slide_commands_roundtrip() {
        let mut doc = Document::new();
        let mut h = History::default();
        let s0 = doc.slides[0].clone();
        let s1 = doc.duplicate_slide(&s0);
        let s2 = Slide::new(doc.alloc_slide_id());
        h.execute(&mut doc, Command::InsertSlide { index: 1, slide: s1 })
            .unwrap();
        h.execute(
            &mut doc,
            Command::InsertSlide {
                index: 2,
                slide: s2.clone(),
            },
        )
        .unwrap();
        h.execute(&mut doc, Command::MoveSlide { from: 2, to: 0 }).unwrap();
        assert_eq!(doc.slides[0].id, s2.id);
        h.execute(
            &mut doc,
            Command::RemoveSlide {
                index: 0,
                slide: s2.clone(),
            },
        )
        .unwrap();
        assert_eq!(doc.slides.len(), 2);
        for _ in 0..4 {
            h.undo(&mut doc).unwrap();
        }
        assert_eq!(doc.slides, vec![s0]);
    }

    #[test]
    fn text_edits_merge_into_one_undo_step() {
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let o = text_obj(&mut doc, "");
        let id = o.id;
        doc.slides[0].objects.push(o.clone());
        let mut h = History::default();
        let mut cur = o.kind.clone();
        for ch in "hello".chars() {
            let mut next = cur.clone();
            if let ObjectKind::Text(t) = &mut next {
                t.text.push(ch);
            }
            h.execute_merging(
                &mut doc,
                Command::SetObject {
                    slide: sid,
                    id,
                    before: (cur.clone(), o.rect),
                    after: (next.clone(), o.rect),
                },
                Some(MergeKey::Object(id, 0)),
            )
            .unwrap();
            cur = next;
        }
        assert_eq!(h.len(), 1);
        h.undo(&mut doc).unwrap();
        assert_eq!(doc.slides[0].objects[0].kind, o.kind);
    }

    #[test]
    fn dirty_state_tracks_undo_to_saved_point() {
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let mut h = History::default();
        let saved = h.state();
        h.execute(
            &mut doc,
            Command::SetBackground {
                slide: sid,
                before: Color::WHITE,
                after: Color::BLACK,
            },
        )
        .unwrap();
        assert_ne!(h.state(), saved);
        h.undo(&mut doc).unwrap();
        assert_eq!(h.state(), saved);
        h.redo(&mut doc).unwrap();
        assert_ne!(h.state(), saved);
    }

    #[test]
    fn failed_batch_rolls_back() {
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let o = text_obj(&mut doc, "x");
        let before = doc.clone();
        let bad = Command::Batch(vec![
            Command::InsertObjects {
                slide: sid,
                items: vec![(0, o)],
            },
            Command::MoveObject {
                slide: sid,
                from: 5,
                to: 0,
            },
        ]);
        assert!(bad.apply(&mut doc).is_err());
        assert_eq!(doc, before);
    }

    #[test]
    fn history_is_bounded() {
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let mut h = History::new(50, 1 << 30);
        for i in 0..200u8 {
            let c = Color::rgb(i, 0, 0);
            let prev = doc.slides[0].background;
            h.execute(
                &mut doc,
                Command::SetBackground {
                    slide: sid,
                    before: prev,
                    after: c,
                },
            )
            .unwrap();
        }
        assert_eq!(h.len(), 50);
        // Memory bound.
        let mut h = History::new(10_000, 64 * 1024);
        for _ in 0..200 {
            let o = text_obj(&mut doc, &"x".repeat(4096));
            h.execute(
                &mut doc,
                Command::InsertObjects {
                    slide: sid,
                    items: vec![(0, o)],
                },
            )
            .unwrap();
        }
        assert!(h.bytes() <= 64 * 1024 + 8192, "{}", h.bytes());
        assert!(h.len() < 200);
    }
}
