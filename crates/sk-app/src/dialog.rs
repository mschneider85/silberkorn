//! Modal dialogs in one style: a header with the icon on a tile, the title and a line below it;
//! the content, grouped in cards; the buttons in a footer below a hairline.

use egui::{Button, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui, vec2};

use crate::theme;

/// Inner margin of the content (the footer's hairline reaches across it).
const PAD: f32 = 22.0;
const FILL: Color32 = Color32::from_rgb(37, 37, 41);
/// Cards inside the dialog.
const CARD: Color32 = Color32::from_rgb(30, 30, 33);
const CARD_BORDER: Color32 = Color32::from_rgb(52, 52, 58);
const BUTTON: egui::Vec2 = vec2(96.0, 30.0);

/// The color of a status, a pill or an icon tile.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Neutral (the accent).
    Accent,
    Good,
    Warn,
    Danger,
    Off,
}

impl Tone {
    fn color(self) -> Color32 {
        match self {
            Tone::Accent => theme::ACCENT,
            Tone::Good => Color32::from_rgb(98, 196, 128),
            Tone::Warn => theme::AI,
            Tone::Danger => theme::DANGER,
            Tone::Off => theme::TEXT_FAINT,
        }
    }
}

pub struct Dialog {
    id: &'static str,
    icon: &'static str,
    title: String,
    subtitle: Option<String>,
    width: f32,
    tone: Tone,
}

impl Dialog {
    pub fn new(id: &'static str, icon: &'static str, title: impl Into<String>) -> Self {
        Self { id, icon, title: title.into(), subtitle: None, width: 440.0, tone: Tone::Accent }
    }

    /// A line below the title: what the dialog is about.
    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// The icon tile's color (red for what deletes).
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Shows the dialog; `content` draws the body and ends with [`footer`].
    pub fn show<T>(self, ctx: &egui::Context, content: impl FnOnce(&mut Ui) -> T) -> egui::ModalResponse<T> {
        let frame = Frame::new()
            .fill(FILL)
            .stroke(Stroke::new(1.0, Color32::from_rgb(64, 64, 71)))
            .corner_radius(CornerRadius::same(14))
            .shadow(egui::Shadow { offset: [0, 16], blur: 48, spread: 0, color: Color32::from_black_alpha(160) })
            .inner_margin(Margin::symmetric(PAD as i8, 20));
        egui::Modal::new(egui::Id::new(self.id)).frame(frame).backdrop_color(Color32::from_black_alpha(140)).show(ctx, |ui| {
            ui.set_width(self.width);
            ui.spacing_mut().item_spacing.y = 6.0;
            header(ui, self.icon, &self.title, self.subtitle.as_deref(), self.tone);
            ui.add_space(14.0);
            content(ui)
        })
    }
}

fn header(ui: &mut Ui, icon: &str, title: &str, subtitle: Option<&str>, tone: Tone) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(38.0, 38.0), egui::Sense::hover());
        let c = tone.color();
        ui.painter().rect_filled(rect, 10, c.gamma_multiply(0.18));
        ui.painter().rect_stroke(rect, 10, Stroke::new(1.0, c.gamma_multiply(0.35)), egui::StrokeKind::Inside);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(19.0), c);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if subtitle.is_none() {
                ui.add_space(8.0);
            }
            ui.label(RichText::new(title).size(17.0).strong().color(Color32::WHITE));
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).size(12.0).color(theme::TEXT_DIM));
            }
        });
    });
}

/// A group of settings: a framed card with a title and, on the right, a status.
pub fn card(ui: &mut Ui, title: &str, status: Option<(Tone, &str)>, body: impl FnOnce(&mut Ui)) {
    Frame::new().fill(CARD).stroke(Stroke::new(1.0, CARD_BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 7.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(13.5).strong().color(theme::TEXT));
            if let Some((tone, text)) = status {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| pill(ui, tone, text));
            }
        });
        body(ui);
    });
    ui.add_space(4.0);
}

/// A short status with a colored dot.
pub fn pill(ui: &mut Ui, tone: Tone, text: &str) {
    let c = tone.color();
    Frame::new().fill(c.gamma_multiply(0.14)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
        // Dot and text as one piece, so that the dot stays in front also in a right-to-left
        // row. The dot is painted: egui's fonts have no filled circle (it showed as a box).
        let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(11.5), c.lerp_to_gamma(Color32::WHITE, 0.35));
        let (rect, _) = ui.allocate_exact_size(vec2(11.0 + galley.size().x, galley.size().y.max(16.0)), egui::Sense::hover());
        ui.painter().circle_filled(egui::pos2(rect.left() + 3.0, rect.center().y), 3.0, c);
        ui.painter().galley(egui::pos2(rect.left() + 11.0, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    });
}

/// A neutral label (a license, a format).
pub fn tag(ui: &mut Ui, text: &str) {
    Frame::new().fill(Color32::from_rgb(48, 48, 54)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
        ui.label(RichText::new(text).size(11.5).color(theme::TEXT));
    });
}

/// Explanations and details: smaller and dimmer than the content.
pub fn note(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).size(12.0).color(theme::TEXT_DIM));
}

/// Text in a sunken box (release notes, previews).
pub fn well<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new().fill(theme::BROWSER).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui)
    })
    .inner
}

/// The buttons, right to left (the first is the rightmost), below a hairline across the dialog.
pub fn footer(ui: &mut Ui, buttons: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    let y = ui.cursor().top();
    let x = ui.max_rect().x_range().expand(PAD);
    ui.painter().hline(x, y, Stroke::new(1.0, Color32::from_rgb(54, 54, 60)));
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        buttons(ui);
    });
}

/// The dialog's main action, which Enter also runs (Esc closes the dialog, see
/// `ModalResponse::should_close`). Not for what deletes: an Enter too many must not destroy.
/// Enter belongs to another widget that has the focus ("Abbrechen" presses itself, a text
/// field ends its input), also when that field gave the focus up earlier in this frame.
pub fn default_button(ui: &mut Ui, enabled: bool, button: Button<'static>) -> bool {
    let response = ui.add_enabled(enabled, button);
    let others = other_focus(ui.ctx(), response.id);
    response.clicked() || (enabled && !others && ui.input(|i| i.key_pressed(egui::Key::Enter)))
}

/// Focus at the end of the last frame and now, kept for `default_button`.
#[derive(Clone, Copy, Default)]
struct FocusTrace {
    frame: u64,
    before: Option<egui::Id>,
    now: Option<egui::Id>,
}

/// Whether a widget other than `own` has the focus now or had it at the end of the last frame.
fn other_focus(ctx: &egui::Context, own: egui::Id) -> bool {
    let key = egui::Id::new("dialog-focus-trace");
    let frame = ctx.cumulative_frame_nr();
    let now = ctx.memory(|m| m.focused());
    let trace: FocusTrace = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
    let before = match frame.checked_sub(trace.frame) {
        Some(0) => trace.before,
        Some(1) => trace.now,
        _ => None,
    };
    ctx.data_mut(|d| d.insert_temp(key, FocusTrace { frame, before, now }));
    focused_elsewhere(before, now, own)
}

fn focused_elsewhere(before: Option<egui::Id>, now: Option<egui::Id>, own: egui::Id) -> bool {
    [before, now].into_iter().flatten().any(|id| id != own)
}

/// The main action: filled with the accent.
pub fn primary(text: impl Into<String>) -> Button<'static> {
    Button::new(RichText::new(text.into()).color(Color32::WHITE).strong()).fill(theme::ACCENT).corner_radius(8).min_size(BUTTON)
}

/// What deletes: filled red.
pub fn danger(text: impl Into<String>) -> Button<'static> {
    Button::new(RichText::new(text.into()).color(Color32::WHITE).strong()).fill(theme::DANGER_FILL).corner_radius(8).min_size(BUTTON)
}

/// Every other button of the footer.
pub fn secondary(text: impl Into<String>) -> Button<'static> {
    Button::new(text.into()).corner_radius(8).min_size(BUTTON)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_stays_with_a_focused_widget() {
        let (own, other) = (egui::Id::new("own"), egui::Id::new("other"));
        assert!(!focused_elsewhere(None, None, own));
        assert!(!focused_elsewhere(Some(own), Some(own), own));
        // "Abbrechen" focused.
        assert!(focused_elsewhere(None, Some(other), own));
        // A text field that ended its input with this very Enter.
        assert!(focused_elsewhere(Some(other), None, own));
    }
}
