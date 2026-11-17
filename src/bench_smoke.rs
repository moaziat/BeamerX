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

const STEPS: [&str; 16] = [
    "warmup",
    "create_text_by_click",
    "type_text",
    "finish_editing",
    "insert_equation_typeset",
    "insert_shape",
    "drag_text",
    "undo_redo",
    "copy_paste_delete",
    "import_image",
    "new_slide",
    "save",
    "reopen",
    "export_beamer",
    "export_pdf",
    "present",
];

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
        match self.step {
            0 => {
                if sub > 10 {
                    Poll::Done
                } else {
                    Poll::Wait
                }
            }
            1 => {
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
            2 => {
                let Some(id) = self.text_id else {
                    return Poll::Fail("needs step 1".into());
                };
                if sub == 0 || !ctx.text_edit_focused() {
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
            3 => {
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
            4 => {
                if sub == 0 {
                    app.on_action(MenuAction::InsertEquation);
                    let id = app.ed.selection[0];
                    let o = app.ed.object(id).cloned().expect("inserted");
                    let mut k = o.kind.clone();
                    if let ObjectKind::Equation(e) = &mut k {
                        e.source = r"\frac{a}{b} + \sqrt{x^2+1}".into();
                    }
                    app.ed.set_object(id, k, o.rect, None);
                    return Poll::Wait;
                }
                let id = app.ed.selection.first().copied();
                let Some(o) = id.and_then(|id| app.ed.object(id)) else {
                    return Poll::Fail("equation lost".into());
                };
                let ObjectKind::Equation(e) = &o.kind else {
                    return Poll::Fail("not an equation".into());
                };
                match app.math.peek(&e.source, e.font_size) {
                    Some(r) if (o.rect.w - r.size_bp.0).abs() < 0.01 && o.rect.w > 10.0 => Poll::Done,
                    _ => Poll::Wait,
                }
            }
            5 => {
                let n = app.ed.slide().objects.len();
                app.on_action(MenuAction::InsertRectangle);
                if app.ed.slide().objects.len() == n + 1 {
                    Poll::Done
                } else {
                    Poll::Fail("rectangle not inserted".into())
                }
            }
            6 => {
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
            7 => {
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
            8 => {
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
            9 => {
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
            10 => {
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
            11 => {
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
            12 => {
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
            13 => {
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
            14 => {
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
            15 => {
                match sub {
                    0 => app.start_presenting(),
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
