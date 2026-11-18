//! Application shell: panels, menus, dialogs and background jobs.

use crate::assets::{AssetMeta, AssetStore};
use crate::document::{ImageRef, Object, ObjectKind, Rect, SLIDE_H, SLIDE_W, SlideId};
use crate::editor::canvas::{self, CanvasRequest, CanvasState};
use crate::editor::layouts::Layout;
use crate::editor::text_edit::{self, Caret};
use crate::editor::{Editor, Tool, Zoom, inspector, slides_panel};
use crate::jobs::WorkerPool;
use crate::latex::Toolchain;
use crate::math::MathService;
use crate::perf::FrameStats;
use crate::persistence::{self, Loaded, SaveSnapshot};
use crate::platform::menu::{MenuAction, NativeMenu};
use crate::renderer::{self, PaintOpts, Services, View, textures::TextureCache};
use crate::ui::icons::{self, Icon};
use crate::{commands::Command, document::ShapeKind, export};
use anyhow::Result;
use egui::{Color32, RichText, ViewportCommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const AUTOSAVE_AFTER: Duration = Duration::from_secs(30);
const TOAST_FOR: Duration = Duration::from_secs(5);

enum IoMsg {
    Imported {
        res: Result<AssetMeta>,
        slide: SlideId,
        at: Option<(f32, f32)>,
        nth: usize,
    },
    Saved {
        path: PathBuf,
        state: u64,
        res: Result<()>,
        autosave: bool,
        original: Option<PathBuf>,
    },
    Opened {
        path: Option<PathBuf>,
        res: Result<Loaded>,
        recovered: bool,
    },
    Exported {
        pdf: bool,
        path: PathBuf,
        res: Result<()>,
        ms: u128,
    },
}

#[derive(Clone, Debug)]
enum After {
    Quit,
    New,
    Open(Option<PathBuf>),
}

#[derive(Default)]
struct Busy {
    saving: bool,
    autosaving: bool,
    opening: bool,
    importing: usize,
    exporting: Option<(String, Arc<AtomicBool>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RibbonTab {
    Home,
    Insert,
    Design,
    Arrange,
    SlideShow,
    View,
}

struct Presenting {
    index: usize,
    was_fullscreen: bool,
}

struct Toast {
    text: String,
    error: bool,
    until: Instant,
}

#[derive(Default)]
struct Autosave {
    last_state: Option<u64>,
    dirty_since: Option<Instant>,
}

pub struct BeamerApp {
    pub ed: Editor,
    pub canvas: CanvasState,
    pub assets: AssetStore,
    pub textures: TextureCache,
    pub math: MathService,
    io: WorkerPool,
    io_tx: Sender<IoMsg>,
    io_rx: Receiver<IoMsg>,
    menu: Option<NativeMenu>,
    menu_rx: Receiver<MenuAction>,
    inject: Vec<egui::Event>,
    busy: Busy,
    after_save: Option<After>,
    allow_close: bool,
    presenting: Option<Presenting>,
    pub perf: FrameStats,
    pub show_hud: bool,
    tab: RibbonTab,
    show_format_pane: bool,
    show_props: bool,
    toasts: Vec<Toast>,
    window_title: String,
    autosave: Autosave,
    recovery_dir: PathBuf,
    rss: Option<(Instant, u64)>,
    pub bench: Option<crate::bench::Bench>,
    ctx: egui::Context,
}

impl BeamerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>, bench: Option<crate::bench::Bench>) -> Self {
        let ctx = cc.egui_ctx.clone();
        renderer::install_fonts(&ctx);
        crate::ui::install_style(&ctx);
        crate::ui::ThemeChoice::Light.apply(&ctx);
        ctx.all_styles_mut(|s| {
            s.spacing.button_padding = egui::vec2(6.0, 3.0);
        });
        let toolchain = Toolchain::detect();
        match &toolchain {
            Some(tc) => tracing::info!("LaTeX engine: {}", tc.describe()),
            None => {
                tracing::warn!("no LaTeX engine found; equations and PDF export are unavailable")
            }
        }
        let waker_ctx = ctx.clone();
        let math = MathService::new(
            toolchain,
            crate::platform::cache_dir(),
            Arc::new(move || waker_ctx.request_repaint()),
        );
        let assets = AssetStore::new_session().expect("cannot create the session asset directory");
        let (menu_tx, menu_rx) = channel();
        let menu = if bench.is_none() {
            NativeMenu::install(ctx.clone(), menu_tx)
        } else {
            None
        };
        let (io_tx, io_rx) = channel();
        let mut app = Self {
            ed: Editor::new(crate::editor::layouts::new_presentation(), None),
            canvas: CanvasState::default(),
            assets,
            textures: TextureCache::new(ctx.clone(), renderer::textures::DEFAULT_BUDGET),
            math,
            io: WorkerPool::new("io", 2),
            io_tx,
            io_rx,
            menu,
            menu_rx,
            inject: Vec::new(),
            busy: Busy::default(),
            after_save: None,
            allow_close: false,
            presenting: None,
            perf: FrameStats::default(),
            show_hud: false,
            tab: RibbonTab::Home,
            show_format_pane: false,
            show_props: false,
            toasts: Vec::new(),
            window_title: String::new(),
            autosave: Autosave::default(),
            recovery_dir: crate::platform::data_dir().join("Recovery"),
            rss: None,
            bench,
            ctx,
        };
        if app.bench.is_none() {
            app.offer_recovery();
        }
        if let Some(p) = open {
            app.open_path(Some(p), false);
        }
        app
    }

    // ---------------------------------------------------------------------
    // Messages & dialogs
    // ---------------------------------------------------------------------

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            tracing::warn!("{text}");
        } else {
            tracing::info!("{text}");
        }
        self.toasts.push(Toast {
            text,
            error,
            until: Instant::now() + TOAST_FOR,
        });
        self.ctx.request_repaint();
    }

    fn error_dialog(&self, title: &str, detail: &str) {
        tracing::error!("{title}: {detail}");
        if self.bench.is_some() {
            return;
        }
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(detail)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }

    /// Returns true if the caller may proceed now (nothing to save or the
    /// user chose not to save). If the user chose to save, `then` runs after
    /// the save completes.
    fn confirm_discard(&mut self, then: After) -> bool {
        if !self.ed.is_dirty() || self.bench.is_some() {
            return true;
        }
        let res = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Save changes?")
            .set_description(format!(
                "Do you want to save the changes you made to “{}”?\n\nYour changes will be lost if you don't save them.",
                self.ed.display_name()
            ))
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                "Save".into(),
                "Don't Save".into(),
                "Cancel".into(),
            ))
            .show();
        match res {
            rfd::MessageDialogResult::Custom(s) if s == "Save" => {
                self.after_save = Some(then);
                self.save(false);
                false
            }
            rfd::MessageDialogResult::Custom(s) if s == "Don't Save" => true,
            rfd::MessageDialogResult::Yes => {
                self.after_save = Some(then);
                self.save(false);
                false
            }
            rfd::MessageDialogResult::No => true,
            _ => false,
        }
    }

    fn run_after(&mut self, a: After) {
        match a {
            After::Quit => {
                self.allow_close = true;
                self.ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            After::New => self.new_document(),
            After::Open(p) => self.open_path(p, false),
        }
    }

    // ---------------------------------------------------------------------
    // Documents
    // ---------------------------------------------------------------------

    fn reset_session(&mut self, ed: Editor) {
        self.ed = ed;
        self.canvas = CanvasState::default();
        self.assets.clear();
        self.textures.clear();
        self.autosave = Autosave::default();
    }

    pub fn new_document(&mut self) {
        self.reset_session(Editor::new(crate::editor::layouts::new_presentation(), None));
    }

    pub fn snapshot(&self) -> SaveSnapshot {
        let assets = self
            .ed
            .doc
            .referenced_assets()
            .into_iter()
            .filter_map(|id| self.assets.get(&id).map(|m| (m.clone(), self.assets.path_of(m))))
            .collect();
        SaveSnapshot {
            doc: self.ed.doc.clone(),
            assets,
        }
    }

    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Open Presentation")
            .add_filter("BeamerX presentation", &[persistence::EXTENSION])
            .pick_file()
        else {
            return;
        };
        self.open_path(Some(path), false);
    }

    pub fn open_path(&mut self, path: Option<PathBuf>, recovered: bool) {
        let Some(path) = path else {
            self.open_dialog();
            return;
        };
        if self.busy.opening {
            return;
        }
        self.busy.opening = true;
        let dir = self.assets.dir().to_path_buf();
        let (tx, ctx) = (self.io_tx.clone(), self.ctx.clone());
        self.io.spawn(move || {
            let res = persistence::load_package(&path, &dir);
            let path = if recovered { None } else { Some(path) };
            let _ = tx.send(IoMsg::Opened { path, res, recovered });
            ctx.request_repaint();
        });
    }

    pub fn save(&mut self, save_as: bool) {
        if self.busy.saving {
            return;
        }
        let path = match (&self.ed.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let name = format!("{}.{}", self.ed.display_name(), persistence::EXTENSION);
                let Some(mut p) = rfd::FileDialog::new()
                    .set_title("Save Presentation")
                    .set_file_name(&name)
                    .add_filter("BeamerX presentation", &[persistence::EXTENSION])
                    .save_file()
                else {
                    self.after_save = None;
                    return;
                };
                if p.extension().and_then(|e| e.to_str()) != Some(persistence::EXTENSION) {
                    p.set_extension(persistence::EXTENSION);
                }
                p
            }
        };
        self.start_save(path, false);
    }

    pub fn start_save(&mut self, path: PathBuf, autosave: bool) {
        let snap = self.snapshot();
        let state = self.ed.history.state();
        let original = self.ed.path.clone();
        if autosave {
            self.busy.autosaving = true;
        } else {
            self.busy.saving = true;
        }
        let (tx, ctx) = (self.io_tx.clone(), self.ctx.clone());
        self.io.spawn(move || {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let res = persistence::save_package(&path, &snap);
            let _ = tx.send(IoMsg::Saved {
                path,
                state,
                res,
                autosave,
                original,
            });
            ctx.request_repaint();
        });
    }

    fn recovery_file(&self) -> PathBuf {
        self.recovery_dir.join("autosave.beamerx")
    }

    fn offer_recovery(&mut self) {
        let file = self.recovery_file();
        if !file.exists() {
            return;
        }
        let res = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Recover unsaved changes?")
            .set_description("BeamerX did not quit normally last time. An automatic backup of your unsaved presentation is available.")
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Recover".into(), "Discard".into()))
            .show();
        match res {
            rfd::MessageDialogResult::Custom(s) if s == "Recover" => self.open_path(Some(file), true),
            rfd::MessageDialogResult::Ok => self.open_path(Some(file), true),
            _ => self.discard_recovery(),
        }
    }

    fn discard_recovery(&self) {
        let _ = std::fs::remove_file(self.recovery_file());
        let _ = std::fs::remove_file(self.recovery_dir.join("autosave.json"));
    }

    fn maybe_autosave(&mut self, now: Instant) {
        let state = self.ed.history.state();
        if !self.ed.is_dirty() || self.autosave.last_state == Some(state) {
            self.autosave.dirty_since = None;
            return;
        }
        let since = *self.autosave.dirty_since.get_or_insert(now);
        if self.busy.autosaving || self.canvas.is_gesturing() || self.bench.is_some() {
            return;
        }
        let due = since + AUTOSAVE_AFTER;
        if now >= due {
            self.autosave.dirty_since = None;
            let f = self.recovery_file();
            self.start_save(f, true);
        } else {
            self.ctx.request_repaint_after(due - now);
        }
    }

    // ---------------------------------------------------------------------
    // Import & export
    // ---------------------------------------------------------------------

    fn insert_image_dialog(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Insert Image")
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_files()
        else {
            return;
        };
        self.import_paths(paths, None);
    }

    pub fn import_paths(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) {
        let slide = self.ed.slide_id();
        for (nth, path) in paths.into_iter().enumerate() {
            self.busy.importing += 1;
            let dir = self.assets.dir().to_path_buf();
            let (tx, ctx) = (self.io_tx.clone(), self.ctx.clone());
            self.io.spawn(move || {
                let res = AssetStore::import_file(&dir, &path);
                let _ = tx.send(IoMsg::Imported { res, slide, at, nth });
                ctx.request_repaint();
            });
        }
    }

    fn place_image(&mut self, meta: AssetMeta, slide: SlideId, at: Option<(f32, f32)>, nth: usize) {
        // Natural size at 150 dpi, fitted into 60 % of the slide.
        let (mut w, mut h) = (meta.width as f32 * 72.0 / 150.0, meta.height as f32 * 72.0 / 150.0);
        let fit = (SLIDE_W * 0.6 / w).min(SLIDE_H * 0.6 / h).min(1.0);
        w *= fit;
        h *= fit;
        let (cx, cy) = at.unwrap_or((SLIDE_W * 0.5, SLIDE_H * 0.5));
        let off = nth as f32 * 14.0;
        let rect = Rect::new(cx - w * 0.5 + off, cy - h * 0.5 + off, w, h);
        let kind = ObjectKind::Image(ImageRef { asset: meta.id.clone() });
        self.assets.insert(meta);
        match self.ed.doc.slide_index(slide) {
            Some(i) if i == self.ed.current => {
                self.ed.insert_object(kind, rect);
            }
            Some(i) => {
                let id = self.ed.doc.alloc_object_id();
                let index = self.ed.doc.slides[i].objects.len();
                self.ed.exec(Command::InsertObjects {
                    slide,
                    items: vec![(index, Object { id, rect, kind })],
                });
            }
            None => {
                self.ed.insert_object(kind, rect);
            }
        }
    }

    fn export(&mut self, pdf: bool) {
        if self.busy.exporting.is_some() {
            self.toast("An export is already running.", true);
            return;
        }
        let name = self.ed.display_name();
        let picked = if pdf {
            rfd::FileDialog::new()
                .set_title("Export PDF")
                .set_file_name(format!("{name}.pdf"))
                .add_filter("PDF", &["pdf"])
                .save_file()
                .map(|mut p| {
                    if p.extension().and_then(|e| e.to_str()) != Some("pdf") {
                        p.set_extension("pdf");
                    }
                    p
                })
        } else {
            rfd::FileDialog::new()
                .set_title("Export Beamer Folder")
                .set_file_name(format!("{name}-beamer"))
                .save_file()
        };
        let Some(path) = picked else { return };
        self.start_export(path, pdf);
    }

    pub fn start_export(&mut self, path: PathBuf, pdf: bool) {
        let tc = self.math.toolchain().cloned();
        if pdf && tc.is_none() {
            self.error_dialog("Cannot export PDF", crate::math::NO_ENGINE);
            return;
        }
        let missing = self
            .ed
            .doc
            .referenced_assets()
            .iter()
            .filter(|a| self.assets.get(a).is_none())
            .count();
        if missing > 0 {
            self.toast(
                format!("{missing} missing image(s) are exported as placeholders."),
                true,
            );
        }
        let snap = self.snapshot();
        let cancel = Arc::new(AtomicBool::new(false));
        let label = if pdf {
            "Exporting PDF…"
        } else {
            "Exporting Beamer folder…"
        };
        self.busy.exporting = Some((label.into(), Arc::clone(&cancel)));
        let (tx, ctx) = (self.io_tx.clone(), self.ctx.clone());
        self.io.spawn(move || {
            let t = Instant::now();
            let res = if pdf {
                export::export_pdf(&path, &snap, tc.as_ref().expect("checked above"), &cancel)
            } else {
                export::export_tex_package(&path, &snap)
            };
            let _ = tx.send(IoMsg::Exported {
                pdf,
                path,
                res,
                ms: t.elapsed().as_millis(),
            });
            ctx.request_repaint();
        });
    }

    // ---------------------------------------------------------------------
    // Event pumping
    // ---------------------------------------------------------------------

    fn pump_io(&mut self) {
        while let Ok(msg) = self.io_rx.try_recv() {
            match msg {
                IoMsg::Imported { res, slide, at, nth } => {
                    self.busy.importing = self.busy.importing.saturating_sub(1);
                    match res {
                        Ok(meta) => self.place_image(meta, slide, at, nth),
                        Err(e) => self.error_dialog("Cannot import image", &format!("{e:#}")),
                    }
                }
                IoMsg::Saved {
                    path,
                    state,
                    res,
                    autosave,
                    original,
                } => {
                    if autosave {
                        self.busy.autosaving = false;
                        match res {
                            Ok(()) => {
                                self.autosave.last_state = Some(state);
                                let meta = serde_json::json!({ "original": original });
                                let _ = std::fs::write(self.recovery_dir.join("autosave.json"), meta.to_string());
                            }
                            Err(e) => tracing::warn!("autosave failed: {e:#}"),
                        }
                        continue;
                    }
                    self.busy.saving = false;
                    match res {
                        Ok(()) => {
                            self.ed.mark_saved(state);
                            self.ed.path = Some(path.clone());
                            self.discard_recovery();
                            self.toast(format!("Saved {}", path.display()), false);
                            if let Some(a) = self.after_save.take() {
                                self.run_after(a);
                            }
                        }
                        Err(e) => {
                            self.after_save = None;
                            self.error_dialog("The presentation could not be saved", &format!("{e:#}"));
                        }
                    }
                }
                IoMsg::Opened { path, res, recovered } => {
                    self.busy.opening = false;
                    match res {
                        Ok(loaded) => {
                            let original = if recovered {
                                std::fs::read_to_string(self.recovery_dir.join("autosave.json"))
                                    .ok()
                                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                    .and_then(|v| v.get("original").and_then(|o| o.as_str()).map(PathBuf::from))
                            } else {
                                path
                            };
                            let mut ed = Editor::new(loaded.doc, original);
                            if recovered {
                                // Recovered work is unsaved by definition.
                                ed.saved_state = u64::MAX;
                            }
                            self.reset_session(ed);
                            for a in loaded.assets {
                                self.assets.insert(a);
                            }
                            if !loaded.warnings.is_empty() {
                                self.error_dialog("Some content could not be loaded", &loaded.warnings.join("\n"));
                            }
                            if recovered {
                                self.discard_recovery();
                            }
                        }
                        Err(e) => self.error_dialog("The presentation could not be opened", &format!("{e:#}")),
                    }
                }
                IoMsg::Exported { pdf, path, res, ms } => {
                    self.busy.exporting = None;
                    match res {
                        Ok(()) => self.toast(
                            format!(
                                "Exported {} in {:.1} s: {}",
                                if pdf { "PDF" } else { "Beamer folder" },
                                ms as f64 / 1000.0,
                                path.display()
                            ),
                            false,
                        ),
                        Err(e) if e.downcast_ref::<crate::latex::Cancelled>().is_some() => {
                            self.toast("Export cancelled.", false)
                        }
                        Err(e) => self.error_dialog(
                            if pdf {
                                "PDF export failed"
                            } else {
                                "Beamer export failed"
                            },
                            &format!("{e:#}"),
                        ),
                    }
                }
            }
        }
    }

    pub fn on_action(&mut self, a: MenuAction) {
        use MenuAction as A;
        let text_focus = self.ctx.text_edit_focused() || self.slide_text_focused();
        match a {
            A::About => {
                let _ = rfd::MessageDialog::new()
                    .set_title("About BeamerX")
                    .set_description(format!(
                        "BeamerX {}\n\nA lightweight native editor for scientific Beamer presentations.\n\nLaTeX engine: {}",
                        env!("CARGO_PKG_VERSION"),
                        self.math.toolchain().map(|t| t.describe()).unwrap_or_else(|| "not found".into())
                    ))
                    .show();
            }
            A::Quit => self.ctx.send_viewport_cmd(ViewportCommand::Close),
            A::New => {
                if self.confirm_discard(After::New) {
                    self.new_document();
                }
            }
            A::Open => {
                if self.confirm_discard(After::Open(None)) {
                    self.open_dialog();
                }
            }
            A::Save => self.save(false),
            A::SaveAs => self.save(true),
            A::ExportBeamer => self.export(false),
            A::ExportPdf => self.export(true),
            A::Undo => self.ed.undo(),
            A::Redo => self.ed.redo(),
            A::Cut if text_focus => self.ctx.send_viewport_cmd(ViewportCommand::RequestCut),
            A::Copy if text_focus => self.ctx.send_viewport_cmd(ViewportCommand::RequestCopy),
            A::Paste if text_focus => self.ctx.send_viewport_cmd(ViewportCommand::RequestPaste),
            A::Cut => {
                self.ed.cut_selection();
            }
            A::Copy => {
                self.ed.copy_selection();
            }
            A::Paste => {
                self.ed.paste();
            }
            A::SelectAll if text_focus => self.inject.push(egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }),
            A::SelectAll => self.ed.select_all(),
            A::Duplicate => self.ed.duplicate_selection(),
            A::Delete if !text_focus => self.ed.delete_selection(),
            A::Delete => {}
            A::InsertText => self.insert_kind(Tool::Text),
            A::InsertMath => self.insert_math(),
            A::InsertImage => self.insert_image_dialog(),
            A::InsertRectangle => self.insert_kind(Tool::Shape(ShapeKind::Rectangle)),
            A::InsertEllipse => self.insert_kind(Tool::Shape(ShapeKind::Ellipse)),
            A::InsertLine => self.insert_kind(Tool::Shape(ShapeKind::Line)),
            A::InsertArrow => self.insert_kind(Tool::Shape(ShapeKind::Arrow)),
            A::BringToFront => self.ed.z_order(crate::editor::ZOrder::Front),
            A::BringForward => self.ed.z_order(crate::editor::ZOrder::Forward),
            A::SendBackward => self.ed.z_order(crate::editor::ZOrder::Backward),
            A::SendToBack => self.ed.z_order(crate::editor::ZOrder::Back),
            A::NewSlide => self.ed.new_slide_with(Layout::TitleAndContent),
            A::DuplicateSlide => self.ed.duplicate_slide(),
            A::DeleteSlide => self.ed.delete_slide(),
            A::MoveSlideUp => {
                let c = self.ed.current;
                if c > 0 {
                    self.ed.move_slide(c, c - 1);
                }
            }
            A::MoveSlideDown => {
                let c = self.ed.current;
                self.ed.move_slide(c, c + 1);
            }
            A::Present => self.start_presenting(None),
            A::ZoomIn => self.zoom_by(1.25),
            A::ZoomOut => self.zoom_by(0.8),
            A::ZoomToFit => self.ed.zoom = Zoom::Fit,
            A::ActualSize => {
                self.ed.zoom = Zoom::Manual {
                    scale: 1.0,
                    center: (SLIDE_W * 0.5, SLIDE_H * 0.5),
                };
            }
            A::TogglePerfHud => self.show_hud = !self.show_hud,
        }
    }

    /// Is the caret in a text box on the slide?
    fn slide_text_focused(&self) -> bool {
        self.ed.editing_text.is_some_and(|id| {
            self.ctx
                .memory(|m| m.has_focus(crate::editor::text_edit::widget_id(id)))
        })
    }

    fn insert_kind(&mut self, tool: Tool) {
        let Some(mut kind) = crate::editor::new_object_kind(tool) else {
            return;
        };
        let rect = match &mut kind {
            ObjectKind::Shape(s) if s.kind.is_linear() => {
                let r = self.ed.default_rect(100.0, 0.0);
                s.set_endpoints((r.x, r.y), (r.right(), r.y))
            }
            ObjectKind::Shape(_) => self.ed.default_rect(90.0, 55.0),
            ObjectKind::Text(t) => self
                .ed
                .default_rect(240.0, t.font_size * crate::document::typeset::LINE_SPREAD),
            ObjectKind::Image(_) => return,
        };
        let is_text = matches!(kind, ObjectKind::Text(_));
        let id = self.ed.insert_object(kind, rect);
        if is_text {
            canvas::begin_text_edit(&mut self.ed, &mut self.canvas, id, Caret::End);
        }
    }

    /// Insert `$$` with the caret between, in the text being edited or in a
    /// new text box. Typing then produces inline math.
    pub fn insert_math(&mut self) {
        if crate::editor::text_edit::insert_at_caret(&mut self.ed, &mut self.canvas.rich, "$$", 1) {
            return;
        }
        let t = crate::document::TextBox {
            text: "$$".into(),
            ..Default::default()
        };
        let rect = self
            .ed
            .default_rect(240.0, t.font_size * crate::document::typeset::LINE_SPREAD);
        let id = self.ed.insert_object(ObjectKind::Text(t), rect);
        canvas::begin_text_edit(&mut self.ed, &mut self.canvas, id, Caret::Index(1));
    }

    fn zoom_by(&mut self, f: f32) {
        let view = self.canvas.last_view;
        let (scale, center) = match (self.ed.zoom, view) {
            (Zoom::Manual { scale, center }, _) => (scale, center),
            (Zoom::Fit, Some(v)) => (v.scale, (SLIDE_W * 0.5, SLIDE_H * 0.5)),
            (Zoom::Fit, None) => (1.0, (SLIDE_W * 0.5, SLIDE_H * 0.5)),
        };
        self.ed.zoom = Zoom::Manual {
            scale: (scale * f).clamp(0.1, 16.0),
            center,
        };
    }

    pub fn start_presenting(&mut self, from: Option<usize>) {
        if self.presenting.is_some() {
            return;
        }
        canvas::finish_text_edit(&mut self.ed, &mut self.canvas);
        let was_fullscreen = self.ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        self.presenting = Some(Presenting {
            index: from.unwrap_or(self.ed.current),
            was_fullscreen,
        });
        self.ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
    }

    /// Slide index shown in presentation mode, if presenting.
    pub fn presenting_index(&self) -> Option<usize> {
        self.presenting.as_ref().map(|p| p.index)
    }

    fn stop_presenting(&mut self) {
        if let Some(p) = self.presenting.take() {
            if !p.was_fullscreen {
                self.ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
            }
            self.ed.go_to_slide(p.index);
        }
    }

    // ---------------------------------------------------------------------
    // UI
    // ---------------------------------------------------------------------

    fn present_ui(&mut self, ui: &mut egui::Ui) {
        let n = self.ed.doc.slides.len();
        let (next, prev, exit, home, end) = ui.input(|i| {
            use egui::Key::*;
            (
                [ArrowRight, ArrowDown, Space, PageDown, Enter, N]
                    .iter()
                    .any(|k| i.key_pressed(*k)),
                [ArrowLeft, ArrowUp, PageUp, Backspace, P]
                    .iter()
                    .any(|k| i.key_pressed(*k)),
                i.key_pressed(Escape),
                i.key_pressed(Home),
                i.key_pressed(End),
            )
        });
        let Some(p) = &mut self.presenting else {
            return;
        };
        if next {
            p.index = (p.index + 1).min(n - 1);
        }
        if prev {
            p.index = p.index.saturating_sub(1);
        }
        if home {
            p.index = 0;
        }
        if end {
            p.index = n - 1;
        }
        let index = p.index.min(n - 1);
        let mut clicked = false;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (resp, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click());
            painter.rect_filled(resp.rect, 0.0, Color32::BLACK);
            let view = View::fit(resp.rect, 0.0);
            let mut sv = Services {
                assets: &self.assets,
                textures: &mut self.textures,
                math: &mut self.math,
                now: Instant::now(),
                text_heights: None,
            };
            renderer::paint_slide(
                &painter,
                &view,
                &self.ed.doc.slides[index],
                &mut sv,
                PaintOpts::default(),
            );
            clicked = resp.clicked();
        });
        if clicked && let Some(p) = &mut self.presenting {
            p.index = (p.index + 1).min(n - 1);
        }
        if exit {
            self.stop_presenting();
        }
    }

    // ---------------------------------------------------------------------
    // Ribbon (PowerPoint-style)
    // ---------------------------------------------------------------------

    fn ribbon(&mut self, ui: &mut egui::Ui) -> bool {
        let mut insert_image = false;
        // Tab row: File menu, tabs, quick actions.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let file = egui::Button::new(RichText::new("  File  ").color(Color32::WHITE).strong()).fill(RIBBON_ACCENT);
            egui::containers::menu::MenuButton::from_button(file).ui(ui, |ui| {
                ui.set_min_width(220.0);
                for (label, action) in [
                    ("New", MenuAction::New),
                    ("Open…", MenuAction::Open),
                    ("Save", MenuAction::Save),
                    ("Save As…", MenuAction::SaveAs),
                    ("Export PDF…", MenuAction::ExportPdf),
                    ("Export Beamer (.tex)…", MenuAction::ExportBeamer),
                ] {
                    if matches!(action, MenuAction::Save | MenuAction::ExportPdf) {
                        ui.separator();
                    }
                    if ui.button(label).clicked() {
                        self.on_action(action);
                    }
                }
            });
            ui.add_space(6.0);
            for (tab, label) in [
                (RibbonTab::Home, "Home"),
                (RibbonTab::Insert, "Insert"),
                (RibbonTab::Design, "Design"),
                (RibbonTab::Arrange, "Arrange"),
                (RibbonTab::SlideShow, "Slide Show"),
                (RibbonTab::View, "View"),
            ] {
                let selected = self.tab == tab;
                let text = if selected {
                    RichText::new(label).strong().color(RIBBON_ACCENT)
                } else {
                    RichText::new(label)
                };
                let r = ui.add(egui::Button::new(text).frame(false).min_size(egui::vec2(64.0, 24.0)));
                if selected {
                    ui.painter().hline(
                        r.rect.x_range().shrink(8.0),
                        r.rect.bottom(),
                        egui::Stroke::new(2.5, RIBBON_ACCENT),
                    );
                }
                if r.clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::small_accent(ui, Icon::Present, "Present", RIBBON_ACCENT)
                    .on_hover_text("Present from the current slide (⌘↩)")
                    .clicked()
                {
                    self.start_presenting(None);
                }
                ui.add_enabled_ui(self.ed.history.can_redo(), |ui| {
                    if icons::small(ui, Icon::Redo, "Redo").on_hover_text("⇧⌘Z").clicked() {
                        self.ed.redo();
                    }
                });
                ui.add_enabled_ui(self.ed.history.can_undo(), |ui| {
                    if icons::small(ui, Icon::Undo, "Undo").on_hover_text("⌘Z").clicked() {
                        self.ed.undo();
                    }
                });
            });
        });
        ui.add_space(2.0);
        // Ribbon body.
        // Labels never wrap (a narrow window would otherwise stack them one
        // character per line); the ribbon scrolls sideways instead.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let pal = crate::ui::palette_of(ui);
        crate::ui::card(&pal).show(ui, |ui| {
            egui::ScrollArea::horizontal().id_salt("ribbon-body").show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.set_height(RIBBON_H);
                    match self.tab {
                        RibbonTab::Home => self.tab_home(ui, &mut insert_image),
                        RibbonTab::Insert => self.tab_insert(ui, &mut insert_image),
                        RibbonTab::Design => self.tab_design(ui),
                        RibbonTab::Arrange => self.tab_arrange(ui),
                        RibbonTab::SlideShow => self.tab_slideshow(ui),
                        RibbonTab::View => self.tab_view(ui),
                    }
                });
            });
        });
        insert_image
    }

    fn tab_home(&mut self, ui: &mut egui::Ui, insert_image: &mut bool) {
        group(ui, "Clipboard", |ui| {
            if icons::big(ui, Icon::Paste, "Paste").clicked() {
                self.on_action(MenuAction::Paste);
            }
            ui.vertical(|ui| {
                if icons::small(ui, Icon::Cut, "Cut").clicked() {
                    self.on_action(MenuAction::Cut);
                }
                if icons::small(ui, Icon::Copy, "Copy").clicked() {
                    self.on_action(MenuAction::Copy);
                }
            });
        });
        group(ui, "Slides", |ui| {
            self.new_slide_buttons(ui);
        });
        self.font_group(ui);
        self.paragraph_group(ui);
        group(ui, "Drawing", |ui| {
            self.shape_gallery(ui, false);
            if icons::big(ui, Icon::Picture, "Picture…").clicked() {
                *insert_image = true;
            }
            self.shape_style(ui);
        });
        group(ui, "Math", |ui| {
            if icons::big(ui, Icon::Math, "Math\n$ … $")
                .on_hover_text("Insert inline LaTeX math (⌥⌘E). Or just type $…$ in any text box.")
                .clicked()
            {
                self.insert_math();
            }
        });
        group(ui, "Arrange", |ui| {
            ui.vertical(|ui| {
                if icons::small(ui, Icon::Front, "Bring to Front").clicked() {
                    self.ed.z_order(crate::editor::ZOrder::Front);
                }
                if icons::small(ui, Icon::Back, "Send to Back").clicked() {
                    self.ed.z_order(crate::editor::ZOrder::Back);
                }
            });
        });
    }

    /// "New Slide" (Title and Content) plus a layout menu, as in PowerPoint.
    fn new_slide_buttons(&mut self, ui: &mut egui::Ui) {
        if icons::big(ui, Icon::NewSlide, "New\nSlide")
            .on_hover_text("New slide with title and content (⇧⌘N)")
            .clicked()
        {
            self.ed.new_slide_with(Layout::TitleAndContent);
        }
        ui.vertical(|ui| {
            let layout = icons::small(ui, Icon::Layout, "Layout").on_hover_text("New slide with a layout");
            egui::Popup::menu(&layout).show(|ui| {
                for l in Layout::ALL {
                    if ui.button(l.label()).clicked() {
                        self.ed.new_slide_with(l);
                        ui.close();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icons::small(ui, Icon::Section, "")
                    .on_hover_text("Add section before the current slide")
                    .clicked()
                {
                    self.ed.add_section_before(self.ed.current);
                }
                if icons::small(ui, Icon::Duplicate, "")
                    .on_hover_text("Duplicate slide")
                    .clicked()
                {
                    self.ed.duplicate_slide();
                }
                if icons::small(ui, Icon::Delete, "")
                    .on_hover_text("Delete slide")
                    .clicked()
                {
                    self.ed.delete_slide();
                }
            });
        });
    }

    fn font_group(&mut self, ui: &mut egui::Ui) {
        let style = self.ed.selected_text_style();
        let cur = text_edit::current_char(&self.ed, &self.canvas.rich);
        let ctx = ui.ctx().clone();
        group(ui, "Font", |ui| {
            ui.add_enabled_ui(style.is_some(), |ui| {
                let mut t = style.clone().unwrap_or_default();
                let cur = cur.unwrap_or(crate::document::Resolved {
                    bold: false,
                    italic: false,
                    color: t.color,
                });
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if inspector::font_size_edit(ui, &mut t.font_size) {
                            let v = t.font_size;
                            self.ed.update_selected_text(0, |x| x.font_size = v);
                        }
                        if icons::text(ui, RichText::new("A+").strong())
                            .on_hover_text("Increase font size")
                            .clicked()
                        {
                            self.ed.update_selected_text(0, |x| {
                                x.font_size = (x.font_size + 2.0).min(crate::document::MAX_FONT_SIZE)
                            });
                        }
                        if icons::text(ui, RichText::new("A−"))
                            .on_hover_text("Decrease font size")
                            .clicked()
                        {
                            self.ed.update_selected_text(0, |x| {
                                x.font_size = (x.font_size - 2.0).max(crate::document::MIN_FONT_SIZE)
                            });
                        }
                    });
                    ui.horizontal(|ui| {
                        let tip = "selection, word at the caret, or what you type next";
                        if icons::text_toggle(ui, RichText::new("B").strong(), cur.bold)
                            .on_hover_text(format!("Bold (⌘B): {tip}"))
                            .clicked()
                        {
                            text_edit::toggle_style(&mut self.ed, &mut self.canvas.rich, 0);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                        if icons::text_toggle(ui, RichText::new("I").italics(), cur.italic)
                            .on_hover_text(format!("Italic (⌘I): {tip}"))
                            .clicked()
                        {
                            text_edit::toggle_style(&mut self.ed, &mut self.canvas.rich, 1);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                        ui.label(RichText::new("Color").small());
                        let mut c = cur.color;
                        if inspector::color_edit(ui, &mut c) {
                            text_edit::set_color(&mut self.ed, &mut self.canvas.rich, c);
                        }
                    });
                });
            });
        });
    }

    fn paragraph_group(&mut self, ui: &mut egui::Ui) {
        use crate::document::{ListKind, ParaStyle, TextAlign};
        let style = self.ed.selected_text_style();
        let para = text_edit::current_para(&self.ed, &self.canvas.rich).unwrap_or_default();
        let ctx = ui.ctx().clone();
        group(ui, "Paragraph", |ui| {
            ui.add_enabled_ui(style.is_some(), |ui| {
                let t = style.clone().unwrap_or_default();
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if icons::small_toggle(ui, Icon::Bullets, "", para.list == ListKind::Bullet)
                            .on_hover_text("Bullets")
                            .clicked()
                        {
                            text_edit::toggle_list(&mut self.ed, &mut self.canvas.rich, ListKind::Bullet);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                        if icons::small_toggle(ui, Icon::Numbering, "", para.list == ListKind::Number)
                            .on_hover_text("Numbering")
                            .clicked()
                        {
                            text_edit::toggle_list(&mut self.ed, &mut self.canvas.rich, ListKind::Number);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                        if icons::small(ui, Icon::Outdent, "")
                            .on_hover_text("Decrease list level (⇧Tab)")
                            .clicked()
                        {
                            text_edit::shift_level(&mut self.ed, &mut self.canvas.rich, -1);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                        if icons::small(ui, Icon::Indent, "")
                            .on_hover_text("Increase list level (Tab)")
                            .clicked()
                        {
                            text_edit::shift_level(&mut self.ed, &mut self.canvas.rich, 1);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                    });
                    ui.horizontal(|ui| {
                        // Paragraph style: titles and headings.
                        let mut chosen = None;
                        egui::ComboBox::from_id_salt("para-style")
                            .selected_text(para.style.label())
                            .width(92.0)
                            .show_ui(ui, |ui| {
                                for st in ParaStyle::ALL {
                                    if ui.selectable_label(para.style == st, st.label()).clicked() {
                                        chosen = Some(st);
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Paragraph style (Title, Subtitle, Headings, Body)");
                        if let Some(st) = chosen {
                            text_edit::update_paras(&mut self.ed, &mut self.canvas.rich, |p| p.style = st);
                            text_edit::refocus(&ctx, &self.ed);
                        }
                        for (a, icon, label) in [
                            (TextAlign::Left, Icon::AlignLeft, "Align left"),
                            (TextAlign::Center, Icon::AlignCenter, "Align center"),
                            (TextAlign::Right, Icon::AlignRight, "Align right"),
                        ] {
                            if icons::small_toggle(ui, icon, "", t.align == a)
                                .on_hover_text(label)
                                .clicked()
                            {
                                self.ed.update_selected_text(4, |x| x.align = a);
                                text_edit::refocus(&ctx, &self.ed);
                            }
                        }
                    });
                });
            });
        });
    }

    /// Compact PowerPoint-style gallery. Home selects a drawing tool; Insert
    /// places a shape immediately at the centre of the current slide.
    fn shape_gallery(&mut self, ui: &mut egui::Ui, insert: bool) {
        let active = !insert && matches!(self.ed.tool, Tool::Shape(_));
        let button = icons::big_toggle(ui, Icon::Shapes, "Shapes", active).on_hover_text(if insert {
            "Insert a shape"
        } else {
            "Choose a shape, then click or drag on the slide"
        });
        let mut chosen = None;
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(250.0);
            crate::ui::heading(ui, "SHAPES");
            egui::Grid::new(if insert { "insert-shapes" } else { "draw-shapes" })
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for (i, (label, icon, kind)) in [
                        ("Rectangle", Icon::Rectangle, ShapeKind::Rectangle),
                        ("Rounded rectangle", Icon::RoundedRectangle, ShapeKind::RoundedRectangle),
                        ("Oval", Icon::Oval, ShapeKind::Ellipse),
                        ("Triangle", Icon::Triangle, ShapeKind::Triangle),
                        ("Diamond", Icon::Diamond, ShapeKind::Diamond),
                        ("Line", Icon::Line, ShapeKind::Line),
                        ("Arrow", Icon::Arrow, ShapeKind::Arrow),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if icons::small(ui, icon, label).clicked() {
                            chosen = Some(kind);
                            ui.close();
                        }
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                    ui.end_row();
                });
        });
        if let Some(kind) = chosen {
            if insert {
                self.insert_kind(Tool::Shape(kind));
            } else {
                self.ed.set_tool(Tool::Shape(kind));
            }
        }
    }

    fn shape_style(&mut self, ui: &mut egui::Ui) {
        let shape = self
            .ed
            .selection
            .iter()
            .find_map(|id| match self.ed.object(*id).map(|o| &o.kind) {
                Some(ObjectKind::Shape(s)) => Some(s.clone()),
                _ => None,
            });
        ui.add_enabled_ui(shape.is_some(), |ui| {
            let mut s = shape.unwrap_or_else(|| crate::document::Shape::new(ShapeKind::Rectangle));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Fill").small());
                    let mut fill = s.fill.unwrap_or(crate::document::Color::LIGHT_FILL);
                    if inspector::color_edit(ui, &mut fill) {
                        self.ed.update_selected_shapes(10, |x| {
                            if !x.kind.is_linear() {
                                x.fill = Some(fill)
                            }
                        });
                    }
                    if ui.small_button("None").on_hover_text("No fill").clicked() {
                        self.ed.update_selected_shapes(10, |x| x.fill = None);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Outline").small());
                    if inspector::color_edit(ui, &mut s.stroke) {
                        let v = s.stroke;
                        self.ed.update_selected_shapes(11, |x| x.stroke = v);
                    }
                    let w = egui::DragValue::new(&mut s.stroke_width)
                        .speed(0.1)
                        .range(0.0..=30.0)
                        .max_decimals(1)
                        .suffix(" pt");
                    if ui.add(w).changed() {
                        let v = s.stroke_width;
                        self.ed.update_selected_shapes(12, |x| x.stroke_width = v);
                    }
                });
            });
        });
    }

    fn tab_insert(&mut self, ui: &mut egui::Ui, insert_image: &mut bool) {
        group(ui, "Slides", |ui| {
            self.new_slide_buttons(ui);
        });
        group(ui, "Text", |ui| {
            if icons::big(ui, Icon::TextBox, "Text\nBox")
                .on_hover_text("Insert a text box and start typing")
                .clicked()
            {
                self.insert_kind(Tool::Text);
            }
            if icons::big(ui, Icon::Math, "Math\n$ … $")
                .on_hover_text("Inline LaTeX math. Type $…$ in any text box; $$…$$ for a centred formula.")
                .clicked()
            {
                self.insert_math();
            }
        });
        group(ui, "Images", |ui| {
            if icons::big(ui, Icon::Picture, "Picture…")
                .on_hover_text("PNG or JPEG. You can also drop files from Finder onto the slide.")
                .clicked()
            {
                *insert_image = true;
            }
        });
        group(ui, "Shapes", |ui| {
            self.shape_gallery(ui, true);
        });
        ui.label(
            RichText::new("Math is written like LaTeX inside text:\n$E = mc^2$ inline  ·  $$\\int_0^1 f$$ centred  ·  \\$ for a dollar sign")
                .small()
                .weak(),
        );
    }

    fn tab_design(&mut self, ui: &mut egui::Ui) {
        group(ui, "Background", |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Slide color");
                    let mut bg = self.ed.slide().background;
                    if inspector::color_edit(ui, &mut bg) {
                        let before = self.ed.slide().background;
                        let sid = self.ed.slide_id();
                        self.ed.exec_merging(
                            Command::SetBackground {
                                slide: sid,
                                before,
                                after: bg,
                            },
                            Some(crate::commands::MergeKey::Background(sid)),
                        );
                    }
                });
                if icons::small(ui, Icon::Background, "Apply to All Slides").clicked() {
                    self.ed.apply_background_to_all();
                }
            });
        });
        group(ui, "Presentation", |ui| {
            if icons::big(ui, Icon::Properties, "Title &\nLaTeX…")
                .on_hover_text("Title, author and the LaTeX preamble (macros, packages)")
                .clicked()
            {
                self.show_props = true;
            }
        });
    }

    fn tab_arrange(&mut self, ui: &mut egui::Ui) {
        use crate::editor::{Align, ZOrder};
        group(ui, "Align", |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    for (l, icon, a) in [
                        ("Left", Icon::AlignLeft, Align::Left),
                        ("Center", Icon::AlignCenter, Align::HCenter),
                        ("Right", Icon::AlignRight, Align::Right),
                    ] {
                        if icons::small(ui, icon, l).clicked() {
                            self.ed.align(a);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    for (l, icon, a) in [
                        ("Top", Icon::AlignTop, Align::Top),
                        ("Middle", Icon::AlignMiddle, Align::VCenter),
                        ("Bottom", Icon::AlignBottom, Align::Bottom),
                    ] {
                        if icons::small(ui, icon, l).clicked() {
                            self.ed.align(a);
                        }
                    }
                });
            });
        });
        group(ui, "Order", |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if icons::small(ui, Icon::Front, "Bring to Front").clicked() {
                        self.ed.z_order(ZOrder::Front);
                    }
                    if icons::small(ui, Icon::Front, "Forward").clicked() {
                        self.ed.z_order(ZOrder::Forward);
                    }
                });
                ui.horizontal(|ui| {
                    if icons::small(ui, Icon::Back, "Send to Back").clicked() {
                        self.ed.z_order(ZOrder::Back);
                    }
                    if icons::small(ui, Icon::Back, "Backward").clicked() {
                        self.ed.z_order(ZOrder::Backward);
                    }
                });
            });
        });
        group(ui, "Size & Position", |ui| {
            let sel = (self.ed.selection.len() == 1)
                .then(|| self.ed.selection[0])
                .and_then(|id| self.ed.object(id).cloned());
            ui.add_enabled_ui(sel.is_some(), |ui| {
                let mut r = sel.as_ref().map(|o| o.rect).unwrap_or_default();
                let is_text = matches!(sel.as_ref().map(|o| &o.kind), Some(ObjectKind::Text(_)));
                egui::Grid::new("ribbon-geom")
                    .num_columns(4)
                    .spacing([4.0, 2.0])
                    .show(ui, |ui| {
                        ui.label("X");
                        ui.add(egui::DragValue::new(&mut r.x).speed(0.5).max_decimals(1));
                        ui.label("W");
                        ui.add(
                            egui::DragValue::new(&mut r.w)
                                .speed(0.5)
                                .range(1.0..=5000.0)
                                .max_decimals(1),
                        );
                        ui.end_row();
                        ui.label("Y");
                        ui.add(egui::DragValue::new(&mut r.y).speed(0.5).max_decimals(1));
                        ui.label("H");
                        ui.add_enabled(
                            !is_text,
                            egui::DragValue::new(&mut r.h)
                                .speed(0.5)
                                .range(1.0..=5000.0)
                                .max_decimals(1),
                        );
                        ui.end_row();
                    });
                if let Some(o) = sel
                    && r != o.rect
                {
                    let key = Some(crate::commands::MergeKey::Object(o.id, 1));
                    self.ed.set_object(o.id, o.kind.clone(), r.sanitized(), key);
                }
            });
        });
        group(ui, "Pane", |ui| {
            if icons::big_toggle(ui, Icon::Pane, "Format\nPane", self.show_format_pane).clicked() {
                self.show_format_pane = !self.show_format_pane;
            }
        });
    }

    fn tab_slideshow(&mut self, ui: &mut egui::Ui) {
        group(ui, "Start Slide Show", |ui| {
            if icons::big(ui, Icon::Present, "From\nBeginning").clicked() {
                self.start_presenting(Some(0));
            }
            if icons::big(ui, Icon::Present, "From Current\nSlide").clicked() {
                self.start_presenting(None);
            }
        });
        ui.label(
            RichText::new("In the show: → / Space / click = next, ← = previous, Esc = end")
                .small()
                .weak(),
        );
    }

    fn tab_view(&mut self, ui: &mut egui::Ui) {
        group(ui, "Zoom", |ui| {
            if icons::big(ui, Icon::ZoomOut, "Zoom\nOut").clicked() {
                self.zoom_by(0.8);
            }
            if icons::big(ui, Icon::ZoomIn, "Zoom\nIn").clicked() {
                self.zoom_by(1.25);
            }
            if icons::big(ui, Icon::Fit, "Fit to\nWindow").clicked() {
                self.ed.zoom = Zoom::Fit;
            }
            if icons::big(ui, Icon::Fit, "100%").clicked() {
                self.on_action(MenuAction::ActualSize);
            }
        });
        group(ui, "Show", |ui| {
            ui.vertical(|ui| {
                ui.checkbox(&mut self.show_format_pane, "Format Pane");
                ui.checkbox(&mut self.show_hud, "Performance HUD");
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            let n = self.ed.doc.slides.len();
            ui.label(
                RichText::new(format!(
                    "Slide {} of {n} · {} objects",
                    self.ed.current + 1,
                    self.ed.slide().objects.len()
                ))
                .small(),
            );
            ui.separator();
            if self.busy.saving {
                ui.spinner();
                ui.label(RichText::new("Saving…").small());
            }
            if self.busy.opening {
                ui.spinner();
                ui.label(RichText::new("Opening…").small());
            }
            if self.busy.importing > 0 {
                ui.spinner();
                ui.label(RichText::new(format!("Importing {} image(s)…", self.busy.importing)).small());
            }
            if self.math.busy() > 0 {
                ui.spinner();
                ui.label(RichText::new("Typesetting math…").small());
            }
            let mut cancel = false;
            if let Some((label, _)) = &self.busy.exporting {
                ui.spinner();
                ui.label(RichText::new(label.as_str()).small());
                cancel = icons::small(ui, Icon::Close, "Cancel").clicked();
            }
            if cancel && let Some((_, flag)) = &self.busy.exporting {
                flag.store(true, Ordering::Relaxed);
            }
            let now = Instant::now();
            self.toasts.retain(|t| t.until > now);
            if let Some(t) = self.toasts.last() {
                let c = if t.error {
                    Color32::from_rgb(210, 70, 50)
                } else {
                    ui.visuals().text_color()
                };
                ui.label(RichText::new(&t.text).small().color(c));
            }
            if let Some(next) = self.toasts.iter().map(|t| t.until).min() {
                ui.ctx().request_repaint_after(next.saturating_duration_since(now));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Zoom (PowerPoint keeps it bottom-right).
                if icons::small(ui, Icon::Fit, "Fit")
                    .on_hover_text("Fit slide to window (⌘0)")
                    .clicked()
                {
                    self.ed.zoom = Zoom::Fit;
                }
                let cur = self.canvas.last_view.map(|v| v.scale).unwrap_or(1.0);
                let mut pct = cur * 100.0;
                let slider = egui::Slider::new(&mut pct, 25.0..=400.0)
                    .logarithmic(true)
                    .show_value(false);
                if ui.add_sized([120.0, 16.0], slider).changed() {
                    let center = match self.ed.zoom {
                        Zoom::Manual { center, .. } => center,
                        Zoom::Fit => (SLIDE_W * 0.5, SLIDE_H * 0.5),
                    };
                    self.ed.zoom = Zoom::Manual {
                        scale: pct / 100.0,
                        center,
                    };
                }
                ui.label(RichText::new(format!("{:.0}%", cur * 100.0)).small());
                ui.separator();
                if self.ed.is_dirty() {
                    ui.label(RichText::new("Edited").small().weak());
                }
                if self.math.toolchain().is_none() {
                    ui.colored_label(Color32::from_rgb(210, 70, 50), RichText::new("No LaTeX engine").small())
                        .on_hover_text(crate::math::NO_ENGINE);
                }
            });
        });
    }

    fn hud(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self
            .rss
            .is_none_or(|(t, _)| now.duration_since(t) > Duration::from_secs(1))
        {
            self.rss = crate::platform::resident_bytes().map(|b| (now, b));
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        egui::Area::new(egui::Id::unique("perf-hud"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-300.0, 60.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new("Performance").strong());
                    ui.monospace(format!(
                        "frame cpu p50 {:.2} ms  p95 {:.2} ms  max {:.2} ms",
                        self.perf.percentile(50.0),
                        self.perf.percentile(95.0),
                        self.perf.max()
                    ));
                    ui.monospace(format!("frames {}", self.perf.frames));
                    ui.monospace(format!(
                        "textures {} ({:.1} MiB, peak {:.1}) decodes {} evictions {}",
                        self.textures.len(),
                        self.textures.bytes() as f64 / 1048576.0,
                        self.textures.stats.peak_bytes as f64 / 1048576.0,
                        self.textures.stats.decodes,
                        self.textures.stats.evictions
                    ));
                    ui.monospace(format!(
                        "equations typeset {}  undo {} ({:.1} KiB)",
                        self.math.compiled_total(),
                        self.ed.history.len(),
                        self.ed.history.bytes() as f64 / 1024.0
                    ));
                    if let Some((_, rss)) = self.rss {
                        ui.monospace(format!("RSS {:.1} MiB", rss as f64 / 1048576.0));
                    }
                });
            });
    }

    fn update_title(&mut self) {
        let title = format!(
            "{}{} — BeamerX",
            self.ed.display_name(),
            if self.ed.is_dirty() { " — Edited" } else { "" }
        );
        if title != self.window_title {
            self.ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if dropped.is_empty() {
            return;
        }
        let (projects, images): (Vec<PathBuf>, Vec<PathBuf>) = dropped
            .into_iter()
            .partition(|p| p.extension().and_then(|e| e.to_str()) == Some(persistence::EXTENSION));
        if !images.is_empty() {
            let at = self.canvas.hover_slide_pos.or(Some((SLIDE_W * 0.5, SLIDE_H * 0.5)));
            self.import_paths(images, at);
        }
        if let Some(p) = projects.into_iter().next()
            && self.confirm_discard(After::Open(Some(p.clone())))
        {
            self.open_path(Some(p), false);
        }
    }

    /// Events injected into egui's input on the next frame (menu-driven
    /// shortcuts in text fields, benchmark pointer input).
    pub(crate) fn inject_events(&mut self) -> &mut Vec<egui::Event> {
        &mut self.inject
    }

    pub fn is_idle(&self) -> bool {
        !self.busy.saving
            && !self.busy.autosaving
            && !self.busy.opening
            && self.busy.importing == 0
            && self.busy.exporting.is_none()
            && self.math.busy() == 0
            && self.textures.pending() == 0
    }
}

const RIBBON_ACCENT: Color32 = Color32::from_rgb(196, 62, 28);
const RIBBON_H: f32 = 74.0;

/// A labelled ribbon group with a separator after it.
fn group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    let r = ui
        .vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let row = ui.horizontal_top(|ui| {
                ui.set_min_height(58.0);
                add(ui);
            });
            // Title centred under the group's own width (not the panel's).
            let w = row.response.rect.width().max(40.0);
            ui.add_sized(
                [w, 14.0],
                egui::Label::new(RichText::new(title).size(10.5).weak()).halign(egui::Align::Center),
            );
        })
        .response
        .rect;
    ui.add_space(6.0);
    let x = ui.cursor().left();
    ui.painter()
        .vline(x, r.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
    ui.add_space(8.0);
}

fn panel_title(ui: &mut egui::Ui, icon: Icon, title: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
        icon,
        ui.visuals().text_color(),
    );
    ui.label(RichText::new(title).strong());
}

impl eframe::App for BeamerApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.events.append(&mut self.inject);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(cpu) = frame.info().cpu_usage {
            self.perf.record(cpu * 1000.0);
        }
        let now = Instant::now();

        // Background results and menu actions.
        self.pump_io();
        while let Ok(a) = self.menu_rx.try_recv() {
            if self.presenting.is_some() && a != MenuAction::Quit {
                continue;
            }
            self.on_action(a);
        }
        self.math.set_preamble(&self.ed.doc.meta.preamble);
        let (_, next) = self.math.pump(now);
        if let Some(d) = next {
            ctx.request_repaint_after(d);
        }
        self.textures.begin_frame();

        // Window close: ask about unsaved changes.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if let Some((_, flag)) = &self.busy.exporting {
                flag.store(true, Ordering::Relaxed);
            }
            if self.confirm_discard(After::Quit) {
                self.allow_close = true;
            } else {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
        }

        if self.presenting.is_some() {
            self.present_ui(ui);
            if let Some(mut b) = self.bench.take() {
                if b.tick(self, &ctx) {
                    self.bench = Some(b);
                }
            }
            return;
        }

        self.handle_drops(&ctx);

        let mut insert_image = false;
        egui::Panel::top("ribbon").show(ui, |ui| {
            ui.add_space(4.0);
            insert_image = self.ribbon(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        {
            let mut sv = Services {
                assets: &self.assets,
                textures: &mut self.textures,
                math: &mut self.math,
                now,
                text_heights: None,
            };
            let ed = &mut self.ed;
            egui::Panel::left("slides")
                .resizable(true)
                .default_size(190.0)
                .size_range(120.0..=360.0)
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.set_min_height(28.0);
                        panel_title(ui, Icon::Layout, "Slides");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::small(ui, Icon::NewSlide, "")
                                .on_hover_text("New slide (⇧⌘N)")
                                .clicked()
                            {
                                ed.new_slide();
                            }
                            if icons::small(ui, Icon::Section, "")
                                .on_hover_text("Add section before the current slide")
                                .clicked()
                            {
                                ed.add_section_before(ed.current);
                            }
                        });
                    });
                    ui.separator();
                    slides_panel::slides_ui(ui, ed, &mut sv);
                });
        }
        if self.show_format_pane {
            egui::Panel::right("format-pane")
                .resizable(true)
                .default_size(270.0)
                .size_range(220.0..=440.0)
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.set_min_height(28.0);
                        panel_title(ui, Icon::Pane, "Format");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::small(ui, Icon::Close, "")
                                .on_hover_text("Close format pane")
                                .clicked()
                            {
                                self.show_format_pane = false;
                            }
                        });
                    });
                    ui.separator();
                    inspector::format_pane_ui(ui, &mut self.ed, &self.assets);
                });
        }
        if self.show_props {
            let mut open = true;
            inspector::properties_window(&ctx, &mut open, &mut self.ed, &mut self.math);
            self.show_props = open;
        }
        let mut req = None;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut sv = Services {
                assets: &self.assets,
                textures: &mut self.textures,
                math: &mut self.math,
                now,
                text_heights: None,
            };
            req = canvas::canvas_ui(ui, &mut self.ed, &mut self.canvas, &mut sv);
            let hovering_files = ui.input(|i| !i.raw.hovered_files.is_empty());
            if hovering_files {
                let r = ui.max_rect();
                ui.painter().rect_stroke(
                    r.shrink(4.0),
                    6.0,
                    egui::Stroke::new(3.0, Color32::from_rgb(10, 132, 255)),
                    egui::StrokeKind::Inside,
                );
            }
        });
        if matches!(req, Some(CanvasRequest::ShowFormatPane)) {
            self.show_format_pane = true;
        }
        if insert_image || matches!(req, Some(CanvasRequest::InsertImage)) {
            self.insert_image_dialog();
        }
        if self.show_hud {
            self.hud(&ctx);
        }

        self.update_title();
        self.maybe_autosave(now);
        if let Some(m) = &self.menu {
            m.set_undo_redo(self.ed.history.can_undo(), self.ed.history.can_redo());
        }
        if let Some(mut b) = self.bench.take() {
            if b.tick(self, &ctx) {
                self.bench = Some(b);
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // A normal exit means the user decided about unsaved work.
        if self.bench.is_none() {
            self.discard_recovery();
        }
    }
}
