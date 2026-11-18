//! Native macOS menu bar (via `muda`).
//!
//! Menu selections are delivered as [`MenuAction`]s over a channel and wake
//! the UI with `request_repaint`, so there is no polling. Clipboard items are
//! translated by the app into egui viewport commands, which keeps native
//! ⌘C/⌘V working inside text fields as well as on the canvas.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuAction {
    About,
    Quit,
    New,
    Open,
    Save,
    SaveAs,
    ExportBeamer,
    ExportPdf,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    InsertText,
    InsertMath,
    InsertImage,
    InsertRectangle,
    InsertEllipse,
    InsertLine,
    InsertArrow,
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    NewSlide,
    DuplicateSlide,
    DeleteSlide,
    MoveSlideUp,
    MoveSlideDown,
    Present,
    ZoomIn,
    ZoomOut,
    ZoomToFit,
    ActualSize,
    TogglePerfHud,
}

#[cfg(target_os = "macos")]
pub struct NativeMenu {
    _menu: muda::Menu,
    undo: muda::MenuItem,
    redo: muda::MenuItem,
}

#[cfg(not(target_os = "macos"))]
pub struct NativeMenu;

#[cfg(target_os = "macos")]
impl NativeMenu {
    /// Build and install the menu bar. Must run on the main thread after the
    /// application has finished launching (i.e. inside the eframe creator).
    pub fn install(ctx: egui::Context, tx: Sender<MenuAction>) -> Option<Self> {
        use MenuAction as A;
        use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu, accelerator::Accelerator};

        let mut ids: HashMap<muda::MenuId, MenuAction> = HashMap::new();
        let mut item = |label: &str, action: MenuAction, accel: Option<&str>| -> MenuItem {
            let id = muda::MenuId::new(format!("{action:?}"));
            ids.insert(id.clone(), action);
            let accel = accel.and_then(|a| a.parse::<Accelerator>().ok());
            MenuItem::with_id(id, label, true, accel)
        };
        let sep = PredefinedMenuItem::separator;

        let about = item("About BeamerX", A::About, None);
        let quit = item("Quit BeamerX", A::Quit, Some("Cmd+Q"));
        let app_menu = Submenu::with_items(
            "BeamerX",
            true,
            &[
                &about,
                &sep(),
                &PredefinedMenuItem::services(None),
                &sep(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &sep(),
                &quit,
            ],
        )
        .ok()?;

        let file_menu = Submenu::with_items(
            "File",
            true,
            &[
                &item("New", A::New, Some("Cmd+N")),
                &item("Open…", A::Open, Some("Cmd+O")),
                &sep(),
                &item("Save", A::Save, Some("Cmd+S")),
                &item("Save As…", A::SaveAs, Some("Cmd+Shift+S")),
                &sep(),
                &item("Export Beamer Folder…", A::ExportBeamer, Some("Cmd+E")),
                &item("Export PDF…", A::ExportPdf, Some("Cmd+Shift+E")),
            ],
        )
        .ok()?;

        let undo = item("Undo", A::Undo, Some("Cmd+Z"));
        let redo = item("Redo", A::Redo, Some("Cmd+Shift+Z"));
        let edit_menu = Submenu::with_items(
            "Edit",
            true,
            &[
                &undo,
                &redo,
                &sep(),
                &item("Cut", A::Cut, Some("Cmd+X")),
                &item("Copy", A::Copy, Some("Cmd+C")),
                &item("Paste", A::Paste, Some("Cmd+V")),
                &item("Duplicate", A::Duplicate, Some("Cmd+D")),
                &item("Delete", A::Delete, None),
                &sep(),
                &item("Select All", A::SelectAll, Some("Cmd+A")),
            ],
        )
        .ok()?;

        let insert_menu = Submenu::with_items(
            "Insert",
            true,
            &[
                &item("Text", A::InsertText, Some("Cmd+Alt+T")),
                &item("Math ($…$)", A::InsertMath, Some("Cmd+Alt+E")),
                &item("Image…", A::InsertImage, Some("Cmd+Alt+I")),
                &sep(),
                &item("Rectangle", A::InsertRectangle, None),
                &item("Ellipse", A::InsertEllipse, None),
                &item("Line", A::InsertLine, None),
                &item("Arrow", A::InsertArrow, None),
            ],
        )
        .ok()?;

        let arrange_menu = Submenu::with_items(
            "Arrange",
            true,
            &[
                &item("Bring to Front", A::BringToFront, Some("Cmd+Shift+]")),
                &item("Bring Forward", A::BringForward, Some("Cmd+]")),
                &item("Send Backward", A::SendBackward, Some("Cmd+[")),
                &item("Send to Back", A::SendToBack, Some("Cmd+Shift+[")),
            ],
        )
        .ok()?;

        let slide_menu = Submenu::with_items(
            "Slide",
            true,
            &[
                &item("New Slide", A::NewSlide, Some("Cmd+Shift+N")),
                &item("Duplicate Slide", A::DuplicateSlide, None),
                &item("Delete Slide", A::DeleteSlide, None),
                &sep(),
                &item("Move Slide Up", A::MoveSlideUp, None),
                &item("Move Slide Down", A::MoveSlideDown, None),
            ],
        )
        .ok()?;

        let view_menu = Submenu::with_items(
            "View",
            true,
            &[
                &item("Present", A::Present, Some("Cmd+Enter")),
                &sep(),
                &item("Zoom In", A::ZoomIn, Some("Cmd+=")),
                &item("Zoom Out", A::ZoomOut, Some("Cmd+-")),
                &item("Zoom to Fit", A::ZoomToFit, Some("Cmd+0")),
                &item("Actual Size", A::ActualSize, Some("Cmd+1")),
                &sep(),
                &item("Performance HUD", A::TogglePerfHud, Some("Cmd+Alt+P")),
            ],
        )
        .ok()?;

        let window_menu = Submenu::with_items(
            "Window",
            true,
            &[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::fullscreen(None),
                &sep(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ],
        )
        .ok()?;

        let menu = Menu::with_items(&[
            &app_menu,
            &file_menu,
            &edit_menu,
            &insert_menu,
            &arrange_menu,
            &slide_menu,
            &view_menu,
            &window_menu,
        ])
        .ok()?;
        menu.init_for_nsapp();
        window_menu.set_as_windows_menu_for_nsapp();

        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            if let Some(action) = ids.get(event.id()) {
                let _ = tx.send(*action);
                ctx.request_repaint();
            }
        }));

        Some(Self {
            _menu: menu,
            undo,
            redo,
        })
    }

    pub fn set_undo_redo(&self, can_undo: bool, can_redo: bool) {
        self.undo.set_enabled(can_undo);
        self.redo.set_enabled(can_redo);
    }
}

#[cfg(not(target_os = "macos"))]
impl NativeMenu {
    pub fn install(_ctx: egui::Context, _tx: Sender<MenuAction>) -> Option<Self> {
        None
    }
    pub fn set_undo_redo(&self, _can_undo: bool, _can_redo: bool) {}
}
