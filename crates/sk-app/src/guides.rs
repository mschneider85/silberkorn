//! Composition guides over the viewer image.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2};
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

impl App {
    /// Draws the guides over `img`.
    pub fn draw_guides(&self, painter: &Painter, img: Rect) {
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
