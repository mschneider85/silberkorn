//! Look and feel: a calm, neutral dark theme, plus the Phosphor icon font.
//!
//! The greys are strictly neutral so they don't tint the judgement of colors; depth comes from
//! a few steps of lightness: chrome (darkest) < browser < viewer < panel < controls.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, vec2};

pub use egui_phosphor::regular as icon;

/// Title bar, toolbar, tool tab rail and status bar.
pub const CHROME: Color32 = Color32::from_rgb(24, 24, 26);
/// Tool panel background.
pub const PANEL: Color32 = Color32::from_rgb(36, 36, 39);
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
/// Mark of the AI functions (a sparkle after their name, see `widgets::ai_text`).
pub const AI: Color32 = Color32::from_rgb(246, 200, 50);
/// Glow around highlighted symbols and the wordmark: the accent, a little brighter.
pub const GLOW: Color32 = Color32::from_rgb(110, 178, 255);

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // Icons also appear in monospace text (key names in the shortcut overview).
    fonts.families.entry(FontFamily::Monospace).or_default().push("phosphor".into());
    ctx.set_fonts(fonts);

    // Always dark: otherwise egui follows a light system theme and ignores the visuals below.
    ctx.set_theme(egui::Theme::Dark);
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
        // Labels are not text to select: no I-beam over names, values and clickable labels
        // (rating stars). Text fields stay editable.
        s.interaction.selectable_labels = false;
    });
}

/// Frame for the dark chrome bars (menu, toolbar, rail, status bar).
pub fn chrome_frame(margin: Margin) -> egui::Frame {
    egui::Frame::new().fill(CHROME).inner_margin(margin)
}

/// Soft glow around text or a symbol, in the style of a neon sign: a blurred light behind it
/// (`strength` 0..1 scales it) and rings of transparent copies that hug the glyphs. The text
/// itself is drawn on top by the caller.
pub fn paint_glow(painter: &egui::Painter, pos: egui::Pos2, galley: &std::sync::Arc<egui::Galley>, radius: f32, strength: f32) {
    let rect = egui::Rect::from_min_size(pos, galley.size());
    let blob = Shadow { offset: [0, 0], blur: (radius * 3.0) as u8, spread: 0, color: GLOW.gamma_multiply(0.3 * strength) };
    painter.add(blob.as_shape(rect.shrink2(rect.size() * 0.2), CornerRadius::same((rect.height() / 2.0) as u8)));
    for (r, alpha) in [(radius, 0.06), (radius * 0.6, 0.1), (radius * 0.3, 0.15)] {
        for i in 0..12 {
            let a = i as f32 / 12.0 * std::f32::consts::TAU;
            painter.galley_with_override_text_color(pos + vec2(a.cos(), a.sin()) * r, galley.clone(), GLOW.gamma_multiply(alpha * strength));
        }
    }
}
