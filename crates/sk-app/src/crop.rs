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
    /// Crop and aspect ratio when the tool opened, for Esc.
    start: Option<(CropRect, sk_core::params::AspectRatio, bool)>,
}

/// Straighten tool: the user draws a line that should become horizontal or vertical. With
/// `guides`, it is the keystone tool instead.
#[derive(Clone, Debug, Default)]
pub struct StraightenTool {
    line: Option<(Pos2, Pos2)>,
    pub guides: Option<KeystoneGuides>,
}

/// Which lines the keystone tool places.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuideMode {
    #[default]
    Vertical,
    Horizontal,
    Both,
}

/// Keystone lines: the user lays them on edges that should become vertical or horizontal, then
/// applies them (see `sk_core::upright::solve_guides`).
#[derive(Clone, Debug)]
pub struct KeystoneGuides {
    pub mode: GuideMode,
    /// End points in source pixels of the preview (so they stay on the image content), with
    /// the axis: `true` vertical.
    pub(crate) lines: Vec<([[f32; 2]; 2], bool)>,
    /// Line and end being dragged (2: the whole line), and the last pointer position.
    drag: Option<(usize, usize, Pos2)>,
    /// A line was moved: switching to another tool applies them instead of dropping them.
    moved: bool,
}

/// Grabbing distance of a line's end points and of the line itself, in screen points.
const GRAB_END: f32 = 12.0;
const GRAB_LINE: f32 = 7.0;

impl App {
    /// The geometric map of the image as the viewer shows it (preview pixels).
    fn shown_geomap(&self) -> Option<sk_core::geometry::GeoMap> {
        let cur = self.current.as_ref()?;
        let p = &cur.preview;
        Some(sk_core::pipeline::geomap(&cur.meta, &cur.adjustments, false, p.width, p.height))
    }

    /// The keystone tool with lines for `mode`, placed over the middle of the image.
    pub fn start_keystone_guides(&mut self, mode: GuideMode) {
        self.end_viewer_tools();
        self.keystone_guide_mode = mode;
        let Some(geo) = self.shown_geomap() else { return };
        let (w, h) = (geo.out_w as f32, geo.out_h as f32);
        let src = |x: f32, y: f32| {
            let (sx, sy) = geo.source_point(x * w, y * h);
            [sx, sy]
        };
        let mut lines = Vec::new();
        if mode != GuideMode::Horizontal {
            lines.extend([0.3, 0.7].map(|x| ([src(x, 0.2), src(x, 0.8)], true)));
        }
        if mode != GuideMode::Vertical {
            lines.extend([0.3, 0.7].map(|y| ([src(0.2, y), src(0.8, y)], false)));
        }
        self.straighten_tool = Some(StraightenTool { line: None, guides: Some(KeystoneGuides { mode, lines, drag: None, moved: false }) });
    }

    /// The straighten line tool is open (not the keystone lines).
    pub fn straightening(&self) -> bool {
        self.straighten_tool.as_ref().is_some_and(|t| t.guides.is_none())
    }

    pub fn keystone_guides_active(&self) -> bool {
        self.straighten_tool.as_ref().is_some_and(|t| t.guides.is_some())
    }

    /// Enter: takes the crop or the keystone lines, whichever tool is open.
    pub fn finish_viewer_tool(&mut self) {
        if self.keystone_guides_active() {
            self.apply_keystone_guides();
        } else {
            self.finish_crop_tool();
        }
    }

    /// Straightens and corrects the keystone so that the lines become vertical or horizontal.
    pub fn apply_keystone_guides(&mut self) {
        let Some(guides) = self.straighten_tool.take().and_then(|t| t.guides) else { return };
        let Some(cur) = &self.current else { return };
        let segments: Vec<_> = guides.lines.iter().map(|([a, b], v)| (sk_core::upright::Segment { a: *a, b: *b }, *v)).collect();
        let adj = &cur.adjustments;
        let src = (cur.preview.width, cur.preview.height);
        let Some(g) = sk_core::upright::solve_guides(&segments, &adj.geometry, &adj.lens, cur.meta.lens.as_ref(), adj.rotation, src) else { return };
        self.status = format!("Trapezkorrektur: vertikal {:.0}, horizontal {:.0}, Drehung {:.1}°", g.keystone_v, g.keystone_h, g.straighten);
        self.edit(|a| a.geometry = g);
        self.commit_and_save();
    }

    /// Keystone lines in the viewer: their ends and the lines themselves can be dragged.
    fn keystone_overlay(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect) {
        let Some(geo) = self.shown_geomap() else { return };
        let Some(guides) = self.straighten_tool.as_mut().and_then(|t| t.guides.as_mut()) else { return };
        let scale = img_rect.width() / geo.out_w as f32;
        let to_screen = |p: [f32; 2]| {
            let (x, y) = geo.output_point(p[0], p[1]);
            img_rect.min + vec2(x, y) * scale
        };
        let to_source = |p: Pos2| {
            let o = (p - img_rect.min) / scale;
            let (x, y) = geo.source_point(o.x, o.y);
            [x, y]
        };
        let screen: Vec<[Pos2; 2]> = guides.lines.iter().map(|(l, _)| [to_screen(l[0]), to_screen(l[1])]).collect();
        // What the pointer would take: an end point first, else a line.
        let grab = |p: Pos2| {
            let end = screen.iter().enumerate().flat_map(|(i, l)| [(i, 0, l[0]), (i, 1, l[1])]).map(|(i, e, q)| (i, e, q.distance(p))).filter(|t| t.2 < GRAB_END).min_by(|a, b| a.2.total_cmp(&b.2));
            end.map(|(i, e, _)| (i, e)).or_else(|| {
                screen.iter().position(|l| distance_to_segment(p, l[0], l[1]) < GRAB_LINE).map(|i| (i, 2))
            })
        };
        let hover = response.hover_pos().and_then(grab);
        if response.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            guides.drag = grab(p).map(|(i, e)| (i, e, p));
        }
        if response.dragged()
            && let (Some(p), Some((i, e, last))) = (response.interact_pointer_pos(), guides.drag)
        {
            let line = &mut guides.lines[i].0;
            match e {
                2 => {
                    let d = p - last;
                    *line = [to_source(screen[i][0] + d), to_source(screen[i][1] + d)];
                }
                e => line[e] = to_source(p),
            }
            guides.drag = Some((i, e, p));
            guides.moved = true;
        }
        if response.drag_stopped() {
            guides.drag = None;
        }
        let active = guides.drag.map(|(i, e, _)| (i, e)).or(hover);
        ui.ctx().set_cursor_icon(match active {
            Some((_, 2)) if guides.drag.is_some() => egui::CursorIcon::Grabbing,
            Some((_, 2)) => egui::CursorIcon::Grab,
            Some(_) => egui::CursorIcon::Crosshair,
            None => egui::CursorIcon::Default,
        });

        let painter = ui.painter_at(img_rect);
        draw_fine_grid(&painter, img_rect);
        for (i, ((_, vertical), s)) in guides.lines.iter().zip(&screen).enumerate() {
            let color = if *vertical { crate::theme::GUIDE } else { crate::theme::GUIDE_ALT };
            let hot = active.is_some_and(|(j, _)| j == i);
            painter.line_segment([s[0], s[1]], Stroke::new(3.0, Color32::from_black_alpha(120)));
            painter.line_segment([s[0], s[1]], Stroke::new(if hot { 2.0 } else { 1.5 }, color));
            for (e, q) in s.iter().enumerate() {
                let r = if active == Some((i, e)) { 7.0 } else { 5.0 };
                painter.circle(*q, r, Color32::from_black_alpha(140), Stroke::new(1.5, color));
            }
        }

        match apply_or_cancel(ui, img_rect) {
            Some(true) => self.apply_keystone_guides(),
            Some(false) => self.cancel_viewer_tools(),
            None => {}
        }
    }
}

/// "Übernehmen" and "Abbrechen" at the bottom of the image, for the tools that are confirmed
/// (crop, keystone lines): Some(true) when applied, Some(false) when cancelled.
fn apply_or_cancel(ui: &mut Ui, img_rect: Rect) -> Option<bool> {
    let size = vec2(110.0, 28.0);
    let at = pos2(img_rect.center().x, img_rect.bottom().min(ui.clip_rect().bottom()) - 44.0);
    let apply = ui.put(Rect::from_center_size(at - vec2(size.x / 2.0 + 4.0, 0.0), size), egui::Button::new(format!("{}  Übernehmen", crate::theme::icon::CHECK)));
    let cancel = ui.put(Rect::from_center_size(at + vec2(size.x / 2.0 + 4.0, 0.0), size), egui::Button::new("Abbrechen"));
    if apply.on_hover_text(crate::commands::shortcut_of("finish-crop")).clicked() {
        Some(true)
    } else if cancel.on_hover_text("Esc").clicked() {
        Some(false)
    } else {
        None
    }
}

/// Shortest distance from `p` to the segment `a`–`b`.
fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// A fine grid over the image, for judging verticals and horizontals while correcting.
pub fn draw_fine_grid(painter: &egui::Painter, r: Rect) {
    let cell = (r.width().max(r.height()) / 16.0).max(12.0);
    let stroke = Stroke::new(1.0, Color32::from_white_alpha(45));
    let (nx, ny) = ((r.width() / cell).ceil() as i32, (r.height() / cell).ceil() as i32);
    for i in 1..nx {
        let x = r.left() + r.width() * i as f32 / nx as f32;
        painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], stroke);
    }
    for i in 1..ny {
        let y = r.top() + r.height() * i as f32 / ny as f32;
        painter.line_segment([pos2(r.left(), y), pos2(r.right(), y)], stroke);
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
        // Like the crop, keystone lines the user placed are kept when another tool starts.
        if self.straighten_tool.as_ref().and_then(|t| t.guides.as_ref()).is_some_and(|g| g.moved) {
            self.apply_keystone_guides();
        }
        self.straighten_tool = None;
        self.picking_wb = false;
        self.picking_skin = false;
        self.picking_focus = false;
        self.local_drag = Default::default();
        self.finish_crop_tool();
    }

    /// Esc: like [`Self::end_viewer_tools`], but without keeping what the open tool changed: the
    /// crop goes back to how it was when the tool opened, keystone lines are dropped.
    pub fn cancel_viewer_tools(&mut self) {
        if self.keystone_guides_active() {
            self.straighten_tool = None;
        }
        if let Some((crop, aspect, portrait)) = self.crop_tool.as_ref().and_then(|t| t.start)
            && self.current.as_ref().is_some_and(|c| {
                let g = &c.adjustments.geometry;
                (g.crop, g.aspect, g.aspect_portrait) != (crop, aspect, portrait)
            })
        {
            self.edit(|a| {
                a.geometry.crop = crop;
                a.geometry.aspect = aspect;
                a.geometry.aspect_portrait = portrait;
            });
            self.commit_and_save();
        }
        self.end_viewer_tools();
    }

    /// The straighten tool (draw a line along the horizon) on or off. Like every tool, it opens
    /// the tab with its settings.
    pub fn toggle_straighten(&mut self) {
        let on = !self.straightening();
        self.end_viewer_tools();
        if on {
            self.straighten_tool = Some(Default::default());
            self.tab = crate::app::Tab::Lens;
        }
    }

    /// The white balance pipette on or off.
    pub fn toggle_wb_picker(&mut self) {
        let on = !self.picking_wb;
        self.end_viewer_tools();
        self.picking_wb = on;
        if on {
            self.tab = crate::app::Tab::Color;
        }
    }

    /// The color pipette for `target` on, or off when it is on for it already.
    pub fn toggle_color_picker(&mut self, target: crate::develop_tools::ColorPick) {
        let on = !(self.picking_skin && self.color_pick == target);
        self.end_viewer_tools();
        self.picking_skin = on;
        self.color_pick = target;
    }

    pub fn start_crop_tool(&mut self) {
        if self.crop_tool.is_none() {
            self.end_viewer_tools();
            let start = self.current.as_ref().map(|c| {
                let g = &c.adjustments.geometry;
                (g.crop, g.aspect, g.aspect_portrait)
            });
            self.crop_tool = Some(CropTool { start, ..Default::default() });
            self.tab = crate::app::Tab::Lens;
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
            let g = sk_core::pipeline::geomap(&cur.meta, &cur.adjustments, true, cur.preview.width, cur.preview.height);
            vec2(g.out_w as f32, g.out_h as f32)
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
            // Corners grab a little further out than edges, they are the usual target.
            let near = |a: f32, b: f32, d: f32| (a - b).abs() < d;
            let corner = |x: f32, y: f32| near(p.x, x, 16.0) && near(p.y, y, 16.0);
            let (l, rr, t, b) = (near(p.x, r.left(), 10.0), near(p.x, r.right(), 10.0), near(p.y, r.top(), 10.0), near(p.y, r.bottom(), 10.0));
            let in_x = p.x > r.left() - 10.0 && p.x < r.right() + 10.0;
            let in_y = p.y > r.top() - 10.0 && p.y < r.bottom() + 10.0;
            if corner(r.left(), r.top()) {
                return Handle::Corner(false, false);
            } else if corner(r.right(), r.top()) {
                return Handle::Corner(true, false);
            } else if corner(r.left(), r.bottom()) {
                return Handle::Corner(false, true);
            } else if corner(r.right(), r.bottom()) {
                return Handle::Corner(true, true);
            }
            match (l, rr, t, b) {
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
                Handle::Move if response.dragged() => egui::CursorIcon::Grabbing,
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
        // Where the button went down: by the time a drag is recognized the pointer has moved
        // on, and a quick grab at a corner would otherwise land outside it and draw a new frame.
        if response.drag_started()
            && let Some(pos) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos())
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
            // Committed (one undo step) when the tool closes; Esc restores the crop it opened with.
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
        // Like the keystone lines: confirm or cancel right in the image.
        match apply_or_cancel(ui, img_rect) {
            Some(true) => self.finish_crop_tool(),
            Some(false) => self.cancel_viewer_tools(),
            None => {}
        }
    }

    /// Straighten line tool: drag along a horizon or vertical edge.
    pub fn straighten_overlay(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect) {
        if self.keystone_guides_active() {
            return self.keystone_overlay(ui, response, img_rect);
        }
        let Some(tool) = &mut self.straighten_tool else { return };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if response.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos())
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
            painter.line_segment([a, b], Stroke::new(2.0, crate::theme::GUIDE));
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
