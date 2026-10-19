//! Automatic settings: the one-click auto adjustment and the "A" in the headers of the tools
//! that can set themselves (white balance, exposure, levels, straighten, …). Each works on all
//! selected images, in the background (see `workers::AutoJob`).

use std::sync::Arc;

use sk_core::auto::AutoOptions;
use sk_core::lensfun::LensProfile;
use sk_core::params::{Adjustments, LensCoefficients};
use sk_core::raw::RawMeta;
use sk_core::tools::Tool;
use sk_core::upright::UprightMode;

use crate::app::App;
use crate::widgets;

/// What an automatic run sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AutoKind {
    /// Tone and color settings (see `sk_core::auto::auto_adjust`).
    Adjust(AutoOptions),
    /// Straighten angle (and keystone) from the straight edges of the image.
    Upright(UprightMode),
    /// The lensfun profile of the lens.
    LensProfile,
}

/// The tools with an "A" in their header: what it sets, its command and what it does.
const TOOLS: &[(Tool, &str, &str)] = &[
    (Tool::WhiteBalance, "auto-wb", "Weißabgleich aus den fast neutralen Stellen des Bildes (Steine, Wolken, weiße Wände), ausgehend vom Kamerawert"),
    (Tool::Exposure, "auto-exposure", "Belichtung und Kontrast automatisch einstellen"),
    (Tool::HighlightsShadows, "auto-tone", "Lichter, Tiefen, Weiß und Schwarz automatisch einstellen"),
    (Tool::Color, "auto-vibrance", "Dynamik automatisch einstellen: mehr für blasse Bilder, keine für bunte"),
    (Tool::Levels, "auto-levels", "Schwarz- und Weißpunkt an den Tonwertumfang des Bildes legen"),
    (Tool::Rotation, "auto-straighten", "Horizont und Kanten automatisch waagerecht, ohne Perspektivkorrektur"),
    (Tool::Perspective, "auto-upright", "Stürzende Linien automatisch korrigieren, die Drehung wird mit ausgerichtet"),
    (Tool::LensCorrection, "auto-lens", "Das passende Objektivprofil aus der Lensfun-Datenbank anwenden"),
];

impl AutoKind {
    /// What the "A" of `tool` sets, if it has one.
    pub fn of_tool(tool: Tool) -> Option<AutoKind> {
        let only = |f: fn(&mut AutoOptions)| {
            let mut o = AutoOptions::NONE;
            f(&mut o);
            Some(AutoKind::Adjust(o))
        };
        match tool {
            Tool::WhiteBalance => only(|o| o.white_balance = true),
            Tool::Exposure => only(|o| (o.exposure, o.contrast) = (true, true)),
            Tool::HighlightsShadows => only(|o| o.tone = true),
            Tool::Color => only(|o| o.vibrance = true),
            Tool::Levels => only(|o| o.levels = true),
            Tool::Rotation => Some(AutoKind::Upright(UprightMode::Level)),
            Tool::Perspective => Some(AutoKind::Upright(UprightMode::Vertical)),
            Tool::LensCorrection => Some(AutoKind::LensProfile),
            _ => None,
        }
    }

    /// `onto` with what this run sets taken from `auto`.
    pub fn take(&self, auto: &Adjustments, onto: &Adjustments) -> Adjustments {
        match self {
            AutoKind::Adjust(opts) => opts.take(auto, onto),
            AutoKind::Upright(_) => Adjustments { geometry: auto.geometry.clone(), ..onto.clone() },
            AutoKind::LensProfile => {
                let mut out = onto.clone();
                out.lens.profile = auto.lens.profile.clone();
                out.lens.profile_coeffs = auto.lens.profile_coeffs;
                // Never both: the profile would correct the same lens a second time.
                out.lens.camera = false;
                out
            }
        }
    }

    /// The image's adjustments with what this run sets, or None when there is nothing to set
    /// (no straight edges, no profile for the lens). Runs in the auto worker.
    pub fn compute(
        &self,
        meta: &RawMeta,
        preview: &sk_core::image::LinearImage,
        adj: Adjustments,
        models: &mut sk_ai::auto::AutoModels,
        lens_db: Option<&[LensProfile]>,
    ) -> Option<Adjustments> {
        match self {
            AutoKind::Adjust(opts) => {
                // On the camera's look, if the image is to get it (see `camera_look`).
                let adj = sk_core::camera_look::apply_if_wanted(meta, preview, &adj, sk_core::camera_look::for_new()).unwrap_or(adj);
                let guide = sk_core::auto::guide_image(meta, &adj, preview, opts);
                let focus = models.focus(&guide);
                let camera = sk_core::auto::camera_brightness(meta);
                Some(sk_core::auto::auto_adjust(meta, &adj, preview, opts, &focus, camera))
            }
            AutoKind::Upright(mode) => {
                let segments = sk_core::upright::detect_segments(preview);
                let src = (preview.width, preview.height);
                let geometry = sk_core::upright::solve(&segments, &adj.geometry, &adj.lens, meta.lens.as_ref(), adj.rotation, src, *mode)?;
                Some(Adjustments { geometry, ..adj })
            }
            AutoKind::LensProfile => {
                let (name, coeffs) = lens_profile(lens_db?, meta)?;
                let mut out = adj;
                out.lens.profile = Some(name);
                out.lens.profile_coeffs = Some(coeffs);
                Some(out)
            }
        }
    }

    /// Name for messages, as the command is called.
    fn name(&self) -> &'static str {
        match self {
            AutoKind::Adjust(_) => "Automatisch anpassen",
            AutoKind::Upright(UprightMode::Level) => "Geraderichten automatisch",
            AutoKind::Upright(_) => "Perspektive automatisch",
            AutoKind::LensProfile => "Objektivprofil automatisch",
        }
    }

    /// Why nothing was set on an image.
    fn nothing_found(&self) -> &'static str {
        match self {
            AutoKind::LensProfile => "kein Profil für dieses Objektiv",
            _ => "keine geraden Kanten gefunden",
        }
    }
}

/// The lensfun profile for the lens of an image: name and coefficients at its focal length.
/// None if the raw file corrects the lens itself.
pub fn lens_profile(db: &[LensProfile], meta: &RawMeta) -> Option<(String, LensCoefficients)> {
    if meta.lens_corrected {
        return None;
    }
    let profile = sk_core::lensfun::find(db, meta.info.lens.as_deref()?)?;
    Some((profile.name(), profile.coefficients(meta.info.focal_mm.unwrap_or(50.0), meta.info.fnumber)))
}

/// Hover text of the "A" in the header of `tool` and its command, if it has one.
pub fn tool_hint(tool: Tool) -> Option<String> {
    let (_, id, what) = TOOLS.iter().find(|t| t.0 == tool)?;
    Some(format!("{}, bei mehreren ausgewählten Bildern für alle.", crate::commands::hint(what, id)))
}

impl App {
    /// The "A" of `tool`: sets it automatically on all selected images.
    pub fn auto_tool(&mut self, tool: Tool) {
        if let Some(kind) = AutoKind::of_tool(tool) {
            self.run_auto(kind);
        }
    }

    /// "Automatisch anpassen": what its options cover, on all selected images.
    pub fn auto_adjust_selection(&mut self) {
        if !self.auto_options.any() {
            self.status = "Automatisch anpassen: nichts ausgewählt, was angepasst werden soll".into();
            return;
        }
        self.run_auto(AutoKind::Adjust(self.auto_options));
    }

    /// Whether an automatic run can start now (see `run_auto`).
    pub fn can_auto(&self, kind: AutoKind) -> bool {
        self.auto_progress.is_none() && !self.on_layer() && (kind != AutoKind::LensProfile || self.lens_db.is_some())
    }

    /// Sets the selected images automatically, in the background; the results arrive as
    /// [`Self::auto_adjusted`].
    pub fn run_auto(&mut self, kind: AutoKind) {
        if self.auto_progress.is_some() {
            return;
        }
        if kind == AutoKind::LensProfile && self.lens_db.is_none() {
            self.status = "Objektivprofil automatisch: die Objektivprofile sind noch nicht geladen".into();
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
        (self.auto_missed, self.auto_failed) = (0, false);
        self.batch_step = None;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.auto_cancel = Some(cancel.clone());
        self.status = if items.len() == 1 { "Passe automatisch an …".into() } else { format!("Passe {} automatisch an …", widgets::image_count(items.len())) };
        self.workers.auto_adjust(crate::workers::AutoJob { items, kind, lens_db: self.lens_db.clone(), cancel });
    }

    /// The automatic run on one image is done: its settings change (the open image's as one
    /// undo step). Only what the run covers changes, so edits made meanwhile stay.
    pub(crate) fn auto_adjusted(&mut self, path: std::path::PathBuf, kind: AutoKind, result: Result<Option<Box<Adjustments>>, String>) {
        let mut single = None;
        match result {
            Ok(Some(adj)) => {
                let before = self.current.as_ref().filter(|_| self.current_path().as_ref() == Some(&path)).map(|c| c.adjustments.clone());
                self.set_adjustments_on(std::slice::from_ref(&path), |a| kind.take(&adj, a));
                // The open image: the strength of the auto adjustment can still be changed.
                if let Some(before) = before
                    && let Some(cur) = &self.current
                {
                    single = Some(cur.adjustments.clone());
                    if let AutoKind::Adjust(opts) = kind {
                        self.auto_mix = Some(crate::develop_tools::AutoMix { path, before, auto: cur.adjustments.clone(), opts, strength: 100.0 });
                    }
                }
            }
            Ok(None) => self.auto_missed += 1,
            Err(e) => {
                log::warn!("{}: {e}", kind.name());
                self.report_error(format!("{} fehlgeschlagen: {e}", kind.name()));
                self.auto_failed = true;
            }
        }
        let Some((done, total)) = self.auto_progress else { return };
        let done = done + 1;
        self.auto_progress = (done < total).then_some((done, total));
        if done < total || self.auto_failed {
            return;
        }
        let changed = total - self.auto_missed;
        self.status = match (total, self.auto_missed) {
            (1, 1) => format!("{}: {}", kind.name(), kind.nothing_found()),
            (1, _) => match (kind, single) {
                (AutoKind::Upright(_), Some(a)) => {
                    let g = &a.geometry;
                    let keystone = if g.keystone_v != 0.0 || g.keystone_h != 0.0 { format!(", Trapezkorrektur {:.0} / {:.0}", g.keystone_v, g.keystone_h) } else { String::new() };
                    format!("Perspektive automatisch korrigiert: {:.1}°{keystone}", g.straighten)
                }
                (AutoKind::LensProfile, Some(a)) => format!("Objektivprofil: {}", a.lens.profile.unwrap_or_default()),
                _ => "Automatisch angepasst".into(),
            },
            (_, 0) => format!("{} automatisch angepasst", widgets::image_count(total)),
            (_, missed) => format!("{} automatisch angepasst, bei {missed}: {}", widgets::image_count(changed), kind.nothing_found()),
        };
    }
}
