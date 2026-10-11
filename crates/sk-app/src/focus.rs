//! Focus mask: the sharp areas of the shown images, marked in green (see `sk_core::focus`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, ColorImage, TextureHandle, TextureId, TextureOptions};

use crate::app::App;

/// Color of the marked areas.
const MARK: Color32 = Color32::from_rgba_unmultiplied_const(40, 235, 70, 200);

/// Sharpness map of one image and its overlay for the current sensitivity.
pub struct FocusMap {
    /// Edit version of the rendering it was computed from, for the image being edited.
    pub version: Option<u64>,
    pub size: [usize; 2],
    pub sharpness: Arc<Vec<u8>>,
    overlay: Option<(f32, TextureHandle)>,
}

impl FocusMap {
    pub fn new(size: [usize; 2], sharpness: Arc<Vec<u8>>) -> Self {
        Self { version: None, size, sharpness, overlay: None }
    }

    /// Overlay texture for a sensitivity (made again only when it changes).
    pub fn overlay(&mut self, ctx: &egui::Context, name: &str, sensitivity: f32) -> TextureId {
        let [w, h] = self.size;
        if self.overlay.as_ref().is_none_or(|(s, _)| *s != sensitivity) {
            let t = sk_core::focus::threshold(sensitivity);
            let pixels = self.sharpness.iter().map(|v| if *v >= t { MARK } else { Color32::TRANSPARENT }).collect();
            let image = ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels };
            match &mut self.overlay {
                Some((s, tex)) => {
                    tex.set(image, TextureOptions::NEAREST);
                    *s = sensitivity;
                }
                None => self.overlay = Some((sensitivity, ctx.load_texture(name, image, TextureOptions::NEAREST))),
            }
        }
        self.overlay.as_ref().map(|(_, t)| t.id()).expect("overlay was just made")
    }
}

/// Sharpness map of a rendering whose long edge is `edge` pixels at full size (`image` may be a
/// part of it).
pub fn sharpness_of(image: &ColorImage, edge: usize) -> Arc<Vec<u8>> {
    let [w, h] = image.size;
    let luma: Vec<f32> =
        image.pixels.iter().map(|c| (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0).collect();
    Arc::new(sk_core::focus::sharpness_at(&luma, w, h, edge as f32 / sk_core::focus::REFERENCE_EDGE))
}

impl App {
    pub fn toggle_focus_mask(&mut self) {
        self.focus_mask = !self.focus_mask;
        if self.focus_mask {
            // The images on screen are analysed as they are rendered or loaded again.
            self.focus_render_for = None;
            self.compare_requested.clear();
        }
    }

    /// Analyses a rendering of an image if the focus mask is on. `version`: edit version of a
    /// rendering of the image being edited.
    pub(crate) fn request_focus(&mut self, path: &Path, image: &ColorImage, version: Option<u64>) {
        if self.focus_mask {
            self.focus_pending.insert(path.to_path_buf());
            self.workers.focus(path.to_path_buf(), Arc::new(image.clone()), version);
        }
    }

    pub(crate) fn focus_received(&mut self, path: PathBuf, version: Option<u64>, size: [usize; 2], sharpness: Arc<Vec<u8>>) {
        self.focus_pending.remove(&path);
        self.focus_maps.insert(path, FocusMap { version, ..FocusMap::new(size, sharpness) });
    }

    /// The focus mask of an image fits what is shown: for the image being edited, it was
    /// computed from the shown rendering (or one being replaced by a draft).
    pub(crate) fn focus_map_current(&self, path: &Path) -> bool {
        let Some(map) = self.focus_maps.get(path) else { return false };
        let editing = self.current_path().as_deref() == Some(path) && self.loading.is_none();
        !editing || map.version == Some(self.texture_version_shown()) || (self.texture_draft && map.version.is_some())
    }

    /// The current image is shown without a fresh rendering (e.g. from memory), or its mask is
    /// outdated: render it once more, so that its focus mask gets computed.
    pub(crate) fn ensure_focus_of_current(&mut self) {
        if !self.focus_mask || self.loading.is_some() {
            return;
        }
        let Some(path) = self.current_path() else { return };
        let key = (self.generation(), self.texture_version_shown());
        if !self.focus_map_current(&path) && !self.focus_pending.contains(&path) && self.focus_render_for != Some(key) {
            self.focus_render_for = Some(key);
            self.request_render();
        }
    }

    /// Overlay texture of an image whose shown rendering has aspect ratio `aspect`.
    pub(crate) fn focus_overlay(&mut self, ctx: &egui::Context, path: &Path, aspect: f32) -> Option<TextureId> {
        if !self.focus_mask {
            return None;
        }
        if !self.focus_map_current(path) {
            return None;
        }
        let sensitivity = self.focus_sensitivity;
        let map = self.focus_maps.get_mut(path)?;
        let [w, h] = map.size;
        // A mask of an older framing (cropped or rotated since) doesn't fit.
        if ((w as f32 / h.max(1) as f32) - aspect).abs() > 0.01 {
            return None;
        }
        Some(map.overlay(ctx, &format!("focus:{}", path.display()), sensitivity))
    }
}
