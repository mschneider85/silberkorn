//! Tool tabs for geometry/lens, details and the advanced color tools.

use cr_core::auto;
use cr_core::params::{AspectRatio, ColorWheel, CropRect, Detail, HUE_BANDS, HslBand, LensCorrection};
use egui::{Color32, RichText, Ui};

use crate::app::App;
use crate::widgets::{section, slider_row};

/// Approximate display color of a hue band (for swatches).
fn band_color(hue: f32) -> Color32 {
    let c = egui::ecolor::Hsva::new(((hue - 25.0 + 360.0) % 360.0 + 5.0) / 360.0, 0.75, 0.9, 1.0);
    c.into()
}

/// Hue slider with a colored swatch; returns true when changed.
fn wheel_ui(ui: &mut Ui, label: &str, wheel: &mut ColorWheel, with_lightness: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        let c = egui::ecolor::Hsva::new(wheel.hue / 360.0, (wheel.amount / 100.0).max(0.15), 0.9, 1.0);
        ui.painter().circle_filled(rect.center(), 6.0, Color32::from(c));
        ui.label(RichText::new(label).strong());
    });
    changed |= slider_row(ui, "Farbton", &mut wheel.hue, 0.0..=360.0, 0.0, 0);
    changed |= slider_row(ui, "Stärke", &mut wheel.amount, 0.0..=100.0, 0.0, 0);
    if with_lightness {
        changed |= slider_row(ui, "Helligkeit", &mut wheel.lightness, -100.0..=100.0, 0.0, 0);
    }
    changed
}

impl App {
    pub fn lens_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            ui.label("Kein Bild ausgewählt.");
            return;
        }
        let mut start_crop = false;
        let mut finish_crop = false;
        let mut apply_aspect = false;
        let mut toggle_straighten = false;
        let crop_active = self.crop_tool.is_some();
        let straighten_active = self.straighten_tool.is_some();
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut changed = false;

        let mut reset_crop = false;
        section(ui, "Zuschnitt", Some(&mut || reset_crop = true), |ui| {
            ui.horizontal(|ui| {
                if crop_active {
                    if ui.button("✔ Übernehmen").on_hover_text("Enter").clicked() {
                        finish_crop = true;
                    }
                } else if ui.button("⛶ Zuschneiden").on_hover_text("C").clicked() {
                    start_crop = true;
                }
            });
            let g = &mut adj.geometry;
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Seitenverhältnis").selected_text(g.aspect.label()).show_ui(ui, |ui| {
                    for a in AspectRatio::PRESETS {
                        if ui.selectable_value(&mut g.aspect, a, a.label()).changed() {
                            apply_aspect = true;
                        }
                    }
                });
            });
            if !matches!(g.aspect, AspectRatio::Free) && ui.checkbox(&mut g.aspect_portrait, "Hochformat").changed() {
                apply_aspect = true;
            }
            let c = g.crop;
            ui.label(RichText::new(format!("{:.0} % × {:.0} %", c.w * 100.0, c.h * 100.0)).color(Color32::from_gray(150)));
        });
        if reset_crop {
            adj.geometry.crop = CropRect::default();
            changed = true;
        }

        let mut reset_rot = false;
        section(ui, "Drehen & Spiegeln", Some(&mut || reset_rot = true), |ui| {
            let g = &mut adj.geometry;
            changed |= slider_row(ui, "Geraderichten", &mut g.straighten, -45.0..=45.0, 0.0, 1);
            ui.horizontal(|ui| {
                if ui.selectable_label(straighten_active, "📏 Linie ziehen").on_hover_text("Horizont oder Kante nachziehen").clicked() {
                    toggle_straighten = true;
                }
                if ui.button("⟲ 90°").clicked() {
                    adj.rotation = (adj.rotation + 3) % 4;
                    changed = true;
                }
                if ui.button("⟳ 90°").clicked() {
                    adj.rotation = (adj.rotation + 1) % 4;
                    changed = true;
                }
            });
            let g = &mut adj.geometry;
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut g.flip_h, "Horizontal spiegeln").changed();
                changed |= ui.checkbox(&mut g.flip_v, "Vertikal").changed();
            });
        });
        if reset_rot {
            adj.geometry.straighten = 0.0;
            adj.geometry.flip_h = false;
            adj.geometry.flip_v = false;
            changed = true;
        }

        let mut reset_key = false;
        section(ui, "Keystone (Perspektive)", Some(&mut || reset_key = true), |ui| {
            let g = &mut adj.geometry;
            changed |= slider_row(ui, "Vertikal", &mut g.keystone_v, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Horizontal", &mut g.keystone_h, -100.0..=100.0, 0.0, 0);
        });
        if reset_key {
            adj.geometry.keystone_v = 0.0;
            adj.geometry.keystone_h = 0.0;
            changed = true;
        }

        let mut reset_lens = false;
        let lens_name = cur.meta.info.lens.clone();
        let focal = cur.meta.info.focal_length.clone();
        let mut profile_action: Option<bool> = None;
        let (db_missing, downloading) = (self.lens_db_missing, self.lens_db_downloading);
        let mut download = false;
        section(ui, "Objektivkorrektur", Some(&mut || reset_lens = true), |ui| {
            ui.label(RichText::new(lens_name.clone().unwrap_or_else(|| "Objektiv unbekannt".into())).color(Color32::from_gray(170)));
            let l = &mut adj.lens;
            ui.horizontal(|ui| {
                let label = l.profile.clone().unwrap_or_else(|| "Kein Profil".into());
                ui.label(format!("Profil: {label}"));
            });
            ui.horizontal(|ui| {
                if ui.button("Profil suchen").on_hover_text("Lensfun-Datenbank").clicked() {
                    profile_action = Some(true);
                }
                if l.profile.is_some() && ui.button("Entfernen").clicked() {
                    profile_action = Some(false);
                }
            });
            if let Some(f) = &focal {
                ui.label(RichText::new(format!("Brennweite {f}")).small().color(Color32::from_gray(140)));
            }
            if db_missing {
                ui.horizontal(|ui| {
                    if downloading {
                        ui.spinner();
                        ui.label("Lade Datenbank…");
                    } else if ui.button("Objektivdatenbank herunterladen").on_hover_text("lensfun-Datenbank von GitHub (CC BY-SA 3.0)").clicked() {
                        profile_action = None;
                        download = true;
                    }
                });
            }
            changed |= slider_row(ui, "Verzeichnung", &mut l.distortion, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "CA Rot/Cyan", &mut l.ca_red, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "CA Blau/Gelb", &mut l.ca_blue, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Vignettierung", &mut l.vignetting, -100.0..=100.0, 0.0, 0);
        });
        if reset_lens {
            adj.lens = LensCorrection::default();
            changed = true;
        }

        if changed {
            self.request_render();
        }
        if start_crop {
            self.start_crop_tool();
        }
        if finish_crop {
            self.finish_crop_tool();
        }
        if apply_aspect {
            self.apply_aspect();
            if self.crop_tool.is_none() {
                self.request_render();
            }
        }
        if toggle_straighten {
            self.straighten_tool = if straighten_active { None } else { Some(Default::default()) };
            self.finish_crop_tool();
        }
        if download {
            self.lens_db_downloading = true;
            self.workers.download_lens_db();
        }
        match profile_action {
            Some(true) => self.apply_lens_profile(),
            Some(false) => self.edit(|a| {
                a.lens.profile = None;
                a.lens.profile_coeffs = None;
            }),
            None => {}
        }
    }

    pub fn details_tab(&mut self, ui: &mut Ui) {
        let Some(cur) = self.current.as_mut() else {
            ui.label("Kein Bild ausgewählt.");
            return;
        };
        let d = &mut cur.adjustments.detail;
        let mut changed = false;
        let defaults = Detail::default();

        let (mut r_sharp, mut r_nr, mut r_clarity, mut r_vig, mut r_grain) = (false, false, false, false, false);
        section(ui, "Schärfen", Some(&mut || r_sharp = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.sharpen_amount, 0.0..=500.0, defaults.sharpen_amount, 0);
            changed |= slider_row(ui, "Radius", &mut d.sharpen_radius, 0.2..=3.0, defaults.sharpen_radius, 1);
            changed |= slider_row(ui, "Schwelle", &mut d.sharpen_threshold, 0.0..=20.0, defaults.sharpen_threshold, 1);
            ui.label(RichText::new("Wirkung bei 100 % prüfen (Z)").small().color(Color32::from_gray(140)));
        });
        section(ui, "Rauschreduzierung", Some(&mut || r_nr = true), |ui| {
            changed |= slider_row(ui, "Luminanz", &mut d.nr_luma, 0.0..=100.0, defaults.nr_luma, 0);
            changed |= slider_row(ui, "Farbe", &mut d.nr_color, 0.0..=100.0, defaults.nr_color, 0);
        });
        section(ui, "Klarheit", Some(&mut || r_clarity = true), |ui| {
            changed |= slider_row(ui, "Klarheit", &mut d.clarity, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Struktur", &mut d.structure, -100.0..=100.0, 0.0, 0);
        });
        section(ui, "Vignettierung", Some(&mut || r_vig = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.vignette_amount, -100.0..=100.0, 0.0, 0);
        });
        section(ui, "Körnung", Some(&mut || r_grain = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.grain_amount, 0.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Größe", &mut d.grain_size, 0.0..=100.0, defaults.grain_size, 0);
        });
        if r_sharp {
            (d.sharpen_amount, d.sharpen_radius, d.sharpen_threshold) = (defaults.sharpen_amount, defaults.sharpen_radius, defaults.sharpen_threshold);
        }
        if r_nr {
            (d.nr_luma, d.nr_color) = (defaults.nr_luma, defaults.nr_color);
        }
        if r_clarity {
            (d.clarity, d.structure) = (0.0, 0.0);
        }
        if r_vig {
            d.vignette_amount = 0.0;
        }
        if r_grain {
            (d.grain_amount, d.grain_size) = (0.0, defaults.grain_size);
        }
        if changed || r_sharp || r_nr || r_clarity || r_vig || r_grain {
            self.request_render();
        }
    }

    /// Color editor, color balance and black & white (appended to the color tab).
    pub fn color_advanced(&mut self, ui: &mut Ui) {
        let band = &mut self.color_band;
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut changed = false;

        let mut reset_ed = false;
        section(ui, "Color Editor", Some(&mut || reset_ed = true), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, (name, hue)) in HUE_BANDS.iter().enumerate() {
                    let edited = adj.color_editor.bands[i] != HslBand::default();
                    let text = RichText::new(if edited { format!("{name}•") } else { name.to_string() }).color(band_color(*hue));
                    if ui.selectable_label(*band == i, text).clicked() {
                        *band = i;
                    }
                }
            });
            let b = &mut adj.color_editor.bands[*band];
            changed |= slider_row(ui, "Farbton", &mut b.hue, -30.0..=30.0, 0.0, 1);
            changed |= slider_row(ui, "Sättigung", &mut b.saturation, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Helligkeit", &mut b.lightness, -100.0..=100.0, 0.0, 0);
        });
        if reset_ed {
            adj.color_editor = Default::default();
            changed = true;
        }

        let mut reset_cb = false;
        section(ui, "Color Balance", Some(&mut || reset_cb = true), |ui| {
            let cb = &mut adj.color_balance;
            changed |= wheel_ui(ui, "Master", &mut cb.master, false);
            changed |= wheel_ui(ui, "Schatten", &mut cb.shadows, true);
            changed |= wheel_ui(ui, "Mitteltöne", &mut cb.midtones, true);
            changed |= wheel_ui(ui, "Lichter", &mut cb.highlights, true);
        });
        if reset_cb {
            adj.color_balance = Default::default();
            changed = true;
        }

        let mut reset_bw = false;
        section(ui, "Schwarzweiß", Some(&mut || reset_bw = true), |ui| {
            let bw = &mut adj.black_white;
            changed |= ui.checkbox(&mut bw.enabled, "Schwarzweiß aktivieren").changed();
            if bw.enabled {
                ui.label(RichText::new("Farbempfindlichkeit").strong());
                for (i, (name, _)) in HUE_BANDS.iter().enumerate() {
                    changed |= slider_row(ui, name, &mut bw.mix[i], -100.0..=100.0, 0.0, 0);
                }
                ui.label(RichText::new("Tonung").strong());
                changed |= wheel_ui(ui, "Schatten", &mut bw.shadow_tint, false);
                changed |= wheel_ui(ui, "Lichter", &mut bw.highlight_tint, false);
            }
        });
        if reset_bw {
            adj.black_white = Default::default();
            changed = true;
        }
        if changed {
            self.request_render();
        }
    }

    /// "Auto" buttons for the exposure tab.
    pub fn auto_section(&mut self, ui: &mut Ui) {
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button("Auto Belichtung").clicked() {
                action = Some(0);
            }
            if ui.button("Auto Levels").clicked() {
                action = Some(1);
            }
            if ui.button("Auto WB").clicked() {
                action = Some(2);
            }
        });
        let Some(cur) = self.current.as_mut() else { return };
        match action {
            Some(0) => {
                cur.adjustments.exposure = auto::auto_exposure(&cur.meta, &cur.adjustments, &cur.preview);
            }
            Some(1) => {
                if let Some(h) = &self.histogram {
                    cur.adjustments.levels = auto::auto_levels(h);
                }
            }
            Some(2) => {
                cur.adjustments.white_balance = auto::auto_white_balance(&cur.meta, &cur.preview);
            }
            _ => return,
        }
        self.request_render();
    }

    /// Looks up the lens of the current image in the lensfun database and applies its profile.
    pub fn apply_lens_profile(&mut self) {
        let db = self.lens_db.get_or_insert_with(|| std::sync::Arc::new(cr_core::lensfun::load_database())).clone();
        let Some(cur) = &self.current else { return };
        if db.is_empty() {
            self.status = "Keine Objektivdatenbank gefunden – bitte herunterladen".into();
            self.lens_db_missing = true;
            return;
        }
        self.lens_db_missing = false;
        let Some(lens) = cur.meta.info.lens.clone() else {
            self.status = "Objektiv in den Metadaten nicht angegeben".into();
            return;
        };
        let Some(profile) = cr_core::lensfun::find(&db, &lens) else {
            self.status = format!("Kein Profil für „{lens}“ gefunden");
            return;
        };
        let focal = cur.meta.info.focal_mm.unwrap_or(50.0);
        let coeffs = profile.coefficients(focal, cur.meta.info.fnumber);
        let name = profile.name();
        self.edit(|a| {
            a.lens.profile = Some(name.clone());
            a.lens.profile_coeffs = Some(coeffs);
        });
        self.status = format!("Objektivprofil: {name} @ {focal:.0} mm");
    }
}
