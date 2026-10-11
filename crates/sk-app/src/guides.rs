//! Composition guides and a custom layout overlay over the viewer image.

use std::path::PathBuf;

use egui::{Color32, Painter, Pos2, Rect, Stroke, TextureHandle, TextureOptions, pos2};
use serde::{Deserialize, Serialize};

use crate::app::App;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Guides {
    #[default]
    Off,
    Thirds,
    Golden,
    Grid,
    Diagonals,
}

impl Guides {
    pub const ALL: [Guides; 5] = [Guides::Off, Guides::Thirds, Guides::Golden, Guides::Grid, Guides::Diagonals];

    pub fn label(self) -> &'static str {
        match self {
            Guides::Off => "Keine",
            Guides::Thirds => "Drittel",
            Guides::Golden => "Goldener Schnitt",
            Guides::Grid => "Raster",
            Guides::Diagonals => "Diagonalen",
        }
    }

    /// O: the next kind of guides.
    pub fn next(self) -> Guides {
        let i = Guides::ALL.iter().position(|g| *g == self).unwrap_or(0);
        Guides::ALL[(i + 1) % Guides::ALL.len()]
    }

    /// Lines in image coordinates (0..1).
    fn lines(self, aspect: f32) -> Vec<[Pos2; 2]> {
        let v = |x: f32| [pos2(x, 0.0), pos2(x, 1.0)];
        let h = |y: f32| [pos2(0.0, y), pos2(1.0, y)];
        match self {
            Guides::Off => Vec::new(),
            Guides::Thirds => vec![v(1.0 / 3.0), v(2.0 / 3.0), h(1.0 / 3.0), h(2.0 / 3.0)],
            Guides::Golden => {
                let g = 0.381_966;
                vec![v(g), v(1.0 - g), h(g), h(1.0 - g)]
            }
            Guides::Grid => {
                // Square cells, about 8 along the long edge.
                let cell = aspect.max(1.0) / 8.0;
                let (nx, ny) = ((aspect / cell).round().max(1.0), (1.0 / cell).round().max(1.0));
                let xs = (1..nx as usize).map(|i| v(i as f32 / nx));
                let ys = (1..ny as usize).map(|i| h(i as f32 / ny));
                xs.chain(ys).collect()
            }
            Guides::Diagonals => vec![[pos2(0.0, 0.0), pos2(1.0, 1.0)], [pos2(1.0, 0.0), pos2(0.0, 1.0)]],
        }
    }
}

/// The custom overlay (a layout PNG with transparency).
#[derive(Default)]
pub struct Overlay {
    pub file: Option<PathBuf>,
    pub shown: bool,
    pub opacity: f32,
    texture: Option<(PathBuf, TextureHandle)>,
    pub error: Option<String>,
}

impl Overlay {
    pub fn new(file: Option<PathBuf>, opacity: Option<f32>) -> Self {
        Self { file, opacity: opacity.unwrap_or(0.8).clamp(0.05, 1.0), ..Default::default() }
    }

    fn texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        let file = self.file.clone()?;
        if self.texture.as_ref().is_none_or(|(p, _)| *p != file) {
            self.texture = None;
            match image::open(&file) {
                Ok(img) => {
                    // Larger textures than the GPU allows would abort the app.
                    let side = ctx.input(|i| i.max_texture_side).min(4096) as u32;
                    let img = if img.width().max(img.height()) > side { img.resize(side, side, image::imageops::FilterType::Triangle) } else { img };
                    let rgba = img.to_rgba8();
                    let image = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
                    self.texture = Some((file, ctx.load_texture("layout overlay", image, TextureOptions::LINEAR)));
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(format!("Overlay nicht lesbar: {e}"));
                    self.shown = false;
                    return None;
                }
            }
        }
        self.texture.as_ref().map(|(_, t)| t)
    }
}

impl App {
    /// Shift+O: the layout overlay on or off; asks for a file the first time.
    pub fn toggle_overlay(&mut self) {
        if self.overlay.shown {
            self.overlay.shown = false;
        } else if self.overlay.file.is_some() {
            self.overlay.shown = true;
        } else {
            self.choose_overlay();
        }
    }

    pub fn choose_overlay(&mut self) {
        if let Some(f) = rfd::FileDialog::new().set_title("Overlay wählen (PNG mit Transparenz)").add_filter("Bilder", &["png", "jpg", "jpeg", "tif", "tiff"]).pick_file() {
            self.overlay.file = Some(f);
            self.overlay.shown = true;
            // Also reload a file of the same name that was changed in the meantime.
            self.overlay.texture = None;
            self.save_settings();
        }
    }

    /// Draws the overlay (fitted into the image, centered) and the guides over `img`.
    pub fn draw_guides(&mut self, ctx: &egui::Context, painter: &Painter, img: Rect) {
        if self.overlay.shown {
            let opacity = self.overlay.opacity;
            if let Some(t) = self.overlay.texture(ctx) {
                let size = t.size_vec2();
                let fit = (img.width() / size.x).min(img.height() / size.y);
                let r = Rect::from_center_size(img.center(), size * fit);
                painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::from_white_alpha((opacity * 255.0) as u8));
            }
        }
        let to_screen = |p: Pos2| img.min + (p.to_vec2() * img.size());
        for [a, b] in self.guides.lines(img.width() / img.height().max(1.0)) {
            let (a, b) = (to_screen(a), to_screen(b));
            // A dark line under a light one, visible on any image.
            painter.line_segment([a + egui::vec2(1.0, 1.0), b + egui::vec2(1.0, 1.0)], Stroke::new(1.0, Color32::from_black_alpha(110)));
            painter.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(170)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_cells_are_square() {
        let lines = Guides::Grid.lines(1.5);
        let xs = lines.iter().filter(|l| l[0].x == l[1].x).count() + 1;
        let ys = lines.iter().filter(|l| l[0].y == l[1].y).count() + 1;
        assert_eq!((xs, ys), (8, 5));
        let lines = Guides::Grid.lines(2.0 / 3.0);
        let xs = lines.iter().filter(|l| l[0].x == l[1].x).count() + 1;
        let ys = lines.iter().filter(|l| l[0].y == l[1].y).count() + 1;
        assert_eq!((xs, ys), (5, 8));
        assert_eq!(Guides::Diagonals.next(), Guides::Off);
    }
}
