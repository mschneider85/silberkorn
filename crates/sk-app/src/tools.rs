//! Tool tabs: Library, Exposure, Color, Effects, Metadata, Output.

use sk_core::export::{ExportFormat, OutputSharpening, Resize};
use sk_core::watermark::{Anchor, WatermarkKind};
use sk_core::params::{BaseCurve, Curve, Levels, WbMode, WhiteBalance};
use sk_core::sidecar::ColorLabel;
use egui::{Color32, RichText, Ui};

use crate::app::{App, CurveChannel, Tab};
use crate::layer_target::Target;
use crate::widgets::{self, section, slider_row};

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
    }

    /// The switched-off and the changed tools of the current image, for the eyes in the headers.
    fn tool_states(&self) -> widgets::ToolStates {
        use sk_core::tools::Tool;
        let Some(cur) = &self.current else { return Default::default() };
        let a = &cur.adjustments;
        let edited = Tool::ALL
            .into_iter()
            .filter(|t| match t {
                // Without copying layers and spots each frame.
                Tool::Layers => !a.layers.is_empty(),
                Tool::Retouch => !a.retouch.is_empty() || !a.removals.is_empty(),
                t => !t.is_default(a),
            })
            .collect();
        widgets::ToolStates { hidden: a.hidden.clone(), edited }
    }

    fn tool_tabs(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
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
            Tab::Output => {
                self.output_tab(ui);
                self.softproof_section(ui);
                self.print_section(ui);
            }
        }
    }

    fn exposure_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            widgets::no_image(ui);
            return;
        }
        // With the right column open, the histogram is shown there.
        if !self.side_column_shown {
            let r = widgets::histogram(ui, self.histogram.as_ref());
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

        let mut reset_exposure = false;
        widgets::tool_section(ui, sk_core::tools::Tool::Exposure, Some(&mut || reset_exposure = true), |ui| {
            let tone = widgets::SliderOpts { rail: Some(widgets::RAIL_TONE), ..Default::default() };
            let ev = widgets::SliderOpts { suffix: " EV", ..tone };
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
            changed |= widgets::slider_row_with(ui, "Lichter", t.pick(&mut adj.highlights, |l| &mut l.highlights), -100.0..=100.0, 0.0, 0, tone);
            changed |= widgets::slider_row_with(ui, "Tiefen", t.pick(&mut adj.shadows, |l| &mut l.shadows), -100.0..=100.0, 0.0, 0, tone);
            changed |= widgets::slider_row_with(ui, "Weiß", t.pick(&mut adj.whites, |l| &mut l.whites), -100.0..=100.0, 0.0, 0, tone);
            changed |= widgets::slider_row_with(ui, "Schwarz", t.pick(&mut adj.blacks, |l| &mut l.blacks), -100.0..=100.0, 0.0, 0, tone);
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
            changed |= widgets::levels_editor(ui, set, bins.as_deref(), color);
        });
        if reset_levels {
            adj.levels = Levels::default();
            changed = true;
        }

        let mut reset_curve = false;
        let channel = &mut self.curve_channel;
        widgets::tool_section(ui, sk_core::tools::Tool::Curves, Some(&mut || reset_curve = true), |ui| {
            ui.horizontal(|ui| {
                for ch in CurveChannel::ALL {
                    let r = ui.selectable_value(channel, ch, ch.label());
                    if ch == CurveChannel::Luma {
                        r.on_hover_text("Nur die Helligkeit: Kontrast ohne Farbverschiebung, Farbton und Sättigung bleiben");
                    }
                }
            });
            let (curve, color, bins): (&mut Curve, Color32, Option<&[u32]>) = match channel {
                CurveChannel::Rgb => (&mut adj.curves.rgb, Color32::from_gray(230), self.histogram.as_ref().map(|h| h.luma.as_slice())),
                CurveChannel::Luma => (&mut adj.curves.luma, Color32::from_rgb(235, 220, 185), self.histogram.as_ref().map(|h| h.luma.as_slice())),
                CurveChannel::Red => (&mut adj.curves.red, Color32::from_rgb(240, 80, 80), self.histogram.as_ref().map(|h| h.r.as_slice())),
                CurveChannel::Green => (&mut adj.curves.green, Color32::from_rgb(80, 210, 80), self.histogram.as_ref().map(|h| h.g.as_slice())),
                CurveChannel::Blue => (&mut adj.curves.blue, Color32::from_rgb(90, 130, 250), self.histogram.as_ref().map(|h| h.b.as_slice())),
            };
            changed |= widgets::curve_editor(ui, curve, color, bins);
        });
        if reset_curve {
            match self.curve_channel {
                CurveChannel::Rgb => adj.curves.rgb = Curve::default(),
                CurveChannel::Luma => adj.curves.luma = Curve::default(),
                CurveChannel::Red => adj.curves.red = Curve::default(),
                CurveChannel::Green => adj.curves.green = Curve::default(),
                CurveChannel::Blue => adj.curves.blue = Curve::default(),
            }
            changed = true;
        }

        if changed {
            self.request_render();
        }
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

    /// Keyword chips (click removes), an input with catalog suggestions.
    /// Rating, color label and keywords shown in the info tab: of the open image, or else of
    /// the first selected one (they are set for the whole selection either way).
    fn shown_meta(&self) -> Option<(u8, ColorLabel, Vec<String>)> {
        if let Some(cur) = &self.current {
            return Some((cur.rating, cur.color_label, cur.keywords.clone()));
        }
        let first = self.selected_paths().into_iter().next()?;
        let e = self.entries.iter().find(|e| e.path == first)?;
        Some((e.rating, e.color_label, e.keywords.clone()))
    }

    pub(crate) fn keywords_ui(&mut self, ui: &mut Ui) {
        let Some((_, _, keywords)) = self.shown_meta() else { return };
        let mut remove_kw = None;
        let mut add_kw = None;
        ui.horizontal_wrapped(|ui| {
            for k in &keywords {
                if ui.add(egui::Button::new(format!("{k}  {}", crate::theme::icon::X)).small()).on_hover_text("Entfernen").clicked() {
                    remove_kw = Some(k.clone());
                }
            }
            if keywords.is_empty() {
                ui.label(RichText::new("Keine Keywords").color(crate::theme::TEXT_FAINT));
            }
        });
        let resp = ui.add(egui::TextEdit::singleline(&mut self.keyword_input).hint_text("Keyword hinzufügen (Enter)"));
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            add_kw = Some(self.keyword_input.clone());
            resp.request_focus();
        }
        // Suggestions from the catalog.
        let typed = self.keyword_input.trim().to_lowercase();
        if !typed.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (k, _) in self.lists.keywords.iter().filter(|(k, _)| k.to_lowercase().starts_with(&typed)).take(8) {
                    if ui.small_button(k).clicked() {
                        add_kw = Some(k.clone());
                    }
                }
            });
        }
        if let Some(k) = remove_kw {
            self.remove_keyword(&k);
        }
        if let Some(k) = add_kw {
            self.add_keyword(&k);
            self.keyword_input.clear();
        }
    }

    fn metadata_tab(&mut self, ui: &mut Ui) {
        let Some((mut rating, current_label, _)) = self.shown_meta() else {
            crate::widgets::no_image(ui);
            return;
        };
        let n_sel = self.selected_count();
        let mut new_rating = None;
        let mut new_label = None;
        let title = if n_sel > 1 { format!("Bewertung ({n_sel} Bilder)") } else { "Bewertung".to_string() };
        section(ui, &title, None, |ui| {
            if widgets::stars(ui, &mut rating, 22.0) {
                new_rating = Some(rating);
            }
            ui.horizontal_wrapped(|ui| {
                for l in ColorLabel::ALL {
                    if widgets::color_label_dot(ui, l, current_label == l).clicked() {
                        new_label = Some(l);
                    }
                }
            });
        });
        section(ui, "Keywords", None, |ui| self.keywords_ui(ui));
        if let Some(r) = new_rating {
            self.set_rating(r);
        }
        if let Some(l) = new_label {
            self.set_color_label(l);
        }

        let Some(cur) = self.current.as_ref() else { return };
        let info = &cur.meta.info;
        section(ui, "Aufnahme", None, |ui| {
            egui::Grid::new("meta").num_columns(2).striped(true).show(ui, |ui| {
                let mut row = |k: &str, v: Option<String>| {
                    ui.label(RichText::new(k).color(crate::theme::TEXT_DIM));
                    let v = v.unwrap_or_else(|| "–".into());
                    ui.add(egui::Label::new(&v).truncate()).on_hover_text(&v);
                    ui.end_row();
                };
                row("Datei", Some(sk_core::variant::display_name(&cur.meta.path)));
                row("Ordner", cur.meta.path.parent().map(|n| n.display().to_string()));
                row("Kamera", Some(format!("{} {}", info.make, info.model)));
                row("Objektiv", info.lens.clone());
                row("Brennweite", info.focal_length.clone());
                row("Blende", info.aperture.clone());
                row("Belichtungszeit", info.exposure_time.clone());
                row("ISO", info.iso.map(|v| v.to_string()));
                row("Datum", info.date.clone());
                row("Sensor", Some(format!("{} × {}", info.width, info.height)));
            });
        });
    }

    fn output_tab(&mut self, ui: &mut Ui) {
        let mut add = false;
        let mut remove = None;
        section(ui, "Ausgaberezepte", None, |ui| {
            for (i, r) in self.recipes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut r.enabled, "").on_hover_text("Beim Exportieren verwenden");
                    if ui.selectable_label(self.selected_recipe == i, &r.name).clicked() {
                        self.selected_recipe = i;
                    }
                    crate::widgets::hint(ui, format!("{} · {}", r.format.label(), r.output_space.label()));
                });
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Rezept", crate::theme::icon::PLUS)).clicked() {
                    add = true;
                }
                if self.recipes.len() > 1 && ui.button(format!("{}  Löschen …", crate::theme::icon::TRASH)).clicked() {
                    remove = Some(self.selected_recipe);
                }
            });
        });
        if add {
            let mut r = self.recipes.get(self.selected_recipe).cloned().unwrap_or_default();
            r.name = format!("Rezept {}", self.recipes.len() + 1);
            self.recipes.push(r);
            self.selected_recipe = self.recipes.len() - 1;
        }
        if let Some(i) = remove {
            let name = self.recipes[i].name.clone();
            self.request_deletion(crate::deletion::Deletion::Recipe(i, name));
        }
        self.selected_recipe = self.selected_recipe.min(self.recipes.len().saturating_sub(1));
        let Some(r) = self.recipes.get_mut(self.selected_recipe) else { return };
        section(ui, "Rezept bearbeiten", None, |ui| {
            widgets::field(ui, "Name", |ui| ui.text_edit_singleline(&mut r.name));
            widgets::combo_field(ui, "Format", r.format.label(), |ui| {
                for f in ExportFormat::ALL {
                    if ui.selectable_value(&mut r.format, f, f.label()).changed() && !f.supports(r.output_space) {
                        r.output_space = sk_core::color::OutputSpace::Srgb;
                    }
                }
            });
            widgets::combo_field(ui, "Farbraum", r.output_space.label(), |ui| {
                for sp in sk_core::color::OutputSpace::ALL {
                    ui.add_enabled_ui(r.format.supports(sp), |ui| ui.selectable_value(&mut r.output_space, sp, sp.label()))
                        .response
                        .on_disabled_hover_text("AVIF-Dateien kennzeichnen den Farbraum ohne ICC-Profil; das geht nur für sRGB und Display P3");
                }
            });
            // Lossless formats have no quality: shown, but disabled.
            ui.add_enabled_ui(r.format.has_quality(), |ui| {
                let mut q = r.quality as f32;
                if slider_row(ui, "Qualität", &mut q, 1.0..=100.0, 90.0, 0) {
                    r.quality = q.round() as _;
                }
            })
            .response
            .on_disabled_hover_text("Verlustfreies Format: ohne Qualitätsstufe");
            let size_label = if r.size.is_some() { r.resize.label() } else { "Originalgröße" };
            let mut resize = r.size.map(|_| r.resize);
            widgets::combo_field(ui, "Größe", size_label, |ui| {
                ui.selectable_value(&mut resize, None, "Originalgröße");
                for m in Resize::ALL {
                    ui.selectable_value(&mut resize, Some(m), m.label());
                }
            });
            if let Some(m) = resize {
                let percent = m == Resize::Percent;
                // The value is kept in its unit when switching between the pixel modes.
                let mut v = match r.size {
                    Some(v) if (r.resize == Resize::Percent) == percent => v,
                    _ if percent => 50,
                    _ => 2048,
                };
                widgets::field(ui, "", |ui| {
                    let drag = if percent { egui::DragValue::new(&mut v).range(1..=100).suffix(" %") } else { egui::DragValue::new(&mut v).range(16..=20000).suffix(" px") };
                    ui.add(drag).on_hover_text("Nie größer als das entwickelte Bild")
                });
                r.resize = m;
                r.size = Some(v);
            } else {
                r.size = None;
            }
            widgets::combo_field(ui, "Ausgabeschärfung", r.output_sharpening.label(), |ui| {
                for s in OutputSharpening::ALL {
                    ui.selectable_value(&mut r.output_sharpening, s, s.label());
                }
            });
            widgets::field(ui, "Ordner", |ui| {
                let mut dir = r.output_dir.to_string_lossy().to_string();
                if ui.text_edit_singleline(&mut dir).changed() {
                    r.output_dir = dir.into();
                }
            });
            crate::widgets::hint(ui, "Relativ zum Bildordner, z. B. „Output“");
            widgets::field(ui, "Dateiname", |ui| ui.add(egui::TextEdit::singleline(&mut r.name_template).hint_text("{Name}")));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (token, help) in sk_core::naming::TOKENS {
                    if ui.small_button(RichText::new(token).small()).on_hover_text(help).clicked() {
                        r.name_template.push_str(token);
                    }
                }
            });
            widgets::field(ui, "Suffix", |ui| ui.text_edit_singleline(&mut r.suffix));
        });
        let mut reset_watermark = false;
        section(ui, "Wasserzeichen", Some(&mut || reset_watermark = true), |ui| {
            let wm = &mut r.watermark;
            ui.checkbox(&mut wm.enabled, "Wasserzeichen einfügen");
            ui.add_enabled_ui(wm.enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut wm.kind, WatermarkKind::Text, "Text");
                    ui.selectable_value(&mut wm.kind, WatermarkKind::Image, "Logo");
                });
                match wm.kind {
                    WatermarkKind::Text => {
                        ui.text_edit_singleline(&mut wm.text);
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut wm.dark, false, "Weiß");
                            ui.selectable_value(&mut wm.dark, true, "Schwarz");
                        });
                    }
                    WatermarkKind::Image => {
                        ui.horizontal(|ui| {
                            if ui.button("Logo wählen …").clicked()
                                && let Some(f) = rfd::FileDialog::new().set_title("Logo für das Wasserzeichen").add_filter("Bilder", &["png", "jpg", "jpeg", "tif", "tiff"]).pick_file()
                            {
                                wm.image = f;
                            }
                            let name = wm.image.file_name().map_or("kein Logo".into(), |n| n.to_string_lossy().to_string());
                            ui.label(RichText::new(name).small().color(Color32::from_gray(160)));
                        });
                        crate::widgets::hint(ui, "PNG mit Transparenz empfohlen");
                    }
                }
                widgets::combo_field(ui, "Position", wm.anchor.label(), |ui| {
                    for a in Anchor::ALL {
                        ui.selectable_value(&mut wm.anchor, a, a.label());
                    }
                });
                let pct = widgets::SliderOpts { suffix: " %", ..Default::default() };
                let d = sk_core::watermark::Watermark::default();
                widgets::slider_row_with(ui, "Größe", &mut wm.size, 1.0..=30.0, d.size, 1, pct);
                widgets::slider_row_with(ui, "Abstand", &mut wm.margin, 0.0..=20.0, d.margin, 1, pct);
                let mut opacity = wm.opacity * 100.0;
                if widgets::slider_row_with(ui, "Deckkraft", &mut opacity, 5.0..=100.0, d.opacity * 100.0, 0, pct) {
                    wm.opacity = opacity / 100.0;
                }
                crate::widgets::hint(ui, format!("Größe und Abstand in % der kurzen Bildkante. {}.", crate::commands::hint("Vorschau in der Ausgabevorschau", "proof")));
            });
        });
        if reset_watermark {
            r.watermark = Default::default();
        }
        section(ui, "Metadaten", None, |ui| {
            ui.checkbox(&mut r.capture_metadata, "Aufnahmedaten übernehmen").on_hover_text("Kamera, Objektiv, Belichtung und Aufnahmedatum");
            widgets::field(ui, "Fotograf", |ui| ui.text_edit_singleline(&mut r.artist));
            widgets::field(ui, "Copyright", |ui| ui.text_edit_singleline(&mut r.copyright));
            let note = if r.format.has_metadata() {
                "GPS-Daten werden nie übernommen."
            } else {
                "TIFF-Dateien erhalten keine Metadaten."
            };
            crate::widgets::hint(ui, note);
        });
        section(ui, "Exportieren", None, |ui| {
            let n = self.selected_count();
            let k = self.recipes.iter().filter(|r| r.enabled).count();
            let text = RichText::new(format!("{}  {} × {} exportieren", crate::theme::icon::EXPORT, widgets::image_count(n), if k == 1 { "1 Rezept".to_string() } else { format!("{k} Rezepte") })).color(Color32::WHITE);
            let button = egui::Button::new(text).fill(crate::theme::ACCENT).corner_radius(6).min_size(egui::vec2(ui.available_width(), 30.0));
            if ui.add_enabled(self.command_available("export"), button).on_disabled_hover_text(self.export_blocked_reason()).clicked() {
                self.export_selection();
            }
            if let (Some((done, total)), Some(f)) = (self.export_progress, self.export_fraction()) {
                ui.add(egui::ProgressBar::new(f).text(format!("{done} / {total}")));
                if let Some(c) = &self.export_cancel {
                    crate::widgets::cancel_button(ui, c);
                }
            }
            for msg in self.export_log.iter().rev().take(8) {
                ui.label(RichText::new(msg).small());
            }
        });
    }
}
