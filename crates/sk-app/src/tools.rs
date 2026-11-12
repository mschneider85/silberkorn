//! Tool tabs: Library, Exposure, Color, Effects, Metadata, Output.

use sk_core::params::{BaseCurve, Curve, Levels, WbMode, WhiteBalance};
use sk_core::pipeline::SliderView;
use egui::{Color32, Ui};

use crate::app::{App, CurveChannel, Tab};
use crate::layer_target::Target;
use crate::widgets::{self, slider_row};

/// The crosshair of the Zielkorrektur (selected while on); returns whether it was clicked.
pub(crate) fn target_button(ui: &mut Ui, on: bool, hint: &str) -> bool {
    ui.add(egui::Button::new(crate::theme::icon::CROSSHAIR_SIMPLE).selected(on)).on_hover_text(hint).clicked()
}

impl App {
    pub fn tools_ui(&mut self, ui: &mut Ui) {
        // The copy buttons of the tool headers carry a tool over to the other selected images.
        let others = if self.current.is_some() { self.selected_count().saturating_sub(1) } else { 0 };
        widgets::set_copy_targets(ui.ctx(), others);
        widgets::set_edit_layer(ui.ctx(), self.on_layer());
        widgets::set_tool_states(ui.ctx(), self.tool_states());
        widgets::set_auto_state(ui.ctx(), widgets::AutoState { busy: self.auto_progress.is_some(), lens_db: self.lens_db.is_some() });
        self.tool_tabs(ui);
        if let Some(tool) = widgets::take_auto_clicked(ui.ctx()) {
            self.auto_tool(tool);
        }
        if let Some(tool) = widgets::take_copy_clicked(ui.ctx()) {
            self.transfer_tool(tool);
        }
        if let Some(tool) = widgets::take_eye_clicked(ui.ctx()) {
            self.edit(|a| a.toggle_hidden(tool));
        }
        // Once begun, leaving a tool out lasts until the mouse button is let go.
        let held = self.tool_peek.filter(|_| ui.input(|i| i.pointer.primary_down()));
        let peek = widgets::take_tool_peek(ui.ctx()).or(held);
        if peek != self.tool_peek {
            // Tools that move the image would leave the tiles of before in the wrong place.
            use sk_core::tools::Tool;
            let moves = |t: Option<Tool>| matches!(t, Some(Tool::LensCorrection | Tool::Crop | Tool::Rotation | Tool::Perspective));
            let stale_ok = !moves(peek) && !moves(self.tool_peek);
            let (starting, ending) = (self.tool_peek.is_none(), peek.is_none());
            if starting && stale_ok {
                self.keep_tiles();
            }
            self.tool_peek = peek;
            self.request_render();
            self.tiles_stale_ok = stale_ok;
            // Let go: the image as before, at once (only the preview is rendered again).
            if ending {
                self.restore_kept_tiles();
            }
        }
    }

    /// The switched-off tools of the current image, for the eyes in the headers.
    fn tool_states(&self) -> widgets::ToolStates {
        let Some(cur) = &self.current else { return Default::default() };
        widgets::ToolStates { hidden: cur.adjustments.hidden.clone() }
    }

    fn tool_tabs(&mut self, ui: &mut Ui) {
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        // Screenshots of sections further down (`SILBERKORN_SCROLL`: points).
        if std::env::var_os("SILBERKORN_SCREENSHOT").is_some()
            && let Some(y) = std::env::var("SILBERKORN_SCROLL").ok().and_then(|v| v.parse().ok())
        {
            area = area.vertical_scroll_offset(y);
        }
        area.show(ui, |ui| {
            self.tool_tab(ui);
        });
    }

    fn tool_tab(&mut self, ui: &mut Ui) {
        match self.tab {
            Tab::Library => self.library_tab(ui),
            Tab::Lens => self.lens_tab(ui),
            Tab::Details => self.details_tab(ui),
            Tab::Effects => self.effects_tab(ui),
            Tab::Layers => self.layers_tab(ui),
            Tab::Retouch => self.retouch_tab(ui),
            Tab::Styles => self.styles_tab(ui),
            Tab::History => self.history_tab(ui),
            Tab::Exposure => self.exposure_tab(ui),
            Tab::Color => self.color_tab(ui),
            Tab::Metadata => self.metadata_tab(ui),
            Tab::Output => self.output_tab(ui),
        }
    }

    fn exposure_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            widgets::no_image(ui);
            return;
        }
        // With the right column open, the histogram is shown there.
        if !self.side_column_shown {
            let r = self.histogram_widget(ui);
            if widgets::clip_triangles(ui, r, self.histogram.as_ref(), self.clip_warning) {
                self.toggle_clip_warning();
            }
            ui.add_space(4.0);
        }
        // Automatic adjustment sets the whole image.
        widgets::background_only(ui, |ui| self.auto_section(ui));
        let Some(cur) = self.current.as_mut() else {
            crate::widgets::no_image(ui);
            return;
        };
        let adj = &mut cur.adjustments;
        let mut t = Target::new(adj, self.active_layer);
        let mut changed = false;

        // Held with Alt: where highlights or shadows clip (see `SliderView`).
        let (high, low) = (std::cell::Cell::new(false), std::cell::Cell::new(false));
        // Under the mouse: its range is marked in the histogram.
        let zones = widgets::ToneZone::ALL.map(|_| std::cell::Cell::new(false));
        let over = |z: widgets::ToneZone| Some(&zones[widgets::ToneZone::ALL.iter().position(|x| *x == z).unwrap_or(0)]);
        let mut reset_exposure = false;
        widgets::tool_section(ui, sk_core::tools::Tool::Exposure, Some(&mut || reset_exposure = true), |ui| {
            let tone = widgets::SliderOpts { rail: Some(widgets::RAIL_TONE), ..Default::default() };
            let ev = widgets::SliderOpts { suffix: " EV", held_with_alt: Some(&high), hovered: over(widgets::ToneZone::Exposure), ..tone };
            changed |= widgets::slider_row_with(ui, "Belichtung", t.pick(&mut adj.exposure, |l| &mut l.exposure), -4.0..=4.0, 0.0, 2, ev);
            changed |= slider_row(ui, "Kontrast", t.pick(&mut adj.contrast, |l| &mut l.contrast), -100.0..=100.0, 0.0, 0);
            changed |= widgets::slider_row_with(ui, "Helligkeit", t.pick(&mut adj.brightness, |l| &mut l.brightness), -100.0..=100.0, 0.0, 0, tone);
        });
        if reset_exposure {
            match t.layer_mut() {
                Some(l) => (l.exposure, l.contrast, l.brightness) = (0.0, 0.0, 0.0),
                None => (adj.exposure, adj.contrast, adj.brightness) = (0.0, 0.0, 0.0),
            }
            changed = true;
        }

        let mut reset_hdr = false;
        widgets::tool_section(ui, sk_core::tools::Tool::HighlightsShadows, Some(&mut || reset_hdr = true), |ui| {
            let tone = widgets::SliderOpts { rail: Some(widgets::RAIL_TONE), ..Default::default() };
            let (hi, lo) = (widgets::SliderOpts { held_with_alt: Some(&high), ..tone }, widgets::SliderOpts { held_with_alt: Some(&low), ..tone });
            use widgets::ToneZone as Z;
            let (o_hi, o_sh) = (widgets::SliderOpts { hovered: over(Z::Highlights), ..hi }, widgets::SliderOpts { hovered: over(Z::Shadows), ..lo });
            let (o_wh, o_bl) = (widgets::SliderOpts { hovered: over(Z::Whites), ..hi }, widgets::SliderOpts { hovered: over(Z::Blacks), ..lo });
            changed |= widgets::slider_row_with(ui, "Lichter", t.pick(&mut adj.highlights, |l| &mut l.highlights), -100.0..=100.0, 0.0, 0, o_hi);
            changed |= widgets::slider_row_with(ui, "Tiefen", t.pick(&mut adj.shadows, |l| &mut l.shadows), -100.0..=100.0, 0.0, 0, o_sh);
            changed |= widgets::slider_row_with(ui, "Weiß", t.pick(&mut adj.whites, |l| &mut l.whites), -100.0..=100.0, 0.0, 0, o_wh);
            changed |= widgets::slider_row_with(ui, "Schwarz", t.pick(&mut adj.blacks, |l| &mut l.blacks), -100.0..=100.0, 0.0, 0, o_bl);
            widgets::hint(ui, "Alt beim Ziehen: ausgefressene und abgesoffene Stellen");
        });
        if reset_hdr {
            match t.layer_mut() {
                Some(l) => (l.highlights, l.shadows, l.whites, l.blacks) = (0.0, 0.0, 0.0, 0.0),
                None => (adj.highlights, adj.shadows, adj.whites, adj.blacks) = (0.0, 0.0, 0.0, 0.0),
            }
            changed = true;
        }
        t.store(adj);
        if changed {
            self.request_render();
        }

        // Near the top: one of the most effective tools for portraits against the light.
        self.relight_section(ui);
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut changed = false;

        let mut reset_levels = false;
        let levels_peek = std::cell::Cell::new(None);
        let levels_channel = &mut self.levels_channel;
        let (meta, preview, levels_hist) = (cur.meta.clone(), cur.preview.clone(), &mut self.levels_hist);
        widgets::tool_section(ui, sk_core::tools::Tool::Levels, Some(&mut || reset_levels = true), |ui| {
            let levels_hist = levels_hist.get(&meta, &preview, adj, &ui.ctx().clone());
            ui.horizontal(|ui| {
                for ch in CurveChannel::ALL.into_iter().filter(|c| *c != CurveChannel::Luma) {
                    ui.selectable_value(levels_channel, ch, ch.label());
                }
            });
            ui.add_space(4.0);
            let lv = &mut adj.levels;
            let h = levels_hist.as_ref();
            // A channel gets what the levels of all channels made of it.
            let rgb = lv.rgb;
            let through_rgb = |bins: &[u32]| widgets::remap_bins(bins, |v| rgb.apply(v));
            let (set, color, bins) = match levels_channel {
                CurveChannel::Red => (&mut lv.red, Color32::from_rgb(240, 80, 80), h.map(|h| through_rgb(&h.r))),
                CurveChannel::Green => (&mut lv.green, Color32::from_rgb(80, 210, 80), h.map(|h| through_rgb(&h.g))),
                CurveChannel::Blue => (&mut lv.blue, Color32::from_rgb(90, 130, 250), h.map(|h| through_rgb(&h.b))),
                CurveChannel::Rgb | CurveChannel::Luma => (&mut lv.rgb, Color32::from_gray(220), h.map(|h| h.luma.iter().map(|&n| n as f32).collect())),
            };
            changed |= widgets::levels_editor(ui, set, bins.as_deref(), color, &levels_peek);
        });
        if reset_levels {
            adj.levels = Levels::default();
            changed = true;
        }

        // The curves of the background or of the chosen layer (on the layer's result).
        let mut t = Target::new(adj, self.active_layer);
        let mut reset_curve = false;
        let target_on = self.target_tool == Some(crate::target_tool::TargetTool::Curve);
        let mut toggle_target = false;
        let channel = &mut self.curve_channel;
        let curves = match t.layer_mut() {
            Some(l) => &mut l.curves,
            None => &mut adj.curves,
        };
        widgets::tool_section(ui, sk_core::tools::Tool::Curves, Some(&mut || reset_curve = true), |ui| {
            ui.horizontal(|ui| {
                for ch in CurveChannel::ALL {
                    let r = ui.selectable_value(channel, ch, ch.label());
                    if ch == CurveChannel::Luma {
                        r.on_hover_text("Nur die Helligkeit: Kontrast ohne Farbverschiebung, Farbton und Sättigung bleiben");
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    toggle_target = target_button(ui, target_on, "Zielkorrektur: im Bild nach oben oder unten ziehen ändert die Kurve beim Ton unter der Maus");
                });
            });
            let h = self.histogram.as_ref();
            let (color, bins): (Color32, Option<&[u32]>) = match channel {
                CurveChannel::Rgb => (Color32::from_gray(230), h.map(|h| h.luma.as_slice())),
                CurveChannel::Luma => (Color32::from_rgb(235, 220, 185), h.map(|h| h.luma.as_slice())),
                CurveChannel::Red => (Color32::from_rgb(240, 80, 80), h.map(|h| h.r.as_slice())),
                CurveChannel::Green => (Color32::from_rgb(80, 210, 80), h.map(|h| h.g.as_slice())),
                CurveChannel::Blue => (Color32::from_rgb(90, 130, 250), h.map(|h| h.b.as_slice())),
            };
            changed |= widgets::curve_editor(ui, channel.curve_mut(curves), color, bins);
        });
        if reset_curve {
            *self.curve_channel.curve_mut(curves) = Curve::default();
            changed = true;
        }
        t.store(adj);

        if changed {
            self.request_render();
        }
        if toggle_target {
            self.toggle_target_tool(crate::target_tool::TargetTool::Curve);
        }
        let zone = widgets::ToneZone::ALL.into_iter().zip(&zones).find(|(_, c)| c.get()).map(|(z, _)| z);
        if zone != self.hover_zone {
            self.hover_zone = zone;
            ui.ctx().request_repaint();
        }
        let view = if high.get() {
            Some(SliderView::ClipHighlights)
        } else if low.get() {
            Some(SliderView::ClipShadows)
        } else {
            levels_peek.get()
        };
        self.set_slider_view(view);
    }

    fn color_tab(&mut self, ui: &mut Ui) {
        let picking = self.picking_wb;
        let mut toggle_picker = false;
        let Some(cur) = self.current.as_mut() else {
            crate::widgets::no_image(ui);
            return;
        };
        let as_shot = cur.as_shot_temp_tint;
        // Matching a look sets the whole image.
        widgets::image_only_section(ui, "Look angleichen", |ui| self.look_section(ui));
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut t = Target::new(adj, self.active_layer);
        let mut changed = false;

        let mut reset_curve = false;
        widgets::tool_section(ui, sk_core::tools::Tool::BaseCurve, Some(&mut || reset_curve = true), |ui| {
            widgets::combo_field(ui, "Kurve", adj.base_curve.label(), |ui| {
                    for c in BaseCurve::ALL {
                        changed |= ui.selectable_value(&mut adj.base_curve, c, c.label()).on_hover_text(c.hint()).changed();
                    }
                });
        });
        if reset_curve {
            sk_core::tools::Tool::BaseCurve.copy(&Default::default(), adj);
            changed = true;
        }
        // The camera's curve is fitted the first time it is chosen.
        let mut no_camera_curve = false;
        if adj.base_curve == BaseCurve::Camera && adj.camera_curve.is_none() {
            match sk_core::camera_look::fit_cached(&cur.meta, &cur.preview) {
                Some(c) => adj.camera_curve = Some(c),
                None => {
                    adj.base_curve = BaseCurve::FilmStandard;
                    no_camera_curve = true;
                }
            }
        }

        let mut reset_wb = false;
        widgets::tool_section(ui, sk_core::tools::Tool::WhiteBalance, Some(&mut || reset_wb = true), |ui| {
            let wb = &mut adj.white_balance;
            // A layer warms or cools relative to the background: no mode, Kelvin or pipette.
            if let Some(l) = t.layer_mut() {
                ui.add_enabled_ui(false, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Modus");
                        let _ = ui.selectable_label(wb.mode == WbMode::AsShot, "Aufnahme");
                        let _ = ui.selectable_label(wb.mode == WbMode::Custom, "Benutzerdefiniert");
                    });
                })
                .response
                .on_disabled_hover_text(widgets::ONLY_IMAGE);
                let opts = |rail| widgets::SliderOpts { rail: Some(rail), ..Default::default() };
                changed |= widgets::slider_row_with(ui, "Temperatur", &mut l.temperature, -100.0..=100.0, 0.0, 0, opts(widgets::RAIL_KELVIN));
                changed |= widgets::slider_row_with(ui, "Tönung", &mut l.tint, -150.0..=150.0, 0.0, 0, opts(widgets::RAIL_TINT));
                ui.add_enabled_ui(false, |ui| {
                    let _ = ui.add(egui::Button::new(format!("{}  Weißabgleich-Pipette", crate::theme::icon::EYEDROPPER)).selected(false));
                })
                .response
                .on_disabled_hover_text(widgets::ONLY_IMAGE);
                return;
            }
            ui.horizontal(|ui| {
                ui.label("Modus");
                if ui.selectable_label(wb.mode == WbMode::AsShot, "Aufnahme").clicked() && wb.mode != WbMode::AsShot {
                    *wb = Default::default();
                    changed = true;
                }
                if ui.selectable_label(wb.mode == WbMode::Custom, "Benutzerdefiniert").clicked() && wb.mode != WbMode::Custom {
                    *wb = WhiteBalance { mode: WbMode::Custom, kelvin: as_shot.0, tint: as_shot.1 };
                    changed = true;
                }
            });
            // In as-shot mode the sliders show the estimated values; moving one switches to custom.
            let (mut kelvin, mut tint) = if wb.mode == WbMode::AsShot { as_shot } else { (wb.kelvin, wb.tint) };
            let mut wb_changed = widgets::slider_row_with(ui, "Kelvin", &mut kelvin, 2000.0..=15000.0, as_shot.0, 0, widgets::SliderOpts { suffix: " K", rail: Some(widgets::RAIL_KELVIN), ..Default::default() });
            wb_changed |= widgets::slider_row_with(ui, "Tönung", &mut tint, -150.0..=150.0, as_shot.1, 0, widgets::SliderOpts { rail: Some(widgets::RAIL_TINT), ..Default::default() });
            if wb_changed {
                *wb = WhiteBalance { mode: WbMode::Custom, kelvin, tint };
                changed = true;
            }
            if ui.add(egui::Button::new(format!("{}  Weißabgleich-Pipette", crate::theme::icon::EYEDROPPER)).selected(picking)).on_hover_text("Auf eine neutrale (graue) Stelle im Bild klicken").clicked() {
                toggle_picker = true;
            }
        });
        if reset_wb {
            match t.layer_mut() {
                Some(l) => (l.temperature, l.tint) = (0.0, 0.0),
                None => adj.white_balance = Default::default(),
            }
            changed = true;
        }
        t.store(adj);
        if no_camera_curve {
            self.status = "Kein passendes Kamera-JPEG in dieser Datei: Film Standard bleibt".into();
        }
        if toggle_picker {
            // Ends the other viewer tools; needs `self`, the adjustments are borrowed again below.
            self.toggle_wb_picker();
        }
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut t = Target::new(adj, self.active_layer);

        let mut reset_color = false;
        widgets::tool_section(ui, sk_core::tools::Tool::Color, Some(&mut || reset_color = true), |ui| {
            let rail = widgets::SliderOpts { rail: Some(widgets::RAIL_SATURATION), ..Default::default() };
            changed |= widgets::slider_row_with(ui, "Sättigung", t.pick(&mut adj.saturation, |l| &mut l.saturation), -100.0..=100.0, 0.0, 0, rail);
            changed |= widgets::slider_row_with(ui, "Dynamik", t.pick(&mut adj.vibrance, |l| &mut l.vibrance), -100.0..=100.0, 0.0, 0, rail);
        });
        if reset_color {
            match t.layer_mut() {
                Some(l) => (l.saturation, l.vibrance) = (0.0, 0.0),
                None => (adj.saturation, adj.vibrance) = (0.0, 0.0),
            }
            changed = true;
        }
        t.store(adj);

        if changed {
            self.request_render();
        }
        self.color_advanced(ui);
    }
}
