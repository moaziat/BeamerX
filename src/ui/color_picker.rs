//! Colour picker: a swatch (or icon) button opening a popup with theme
//! colours (tints and shades), standard colours, suggestions derived from the
//! current colour (lighter/darker, complementary, analogous, triadic),
//! recent colours, and a colour wheel with brightness and hex input.

use super::icons::{self, Icon};
use super::{heading, palette_of};
use crate::document::Color;
use egui::epaint::Mesh;
use egui::{Color32, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::f32::consts::TAU;

/// Result of a pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Color(Color),
    /// "No colour" (e.g. no fill).
    None,
}

pub fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

// ---- colour maths (sRGB) ----------------------------------------------------

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::rgb(to(r), to(g), to(b))
}

pub fn rgb_to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::rgba(f(a.r, b.r), f(a.g, b.g), f(a.b, b.b), a.a)
}

/// Mix towards white by `t` (0..1).
pub fn tint(c: Color, t: f32) -> Color {
    mix(c, Color::WHITE, t)
}

/// Mix towards black by `t` (0..1).
pub fn shade(c: Color, t: f32) -> Color {
    mix(c, Color::BLACK, t)
}

fn luminance(c: Color) -> f32 {
    (0.299 * f32::from(c.r) + 0.587 * f32::from(c.g) + 0.114 * f32::from(c.b)) / 255.0
}

/// Theme colours (first row) used for presentations.
pub const THEME: [Color; 10] = [
    Color::rgb(255, 255, 255),
    Color::rgb(0, 0, 0),
    Color::rgb(68, 84, 106),
    Color::rgb(231, 230, 230),
    Color::rgb(31, 95, 168),
    Color::rgb(224, 122, 31),
    Color::rgb(58, 154, 77),
    Color::rgb(192, 57, 43),
    Color::rgb(123, 79, 160),
    Color::rgb(30, 158, 154),
];

/// Office's standard colours.
pub const STANDARD: [Color; 10] = [
    Color::rgb(192, 0, 0),
    Color::rgb(255, 0, 0),
    Color::rgb(255, 192, 0),
    Color::rgb(255, 255, 0),
    Color::rgb(146, 208, 80),
    Color::rgb(0, 176, 80),
    Color::rgb(0, 176, 240),
    Color::rgb(0, 112, 192),
    Color::rgb(0, 32, 96),
    Color::rgb(112, 48, 160),
];

/// Five variations of a theme colour (rows under the theme row), like
/// Office: light colours get darker shades, dark colours lighter tints,
/// others three tints and two shades.
pub fn variations(c: Color) -> [Color; 5] {
    let l = luminance(c);
    if l > 0.85 {
        [
            shade(c, 0.05),
            shade(c, 0.15),
            shade(c, 0.25),
            shade(c, 0.35),
            shade(c, 0.5),
        ]
    } else if l < 0.15 {
        [tint(c, 0.5), tint(c, 0.35), tint(c, 0.25), tint(c, 0.15), tint(c, 0.05)]
    } else {
        [tint(c, 0.8), tint(c, 0.6), tint(c, 0.4), shade(c, 0.25), shade(c, 0.5)]
    }
}

/// Colour suggestions derived from `c`: lighter, darker, complementary,
/// two analogous, two triadic and a muted variant.
pub fn suggestions(c: Color) -> Vec<(Color, &'static str)> {
    let (h, s, v) = rgb_to_hsv(c);
    // Greys have no hue to rotate; suggest the accent palette instead.
    let (h, s, v) = if s < 0.08 {
        (rgb_to_hsv(THEME[4]).0, 0.7, 0.75)
    } else {
        (h, s, v.max(0.35))
    };
    vec![
        (tint(c, 0.45), "Lighter"),
        (shade(c, 0.35), "Darker"),
        (hsv_to_rgb(h + 0.5, s, v), "Complementary"),
        (hsv_to_rgb(h - 1.0 / 12.0, s, v), "Analogous"),
        (hsv_to_rgb(h + 1.0 / 12.0, s, v), "Analogous"),
        (hsv_to_rgb(h + 1.0 / 3.0, s, v), "Triadic"),
        (hsv_to_rgb(h - 1.0 / 3.0, s, v), "Triadic"),
        (hsv_to_rgb(h + 5.0 / 12.0, s * 0.8, v), "Split-complementary"),
        (hsv_to_rgb(h, s * 0.35, v), "Muted"),
    ]
}

// ---- state -------------------------------------------------------------------

#[derive(Clone, Default)]
struct WheelState {
    open: bool,
    hsv: (f32, f32, f32),
    hex: String,
    /// The colour when the popup opened (for the before/after preview).
    original: Option<Color>,
    synced_from: Option<Color>,
}

fn recent_id() -> Id {
    Id::unique("bx-recent-colors")
}

/// Remember a picked colour in the "Recent" row (most recent first).
pub fn remember(ctx: &egui::Context, c: Color) {
    ctx.data_mut(|d| {
        let v: &mut Vec<Color> = d.get_temp_mut_or_default(recent_id());
        v.retain(|x| *x != c);
        v.insert(0, c);
        v.truncate(10);
    });
}

// ---- widgets -----------------------------------------------------------------

fn swatch(ui: &mut Ui, c: Color, selected: bool, size: f32, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    if ui.is_rect_visible(r) {
        let p = ui.painter();
        p.rect_filled(r, 2.0, c32(c));
        let pal = palette_of(ui);
        let border = if resp.hovered() || selected {
            super::ACCENT
        } else {
            pal.border
        };
        p.rect_stroke(
            r,
            2.0,
            Stroke::new(if resp.hovered() || selected { 2.0 } else { 1.0 }, border),
            StrokeKind::Inside,
        );
    }
    let hex = c.to_hex().to_uppercase();
    if tip.is_empty() {
        resp.on_hover_text(hex)
    } else {
        resp.on_hover_text(format!("{tip} {hex}"))
    }
}

fn row(ui: &mut Ui, colors: &[Color], current: Option<Color>, out: &mut Option<Color>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for c in colors {
            if swatch(ui, *c, current == Some(*c), 19.0, "").clicked() {
                *out = Some(*c);
            }
        }
    });
}

/// Hue/saturation wheel at brightness `v`. Returns true while being changed.
fn wheel(ui: &mut Ui, hsv: &mut (f32, f32, f32), size: f32) -> (bool, bool) {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let center = r.center();
    let radius = size * 0.5 - 2.0;
    if ui.is_rect_visible(r) {
        let (rings, sectors) = (14u32, 72u32);
        let mut mesh = Mesh::default();
        let col = |h: f32, s: f32| c32(hsv_to_rgb(h, s, hsv.2));
        mesh.colored_vertex(center, col(0.0, 0.0));
        for i in 1..=rings {
            let rr = radius * i as f32 / rings as f32;
            for j in 0..sectors {
                let a = TAU * j as f32 / sectors as f32;
                mesh.colored_vertex(
                    center + vec2(a.cos(), -a.sin()) * rr,
                    col(j as f32 / sectors as f32, i as f32 / rings as f32),
                );
            }
        }
        let idx = |i: u32, j: u32| 1 + (i - 1) * sectors + (j % sectors);
        for j in 0..sectors {
            mesh.add_triangle(0, idx(1, j), idx(1, j + 1));
        }
        for i in 2..=rings {
            for j in 0..sectors {
                let (a, b, c, d) = (idx(i - 1, j), idx(i - 1, j + 1), idx(i, j), idx(i, j + 1));
                mesh.add_triangle(a, c, d);
                mesh.add_triangle(a, d, b);
            }
        }
        ui.painter().add(mesh);
        ui.painter()
            .circle_stroke(center, radius, Stroke::new(1.0, palette_of(ui).border));
        let a = hsv.0 * TAU;
        let m = center + vec2(a.cos(), -a.sin()) * radius * hsv.1;
        ui.painter().circle_stroke(m, 5.0, Stroke::new(2.5, Color32::WHITE));
        ui.painter().circle_stroke(m, 5.0, Stroke::new(1.0, Color32::BLACK));
    }
    let mut changed = false;
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked() || resp.drag_started())
    {
        let d = p - center;
        hsv.1 = (d.length() / radius).min(1.0);
        hsv.0 = (-d.y).atan2(d.x).rem_euclid(TAU) / TAU;
        changed = true;
    }
    (changed, resp.drag_stopped() || resp.clicked())
}

/// Brightness bar for the current hue/saturation.
fn value_bar(ui: &mut Ui, hsv: &mut (f32, f32, f32), w: f32) -> (bool, bool) {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 16.0), Sense::click_and_drag());
    if ui.is_rect_visible(r) {
        let mut mesh = Mesh::default();
        let steps = 16;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let c = c32(hsv_to_rgb(hsv.0, hsv.1, t));
            let x = r.left() + r.width() * t;
            mesh.colored_vertex(pos2(x, r.top()), c);
            mesh.colored_vertex(pos2(x, r.bottom()), c);
            if k > 0 {
                let i = 2 * k;
                mesh.add_triangle(i - 2, i - 1, i);
                mesh.add_triangle(i - 1, i + 1, i);
            }
        }
        ui.painter().add(mesh);
        ui.painter()
            .rect_stroke(r, 3.0, Stroke::new(1.0, palette_of(ui).border), StrokeKind::Inside);
        let x = r.left() + r.width() * hsv.2;
        let m = Rect::from_center_size(pos2(x, r.center().y), vec2(5.0, r.height() + 4.0));
        ui.painter().rect_filled(m, 2.0, Color32::WHITE);
        ui.painter()
            .rect_stroke(m, 2.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
    }
    let mut changed = false;
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked() || resp.drag_started())
    {
        hsv.2 = ((p.x - r.left()) / r.width()).clamp(0.0, 1.0);
        changed = true;
    }
    (changed, resp.drag_stopped() || resp.clicked())
}

/// The popup body. Returns a choice when the user picks something.
fn popup_body(ui: &mut Ui, id: Id, current: Option<Color>, allow_none: bool) -> Option<Choice> {
    let mut picked: Option<Color> = None;
    let mut chosen: Option<Choice> = None;
    let mut st: WheelState = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if st.original.is_none() {
        st.original = current;
    }
    if st.synced_from != current
        && let Some(c) = current
    {
        // Follow external changes (but keep the hue of greys while dragging).
        let (h, s, v) = rgb_to_hsv(c);
        st.hsv = if s == 0.0 { (st.hsv.0, 0.0, v) } else { (h, s, v) };
        st.hex = c.to_hex().to_uppercase();
        st.synced_from = current;
    }

    ui.set_width(10.0 * 22.0);
    heading(ui, "Theme Colors");
    row(ui, &THEME, current, &mut picked);
    ui.add_space(2.0);
    for k in 0..5 {
        let colors: Vec<Color> = THEME.iter().map(|c| variations(*c)[k]).collect();
        row(ui, &colors, current, &mut picked);
    }
    ui.add_space(4.0);
    heading(ui, "Standard Colors");
    row(ui, &STANDARD, current, &mut picked);
    if let Some(c) = current {
        ui.add_space(4.0);
        heading(ui, "Suggestions for this color");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (s, why) in suggestions(c) {
                if swatch(ui, s, false, 19.0, why).clicked() {
                    picked = Some(s);
                }
            }
        });
    }
    let recent: Vec<Color> = ui.data(|d| d.get_temp(recent_id())).unwrap_or_default();
    if !recent.is_empty() {
        ui.add_space(4.0);
        heading(ui, "Recent Colors");
        row(ui, &recent, current, &mut picked);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if allow_none && icons::small(ui, Icon::Delete, "No Color").clicked() {
            chosen = Some(Choice::None);
        }
        let label = if st.open { "Hide Wheel" } else { "More Colors…" };
        if icons::small_toggle(ui, Icon::Properties, label, st.open).clicked() {
            st.open = !st.open;
        }
    });

    if st.open {
        ui.add_space(4.0);
        ui.separator();
        heading(ui, "Color Wheel");
        let (c1, done1) = wheel(ui, &mut st.hsv, 10.0 * 22.0);
        ui.add_space(4.0);
        let (c2, done2) = value_bar(ui, &mut st.hsv, 10.0 * 22.0);
        let wheel_color = hsv_to_rgb(st.hsv.0, st.hsv.1, st.hsv.2);
        if c1 || c2 {
            st.hex = wheel_color.to_hex().to_uppercase();
            st.synced_from = Some(wheel_color);
            chosen = Some(Choice::Color(wheel_color));
        }
        if done1 || done2 {
            remember(ui.ctx(), wheel_color);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Hex");
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.hex)
                    .desired_width(72.0)
                    .font(egui::TextStyle::Monospace),
            );
            if r.changed()
                && let Some(c) = Color::parse_hex(st.hex.trim())
            {
                let (h, s, v) = rgb_to_hsv(c);
                st.hsv = (h, s, v);
                st.synced_from = Some(c);
                chosen = Some(Choice::Color(c));
            }
            if r.lost_focus()
                && let Some(c) = Color::parse_hex(st.hex.trim())
            {
                remember(ui.ctx(), c);
            }
            // Before / after preview.
            ui.add_space(8.0);
            let (pr, _) = ui.allocate_exact_size(vec2(64.0, 20.0), Sense::hover());
            let half = pr.width() / 2.0;
            if let Some(o) = st.original {
                ui.painter()
                    .rect_filled(Rect::from_min_size(pr.min, vec2(half, pr.height())), 0.0, c32(o));
            }
            ui.painter().rect_filled(
                Rect::from_min_size(pr.min + vec2(half, 0.0), vec2(half, pr.height())),
                0.0,
                c32(wheel_color),
            );
            ui.painter()
                .rect_stroke(pr, 2.0, Stroke::new(1.0, palette_of(ui).border), StrokeKind::Inside);
        })
        .response
        .on_hover_text("Left: before · Right: new");
    }

    if let Some(c) = picked {
        remember(ui.ctx(), c);
        chosen = Some(Choice::Color(c));
    }
    ui.data_mut(|d| d.insert_temp(id, st));
    chosen
}

/// A colour button. `icon` shows an icon with a colour bar under it (like
/// Office's font colour button); otherwise a plain swatch is shown.
pub fn color_button(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    current: Option<Color>,
    allow_none: bool,
    icon: Option<Icon>,
    tooltip: &str,
) -> Option<Choice> {
    let id = ui.scope_id().with(("color-picker", salt));
    let resp = match icon {
        Some(icon) => {
            let (r, resp) = ui.allocate_exact_size(vec2(34.0, 24.0), Sense::click());
            if ui.is_rect_visible(r) {
                let pal = palette_of(ui);
                if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, pal.hover);
                }
                let fg = if ui.is_enabled() { pal.text } else { pal.weak };
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(pos2(r.left() + 13.0, r.center().y - 2.0), vec2(15.0, 15.0)),
                    icon,
                    fg,
                );
                let bar = Rect::from_center_size(pos2(r.left() + 13.0, r.bottom() - 4.0), vec2(16.0, 3.5));
                match current {
                    Some(c) => {
                        ui.painter().rect_filled(bar, 1.0, c32(c));
                    }
                    None => {
                        ui.painter()
                            .rect_stroke(bar, 1.0, Stroke::new(1.0, pal.weak), StrokeKind::Inside);
                    }
                }
                // Drop-down chevron.
                let x = r.right() - 7.0;
                let y = r.center().y;
                ui.painter()
                    .line_segment([pos2(x - 3.0, y - 1.5), pos2(x, y + 1.5)], Stroke::new(1.2, fg));
                ui.painter()
                    .line_segment([pos2(x, y + 1.5), pos2(x + 3.0, y - 1.5)], Stroke::new(1.2, fg));
            }
            resp
        }
        None => {
            let (r, resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
            if ui.is_rect_visible(r) {
                let pal = palette_of(ui);
                let sw = r.shrink(3.0);
                match current {
                    Some(c) => {
                        ui.painter().rect_filled(sw, 3.0, c32(c));
                    }
                    None => {
                        ui.painter().rect_filled(sw, 3.0, Color32::WHITE);
                        ui.painter().line_segment(
                            [sw.left_bottom(), sw.right_top()],
                            Stroke::new(1.5, Color32::from_rgb(200, 40, 40)),
                        );
                    }
                }
                let border = if resp.hovered() { super::ACCENT } else { pal.border };
                ui.painter()
                    .rect_stroke(r, 4.0, Stroke::new(1.0, border), StrokeKind::Inside);
            }
            resp
        }
    };
    let resp = if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    };
    let mut out = None;
    egui::Popup::from_toggle_button_response(&resp)
        .id(id.with("popup"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            out = popup_body(ui, id, current, allow_none);
        });
    if !egui::Popup::is_id_open(ui.ctx(), id.with("popup")) {
        // Reset the before/after reference for the next opening.
        ui.data_mut(|d| {
            let st: &mut WheelState = d.get_temp_mut_or_default(id);
            st.original = None;
        });
    }
    out
}

/// Convenience: a swatch button editing `c` in place. Returns true if changed.
pub fn edit(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, c: &mut Color) -> bool {
    match color_button(ui, salt, Some(*c), false, None, "") {
        Some(Choice::Color(n)) if n != *c => {
            *c = n;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrip() {
        for c in [
            Color::rgb(255, 0, 0),
            Color::rgb(31, 95, 168),
            Color::rgb(10, 200, 30),
            Color::rgb(128, 128, 128),
        ] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            assert!((i16::from(back.r) - i16::from(c.r)).abs() <= 1, "{c:?} -> {back:?}");
            assert!((i16::from(back.g) - i16::from(c.g)).abs() <= 1);
            assert!((i16::from(back.b) - i16::from(c.b)).abs() <= 1);
        }
    }

    #[test]
    fn suggestions_are_distinct_and_complement_is_opposite() {
        let red = Color::rgb(200, 30, 30);
        let s = suggestions(red);
        let comp = s.iter().find(|(_, w)| *w == "Complementary").unwrap().0;
        let (h, _, _) = rgb_to_hsv(comp);
        assert!((h - 0.5).abs() < 0.03, "complement hue {h}");
        assert!(s.iter().all(|(c, _)| *c != red));
        // Light colours get darker variations, dark colours lighter ones.
        assert!(luminance(variations(Color::WHITE)[4]) < 0.9);
        assert!(luminance(variations(Color::BLACK)[0]) > 0.3);
    }
}
