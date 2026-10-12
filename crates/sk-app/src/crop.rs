//! Interactive crop and straighten tools in the viewer.

use sk_core::params::CropRect;
use egui::{Color32, Pos2, Rect, Response, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::App;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    Move,
    Corner(bool, bool),
    Edge(Side),
    New,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Debug, Default)]
pub struct CropTool {
    drag: Option<(Handle, CropRect, Pos2)>,
}

/// Straighten tool: the user draws a line that should become horizontal or vertical.
#[derive(Clone, Debug, Default)]
pub struct StraightenTool {
    line: Option<(Pos2, Pos2)>,
}

impl App {
    /// Straighten angle and keystone from the image's straight edges.
    pub fn auto_upright(&mut self, mode: sk_core::upright::UprightMode) {
        let Some(cur) = &self.current else { return };
        let segments = sk_core::upright::detect_segments(&cur.preview);
        let adj = &cur.adjustments;
        let src = (cur.preview.width, cur.preview.height);
        let Some(g) = sk_core::upright::solve(&segments, &adj.geometry, &adj.lens, adj.rotation, src, mode) else {
            self.status = "Automatisch geraderichten: keine geraden Kanten gefunden".into();
            return;
        };
        let keystone = if g.keystone_v != 0.0 || g.keystone_h != 0.0 {
            format!(", Keystone {:.0} / {:.0}", g.keystone_v, g.keystone_h)
        } else {
            String::new()
        };
        self.status = format!("Automatisch geraderichten: {:.1}°{keystone}", g.straighten).replace('.', ",");
        self.edit(|a| a.geometry = g);
        self.commit_and_save();
    }
}

/// Rotation (degrees) that makes a line with direction `d` (screen coordinates) axis aligned.
pub fn straighten_delta(d: Vec2) -> f32 {
    let a = d.y.atan2(d.x).to_degrees();
    a - (a / 90.0).round() * 90.0
}

/// Enforces the aspect ratio (pixel w/h) on a normalized rect, anchored at `anchor` (0..1 in rect).
pub fn constrain(rect: CropRect, ratio: Option<f32>, frame: Vec2, anchor: Vec2, by_width: bool) -> CropRect {
    let Some(r) = ratio else { return rect.clamped() };
    let mut c = rect;
    // r = (w * fw) / (h * fh)
    if by_width {
        c.h = c.w * frame.x / (frame.y * r);
    } else {
        c.w = c.h * frame.y * r / frame.x;
    }
    // Shrink to fit into the frame.
    let scale = (1.0f32).min(1.0 / c.w).min(1.0 / c.h);
    c.w *= scale;
    c.h *= scale;
    let ax = rect.x + rect.w * anchor.x;
    let ay = rect.y + rect.h * anchor.y;
    c.x = ax - c.w * anchor.x;
    c.y = ay - c.h * anchor.y;
    c.clamped()
}

impl App {
    fn aspect_value(&self, frame: Vec2) -> Option<f32> {
        let g = &self.current.as_ref()?.adjustments.geometry;
        g.aspect.value(frame.x / frame.y, g.aspect_portrait)
    }

    /// Ends every tool that takes the mouse in the viewer (layer tools, crop, straighten,
    /// pipettes). Only one may be active: the viewer hands the mouse to the first active one
    /// (see `viewer_content`), so a tool left on would swallow the input of the new one.
    pub fn end_viewer_tools(&mut self) {
        if self.local_tool.take().is_some() {
            self.request_render();
        }
        self.straighten_tool = None;
        self.picking_wb = false;
        self.picking_skin = false;
        self.local_drag = Default::default();
        self.finish_crop_tool();
    }

    /// The straighten tool (draw a line along the horizon) on or off.
    pub fn toggle_straighten(&mut self) {
        let on = self.straighten_tool.is_none();
        self.end_viewer_tools();
        if on {
            self.straighten_tool = Some(Default::default());
        }
    }

    /// The white balance pipette on or off.
    pub fn toggle_wb_picker(&mut self) {
        let on = !self.picking_wb;
        self.end_viewer_tools();
        self.picking_wb = on;
    }

    /// The skin tone pipette on or off.
    pub fn toggle_skin_picker(&mut self) {
        let on = !self.picking_skin;
        self.end_viewer_tools();
        self.picking_skin = on;
    }

    pub fn start_crop_tool(&mut self) {
        if self.crop_tool.is_none() {
            self.end_viewer_tools();
            self.crop_tool = Some(CropTool::default());
            self.zoom = None;
            self.request_render();
        }
    }

    pub fn finish_crop_tool(&mut self) {
        if self.crop_tool.take().is_some() {
            self.request_render();
        }
    }

    /// Applies the selected aspect ratio to the current crop.
    pub fn apply_aspect(&mut self) {
        let Some(tex) = &self.texture else { return };
        let Some(cur) = &self.current else { return };
        let frame = if self.crop_tool.is_some() {
            tex.size_vec2()
        } else {
            let p = sk_core::pipeline::Processor::with_options(
                &cur.meta,
                &cur.adjustments,
                sk_core::pipeline::RenderOptions { ignore_crop: true, ..Default::default() },
            );
            let (w, h) = p.output_size(cur.preview.width, cur.preview.height);
            vec2(w as f32, h as f32)
        };
        let ratio = self.aspect_value(frame);
        self.edit(|a| {
            let c = a.geometry.crop;
            let by_width = c.w * frame.x >= c.h * frame.y * ratio.unwrap_or(1.0);
            a.geometry.crop = constrain(c, ratio, frame, vec2(0.5, 0.5), !by_width);
        });
    }

    /// Crop overlay + interaction. `img_rect` shows the uncropped frame.
    pub fn crop_overlay(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect) {
        let Some(cur) = &self.current else { return };
        let crop = cur.adjustments.geometry.crop;
        let frame = self.texture.as_ref().map_or(vec2(1.0, 1.0), |t| t.size_vec2());
        let ratio = self.aspect_value(frame);
        let to_screen = |c: &CropRect| {
            Rect::from_min_size(
                img_rect.min + vec2(c.x * img_rect.width(), c.y * img_rect.height()),
                vec2(c.w * img_rect.width(), c.h * img_rect.height()),
            )
        };
        let r = to_screen(&crop);

        // Interaction.
        let hit = |p: Pos2| -> Handle {
            let near = |a: f32, b: f32| (a - b).abs() < 10.0;
            let (l, rr, t, b) = (near(p.x, r.left()), near(p.x, r.right()), near(p.y, r.top()), near(p.y, r.bottom()));
            let in_x = p.x > r.left() - 10.0 && p.x < r.right() + 10.0;
            let in_y = p.y > r.top() - 10.0 && p.y < r.bottom() + 10.0;
            match (l, rr, t, b) {
                (true, _, true, _) => Handle::Corner(false, false),
                (_, true, true, _) => Handle::Corner(true, false),
                (true, _, _, true) => Handle::Corner(false, true),
                (_, true, _, true) => Handle::Corner(true, true),
                (true, ..) if in_y => Handle::Edge(Side::Left),
                (_, true, ..) if in_y => Handle::Edge(Side::Right),
                (_, _, true, _) if in_x => Handle::Edge(Side::Top),
                (_, _, _, true) if in_x => Handle::Edge(Side::Bottom),
                _ if r.contains(p) => Handle::Move,
                _ => Handle::New,
            }
        };
        if let Some(pos) = response.hover_pos() {
            let icon = match hit(pos) {
                Handle::Move => egui::CursorIcon::Grab,
                Handle::Corner(a, b) if a == b => egui::CursorIcon::ResizeNwSe,
                Handle::Corner(..) => egui::CursorIcon::ResizeNeSw,
                Handle::Edge(Side::Left | Side::Right) => egui::CursorIcon::ResizeHorizontal,
                Handle::Edge(_) => egui::CursorIcon::ResizeVertical,
                Handle::New => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }
        let to_norm = |p: Pos2| vec2((p.x - img_rect.left()) / img_rect.width(), (p.y - img_rect.top()) / img_rect.height());
        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(tool) = &mut self.crop_tool
        {
            // Without a crop yet, dragging inside the image draws a new one.
            let h = match hit(pos) {
                Handle::Move if crop.is_full() => Handle::New,
                h => h,
            };
            tool.drag = Some((h, crop, pos));
        }
        let mut new_crop = None;
        if response.dragged()
            && let (Some(pos), Some((handle, start, origin))) =
                (response.interact_pointer_pos(), self.crop_tool.as_ref().and_then(|t| t.drag))
        {
            let n = to_norm(pos);
            let d = to_norm(pos) - to_norm(origin);
            let s = start;
            let (right, bottom) = (s.x + s.w, s.y + s.h);
            let c = match handle {
                Handle::Move => {
                    let mut c = s;
                    c.x = (s.x + d.x).clamp(0.0, 1.0 - s.w);
                    c.y = (s.y + d.y).clamp(0.0, 1.0 - s.h);
                    c
                }
                Handle::New => {
                    let o = to_norm(origin);
                    let (x0, x1) = (o.x.min(n.x).clamp(0.0, 1.0), o.x.max(n.x).clamp(0.0, 1.0));
                    let (y0, y1) = (o.y.min(n.y).clamp(0.0, 1.0), o.y.max(n.y).clamp(0.0, 1.0));
                    let anchor = vec2(if n.x < o.x { 1.0 } else { 0.0 }, if n.y < o.y { 1.0 } else { 0.0 });
                    constrain(CropRect { x: x0, y: y0, w: (x1 - x0).max(0.02), h: (y1 - y0).max(0.02) }, ratio, frame, anchor, true)
                }
                Handle::Corner(is_right, is_bottom) => {
                    let (x0, x1) = if is_right { (s.x, n.x.clamp(s.x + 0.02, 1.0)) } else { (n.x.clamp(0.0, right - 0.02), right) };
                    let (y0, y1) = if is_bottom { (s.y, n.y.clamp(s.y + 0.02, 1.0)) } else { (n.y.clamp(0.0, bottom - 0.02), bottom) };
                    let anchor = vec2(if is_right { 0.0 } else { 1.0 }, if is_bottom { 0.0 } else { 1.0 });
                    constrain(CropRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }, ratio, frame, anchor, true)
                }
                Handle::Edge(side) => {
                    let mut c = s;
                    match side {
                        Side::Left => {
                            c.x = n.x.clamp(0.0, right - 0.02);
                            c.w = right - c.x;
                        }
                        Side::Right => c.w = (n.x - s.x).clamp(0.02, 1.0 - s.x),
                        Side::Top => {
                            c.y = n.y.clamp(0.0, bottom - 0.02);
                            c.h = bottom - c.y;
                        }
                        Side::Bottom => c.h = (n.y - s.y).clamp(0.02, 1.0 - s.y),
                    }
                    let horizontal = matches!(side, Side::Left | Side::Right);
                    let anchor = match side {
                        Side::Left => vec2(1.0, 0.5),
                        Side::Right => vec2(0.0, 0.5),
                        Side::Top => vec2(0.5, 1.0),
                        Side::Bottom => vec2(0.5, 0.0),
                    };
                    constrain(c, ratio, frame, anchor, horizontal)
                }
            };
            new_crop = Some(c);
        }
        if response.drag_stopped()
            && let Some(tool) = &mut self.crop_tool
        {
            tool.drag = None;
        }
        if let Some(c) = new_crop {
            // The crop is applied when the tool closes; while active only the overlay changes.
            if let Some(cur) = &mut self.current {
                cur.adjustments.geometry.crop = c;
            }
        }

        // Drawing.
        let crop = self.current.as_ref().map(|c| c.adjustments.geometry.crop).unwrap_or_default();
        let r = to_screen(&crop);
        let painter = ui.painter_at(img_rect);
        let shade = Color32::from_black_alpha(150);
        painter.rect_filled(Rect::from_min_max(img_rect.min, pos2(img_rect.right(), r.top())), 0.0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(img_rect.left(), r.bottom()), img_rect.max), 0.0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(img_rect.left(), r.top()), pos2(r.left(), r.bottom())), 0.0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(r.right(), r.top()), pos2(img_rect.right(), r.bottom())), 0.0, shade);
        let thin = Stroke::new(1.0, Color32::from_white_alpha(90));
        for i in 1..3 {
            let t = i as f32 / 3.0;
            painter.line_segment([pos2(r.left() + r.width() * t, r.top()), pos2(r.left() + r.width() * t, r.bottom())], thin);
            painter.line_segment([pos2(r.left(), r.top() + r.height() * t), pos2(r.right(), r.top() + r.height() * t)], thin);
        }
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
        for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(9.0)), 1.0, Color32::WHITE);
        }
        let px = (crop.w * frame.x, crop.h * frame.y);
        let full = self.current.as_ref().map_or(1.0, |c| c.meta.info.width.max(c.meta.info.height) as f32 / frame.x.max(frame.y));
        painter.text(
            r.center_bottom() + vec2(0.0, -8.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.0} × {:.0}", px.0 * full, px.1 * full),
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
    }

    /// Straighten line tool: drag along a horizon or vertical edge.
    pub fn straighten_overlay(&mut self, ui: &mut Ui, response: &Response) {
        let Some(tool) = &mut self.straighten_tool else { return };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
        {
            tool.line = Some((p, p));
        }
        if response.dragged()
            && let (Some(p), Some((a, _))) = (response.interact_pointer_pos(), tool.line)
        {
            tool.line = Some((a, p));
        }
        if let Some((a, b)) = tool.line {
            let painter = ui.painter();
            painter.line_segment([a, b], Stroke::new(2.0, Color32::from_rgb(255, 210, 0)));
            if (b - a).length() > 10.0 {
                painter.text(
                    b + vec2(10.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{:+.1}°", straighten_delta(b - a)),
                    egui::FontId::proportional(13.0),
                    Color32::WHITE,
                );
            }
        }
        if response.drag_stopped()
            && let Some((a, b)) = tool.line.take()
            && (b - a).length() > 10.0
        {
            let delta = straighten_delta(b - a);
            self.straighten_tool = None;
            self.edit(|adj| adj.geometry.straighten = (adj.geometry.straighten + delta).clamp(-45.0, 45.0));
            self.status = format!("Geraderichten: {delta:+.1}°");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straighten_delta_snaps_to_nearest_axis() {
        assert!((straighten_delta(vec2(100.0, 5.0)) - 2.862).abs() < 0.01);
        assert!((straighten_delta(vec2(-100.0, 5.0)) + 2.862).abs() < 0.01);
        assert!((straighten_delta(vec2(4.0, 100.0)) + 2.29).abs() < 0.01);
    }

    #[test]
    fn constrain_keeps_ratio_and_bounds() {
        let frame = vec2(300.0, 200.0);
        let c = constrain(CropRect { x: 0.5, y: 0.5, w: 0.8, h: 0.8 }, Some(1.0), frame, vec2(0.0, 0.0), true);
        assert!((c.w * frame.x - c.h * frame.y).abs() < 1.0, "{c:?}");
        assert!(c.x + c.w <= 1.0 + 1e-6 && c.y + c.h <= 1.0 + 1e-6);
    }
}
