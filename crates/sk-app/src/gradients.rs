//! Linear and radial gradients of a layer mask in the viewer, edited with
//! three lines or ellipses (full effect, middle, end of the transition) with handles.
//!
//! Linear: the middle line or its center point moves the gradient, the outer lines widen or
//! narrow the transition (symmetrically around the middle), the round handle rotates it.
//! Radial: the center point moves it, the middle ellipse scales it, its four square handles
//! change width or height, the inner and outer ellipse the soft edge, the round handle rotates.
//! A drag anywhere else draws a new gradient.

use egui::{Color32, Pos2, Rect, Response, Stroke, Ui, Vec2, vec2};
use sk_core::params::{LinearGradient, RadialGradient};

use crate::app::App;
use crate::layers::{LocalDrag, LocalTool, UvToScreen};

/// Grab distance of handles and lines on screen (pixels).
const GRAB: f32 = 8.0;
/// Distance of the rotate handle from the line or ellipse (pixels).
const ROTATE_OFFSET: f32 = 28.0;

/// A gradient of the active layer's mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GradRef {
    pub layer: usize,
    pub linear: bool,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Move,
    Rotate,
    /// Linear: an outer line. Radial: the inner or outer ellipse.
    Feather,
    /// Radial: the middle ellipse.
    Scale,
    /// Radial: the handles at the ends of the first and second axis.
    SizeX,
    SizeY,
}

/// Drag state: what is dragged, the gradient as it was and where the drag started.
#[derive(Clone, Copy, Debug)]
pub struct GradDrag {
    pub grad: GradRef,
    pub part: Part,
    pub grab: [f32; 2],
    pub linear: LinearGradient,
    pub radial: RadialGradient,
}

/// Normalized source coordinates <-> a frame in units of the long edge (undistorted).
#[derive(Clone, Copy)]
struct Frame {
    kx: f32,
    ky: f32,
}

impl Frame {
    fn new(aspect: f32) -> Self {
        if aspect >= 1.0 { Self { kx: 1.0, ky: 1.0 / aspect } } else { Self { kx: aspect, ky: 1.0 } }
    }
    fn to(self, uv: [f32; 2]) -> Vec2 {
        vec2(uv[0] * self.kx, uv[1] * self.ky)
    }
    fn from(self, v: Vec2) -> [f32; 2] {
        [v.x / self.kx, v.y / self.ky]
    }
}

fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    vec2(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Point on the ellipse scaled by `k` (1 = outer) at parameter `t`.
fn ellipse_point(f: Frame, g: &RadialGradient, k: f32, t: f32) -> [f32; 2] {
    let local = vec2(g.radius_x * t.cos(), g.radius_y * t.sin()) * k;
    f.from(f.to(g.center) + rotate(local, g.angle))
}

/// Position in the ellipse's own axes (long-edge units).
fn ellipse_local(f: Frame, g: &RadialGradient, uv: [f32; 2]) -> Vec2 {
    rotate(f.to(uv) - f.to(g.center), -g.angle)
}

/// Middle ellipse (50 %) relative to the outer one.
fn middle_k(g: &RadialGradient) -> f32 {
    1.0 - g.feather.clamp(0.0, 1.0) / 2.0
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

fn dist_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    pts.windows(2).map(|w| dist_to_segment(p, w[0], w[1])).fold(f32::MAX, f32::min)
}

/// Screen geometry of a linear gradient: start (100 %), middle and end (0 %) points, the
/// direction from start to end and the rotate handle.
struct LinearShape {
    start: Pos2,
    mid: Pos2,
    end: Pos2,
    dir: Vec2,
    rotate: Pos2,
}

fn linear_shape(map: &UvToScreen, g: &LinearGradient) -> LinearShape {
    let (start, end) = (map.map(g.start), map.map(g.end));
    let mid = start + (end - start) / 2.0;
    let dir = (end - start).normalized();
    let normal = vec2(-dir.y, dir.x);
    LinearShape { start, mid, end, dir, rotate: mid + normal * (ROTATE_OFFSET * 2.0) }
}

/// Screen geometry of a radial gradient: the three ellipses, the size handles and the rotate handle.
struct RadialShape {
    center: Pos2,
    outer: Vec<Pos2>,
    middle: Vec<Pos2>,
    inner: Vec<Pos2>,
    size_x: [Pos2; 2],
    size_y: [Pos2; 2],
    rotate: Pos2,
    rotate_base: Pos2,
}

fn radial_shape(map: &UvToScreen, f: Frame, g: &RadialGradient) -> RadialShape {
    let ring = |k: f32| (0..=64).map(|i| map.map(ellipse_point(f, g, k, i as f32 / 64.0 * std::f32::consts::TAU))).collect::<Vec<_>>();
    let m = middle_k(g);
    let at = |k: f32, t: f32| map.map(ellipse_point(f, g, k, t));
    use std::f32::consts::{FRAC_PI_2, PI};
    let center = map.map(g.center);
    let rotate_base = at(1.0, -FRAC_PI_2);
    let up = (rotate_base - center).normalized();
    RadialShape {
        center,
        outer: ring(1.0),
        middle: ring(m),
        inner: ring(1.0 - g.feather.clamp(0.0, 1.0)),
        size_x: [at(m, 0.0), at(m, PI)],
        size_y: [at(m, FRAC_PI_2), at(m, -FRAC_PI_2)],
        rotate: rotate_base + up * ROTATE_OFFSET,
        rotate_base,
    }
}

impl App {
    fn frame(&self) -> Frame {
        Frame::new(self.current.as_ref().map_or(1.0, |c| c.preview.width as f32 / c.preview.height.max(1) as f32))
    }

    /// The gradient part under the pointer, the selected gradient first.
    fn gradient_at(&self, p: Pos2, map: &UvToScreen) -> Option<(GradRef, Part)> {
        let cur = self.current.as_ref()?;
        let li = self.active_layer?;
        let mask = &cur.adjustments.layers.get(li)?.mask;
        let f = self.frame();
        let mut best: Option<(f32, GradRef, Part)> = None;
        let mut consider = |d: f32, grad: GradRef, part: Part, bias: f32| {
            if d <= GRAB && best.is_none_or(|b| d - bias < b.0) {
                best = Some((d - bias, grad, part));
            }
        };
        for (i, g) in mask.linear.iter().enumerate() {
            let grad = GradRef { layer: li, linear: true, index: i };
            let bias = if self.selected_gradient == Some(grad) { 2.0 } else { 0.0 };
            let s = linear_shape(map, g);
            // Handles before lines.
            consider((p - s.mid).length(), grad, Part::Move, bias + 4.0);
            consider((p - s.rotate).length(), grad, Part::Rotate, bias + 4.0);
            let across = |o: Pos2| (p - o).dot(s.dir).abs();
            consider(across(s.mid), grad, Part::Move, bias);
            consider(across(s.start), grad, Part::Feather, bias);
            consider(across(s.end), grad, Part::Feather, bias);
        }
        for (i, g) in mask.radial.iter().enumerate() {
            let grad = GradRef { layer: li, linear: false, index: i };
            let bias = if self.selected_gradient == Some(grad) { 2.0 } else { 0.0 };
            let s = radial_shape(map, f, g);
            consider((p - s.center).length(), grad, Part::Move, bias + 4.0);
            consider((p - s.rotate).length(), grad, Part::Rotate, bias + 4.0);
            for h in s.size_x {
                consider((p - h).length(), grad, Part::SizeX, bias + 4.0);
            }
            for h in s.size_y {
                consider((p - h).length(), grad, Part::SizeY, bias + 4.0);
            }
            consider(dist_to_polyline(p, &s.middle), grad, Part::Scale, bias);
            consider(dist_to_polyline(p, &s.outer), grad, Part::Feather, bias);
            consider(dist_to_polyline(p, &s.inner), grad, Part::Feather, bias);
        }
        best.map(|(_, g, p)| (g, p))
    }

    /// Viewer interaction of the linear and radial tools: edit an existing gradient or draw a new one.
    pub(crate) fn gradient_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32, tool: LocalTool) {
        let Some(map) = self.uv_mapper(img_rect, scale) else { return };
        let hovered = response.hover_pos().and_then(|p| self.gradient_at(p, &map));
        let cursor = match (&self.local_drag, hovered) {
            (LocalDrag::EditGradient(_), _) => egui::CursorIcon::Grabbing,
            (_, Some((_, Part::Move))) => egui::CursorIcon::Move,
            (_, Some(_)) => egui::CursorIcon::Grab,
            _ => egui::CursorIcon::Crosshair,
        };
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }

        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            && let Some(uv) = self.screen_to_uv(origin, img_rect, scale)
        {
            if let Some((grad, part)) = self.gradient_at(origin, &map) {
                let mask = &self.current.as_ref().expect("gradient without image").adjustments.layers[grad.layer].mask;
                let linear = if grad.linear { mask.linear[grad.index] } else { LinearGradient { start: uv, end: uv } };
                let radial = if grad.linear { RadialGradient::default() } else { mask.radial[grad.index] };
                self.selected_gradient = Some(grad);
                self.local_drag = LocalDrag::EditGradient(GradDrag { grad, part, grab: uv, linear, radial });
            } else if let Some(li) = self.ensure_layer() {
                let linear = tool == LocalTool::Linear;
                let mut index = 0;
                self.edit(|a| {
                    let m = &mut a.layers[li].mask;
                    if linear {
                        m.linear.push(LinearGradient { start: uv, end: uv });
                        index = m.linear.len() - 1;
                    } else {
                        m.radial.push(RadialGradient { center: uv, radius_x: 0.001, radius_y: 0.001, feather: 0.5, invert: false, angle: 0.0 });
                        index = m.radial.len() - 1;
                    }
                });
                self.selected_gradient = Some(GradRef { layer: li, linear, index });
                self.local_drag = LocalDrag::Gradient;
            }
        }

        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            let f = self.frame();
            match self.local_drag.clone() {
                LocalDrag::Gradient => {
                    if let Some(GradRef { layer, linear, index }) = self.selected_gradient {
                        self.edit(|a| {
                            let Some(m) = a.layers.get_mut(layer).map(|l| &mut l.mask) else { return };
                            if linear {
                                if let Some(g) = m.linear.get_mut(index) {
                                    g.end = uv;
                                }
                            } else if let Some(g) = m.radial.get_mut(index) {
                                let r = (f.to(uv) - f.to(g.center)).length().max(0.002);
                                g.radius_x = r;
                                g.radius_y = r;
                            }
                        });
                    }
                }
                LocalDrag::EditGradient(d) => {
                    self.edit(|a| {
                        let Some(m) = a.layers.get_mut(d.grad.layer).map(|l| &mut l.mask) else { return };
                        if d.grad.linear {
                            if let Some(g) = m.linear.get_mut(d.grad.index) {
                                *g = drag_linear(f, &d, uv);
                            }
                        } else if let Some(g) = m.radial.get_mut(d.grad.index) {
                            *g = drag_radial(f, &d, uv);
                        }
                    });
                }
                _ => {}
            }
        }
        self.draw_gradients(&ui.painter_at(response.rect), &map, hovered);
    }

    /// The gradients of the active layer; the selected one with its handles.
    fn draw_gradients(&self, painter: &egui::Painter, map: &UvToScreen, hovered: Option<(GradRef, Part)>) {
        let Some(cur) = &self.current else { return };
        let Some(li) = self.active_layer else { return };
        let Some(layer) = cur.adjustments.layers.get(li) else { return };
        let f = self.frame();
        // Long enough to cross the viewer, short enough for the dashes.
        let reach = painter.clip_rect().size().length();
        let shadow = Stroke::new(3.0, Color32::from_black_alpha(90));
        let line = |pts: &[Pos2], width: f32, color: Color32| {
            painter.add(egui::Shape::line(pts.to_vec(), shadow));
            painter.add(egui::Shape::line(pts.to_vec(), Stroke::new(width, color)));
        };
        let dashed = |pts: &[Pos2], color: Color32| {
            painter.extend(egui::Shape::dashed_line(pts, Stroke::new(1.0, color), 6.0, 4.0));
        };
        let dot = |p: Pos2, r: f32, active: bool| {
            painter.circle_filled(p, r + 1.5, Color32::from_black_alpha(140));
            painter.circle_filled(p, r, if active { crate::theme::ACCENT } else { Color32::WHITE });
        };
        let square = |p: Pos2, active: bool| {
            let r = Rect::from_center_size(p, vec2(8.0, 8.0));
            painter.rect_filled(r.expand(1.5), 1.0, Color32::from_black_alpha(140));
            painter.rect_filled(r, 1.0, if active { crate::theme::ACCENT } else { Color32::WHITE });
        };
        let faint = Color32::from_white_alpha(150);
        let is = |grad: GradRef, part: Part| hovered == Some((grad, part)) || matches!(&self.local_drag, LocalDrag::EditGradient(d) if d.grad == grad && d.part == part);

        for (i, g) in layer.mask.linear.iter().enumerate() {
            let grad = GradRef { layer: li, linear: true, index: i };
            let s = linear_shape(map, g);
            let n = vec2(-s.dir.y, s.dir.x) * reach;
            let selected = self.selected_gradient == Some(grad);
            let w = |part| if is(grad, part) { 2.5 } else { 1.5 };
            line(&[s.mid - n, s.mid + n], w(Part::Move), Color32::WHITE);
            if selected || is(grad, Part::Feather) {
                dashed(&[s.start - n, s.start + n], if is(grad, Part::Feather) { Color32::WHITE } else { faint });
                dashed(&[s.end - n, s.end + n], if is(grad, Part::Feather) { Color32::WHITE } else { faint });
            }
            dot(s.mid, 4.5, is(grad, Part::Move));
            if selected {
                painter.line_segment([s.mid, s.rotate], Stroke::new(1.0, faint));
                dot(s.rotate, 4.0, is(grad, Part::Rotate));
            }
        }
        for (i, g) in layer.mask.radial.iter().enumerate() {
            let grad = GradRef { layer: li, linear: false, index: i };
            let s = radial_shape(map, f, g);
            let selected = self.selected_gradient == Some(grad);
            line(&s.middle, if is(grad, Part::Scale) { 2.5 } else { 1.5 }, Color32::WHITE);
            if selected || is(grad, Part::Feather) {
                let c = if is(grad, Part::Feather) { Color32::WHITE } else { faint };
                dashed(&s.outer, c);
                dashed(&s.inner, c);
            }
            dot(s.center, 4.0, is(grad, Part::Move));
            if selected {
                for h in s.size_x {
                    square(h, is(grad, Part::SizeX));
                }
                for h in s.size_y {
                    square(h, is(grad, Part::SizeY));
                }
                painter.line_segment([s.rotate_base, s.rotate], Stroke::new(1.0, faint));
                dot(s.rotate, 4.0, is(grad, Part::Rotate));
            }
        }
    }

    /// Deletes the selected gradient (Entf). Returns whether there was one.
    pub(crate) fn delete_selected_gradient(&mut self) -> bool {
        let Some(grad) = self.selected_gradient.take() else { return false };
        if self.active_layer != Some(grad.layer) || !matches!(self.local_tool, Some(LocalTool::Linear | LocalTool::Radial)) {
            return false;
        }
        self.edit(|a| {
            if let Some(l) = a.layers.get_mut(grad.layer) {
                if grad.linear && grad.index < l.mask.linear.len() {
                    l.mask.linear.remove(grad.index);
                } else if !grad.linear && grad.index < l.mask.radial.len() {
                    l.mask.radial.remove(grad.index);
                }
            }
        });
        true
    }
}

/// The linear gradient while dragging `d.part` to `uv`.
fn drag_linear(f: Frame, d: &GradDrag, uv: [f32; 2]) -> LinearGradient {
    let g = d.linear;
    let (s, e) = (f.to(g.start), f.to(g.end));
    let mid = (s + e) / 2.0;
    let half = (e - s) / 2.0;
    let (p, p0) = (f.to(uv), f.to(d.grab));
    match d.part {
        Part::Move => LinearGradient { start: f.from(s + p - p0), end: f.from(e + p - p0) },
        Part::Feather => {
            let dir = half.normalized();
            let h = (p - mid).dot(dir).abs().max(0.001);
            LinearGradient { start: f.from(mid - dir * h), end: f.from(mid + dir * h) }
        }
        Part::Rotate => {
            let a = (p - mid).angle() - (p0 - mid).angle();
            let half = rotate(half, a);
            LinearGradient { start: f.from(mid - half), end: f.from(mid + half) }
        }
        _ => g,
    }
}

/// The radial gradient while dragging `d.part` to `uv`.
fn drag_radial(f: Frame, d: &GradDrag, uv: [f32; 2]) -> RadialGradient {
    let g0 = d.radial;
    let mut g = g0;
    let local = ellipse_local(f, &g0, uv);
    let m = middle_k(&g0);
    let min_r = 0.002;
    match d.part {
        Part::Move => g.center = f.from(f.to(g0.center) + f.to(uv) - f.to(d.grab)),
        Part::Rotate => {
            let c = f.to(g0.center);
            g.angle = g0.angle + (f.to(uv) - c).angle() - (f.to(d.grab) - c).angle();
        }
        // The middle ellipse follows the pointer, the soft edge keeps its share.
        Part::SizeX => g.radius_x = (local.x.abs() / m).max(min_r),
        Part::SizeY => g.radius_y = (local.y.abs() / m).max(min_r),
        Part::Scale => {
            let e = ((local.x / g0.radius_x).powi(2) + (local.y / g0.radius_y).powi(2)).sqrt();
            let k = (e / m).max(min_r / g0.radius_x.min(g0.radius_y));
            g.radius_x = g0.radius_x * k;
            g.radius_y = g0.radius_y * k;
        }
        // The middle ellipse stays; inner and outer move apart or together.
        Part::Feather => {
            let e = ((local.x / g0.radius_x).powi(2) + (local.y / g0.radius_y).powi(2)).sqrt();
            let h = (e - m).abs().clamp(0.0, m);
            let k = m + h;
            g.radius_x = g0.radius_x * k;
            g.radius_y = g0.radius_y * k;
            g.feather = (2.0 * h / k).clamp(0.0, 1.0);
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(part: Part, radial: RadialGradient, grab: [f32; 2]) -> GradDrag {
        GradDrag { grad: GradRef { layer: 0, linear: false, index: 0 }, part, grab, linear: LinearGradient { start: [0.0; 2], end: [0.0; 2] }, radial }
    }

    #[test]
    fn radial_feather_keeps_the_middle_ellipse() {
        let f = Frame::new(1.5);
        let g0 = RadialGradient { center: [0.5, 0.5], radius_x: 0.2, radius_y: 0.1, feather: 0.4, invert: false, angle: 0.3 };
        // Drag the outer ellipse further out.
        let target = ellipse_point(f, &g0, 1.3, 1.0);
        let g = drag_radial(f, &drag(Part::Feather, g0, target), target);
        assert!((g.radius_x * middle_k(&g) - g0.radius_x * middle_k(&g0)).abs() < 1e-4);
        assert!((g.radius_y * middle_k(&g) - g0.radius_y * middle_k(&g0)).abs() < 1e-4);
        assert!(g.feather > g0.feather);
        // The outer ellipse now runs through the pointer.
        let local = ellipse_local(f, &g, target);
        let e = ((local.x / g.radius_x).powi(2) + (local.y / g.radius_y).powi(2)).sqrt();
        assert!((e - 1.0).abs() < 1e-3, "{e}");
    }

    #[test]
    fn radial_size_handle_moves_the_middle_ellipse_to_the_pointer() {
        let f = Frame::new(0.7);
        let g0 = RadialGradient { center: [0.4, 0.6], radius_x: 0.2, radius_y: 0.1, feather: 0.5, invert: false, angle: 1.0 };
        let target = ellipse_point(f, &g0, middle_k(&g0) * 1.5, 0.0);
        let g = drag_radial(f, &drag(Part::SizeX, g0, target), target);
        assert!((g.radius_x - g0.radius_x * 1.5).abs() < 1e-4);
        assert_eq!(g.radius_y, g0.radius_y);
    }

    #[test]
    fn linear_feather_is_symmetric_around_the_middle() {
        let f = Frame::new(1.5);
        let g0 = LinearGradient { start: [0.3, 0.5], end: [0.5, 0.5] };
        let d = GradDrag { grad: GradRef { layer: 0, linear: true, index: 0 }, part: Part::Feather, grab: g0.end, linear: g0, radial: RadialGradient::default() };
        let g = drag_linear(f, &d, [0.6, 0.2]);
        assert!((g.start[0] - 0.2).abs() < 1e-5 && (g.end[0] - 0.6).abs() < 1e-5);
        assert!((g.start[1] - 0.5).abs() < 1e-5 && (g.end[1] - 0.5).abs() < 1e-5);
    }
}
