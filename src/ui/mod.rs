//! Look and feel: light/dark themes, the system UI font, shared colours,
//! drawn icons and the colour picker.

pub mod color_picker;
pub mod icons;

use egui::{Color32, CornerRadius, FontFamily, Stroke, Theme};
use std::sync::Arc;

/// Brand accent (File button, active tab, Present).
pub const ACCENT: Color32 = Color32::from_rgb(196, 62, 28);
/// Canvas selection colour.
pub const SELECT: Color32 = Color32::from_rgb(10, 132, 255);

/// Surface colours for one theme.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Window background behind everything (tab bar, status bar).
    pub chrome: Color32,
    /// Ribbon card and side panes.
    pub card: Color32,
    /// Workspace around the slide.
    pub workspace: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    /// Hover / pressed backgrounds of flat buttons.
    pub hover: Color32,
    pub pressed: Color32,
    /// Background of toggled ribbon buttons.
    pub on: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            chrome: Color32::from_rgb(32, 32, 34),
            card: Color32::from_rgb(43, 43, 46),
            workspace: Color32::from_rgb(24, 24, 26),
            border: Color32::from_rgb(64, 64, 68),
            text: Color32::from_rgb(232, 232, 235),
            weak: Color32::from_rgb(150, 150, 156),
            hover: Color32::from_rgb(60, 60, 64),
            pressed: Color32::from_rgb(72, 72, 78),
            on: Color32::from_rgb(88, 52, 40),
        }
    } else {
        Palette {
            chrome: Color32::from_rgb(243, 242, 241),
            card: Color32::from_rgb(255, 255, 255),
            workspace: Color32::from_rgb(228, 228, 230),
            border: Color32::from_rgb(222, 220, 218),
            text: Color32::from_rgb(37, 36, 35),
            weak: Color32::from_rgb(110, 108, 106),
            hover: Color32::from_rgb(237, 235, 233),
            pressed: Color32::from_rgb(225, 223, 221),
            on: Color32::from_rgb(252, 228, 218),
        }
    }
}

pub fn palette_of(ui: &egui::Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

/// Theme choice (View ▸ Theme), persisted in the user's preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    Light,
    Dark,
    System,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::System => "System",
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
            ThemeChoice::System => egui::ThemePreference::System,
        });
    }
}

/// User interface preferences, stored as JSON in the application-support
/// directory (`prefs.json`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    #[serde(default = "default_theme")]
    pub theme: ThemeChoice,
}

fn default_theme() -> ThemeChoice {
    ThemeChoice::Light
}

impl Default for Prefs {
    fn default() -> Self {
        Self { theme: default_theme() }
    }
}

impl Prefs {
    fn path() -> std::path::PathBuf {
        crate::platform::data_dir().join("prefs.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = crate::assets::write_atomic(&p, &bytes);
        }
    }
}

/// Use the macOS system font (San Francisco) for the interface when it is
/// available; it is read from the system, not bundled.
pub fn add_system_ui_font(defs: &mut egui::FontDefinitions) {
    #[cfg(target_os = "macos")]
    for path in [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            defs.font_data
                .insert("system-ui".into(), Arc::new(egui::FontData::from_owned(bytes)));
            if let Some(f) = defs.families.get_mut(&FontFamily::Proportional) {
                f.insert(0, "system-ui".into());
            }
            return;
        }
    }
    let _ = defs;
}

/// Style both themes (flat, Office-like light theme by default).
pub fn install_style(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        let pal = palette(theme == Theme::Dark);
        ctx.style_mut_of(theme, |s| {
            s.spacing.item_spacing = egui::vec2(6.0, 5.0);
            s.spacing.button_padding = egui::vec2(8.0, 4.0);
            s.spacing.interact_size.y = 22.0;
            s.spacing.menu_margin = egui::Margin::same(6);
            let v = &mut s.visuals;
            v.panel_fill = pal.chrome;
            v.window_fill = pal.card;
            v.window_stroke = Stroke::new(1.0, pal.border);
            v.window_corner_radius = CornerRadius::same(8);
            v.menu_corner_radius = CornerRadius::same(8);
            v.faint_bg_color = pal.hover;
            v.extreme_bg_color = if theme == Theme::Dark {
                Color32::from_rgb(28, 28, 30)
            } else {
                Color32::WHITE
            };
            v.selection.bg_fill = if theme == Theme::Dark {
                Color32::from_rgb(120, 60, 40)
            } else {
                pal.on
            };
            v.selection.stroke = Stroke::new(1.0, if theme == Theme::Dark { Color32::WHITE } else { ACCENT });
            v.hyperlink_color = ACCENT;
            for (w, fill) in [
                (&mut v.widgets.noninteractive, pal.card),
                (&mut v.widgets.inactive, pal.card),
                (&mut v.widgets.hovered, pal.hover),
                (&mut v.widgets.active, pal.pressed),
                (&mut v.widgets.open, pal.hover),
            ] {
                w.corner_radius = CornerRadius::same(5);
                w.weak_bg_fill = fill;
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.text);
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.border);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.text);
            v.widgets.inactive.bg_fill = if theme == Theme::Dark {
                Color32::from_rgb(52, 52, 56)
            } else {
                Color32::WHITE
            };
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.border);
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.text);
            v.widgets.active.fg_stroke = Stroke::new(1.0, pal.text);
        });
    }
}

/// Frame of a side pane: card surface with a border towards the workspace.
pub fn pane(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.card)
        .stroke(Stroke::new(1.0, pal.border))
        .inner_margin(egui::Margin::symmetric(8, 6))
}

/// Frame of the top band (tabs + ribbon) and the status bar.
pub fn band(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.chrome)
        .stroke(Stroke::new(1.0, pal.border))
        .inner_margin(egui::Margin::symmetric(8, 4))
}

/// A rounded card (ribbon, panes).
pub fn card(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.card)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 6))
}

/// Section heading inside panes and popups.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    let pal = palette_of(ui);
    ui.label(egui::RichText::new(text).size(11.0).strong().color(pal.weak));
}
