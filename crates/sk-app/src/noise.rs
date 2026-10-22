//! The "Rauschreduzierung" section: the classic noise reduction (KI-Entrauschen is in
//! "Verbessern", see `ai_products`).

use sk_core::params::Detail;

use sk_core::pipeline::SliderView;

use crate::app::App;
use crate::widgets::{SliderOpts, slider_row_with};

/// Title of the section in the Details panel.
pub const NOISE_SECTION: &str = "Rauschreduzierung";

impl App {
    /// The "Rauschreduzierung" section of the Details panel; returns whether something changed,
    /// and what to show while a slider is held with Strg.
    pub(crate) fn noise_section(&mut self, ui: &mut egui::Ui) -> (bool, Option<SliderView>) {
        let Some(cur) = self.current.as_mut() else { return (false, None) };
        let d = &mut cur.adjustments.detail;
        let defaults = Detail::default();
        let (mut reset, mut changed) = (false, false);
        let help = "Luminanz glättet das Helligkeitsrauschen, Farbe die bunten Flecken. Auch zusätzlich zum KI-Entrauschen (Abschnitt „Verbessern“).\n\n\
                    Strg beim Ziehen zeigt, was entfernt wird: Körnchen und Farbflecken sind richtig, erkennbare Umrisse heißen, es geht zu weit.";
        let held = std::cell::Cell::new(false);
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::NoiseReduction, help, Some(&mut || reset = true), |ui| {
            let opts = SliderOpts { held_with_ctrl: Some(&held), ..Default::default() };
            changed |= slider_row_with(ui, "Luminanz", &mut d.nr_luma, 0.0..=100.0, defaults.nr_luma, 0, opts);
            changed |= slider_row_with(ui, "Farbe", &mut d.nr_color, 0.0..=100.0, defaults.nr_color, 0, opts);
        });
        if reset {
            (d.nr_luma, d.nr_color) = (defaults.nr_luma, defaults.nr_color);
            changed = true;
        }
        (changed, held.get().then_some(SliderView::NoiseRemoved))
    }
}
