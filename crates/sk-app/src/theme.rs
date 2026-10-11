//! Look and feel: a neutral dark theme, plus the Phosphor icon font.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, vec2};

pub use egui_phosphor::regular as icon;

/// Menu bar, toolbar, tool tab rail and status bar.
pub const CHROME: Color32 = Color32::from_rgb(30, 30, 32);
/// Tool panel background.
pub const PANEL: Color32 = Color32::from_rgb(40, 40, 42);
/// Tool section header strip.
pub const SECTION: Color32 = Color32::from_rgb(52, 52, 55);
/// Neutral background behind the image, dark enough to judge tones.
pub const VIEWER: Color32 = Color32::from_rgb(33, 33, 34);
/// Browser / filmstrip background.
pub const BROWSER: Color32 = Color32::from_rgb(26, 26, 27);
pub const ACCENT: Color32 = Color32::from_rgb(64, 140, 220);
pub const TEXT: Color32 = Color32::from_rgb(218, 218, 220);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 150, 154);

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // Icons also appear in monospace text (key names in the shortcut overview).
    fonts.families.entry(FontFamily::Monospace).or_default().push("phosphor".into());
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(44, 44, 47);
    v.extreme_bg_color = Color32::from_rgb(24, 24, 25);
    v.faint_bg_color = Color32::from_rgb(46, 46, 49);
    v.code_bg_color = Color32::from_rgb(24, 24, 25);
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(64, 64, 68));
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(4);
    v.window_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) };
    v.popup_shadow = Shadow { offset: [0, 3], blur: 10, spread: 0, color: Color32::from_black_alpha(100) };
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.slider_trailing_fill = false;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.indent_has_left_vline = false;

    let r = CornerRadius::same(3);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(58, 58, 62));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;
    for (state, bg, fg) in [
        (&mut w.inactive, Color32::from_rgb(58, 58, 62), Color32::from_rgb(205, 205, 208)),
        (&mut w.hovered, Color32::from_rgb(72, 72, 77), Color32::WHITE),
        (&mut w.active, Color32::from_rgb(84, 84, 90), Color32::WHITE),
        (&mut w.open, Color32::from_rgb(64, 64, 69), Color32::WHITE),
    ] {
        state.bg_fill = bg;
        state.weak_bg_fill = bg;
        state.fg_stroke = Stroke::new(1.0, fg);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(96, 96, 102));
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(96, 96, 102));
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
        sp.button_padding = vec2(7.0, 3.0);
        sp.interact_size = vec2(36.0, 22.0);
        sp.slider_width = 150.0;
        sp.slider_rail_height = 4.0;
        sp.menu_margin = Margin::same(6);
        sp.menu_width = 260.0;
        sp.window_margin = Margin::same(12);
        sp.indent = 14.0;
    });
}

/// Frame for the dark chrome bars (menu, toolbar, rail, status bar).
pub fn chrome_frame(margin: Margin) -> egui::Frame {
    egui::Frame::new().fill(CHROME).inner_margin(margin)
}
