//! What the one-click auto adjustment (`sk_core::auto::auto_adjust`) should pay most attention
//! to: faces and the main subject, from the models that are already downloaded (auto never
//! downloads one).

use sk_core::auto::Focus;
use sk_core::look_match::{Mask8, RegionMasks};
use sk_core::detail::Plane;
use sk_core::pipeline::Rendered;

use crate::faces::{FaceDetector, Image};

/// The models, loaded on first use and kept for further images.
#[derive(Default)]
pub struct AutoModels {
    faces: Option<Option<FaceDetector>>,
    subject: Option<Option<crate::SubjectModel>>,
}

impl AutoModels {
    /// Faces and subject of `guide` (see `sk_core::auto::guide_image`).
    pub fn focus(&mut self, guide: &Rendered) -> Focus {
        let rgb: Vec<[f32; 3]> = guide.rgba.as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]].map(|v| v as f32 / 255.0)).collect();
        let faces = self
            .faces
            .get_or_insert_with(|| FaceDetector::available().then(|| FaceDetector::load().map_err(|e| log::warn!("Gesichter: {e:#}")).ok()).flatten())
            .as_ref()
            .and_then(|d| {
                let img = Image { width: guide.width, height: guide.height, rgb: &rgb };
                // Faces smaller than about 2 % of the long edge don't matter for the exposure.
                d.faces(&img, guide.width.max(guide.height) as f32 * 0.02).map_err(|e| log::warn!("Gesichter: {e:#}")).ok()
            })
            .unwrap_or_default();
        let subject = self
            .subject_model()
            .and_then(|m| {
                let g = crate::Guide { width: guide.width, height: guide.height, rgb };
                m.subject_mask(&g).map_err(|e| log::warn!("Motiv: {e:#}")).ok()
            })
            .map(|m| Plane::new(m.width, m.height, m.data.iter().map(|v| *v as f32 / 255.0).collect()))
            // A "subject" covering nearly everything or nothing says nothing.
            .filter(|p| {
                let mean = p.data.iter().sum::<f32>() / p.data.len().max(1) as f32;
                (0.03..0.7).contains(&mean)
            });
        Focus { faces, subject }
    }

    /// The subject model, if downloaded (loaded on first use).
    fn subject_model(&mut self) -> Option<&crate::SubjectModel> {
        self.subject
            .get_or_insert_with(|| {
                crate::model_available().then(|| crate::SubjectModel::load(&crate::default_model_path()).map_err(|e| log::warn!("Motiv: {e:#}")).ok()).flatten()
            })
            .as_ref()
    }

    /// Sky and main subject of a picture (8-bit sRGB RGBA), for "Look angleichen"
    /// (see `sk_core::look_match`): the sky by its heuristic, the subject only with the model.
    pub fn look_regions(&mut self, width: usize, height: usize, rgba: &[u8]) -> RegionMasks {
        let guide = crate::guide_from_rgba(width, height, rgba);
        let mask = |m: crate::MaskImage| Mask8 { width: m.width, height: m.height, data: m.data };
        let sky = Some(mask(crate::sky_mask(&guide)));
        let subject = self.subject_model().and_then(|m| m.subject_mask(&guide).map_err(|e| log::warn!("Motiv: {e:#}")).ok()).map(mask);
        RegionMasks { sky, subject }
    }
}
