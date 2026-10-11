//! Tool tabs: Library, Exposure, Color, Metadata, Output.

use sk_core::export::ExportFormat;
use sk_core::watermark::{Anchor, WatermarkKind};
use sk_core::params::{BaseCurve, Curve, Levels, WbMode, WhiteBalance};
use sk_core::sidecar::ColorLabel;
use egui::{Color32, RichText, Ui};

use crate::app::{App, CurveChannel, Tab};
use crate::widgets::{self, section, slider_row};

impl App {
    pub fn tools_ui(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
            Tab::Library => self.library_tab(ui),
            Tab::Lens => self.lens_tab(ui),
            Tab::Details => self.details_tab(ui),
            Tab::Layers => self.layers_tab(ui),
            Tab::Styles => self.styles_tab(ui),
            Tab::History => self.history_tab(ui),
            Tab::Exposure => self.exposure_tab(ui),
            Tab::Color => self.color_tab(ui),
            Tab::Metadata => self.metadata_tab(ui),
            Tab::Output => self.output_tab(ui),
        });
    }

    fn exposure_tab(&mut self, ui: &mut Ui) {
        widgets::histogram(ui, self.histogram.as_ref());
        ui.add_space(4.0);
        self.auto_section(ui);
        let Some(cur) = self.current.as_mut() else {
            ui.label("Kein Bild ausgewählt.");
            return;
        };
        let adj = &mut cur.adjustments;
        let mut changed = false;

        let mut reset_exposure = false;
        section(ui, "Belichtung", Some(&mut || reset_exposure = true), |ui| {
            changed |= slider_row(ui, "Belichtung", &mut adj.exposure, -4.0..=4.0, 0.0, 2);
            changed |= slider_row(ui, "Kontrast", &mut adj.contrast, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Helligkeit", &mut adj.brightness, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Sättigung", &mut adj.saturation, -100.0..=100.0, 0.0, 0);
        });
        if reset_exposure {
            adj.exposure = 0.0;
            adj.contrast = 0.0;
            adj.brightness = 0.0;
            adj.saturation = 0.0;
            changed = true;
        }

        let mut reset_hdr = false;
        section(ui, "High Dynamic Range", Some(&mut || reset_hdr = true), |ui| {
            changed |= slider_row(ui, "Lichter", &mut adj.highlights, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Tiefen", &mut adj.shadows, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Weiß", &mut adj.whites, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Schwarz", &mut adj.blacks, -100.0..=100.0, 0.0, 0);
        });
        if reset_hdr {
            adj.highlights = 0.0;
            adj.shadows = 0.0;
            adj.whites = 0.0;
            adj.blacks = 0.0;
            changed = true;
        }

        let mut reset_levels = false;
        section(ui, "Tonwerte (Levels)", Some(&mut || reset_levels = true), |ui| {
            let lv = &mut adj.levels;
            changed |= slider_row(ui, "Eingang Schwarz", &mut lv.in_black, 0.0..=0.5, 0.0, 3);
            changed |= slider_row(ui, "Eingang Weiß", &mut lv.in_white, 0.5..=1.0, 1.0, 3);
            changed |= slider_row(ui, "Mitteltöne", &mut lv.midtone, 0.2..=5.0, 1.0, 2);
            changed |= slider_row(ui, "Ausgang Schwarz", &mut lv.out_black, 0.0..=0.5, 0.0, 3);
            changed |= slider_row(ui, "Ausgang Weiß", &mut lv.out_white, 0.5..=1.0, 1.0, 3);
            if lv.in_white <= lv.in_black + 0.01 {
                lv.in_white = lv.in_black + 0.01;
            }
        });
        if reset_levels {
            adj.levels = Levels::default();
            changed = true;
        }

        let mut reset_curve = false;
        let channel = &mut self.curve_channel;
        section(ui, "Gradationskurve", Some(&mut || reset_curve = true), |ui| {
            ui.horizontal(|ui| {
                for ch in CurveChannel::ALL {
                    ui.selectable_value(channel, ch, ch.label());
                }
            });
            let (curve, color, bins): (&mut Curve, Color32, Option<&[u32]>) = match channel {
                CurveChannel::Rgb => (&mut adj.curves.rgb, Color32::from_gray(230), self.histogram.as_ref().map(|h| h.luma.as_slice())),
                CurveChannel::Red => (&mut adj.curves.red, Color32::from_rgb(240, 80, 80), self.histogram.as_ref().map(|h| h.r.as_slice())),
                CurveChannel::Green => (&mut adj.curves.green, Color32::from_rgb(80, 210, 80), self.histogram.as_ref().map(|h| h.g.as_slice())),
                CurveChannel::Blue => (&mut adj.curves.blue, Color32::from_rgb(90, 130, 250), self.histogram.as_ref().map(|h| h.b.as_slice())),
            };
            changed |= widgets::curve_editor(ui, curve, color, bins);
        });
        if reset_curve {
            match self.curve_channel {
                CurveChannel::Rgb => adj.curves.rgb = Curve::default(),
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
        let picking = &mut self.picking_wb;
        let Some(cur) = self.current.as_mut() else {
            ui.label("Kein Bild ausgewählt.");
            return;
        };
        let as_shot = cur.as_shot_temp_tint;
        let adj = &mut cur.adjustments;
        let mut changed = false;

        section(ui, "Basismerkmale", None, |ui| {
            egui::ComboBox::from_label("Kurve")
                .selected_text(adj.base_curve.label())
                .show_ui(ui, |ui| {
                    for c in BaseCurve::ALL {
                        changed |= ui.selectable_value(&mut adj.base_curve, c, c.label()).changed();
                    }
                });
        });

        let mut reset_wb = false;
        section(ui, "Weißabgleich", Some(&mut || reset_wb = true), |ui| {
            let wb = &mut adj.white_balance;
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
            let mut wb_changed = slider_row(ui, "Kelvin", &mut kelvin, 2000.0..=15000.0, as_shot.0, 0);
            wb_changed |= slider_row(ui, "Tint", &mut tint, -150.0..=150.0, as_shot.1, 1);
            if wb_changed {
                *wb = WhiteBalance { mode: WbMode::Custom, kelvin, tint };
                changed = true;
            }
            ui.horizontal(|ui| {
                if ui.selectable_label(*picking, "\u{E568}  Weißabgleich-Pipette").on_hover_text("Auf eine neutrale (graue) Stelle im Bild klicken").clicked() {
                    *picking = !*picking;
                }
            });
        });
        if reset_wb {
            adj.white_balance = Default::default();
            changed = true;
        }

        let mut reset_color = false;
        section(ui, "Farbe", Some(&mut || reset_color = true), |ui| {
            changed |= slider_row(ui, "Sättigung", &mut adj.saturation, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Dynamik", &mut adj.vibrance, -100.0..=100.0, 0.0, 0);
        });
        if reset_color {
            adj.saturation = 0.0;
            adj.vibrance = 0.0;
            changed = true;
        }

        if changed {
            self.request_render();
        }
        self.color_advanced(ui);
    }

    fn metadata_tab(&mut self, ui: &mut Ui) {
        let Some(cur) = self.current.as_ref() else {
            ui.label("Kein Bild ausgewählt.");
            return;
        };
        let n_sel = self.selected_paths().len();
        let (mut rating, current_label, keywords) = (cur.rating, cur.color_label, cur.keywords.clone());
        let mut new_rating = None;
        let mut new_label = None;
        let mut remove_kw = None;
        let mut add_kw = None;
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
        section(ui, "Keywords", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                for k in &keywords {
                    if ui.add(egui::Button::new(format!("{k}  \u{E4F6}")).small()).on_hover_text("Entfernen").clicked() {
                        remove_kw = Some(k.clone());
                    }
                }
                if keywords.is_empty() {
                    ui.label(RichText::new("Keine Keywords").color(Color32::from_gray(140)));
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
        });
        if let Some(r) = new_rating {
            self.set_rating(r);
        }
        if let Some(l) = new_label {
            self.set_color_label(l);
        }
        if let Some(k) = remove_kw {
            self.remove_keyword(&k);
        }
        if let Some(k) = add_kw {
            self.add_keyword(&k);
            self.keyword_input.clear();
        }

        let Some(cur) = self.current.as_ref() else { return };
        let info = &cur.meta.info;
        section(ui, "Aufnahme", None, |ui| {
            egui::Grid::new("meta").num_columns(2).striped(true).show(ui, |ui| {
                let mut row = |k: &str, v: Option<String>| {
                    ui.label(RichText::new(k).color(Color32::from_gray(150)));
                    ui.label(v.unwrap_or_else(|| "–".into()));
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
                    ui.checkbox(&mut r.enabled, "").on_hover_text("Beim Verarbeiten verwenden");
                    if ui.selectable_label(self.selected_recipe == i, &r.name).clicked() {
                        self.selected_recipe = i;
                    }
                    ui.label(RichText::new(format!("{} · {}", r.format.label(), r.output_space.label())).small().color(Color32::from_gray(140)));
                });
            }
            ui.horizontal(|ui| {
                if ui.button("+ Rezept").clicked() {
                    add = true;
                }
                if self.recipes.len() > 1 && ui.button("\u{E4A6}  Entfernen").clicked() {
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
            self.recipes.remove(i);
            self.selected_recipe = self.selected_recipe.min(self.recipes.len() - 1);
        }
        self.selected_recipe = self.selected_recipe.min(self.recipes.len().saturating_sub(1));
        let Some(r) = self.recipes.get_mut(self.selected_recipe) else { return };
        section(ui, "Rezept bearbeiten", None, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut r.name);
            });
            egui::ComboBox::from_label("Format").selected_text(r.format.label()).show_ui(ui, |ui| {
                for f in ExportFormat::ALL {
                    ui.selectable_value(&mut r.format, f, f.label());
                }
            });
            egui::ComboBox::from_label("Farbraum").selected_text(r.output_space.label()).show_ui(ui, |ui| {
                for sp in sk_core::color::OutputSpace::ALL {
                    ui.selectable_value(&mut r.output_space, sp, sp.label());
                }
            });
            if r.format == ExportFormat::Jpeg {
                ui.add(egui::Slider::new(&mut r.quality, 1..=100).text("Qualität"));
            }
            let mut resize = r.long_edge.is_some();
            ui.checkbox(&mut resize, "Skalieren (lange Kante)");
            if resize {
                let mut edge = r.long_edge.unwrap_or(2048);
                ui.add(egui::DragValue::new(&mut edge).range(64..=20000).suffix(" px"));
                r.long_edge = Some(edge);
            } else {
                r.long_edge = None;
            }
            ui.horizontal(|ui| {
                ui.label("Ordner");
                let mut dir = r.output_dir.to_string_lossy().to_string();
                if ui.text_edit_singleline(&mut dir).changed() {
                    r.output_dir = dir.into();
                }
            });
            ui.label(RichText::new("Relativ zum Bildordner, z. B. „Output“").small().color(Color32::from_gray(140)));
            ui.horizontal(|ui| {
                ui.label("Dateiname");
                ui.add(egui::TextEdit::singleline(&mut r.name_template).hint_text("{Name}"));
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (token, help) in sk_core::naming::TOKENS {
                    if ui.small_button(RichText::new(token).small()).on_hover_text(help).clicked() {
                        r.name_template.push_str(token);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Suffix");
                ui.text_edit_singleline(&mut r.suffix);
            });
        });
        section(ui, "Wasserzeichen", None, |ui| {
            let wm = &mut r.watermark;
            ui.checkbox(&mut wm.enabled, "Wasserzeichen einfügen");
            if !wm.enabled {
                return;
            }
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
                    ui.label(RichText::new("PNG mit Transparenz empfohlen").small().color(Color32::from_gray(140)));
                }
            }
            egui::ComboBox::from_label("Position").selected_text(wm.anchor.label()).show_ui(ui, |ui| {
                for a in Anchor::ALL {
                    ui.selectable_value(&mut wm.anchor, a, a.label());
                }
            });
            ui.add(egui::Slider::new(&mut wm.size, 1.0..=30.0).text("Größe").suffix(" %").fixed_decimals(1));
            ui.add(egui::Slider::new(&mut wm.margin, 0.0..=20.0).text("Abstand").suffix(" %").fixed_decimals(1));
            let mut opacity = wm.opacity * 100.0;
            if ui.add(egui::Slider::new(&mut opacity, 5.0..=100.0).text("Deckkraft").suffix(" %").fixed_decimals(0)).changed() {
                wm.opacity = opacity / 100.0;
            }
            ui.label(RichText::new("Größe und Abstand in % der kurzen Bildkante. Vorschau mit P (Proof).").small().color(Color32::from_gray(140)));
        });
        section(ui, "Metadaten", None, |ui| {
            ui.checkbox(&mut r.capture_metadata, "Aufnahmedaten übernehmen").on_hover_text("Kamera, Objektiv, Belichtung und Aufnahmedatum");
            ui.horizontal(|ui| {
                ui.label("Fotograf");
                ui.text_edit_singleline(&mut r.artist);
            });
            ui.horizontal(|ui| {
                ui.label("Copyright");
                ui.text_edit_singleline(&mut r.copyright);
            });
            let note = if r.format == ExportFormat::Jpeg || r.format == ExportFormat::Png {
                "GPS-Daten werden nie übernommen."
            } else {
                "TIFF-Dateien erhalten keine Metadaten."
            };
            ui.label(RichText::new(note).small().color(Color32::from_gray(140)));
        });
        section(ui, "Verarbeiten", None, |ui| {
            let n = self.selected_paths().len();
            let k = self.recipes.iter().filter(|r| r.enabled).count();
            if ui.add_enabled(n > 0 && k > 0, egui::Button::new(format!("Verarbeiten: {n} Bild(er) × {k} Rezept(e)"))).clicked() {
                self.export_selection();
            }
            if let Some((done, total)) = self.export_progress {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!("{done} / {total}")));
            }
            for msg in self.export_log.iter().rev().take(8) {
                ui.label(RichText::new(msg).small());
            }
        });
    }
}
