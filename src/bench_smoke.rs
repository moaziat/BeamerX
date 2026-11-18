//! End-to-end smoke test of the running application (`--bench smoke`).
//!
//! Drives the real UI with injected pointer/keyboard events and the same
//! actions the native menu sends, then checks the document and files.

use crate::app::BeamerApp;
use crate::document::{ObjectId, ObjectKind, Rect};
use crate::platform::menu::MenuAction;
use egui::{Event, Key, Modifiers, PointerButton};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

enum Poll {
    Wait,
    Done,
    Fail(String),
}

#[derive(Default)]
pub struct Smoke {
    step: usize,
    sub: u32,
    started: Option<Instant>,
    results: Vec<Value>,
    text_id: Option<ObjectId>,
    rect_before: Option<Rect>,
    count_before: usize,
    undo_before: usize,
    saved_doc: Option<crate::document::Document>,
}

pub struct Report {
    pub steps: Value,
    pub passed: usize,
    pub failed: usize,
}

const STEPS: [&str; 19] = [
    "warmup",
    "create_text_by_click",
    "type_text",
    "finish_editing",
    "type_on_selected_text",
    "click_selected_text_to_edit",
    "type_inline_math",
    "insert_shape",
    "drag_text",
    "undo_redo",
    "copy_paste_delete",
    "import_image",
    "new_slide",
    "save",
    "reopen",
    "select_without_edit",
    "export_beamer",
    "export_pdf",
    "present",
];

/// Is the on-slide text editor focused (caret in a text box)?
fn slide_editor_focused(app: &BeamerApp, ctx: &egui::Context) -> bool {
    app.ed
        .editing_text
        .is_some_and(|id| ctx.memory(|m| m.has_focus(crate::editor::text_edit::widget_id(id))))
}

fn key(k: Key, pressed: bool) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

impl Smoke {
    pub fn tick(&mut self, app: &mut BeamerApp, ctx: &egui::Context, work: &Path) -> Option<Report> {
        if self.step >= STEPS.len() {
            let passed = self.results.iter().filter(|r| r["ok"] == json!(true)).count();
            return Some(Report {
                steps: Value::Array(self.results.clone()),
                passed,
                failed: self.results.len() - passed,
            });
        }
        let started = *self.started.get_or_insert_with(Instant::now);
        let poll = if started.elapsed() > Duration::from_secs(90) {
            Poll::Fail("timeout".into())
        } else {
            self.poll(app, ctx, work)
        };
        let name = STEPS[self.step];
        match poll {
            Poll::Wait => {
                self.sub += 1;
                return None;
            }
            Poll::Done => {
                self.results
                    .push(json!({"step": name, "ok": true, "ms": started.elapsed().as_millis() as u64}));
            }
            Poll::Fail(why) => {
                tracing::error!("smoke step {name} failed: {why}");
                self.results.push(json!({"step": name, "ok": false, "error": why}));
            }
        }
        self.step += 1;
        self.sub = 0;
        self.started = None;
        None
    }

    fn poll(&mut self, app: &mut BeamerApp, ctx: &egui::Context, work: &Path) -> Poll {
        let sub = self.sub;
        let view = match app.canvas.last_view {
            Some(v) => v,
            None => return Poll::Wait,
        };
        match STEPS[self.step] {
            "warmup" => {
                if sub > 10 {
                    Poll::Done
                } else {
                    Poll::Wait
                }
            }
            "create_text_by_click" => {
                let p = view.pt(60.0, 60.0);
                match sub {
                    0 => {
                        app.ed.set_tool(crate::editor::Tool::Text);
                        app.inject_events().push(Event::PointerMoved(p));
                    }
                    1 => app.inject_events().push(Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    }),
                    2 => app.inject_events().push(Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    }),
                    _ => {
                        if let Some(id) = app.ed.editing_text {
                            self.text_id = Some(id);
                            return Poll::Done;
                        }
                        if sub > 20 {
                            return Poll::Fail("no text object in edit mode after click".into());
                        }
                    }
                }
                Poll::Wait
            }
            "type_text" => {
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                if sub == 0 || !slide_editor_focused(app, ctx) {
                    return if sub > 30 {
                        Poll::Fail("inline editor never got focus".into())
                    } else {
                        Poll::Wait
                    };
                }
                if sub < 1000 && self.rect_before.is_none() {
                    app.inject_events().push(Event::Text("Hello BeamerX".into()));
                    self.rect_before = Some(Rect::default());
                    return Poll::Wait;
                }
                match app.ed.object(id).map(|o| &o.kind) {
                    Some(ObjectKind::Text(t)) if t.text == "Hello BeamerX" => {
                        self.rect_before = None;
                        Poll::Done
                    }
                    _ => Poll::Wait,
                }
            }
            "finish_editing" => {
                if sub == 0 {
                    app.inject_events().push(key(Key::Escape, true));
                    return Poll::Wait;
                }
                if sub == 1 {
                    app.inject_events().push(key(Key::Escape, false));
                }
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                if app.ed.editing_text.is_none() {
                    match app.ed.object(id) {
                        Some(o) if o.rect.h > 0.0 => Poll::Done,
                        Some(_) => Poll::Fail("text box has zero height".into()),
                        None => Poll::Fail("text object vanished".into()),
                    }
                } else {
                    Poll::Wait
                }
            }
            "type_on_selected_text" => {
                // With the box selected (not editing), just typing edits it
                // in place on the slide.
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                match sub {
                    0 => {
                        app.ed.selection = vec![id];
                        app.ed.editing_text = None;
                    }
                    1 => app.inject_events().push(Event::Text("!".into())),
                    2 => {}
                    3 => {
                        if app.ed.editing_text != Some(id) {
                            return Poll::Fail("typing did not start in-place editing".into());
                        }
                        app.inject_events().push(key(Key::Escape, true));
                    }
                    4 => app.inject_events().push(key(Key::Escape, false)),
                    _ => {
                        return match app.ed.object(id).map(|o| &o.kind) {
                            Some(ObjectKind::Text(t))
                                if t.text == "Hello BeamerX!" && app.ed.editing_text.is_none() =>
                            {
                                Poll::Done
                            }
                            Some(ObjectKind::Text(t)) if sub > 20 => Poll::Fail(format!("text is {:?}", t.text)),
                            _ if sub > 20 => Poll::Fail("text object missing".into()),
                            _ => Poll::Wait,
                        };
                    }
                }
                Poll::Wait
            }
            "click_selected_text_to_edit" => {
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                let Some(r) = app.ed.object(id).map(|o| o.rect) else {
                    return Poll::Fail("text gone".into());
                };
                let p = view.pt(r.x + 4.0, r.y + r.h * 0.5);
                match sub {
                    0 => {
                        app.ed.selection = vec![id];
                        app.inject_events().push(Event::PointerMoved(p));
                    }
                    1 => app.inject_events().push(Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    }),
                    2 => app.inject_events().push(Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    }),
                    3..=8 => {}
                    9 => {
                        if app.ed.editing_text != Some(id) || !slide_editor_focused(app, ctx) {
                            return Poll::Fail("click on selected text did not start focused editing".into());
                        }
                        // Typed at the click position (near the start).
                        app.inject_events().push(Event::Text("X".into()));
                    }
                    10 => app.inject_events().push(key(Key::Escape, true)),
                    11 => app.inject_events().push(key(Key::Escape, false)),
                    _ => {
                        return match app.ed.object(id).map(|o| &o.kind) {
                            Some(ObjectKind::Text(t)) if t.text.chars().count() == "Hello BeamerX!".len() + 1 => {
                                // The caret was placed at the click (start of
                                // the box), not at the end.
                                if !t.text.chars().take(2).any(|c| c == 'X') {
                                    return Poll::Fail(format!("caret not at click position: {:?}", t.text));
                                }
                                let mut t = t.clone();
                                t.text = "Hello BeamerX".into();
                                let rect = app.ed.object(id).map(|o| o.rect).unwrap_or_default();
                                app.ed.set_object(id, ObjectKind::Text(t), rect, None);
                                Poll::Done
                            }
                            Some(ObjectKind::Text(t)) if sub > 30 => Poll::Fail(format!("text is {:?}", t.text)),
                            _ if sub > 30 => Poll::Fail("text object missing".into()),
                            _ => Poll::Wait,
                        };
                    }
                }
                Poll::Wait
            }
            "type_inline_math" => {
                // Click at the end of the text, type `$f(x)=x^2$`, and wait
                // until LaTeX has rendered it inline.
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                let Some(o) = app.ed.object(id).cloned() else {
                    return Poll::Fail("text gone".into());
                };
                let ObjectKind::Text(t) = &o.kind else {
                    return Poll::Fail("not text".into());
                };
                let p = view.pt(o.rect.right() - 2.0, o.rect.y + o.rect.h * 0.5);
                match sub {
                    0 => {
                        app.ed.selection.clear();
                        app.inject_events().push(Event::PointerMoved(p));
                    }
                    1 => app.inject_events().push(Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    }),
                    2 => app.inject_events().push(Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    }),
                    3..=6 => {}
                    7 => {
                        if app.ed.editing_text != Some(id) {
                            return Poll::Fail("one click on text did not start editing".into());
                        }
                        app.inject_events().push(key(Key::End, true));
                    }
                    8 => app.inject_events().push(Event::Text(" with $f(x)=x^2$".into())),
                    _ => {
                        if !t.text.ends_with("with $f(x)=x^2$") {
                            return if sub > 40 {
                                Poll::Fail(format!("text is {:?}", t.text))
                            } else {
                                Poll::Wait
                            };
                        }
                        // Closed math renders while still editing.
                        if app.math.peek("f(x)=x^2", t.font_size, false).is_some() {
                            match sub {
                                s if s < 1000 && app.ed.editing_text.is_some() => {
                                    app.inject_events().push(key(Key::Escape, true));
                                    self.sub = 1000;
                                }
                                _ if app.ed.editing_text.is_none() => return Poll::Done,
                                _ => {}
                            }
                        }
                    }
                }
                Poll::Wait
            }
            "insert_shape" => {
                let n = app.ed.slide().objects.len();
                app.on_action(MenuAction::InsertRectangle);
                if app.ed.slide().objects.len() == n + 1 {
                    Poll::Done
                } else {
                    Poll::Fail("rectangle not inserted".into())
                }
            }
            "drag_text" => {
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                let Some(r) = app.ed.object(id).map(|o| o.rect) else {
                    return Poll::Fail("text gone".into());
                };
                if sub == 0 {
                    self.rect_before = Some(r);
                    self.undo_before = app.ed.history.len();
                }
                let r0 = self.rect_before.expect("set");
                let start = view.pt(r0.x + 10.0, r0.y + r0.h * 0.5);
                let inject = app.inject_events();
                match sub {
                    0 => inject.push(Event::PointerMoved(start)),
                    1 => inject.push(Event::PointerButton {
                        pos: start,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    }),
                    2..=21 => {
                        let t = (sub - 1) as f32 / 20.0;
                        inject.push(Event::PointerMoved(start + egui::vec2(40.0, 20.0) * view.scale * t));
                    }
                    22 => {
                        let end = start + egui::vec2(40.0, 20.0) * view.scale;
                        inject.push(Event::PointerButton {
                            pos: end,
                            button: PointerButton::Primary,
                            pressed: false,
                            modifiers: Modifiers::NONE,
                        });
                    }
                    _ => {
                        let dx = r.x - r0.x;
                        let dy = r.y - r0.y;
                        return if dx > 30.0 && dy > 10.0 && app.ed.history.len() == self.undo_before + 1 {
                            Poll::Done
                        } else if sub > 40 {
                            Poll::Fail(format!(
                                "moved by ({dx:.1},{dy:.1}); undo entries {} -> {}",
                                self.undo_before,
                                app.ed.history.len()
                            ))
                        } else {
                            Poll::Wait
                        };
                    }
                }
                Poll::Wait
            }
            "undo_redo" => {
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                let Some(r0) = self.rect_before else {
                    return Poll::Fail("needs step 6".into());
                };
                let Some(moved) = app.ed.object(id).map(|o| o.rect) else {
                    return Poll::Fail("text gone".into());
                };
                app.on_action(MenuAction::Undo);
                let undone = app.ed.object(id).map(|o| o.rect);
                app.on_action(MenuAction::Redo);
                let redone = app.ed.object(id).map(|o| o.rect);
                if undone == Some(r0) && redone == Some(moved) {
                    Poll::Done
                } else {
                    Poll::Fail(format!("undo {undone:?} redo {redone:?}"))
                }
            }
            "copy_paste_delete" => {
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                app.ed.selection = vec![id];
                let n = app.ed.slide().objects.len();
                app.on_action(MenuAction::Copy);
                app.on_action(MenuAction::Paste);
                let after_paste = app.ed.slide().objects.len();
                let pasted_new = app.ed.selection.first().is_some_and(|s| *s != id);
                app.on_action(MenuAction::Delete);
                let after_delete = app.ed.slide().objects.len();
                if after_paste == n + 1 && pasted_new && after_delete == n {
                    Poll::Done
                } else {
                    Poll::Fail(format!("{n} -> paste {after_paste} -> delete {after_delete}"))
                }
            }
            "import_image" => {
                if sub == 0 {
                    self.count_before = app.ed.slide().objects.len();
                    let img =
                        image::RgbImage::from_fn(800, 600, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
                    let p = work.join("smoke.png");
                    img.save(&p).expect("write png");
                    app.import_paths(vec![p], Some((330.0, 150.0)));
                    return Poll::Wait;
                }
                let has_image = app
                    .ed
                    .slide()
                    .objects
                    .iter()
                    .any(|o| matches!(o.kind, ObjectKind::Image(_)));
                if has_image && app.is_idle() && sub > 3 {
                    Poll::Done
                } else {
                    Poll::Wait
                }
            }
            "new_slide" => {
                app.on_action(MenuAction::NewSlide);
                let ok = app.ed.doc.slides.len() == 2 && app.ed.current == 1;
                app.on_action(MenuAction::InsertArrow);
                app.ed.go_to_slide(0);
                if ok {
                    Poll::Done
                } else {
                    Poll::Fail("new slide not created/selected".into())
                }
            }
            "save" => {
                let p = work.join("smoke.beamerx");
                if sub == 0 {
                    app.start_save(p, false);
                    return Poll::Wait;
                }
                if app.is_idle() && !app.ed.is_dirty() && work.join("smoke.beamerx").is_file() {
                    self.saved_doc = Some(app.ed.doc.clone());
                    Poll::Done
                } else {
                    Poll::Wait
                }
            }
            "reopen" => {
                let p = work.join("smoke.beamerx");
                if sub == 0 {
                    app.new_document();
                    app.open_path(Some(p), false);
                    return Poll::Wait;
                }
                if !app.is_idle() || app.ed.path.is_none() {
                    return Poll::Wait;
                }
                let Some(saved) = self.saved_doc.as_ref() else {
                    return Poll::Fail("needs step 11".into());
                };
                if &app.ed.doc == saved && !app.ed.is_dirty() {
                    Poll::Done
                } else {
                    Poll::Fail("reopened document differs from saved one".into())
                }
            }
            "select_without_edit" => {
                // Selecting objects shows the inspector; that must not edit.
                let ids: Vec<_> = app.ed.slide().objects.iter().map(|o| o.id).collect();
                let i = (sub / 3) as usize;
                if let Some(id) = ids.get(i) {
                    app.ed.selection = vec![*id];
                    return Poll::Wait;
                }
                app.ed.selection.clear();
                if !app.ed.is_dirty() && !app.ed.history.can_undo() {
                    Poll::Done
                } else {
                    Poll::Fail(format!("document became dirty ({} undo entries)", app.ed.history.len()))
                }
            }
            "export_beamer" => {
                let dir = work.join("smoke-beamer");
                if sub == 0 {
                    app.start_export(dir, false);
                    return Poll::Wait;
                }
                if !app.is_idle() {
                    return Poll::Wait;
                }
                let n_img = std::fs::read_dir(dir.join("images")).map(|d| d.count()).unwrap_or(0);
                if dir.join("main.tex").is_file() && n_img == 1 {
                    Poll::Done
                } else {
                    Poll::Fail(format!("main.tex or images missing ({n_img})"))
                }
            }
            "export_pdf" => {
                let pdf = work.join("smoke.pdf");
                if sub == 0 {
                    app.start_export(pdf, true);
                    return Poll::Wait;
                }
                if !app.is_idle() {
                    return Poll::Wait;
                }
                match std::fs::read(&pdf)
                    .ok()
                    .and_then(|b| crate::platform::pdf_raster::page_sizes(&b).ok())
                {
                    Some(p) if p.len() == 2 => Poll::Done,
                    Some(p) => Poll::Fail(format!("{} pages", p.len())),
                    None => Poll::Fail("no PDF produced".into()),
                }
            }
            "present" => {
                match sub {
                    0 => app.start_presenting(None),
                    5 => app.inject_events().push(key(Key::ArrowRight, true)),
                    6 => app.inject_events().push(key(Key::ArrowRight, false)),
                    8 => {
                        if app.presenting_index() != Some(1) {
                            return Poll::Fail(format!("presenting index {:?}", app.presenting_index()));
                        }
                        app.inject_events().push(key(Key::Escape, true));
                    }
                    9 => app.inject_events().push(key(Key::Escape, false)),
                    s if s > 12 => {
                        return if app.presenting_index().is_none() {
                            Poll::Done
                        } else {
                            Poll::Fail("did not exit".into())
                        };
                    }
                    _ => {}
                }
                Poll::Wait
            }
            _ => Poll::Done,
        }
    }
}
