//! The tools of the develop panel as units of the adjustments: what "Anpassungen kopieren"
//! copies when only some tools are chosen, what the
//! copy button of a tool carries over to the other selected images, and what a style holds.

use serde::{Deserialize, Serialize};

use crate::params::Adjustments;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tool {
    WhiteBalance,
    BaseCurve,
    /// Saturation and vibrance (the "Farbe" section).
    Color,
    ColorEditor,
    SkinTone,
    ColorBalance,
    BlackWhite,
    /// Exposure, contrast and brightness (the "Belichtung" section).
    Exposure,
    HighlightsShadows,
    Levels,
    Curves,
    Sharpening,
    NoiseReduction,
    Clarity,
    Dehaze,
    Vignette,
    Grain,
    LensBlur,
    Glow,
    Relight,
    Atmosphere,
    LensCorrection,
    Crop,
    Rotation,
    Perspective,
    Layers,
    Retouch,
}

/// The tools in groups as in the tool tabs.
pub const GROUPS: &[(&str, &[Tool])] = &[
    ("Farbe", &[Tool::BaseCurve, Tool::WhiteBalance, Tool::Color, Tool::ColorEditor, Tool::SkinTone, Tool::ColorBalance, Tool::BlackWhite]),
    ("Belichtung", &[Tool::Exposure, Tool::HighlightsShadows, Tool::Relight, Tool::Levels, Tool::Curves]),
    ("Details", &[Tool::NoiseReduction, Tool::Sharpening, Tool::Clarity, Tool::Dehaze]),
    ("Effekte", &[Tool::LensBlur, Tool::Atmosphere, Tool::Glow, Tool::Vignette, Tool::Grain]),
    ("Objektiv", &[Tool::LensCorrection, Tool::Crop, Tool::Rotation, Tool::Perspective]),
    ("Ebenen und Retusche", &[Tool::Layers, Tool::Retouch]),
];

/// Whether a tool's settings can also be made on a local layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerSupport {
    /// Every setting of the tool.
    Full,
    /// Some settings; the others only on the background.
    Partial,
    /// Only on the background.
    None,
}

impl Tool {
    pub const ALL: [Tool; 27] = [
        Tool::WhiteBalance,
        Tool::BaseCurve,
        Tool::Color,
        Tool::ColorEditor,
        Tool::SkinTone,
        Tool::ColorBalance,
        Tool::BlackWhite,
        Tool::Exposure,
        Tool::HighlightsShadows,
        Tool::Levels,
        Tool::Curves,
        Tool::Sharpening,
        Tool::NoiseReduction,
        Tool::Clarity,
        Tool::Dehaze,
        Tool::Vignette,
        Tool::Grain,
        Tool::LensBlur,
        Tool::Glow,
        Tool::Relight,
        Tool::Atmosphere,
        Tool::LensCorrection,
        Tool::Crop,
        Tool::Rotation,
        Tool::Perspective,
        Tool::Layers,
        Tool::Retouch,
    ];

    /// Works with an AI model (the depth map): marked as such in the tool tabs.
    pub fn is_ai(self) -> bool {
        matches!(self, Tool::LensBlur | Tool::Relight | Tool::Atmosphere)
    }

    /// The title of the tool's section.
    pub fn label(self) -> &'static str {
        match self {
            Tool::WhiteBalance => "Weißabgleich",
            Tool::BaseCurve => "Basismerkmale",
            Tool::Color => "Farbe",
            Tool::ColorEditor => "Farbeditor",
            Tool::SkinTone => "Hauttöne",
            Tool::ColorBalance => "Farbbalance",
            Tool::BlackWhite => "Schwarzweiß",
            Tool::Exposure => "Belichtung",
            Tool::HighlightsShadows => "Lichter & Tiefen",
            Tool::Levels => "Tonwerte",
            Tool::Curves => "Gradationskurve",
            Tool::Sharpening => "Schärfen",
            Tool::NoiseReduction => "Rauschreduzierung",
            Tool::Clarity => "Klarheit",
            Tool::Dehaze => "Dunst entfernen",
            Tool::Vignette => "Vignette",
            Tool::Grain => "Körnung",
            Tool::LensBlur => "Objektivunschärfe",
            Tool::Glow => "Leuchten",
            Tool::Relight => "Neu ausleuchten",
            Tool::Atmosphere => "Atmosphäre",
            Tool::LensCorrection => "Objektivkorrektur",
            Tool::Crop => "Zuschnitt",
            Tool::Rotation => "Drehen & Spiegeln",
            Tool::Perspective => "Trapezkorrektur",
            Tool::Layers => "Ebenen",
            Tool::Retouch => "Retusche",
        }
    }

    /// How far the tool also works on a local layer (`LayerAdjustments`).
    pub fn layer_support(self) -> LayerSupport {
        match self {
            Tool::Color | Tool::ColorBalance | Tool::Exposure | Tool::HighlightsShadows | Tool::Curves | Tool::Clarity | Tool::Dehaze => LayerSupport::Full,
            // Temperature and tint relative to the background; the color editor's bands, not its
            // ranges of one's own.
            Tool::WhiteBalance | Tool::ColorEditor => LayerSupport::Partial,
            _ => LayerSupport::None,
        }
    }

    /// Settings that belong to the content of one image (framing, masks, retouching): left out
    /// unless chosen explicitly.
    pub fn image_specific(self) -> bool {
        matches!(self, Tool::Crop | Tool::Rotation | Tool::Perspective | Tool::Layers | Tool::Retouch)
    }

    /// Copies this tool's settings from `from` into `to`.
    pub fn copy(self, from: &Adjustments, to: &mut Adjustments) {
        match self {
            Tool::WhiteBalance => to.white_balance = from.white_balance.clone(),
            Tool::BaseCurve => to.base_curve = from.base_curve,
            Tool::Color => (to.saturation, to.vibrance) = (from.saturation, from.vibrance),
            Tool::ColorEditor => to.color_editor = from.color_editor.clone(),
            Tool::SkinTone => to.skin_tone = from.skin_tone,
            Tool::ColorBalance => to.color_balance = from.color_balance,
            Tool::BlackWhite => to.black_white = from.black_white.clone(),
            Tool::Exposure => {
                (to.exposure, to.contrast, to.brightness) = (from.exposure, from.contrast, from.brightness);
            }
            Tool::HighlightsShadows => {
                (to.highlights, to.shadows, to.whites, to.blacks) = (from.highlights, from.shadows, from.whites, from.blacks);
            }
            Tool::Levels => to.levels = from.levels.clone(),
            Tool::Curves => to.curves = from.curves.clone(),
            Tool::Sharpening => {
                let (f, t) = (&from.detail, &mut to.detail);
                (t.sharpen_amount, t.sharpen_radius, t.sharpen_threshold) = (f.sharpen_amount, f.sharpen_radius, f.sharpen_threshold);
                t.ai_deblur = f.ai_deblur;
            }
            Tool::NoiseReduction => {
                let (f, t) = (&from.detail, &mut to.detail);
                (t.ai_denoise, t.nr_luma, t.nr_color) = (f.ai_denoise, f.nr_luma, f.nr_color);
            }
            Tool::Clarity => (to.detail.clarity, to.detail.structure) = (from.detail.clarity, from.detail.structure),
            Tool::Dehaze => to.detail.dehaze = from.detail.dehaze,
            Tool::Vignette => to.detail.vignette_amount = from.detail.vignette_amount,
            Tool::Grain => (to.detail.grain_amount, to.detail.grain_size) = (from.detail.grain_amount, from.detail.grain_size),
            Tool::LensBlur => to.lens_blur = from.lens_blur,
            Tool::Glow => (to.detail.glow_amount, to.detail.glow_size) = (from.detail.glow_amount, from.detail.glow_size),
            Tool::Relight => to.relight = from.relight,
            Tool::Atmosphere => to.atmosphere = from.atmosphere,
            Tool::LensCorrection => to.lens = from.lens.clone(),
            Tool::Crop => {
                let (f, t) = (&from.geometry, &mut to.geometry);
                (t.crop, t.aspect, t.aspect_portrait) = (f.crop, f.aspect, f.aspect_portrait);
            }
            Tool::Rotation => {
                to.rotation = from.rotation;
                let (f, t) = (&from.geometry, &mut to.geometry);
                (t.straighten, t.flip_h, t.flip_v) = (f.straighten, f.flip_h, f.flip_v);
            }
            Tool::Perspective => {
                let (t, f) = (&mut to.geometry, &from.geometry);
                (t.keystone_v, t.keystone_h, t.keystone_amount) = (f.keystone_v, f.keystone_h, f.keystone_amount);
            }
            Tool::Layers => to.layers = from.layers.clone(),
            Tool::Retouch => (to.retouch, to.removals) = (from.retouch.clone(), from.removals.clone()),
        }
    }

    /// The tool is as on a new image.
    pub fn is_default(self, a: &Adjustments) -> bool {
        let mut d = Adjustments::default();
        self.copy(a, &mut d);
        d == Adjustments::default()
    }
}

/// The tools changed in `a`, without the image-specific ones (what copying takes by default).
pub fn edited(a: &Adjustments) -> Vec<Tool> {
    Tool::ALL.into_iter().filter(|t| !t.image_specific() && !t.is_default(a)).collect()
}

/// `target` with the settings of `tools` taken from `from`; the process version stays the
/// target's.
pub fn apply(tools: &[Tool], from: &Adjustments, target: &Adjustments) -> Adjustments {
    let mut out = target.clone();
    for t in tools {
        t.copy(from, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{CropRect, Layer};

    #[test]
    fn groups_hold_every_tool_once() {
        let mut grouped: Vec<Tool> = GROUPS.iter().flat_map(|(_, t)| t.iter().copied()).collect();
        grouped.sort();
        let mut all = Tool::ALL.to_vec();
        all.sort();
        assert_eq!(grouped, all);
    }

    #[test]
    fn copying_every_tool_copies_everything_but_the_process() {
        let mut from = Adjustments { process: 1, exposure: 1.0, contrast: 20.0, vibrance: 5.0, rotation: 1, ..Default::default() };
        from.detail.grain_amount = 30.0;
        from.detail.nr_color = 0.0;
        from.geometry.crop = CropRect { x: 0.1, y: 0.1, w: 0.5, h: 0.5 };
        from.layers.push(Layer::default());
        from.lens.camera = false;
        let out = apply(&Tool::ALL, &from, &Adjustments::default());
        assert_eq!(out, Adjustments { process: Adjustments::default().process, ..from });
    }

    #[test]
    fn edited_tools_leave_out_the_image_specific_ones() {
        let mut a = Adjustments { exposure: 0.5, ..Default::default() };
        a.detail.grain_amount = 10.0;
        a.geometry.crop = CropRect { x: 0.1, y: 0.1, w: 0.5, h: 0.5 };
        assert_eq!(edited(&a), vec![Tool::Exposure, Tool::Grain]);
        assert!(!Tool::Crop.is_default(&a));
    }

    #[test]
    fn applying_some_tools_keeps_the_others() {
        let from = Adjustments { exposure: 1.0, contrast: 30.0, highlights: 50.0, ..Default::default() };
        let target = Adjustments { exposure: -1.0, highlights: -20.0, rotation: 2, ..Default::default() };
        let out = apply(&[Tool::HighlightsShadows], &from, &target);
        assert_eq!((out.exposure, out.contrast, out.highlights, out.rotation), (-1.0, 0.0, 50.0, 2));
    }

    #[test]
    fn hidden_tools_take_no_effect_but_keep_their_settings() {
        let mut a = Adjustments { exposure: 1.0, contrast: 20.0, highlights: -40.0, ..Default::default() };
        a.geometry.crop = CropRect { x: 0.1, y: 0.1, w: 0.5, h: 0.5 };
        assert!(matches!(a.visible(), std::borrow::Cow::Borrowed(_)));
        a.toggle_hidden(Tool::Exposure);
        a.toggle_hidden(Tool::Crop);
        let v = a.visible();
        assert_eq!((v.exposure, v.contrast, v.highlights), (0.0, 0.0, -40.0));
        assert_eq!(v.geometry.crop, CropRect::default());
        assert!(v.hidden.is_empty());
        assert_eq!((a.exposure, a.geometry.crop.w), (1.0, 0.5));
        // Switched on again: as before.
        a.toggle_hidden(Tool::Exposure);
        assert_eq!(a.hidden, vec![Tool::Crop]);
        assert_eq!(a.visible().exposure, 1.0);
    }

    #[test]
    fn hidden_tools_are_saved_only_when_there_are_some() {
        let mut a = Adjustments::default();
        assert!(!serde_json::to_string(&a).unwrap().contains("hidden"));
        a.toggle_hidden(Tool::Vignette);
        let back: Adjustments = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back.hidden, vec![Tool::Vignette]);
    }
}
