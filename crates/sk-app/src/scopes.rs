//! "Messanzeige": histogram, waveform or vectorscope above the tools (see `sk_core::scopes`).
//! Shift+H switches, as do View → "Messanzeige" and a right-click on it. Waveform and vectorscope
//! are measured from the displayed image, and only while one of them is shown.

use std::sync::Arc;

use egui::{Color32, ColorImage, Rect, Response, Sense, Stroke, TextureOptions, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use sk_core::scopes::{self, Scopes, SKIN_ANGLE, VECTOR_RANGE, VECTOR_SIZE, WAVE_COLUMNS, WAVE_LEVELS};

use crate::app::App;
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeView {
    #[default]
    Histogram,
    Waveform,
    Vectorscope,
}

impl ScopeView {
    pub const ALL: [ScopeView; 3] = [ScopeView::Histogram, ScopeView::Waveform, ScopeView::Vectorscope];

    pub fn label(self) -> &'static str {
        match self {
            ScopeView::Histogram => "Histogramm",
            ScopeView::Waveform => "Wellenform",
            ScopeView::Vectorscope => "Vektorskop",
        }
    }

    fn next(self) -> Self {
        match self {
            ScopeView::Histogram => ScopeView::Waveform,
            ScopeView::Waveform => ScopeView::Vectorscope,
            ScopeView::Vectorscope => ScopeView::Histogram,
        }
    }
}

/// Brightness of a cell holding `share` of what it could: logarithmic, so that a few pixels show
/// and many do not merely saturate.
fn density(share: f32, gain: f32) -> f32 {
    ((1.0 + share * gain).ln() / (1.0 + gain).ln()).min(1.0)
}

/// Color with `alpha` (0..1) over the background of the scope.
fn shade(rgb: [f32; 3], alpha: f32) -> Color32 {
    let c = rgb.map(|v| (v * alpha * 255.0).round().clamp(0.0, 255.0) as u8);
    Color32::from_rgba_premultiplied(c[0], c[1], c[2], (alpha * 255.0).round() as u8)
}

/// The waveform: R, G and B added, so that neutral tones are white.
fn waveform_image(s: &Scopes) -> ColorImage {
    let pixels = (0..WAVE_LEVELS * WAVE_COLUMNS)
        .map(|i| {
            let (row, col) = (i / WAVE_COLUMNS, i % WAVE_COLUMNS);
            let n = s.waveform[(WAVE_LEVELS - 1 - row) * WAVE_COLUMNS + col];
            let per = s.per_column[col].max(1) as f32;
            let d = n.map(|v| density(v as f32 / per, 2000.0));
            let a = d[0].max(d[1]).max(d[2]);
            if a <= 0.0 {
                return Color32::TRANSPARENT;
            }
            // Blue alone is hard to see on dark gray: a little lighter.
            shade([d[0] / a + 0.15 * d[2] / a, d[1] / a + 0.15 * d[2] / a, d[2] / a], a)
        })
        .collect();
    ColorImage::new([WAVE_COLUMNS, WAVE_LEVELS], pixels)
}

/// The color a vectorscope cell stands for (light, so that it shows).
fn cell_color(x: usize, y: usize) -> [f32; 3] {
    let at = |c: usize| ((c as f32 + 0.5) / VECTOR_SIZE as f32 * 2.0 - 1.0) * VECTOR_RANGE;
    let (cb, cr) = (at(x), -at(y));
    let luma = 0.6;
    let (r, b) = (luma + 1.5748 * cr, luma + 1.8556 * cb);
    let g = (luma - 0.2126 * r - 0.0722 * b) / 0.7152;
    [r, g, b].map(|v| 0.45 + 0.55 * v.clamp(0.0, 1.0))
}

fn vectorscope_image(s: &Scopes) -> ColorImage {
    let total = s.total.max(1) as f32;
    let pixels = (0..VECTOR_SIZE * VECTOR_SIZE)
        .map(|i| {
            let n = s.vector[i];
            if n == 0 {
                return Color32::TRANSPARENT;
            }
            shade(cell_color(i % VECTOR_SIZE, i / VECTOR_SIZE), density(n as f32 / total, 2e5))
        })
        .collect();
    ColorImage::new([VECTOR_SIZE, VECTOR_SIZE], pixels)
}

impl App {
    /// Histogram → waveform → vectorscope → histogram.
    pub fn next_scope(&mut self) {
        self.set_scope(self.scope_view.next());
    }

    pub fn set_scope(&mut self, view: ScopeView) {
        self.scope_view = view;
        self.save_settings();
    }

    /// Entries to choose the scope (menu and right-click).
    pub(crate) fn scope_menu(&mut self, ui: &mut Ui) {
        for s in ScopeView::ALL {
            let mark = if self.scope_view == s { theme::icon::CHECK } else { "   " };
            if ui.button(format!("{mark}  {}", s.label())).clicked() {
                self.set_scope(s);
            }
        }
    }

    /// The texture of the waveform or vectorscope of the displayed image, made when either changed.
    fn scope_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        let shown = self.shown_image.clone()?;
        let current = self.scope_texture.as_ref().is_some_and(|(img, scope, _)| Arc::ptr_eq(img, &shown) && *scope == self.scope_view);
        if !current {
            let s = Scopes::measure(&shown.pixels, shown.size[0], |c| [c.r(), c.g(), c.b()]);
            let image = if self.scope_view == ScopeView::Waveform { waveform_image(&s) } else { vectorscope_image(&s) };
            match &mut self.scope_texture {
                Some((img, scope, tex)) => {
                    tex.set(image, TextureOptions::LINEAR);
                    (*img, *scope) = (shown, self.scope_view);
                }
                None => self.scope_texture = Some((shown, self.scope_view, ctx.load_texture("scope", image, TextureOptions::LINEAR))),
            }
        }
        self.scope_texture.as_ref().map(|(.., t)| t.id())
    }

    /// Waveform or vectorscope; the color under the mouse is marked.
    pub(crate) fn scope_widget(&mut self, ui: &mut Ui) -> Response {
        let width = ui.available_width();
        let height = if self.scope_view == ScopeView::Vectorscope { width.min(190.0) } else { 110.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        let tex = self.scope_texture(ui.ctx());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, theme::BROWSER);
        let grid = Stroke::new(1.0, Color32::from_white_alpha(18));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let small = egui::FontId::proportional(10.0);
        match self.scope_view {
            ScopeView::Waveform | ScopeView::Histogram => {
                let area = rect.shrink2(vec2(0.0, 3.0));
                for k in 1..4 {
                    let y = area.bottom() - area.height() * k as f32 / 4.0;
                    painter.hline(area.x_range(), y, grid);
                }
                if let Some(t) = tex {
                    painter.image(t, area, uv, Color32::WHITE);
                }
                if let Some(rgb) = self.hover_rgb {
                    let colors = [Color32::from_rgb(240, 90, 90), Color32::from_rgb(90, 215, 90), Color32::from_rgb(110, 150, 255)];
                    for (v, c) in rgb.iter().zip(colors) {
                        let y = area.bottom() - area.height() * *v as f32 / 255.0;
                        painter.hline(area.x_range(), y, Stroke::new(1.0, c.gamma_multiply(0.8)));
                    }
                }
            }
            ScopeView::Vectorscope => {
                let side = rect.height() - 6.0;
                let square = Rect::from_center_size(rect.center(), vec2(side, side));
                let (c, radius) = (square.center(), side / 2.0);
                let at = |chroma: [f32; 2]| c + vec2(chroma[0], -chroma[1]) / VECTOR_RANGE * radius;
                painter.circle_stroke(c, radius, grid);
                painter.hline(square.x_range(), c.y, grid);
                painter.vline(c.x, square.y_range(), grid);
                // Skin tones lie on this line, whatever the skin (only the distance differs).
                let skin = SKIN_ANGLE.to_radians();
                painter.line_segment([c, c + vec2(skin.cos(), -skin.sin()) * radius], Stroke::new(1.0, Color32::from_rgba_unmultiplied(230, 170, 130, 70)));
                // Where the primaries and secondaries lie at 75 %.
                for (rgb, name) in [([0.75, 0.0, 0.0], "R"), ([0.75, 0.75, 0.0], "Ge"), ([0.0, 0.75, 0.0], "Gr"), ([0.0, 0.75, 0.75], "C"), ([0.0, 0.0, 0.75], "B"), ([0.75, 0.0, 0.75], "M")] {
                    let p = at(scopes::chroma(rgb));
                    let color = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8).gamma_multiply(0.6);
                    painter.rect_stroke(Rect::from_center_size(p, vec2(7.0, 7.0)), 0.0, Stroke::new(1.0, color), egui::StrokeKind::Middle);
                    let out = (p - c).normalized() * 11.0;
                    painter.text(p + out, egui::Align2::CENTER_CENTER, name, small.clone(), color);
                }
                if let Some(t) = tex {
                    painter.image(t, square, uv, Color32::WHITE);
                }
                if let Some(rgb) = self.hover_rgb {
                    let p = at(scopes::chroma(rgb.map(|v| v as f32 / 255.0)));
                    painter.circle(p, 3.5, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
                }
            }
        }
        painter.text(rect.left_bottom() + vec2(6.0, -4.0), egui::Align2::LEFT_BOTTOM, self.scope_view.label(), small, theme::TEXT_DIM);
        crate::widgets::context_menu(&resp, |ui| self.scope_menu(ui));
        resp
    }
}
