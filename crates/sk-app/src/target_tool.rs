//! Zielkorrektur (as in Lightroom): with the crosshair of the Gradationskurve or the Farbeditor
//! on, dragging up or down in the image changes what lies under the mouse where the drag began:
//! the curve at that tone, or the color bands of that hue (Farbton, Sättigung or Helligkeit, as
//! the editor shows). On the active layer, or on the whole image.

use egui::Vec2;
use sk_core::params::{Curve, HslBand};
use sk_core::pipeline::Processor;

use crate::app::{App, CurveChannel};
use crate::develop_tools::ColorView;

/// What dragging in the image changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetTool {
    Curve,
    /// The bands' hue, saturation or lightness.
    Colors(ColorView),
}

/// What a drag holds on to (taken where it began).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Grip {
    /// The curve point at the tone under the mouse (index in the curve's points).
    CurvePoint(usize),
    /// How much each color band has of the hue under the mouse.
    Bands([f32; 8]),
}

/// Curve change per point dragged.
const CURVE_PER_POINT: f32 = 1.0 / 300.0;
/// Points of the curve within this of the tone are taken instead of adding one.
const NEAR_POINT: f32 = 0.04;

impl App {
    /// Switches the Zielkorrektur for `tool` on, or off if it is on.
    pub(crate) fn toggle_target_tool(&mut self, tool: TargetTool) {
        self.target_tool = if self.target_tool == Some(tool) { None } else { Some(tool) };
        self.target_grip = None;
        if self.target_tool.is_some() {
            (self.picking_wb, self.picking_skin, self.picking_focus) = (false, false, false);
        }
    }

    /// A drag began at `image_pos` (displayed preview pixels): what it will change.
    pub(crate) fn target_grip_at(&mut self, image_pos: Vec2) {
        self.target_grip = match self.target_tool {
            Some(TargetTool::Curve) => self.curve_grip(image_pos),
            Some(TargetTool::Colors(_)) => self.band_grip(image_pos),
            None => None,
        };
    }

    /// The point of the curve shown at the tone under the mouse (added if there is none near).
    fn curve_grip(&mut self, image_pos: Vec2) -> Option<Grip> {
        let rgb = self.shown_pixel(image_pos)?;
        let channel = self.curve_channel;
        // The shown tone is the curve's output: the input is where the curve gives it.
        let out = match channel {
            CurveChannel::Red => rgb[0] as f32,
            CurveChannel::Green => rgb[1] as f32,
            CurveChannel::Blue => rgb[2] as f32,
            CurveChannel::Rgb | CurveChannel::Luma => 0.2126 * rgb[0] as f32 + 0.7152 * rgb[1] as f32 + 0.0722 * rgb[2] as f32,
        } / 255.0;
        let layer = self.active_layer;
        let cur = self.current.as_mut()?;
        let curves = match layer.and_then(|i| cur.adjustments.layers.get_mut(i)) {
            Some(l) => &mut l.adjustments.curves,
            None => &mut cur.adjustments.curves,
        };
        let curve = channel.curve_mut(curves);
        let x = curve_input(curve, out);
        let near = curve.points.iter().enumerate().filter(|(_, p)| (p[0] - x).abs() < NEAR_POINT).min_by(|a, b| (a.1[0] - x).abs().total_cmp(&(b.1[0] - x).abs())).map(|(i, _)| i);
        Some(Grip::CurvePoint(near.unwrap_or_else(|| {
            let lut = curve.build_lut(256);
            curve.points.push([x, lut[(x * 255.0).round() as usize]]);
            curve.points.len() - 1
        })))
    }

    /// The bands of the hue under the mouse, where the color adjustments see it.
    fn band_grip(&self, image_pos: Vec2) -> Option<Grip> {
        let avg = self.sample_camera_rgb(image_pos)?;
        let cur = self.current.as_ref()?;
        let [_, a, b] = Processor::new(&cur.meta, &cur.adjustments).color_input(avg);
        // Grey has no hue to go by.
        if (a * a + b * b).sqrt() < 0.01 {
            return None;
        }
        let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
        Some(Grip::Bands(sk_core::pipeline::band_weights(hue)))
    }

    /// The mouse moved `dy` points down while dragging: changes what the grip holds.
    pub(crate) fn target_drag(&mut self, dy: f32) {
        let (Some(tool), Some(grip)) = (self.target_tool, self.target_grip) else { return };
        let up = -dy;
        let layer = self.active_layer;
        let channel = self.curve_channel;
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        match (tool, grip) {
            (TargetTool::Curve, Grip::CurvePoint(i)) => {
                let curves = match layer.and_then(|l| adj.layers.get_mut(l)) {
                    Some(l) => &mut l.adjustments.curves,
                    None => &mut adj.curves,
                };
                let Some(p) = channel.curve_mut(curves).points.get_mut(i) else { return };
                p[1] = (p[1] + up * CURVE_PER_POINT).clamp(0.0, 1.0);
                self.speed_message = Some(format!("   Gradationskurve: Eingang {:.0}, Ausgang {:.0}", p[0] * 255.0, p[1] * 255.0));
            }
            (TargetTool::Colors(view), Grip::Bands(weights)) => {
                let bands = match layer.and_then(|l| adj.layers.get_mut(l)) {
                    Some(l) => &mut l.adjustments.color_bands,
                    None => &mut adj.color_editor.bands,
                };
                for (b, w) in bands.iter_mut().zip(weights) {
                    change_band(b, view, up * w);
                }
                let main = weights.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i);
                self.speed_message = Some(format!("   Farbeditor: {} {}", sk_core::params::HUE_BANDS[main].0, view.label()));
            }
            _ => return,
        }
        self.request_render();
    }
}

/// The input of `curve` that gives `out` (the nearest, for a curve that is not monotonic).
fn curve_input(curve: &Curve, out: f32) -> f32 {
    let lut = curve.build_lut(256);
    let i = lut.iter().enumerate().min_by(|a, b| (a.1 - out).abs().total_cmp(&(b.1 - out).abs())).map_or(0, |(i, _)| i);
    i as f32 / 255.0
}

/// Changes a band by `points` of mouse way: hue in tenths of a degree, the rest in halves.
fn change_band(b: &mut HslBand, view: ColorView, points: f32) {
    match view {
        ColorView::Hue => b.hue = (b.hue + points * 0.1).clamp(-30.0, 30.0),
        ColorView::Saturation => b.saturation = (b.saturation + points * 0.5).clamp(-100.0, 100.0),
        _ => b.lightness = (b.lightness + points * 0.5).clamp(-100.0, 100.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_input_of_a_shown_tone() {
        let mut c = Curve::default();
        assert!((curve_input(&c, 0.5) - 0.5).abs() < 0.01);
        c.points = vec![[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]];
        assert!((curve_input(&c, 0.75) - 0.5).abs() < 0.01);
    }

    #[test]
    fn bands_change_by_their_share() {
        let mut b = HslBand::default();
        change_band(&mut b, ColorView::Saturation, 20.0);
        change_band(&mut b, ColorView::Hue, 1000.0);
        assert_eq!((b.saturation, b.hue, b.lightness), (10.0, 30.0, 0.0));
    }
}
