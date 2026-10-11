//! Soft proofing: the display shows the image as it would come out in an output color space or
//! on a printer (ICC profile, e.g. printer and paper), optionally with out-of-gamut colors
//! marked. Built on Little CMS.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use lcms2::{CIExyY, CIExyYTRIPLE, DisallowCache, Flags, Intent, PixelFormat, Profile, ThreadContext, ToneCurve, Transform};
use serde::{Deserialize, Serialize};

use crate::color::OutputSpace;

/// What to proof against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProofTarget {
    Space(OutputSpace),
    /// An ICC profile file (printer/paper, other output devices).
    Icc(PathBuf),
}

impl Default for ProofTarget {
    fn default() -> Self {
        ProofTarget::Space(OutputSpace::Srgb)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RenderIntent {
    #[default]
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl RenderIntent {
    pub const ALL: [RenderIntent; 4] =
        [RenderIntent::Perceptual, RenderIntent::RelativeColorimetric, RenderIntent::Saturation, RenderIntent::AbsoluteColorimetric];

    pub fn label(self) -> &'static str {
        match self {
            RenderIntent::Perceptual => "Wahrnehmungsorientiert",
            RenderIntent::RelativeColorimetric => "Relativ farbmetrisch",
            RenderIntent::Saturation => "Sättigung",
            RenderIntent::AbsoluteColorimetric => "Absolut farbmetrisch",
        }
    }

    fn lcms(self) -> Intent {
        match self {
            RenderIntent::Perceptual => Intent::Perceptual,
            RenderIntent::RelativeColorimetric => Intent::RelativeColorimetric,
            RenderIntent::Saturation => Intent::Saturation,
            RenderIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
        }
    }
}

/// Soft proof settings (kept between sessions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProofSettings {
    pub target: ProofTarget,
    pub intent: RenderIntent,
    pub black_point_compensation: bool,
    /// Paper white and ink black as they are (the display shows the paper's color).
    pub simulate_paper: bool,
    pub gamut_warning: bool,
    /// ICC profiles chosen by the user (besides those found in the system folders).
    pub profiles: Vec<PathBuf>,
}

impl Default for ProofSettings {
    fn default() -> Self {
        Self {
            target: ProofTarget::default(),
            intent: RenderIntent::Perceptual,
            black_point_compensation: true,
            simulate_paper: false,
            gamut_warning: true,
            profiles: Vec::new(),
        }
    }
}

/// Color that marks out-of-gamut pixels on the display.
pub const GAMUT_ALARM: [u8; 3] = [255, 0, 255];

/// A ready soft proof transform: encoded working RGB (Rec.2020 primaries, sRGB transfer,
/// 16 bit) -> proofing device -> sRGB display (8 bit).
pub struct SoftProof {
    // Dropped before the context it was created in.
    transform: Transform<[u16; 3], [u8; 3], ThreadContext, DisallowCache>,
    _context: KeepAlive,
    /// Name of the target for display.
    pub name: String,
}

/// Keeps the Little CMS context alive for the transform created in it. It is never used after
/// creating the transform, so sharing it between threads is fine (the transform itself is
/// `Sync` without a cache).
struct KeepAlive(#[allow(dead_code)] ThreadContext);

unsafe impl Sync for KeepAlive {}

/// sRGB transfer function as an ICC parametric curve (type 4).
fn srgb_curve(ctx: &ThreadContext) -> Result<ToneCurve> {
    let _ = ctx;
    ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]).map_err(|e| anyhow!("{e}"))
}

fn xyy(x: f64, y: f64) -> CIExyY {
    CIExyY { x, y, Y: 1.0 }
}

/// RGB profile with the given primaries (D65) and transfer curve.
fn rgb_profile(ctx: &ThreadContext, primaries: [[f64; 2]; 3], curve: &ToneCurve) -> Result<Profile<ThreadContext>> {
    let p = CIExyYTRIPLE { Red: xyy(primaries[0][0], primaries[0][1]), Green: xyy(primaries[1][0], primaries[1][1]), Blue: xyy(primaries[2][0], primaries[2][1]) };
    Profile::new_rgb_context(ctx, &xyy(0.3127, 0.3290), &p, &[curve, curve, curve]).map_err(|e| anyhow!("{e}"))
}

/// The working space as stored in the encoded rendering: Rec.2020 primaries, sRGB curve.
fn working_profile(ctx: &ThreadContext) -> Result<Profile<ThreadContext>> {
    rgb_profile(ctx, [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]], &srgb_curve(ctx)?)
}

fn space_profile(ctx: &ThreadContext, space: OutputSpace) -> Result<Profile<ThreadContext>> {
    // Built from the same data that goes into the exported files.
    Profile::new_icc_context(ctx, &crate::icc::profile_for(space)).map_err(|e| anyhow!("{e}"))
}

/// Profile description (or file name).
pub fn profile_name(path: &Path) -> String {
    Profile::new_file(path)
        .ok()
        .and_then(|p| p.info(lcms2::InfoType::Description, lcms2::Locale::new("de_DE")))
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string())
}

pub fn target_name(t: &ProofTarget) -> String {
    match t {
        ProofTarget::Space(s) => s.label().to_string(),
        ProofTarget::Icc(p) => profile_name(p),
    }
}

impl SoftProof {
    pub fn new(settings: &ProofSettings) -> Result<Self> {
        let mut ctx = ThreadContext::new();
        let mut alarm = [0u16; 16];
        for (a, v) in alarm.iter_mut().zip(GAMUT_ALARM) {
            *a = v as u16 * 257;
        }
        ctx.set_alarm_codes(alarm);
        let input = working_profile(&ctx)?;
        let display = Profile::new_srgb_context(&ctx);
        let proofing = match &settings.target {
            ProofTarget::Space(s) => space_profile(&ctx, *s)?,
            ProofTarget::Icc(p) => {
                let data = std::fs::read(p).with_context(|| format!("{} nicht lesbar", p.display()))?;
                Profile::new_icc_context(&ctx, &data).map_err(|e| anyhow!("{}: kein gültiges ICC-Profil ({e})", p.display()))?
            }
        };
        let mut flags = Flags::NO_CACHE | Flags::SOFT_PROOFING;
        if settings.gamut_warning {
            flags = flags | Flags::GAMUT_CHECK;
        }
        if settings.black_point_compensation {
            flags = flags | Flags::BLACKPOINT_COMPENSATION;
        }
        // Without paper simulation the display is adapted to the paper white (relative).
        let display_intent = if settings.simulate_paper { Intent::AbsoluteColorimetric } else { Intent::RelativeColorimetric };
        let transform = Transform::new_proofing_context(
            &ctx,
            &input,
            PixelFormat::RGB_16,
            &display,
            PixelFormat::RGB_8,
            &proofing,
            settings.intent.lcms(),
            display_intent,
            flags,
        )
        .map_err(|e| anyhow!("Softproof nicht möglich: {e}"))?;
        Ok(Self { transform, _context: KeepAlive(ctx), name: target_name(&settings.target) })
    }

    /// Converts encoded working RGB (0..1) into display sRGB.
    pub fn apply(&self, src: &[[f32; 3]], dst: &mut [[u8; 3]]) {
        let input: Vec<[u16; 3]> = src.iter().map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)).collect();
        self.transform.transform_pixels(&input, dst);
    }
}

/// ICC profiles in the usual system and user folders: (file, description), sorted by description.
pub fn find_profiles() -> Vec<(PathBuf, String)> {
    let mut dirs: Vec<PathBuf> = vec![crate::paths::config_dir().join("profiles")];
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            dirs.push(PathBuf::from(root).join("System32").join("spool").join("drivers").join("color"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/share/color/icc"));
        dirs.push(PathBuf::from("/usr/local/share/color/icc"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/share/icc"));
            dirs.push(PathBuf::from(&home).join(".color/icc"));
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("icc") || e.eq_ignore_ascii_case("icm")) {
                    // Only output profiles that a proof makes sense for.
                    if let Ok(prof) = Profile::new_file(&p)
                        && matches!(prof.device_class(), lcms2::ProfileClassSignature::OutputClass | lcms2::ProfileClassSignature::DisplayClass | lcms2::ProfileClassSignature::ColorSpaceClass)
                    {
                        out.push((p.clone(), profile_name(&p)));
                    }
                }
            }
        }
    }
    out.sort_by_key(|(_, n)| n.to_lowercase());
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_proof_keeps_srgb_colors_and_flags_wide_ones() {
        let sp = SoftProof::new(&ProofSettings { target: ProofTarget::Space(OutputSpace::Srgb), intent: RenderIntent::RelativeColorimetric, ..Default::default() }).unwrap();
        // Rec.2020 encoded values of an sRGB mid grey and of saturated Rec.2020 green.
        let grey = crate::color::srgb_encode(0.18);
        let mut out = [[0u8; 3]; 2];
        sp.apply(&[[grey; 3], [0.0, 1.0, 0.0]], &mut out);
        let expect = (grey * 255.0).round() as i32;
        assert!(out[0].iter().all(|&v| (v as i32 - expect).abs() <= 2), "{:?}", out[0]);
        assert_eq!(out[1], GAMUT_ALARM);
        // Without the warning the green is mapped into sRGB.
        let sp = SoftProof::new(&ProofSettings { gamut_warning: false, ..Default::default() }).unwrap();
        sp.apply(&[[grey; 3], [0.0, 1.0, 0.0]], &mut out);
        assert!(out[1][1] > 200 && out[1][0] < 100, "{:?}", out[1]);
        // Wide spaces contain it.
        let sp = SoftProof::new(&ProofSettings { target: ProofTarget::Space(OutputSpace::ProPhoto), ..Default::default() }).unwrap();
        sp.apply(&[[grey; 3], [0.0, 1.0, 0.0]], &mut out);
        assert_ne!(out[1], GAMUT_ALARM);
    }

    #[test]
    fn icc_file_target() {
        let dir = std::env::temp_dir().join(format!("sk-proof-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("adobe.icc");
        std::fs::write(&p, crate::icc::profile_for(OutputSpace::AdobeRgb)).unwrap();
        let sp = SoftProof::new(&ProofSettings { target: ProofTarget::Icc(p.clone()), ..Default::default() }).unwrap();
        assert!(sp.name.contains("Adobe"), "{}", sp.name);
        assert!(SoftProof::new(&ProofSettings { target: ProofTarget::Icc(dir.join("fehlt.icc")), ..Default::default() }).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
