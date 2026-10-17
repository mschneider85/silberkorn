//! Tool tabs for geometry/lens, details and the advanced color tools.

use sk_core::auto;
use sk_core::params::{AspectRatio, ColorWheel, ColorZone, CropRect, Detail, HUE_BANDS, HslBand, LensCorrection, MAX_COLOR_ZONES, SkinTone};
use egui::{Color32, RichText, Ui};

use crate::app::App;
use crate::layer_target::Target;
use crate::theme::icon;
use crate::widgets::{self, slider_row};

/// Approximate display color of a hue band (for swatches).
fn band_color(hue: f32) -> Color32 {
    band_hsv(hue, 0.75, 0.9)
}

/// A band hue (Oklab degrees) at the given HSV saturation and value.
fn band_hsv(hue: f32, s: f32, v: f32) -> Color32 {
    egui::ecolor::Hsva::new(((hue - 25.0 + 720.0) % 360.0 + 5.0) / 360.0, s, v, 1.0).into()
}

/// The last auto adjustment of an image: its strength can still be changed,
/// between the settings before and the full auto result.
pub struct AutoMix {
    pub path: std::path::PathBuf,
    pub before: sk_core::params::Adjustments,
    pub auto: sk_core::params::Adjustments,
    pub opts: auto::AutoOptions,
    /// 0..100 %
    pub strength: f32,
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
    /// Color ranges of one's own.
    Zones,
}

impl ColorView {
    const ALL: [ColorView; 5] = [ColorView::Hue, ColorView::Saturation, ColorView::Lightness, ColorView::Single, ColorView::Zones];

    fn label(self) -> &'static str {
        match self {
            ColorView::Hue => "Farbton",
            ColorView::Saturation => "Sättigung",
            ColorView::Lightness => "Helligkeit",
            ColorView::Single => "Einzeln",
            ColorView::Zones => "Eigene",
        }
    }
}

/// What the color pipette picks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorPick {
    #[default]
    Skin,
    /// A new color range of one's own.
    NewZone,
    /// The color of this range.
    Zone(usize),
}

/// The hue circle as a strip, darkened outside the range (soft edge included).
fn zone_strip(ui: &mut Ui, zone: &ColorZone) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), egui::Sense::hover());
    let p = ui.painter();
    let n = 120;
    let w = rect.width() / n as f32;
    for i in 0..n {
        let h = (i as f32 + 0.5) * 360.0 / n as f32;
        let r = egui::Rect::from_min_size(egui::pos2(rect.left() + i as f32 * w, rect.top()), egui::vec2(w + 0.5, rect.height()));
        p.rect_filled(r, 0.0, band_hsv(h, 0.75, 0.9));
        let shade = 1.0 - zone.weight(h);
        if shade > 0.0 {
            p.rect_filled(r, 0.0, Color32::from_black_alpha((shade * 200.0) as u8));
        }
    }
    p.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, Color32::from_gray(70)), egui::StrokeKind::Outside);
}

/// Colors of the hue circle, for the rail of the center slider.
fn hue_rail() -> Vec<Color32> {
    (0..=12).map(|i| band_hsv(i as f32 * 30.0, 0.75, 0.9)).collect()
}

/// Rail colors of a band slider: the neighbouring hues, grey to color, or dark to light.
fn band_rail(view: ColorView, hue: f32) -> [Color32; 3] {
    match view {
        ColorView::Hue => [band_hsv(hue - 30.0, 0.75, 0.9), band_hsv(hue, 0.75, 0.9), band_hsv(hue + 30.0, 0.75, 0.9)],
        ColorView::Saturation => [Color32::from_gray(140), band_hsv(hue, 0.45, 0.85), band_hsv(hue, 0.95, 0.95)],
        ColorView::Lightness | ColorView::Single | ColorView::Zones => [band_hsv(hue, 0.8, 0.3), band_hsv(hue, 0.75, 0.7), band_hsv(hue, 0.35, 1.0)],
    }
}

/// Slider of one property of a band, with its colored rail.
fn band_slider(ui: &mut Ui, label: &str, band: &mut HslBand, view: ColorView, hue: f32) -> bool {
    let rail = band_rail(view, hue);
    let opts = widgets::SliderOpts { rail: Some(&rail), ..Default::default() };
    match view {
        ColorView::Hue => widgets::slider_row_with(ui, label, &mut band.hue, -30.0..=30.0, 0.0, 1, opts),
        ColorView::Saturation => widgets::slider_row_with(ui, label, &mut band.saturation, -100.0..=100.0, 0.0, 0, opts),
        ColorView::Lightness | ColorView::Single | ColorView::Zones => widgets::slider_row_with(ui, label, &mut band.lightness, -100.0..=100.0, 0.0, 0, opts),
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

/// Hue slider with a colored swatch; returns true when changed.
fn wheel_ui(ui: &mut Ui, label: &str, wheel: &mut ColorWheel, with_lightness: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        let c = egui::ecolor::Hsva::new(wheel.hue / 360.0, (wheel.amount / 100.0).max(0.15), 0.9, 1.0);
        ui.painter().circle_filled(rect.center(), 6.0, Color32::from(c));
        ui.label(RichText::new(label).strong());
    });
    changed |= crate::widgets::slider_row_with(ui, "Farbton", &mut wheel.hue, 0.0..=360.0, 0.0, 0, crate::widgets::SliderOpts { suffix: "°", rail: Some(crate::widgets::RAIL_HUE), ..Default::default() });
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

        let mut reset_lens = false;
        let lens_name = cur.meta.info.lens.clone();
        let focal = cur.meta.info.focal_length.clone();
        let camera_source = cur.meta.lens.as_ref().map(|l| l.source);
        let mut profile_action: Option<bool> = None;
        widgets::tool_section(ui, sk_core::tools::Tool::LensCorrection, Some(&mut || reset_lens = true), |ui| {
            ui.label(RichText::new(lens_name.clone().unwrap_or_else(|| "Objektiv unbekannt".into())).color(Color32::from_gray(170)));
            let l = &mut adj.lens;
            if let Some(source) = camera_source {
                let r = ui
                    .checkbox(&mut l.camera, format!("Korrektur aus den Kameradaten ({source})"))
                    .on_hover_text("Verzeichnung, Farbsäume und Randabschattung, wie die Kamera sie für dieses Objektiv gespeichert hat");
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
                        // Right to left, so the button keeps its place and a long name is cut.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Entfernen").clicked() {
                                profile_action = Some(false);
                            }
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(format!("Profil: {p}")).truncate()).on_hover_text(p.as_str());
                            });
                        });
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
            changed |= slider_row(ui, "Randabschattung", &mut l.vignetting, -100.0..=100.0, 0.0, 0);
        });
        if reset_lens {
            adj.lens = LensCorrection::default();
            changed = true;
            // Back to what a new image gets.
            if camera_source.is_none() && match_name.is_some() {
                profile_action = Some(true);
            }
        }

        let mut reset_crop = false;
        widgets::tool_section(ui, sk_core::tools::Tool::Crop, Some(&mut || reset_crop = true), |ui| {
            ui.horizontal(|ui| {
                if crop_active {
                    if ui.button("\u{E182}  Übernehmen").on_hover_text("Enter").clicked() {
                        finish_crop = true;
                    }
                } else if ui.button("\u{E1D4}  Zuschneiden").on_hover_text(crate::commands::hint("Zuschneiden", "crop")).clicked() {
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
        widgets::tool_section(ui, sk_core::tools::Tool::Rotation, Some(&mut || reset_rot = true), |ui| {
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
        widgets::tool_section(ui, sk_core::tools::Tool::Perspective, Some(&mut || reset_key = true), |ui| {
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
        let mut t = Target::new(&cur.adjustments, self.active_layer);
        let d = &mut cur.adjustments.detail;
        let defaults = Detail::default();

        let (mut r_sharp, mut r_clarity, mut r_haze) = (false, false, false);
        widgets::tool_section(ui, sk_core::tools::Tool::Sharpening, Some(&mut || r_sharp = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.sharpen_amount, 0.0..=500.0, defaults.sharpen_amount, 0);
            // Without an effect, what shapes it is shown but disabled (here and below).
            ui.add_enabled_ui(d.sharpen_amount > 0.0, |ui| {
                changed |= slider_row(ui, "Radius", &mut d.sharpen_radius, 0.2..=3.0, defaults.sharpen_radius, 1);
                changed |= slider_row(ui, "Schwelle", &mut d.sharpen_threshold, 0.0..=20.0, defaults.sharpen_threshold, 1);
            });
            ui.label(RichText::new(crate::commands::hint("Wirkung bei 100 % prüfen", "zoom-100")).small().color(crate::theme::TEXT_FAINT));
        });
        widgets::tool_section(ui, sk_core::tools::Tool::Clarity, Some(&mut || r_clarity = true), |ui| {
            changed |= slider_row(ui, "Klarheit", t.pick(&mut d.clarity, |l| &mut l.clarity), -100.0..=100.0, 0.0, 0);
            changed |= slider_row(ui, "Struktur", t.pick(&mut d.structure, |l| &mut l.structure), -100.0..=100.0, 0.0, 0);
        });
        widgets::tool_section(ui, sk_core::tools::Tool::Dehaze, Some(&mut || r_haze = true), |ui| {
            changed |= slider_row(ui, "Stärke", t.pick(&mut d.dehaze, |l| &mut l.dehaze), -100.0..=100.0, 0.0, 0);
        });
        if r_sharp {
            (d.sharpen_amount, d.sharpen_radius, d.sharpen_threshold) = (defaults.sharpen_amount, defaults.sharpen_radius, defaults.sharpen_threshold);
        }
        if r_clarity {
            match t.layer_mut() {
                Some(l) => (l.clarity, l.structure) = (0.0, 0.0),
                None => (d.clarity, d.structure) = (0.0, 0.0),
            }
        }
        if r_haze {
            match t.layer_mut() {
                Some(l) => l.dehaze = 0.0,
                None => d.dehaze = 0.0,
            }
        }
        t.store(&mut cur.adjustments);
        if changed || r_sharp || r_clarity || r_haze {
            self.request_render();
        }
    }

    /// Creative looks: depth effects first, the finishing touches (vignette, grain) last.
    pub fn effects_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        self.lens_blur_section(ui);
        self.atmosphere_section(ui);
        let Some(cur) = self.current.as_mut() else { return };
        let d = &mut cur.adjustments.detail;
        let defaults = Detail::default();
        let mut changed = false;

        let (mut r_glow, mut r_vig, mut r_grain) = (false, false, false);
        let glow_help = "Lässt Lichter leuchten und in ihre Umgebung strahlen, wie der Orton-Effekt.\n\nGröße: wie weit das Leuchten reicht.";
        widgets::tool_section_help(ui, sk_core::tools::Tool::Glow, glow_help, Some(&mut || r_glow = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.glow_amount, 0.0..=100.0, 0.0, 0);
            ui.add_enabled_ui(d.glow_amount > 0.0, |ui| {
                changed |= slider_row(ui, "Größe", &mut d.glow_size, 0.0..=100.0, sk_core::params::GLOW_SIZE, 0);
            });
        });
        widgets::tool_section(ui, sk_core::tools::Tool::Vignette, Some(&mut || r_vig = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.vignette_amount, -100.0..=100.0, 0.0, 0);
        });
        widgets::tool_section(ui, sk_core::tools::Tool::Grain, Some(&mut || r_grain = true), |ui| {
            changed |= slider_row(ui, "Stärke", &mut d.grain_amount, 0.0..=100.0, 0.0, 0);
            ui.add_enabled_ui(d.grain_amount > 0.0, |ui| {
                changed |= slider_row(ui, "Größe", &mut d.grain_size, 0.0..=100.0, defaults.grain_size, 0);
            });
        });
        if r_glow {
            (d.glow_amount, d.glow_size) = (0.0, sk_core::params::GLOW_SIZE);
        }
        if r_vig {
            d.vignette_amount = 0.0;
        }
        if r_grain {
            (d.grain_amount, d.grain_size) = (0.0, defaults.grain_size);
        }
        if changed || r_glow || r_vig || r_grain {
            self.request_render();
        }
    }

    /// Color editor, color balance and black & white (appended to the color tab).
    pub fn color_advanced(&mut self, ui: &mut Ui) {
        let band = &mut self.color_band;
        let view = &mut self.color_view;
        let zone_sel = &mut self.color_zone;
        let picking = |t: ColorPick| self.picking_skin && self.color_pick == t;
        let (picking_skin, picking_new_zone) = (picking(ColorPick::Skin), picking(ColorPick::NewZone));
        let picking_zone = match self.color_pick {
            ColorPick::Zone(i) if self.picking_skin => Some(i),
            _ => None,
        };
        let mut toggle_pick: Option<ColorPick> = None;
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let mut t = Target::new(adj, self.active_layer);
        let on_layer = t.is_layer();
        let mut changed = false;

        let mut reset_ed = false;
        widgets::tool_section(ui, sk_core::tools::Tool::ColorEditor, Some(&mut || reset_ed = true), |ui| {
            // Layers shift the bands; ranges of one's own belong to the whole image.
            if on_layer && *view == ColorView::Zones {
                *view = ColorView::Single;
            }
            ui.horizontal_wrapped(|ui| {
                // Ranges of one's own only for the whole image.
                for v in ColorView::ALL {
                    let zones = v == ColorView::Zones;
                    let r = ui.add_enabled(!(zones && on_layer), egui::Button::selectable(*view == v, v.label()));
                    let r = if zones {
                        r.on_hover_text("Eigene Farbbereiche: eine Farbe im Bild aufnehmen und nur sie ändern").on_disabled_hover_text(crate::widgets::ONLY_IMAGE)
                    } else {
                        r
                    };
                    if r.clicked() {
                        *view = v;
                    }
                }
            });
            ui.add_space(4.0);
            let bands = match t.layer_mut() {
                Some(l) => &mut l.color_bands,
                None => &mut adj.color_editor.bands,
            };
            if *view == ColorView::Zones {
                let zones = &mut adj.color_editor.zones;
                ui.horizontal_wrapped(|ui| {
                    for (i, z) in zones.iter().enumerate() {
                        let name = format!("Bereich {}", i + 1);
                        let color = if z.enabled { band_color(z.hue) } else { band_hsv(z.hue, 0.2, 0.5) };
                        if band_swatch(ui, color, *zone_sel == i, z.is_active(), &name).clicked() {
                            *zone_sel = i;
                        }
                    }
                    let full = zones.len() >= MAX_COLOR_ZONES;
                    let r = ui
                        .add_enabled(!full, egui::Button::new(format!("{}  Aufnehmen", icon::EYEDROPPER)).selected(picking_new_zone))
                        .on_hover_text("Neuer Bereich: auf die Farbe im Bild klicken")
                        .on_disabled_hover_text(format!("Höchstens {MAX_COLOR_ZONES} Bereiche"));
                    if r.clicked() {
                        toggle_pick = Some(ColorPick::NewZone);
                    }
                });
                *zone_sel = (*zone_sel).min(zones.len().saturating_sub(1));
                if zones.is_empty() {
                    ui.label(
                        RichText::new("Eine Farbe im Bild aufnehmen; nur sie und ähnliche Farbtöne ändern sich dann, Breite und Weichheit bestimmen wie viele.")
                            .small()
                            .color(crate::theme::TEXT_FAINT),
                    );
                } else {
                    let i = *zone_sel;
                    let mut delete = false;
                    let z = &mut zones[i];
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut z.enabled, format!("Bereich {}", i + 1)).changed();
                        if ui.selectable_label(picking_zone == Some(i), icon::EYEDROPPER).on_hover_text("Farbe neu aufnehmen").clicked() {
                            toggle_pick = Some(ColorPick::Zone(i));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            delete = widgets::small_button(ui, icon::TRASH, "Bereich löschen").clicked();
                        });
                    });
                    zone_strip(ui, z);
                    let d = ColorZone::default();
                    let rail = hue_rail();
                    let deg = widgets::SliderOpts { suffix: "°", ..Default::default() };
                    let picked = z.hue;
                    changed |= widgets::slider_row_with(ui, "Farbe", &mut z.hue, 0.0..=360.0, picked, 0, widgets::SliderOpts { suffix: "°", rail: Some(&rail), ..Default::default() });
                    changed |= widgets::slider_row_with(ui, "Breite", &mut z.width, 3.0..=90.0, d.width, 0, deg);
                    changed |= widgets::slider_row_with(ui, "Weichheit", &mut z.softness, 0.0..=100.0, d.softness, 0, widgets::SliderOpts { suffix: " %", ..Default::default() });
                    ui.label(RichText::new("Ändern").strong());
                    let hue = z.hue;
                    changed |= band_slider(ui, "Farbton", &mut z.adjust, ColorView::Hue, hue);
                    changed |= band_slider(ui, "Sättigung", &mut z.adjust, ColorView::Saturation, hue);
                    changed |= band_slider(ui, "Helligkeit", &mut z.adjust, ColorView::Lightness, hue);
                    if delete {
                        zones.remove(i);
                        *zone_sel = i.saturating_sub(1);
                        changed = true;
                    }
                }
            } else if *view == ColorView::Single {
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
            match t.layer_mut() {
                Some(l) => l.color_bands = Default::default(),
                None => adj.color_editor = Default::default(),
            }
            changed = true;
        }

        let mut reset_skin = false;
        widgets::tool_section(ui, sk_core::tools::Tool::SkinTone, Some(&mut || reset_skin = true), |ui| {
            let st = &mut adj.skin_tone;
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, skin_color(st));
                ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);
                let pipette = format!("{}  Hautton aufnehmen", crate::theme::icon::EYEDROPPER);
                if ui.selectable_label(picking_skin, pipette).on_hover_text("Auf eine typische Hautstelle im Bild klicken").clicked() {
                    toggle_pick = Some(ColorPick::Skin);
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
        widgets::tool_section(ui, sk_core::tools::Tool::ColorBalance, Some(&mut || reset_cb = true), |ui| {
            let cb = match t.layer_mut() {
                Some(l) => &mut l.color_balance,
                None => &mut adj.color_balance,
            };
            changed |= wheel_ui(ui, "Master", &mut cb.master, false);
            changed |= wheel_ui(ui, "Schatten", &mut cb.shadows, true);
            changed |= wheel_ui(ui, "Mitteltöne", &mut cb.midtones, true);
            changed |= wheel_ui(ui, "Lichter", &mut cb.highlights, true);
        });
        if reset_cb {
            match t.layer_mut() {
                Some(l) => l.color_balance = Default::default(),
                None => adj.color_balance = Default::default(),
            }
            changed = true;
        }
        t.store(adj);

        let mut reset_bw = false;
        widgets::tool_section(ui, sk_core::tools::Tool::BlackWhite, Some(&mut || reset_bw = true), |ui| {
            let bw = &mut adj.black_white;
            changed |= ui.checkbox(&mut bw.enabled, "Schwarzweiß aktivieren").changed();
            ui.add_enabled_ui(bw.enabled, |ui| {
                ui.label(RichText::new("Farbempfindlichkeit").strong());
                for (i, (name, _)) in HUE_BANDS.iter().enumerate() {
                    changed |= slider_row(ui, name, &mut bw.mix[i], -100.0..=100.0, 0.0, 0);
                }
                ui.label(RichText::new("Tonung").strong());
                changed |= wheel_ui(ui, "Schatten", &mut bw.shadow_tint, false);
                changed |= wheel_ui(ui, "Lichter", &mut bw.highlight_tint, false);
            });
        });
        if reset_bw {
            adj.black_white = Default::default();
            changed = true;
        }
        if changed {
            self.request_render();
        }
        if let Some(target) = toggle_pick {
            self.toggle_color_picker(target);
        }
    }

    /// "Automatisch anpassen" with its options and strength, for the exposure tab.
    pub fn auto_section(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let busy = self.auto_progress.is_some();
            let text = if busy { "Passe an …" } else { "Automatisch anpassen" };
            let hover = "Weißabgleich, Belichtung, Kontrast und Dynamik in einem Schritt einstellen (Strg+L), \
                         bei mehreren ausgewählten Bildern für alle.";
            if ui.add_enabled(!busy, egui::Button::new(format!("{}  {text}", icon::MAGIC_WAND))).on_hover_text(hover).clicked() {
                self.auto_adjust_selection();
            }
            let mut opts = self.auto_options;
            ui.menu_button(icon::CARET_DOWN, |ui| {
                ui.label(RichText::new("Automatisch anpassen setzt:").weak());
                ui.checkbox(&mut opts.white_balance, "Weißabgleich");
                ui.checkbox(&mut opts.exposure, "Belichtung");
                ui.checkbox(&mut opts.tone, "Lichter, Schatten, Weiß und Schwarz");
                ui.checkbox(&mut opts.contrast, "Kontrast");
                ui.checkbox(&mut opts.vibrance, "Dynamik");
            })
            .response
            .on_hover_text("Was automatisch angepasst wird");
            if opts != self.auto_options {
                self.auto_options = opts;
                self.save_settings();
            }
        });
        self.auto_strength(ui);
    }

    /// Strength of the last auto adjustment of the open image; disabled until there is one.
    fn auto_strength(&mut self, ui: &mut Ui) {
        let current = self.current_path();
        let mix = self.auto_mix.as_mut().filter(|m| Some(&m.path) == current.as_ref());
        let enabled = mix.is_some();
        let mut idle = 100.0;
        let mut changed = false;
        let strength = match mix {
            Some(m) => &mut m.strength,
            None => &mut idle,
        };
        ui.add_enabled_ui(enabled, |ui| {
            changed = widgets::slider_row_with(ui, "Stärke", strength, 0.0..=100.0, 100.0, 0, widgets::SliderOpts { suffix: " %", ..Default::default() });
        })
        .response
        .on_disabled_hover_text("Wie stark die letzte automatische Anpassung wirkt: erst nach „Automatisch anpassen“");
        let Some(mix) = self.auto_mix.as_ref().filter(|_| enabled) else { return };
        let Some(cur) = self.current.as_mut() else { return };
        if changed {
            cur.adjustments = mix.opts.blend(&cur.adjustments, &mix.before, &mix.auto, mix.strength / 100.0, cur.as_shot_temp_tint);
            self.request_render();
        }
    }

    /// Adjusts the selected images automatically (see `sk_core::auto::auto_adjust`), in the
    /// background; the results arrive as [`Self::auto_adjusted`].
    pub fn auto_adjust_selection(&mut self) {
        if self.auto_progress.is_some() {
            return;
        }
        if !self.auto_options.any() {
            self.status = "Automatisch anpassen: nichts ausgewählt, was angepasst werden soll".into();
            return;
        }
        let current = self.current_path();
        let items: Vec<crate::workers::AutoItem> = self
            .selected_paths()
            .into_iter()
            .map(|path| {
                let open = (Some(&path) == current.as_ref())
                    .then(|| self.current.as_ref().map(|c| (c.meta.clone(), c.preview.clone(), c.adjustments.clone())))
                    .flatten();
                crate::workers::AutoItem { path, open }
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.auto_progress = Some((0, items.len()));
        self.status = if items.len() == 1 { "Passe automatisch an …".into() } else { format!("Passe {} automatisch an …", widgets::image_count(items.len())) };
        self.workers.auto_adjust(crate::workers::AutoJob { items, opts: self.auto_options });
    }

    /// The auto adjustment of one image is done: its settings change (the open image's as one
    /// undo step). Only what the options cover changes, so edits made meanwhile stay.
    pub(crate) fn auto_adjusted(&mut self, path: std::path::PathBuf, opts: auto::AutoOptions, result: Result<Box<sk_core::params::Adjustments>, String>) {
        let failed = match result {
            Ok(adj) => {
                let before = self.current.as_ref().filter(|_| self.current_path().as_ref() == Some(&path)).map(|c| c.adjustments.clone());
                self.set_adjustments_on(std::slice::from_ref(&path), |a| opts.take(&adj, a));
                // The open image: its strength can still be changed.
                if let Some(before) = before
                    && let Some(cur) = &self.current
                {
                    self.auto_mix = Some(AutoMix { path, before, auto: cur.adjustments.clone(), opts, strength: 100.0 });
                }
                false
            }
            Err(e) => {
                log::warn!("Automatisch anpassen: {e}");
                self.status = format!("Automatisch anpassen fehlgeschlagen: {e}");
                true
            }
        };
        if let Some((done, total)) = self.auto_progress {
            let done = done + 1;
            self.auto_progress = (done < total).then_some((done, total));
            if done == total && !failed && !self.status.starts_with("Automatisch anpassen fehlgeschlagen") {
                self.status = if total == 1 { "Automatisch angepasst".into() } else { format!("{} automatisch angepasst", widgets::image_count(total)) };
            }
        }
    }

    /// White balance from the nearly neutral parts of the image (see `auto::auto_white_balance`).
    pub fn auto_white_balance(&mut self) {
        let Some(cur) = &self.current else { return };
        let wb = auto::auto_white_balance(&cur.meta, &cur.adjustments, &cur.preview);
        self.edit(|a| a.white_balance = wb.clone());
    }

    /// The lensfun profile for the current image's lens: name and coefficients at its focal length.
    fn lens_profile(&self) -> Option<(String, sk_core::params::LensCoefficients)> {
        let (db, cur) = (self.lens_db.as_ref()?, self.current.as_ref()?);
        if cur.meta.lens_corrected {
            return None;
        }
        let profile = sk_core::lensfun::find(db, cur.meta.info.lens.as_deref()?)?;
        let coeffs = profile.coefficients(cur.meta.info.focal_mm.unwrap_or(50.0), cur.meta.info.fnumber);
        Some((profile.name(), coeffs))
    }

    /// Name of the lensfun profile for the current lens, cached per lens name.
    fn lens_profile_name(&mut self) -> Option<String> {
        let meta = &self.current.as_ref()?.meta;
        let lens = meta.info.lens.clone().filter(|_| !meta.lens_corrected)?;
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
        // The camera's look (see `camera_look`) doesn't count as an edit.
        let plain = sk_core::params::Adjustments { base_curve: Default::default(), camera_curve: None, ..cur.committed.clone() };
        if cur.meta.lens.is_some() || cur.adjustments != cur.committed || plain.normalized() != sk_core::params::Adjustments::default() {
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
