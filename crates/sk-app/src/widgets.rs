//! Custom widgets: histogram, curve editor, star rating, slider rows.

use sk_core::params::{ColorWheel, Curve, LevelSet};
use sk_core::pipeline::Histogram;
use egui::{Color32, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::theme;

fn reveal_id() -> egui::Id {
    egui::Id::new("reveal_section")
}

/// Asks the tool section with this title to open, scroll into view and flash briefly the next
/// time it is drawn (used by the command palette).
pub fn reveal_section(ctx: &egui::Context, title: &str) {
    ctx.data_mut(|d| d.insert_temp(reveal_id(), title.to_string()));
}

/// Collapsible tool section with an optional reset button.
pub fn section(ui: &mut Ui, title: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_full(ui, title, true, None, reset, body);
}

/// [`section`] with an explanation behind an info icon in the header instead of text in the panel.
pub fn section_help(ui: &mut Ui, title: &str, help: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_full(ui, title, true, Some(help), reset, body);
}

/// [`section_help`] of an AI function: marked with the yellow sparkle after the title.
pub fn section_help_ai(ui: &mut Ui, title: &str, help: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_inner(ui, title, true, Some(help), None, true, None, reset, body);
}

/// Hover text of the AI mark.
pub const AI_HINT: &str = "KI-Funktion: rechnet mit einem neuronalen Netz, das beim ersten Gebrauch einmalig geladen wird";

/// `text` followed by the yellow AI mark, for buttons, checkboxes and menu items.
pub fn ai_text(ui: &Ui, text: &str) -> egui::WidgetText {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: font_id.clone(), color: Color32::PLACEHOLDER, ..Default::default() });
    job.append(theme::icon::SPARKLE, 5.0, egui::TextFormat { font_id, color: theme::AI, ..Default::default() });
    job.into()
}

/// Section that starts closed unless `default_open`, with an optional help icon.
/// Flat header: a fine line above, the title on the left, help, reset and caret on the right.
pub fn section_full(
    ui: &mut Ui,
    title: &str,
    default_open: bool,
    help: Option<&str>,
    reset: Option<&mut dyn FnMut()>,
    body: impl FnOnce(&mut Ui),
) {
    section_inner(ui, title, default_open, help, None, false, None, reset, body);
}

/// A section that starts closed and shows in its header how many entries it holds
/// ("Bearbeitung (3)"; nothing for none).
pub fn section_counted(ui: &mut Ui, title: &str, help: Option<&str>, count: usize, body: impl FnOnce(&mut Ui)) {
    section_inner(ui, title, false, help, None, false, Some(count), None, body);
}

fn copy_targets_id() -> egui::Id {
    egui::Id::new("tool_copy_targets")
}

fn copy_clicked_id() -> egui::Id {
    egui::Id::new("tool_copy_clicked")
}

/// How many other images the copy button of a tool section carries its settings to (0: no
/// button); set before the tool tabs are drawn.
pub fn set_copy_targets(ctx: &egui::Context, n: usize) {
    ctx.data_mut(|d| d.insert_temp(copy_targets_id(), n));
}

fn tool_states_id() -> egui::Id {
    egui::Id::new("tool_states")
}

fn eye_clicked_id() -> egui::Id {
    egui::Id::new("tool_eye_clicked")
}

/// Which tools of the image are switched off: a tool section shows the eye to switch them on
/// again. Set before the tool tabs are drawn.
#[derive(Clone, Default)]
pub struct ToolStates {
    pub hidden: Vec<sk_core::tools::Tool>,
}

pub fn set_tool_states(ctx: &egui::Context, states: ToolStates) {
    ctx.data_mut(|d| d.insert_temp(tool_states_id(), states));
}

/// The tool whose eye was clicked since the last call.
pub fn take_eye_clicked(ctx: &egui::Context) -> Option<sk_core::tools::Tool> {
    ctx.data_mut(|d| {
        let tool = d.get_temp(eye_clicked_id());
        d.remove::<sk_core::tools::Tool>(eye_clicked_id());
        tool
    })
}

fn peek_id() -> egui::Id {
    egui::Id::new("tool_peek")
}

/// The tool whose effect is left out of the view while Shift and the mouse button are held on
/// its reset button (as in Capture One); set anew each frame.
pub fn take_tool_peek(ctx: &egui::Context) -> Option<sk_core::tools::Tool> {
    ctx.data_mut(|d| {
        let tool = d.get_temp(peek_id());
        d.remove::<sk_core::tools::Tool>(peek_id());
        tool
    })
}

fn auto_state_id() -> egui::Id {
    egui::Id::new("tool_auto_state")
}

fn auto_clicked_id() -> egui::Id {
    egui::Id::new("tool_auto_clicked")
}

/// Whether the "A" in the tool headers (see `auto_tools`) can start now. Set before the tool tabs
/// are drawn.
#[derive(Clone, Copy, Default)]
pub struct AutoState {
    /// An automatic run is going on.
    pub busy: bool,
    /// The lens profiles are loaded.
    pub lens_db: bool,
}

pub fn set_auto_state(ctx: &egui::Context, state: AutoState) {
    ctx.data_mut(|d| d.insert_temp(auto_state_id(), state));
}

/// The tool whose "A" was clicked since the last call.
pub fn take_auto_clicked(ctx: &egui::Context) -> Option<sk_core::tools::Tool> {
    ctx.data_mut(|d| {
        let tool = d.get_temp(auto_clicked_id());
        d.remove::<sk_core::tools::Tool>(auto_clicked_id());
        tool
    })
}

fn edit_layer_id() -> egui::Id {
    egui::Id::new("tool_edit_layer")
}

/// Whether the develop tools change a layer; set before the tool tabs are drawn. Tool sections
/// then show whether they work on it; those that don't are shown disabled and closed.
pub fn set_edit_layer(ctx: &egui::Context, on_layer: bool) {
    ctx.data_mut(|d| d.insert_temp(edit_layer_id(), on_layer));
}

fn on_layer(ui: &Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(edit_layer_id())).unwrap_or(false)
}

/// Explanation on what only works on the whole image while a layer is chosen.
pub const ONLY_IMAGE: &str = "Wirkt nur aufs ganze Bild. Zum Ändern oben in der Leiste den Hintergrund wählen.";

/// Shows controls that only work on the whole image disabled while a layer is chosen (for a
/// row of buttons; a whole section uses [`image_only_section`]).
pub fn background_only(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    let layer = on_layer(ui);
    ui.add_enabled_ui(!layer, body).response.on_disabled_hover_text(ONLY_IMAGE);
}

/// A section that only works on the whole image: while a layer is chosen, only its closed
/// header with "nur ganzes Bild" (as for the develop tools, see `section_inner`).
pub fn image_only_section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    if on_layer(ui) {
        image_only_header(ui, title);
    } else {
        body(ui);
    }
}

fn image_only_header(ui: &mut Ui, title: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let painter = ui.painter();
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
    let inner = rect.shrink2(vec2(0.0, 2.0)).translate(vec2(0.0, 1.0));
    painter.text(pos2(inner.left() + 6.0, inner.center().y), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(13.5), theme::TEXT_FAINT);
    painter.text(inner.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, "nur ganzes Bild", egui::FontId::proportional(11.5), theme::TEXT_FAINT);
    resp.on_hover_text(ONLY_IMAGE);
    ui.add_space(2.0);
}

/// The tool whose copy button was clicked since the last call.
pub fn take_copy_clicked(ctx: &egui::Context) -> Option<sk_core::tools::Tool> {
    ctx.data_mut(|d| {
        let tool = d.get_temp(copy_clicked_id());
        d.remove::<sk_core::tools::Tool>(copy_clicked_id());
        tool
    })
}

/// [`section`] of a develop tool: titled after it, with a button in the header that carries
/// its settings over to the other selected images.
pub fn tool_section(ui: &mut Ui, tool: sk_core::tools::Tool, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_inner(ui, tool.label(), true, None, Some(tool), tool.is_ai(), None, reset, body);
}

/// [`tool_section`] with an explanation behind an info icon in the header.
pub fn tool_section_help(ui: &mut Ui, tool: sk_core::tools::Tool, help: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_inner(ui, tool.label(), true, Some(help), Some(tool), tool.is_ai(), None, reset, body);
}

/// Sections of functions that not everyone uses: closed until opened (then they stay open).
const SPECIALIZED: &[&str] = &[
    "Tonwerte",
    "Gesichter & Sichtung",
    "Hauttöne",
    "Schwarzweiß",
    "Objektivunschärfe",
    "Atmosphäre",
    "Leuchten",
    "Softproof",
    "Wasserzeichen",
    "Druckvorschau",
];

#[allow(clippy::too_many_arguments)]
fn section_inner(
    ui: &mut Ui,
    title: &str,
    default_open: bool,
    help: Option<&str>,
    copy: Option<sk_core::tools::Tool>,
    ai: bool,
    count: Option<usize>,
    reset: Option<&mut dyn FnMut()>,
    body: impl FnOnce(&mut Ui),
) {
    // While a layer is chosen, the tools that don't work on it are there, but disabled and closed.
    let on_layer = on_layer(ui);
    let layer_support = copy.filter(|_| on_layer).map(|t| t.layer_support());
    if layer_support == Some(sk_core::tools::LayerSupport::None) {
        image_only_header(ui, title);
        return;
    }
    // Specialized and counted sections start closed; their state is kept under a key of its own,
    // so that they also close once where they had been open before (every section used to start
    // open).
    let special = SPECIALIZED.contains(&title);
    let id = if special || count.is_some() { ui.make_persistent_id((title, "eingeklappt")) } else { ui.make_persistent_id(title) };
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open && !special));
    let reveal = ui.data(|d| d.get_temp::<String>(reveal_id())).is_some_and(|t| t == title);
    let flash_id = id.with("flash");
    let now = ui.input(|i| i.time);
    if reveal {
        open = true;
        ui.data_mut(|d| {
            d.insert_persisted(id, true);
            d.remove::<String>(reveal_id());
            d.insert_temp(flash_id, now);
        });
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if reveal {
        resp.scroll_to_me(Some(egui::Align::TOP));
    }
    let painter = ui.painter();
    let hovered = resp.hovered();
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
    let inner = rect.shrink2(vec2(0.0, 2.0)).translate(vec2(0.0, 1.0));
    if hovered {
        painter.rect_filled(inner, 5.0, Color32::from_rgb(42, 42, 46));
    }
    // The eye switches the tool's effect off and on (on the background, not on a layer).
    let eye_tool = copy.filter(|_| !on_layer);
    let states = eye_tool.and_then(|_| ui.data(|d| d.get_temp::<ToolStates>(tool_states_id()))).unwrap_or_default();
    let hidden = eye_tool.is_some_and(|t| states.hidden.contains(&t));
    let title_color = if hidden {
        theme::TEXT_FAINT
    } else if open || hovered {
        Color32::WHITE
    } else {
        theme::TEXT
    };
    let galley = painter.layout_no_wrap(title.to_string(), egui::FontId::proportional(13.5), title_color);
    let mut title_w = galley.size().x;
    painter.galley(pos2(inner.left() + 6.0, inner.center().y - galley.size().y / 2.0), galley, title_color);
    if let Some(n) = count.filter(|n| *n > 0) {
        let galley = painter.layout_no_wrap(format!("({n})"), egui::FontId::proportional(12.5), theme::TEXT_DIM);
        painter.galley(pos2(inner.left() + 6.0 + title_w + 5.0, inner.center().y - galley.size().y / 2.0), galley.clone(), theme::TEXT_DIM);
        title_w += 5.0 + galley.size().x;
    }
    if ai {
        let r = Rect::from_center_size(pos2(inner.left() + 6.0 + title_w + 12.0, inner.center().y), vec2(16.0, 20.0));
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, theme::icon::SPARKLE, egui::FontId::proportional(13.0), theme::AI);
        ui.interact(r, id.with("ai"), Sense::hover()).on_hover_text(AI_HINT);
        title_w += 18.0;
    }
    let marked = layer_support.is_some();
    if marked {
        painter.rect_filled(Rect::from_min_size(pos2(rect.left(), inner.top() + 3.0), vec2(2.0, inner.height() - 6.0)), 1.0, theme::ACCENT);
    }
    let caret = if open { theme::icon::CARET_DOWN } else { theme::icon::CARET_RIGHT };
    let dim = if hovered { theme::TEXT } else { theme::TEXT_FAINT };
    painter.text(inner.right_center() - vec2(10.0, 0.0), egui::Align2::CENTER_CENTER, caret, egui::FontId::proportional(11.0), dim);
    // Brief accent outline after the command palette jumped here.
    if let Some(t0) = ui.data(|d| d.get_temp::<f64>(flash_id)) {
        let a = 1.0 - ((now - t0) / 1.4) as f32;
        if a > 0.0 {
            painter.rect_stroke(inner, 5.0, Stroke::new(1.5, theme::ACCENT.gamma_multiply(a)), StrokeKind::Inside);
            ui.ctx().request_repaint();
        } else {
            ui.data_mut(|d| d.remove::<f64>(flash_id));
        }
    }
    let mut header_button_clicked = false;
    if let Some(help) = help {
        let r = Rect::from_center_size(pos2(inner.left() + 6.0 + title_w + 14.0, inner.center().y), vec2(20.0, 20.0));
        let hr = ui.interact(r, id.with("help"), Sense::hover());
        let c = if hr.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, theme::icon::INFO, egui::FontId::proportional(13.0), c);
        hr.on_hover_ui(|ui| {
            ui.set_max_width(260.0);
            ui.label(help);
        });
    }
    // Right to left: caret, eye (only for a tool switched off with the former eye, to switch it
    // on again), reset, "A" (tools that set themselves), copy.
    let mut x = 34.0;
    if let Some(tool) = eye_tool.filter(|_| hidden) {
        let r = Rect::from_center_size(inner.right_center() - vec2(x, 0.0), vec2(24.0, 22.0));
        let b = egui::Button::new(RichText::new(theme::icon::EYE_SLASH).size(13.0).color(theme::ACCENT)).frame_when_inactive(false);
        if ui.put(r, b).on_hover_text("Wirkung wieder einblenden").clicked() {
            ui.data_mut(|d| d.insert_temp(eye_clicked_id(), tool));
            header_button_clicked = true;
        }
        x += 26.0;
    }
    if let Some(reset) = reset {
        let r = Rect::from_center_size(inner.right_center() - vec2(x, 0.0), vec2(24.0, 22.0));
        let color = if hovered || ui.rect_contains_pointer(r) { theme::TEXT } else { theme::TEXT_FAINT };
        let b = egui::Button::new(RichText::new(theme::icon::ARROWS_COUNTER_CLOCKWISE).size(13.0).color(color)).frame_when_inactive(false);
        // With Shift the tool is only left out of the view while the button is held.
        let peek = eye_tool.filter(|_| ui.input(|i| i.modifiers.shift));
        let hover = if eye_tool.is_some() { "Werkzeug zurücksetzen\nUmschalt gedrückt halten: Wirkung vorübergehend ausblenden" } else { "Werkzeug zurücksetzen" };
        let br = ui.put(r, b).on_hover_text(hover);
        if let Some(tool) = peek.filter(|_| br.is_pointer_button_down_on()) {
            ui.data_mut(|d| d.insert_temp(peek_id(), tool));
        }
        if br.clicked() {
            if peek.is_none() {
                reset();
            }
            header_button_clicked = true;
        }
        x += 26.0;
    }
    if let Some((tool, hint)) = copy.and_then(|t| Some((t, crate::auto_tools::tool_hint(t)?))) {
        let r = Rect::from_center_size(inner.right_center() - vec2(x, 0.0), vec2(24.0, 22.0));
        let state = ui.data(|d| d.get_temp::<AutoState>(auto_state_id())).unwrap_or_default();
        let lens_missing = tool == sk_core::tools::Tool::LensCorrection && !state.lens_db;
        let enabled = !on_layer && !state.busy && !lens_missing;
        let color = if hovered || ui.rect_contains_pointer(r) { theme::TEXT } else { theme::TEXT_FAINT };
        let b = egui::Button::new(RichText::new("A").size(13.0).strong().color(color)).frame_when_inactive(false);
        let disabled = if on_layer {
            ONLY_IMAGE
        } else if state.busy {
            "Erst wenn die laufende automatische Anpassung fertig ist"
        } else {
            "Die Objektivprofile werden noch geladen"
        };
        if ui.put(r, |ui: &mut Ui| ui.add_enabled(enabled, b)).on_hover_text(hint).on_disabled_hover_text(disabled).clicked() {
            ui.data_mut(|d| d.insert_temp(auto_clicked_id(), tool));
            header_button_clicked = true;
        }
        x += 26.0;
    }
    let targets = ui.data(|d| d.get_temp::<usize>(copy_targets_id())).unwrap_or(0);
    // Layers belong to one image: nothing to carry over while one is chosen.
    if let Some(tool) = copy.filter(|_| targets > 0 && !on_layer) {
        let r = Rect::from_center_size(inner.right_center() - vec2(x, 0.0), vec2(24.0, 22.0));
        let color = if hovered || ui.rect_contains_pointer(r) { theme::TEXT } else { theme::TEXT_FAINT };
        let b = egui::Button::new(RichText::new(theme::icon::COPY).size(13.0).color(color)).frame_when_inactive(false);
        let hover = format!("„{title}“ auf {} übertragen", if targets == 1 { "das andere ausgewählte Bild".to_string() } else { format!("die anderen {targets} ausgewählten Bilder") });
        if ui.put(r, b).on_hover_text(hover).clicked() {
            ui.data_mut(|d| d.insert_temp(copy_clicked_id(), tool));
            header_button_clicked = true;
        }
    }
    if resp.clicked() && !header_button_clicked {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    if open {
        ui.add_space(2.0);
        let frame = egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 4, top: 0, bottom: 0 });
        // Switched off: shown as it is set, but only to be changed once switched on again.
        let r = frame
            .show(ui, |ui| {
                ui.add_enabled_ui(!hidden, body).response.on_disabled_hover_text("Ausgeblendet: zum Bearbeiten mit dem Auge wieder einblenden");
            })
            .response;
        if marked {
            ui.painter().rect_filled(Rect::from_min_size(r.rect.left_top(), vec2(2.0, r.rect.height())), 1.0, theme::ACCENT.gamma_multiply(0.45));
        }
        ui.add_space(8.0);
    }
    ui.add_space(2.0);
}

/// Width of the label column of slider rows and form fields, so they line up.
fn label_width(ui: &Ui) -> f32 {
    (ui.available_width() * 0.32).clamp(70.0, 112.0)
}

/// Form row: a dim label in the label column, then the control filling the rest of the width.
pub fn field<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let w = label_width(ui);
        ui.allocate_ui_with_layout(vec2(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(w);
            ui.add(egui::Label::new(RichText::new(label).color(theme::TEXT_DIM)).truncate());
        });
        // Like the combo boxes: the field's frame must not reach past the panel edge.
        ui.spacing_mut().text_edit_width = ui.available_width() - 8.0;
        add(ui)
    })
    .inner
}

/// Drop-down list as a form row; `show` lists the choices.
pub fn combo_field(ui: &mut Ui, label: &str, selected: impl Into<egui::WidgetText>, show: impl FnOnce(&mut Ui)) {
    field(ui, label, |ui| {
        let r = egui::ComboBox::from_id_salt(label).selected_text(selected).width(ui.available_width() - 8.0).show_ui(ui, show);
        // Named after the row (screen readers, tests), not after the chosen entry.
        r.response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), label));
    });
}

/// Options of a [`slider_row_with`].
#[derive(Clone, Copy, Default)]
pub struct SliderOpts<'a> {
    /// Unit after the value, e.g. " %".
    pub suffix: &'static str,
    /// Colors painted along the rail (left to right) instead of the accent fill, e.g. blue to
    /// yellow for a color temperature.
    pub rail: Option<&'a [Color32]>,
    /// An AI function: the label gets the yellow mark.
    pub ai: bool,
    /// How it works, behind an info icon after the label.
    pub help: Option<&'a str>,
    /// Set while the slider is held with Alt (for a view of what it does, as in Schärfen).
    pub held_with_alt: Option<&'a std::cell::Cell<bool>>,
    /// Set while the mouse is over the row (or it is dragged).
    pub hovered: Option<&'a std::cell::Cell<bool>>,
}

/// Rail colors for color temperature (cool to warm).
pub const RAIL_KELVIN: &[Color32] = &[Color32::from_rgb(70, 120, 230), Color32::from_rgb(200, 200, 200), Color32::from_rgb(235, 190, 60)];
/// Rail colors for tint (green to magenta).
pub const RAIL_TINT: &[Color32] = &[Color32::from_rgb(70, 190, 90), Color32::from_rgb(200, 200, 200), Color32::from_rgb(210, 80, 200)];
/// Rail colors for saturation (grey to colorful).
pub const RAIL_SATURATION: &[Color32] = &[Color32::from_rgb(140, 140, 140), Color32::from_rgb(220, 90, 170), Color32::from_rgb(240, 160, 40)];

/// Rail colors for tones (dark to light).
pub const RAIL_TONE: &[Color32] = &[Color32::from_rgb(40, 40, 40), Color32::from_rgb(125, 125, 125), Color32::from_rgb(215, 215, 215)];

/// Zoom factor for a scroll of `scroll` points: a wheel notch (50 points) zooms by about 1.19,
/// four of them double the size.
pub fn wheel_zoom(scroll: f32) -> f32 {
    (scroll / 200.0).exp2()
}

/// A hue (Oklab degrees, as the color tools take them) at the given saturation and value, for
/// display: HSV of the shown sRGB values. (egui's `Hsva` works on linear values, which turned a
/// red of saturation 0.75 into pink.)
pub fn band_hsv(hue: f32, s: f32, v: f32) -> Color32 {
    let h = srgb_hue(hue) / 60.0;
    let x = |k: f32| {
        let k = (k + h) % 6.0;
        v * (1.0 - s * (k.min(4.0 - k)).clamp(0.0, 1.0))
    };
    let c = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(c(x(5.0)), c(x(3.0)), c(x(1.0)))
}

/// The HSV hue (degrees) of the sRGB colors with the Oklab hue `hue`.
fn srgb_hue(hue: f32) -> f32 {
    // Oklab hue of the fully saturated colors around the HSV circle (it rises with it), unwrapped.
    static TABLE: std::sync::LazyLock<Vec<(f32, f32)>> = std::sync::LazyLock::new(|| {
        use sk_core::color::{REC2020_TO_XYZ, SRGB_TO_XYZ, mat_inv, mat_mul, mat_vec, srgb_decode};
        let ok = sk_core::color::Oklab::default();
        let to_working = mat_mul(&mat_inv(&REC2020_TO_XYZ).expect("invertible"), &SRGB_TO_XYZ);
        let mut table: Vec<(f32, f32)> = Vec::with_capacity(721);
        for i in 0..=720 {
            let h = i as f32 / 2.0;
            let k = |n: f32| {
                let k = (n + h / 60.0) % 6.0;
                1.0 - k.min(4.0 - k).clamp(0.0, 1.0)
            };
            let [_, a, b] = ok.from_linear(mat_vec(&to_working, [k(5.0), k(3.0), k(1.0)].map(srgb_decode)));
            let mut deg = b.atan2(a).to_degrees().rem_euclid(360.0);
            if let Some(&(prev, _)) = table.last() {
                deg += ((prev - deg) / 360.0).round() * 360.0;
                // Kept rising where it wavers slightly (between blue and purple).
                deg = deg.max(prev);
            }
            table.push((deg, h));
        }
        table
    });
    let t = &TABLE;
    let first = t[0].0;
    let q = first + (hue - first).rem_euclid(360.0);
    let i = t.partition_point(|e| e.0 < q).clamp(1, t.len() - 1);
    let ((o0, h0), (o1, h1)) = (t[i - 1], t[i]);
    (h0 + (h1 - h0) * ((q - o0) / (o1 - o0).max(1e-6)).clamp(0.0, 1.0)).rem_euclid(360.0)
}

/// Colors of the hue circle (Oklab degrees 0..360), for a hue rail.
pub fn hue_rail() -> Vec<Color32> {
    (0..=12).map(|i| band_hsv(i as f32 * 30.0, 0.75, 0.9)).collect()
}

/// A labeled slider that resets to `default` on double-click of the label.
/// Returns true when the value changed.
pub fn slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
) -> bool {
    slider_row_with(ui, label, value, range, default, decimals, SliderOpts::default())
}

/// [`slider_row`] for an amount in percent ("Stärke", "Deckkraft" …), whole numbers.
pub fn percent_row(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32) -> bool {
    slider_row_with(ui, label, value, range, default, 0, SliderOpts { suffix: " %", ..Default::default() })
}

/// [`percent_row`] for a share stored as 0..1 (`range` and `default` in that unit too): shown
/// in percent like every other amount. Writes back only when moved, so stored values keep
/// their precision.
pub fn unit_percent_row(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, opts: SliderOpts) -> bool {
    let mut p = *value * 100.0;
    let range = range.start() * 100.0..=range.end() * 100.0;
    let moved = slider_row_with(ui, label, &mut p, range, default * 100.0, 0, SliderOpts { suffix: " %", ..opts });
    if moved {
        *value = p / 100.0;
    }
    moved
}

/// [`slider_row`] with a unit and/or a colored rail.
pub fn slider_row_with(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
    opts: SliderOpts<'_>,
) -> bool {
    let mut changed = false;
    // Its own ids, so that the row (label, rail, number) keeps them while widgets before it
    // come and go; a changed id would end a drag.
    let id = stable_id(ui, label);
    ui.scope_builder(egui::UiBuilder::new().id(id), |ui| ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let modified = (*value - default).abs() > f32::EPSILON;
        let color = if modified { Color32::WHITE } else { theme::TEXT_DIM };
        let label_w = label_width(ui);
        let lbl = ui
            .allocate_ui_with_layout(vec2(label_w, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_width(label_w);
                let text: egui::WidgetText = if opts.ai {
                    let font_id = egui::TextStyle::Body.resolve(ui.style());
                    let mut job = egui::text::LayoutJob::default();
                    job.append(label, 0.0, egui::TextFormat { font_id: font_id.clone(), color, ..Default::default() });
                    job.append(theme::icon::SPARKLE, 5.0, egui::TextFormat { font_id, color: theme::AI, ..Default::default() });
                    job.into()
                } else {
                    RichText::new(label).color(color).into()
                };
                let lbl = ui.add(egui::Label::new(text).sense(Sense::click()).truncate());
                if let Some(help) = opts.help {
                    let i = ui.add(egui::Label::new(RichText::new(theme::icon::INFO).size(12.0).color(theme::TEXT_FAINT)).sense(Sense::hover()));
                    i.on_hover_ui(|ui| {
                        ui.set_max_width(280.0);
                        ui.label(help);
                    });
                }
                lbl
            })
            .inner;
        if lbl.on_hover_text("Doppelklick: zurücksetzen").double_clicked() && modified {
            *value = default;
            changed = true;
        }
        // Wide enough for the longest value, so the row never overflows (which would make the panel grow).
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_w = [*range.start(), *range.end()]
            .map(|v| ui.painter().layout_no_wrap(format!("{v:.decimals$}{}", opts.suffix), font.clone(), Color32::WHITE).size().x)
            .into_iter()
            .fold(0.0, f32::max);
        let value_w = (text_w + 2.0 * ui.spacing().button_padding.x + 4.0).max(44.0);
        let slider_w = (ui.available_width() - value_w - 8.0).max(60.0);
        let (moved, held) = rail_slider(ui, label, value, range.clone(), default, decimals, slider_w, opts.rail);
        changed |= moved;
        if let Some(cell) = opts.held_with_alt
            && held
            && ui.input(|i| i.modifiers.alt)
        {
            cell.set(true);
        }
        if let Some(cell) = opts.hovered
            && (held || ui.rect_contains_pointer(ui.min_rect()))
        {
            cell.set(true);
        }
        // The number: plain text until hovered, then an editable field (drag or type).
        ui.scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            w.inactive.weak_bg_fill = Color32::TRANSPARENT;
            w.inactive.bg_fill = Color32::TRANSPARENT;
            w.inactive.fg_stroke.color = if modified { Color32::WHITE } else { theme::TEXT_DIM };
            let span = range.end() - range.start();
            // Without `clamp_existing_to_range(false)` merely drawing the row would pull a value
            // from an older, wider range (e.g. keystone ±100) into the current one.
            let mut drag = egui::DragValue::new(value)
                .range(range.clone())
                .clamp_existing_to_range(false)
                .fixed_decimals(decimals)
                .suffix(opts.suffix)
                .speed(span as f64 / 400.0);
            // Sliders that go both ways show the direction: "+12", "-12", "0".
            if *range.start() < 0.0 {
                drag = drag.custom_formatter(move |v, _| if v.abs() < 0.5 * 10f64.powi(-(decimals as i32)) { format!("{:.decimals$}", 0.0) } else { format!("{v:+.decimals$}") });
            }
            changed |= ui.add_sized(vec2(value_w, 20.0), drag).changed();
        });
    }));
    changed
}

/// An id for the widget `name` that stays the same while widgets before it come and go: egui's
/// automatic ids count those, and a slider whose id changes mid-drag loses the drag (as when the
/// tool header gained a button with the first change). A second widget of the same name in the
/// same place gets the automatic id.
fn stable_id(ui: &Ui, name: &str) -> egui::Id {
    let id = ui.id().with(("stable", name));
    let pass = ui.ctx().cumulative_pass_nr();
    let fresh = ui.data_mut(|d| {
        let used = d.get_temp_mut_or_default::<(u64, std::collections::HashSet<egui::Id>)>(egui::Id::new("stable_ids"));
        if used.0 != pass {
            *used = (pass, Default::default());
        }
        used.1.insert(id)
    });
    if fresh { id } else { ui.next_auto_id() }
}

/// Slim slider: a thin rail filled from the default to the value, a small round handle.
/// Grabbing the handle moves it from where it is; a click elsewhere on the rail jumps there.
/// Shift while dragging for fine steps; double-click resets. After a click the arrow keys and,
/// over the rail, the mouse wheel change the value (Shift: coarse, Alt: fine). Without that
/// click the wheel scrolls the panel, so scrolling past a slider never changes it. Returns
/// whether the value changed and whether the slider is held.
#[allow(clippy::too_many_arguments)]
fn rail_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
    width: f32,
    rail: Option<&[Color32]>,
) -> (bool, bool) {
    let (lo, hi) = (*range.start(), *range.end());
    let span = (hi - lo).max(f32::EPSILON);
    let fill_from = default.clamp(lo, hi);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click_and_drag());
    if ui.is_enabled() && (resp.hovered() || resp.dragged()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let x0 = rect.left() + 7.0;
    let x1 = rect.right() - 7.0;
    let to_x = |v: f32| x0 + (x1 - x0) * ((v - lo) / span).clamp(0.0, 1.0);
    let from_x = |x: f32| lo + span * ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let before = *value;
    let min_step = 1.0 / 10f32.powi(decimals as i32);
    let step = |m: egui::Modifiers| {
        let base = (span / 100.0).max(min_step);
        if m.shift {
            base * 5.0
        } else if m.alt {
            (base / 10.0).max(min_step)
        } else {
            base
        }
    };

    // Dragging is relative: the value follows the pointer's way from where it was pressed, with
    // the value at that moment as the anchor. Shift re-anchors (fine steps from where the handle
    // is), and the unclamped way keeps the handle under the pointer after it went past an end.
    let anchor_id = resp.id.with("anchor");
    let fine = ui.input(|i| i.modifiers.shift);
    if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        let p = resp.interact_pointer_pos().unwrap_or(rect.center());
        let on_handle = (p.x - to_x(*value)).abs() <= 9.0;
        if !on_handle && !fine {
            *value = from_x(p.x);
        }
        ui.data_mut(|d| d.insert_temp(anchor_id, DragAnchor { x: p.x, value: *value, fine }));
        resp.request_focus();
    }
    if resp.is_pointer_button_down_on()
        && let Some(p) = resp.interact_pointer_pos()
        && let Some(mut a) = ui.data(|d| d.get_temp::<DragAnchor>(anchor_id))
    {
        let scale = |fine: bool| span / (x1 - x0) * if fine { 0.1 } else { 1.0 };
        if a.fine != fine {
            a = DragAnchor { x: p.x, value: a.value + (p.x - a.x) * scale(a.fine), fine };
            ui.data_mut(|d| d.insert_temp(anchor_id, a));
        }
        let v = (a.value + (p.x - a.x) * scale(fine)).clamp(lo, hi);
        if (v - *value).abs() > f32::EPSILON && (p.x - a.x).abs() > 0.0 {
            *value = v;
        }
    }
    if resp.has_focus() {
        // Arrows in the slider's direction must not move the focus on (as in egui's slider);
        // the app leaves left/right to a focused slider instead of switching images.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(resp.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() })
        });
        ui.data_mut(|d| d.insert_temp(focused_slider_id(), resp.id));
        let delta: f32 = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } if !modifiers.command => match key {
                        egui::Key::ArrowRight | egui::Key::ArrowUp => Some(step(*modifiers)),
                        egui::Key::ArrowLeft | egui::Key::ArrowDown => Some(-step(*modifiers)),
                        _ => None,
                    },
                    _ => None,
                })
                .sum()
        });
        if delta != 0.0 {
            *value = (*value + delta).clamp(lo, hi);
            mark_stepped(ui.ctx(), resp.id);
        }
    }
    if resp.hovered() {
        let pass = ui.ctx().cumulative_pass_nr();
        ui.data_mut(|d| d.insert_temp(hovered_rail_id(), HoveredRail(resp.id, pass)));
    }
    let owns_wheel = ui.data(|d| d.get_temp::<WheelGesture>(wheel_gesture_id())).is_some_and(|g| g.owner == Some(resp.id));
    if ui.is_enabled() && resp.hovered() && resp.has_focus() && owns_wheel {
        let (notches, mods) = ui.input(|i| {
            let n: f32 = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Line => delta.y,
                        egui::MouseWheelUnit::Point => delta.y / 40.0,
                        egui::MouseWheelUnit::Page => delta.y.signum(),
                    }),
                    _ => None,
                })
                .sum();
            (n, i.modifiers)
        });
        // Touchpads send small deltas: collect them until a whole step is reached.
        let acc_id = resp.id.with("wheel");
        let total = ui.data(|d| d.get_temp::<f32>(acc_id)).unwrap_or(0.0) + notches;
        let whole = total.trunc();
        ui.data_mut(|d| d.insert_temp(acc_id, total - whole));
        if whole != 0.0 && !mods.command {
            *value = (*value + whole * step(mods)).clamp(lo, hi);
            mark_stepped(ui.ctx(), resp.id);
        }
        // The wheel belongs to the slider, not to the panel around it.
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    if *value != before {
        let q = 10f32.powi(decimals as i32);
        *value = (*value * q).round() / q;
    }
    // Double-click resets.
    if resp.double_clicked() {
        *value = default;
    }
    resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *value as f64, label));

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let cy = rect.center().y;
        let rail_rect = Rect::from_min_max(pos2(x0, cy - 1.5), pos2(x1, cy + 1.5));
        match rail {
            Some(colors) if colors.len() >= 2 => {
                let mut mesh = egui::Mesh::default();
                let n = colors.len() - 1;
                for (i, c) in colors.iter().enumerate() {
                    let x = x0 + (x1 - x0) * i as f32 / n as f32;
                    let c = c.gamma_multiply(0.85);
                    mesh.colored_vertex(pos2(x, cy - 2.0), c);
                    mesh.colored_vertex(pos2(x, cy + 2.0), c);
                    if i > 0 {
                        let k = (2 * i) as u32;
                        mesh.add_triangle(k - 2, k - 1, k);
                        mesh.add_triangle(k - 1, k, k + 1);
                    }
                }
                painter.add(mesh);
            }
            _ => {
                painter.rect_filled(rail_rect, 1.5, Color32::from_rgb(64, 64, 70));
                let (a, b) = (to_x(fill_from), to_x(*value));
                if (a - b).abs() > 0.5 {
                    let fill = Rect::from_min_max(pos2(a.min(b), cy - 1.5), pos2(a.max(b), cy + 1.5));
                    painter.rect_filled(fill, 1.5, theme::ACCENT);
                }
                // A tick at the default when it lies inside the range (bipolar sliders).
                let d = to_x(default);
                if d > x0 + 1.0 && d < x1 - 1.0 {
                    painter.line_segment([pos2(d, cy - 4.0), pos2(d, cy + 4.0)], Stroke::new(1.0, Color32::from_rgb(90, 90, 98)));
                }
            }
        }
        let active = resp.dragged() || resp.is_pointer_button_down_on();
        let r = if active { 7.0 } else if resp.hovered() { 6.5 } else { 5.5 };
        let r = ui.ctx().animate_value_with_time(resp.id.with("r"), r, 0.08);
        let c = pos2(to_x(*value), cy);
        painter.circle_filled(c + vec2(0.0, 1.0), r + 1.0, Color32::from_black_alpha(90));
        painter.circle_filled(c, r, if ui.is_enabled() { Color32::from_rgb(238, 238, 242) } else { Color32::from_gray(120) });
        if resp.has_focus() {
            painter.circle_stroke(c, r + 2.5, Stroke::new(1.5, theme::ACCENT));
        }
    }
    (*value != before, resp.is_pointer_button_down_on())
}

/// Where a slider drag was pressed (or re-anchored when Shift changed) and the value then.
#[derive(Clone, Copy)]
struct DragAnchor {
    x: f32,
    value: f32,
    fine: bool,
}

/// Which slider a mouse-wheel gesture belongs to: one that starts over a slider's rail adjusts
/// it, one that starts elsewhere keeps scrolling the panel while sliders pass under the pointer.
#[derive(Clone, Copy)]
struct WheelGesture {
    owner: Option<egui::Id>,
    last: f64,
}

impl Default for WheelGesture {
    fn default() -> Self {
        // The clock starts at 0: the first wheel event must start a gesture too.
        Self { owner: None, last: f64::NEG_INFINITY }
    }
}

/// The slider rail under the pointer and the pass it was drawn in.
#[derive(Clone, Copy)]
struct HoveredRail(egui::Id, u64);

/// Wheel events closer together than this belong to the same gesture.
const WHEEL_GESTURE_GAP: f64 = 0.3;

/// Changes by arrow keys or wheel notches come one step at a time: the steps of one control
/// become one undo step once they have paused this long (see `App::finish_frame`).
const STEP_PAUSE: f64 = 0.8;

fn stepped_id() -> egui::Id {
    egui::Id::new("value_stepped")
}

#[derive(Clone, Copy, Debug)]
struct Stepped {
    time: f64,
    source: egui::Id,
    /// Not yet seen by `take_steps` (the viewer window has frames of its own, so no frame number).
    fresh: bool,
}

/// Steps of one control that haven't paused yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingSteps {
    /// The control (slider, speed edit key).
    pub source: egui::Id,
    /// Seconds until they count as finished.
    pub left: f64,
    /// A step came in since the last call.
    pub now: bool,
}

/// A value of `source` was changed by a key or a wheel notch (not by dragging).
pub fn mark_stepped(ctx: &egui::Context, source: egui::Id) {
    let s = Stepped { time: ctx.input(|i| i.time), source, fresh: true };
    ctx.data_mut(|d| d.insert_temp(stepped_id(), Some(s)));
}

/// The key or wheel steps still running (`None`: they have paused, or there were none). Called
/// once per frame, by `App::finish_frame`.
pub fn take_steps(ctx: &egui::Context) -> Option<PendingSteps> {
    let s = ctx.data_mut(|d| {
        let s = d.get_temp_mut_or_default::<Option<Stepped>>(stepped_id()).as_mut()?;
        let seen = *s;
        s.fresh = false;
        Some(seen)
    })?;
    let left = STEP_PAUSE - (ctx.input(|i| i.time) - s.time);
    (left > 0.0).then_some(PendingSteps { source: s.source, left, now: s.fresh })
}

/// The running steps count as finished (another change ended them).
pub fn end_steps(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Option<Stepped>>(stepped_id()));
}

fn wheel_gesture_id() -> egui::Id {
    egui::Id::new("slider_wheel_gesture")
}

fn hovered_rail_id() -> egui::Id {
    egui::Id::new("slider_hovered_rail")
}

fn focused_slider_id() -> egui::Id {
    egui::Id::new("slider_focused")
}

/// Assigns a starting mouse-wheel gesture to the slider under the pointer (or to none). Call once
/// per frame before the panels are drawn.
pub fn begin_wheel_gesture(ctx: &egui::Context) {
    let (wheel, now) = ctx.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. })), i.time));
    if !wheel {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let hovered = d.get_temp::<HoveredRail>(hovered_rail_id()).filter(|h| h.1 + 1 >= pass).map(|h| h.0);
        let g = d.get_temp_mut_or_default::<WheelGesture>(wheel_gesture_id());
        if now - g.last > WHEEL_GESTURE_GAP {
            g.owner = hovered;
        }
        g.last = now;
    });
}

/// True while a slider has the keyboard focus (its arrow keys then change the value).
pub fn slider_has_focus(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(|m| m.focused());
    focused.is_some() && ctx.data(|d| d.get_temp::<egui::Id>(focused_slider_id())) == focused
}

/// Slider without label and number, e.g. for the thumbnail size; double-click resets.
pub fn compact_slider(ui: &mut Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, width: f32) -> bool {
    rail_slider(ui, name, value, range, default, 2, width, None).0
}

/// What a drag on a color wheel changes, and where it was pressed (or re-anchored when Shift
/// changed) with the value then.
#[derive(Clone, Copy)]
struct WheelDrag {
    part: WheelPart,
    pos: Pos2,
    /// The point as an offset from the center in radii (disc), or the value (arcs).
    start: Vec2,
    fine: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum WheelPart {
    Disc,
    /// The arc on the left: the amount.
    Amount,
    /// The arc on the right: the lightness.
    Lightness,
}

/// Half the angle an arc beside the wheel spans.
const ARC_SPAN: f32 = 62.0;

/// Point at `deg` (counter-clockwise from the right, as the hue) on a circle.
fn on_circle(center: Pos2, radius: f32, deg: f32) -> Pos2 {
    let a = deg.to_radians();
    center + vec2(a.cos(), -a.sin()) * radius
}

/// An arc from `from` to `to` degrees, its color running through `colors` along the way.
/// Its edges fade out over a pixel, so that they look smooth (egui smooths only its own shapes).
fn paint_arc(p: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, width: f32, colors: impl Fn(f32) -> Color32) {
    let n = ((to - from).abs() / 2.0).ceil().max(8.0) as u32;
    let feather = 1.0 / p.ctx().pixels_per_point();
    let radii = [radius - width / 2.0 - feather / 2.0, radius - width / 2.0 + feather / 2.0, radius + width / 2.0 - feather / 2.0, radius + width / 2.0 + feather / 2.0];
    let mut mesh = egui::Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let deg = from + (to - from) * t;
        let c = colors(t);
        for (j, r) in radii.iter().enumerate() {
            mesh.colored_vertex(on_circle(center, *r, deg), if j == 0 || j == 3 { Color32::TRANSPARENT } else { c });
        }
        if i > 0 {
            let k = 4 * i;
            for j in 0..3 {
                mesh.add_triangle(k - 4 + j, k - 3 + j, k + j);
                mesh.add_triangle(k - 3 + j, k + j, k + 1 + j);
            }
        }
    }
    p.add(mesh);
}

/// A color wheel `width` wide, after Capture One: the name above, a dark disc in a ring of
/// hues whose point gives the hue (angle) and the amount (distance from the center), on the
/// left an arc for the amount alone, on the right one for the lightness when `with_lightness`.
/// A click sets the point or the arc's value, dragging moves it from where it is (Shift:
/// finely); double-click resets the part under the pointer. Returns true when it changed.
pub fn color_wheel(ui: &mut Ui, label: &str, wheel: &mut ColorWheel, with_lightness: bool, width: f32) -> bool {
    let before = *wheel;
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        let modified = *wheel != ColorWheel::default();
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(label).color(if modified { Color32::WHITE } else { theme::TEXT_DIM }));
        });
        let (rect, resp) = ui.allocate_exact_size(vec2(width, width * 0.86), Sense::click_and_drag());
        let center = rect.center();
        let arc_r = rect.height() / 2.0 - 4.0;
        let radius = arc_r - 11.0;
        let offset_of = |w: &ColorWheel| {
            let a = w.hue.to_radians();
            vec2(a.cos(), -a.sin()) * (w.amount / 100.0)
        };
        let set_point = |w: &mut ColorWheel, v: Vec2| {
            let len = v.length().min(1.0);
            w.amount = (len * 100.0).round();
            // At the center the hue has no direction: it stays.
            if len > 0.005 {
                w.hue = (-v.y).atan2(v.x).to_degrees().rem_euclid(360.0).round() % 360.0;
            }
        };
        // Angle of the pointer on an arc as 0..1 from its lower end, left (amount) or right.
        let arc_t = |p: Pos2, left: bool| {
            let d = p - center;
            let deg = (-d.y).atan2(if left { -d.x } else { d.x }).to_degrees();
            ((deg + ARC_SPAN) / (2.0 * ARC_SPAN)).clamp(0.0, 1.0)
        };
        let part_at = |p: Pos2| {
            let d = p - center;
            if d.length() <= radius + 4.0 {
                Some(WheelPart::Disc)
            } else if d.length() <= arc_r + 9.0 && d.x < 0.0 {
                Some(WheelPart::Amount)
            } else if d.length() <= arc_r + 9.0 && with_lightness {
                Some(WheelPart::Lightness)
            } else {
                None
            }
        };

        let drag_id = resp.id.with("drag");
        let fine = ui.input(|i| i.modifiers.shift);
        let pressed = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed());
        // A drag lasts from press to release: a press beside every part must not go on with
        // the previous one.
        if pressed || !resp.is_pointer_button_down_on() {
            ui.data_mut(|d| d.remove::<WheelDrag>(drag_id));
        }
        if pressed
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(part) = part_at(p)
        {
            match part {
                WheelPart::Disc => {
                    let on_point = (p - (center + offset_of(wheel) * radius)).length() <= 9.0;
                    if !on_point && !fine {
                        set_point(wheel, (p - center) / radius);
                    }
                }
                WheelPart::Amount if !fine => wheel.amount = (arc_t(p, true) * 100.0).round(),
                WheelPart::Lightness if !fine => wheel.lightness = (arc_t(p, false) * 200.0 - 100.0).round(),
                _ => {}
            }
            let start = match part {
                WheelPart::Disc => offset_of(wheel),
                WheelPart::Amount => vec2(wheel.amount, 0.0),
                WheelPart::Lightness => vec2(wheel.lightness, 0.0),
            };
            ui.data_mut(|d| d.insert_temp(drag_id, WheelDrag { part, pos: p, start, fine }));
        }
        let drag = ui.data(|d| d.get_temp::<WheelDrag>(drag_id)).filter(|_| resp.is_pointer_button_down_on());
        if let (Some(mut a), Some(p)) = (drag, resp.interact_pointer_pos()) {
            // The disc follows the pointer's way; an arc its way upwards along it.
            let delta = |a: &WheelDrag, p: Pos2| match a.part {
                WheelPart::Disc => (p - a.pos) / radius,
                WheelPart::Amount => vec2((a.pos.y - p.y) / (arc_r * 1.6) * 100.0, 0.0),
                WheelPart::Lightness => vec2((a.pos.y - p.y) / (arc_r * 1.6) * 200.0, 0.0),
            };
            let scale = |fine: bool| if fine { 0.1 } else { 1.0 };
            if a.fine != fine {
                a = WheelDrag { pos: p, start: a.start + delta(&a, p) * scale(a.fine), fine, ..a };
                ui.data_mut(|d| d.insert_temp(drag_id, a));
            }
            if p != a.pos {
                let v = a.start + delta(&a, p) * scale(fine);
                match a.part {
                    WheelPart::Disc => set_point(wheel, v),
                    WheelPart::Amount => wheel.amount = v.x.clamp(0.0, 100.0).round(),
                    WheelPart::Lightness => wheel.lightness = v.x.clamp(-100.0, 100.0).round(),
                }
            }
        }
        let hovered_part = resp.hover_pos().and_then(part_at);
        if resp.double_clicked() {
            match hovered_part {
                Some(WheelPart::Disc) => (wheel.hue, wheel.amount) = (0.0, 0.0),
                Some(WheelPart::Amount) => wheel.amount = 0.0,
                Some(WheelPart::Lightness) => wheel.lightness = 0.0,
                None => {}
            }
        }
        let shown_part = drag.map(|d| d.part).or(hovered_part);
        if ui.is_enabled() {
            match shown_part {
                Some(WheelPart::Disc) => ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair),
                Some(_) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical),
                None => {}
            }
        }
        let mut tip = format!("Farbton {:.0}° · Stärke {:.0} %", wheel.hue, wheel.amount);
        if with_lightness {
            tip += &format!(" · Helligkeit {:+.0}", wheel.lightness);
        }
        let resp = resp.on_hover_text(tip + "\nZiehen (Umschalt: fein) · Doppelklick: zurücksetzen");

        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let lit = |part: WheelPart| shown_part == Some(part);
            // The disc: dark inside, the hues as a ring around it.
            p.circle_filled(center, radius, Color32::from_rgb(26, 26, 28));
            let ring = 4.5;
            paint_arc(p, center, radius - ring / 2.0, 0.0, 360.0, ring, |t| band_hsv(t * 360.0, 0.85, 0.95));
            p.circle_stroke(center, radius + 0.5, Stroke::new(1.0, Color32::from_black_alpha(160)));

            // The arcs: amount from white to the hue, lightness from dark to light.
            let arc_w = if lit(WheelPart::Amount) { 4.0 } else { 3.0 };
            let hue = wheel.hue;
            paint_arc(p, center, arc_r, 180.0 + ARC_SPAN, 180.0 - ARC_SPAN, arc_w, |t| {
                lerp_color(Color32::from_gray(225), band_hsv(hue, 0.9, 0.95), t)
            });
            let handle = |deg: f32| {
                let s = Stroke::new(2.0, if ui.is_enabled() { Color32::WHITE } else { Color32::from_gray(120) });
                p.line_segment([on_circle(center, arc_r - 5.0, deg), on_circle(center, arc_r + 6.0, deg)], s);
            };
            handle(180.0 + ARC_SPAN - wheel.amount / 100.0 * 2.0 * ARC_SPAN);
            if with_lightness {
                let arc_w = if lit(WheelPart::Lightness) { 4.0 } else { 3.0 };
                paint_arc(p, center, arc_r, -ARC_SPAN, ARC_SPAN, arc_w, |t| lerp_color(Color32::from_gray(20), Color32::from_gray(235), t));
                // The middle (no change) as a fine mark.
                p.line_segment([on_circle(center, arc_r + 3.0, 0.0), on_circle(center, arc_r + 6.0, 0.0)], Stroke::new(1.0, theme::TEXT_FAINT));
                handle(-ARC_SPAN + (wheel.lightness + 100.0) / 200.0 * 2.0 * ARC_SPAN);
            }

            // The point: a ring with a dot, joined to the center once moved.
            let point = center + offset_of(wheel) * radius * (1.0 - (ring + 3.0) / radius).max(0.0);
            if wheel.amount > 0.0 {
                p.circle_filled(center, 1.5, theme::TEXT_FAINT);
                p.line_segment([center, point], Stroke::new(1.0, Color32::from_white_alpha(70)));
            }
            let r = if lit(WheelPart::Disc) { 6.5 } else { 5.5 };
            let r = ui.ctx().animate_value_with_time(resp.id.with("r"), r, 0.08);
            let stroke = if ui.is_enabled() { Color32::from_rgb(230, 230, 234) } else { Color32::from_gray(120) };
            p.circle_filled(point, r, Color32::from_black_alpha(120));
            p.circle_stroke(point, r, Stroke::new(1.5, stroke));
            p.circle_filled(point, 1.6, stroke);
        }
    });
    *wheel != before
}

/// Linear blend of two colors, `t` from 0 (a) to 1 (b).
fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Tool tile: icon above the name, the key in the corner; `selected` highlights it.
pub fn tool_tile(ui: &mut Ui, ic: &str, label: &str, key: &str, selected: bool, ai: bool, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    let fill = if selected {
        theme::ACCENT_SOFT
    } else if hovered {
        Color32::from_rgb(54, 54, 60)
    } else {
        Color32::from_rgb(44, 44, 49)
    };
    p.rect_filled(rect, 7.0, fill);
    if selected {
        p.rect_stroke(rect, 7.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
    }
    let color = if selected || hovered { Color32::WHITE } else { theme::TEXT };
    p.text(rect.center_top() + vec2(0.0, 18.0), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(19.0), color);
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), egui::FontId::proportional(11.5), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width - 8.0);
    let galley = p.layout_job(job);
    p.galley(pos2(rect.center().x - galley.size().x / 2.0, rect.bottom() - 18.0), galley, color);
    // Only short keys fit in the corner; longer ones (with modifiers) are in the tooltip.
    let key = p.layout_no_wrap(key.to_string(), egui::FontId::proportional(10.0), theme::TEXT_FAINT);
    if key.size().x > 0.0 && key.size().x <= width * 0.2 {
        p.galley(rect.right_top() + vec2(-6.0 - key.size().x, 5.0), key, theme::TEXT_FAINT);
    }
    if ai {
        p.text(rect.left_top() + vec2(8.0, 9.0), egui::Align2::CENTER_CENTER, theme::icon::SPARKLE, egui::FontId::proportional(11.0), theme::AI);
    }
    resp
}

/// Small faint explanatory text.
/// A checkbox that reads as one: square with a visible edge, filled with the accent when on.
/// egui's takes the look of the buttons (rounded, no edge), which on the dark panel made an
/// empty box look like a radio button and a ticked one like a bare tick.
pub struct Check<'a> {
    inner: egui::Checkbox<'a>,
    on: bool,
}

impl<'a> Check<'a> {
    pub fn new(checked: &'a mut bool, text: impl egui::IntoAtoms<'a>) -> Self {
        let on = *checked;
        Self { inner: egui::Checkbox::new(checked, text), on }
    }
}

impl egui::Widget for Check<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            for (state, edge) in [(&mut w.inactive, 110), (&mut w.hovered, 150), (&mut w.active, 170)] {
                state.corner_radius = egui::CornerRadius::same(3);
                if self.on {
                    state.bg_fill = theme::ACCENT;
                    state.bg_stroke = Stroke::new(1.0, theme::ACCENT);
                } else {
                    state.bg_fill = theme::PANEL;
                    state.bg_stroke = Stroke::new(1.0, Color32::from_gray(edge));
                }
            }
            ui.add(self.inner)
        })
        .inner
    }
}

/// `text` shortened in front ("…/Bilder/2026") to fit `width` in the body font.
pub fn elide_start(ui: &Ui, text: &str, width: f32) -> String {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let fits = |t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len())
        .map(|i| format!("…{}", chars[i..].iter().collect::<String>()))
        .find(|t| fits(t))
        .unwrap_or_else(|| "…".into())
}

/// [`Check`], like `ui.checkbox`.
pub fn checkbox<'a>(ui: &mut Ui, checked: &'a mut bool, text: impl egui::IntoAtoms<'a>) -> Response {
    ui.add(Check::new(checked, text))
}

pub fn hint(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text.into()).small().color(theme::TEXT_FAINT))
}

/// Centered placeholder for an empty view or tool: a large icon, a line of text and a hint.
pub fn empty_state(ui: &mut Ui, ic: &str, title: &str, hint: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(ic).size(34.0).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(14.0).color(theme::TEXT_DIM));
        if !hint.is_empty() {
            ui.add_space(2.0);
            ui.label(RichText::new(hint).small().color(theme::TEXT_FAINT));
        }
    });
    ui.add_space(12.0);
}

/// Placeholder of the develop tools while no image is open.
pub fn no_image(ui: &mut Ui) {
    empty_state(ui, theme::icon::IMAGE_SQUARE, "Kein Bild ausgewählt", "Ein Bild im Filmstreifen oder Raster anklicken");
}

/// Returns the area it was drawn in.
/// The tonal ranges of the histogram and the sliders that move them (as in Lightroom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneZone {
    Blacks,
    Shadows,
    Exposure,
    Highlights,
    Whites,
}

impl ToneZone {
    pub const ALL: [ToneZone; 5] = [ToneZone::Blacks, ToneZone::Shadows, ToneZone::Exposure, ToneZone::Highlights, ToneZone::Whites];

    /// The slider's name.
    pub fn label(self) -> &'static str {
        match self {
            ToneZone::Blacks => "Schwarz",
            ToneZone::Shadows => "Tiefen",
            ToneZone::Exposure => "Belichtung",
            ToneZone::Highlights => "Lichter",
            ToneZone::Whites => "Weiß",
        }
    }

    /// Where it lies in the histogram (0..1 of its width).
    pub fn span(self) -> (f32, f32) {
        match self {
            ToneZone::Blacks => (0.0, 0.1),
            ToneZone::Shadows => (0.1, 0.35),
            ToneZone::Exposure => (0.35, 0.65),
            ToneZone::Highlights => (0.65, 0.9),
            ToneZone::Whites => (0.9, 1.0),
        }
    }

    fn at(x: f32) -> ToneZone {
        ToneZone::ALL.into_iter().find(|z| x < z.span().1).unwrap_or(ToneZone::Whites)
    }
}

/// What the histogram shows besides the image's tones.
#[derive(Clone, Copy, Default)]
pub struct HistogramMarks {
    /// The color under the mouse in the image: a mark per channel and the values in percent.
    pub rgb: Option<[u8; 3]>,
    /// The range of the slider under the mouse.
    pub zone: Option<ToneZone>,
}

/// The histogram; hovering it shows the tonal ranges, dragging in one moves its slider (returned
/// as the range and the way since the last frame, in histogram widths).
pub fn histogram(ui: &mut Ui, hist: Option<&Histogram>, marks: HistogramMarks) -> (Rect, Option<(ToneZone, f32)>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::BROWSER);
    let Some(h) = hist else { return (rect, None) };
    // The range where a drag began holds while it lasts.
    let drag_id = resp.id.with("zone");
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        ui.data_mut(|d| d.insert_temp(drag_id, ToneZone::at((p.x - rect.left()) / rect.width())));
    }
    let dragging = resp.dragged().then(|| ui.data(|d| d.get_temp::<ToneZone>(drag_id))).flatten();
    let hovered = resp.hover_pos().map(|p| ToneZone::at((p.x - rect.left()) / rect.width()));
    let zone = dragging.or(hovered).or(marks.zone);
    if let Some(z) = zone {
        let (a, b) = z.span();
        let r = Rect::from_min_max(pos2(rect.left() + rect.width() * a, rect.top()), pos2(rect.left() + rect.width() * b, rect.bottom()));
        painter.rect_filled(r, 0.0, Color32::from_white_alpha(14));
    }
    if hovered.is_some() || dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    // Ignore the extreme bins for scaling so clipping spikes do not flatten everything.
    let max = [&h.r, &h.g, &h.b]
        .iter()
        .flat_map(|c| c[1..255].iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let channel = |bins: &[u32]| {
        let mut pts = Vec::with_capacity(258);
        pts.push(rect.left_bottom());
        for (i, &v) in bins.iter().enumerate() {
            let x = rect.left() + rect.width() * i as f32 / 255.0;
            let y = rect.bottom() - rect.height() * ((v as f32 / max).sqrt()).min(1.0);
            pts.push(pos2(x, y));
        }
        pts.push(rect.right_bottom());
        pts
    };
    for (bins, color) in [
        (&h.r, Color32::from_rgba_unmultiplied(230, 60, 60, 90)),
        (&h.g, Color32::from_rgba_unmultiplied(60, 200, 60, 90)),
        (&h.b, Color32::from_rgba_unmultiplied(70, 110, 240, 90)),
    ] {
        let pts = channel(bins);
        // One strip of triangles down to the bottom (a concave polygon would fill wrongly), in a
        // single mesh rather than 255 shapes.
        let mut mesh = egui::Mesh::default();
        for (i, p) in pts[1..pts.len() - 1].iter().enumerate() {
            mesh.colored_vertex(*p, color);
            mesh.colored_vertex(pos2(p.x, rect.bottom()), color);
            if i > 0 {
                let k = 2 * i as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k + 1, k);
            }
        }
        painter.add(Shape::mesh(mesh));
    }
    let luma = channel(&h.luma);
    painter.add(Shape::line(luma[1..luma.len() - 1].to_vec(), Stroke::new(1.0, Color32::from_gray(200))));
    // clipping indicators
    let total: u32 = h.luma.iter().sum::<u32>().max(1);
    let hi = h.r[255].max(h.g[255]).max(h.b[255]) as f32 / total as f32;
    let lo = h.luma[0] as f32 / total as f32;
    if hi > 0.001 {
        painter.circle_filled(rect.right_top() + vec2(-8.0, 8.0), 4.0, Color32::from_rgb(255, 60, 60));
    }
    if lo > 0.001 {
        painter.circle_filled(rect.left_top() + vec2(8.0, 8.0), 4.0, Color32::from_rgb(60, 120, 255));
    }
    let small = egui::FontId::proportional(11.0);
    if let Some(z) = zone {
        painter.text(rect.center_top() + vec2(0.0, 6.0), egui::Align2::CENTER_TOP, z.label(), small.clone(), theme::TEXT);
    }
    // The color under the mouse: where each channel lies, and its values.
    if let Some(rgb) = marks.rgb {
        let colors = [Color32::from_rgb(240, 90, 90), Color32::from_rgb(90, 215, 90), Color32::from_rgb(110, 150, 255)];
        for (v, c) in rgb.iter().zip(colors) {
            let x = rect.left() + rect.width() * *v as f32 / 255.0;
            painter.line_segment([pos2(x, rect.top() + 4.0), pos2(x, rect.bottom())], Stroke::new(1.0, c));
        }
        let pct = rgb.map(|v| (v as f32 / 2.55).round() as u32);
        let text = format!("R {} %   G {} %   B {} %", pct[0], pct[1], pct[2]);
        let galley = painter.layout_no_wrap(text, small, theme::TEXT);
        let at = pos2(rect.center().x - galley.size().x / 2.0, rect.bottom() - galley.size().y - 5.0);
        painter.rect_filled(Rect::from_min_size(at - vec2(6.0, 2.0), galley.size() + vec2(12.0, 4.0)), 4.0, Color32::from_black_alpha(150));
        painter.galley(at, galley, theme::TEXT);
    }
    let moved = dragging.map(|z| (z, resp.drag_delta().x / rect.width().max(1.0))).filter(|(_, d)| *d != 0.0);
    (rect, moved)
}

/// Levels: the histogram of what goes into them (`bins`, 256) with handles below it for the black
/// point, the midtones and the white point, then a gray bar with handles for the output range,
/// and the values as numbers (0–255, midtones as gamma). Double click on a handle resets it.
/// `peek` is set to what the black or white point of the input shows while held with Alt.
pub fn levels_editor(ui: &mut Ui, set: &mut LevelSet, bins: Option<&[f32]>, color: Color32, peek: &std::cell::Cell<Option<sk_core::pipeline::SliderView>>) -> bool {
    const MIN_GAP: f32 = 2.0 / 255.0;
    let before = *set;
    let width = ui.available_width();
    let (hist, _) = ui.allocate_exact_size(vec2(width, 80.0), Sense::hover());
    let (input, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
    ui.add_space(4.0);
    let (bar, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let (output, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
    let x_of = |v: f32| hist.left() + v * hist.width();
    let v_of = |x: f32| ((x - hist.left()) / hist.width()).clamp(0.0, 1.0);
    let painter = ui.painter();

    // What the black and white points cut off is dimmed.
    painter.rect_filled(hist, 3.0, theme::BROWSER);
    if let Some(bins) = bins.filter(|b| b.len() >= 256) {
        let max = bins[1..255].iter().copied().fold(0.0, f32::max).max(1e-6);
        let mut pts = vec![hist.left_bottom()];
        for (i, &n) in bins.iter().enumerate() {
            let h = hist.height() * (n / max).sqrt().min(1.0);
            pts.push(pos2(x_of((i as f32 + 0.5) / 256.0), hist.bottom() - h));
        }
        pts.push(hist.right_bottom());
        // One mesh of columns (the outline is not convex); separate shapes would leave seams.
        let fill = color.gamma_multiply(0.45);
        let mut mesh = egui::Mesh::default();
        for p in &pts {
            mesh.colored_vertex(pos2(p.x, hist.bottom()), fill);
            mesh.colored_vertex(*p, fill);
        }
        for i in 0..pts.len() as u32 - 1 {
            let k = i * 2;
            mesh.add_triangle(k, k + 1, k + 2);
            mesh.add_triangle(k + 1, k + 2, k + 3);
        }
        painter.add(mesh);
    }
    let shade = Color32::from_black_alpha(130);
    painter.rect_filled(Rect::from_min_max(hist.min, pos2(x_of(set.in_black), hist.bottom())), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(pos2(x_of(set.in_white), hist.top()), hist.max), 0.0, shade);
    // Output: from the darkest to the brightest tone it produces.
    let mut mesh = egui::Mesh::default();
    for (x, g) in [(bar.left(), 0u8), (bar.right(), 255)] {
        mesh.colored_vertex(pos2(x, bar.top()), Color32::from_gray(g));
        mesh.colored_vertex(pos2(x, bar.bottom()), Color32::from_gray(g));
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(mesh);
    painter.rect_stroke(bar, 0.0, Stroke::new(1.0, theme::BORDER), StrokeKind::Outside);

    // Midtone handle: where the input maps to middle gray, i.e. 0.5^gamma of the input range.
    let mid_x = |s: &LevelSet| x_of(s.in_black + 0.5f32.powf(s.midtone) * (s.in_white - s.in_black));
    let id = ui.id().with("levels");
    let handle = |ui: &mut Ui, track: Rect, x: f32, fill: Color32, name: &str| -> Option<Option<f32>> {
        let r = Rect::from_center_size(pos2(x, track.center().y), vec2(14.0, track.height()));
        let resp = ui.interact(r, id.with(name), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, name));
        let resp = resp.on_hover_text(format!("{name} – Doppelklick: zurücksetzen"));
        let hot = resp.hovered() || resp.dragged();
        if resp.is_pointer_button_down_on() && ui.input(|i| i.modifiers.alt) {
            match name {
                "Eingang Schwarz" => peek.set(Some(sk_core::pipeline::SliderView::ClipShadows)),
                "Eingang Weiß" => peek.set(Some(sk_core::pipeline::SliderView::ClipHighlights)),
                _ => {}
            }
        }
        let tip = pos2(x, track.top());
        let size = if hot { 7.0 } else { 6.0 };
        let tri = vec![tip, pos2(x + size, track.top() + size * 1.6), pos2(x - size, track.top() + size * 1.6)];
        ui.painter().add(Shape::convex_polygon(tri, fill, Stroke::new(1.0, if hot { theme::ACCENT } else { Color32::from_gray(120) })));
        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp.double_clicked() {
            Some(None)
        } else if resp.dragged() {
            resp.interact_pointer_pos().map(|p| Some(v_of(p.x)))
        } else {
            None
        }
    };
    let d = LevelSet::default();
    match handle(ui, input, x_of(set.in_black), Color32::from_gray(20), "Eingang Schwarz") {
        Some(Some(v)) => set.in_black = v.min(set.in_white - MIN_GAP),
        Some(None) => set.in_black = d.in_black,
        None => {}
    }
    match handle(ui, input, x_of(set.in_white), Color32::from_gray(245), "Eingang Weiß") {
        Some(Some(v)) => set.in_white = v.max(set.in_black + MIN_GAP),
        Some(None) => set.in_white = d.in_white,
        None => {}
    }
    match handle(ui, input, mid_x(set), Color32::from_gray(128), "Mitteltöne") {
        Some(Some(v)) => {
            let p = ((v - set.in_black) / (set.in_white - set.in_black).max(1e-4)).clamp(0.01, 0.99);
            set.midtone = ((p.ln() / 0.5f32.ln()).clamp(0.2, 5.0) * 100.0).round() / 100.0;
        }
        Some(None) => set.midtone = d.midtone,
        None => {}
    }
    match handle(ui, output, x_of(set.out_black), Color32::from_gray(20), "Ausgang Schwarz") {
        Some(Some(v)) => set.out_black = v.min(set.out_white - MIN_GAP),
        Some(None) => set.out_black = d.out_black,
        None => {}
    }
    match handle(ui, output, x_of(set.out_white), Color32::from_gray(245), "Ausgang Weiß") {
        Some(Some(v)) => set.out_white = v.max(set.out_black + MIN_GAP),
        Some(None) => set.out_white = d.out_white,
        None => {}
    }

    // The values as numbers, for exact input.
    let byte = |ui: &mut Ui, v: &mut f32, lo: f32, hi: f32| {
        let mut n = (*v * 255.0).round() as i32;
        let r = ui.add(egui::DragValue::new(&mut n).range((lo * 255.0).ceil() as i32..=(hi * 255.0).floor() as i32).speed(0.5));
        if r.changed() {
            *v = n as f32 / 255.0;
        }
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Eingang").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (b, w) = (set.in_black, set.in_white);
            byte(ui, &mut set.in_white, b + MIN_GAP, 1.0);
            ui.add(egui::DragValue::new(&mut set.midtone).range(0.2..=5.0).speed(0.01).fixed_decimals(2))
                .on_hover_text("Mitteltöne: über 1 heller, unter 1 dunkler");
            byte(ui, &mut set.in_black, 0.0, w - MIN_GAP);
        });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ausgang").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (b, w) = (set.out_black, set.out_white);
            byte(ui, &mut set.out_white, b + MIN_GAP, 1.0);
            byte(ui, &mut set.out_black, 0.0, w - MIN_GAP);
        });
    });
    *set != before
}

/// Sets edge `i` of a range (falloff start, start, end, falloff end, on 0..1) to `v`; the edges
/// it runs into are pushed along, keeping the range at least `gap` wide.
fn push_edge(e: &mut [f32; 4], i: usize, v: f32, gap: f32) {
    let gaps = [0.0, gap, 0.0];
    e[i] = v.clamp(gaps[..i].iter().sum(), 1.0 - gaps[i..].iter().sum::<f32>());
    for j in (0..i).rev() {
        e[j] = e[j].min(e[j + 1] - gaps[j]);
    }
    for j in i + 1..4 {
        e[j] = e[j].max(e[j - 1] + gaps[j - 1]);
    }
}

/// What a [`range_editor`] edits: its reset values, the colors of the scale from 0 to 1, and its
/// numbers (0–255 or percent).
pub struct RangeScale {
    pub defaults: [f32; 4],
    pub colors: [Color32; 2],
    pub percent: bool,
    /// Hover text of the trapezoid.
    pub hint: &'static str,
    /// Symbols for the two ends of the scale, drawn in their colors.
    pub ends: Option<[&'static str; 2]>,
}

/// The luminance range of a mask as in Capture One.
pub fn luma_range_editor(ui: &mut Ui, r: &mut sk_core::params::LumaRange, bins: Option<&[f32]>) -> bool {
    let d = sk_core::params::LumaRange::default();
    let scale = RangeScale {
        defaults: [d.falloff_low, d.low, d.high, d.falloff_high],
        colors: [Color32::BLACK, Color32::WHITE],
        percent: false,
        hint: "Ziehen: Bereich verschieben – Doppelklick: ganzer Bereich",
        ends: None,
    };
    let mut e = [r.falloff_low, r.low, r.high, r.falloff_high];
    let changed = range_editor(ui, "luma-range", &mut e, bins, &scale);
    [r.falloff_low, r.low, r.high, r.falloff_high] = e;
    changed
}

/// The three rows of a [`range_editor`] or [`split_editor`]: the upper track for handles, the
/// graph, the lower track.
fn scale_rows(ui: &mut Ui) -> (Rect, Rect, Rect) {
    let width = ui.available_width();
    let spacing = ui.spacing().item_spacing.y;
    let (top, _) = ui.allocate_exact_size(vec2(width, 12.0), Sense::hover());
    ui.spacing_mut().item_spacing.y = 0.0;
    let (graph, _) = ui.allocate_exact_size(vec2(width, 56.0), Sense::hover());
    let (bottom, _) = ui.allocate_exact_size(vec2(width, 12.0), Sense::hover());
    ui.spacing_mut().item_spacing.y = spacing;
    (top, graph, bottom)
}

/// The graph's background with the distribution of the values (`bins`, 256 over 0..1).
fn paint_distribution(painter: &egui::Painter, graph: Rect, bins: Option<&[f32]>) {
    painter.rect_filled(graph, 3.0, theme::BROWSER);
    let Some(bins) = bins.filter(|b| b.len() >= 256) else { return };
    let max = bins[1..255].iter().copied().fold(0.0, f32::max).max(1e-6);
    let fill = Color32::from_gray(150).gamma_multiply(0.25);
    let mut mesh = egui::Mesh::default();
    for (i, &n) in bins.iter().enumerate().take(256) {
        let h = graph.height() * (n / max).sqrt().min(1.0);
        let x = graph.left() + (i as f32 + 0.5) / 256.0 * graph.width();
        mesh.colored_vertex(pos2(x, graph.bottom()), fill);
        mesh.colored_vertex(pos2(x, graph.bottom() - h), fill);
    }
    for i in 0..255u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 2, k + 3);
    }
    painter.add(mesh);
}

/// A horizontal band from `left` to `right` in `rect`, colored from `colors[0]` to `colors[1]`.
fn paint_gradient(painter: &egui::Painter, rect: Rect, colors: [Color32; 2]) {
    let mut mesh = egui::Mesh::default();
    for (x, c) in [(rect.left(), colors[0]), (rect.right(), colors[1])] {
        mesh.colored_vertex(pos2(x, rect.top()), c);
        mesh.colored_vertex(pos2(x, rect.bottom()), c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(mesh);
}

/// Symbols for the ends of the scale in the graph's upper corners, in the colors of the ends.
fn paint_ends(painter: &egui::Painter, graph: Rect, ends: Option<[&str; 2]>, colors: [Color32; 2]) {
    let Some([left, right]) = ends else { return };
    let font = egui::FontId::proportional(13.0);
    painter.text(graph.left_top() + vec2(7.0, 6.0), egui::Align2::LEFT_TOP, left, font.clone(), colors[0]);
    painter.text(graph.right_top() + vec2(-7.0, 6.0), egui::Align2::RIGHT_TOP, right, font, colors[1]);
}

/// The color of the scale at `v` (0..1).
fn scale_color(colors: [Color32; 2], v: f32) -> Color32 {
    let [a, b] = colors;
    let t = v.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// A triangle on a track pointing at the graph (`down` on the upper one), for the value `v`
/// at `x`: `Some(Some(v))` while dragged, `Some(None)` on a double click (reset).
fn track_handle(ui: &mut Ui, id: egui::Id, track: Rect, graph: Rect, v: f32, down: bool, fill: Color32, name: &str) -> Option<Option<f32>> {
    let x = graph.left() + v.clamp(0.0, 1.0) * graph.width();
    let rect = Rect::from_center_size(pos2(x, track.center().y), vec2(14.0, track.height()));
    let resp = ui.interact(rect, id.with(name), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, name));
    let resp = resp.on_hover_text(format!("{name} – Doppelklick: zurücksetzen"));
    let hot = resp.hovered() || resp.dragged();
    let s = if hot { 6.0 } else { 5.0 };
    let (tip, base) = if down { (track.bottom(), track.bottom() - s * 1.6) } else { (track.top(), track.top() + s * 1.6) };
    let tri = vec![pos2(x, tip), pos2(x + s, base), pos2(x - s, base)];
    ui.painter().add(Shape::convex_polygon(tri, fill, Stroke::new(1.0, if hot { theme::ACCENT } else { Color32::from_gray(150) })));
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if resp.double_clicked() {
        Some(None)
    } else if resp.dragged() {
        resp.interact_pointer_pos().map(|p| Some(((p.x - graph.left()) / graph.width()).clamp(0.0, 1.0)))
    } else {
        None
    }
}

/// The graph as a handle for moving everything: the shift (in 0..1) while dragged, `Some(None)`
/// on a double click (reset); and whether the pointer is on it.
fn graph_drag(ui: &mut Ui, id: egui::Id, graph: Rect, hint: &str) -> (Option<Option<f32>>, bool) {
    let body = ui.interact(graph, id.with("body"), Sense::click_and_drag());
    let hot = body.hovered() || body.dragged();
    if hot {
        ui.ctx().set_cursor_icon(if body.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
    let out = if body.double_clicked() {
        Some(None)
    } else if body.dragged() {
        Some(Some(body.drag_delta().x / graph.width()))
    } else {
        None
    };
    body.on_hover_text(hint);
    (out, hot)
}

/// A value of the scale as a number field (0–255 or percent).
fn scale_number(ui: &mut Ui, v: f32, percent: bool) -> Option<f32> {
    let max = if percent { 100 } else { 255 };
    let mut n = (v.clamp(0.0, 1.0) * max as f32).round() as i32;
    let field = egui::DragValue::new(&mut n).range(0..=max).speed(0.5).suffix(if percent { " %" } else { "" });
    ui.add(field).changed().then(|| n as f32 / max as f32)
}

/// A range on a scale from 0 to 1 (`e`: falloff start, start, end, falloff end) as a trapezoid
/// over the distribution of the values (`bins`, 256): fully in between the upper handles, fading
/// out to the lower ones. A handle pushes the ones it runs into; dragging the trapezoid moves
/// all four; double-click resets.
pub fn range_editor(ui: &mut Ui, salt: &str, e: &mut [f32; 4], bins: Option<&[f32]>, scale: &RangeScale) -> bool {
    let gap = if scale.percent { 0.01 } else { 1.0 / 255.0 };
    let before = *e;
    let (top, graph, bottom) = scale_rows(ui);
    let x_of = |v: f32| graph.left() + v.clamp(0.0, 1.0) * graph.width();
    let id = ui.id().with(salt);

    let (moved, hot) = graph_drag(ui, id, graph, scale.hint);
    match moved {
        Some(Some(d)) => {
            let d = d.clamp(-e[0].min(e[1]), 1.0 - e[3].max(e[2]));
            for v in e.iter_mut() {
                *v += d;
            }
        }
        Some(None) => *e = scale.defaults,
        None => {}
    }

    let painter = ui.painter();
    paint_distribution(painter, graph, bins);
    let shape = vec![pos2(x_of(e[0]), graph.bottom()), pos2(x_of(e[1]), graph.top()), pos2(x_of(e[2]), graph.top()), pos2(x_of(e[3]), graph.bottom())];
    let edge = if hot { theme::ACCENT } else { theme::ACCENT.gamma_multiply(0.8) };
    painter.add(Shape::convex_polygon(shape, theme::ACCENT.gamma_multiply(if hot { 0.35 } else { 0.25 }), Stroke::new(1.0, edge)));
    for v in [e[1], e[2]] {
        painter.vline(x_of(v), graph.y_range(), Stroke::new(1.0, Color32::from_white_alpha(110)));
    }
    paint_gradient(painter, Rect::from_min_max(pos2(graph.left(), graph.bottom() - 4.0), graph.max), scale.colors);
    paint_ends(painter, graph, scale.ends, scale.colors);

    for (i, track, down, name) in [(1, top, true, "Bereich Anfang"), (2, top, true, "Bereich Ende"), (0, bottom, false, "Übergang Anfang"), (3, bottom, false, "Übergang Ende")] {
        if let Some(v) = track_handle(ui, id, track, graph, e[i], down, scale_color(scale.colors, e[i]), name) {
            push_edge(e, i, v.unwrap_or(scale.defaults[i]), gap);
        }
    }

    // The values as numbers, for exact input; they push the others too.
    ui.add_space(4.0);
    for (label, [left, right]) in [("Bereich", [1, 2]), ("Übergang", [0, 3])] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for i in [right, left] {
                    if let Some(v) = scale_number(ui, e[i], scale.percent) {
                        push_edge(e, i, v, gap);
                    }
                }
            });
        });
    }
    *e != before
}

/// A division of a scale from 0 to 1 into two parts, at `split` with a transition `width` wide,
/// over the distribution of the values (`bins`, 256): the line of the split with a handle above,
/// the transition's ends with handles below, the parts in their `colors` (from 0 and from 1).
/// Dragging the graph moves the split; double-click resets to `defaults` (split, width).
#[allow(clippy::too_many_arguments)]
pub fn split_editor(
    ui: &mut Ui,
    salt: &str,
    split: &mut f32,
    width: &mut f32,
    bins: Option<&[f32]>,
    colors: [Color32; 2],
    ends: Option<[&str; 2]>,
    defaults: (f32, f32),
    min_width: f32,
) -> bool {
    let before = (*split, *width);
    let (top, graph, bottom) = scale_rows(ui);
    let x_of = |v: f32| graph.left() + v.clamp(0.0, 1.0) * graph.width();
    let id = ui.id().with(salt);

    let (moved, hot) = graph_drag(ui, id, graph, "Ziehen: Grenze verschieben – Doppelklick: zurücksetzen");
    match moved {
        Some(Some(d)) => *split = (*split + d).clamp(0.0, 1.0),
        Some(None) => (*split, *width) = defaults,
        None => {}
    }

    let painter = ui.painter();
    paint_distribution(painter, graph, bins);
    // The parts, with the transition from one to the other.
    let (a, b) = (*split - *width / 2.0, *split + *width / 2.0);
    let tint = |c: Color32| c.gamma_multiply(if hot { 0.30 } else { 0.22 });
    let (xa, xb) = (x_of(a), x_of(b));
    painter.rect_filled(Rect::from_min_max(graph.min, pos2(xa, graph.bottom())), 0.0, tint(colors[0]));
    painter.rect_filled(Rect::from_min_max(pos2(xb, graph.top()), graph.max), 0.0, tint(colors[1]));
    if xb > xa {
        paint_gradient(painter, Rect::from_min_max(pos2(xa, graph.top()), pos2(xb, graph.bottom())), [tint(colors[0]), tint(colors[1])]);
    }
    let dashed = Stroke::new(1.0, Color32::from_white_alpha(120));
    for x in [xa, xb] {
        painter.extend(Shape::dashed_line(&[pos2(x, graph.top()), pos2(x, graph.bottom())], dashed, 3.0, 3.0));
    }
    painter.vline(x_of(*split), graph.y_range(), Stroke::new(1.5, if hot { theme::ACCENT } else { Color32::from_white_alpha(200) }));
    paint_gradient(painter, Rect::from_min_max(pos2(graph.left(), graph.bottom() - 4.0), graph.max), colors);
    paint_ends(painter, graph, ends, colors);

    // The ends of the transition move one at a time; the split stays in its middle.
    let set_ends = |split: &mut f32, width: &mut f32, a: f32, b: f32| {
        *width = (b - a).max(min_width);
        *split = (a + b) / 2.0;
    };
    if let Some(v) = track_handle(ui, id, top, graph, *split, true, Color32::from_gray(230), "Grenze") {
        *split = v.unwrap_or(defaults.0);
    }
    if let Some(v) = track_handle(ui, id, bottom, graph, a, false, scale_color(colors, a), "Übergang Anfang") {
        let a = v.map_or(*split - defaults.1 / 2.0, |v| v.min(b - min_width));
        set_ends(split, width, a, b);
    }
    let a = *split - *width / 2.0;
    if let Some(v) = track_handle(ui, id, bottom, graph, b, false, scale_color(colors, b), "Übergang Ende") {
        let b = v.map_or(*split + defaults.1 / 2.0, |v| v.max(a + min_width));
        set_ends(split, width, a, b);
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Grenze").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(v) = scale_number(ui, *split, true) {
                *split = v;
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Übergang").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(v) = scale_number(ui, *width, true) {
                *width = v.max(min_width);
            }
        });
    });
    (*split, *width) != before
}

/// `bins` (256, over 0..1) after the monotonic `f`: each bin spread over the range it lands in.
pub fn remap_bins(bins: &[u32], f: impl Fn(f32) -> f32) -> Vec<f32> {
    let mut out = vec![0.0f32; 256];
    for (i, &n) in bins.iter().enumerate().take(256) {
        if n == 0 {
            continue;
        }
        let lo = f(i as f32 / 256.0).clamp(0.0, 1.0) * 256.0;
        let hi = (f((i + 1) as f32 / 256.0).clamp(0.0, 1.0) * 256.0).max(lo + 1e-3);
        let (a, b) = (lo.floor() as usize, (hi.ceil() as usize).min(256));
        for (j, slot) in out.iter_mut().enumerate().take(b).skip(a) {
            let overlap = (hi.min((j + 1) as f32) - lo.max(j as f32)).max(0.0);
            *slot += n as f32 * overlap / (hi - lo);
        }
    }
    out
}

/// A point added to a curve: its x stays inside the borders, which belong to the endpoints.
fn inner_point(p: [f32; 2]) -> [f32; 2] {
    [p[0].clamp(0.001, 0.999), p[1]]
}

/// Interactive curve editor. Click to add a point, drag to move, right-click to remove.
pub fn curve_editor(ui: &mut Ui, curve: &mut Curve, color: Color32, hist: Option<&[u32]>) -> bool {
    let size = ui.available_width().min(280.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(4.0));
    painter.rect_filled(rect, 2.0, Color32::from_gray(22));
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let s = Stroke::new(1.0, Color32::from_gray(45));
        painter.line_segment([pos2(rect.left() + rect.width() * t, rect.top()), pos2(rect.left() + rect.width() * t, rect.bottom())], s);
        painter.line_segment([pos2(rect.left(), rect.top() + rect.height() * t), pos2(rect.right(), rect.top() + rect.height() * t)], s);
    }
    if let Some(bins) = hist {
        let max = bins[1..255].iter().copied().max().unwrap_or(1).max(1) as f32;
        for (i, &v) in bins.iter().enumerate() {
            let x = rect.left() + rect.width() * i as f32 / 255.0;
            let h = rect.height() * (v as f32 / max).sqrt().min(1.0) * 0.6;
            painter.line_segment([pos2(x, rect.bottom()), pos2(x, rect.bottom() - h)], Stroke::new(1.5, Color32::from_gray(38)));
        }
    }
    painter.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_gray(60)));

    let to_screen = |p: [f32; 2]| pos2(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.height());
    let from_screen = |p: Pos2| {
        [((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0), ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0)]
    };

    // A new point stays off the borders: there it would be an endpoint the curve drops.
    let new_point = |p: Pos2| inner_point(from_screen(p));

    let mut changed = false;
    let drag_id = response.id.with("drag_point");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    let hit = |pos: Pos2, pts: &[[f32; 2]]| pts.iter().position(|p| to_screen(*p).distance(pos) < 8.0);
    // Grab a point, or click elsewhere to add one.
    if response.dragged() && dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if let Some(pos) = response.hover_pos() {
        ui.ctx().set_cursor_icon(if hit(pos, &curve.points).is_some() { egui::CursorIcon::Grab } else { egui::CursorIcon::Crosshair });
    }

    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos()
    {
        dragging = hit(pos, &curve.points).or_else(|| {
            curve.points.push(new_point(pos));
            changed = true;
            Some(curve.points.len() - 1)
        });
    }
    if response.dragged()
        && let (Some(i), Some(pos)) = (dragging, response.interact_pointer_pos())
        && i < curve.points.len()
    {
        let mut p = from_screen(pos);
        // Endpoints stay on the left/right border and only move vertically.
        let x = curve.points[i][0];
        if x == 0.0 || x == 1.0 {
            p[0] = x;
        } else {
            p[0] = p[0].clamp(0.001, 0.999);
        }
        if curve.points[i] != p {
            curve.points[i] = p;
            changed = true;
        }
    }
    if response.drag_stopped() {
        dragging = None;
    }
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
        && hit(pos, &curve.points).is_none()
    {
        curve.points.push(new_point(pos));
        changed = true;
    }
    if response.secondary_clicked()
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(i) = hit(pos, &curve.points)
        && curve.points[i][0] > 0.0
        && curve.points[i][0] < 1.0
    {
        curve.points.remove(i);
        changed = true;
    }
    ui.data_mut(|d| match dragging {
        Some(i) => {
            d.insert_temp(drag_id, i);
        }
        None => d.remove::<usize>(drag_id),
    });

    let lut = curve.build_lut(128);
    let line: Vec<Pos2> = lut.iter().enumerate().map(|(i, &y)| to_screen([i as f32 / 127.0, y])).collect();
    painter.add(Shape::line(line, Stroke::new(2.0, color)));
    for p in &curve.points {
        painter.circle_filled(to_screen(*p), 4.0, color);
        painter.circle_stroke(to_screen(*p), 4.0, Stroke::new(1.0, Color32::BLACK));
    }
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(70)), StrokeKind::Inside);
    response.on_hover_text("Klicken: Punkt hinzufügen · Ziehen: verschieben · Rechtsklick: entfernen");
    changed
}

/// Star rating widget (0..5). Clicking the current rating clears it.
pub fn stars(ui: &mut Ui, rating: &mut u8, size: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for i in 1..=5u8 {
            let filled = i <= *rating;
            let text = egui::RichText::new(if filled { "★" } else { "☆" })
                .size(size)
                .color(if filled { theme::STAR } else { Color32::from_gray(110) });
            if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() {
                *rating = if *rating == i { 0 } else { i };
                changed = true;
            }
        }
    });
    changed
}

/// Paints a small rating + color label overlay inside `rect`.
pub fn paint_badges(ui: &Ui, rect: Rect, rating: u8, label: Option<[u8; 3]>) {
    let painter = ui.painter_at(rect);
    if rating > 0 {
        painter.text(
            rect.left_bottom() + vec2(4.0, -3.0),
            egui::Align2::LEFT_BOTTOM,
            "★".repeat(rating as usize),
            egui::FontId::proportional(11.0),
            theme::STAR,
        );
    }
    if let Some([r, g, b]) = label {
        painter.circle_filled(rect.right_bottom() + vec2(-8.0, -8.0), 5.0, Color32::from_rgb(r, g, b));
    }
}

/// Clickable color label swatch.
pub fn color_label_dot(ui: &mut Ui, label: sk_core::sidecar::ColorLabel, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let painter = ui.painter_at(rect);
    let c = rect.center();
    match label.rgb() {
        Some([r, g, b]) => {
            painter.circle_filled(c, 7.0, Color32::from_rgb(r, g, b));
        }
        None => {
            painter.circle_stroke(c, 6.5, Stroke::new(1.5, Color32::from_gray(140)));
            painter.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], Stroke::new(1.5, Color32::from_gray(140)));
        }
    }
    if selected || resp.hovered() {
        painter.circle_stroke(c, 10.0, Stroke::new(1.5, if selected { Color32::WHITE } else { Color32::from_gray(120) }));
    }
    resp.on_hover_text(label.label())
}

pub fn small_button(ui: &mut Ui, text: &str, hover: &str) -> Response {
    ui.add(egui::Button::new(text).small()).on_hover_text(hover)
}

/// "1 Bild" / "n Bilder".
pub fn image_count(n: usize) -> String {
    if n == 1 { "1 Bild".to_string() } else { format!("{n} Bilder") }
}

/// "Abbrechen" next to a progress bar: sets `cancel` (the work stops at its next step).
pub fn cancel_button(ui: &mut Ui, cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let stopping = cancel.load(std::sync::atomic::Ordering::Relaxed);
    if ui.add_enabled(!stopping, egui::Button::new(if stopping { "Wird abgebrochen …" } else { "Abbrechen" }).small()).clicked() {
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// [`image_count`] after "von": "1 Bild", "3 Bildern".
pub fn images_dative(n: usize) -> String {
    if n == 1 { "1 Bild".to_string() } else { format!("{n} Bildern") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_edges_push_each_other() {
        let gap = 1.0 / 255.0;
        let mut e = [0.0, 70.0 / 255.0, 239.0 / 255.0, 239.0 / 255.0];
        // The end of the falloff dragged left takes the range's end along, and then its start.
        push_edge(&mut e, 3, 0.5, gap);
        assert_eq!((e[2], e[3]), (0.5, 0.5));
        push_edge(&mut e, 3, 0.1, gap);
        assert!((e[1] - (0.1 - gap)).abs() < 1e-6 && e[2] == 0.1, "{e:?}");
        assert!(e[0] <= e[1]);
        // From the other side, up to the last levels.
        push_edge(&mut e, 0, 1.0, gap);
        assert!(e[0] <= e[1] && e[1] < e[2] && e[2] <= e[3] && e[3] <= 1.0, "{e:?}");
        assert!((e[2] - 1.0).abs() < 1e-6 && (e[1] - (1.0 - gap)).abs() < 1e-6, "{e:?}");
    }

    /// The colors of the hue bands look like their names: red is red, not pink.
    #[test]
    fn band_colors_match_their_hues() {
        for (name, hue) in sk_core::params::HUE_BANDS {
            println!("{name}: {:?}", band_hsv(hue, 0.75, 0.9));
        }
        // sRGB red has Oklab hue 29°, yellow 110°, blue 264°.
        let near = |a: Color32, b: [u8; 3]| [a.r(), a.g(), a.b()].iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 4);
        assert!(near(band_hsv(29.2, 1.0, 1.0), [255, 0, 0]));
        assert!(near(band_hsv(109.8, 1.0, 1.0), [255, 255, 0]));
        // Near blue the Oklab hue hardly changes towards cyan: a slightly cyan blue may come.
        let blue = band_hsv(264.1, 1.0, 1.0);
        assert!(blue.b() == 255 && blue.r() < 10 && blue.g() < 80, "{blue:?}");
        let red = band_hsv(sk_core::params::HUE_BANDS[0].1, 0.75, 0.9);
        assert!(red.r() > 200 && red.g() < 80 && red.b() < 100, "{red:?}");
    }

    /// A point clicked on the border becomes an inner point, not a second endpoint.
    #[test]
    fn new_curve_points_stay_inside() {
        assert_eq!(inner_point([0.0, 0.3]), [0.001, 0.3]);
        assert_eq!(inner_point([1.0, 0.7]), [0.999, 0.7]);
        assert_eq!(inner_point([0.5, 1.0]), [0.5, 1.0]);
    }

    /// Key and wheel steps count as one gesture until they pause.
    #[test]
    fn steps_are_pending_until_they_pause() {
        let ctx = egui::Context::default();
        let run = |t: f64, f: &dyn Fn(&egui::Context)| {
            ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| f(ui.ctx())).textures_delta.clear();
        };
        let id = egui::Id::new("slider");
        run(0.0, &|ctx| assert_eq!(take_steps(ctx), None));
        run(1.0, &|ctx| {
            mark_stepped(ctx, id);
            assert!(take_steps(ctx).is_some_and(|s| s.now && s.source == id));
        });
        run(1.5, &|ctx| assert!(take_steps(ctx).is_some_and(|s| !s.now)));
        run(1.0 + STEP_PAUSE + 0.1, &|ctx| assert_eq!(take_steps(ctx), None));
        run(3.0, &|ctx| {
            mark_stepped(ctx, id);
            end_steps(ctx);
            assert_eq!(take_steps(ctx), None);
        });
    }

    /// One frame with a slider row; returns the slider's screen rect (via AccessKit).
    fn frame(ctx: &egui::Context, value: &mut f32, events: Vec<egui::Event>) -> Option<egui::Rect> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 100.0))),
            events,
            ..Default::default()
        };
        ctx.enable_accesskit();
        let mut out = ctx.run_ui(input, |ui| {
            slider_row(ui, "Belichtung", value, -4.0..=4.0, 0.0, 2);
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update?;
        update.nodes.iter().find(|(_, n)| n.role() == egui::accesskit::Role::Slider).and_then(|(_, n)| {
            let b = n.bounds()?;
            Some(egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))
        })
    }

    #[test]
    fn double_click_on_slider_resets_it() {
        let ctx = egui::Context::default();
        let mut value = 1.5;
        let slider = frame(&ctx, &mut value, vec![]).expect("Regler");
        // Off-center: a single click there moves the value away from the default.
        let pos = slider.lerp_inside(vec2(0.75, 0.5));
        let click = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, &mut value, vec![egui::Event::PointerMoved(pos)]);
        for _ in 0..2 {
            frame(&ctx, &mut value, vec![click(true)]);
            frame(&ctx, &mut value, vec![click(false)]);
        }
        frame(&ctx, &mut value, vec![]);
        assert_eq!(value, 0.0);
    }
}

/// Clipping triangles in the top corners of a histogram (shadows left, highlights right):
/// lit when the image clips there, filled while the warning is shown. True when clicked.
pub fn clip_triangles(ui: &mut Ui, rect: Rect, hist: Option<&Histogram>, warning_on: bool) -> bool {
    // Share of pixels in the outermost bin of any channel.
    let clipped = |bin: usize| {
        hist.is_some_and(|h| {
            let total: u32 = h.luma.iter().sum::<u32>().max(1);
            [&h.r, &h.g, &h.b].iter().any(|c| c.get(bin).is_some_and(|&n| n as f32 / total as f32 > 0.001))
        })
    };
    let mut clicked = false;
    for (left, bin, hover) in [(true, 0, "Abgesoffene Tiefen"), (false, 255, "Ausgefressene Lichter")] {
        let size = vec2(16.0, 14.0);
        let r = if left {
            Rect::from_min_size(rect.min + vec2(4.0, 4.0), size)
        } else {
            Rect::from_min_size(pos2(rect.right() - 4.0 - size.x, rect.top() + 4.0), size)
        };
        let resp = ui.interact(r, ui.id().with(("clip_triangle", left)), Sense::click());
        let is_clipped = clipped(bin);
        let color = if warning_on {
            theme::ACCENT
        } else if is_clipped {
            if left { Color32::from_rgb(110, 160, 255) } else { theme::DANGER }
        } else if resp.hovered() {
            theme::TEXT_DIM
        } else {
            Color32::from_gray(80)
        };
        let c = r.shrink(2.0);
        let pts = if left {
            vec![c.left_top(), c.right_top(), c.left_bottom()]
        } else {
            vec![c.left_top(), c.right_top(), c.right_bottom()]
        };
        let stroke = Stroke::new(1.2, color);
        if warning_on || is_clipped {
            ui.painter().add(Shape::convex_polygon(pts, color, stroke));
        } else {
            ui.painter().add(Shape::closed_line(pts, stroke));
        }
        let state = if is_clipped { "vorhanden" } else { "keine" };
        if resp.on_hover_text(format!("{hover}: {state}\nKlick: {}", crate::commands::hint("Belichtungswarnung im Bild", "clip-warning"))).clicked() {
            clicked = true;
        }
    }
    clicked
}

/// Small heading inside a section, e.g. "Eingrenzen" within "Maske".
pub(crate) fn subheading(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).size(11.5).strong().color(crate::theme::TEXT_DIM));
}
