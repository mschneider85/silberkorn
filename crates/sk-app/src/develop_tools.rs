//! Tool tabs for geometry/lens, details and the advanced color tools.

use sk_core::auto;
use sk_core::params::{AspectRatio, ColorWheel, CropRect, Detail, HUE_BANDS, HslBand, LensCorrection, SkinTone};
use egui::{Color32, RichText, Ui};

use crate::app::App;
use crate::theme::icon;
use crate::widgets::{self, section, slider_row};

/// Approximate display color of a hue band (for swatches).
fn band_color(hue: f32) -> Color32 {
    band_hsv(hue, 0.75, 0.9)
}

/// A band hue (Oklab degrees) at the given HSV saturation and value.
fn band_hsv(hue: f32, s: f32, v: f32) -> Color32 {
    egui::ecolor::Hsva::new(((hue - 25.0 + 720.0) % 360.0 + 5.0) / 360.0, s, v, 1.0).into()
}

/// What the color editor shows: one property for all colors (hue, saturation or
/// lightness), or all properties of one color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorView {
    #[default]
    Hue,
    Saturation,
    Lightness,
    Single,
}

impl ColorView {
    const ALL: [ColorView; 4] = [ColorView::Hue, ColorView::Saturation, ColorView::Lightness, ColorView::Single];

    fn label(self) -> &'static str {
        match self {
            ColorView::Hue => "Farbton",
            ColorView::Saturation => "Sättigung",
            ColorView::Lightness => "Helligkeit",
            ColorView::Single => "Einzeln",
        }
    }
}

/// Rail colors of a band slider: the neighbouring hues, grey to color, or dark to light.
fn band_rail(view: ColorView, hue: f32) -> [Color32; 3] {
    match view {
        ColorView::Hue => [band_hsv(hue - 30.0, 0.75, 0.9), band_hsv(hue, 0.75, 0.9), band_hsv(hue + 30.0, 0.75, 0.9)],
        ColorView::Saturation => [Color32::from_gray(140), band_hsv(hue, 0.45, 0.85), band_hsv(hue, 0.95, 0.95)],
        ColorView::Lightness | ColorView::Single => [band_hsv(hue, 0.8, 0.3), band_hsv(hue, 0.75, 0.7), band_hsv(hue, 0.35, 1.0)],
    }
}

/// Slider of one property of a band, with its colored rail.
fn band_slider(ui: &mut Ui, label: &str, band: &mut HslBand, view: ColorView, hue: f32) -> bool {
    let rail = band_rail(view, hue);
    let opts = widgets::SliderOpts { rail: Some(&rail), ..Default::default() };
    match view {
        ColorView::Hue => widgets::slider_row_with(ui, label, &mut band.hue, -30.0..=30.0, 0.0, 1, opts),
        ColorView::Saturation => widgets::slider_row_with(ui, label, &mut band.saturation, -100.0..=100.0, 0.0, 0, opts),
        ColorView::Lightness | ColorView::Single => widgets::slider_row_with(ui, label, &mut band.lightness, -100.0..=100.0, 0.0, 0, opts),
    }
}

/// Round color swatch of a band: a ring when selected, a dot below when edited.
fn band_swatch(ui: &mut Ui, color: Color32, selected: bool, edited: bool, name: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 32.0), egui::Sense::click());
    let p = ui.painter();
    let c = rect.center_top() + egui::vec2(0.0, 13.0);
    p.circle_filled(c, 10.0, color);
    if selected {
        p.circle_stroke(c, 12.5, egui::Stroke::new(2.0, Color32::WHITE));
    } else if resp.hovered() {
        p.circle_stroke(c, 12.0, egui::Stroke::new(1.0, Color32::from_gray(150)));
    }
    if edited {
        p.circle_filled(rect.center_bottom() - egui::vec2(0.0, 2.5), 2.0, crate::theme::TEXT);
    }
    resp.on_hover_text(if edited { format!("{name} (bearbeitet)") } else { name.to_string() })
}

/// Display color of the skin tone target (Oklab in the working space).
fn skin_color(st: &SkinTone) -> Color32 {
    use sk_core::color::{Oklab, OutputSpace, mat_vec, srgb_encode};
    let hr = st.hue.to_radians();
    let linear = Oklab::default().to_linear([st.lightness, st.chroma * hr.cos(), st.chroma * hr.sin()]);
    let [r, g, b] = mat_vec(&OutputSpace::Srgb.from_working(), linear).map(|v| (srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
    Color32::from_rgb(r, g, b)
}

/// The automatic adjustments of the exposure tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auto {
    Exposure,
    Levels,
    WhiteBalance,
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
    changed |= crate::widgets::slider_row_with(ui, "Farbton", &mut wheel.hue, 0.0..=360.0, 0.0, 0, crate::widgets::SliderOpts { suffix: "°", rail: Some(crate::widgets::RAIL_HUE) });
    changed |= slider_row(ui, "Stärke", &mut wheel.amount, 0.0..=100.0, 0.0, 0);
    if with_lightness {
        changed |= slider_row(ui, "Helligkeit", &mut wheel.lightness, -100.0..=100.0, 0.0, 0);
    }
    changed
}

impl App {
    pub fn lens_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        let mut start_crop = false;
        let mut finish_crop = false;
        let mut apply_aspect = false;
        let mut toggle_straighten = false;
        let mut upright = None;
        let crop_active = self.crop_tool.is_some();
        let straighten_active = self.straighten_tool.is_some();
        let downloading = self.lens_db_downloading;
        let db_loaded = self.lens_db.is_some();
        let match_name = self.lens_profile_name();
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut changed = false;

        let mut reset_crop = false;
        section(ui, "Zuschnitt", Some(&mut || reset_crop = true), |ui| {
            ui.horizontal(|ui| {
                if crop_active {
                    if ui.button("\u{E182}  Übernehmen").on_hover_text("Enter").clicked() {
                        finish_crop = true;
                    }
                } else if ui.button("\u{E1D4}  Zuschneiden").on_hover_text("C").clicked() {
                    start_crop = true;
                }
            });
            let g = &mut adj.geometry;
            ui.horizontal(|ui| {
                widgets::combo_field(ui, "Seitenverhältnis", g.aspect.label(), |ui| {
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
                if ui.selectable_label(straighten_active, "\u{E6B8}  Linie ziehen").on_hover_text("Horizont oder Kante nachziehen").clicked() {
                    toggle_straighten = true;
                }
                if ui.button("\u{E038}  90°").clicked() {
                    adj.rotation = (adj.rotation + 3) % 4;
                    changed = true;
                }
                if ui.button("\u{E036}  90°").clicked() {
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
        section(ui, "Perspektive", Some(&mut || reset_key = true), |ui| {
            use sk_core::upright::UprightMode;
            ui.horizontal(|ui| {
                ui.label("Automatisch");
                for (mode, label, tip) in [
                    (UprightMode::Vertical, "Auto", "Drehung und stürzende Linien (Umschalt+A)"),
                    (UprightMode::Level, "Nur Drehung", "Horizont und Kanten waagerecht, ohne Perspektivkorrektur"),
                    (UprightMode::Full, "Voll", "Drehung, vertikale und horizontale Perspektive"),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        upright = Some(mode);
                    }
                }
            });
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
        let camera_source = cur.meta.lens.as_ref().map(|l| l.source);
        let mut profile_action: Option<bool> = None;
        section(ui, "Objektivkorrektur", Some(&mut || reset_lens = true), |ui| {
            ui.label(RichText::new(lens_name.clone().unwrap_or_else(|| "Objektiv unbekannt".into())).color(Color32::from_gray(170)));
            let l = &mut adj.lens;
            if let Some(source) = camera_source {
                let r = ui
                    .checkbox(&mut l.camera, format!("Korrektur aus den Kameradaten ({source})"))
                    .on_hover_text("Verzeichnung, Farbsäume und Vignettierung, wie die Kamera sie für dieses Objektiv gespeichert hat");
                if r.changed() {
                    changed = true;
                    // Never both: the profile would correct the same lens a second time.
                    if l.camera {
                        l.profile = None;
                        l.profile_coeffs = None;
                    }
                }
            }
            ui.horizontal(|ui| {
                match (&l.profile, &match_name) {
                    (Some(p), _) => {
                        ui.label(format!("Profil: {p}"));
                        if ui.button("Entfernen").clicked() {
                            profile_action = Some(false);
                        }
                    }
                    (None, Some(p)) => {
                        ui.label("Kein Profil");
                        if ui.button("Anwenden").on_hover_text(format!("Profil „{p}“")).clicked() {
                            profile_action = Some(true);
                        }
                    }
                    (None, None) if downloading || (!db_loaded && lens_name.is_some()) => {
                        ui.spinner();
                        ui.label("Objektivprofile werden geladen…");
                    }
                    (None, None) if lens_name.is_some() => {
                        ui.label(RichText::new("Kein Profil für dieses Objektiv").color(crate::theme::TEXT_FAINT));
                    }
                    (None, None) => {}
                }
            });
            if let Some(f) = &focal {
                ui.label(RichText::new(format!("Brennweite {f}")).small().color(crate::theme::TEXT_FAINT));
            }
            changed |= slider_row(ui, "Verzeichnung", &mut l.distortion, -100.0..=100.0, 0.0, 0);
            changed |= crate::widgets::slider_row_with(ui, "Fisheye", &mut l.fisheye, 0.0..=220.0, 0.0, 0, crate::widgets::SliderOpts { suffix: "°", ..Default::default() });
            ui.label(RichText::new("Fisheye: Bildwinkel über die Diagonale (meist 180°), rechnet in ein normales Weitwinkelbild um").small().color(crate::theme::TEXT_FAINT));
            changed |= slider_row(ui, "CA Rot/Cyan", &mut l.ca_red, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "CA Blau/Gelb", &mut l.ca_blue, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Vignettierung", &mut l.vignetting, -100.0..=100.0, 0.0, 0);
        });
        if reset_lens {
            adj.lens = LensCorrection::default();
            changed = true;
            // Back to what a new image gets.
            if camera_source.is_none() && match_name.is_some() {
                profile_action = Some(true);
            }
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
        if let Some(mode) = upright {
            self.auto_upright(mode);
        }
        if toggle_straighten {
            self.toggle_straighten();
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
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        let mut changed = self.noise_section(ui);
        let Some(cur) = self.current.as_mut() else { return };
        let d = &mut cur.adjustments.detail;
        let defaults = Detail::default();

        let (mut r_sharp, mut r_clarity, mut r_vig, mut r_grain) = (false, false, false, false);
        section(ui, "Schärfen", Some(&mut || r_sharp = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.sharpen_amount, 0.0..=500.0, defaults.sharpen_amount, 0);
            changed |= slider_row(ui, "Radius", &mut d.sharpen_radius, 0.2..=3.0, defaults.sharpen_radius, 1);
            changed |= slider_row(ui, "Schwelle", &mut d.sharpen_threshold, 0.0..=20.0, defaults.sharpen_threshold, 1);
            ui.label(RichText::new("Wirkung bei 100 % prüfen (Z)").small().color(crate::theme::TEXT_FAINT));
        });
        section(ui, "Klarheit", Some(&mut || r_clarity = true), |ui| {
            changed |= slider_row(ui, "Klarheit", &mut d.clarity, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Struktur", &mut d.structure, -100.0..=100.0, 0.0, 0);
        });
        let mut r_haze = false;
        section(ui, "Dunst entfernen", Some(&mut || r_haze = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.dehaze, -100.0..=100.0, 0.0, 0);
        });
        if r_haze {
            d.dehaze = 0.0;
            changed = true;
        }
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
        if r_clarity {
            (d.clarity, d.structure) = (0.0, 0.0);
        }
        if r_vig {
            d.vignette_amount = 0.0;
        }
        if r_grain {
            (d.grain_amount, d.grain_size) = (0.0, defaults.grain_size);
        }
        if changed || r_sharp || r_clarity || r_vig || r_grain {
            self.request_render();
        }
        self.lens_blur_section(ui);
    }

    /// Color editor, color balance and black & white (appended to the color tab).
    pub fn color_advanced(&mut self, ui: &mut Ui) {
        let band = &mut self.color_band;
        let view = &mut self.color_view;
        let picking_skin = &mut self.picking_skin;
        let mut toggle_skin = false;
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut changed = false;

        let mut reset_ed = false;
        section(ui, "Farbeditor", Some(&mut || reset_ed = true), |ui| {
            ui.horizontal(|ui| {
                for v in ColorView::ALL {
                    if ui.selectable_label(*view == v, v.label()).clicked() {
                        *view = v;
                    }
                }
            });
            ui.add_space(4.0);
            let bands = &mut adj.color_editor.bands;
            if *view == ColorView::Single {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ((ui.available_width() - 8.0 * 28.0) / 7.0).max(0.0);
                    for (i, (name, hue)) in HUE_BANDS.iter().enumerate() {
                        let edited = bands[i] != HslBand::default();
                        if band_swatch(ui, band_color(*hue), *band == i, edited, name).clicked() {
                            *band = i;
                        }
                    }
                });
                ui.add_space(4.0);
                let (_, hue) = HUE_BANDS[*band];
                let b = &mut bands[*band];
                changed |= band_slider(ui, "Farbton", b, ColorView::Hue, hue);
                changed |= band_slider(ui, "Sättigung", b, ColorView::Saturation, hue);
                changed |= band_slider(ui, "Helligkeit", b, ColorView::Lightness, hue);
            } else {
                for (i, (name, hue)) in HUE_BANDS.iter().enumerate() {
                    changed |= band_slider(ui, name, &mut bands[i], *view, *hue);
                }
            }
        });
        if reset_ed {
            adj.color_editor = Default::default();
            changed = true;
        }

        let mut reset_skin = false;
        section(ui, "Hauttöne", Some(&mut || reset_skin = true), |ui| {
            let st = &mut adj.skin_tone;
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, skin_color(st));
                ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);
                let pipette = format!("{}  Hautton aufnehmen", crate::theme::icon::EYEDROPPER);
                if ui.selectable_label(*picking_skin, pipette).on_hover_text("Auf eine typische Hautstelle im Bild klicken").clicked() {
                    toggle_skin = true;
                }
            });
            changed |= slider_row(ui, "Bereich", &mut st.range, 10.0..=60.0, SkinTone::default().range, 0);
            ui.label(RichText::new("Gleichmäßigkeit").strong());
            changed |= slider_row(ui, "Farbton", &mut st.hue_uniformity, 0.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Sättigung", &mut st.saturation_uniformity, 0.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Helligkeit", &mut st.lightness_uniformity, 0.0..=100.0, 0.0, 0);
            ui.label(RichText::new("Hautton verändern").strong());
            changed |= slider_row(ui, "Farbton", &mut st.hue_shift, -30.0..=30.0, 0.0, 1);
            changed |= slider_row(ui, "Sättigung", &mut st.saturation, -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Helligkeit", &mut st.lightness_shift, -100.0..=100.0, 0.0, 0);
        });
        if reset_skin {
            // The picked target stays.
            let st = adj.skin_tone;
            adj.skin_tone = SkinTone { hue: st.hue, chroma: st.chroma, lightness: st.lightness, ..Default::default() };
            changed = true;
        }

        let mut reset_cb = false;
        section(ui, "Farbbalance", Some(&mut || reset_cb = true), |ui| {
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
        if toggle_skin {
            self.toggle_skin_picker();
        }
    }

    /// "Auto" buttons for the exposure tab.
    pub fn auto_section(&mut self, ui: &mut Ui) {
        let mut action = None;
        ui.horizontal(|ui| {
            for (kind, text, hover) in [
                (Auto::Exposure, "Auto-Belichtung", "Belichtung so setzen, dass die Mitteltöne stimmen"),
                (Auto::Levels, "Auto-Tonwerte", "Schwarz- und Weißpunkt aus dem Histogramm"),
                (Auto::WhiteBalance, "Auto-Weißabgleich", "Weißabgleich aus dem Bildinhalt schätzen"),
            ] {
                if ui.add_enabled(self.current.is_some(), egui::Button::new(format!("{}  {text}", icon::MAGIC_WAND))).on_hover_text(hover).clicked() {
                    action = Some(kind);
                }
            }
        });
        if let Some(kind) = action {
            self.run_auto(kind);
        }
    }

    pub fn run_auto(&mut self, kind: Auto) {
        let Some(cur) = self.current.as_mut() else { return };
        match kind {
            Auto::Exposure => {
                cur.adjustments.exposure = auto::auto_exposure(&cur.meta, &cur.adjustments, &cur.preview);
            }
            Auto::Levels => {
                let Some(h) = &self.histogram else { return };
                cur.adjustments.levels = auto::auto_levels(h);
            }
            Auto::WhiteBalance => {
                cur.adjustments.white_balance = auto::auto_white_balance(&cur.meta, &cur.preview);
            }
        }
        self.request_render();
    }

    /// The lensfun profile for the current image's lens: name and coefficients at its focal length.
    fn lens_profile(&self) -> Option<(String, sk_core::params::LensCoefficients)> {
        let (db, cur) = (self.lens_db.as_ref()?, self.current.as_ref()?);
        let profile = sk_core::lensfun::find(db, cur.meta.info.lens.as_deref()?)?;
        let coeffs = profile.coefficients(cur.meta.info.focal_mm.unwrap_or(50.0), cur.meta.info.fnumber);
        Some((profile.name(), coeffs))
    }

    /// Name of the lensfun profile for the current lens, cached per lens name.
    fn lens_profile_name(&mut self) -> Option<String> {
        let lens = self.current.as_ref()?.meta.info.lens.clone()?;
        self.lens_db.as_ref()?;
        if self.lens_match.as_ref().is_none_or(|(l, _)| *l != lens) {
            self.lens_match = Some((lens.clone(), self.lens_profile().map(|p| p.0)));
        }
        self.lens_match.as_ref().and_then(|m| m.1.clone())
    }

    /// Applies the lensfun profile to the current image.
    pub fn apply_lens_profile(&mut self) {
        let Some((name, coeffs)) = self.lens_profile() else { return };
        self.edit(|a| {
            a.lens.profile = Some(name.clone());
            a.lens.profile_coeffs = Some(coeffs);
            // Never both: the profile would correct the same lens a second time.
            a.lens.camera = false;
        });
        self.status = format!("Objektivprofil: {name}");
    }

    /// Applies the lensfun profile to an image that has never been edited and whose raw file
    /// carries no lens correction of its own, as on import. Saved right
    /// away, so that export and thumbnails see it too. Returns whether it applied one.
    pub fn auto_lens_profile(&mut self) -> bool {
        let Some(cur) = &self.current else { return false };
        if cur.meta.lens.is_some() || cur.adjustments != cur.committed || cur.committed.clone().normalized() != sk_core::params::Adjustments::default() {
            return false;
        }
        let Some((name, coeffs)) = self.lens_profile() else { return false };
        let Some(cur) = self.current.as_mut() else { return false };
        for a in [&mut cur.adjustments, &mut cur.committed] {
            a.lens.profile = Some(name.clone());
            a.lens.profile_coeffs = Some(coeffs);
        }
        self.sidecar_dirty = true;
        self.commit_and_save();
        true
    }
}
