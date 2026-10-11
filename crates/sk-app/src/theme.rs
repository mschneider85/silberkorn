//! Look and feel: a calm, neutral dark theme, plus the Phosphor icon font.
//!
//! The greys are strictly neutral so they don't tint the judgement of colors; depth comes from
//! a few steps of lightness: chrome (darkest) < browser < viewer < panel < section < controls.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, vec2};

pub use egui_phosphor::regular as icon;

/// Title bar, toolbar, tool tab rail and status bar.
pub const CHROME: Color32 = Color32::from_rgb(24, 24, 26);
/// Tool panel background.
pub const PANEL: Color32 = Color32::from_rgb(36, 36, 39);
/// Tool section header strip.
pub const SECTION: Color32 = Color32::from_rgb(45, 45, 49);
/// Neutral background behind the image, dark enough to judge tones.
pub const VIEWER: Color32 = Color32::from_rgb(30, 30, 32);
/// Browser / filmstrip background.
pub const BROWSER: Color32 = Color32::from_rgb(27, 27, 29);
/// Fill of controls (buttons, text fields, slider rails).
pub const CONTROL: Color32 = Color32::from_rgb(54, 54, 59);
/// Fine separating lines.
pub const BORDER: Color32 = Color32::from_rgb(50, 50, 55);
pub const ACCENT: Color32 = Color32::from_rgb(78, 146, 230);
/// Accent for pressed toggles and selected rows: the accent, darkened towards the panel.
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(45, 70, 102);
pub const TEXT: Color32 = Color32::from_rgb(222, 222, 226);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 150, 158);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(108, 108, 116);
pub const DANGER: Color32 = Color32::from_rgb(230, 110, 100);
pub const STAR: Color32 = Color32::from_rgb(240, 196, 70);

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // Icons also appear in monospace text (key names in the shortcut overview).
    fonts.families.entry(FontFamily::Monospace).or_default().push("phosphor".into());
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(40, 40, 44);
    v.extreme_bg_color = Color32::from_rgb(26, 26, 28);
    v.faint_bg_color = Color32::from_rgb(42, 42, 46);
    v.code_bg_color = Color32::from_rgb(26, 26, 28);
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(62, 62, 68));
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 10], blur: 32, spread: 0, color: Color32::from_black_alpha(140) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(120) };
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.indent_has_left_vline = false;
    v.striped = false;
    v.text_cursor.stroke = Stroke::new(1.5, ACCENT);

    let r = CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;
    for (state, bg, fg) in [
        (&mut w.inactive, CONTROL, Color32::from_rgb(210, 210, 215)),
        (&mut w.hovered, Color32::from_rgb(66, 66, 72), Color32::WHITE),
        (&mut w.active, Color32::from_rgb(78, 78, 85), Color32::WHITE),
        (&mut w.open, Color32::from_rgb(62, 62, 68), Color32::WHITE),
    ] {
        state.bg_fill = bg;
        state.weak_bg_fill = bg;
        state.fg_stroke = Stroke::new(1.0, fg);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(88, 88, 96));
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(88, 88, 96));
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(16.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
        let sp = &mut s.spacing;
        sp.item_spacing = vec2(6.0, 5.0);
        sp.button_padding = vec2(9.0, 4.0);
        sp.interact_size = vec2(36.0, 24.0);
        sp.slider_width = 150.0;
        sp.slider_rail_height = 3.0;
        sp.menu_margin = Margin::same(6);
        sp.menu_width = 270.0;
        sp.window_margin = Margin::same(16);
        sp.indent = 14.0;
        sp.combo_height = 300.0;
        s.interaction.tooltip_delay = 0.35;
    });
}

/// Frame for the dark chrome bars (menu, toolbar, rail, status bar).
pub fn chrome_frame(margin: Margin) -> egui::Frame {
    egui::Frame::new().fill(CHROME).inner_margin(margin)
}
