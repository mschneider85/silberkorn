//! Local adjustments: layer list, mask tools (brush, eraser, gradients) and retouching
//! (heal/clone spots), including the viewer interaction.

use sk_core::mask;
use sk_core::params::{BrushStroke, Layer, LayerAdjustments, LinearGradient, Mask, RadialGradient, Spot, SpotMode};
use sk_core::pipeline::{Processor, RenderOptions};
use egui::{Color32, Pos2, Rect, Response, RichText, Stroke, Ui, Vec2, vec2};

use crate::app::App;
use crate::widgets::{section, slider_row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalTool {
    Brush,
    Eraser,
    Linear,
    Radial,
    Heal,
    Clone,
    /// Region growing from a click (Magic Brush).
    Magic,
    /// Picks the color for the color range refinement.
    ColorPick,
    /// Generative removal (see `remove.rs`).
    Remove,
}

/// Style brushes: brush presets with predefined layer adjustments.
pub const STYLE_BRUSHES: &[(&str, LayerAdjustments)] = &[
    ("Aufhellen", LayerAdjustments { exposure: 0.7, ..ZERO }),
    ("Abdunkeln", LayerAdjustments { exposure: -0.7, ..ZERO }),
    ("Wärmer", LayerAdjustments { temperature: 40.0, ..ZERO }),
    ("Kühler", LayerAdjustments { temperature: -40.0, ..ZERO }),
    ("Mehr Farbe", LayerAdjustments { saturation: 40.0, ..ZERO }),
    ("Entsättigen", LayerAdjustments { saturation: -70.0, ..ZERO }),
    ("Details betonen", LayerAdjustments { clarity: 40.0, structure: 50.0, ..ZERO }),
    ("Haut glätten", LayerAdjustments { clarity: -40.0, structure: -60.0, ..ZERO }),
    ("Lichter zurückholen", LayerAdjustments { highlights: 70.0, ..ZERO }),
    ("Tiefen aufhellen", LayerAdjustments { shadows: 70.0, ..ZERO }),
    ("Dunst entfernen", LayerAdjustments { dehaze: 60.0, ..ZERO }),
    ("Augen aufhellen", LayerAdjustments { exposure: 0.4, clarity: 20.0, saturation: 10.0, ..ZERO }),
];

const ZERO: LayerAdjustments = LayerAdjustments {
    exposure: 0.0,
    contrast: 0.0,
    brightness: 0.0,
    saturation: 0.0,
    highlights: 0.0,
    shadows: 0.0,
    clarity: 0.0,
    structure: 0.0,
    temperature: 0.0,
    tint: 0.0,
    dehaze: 0.0,
};

impl LocalTool {
    pub fn label(self) -> &'static str {
        match self {
            LocalTool::Brush => "\u{E6F0}  Pinsel (B)",
            LocalTool::Eraser => "\u{E21E}  Radierer (N)",
            LocalTool::Linear => "\u{E098}  Linear (L)",
            LocalTool::Radial => "\u{E602}  Radial (R)",
            LocalTool::Heal => "\u{E0B2}  Reparieren (H)",
            LocalTool::Clone => "\u{EA48}  Klonen (S)",
            LocalTool::Magic => "\u{E6B6}  Magic Brush (J)",
            LocalTool::ColorPick => "\u{E568}  Farbe wählen",
            LocalTool::Remove => "\u{E6F0}  Entfernen (X)",
        }
    }

    /// Icon, short name and key for the tool tiles.
    pub fn tile(self) -> (&'static str, &'static str, &'static str) {
        use crate::theme::icon;
        match self {
            LocalTool::Brush => (icon::PAINT_BRUSH, "Pinsel", "B"),
            LocalTool::Eraser => (icon::ERASER, "Radierer", "N"),
            LocalTool::Linear => (icon::GRADIENT, "Linear", "L"),
            LocalTool::Radial => (icon::CIRCLE_DASHED, "Radial", "R"),
            LocalTool::Heal => (icon::BANDAIDS, "Reparieren", "H"),
            LocalTool::Clone => (icon::STAMP, "Klonen", "S"),
            LocalTool::Magic => (icon::MAGIC_WAND, "Magic Brush", "J"),
            LocalTool::ColorPick => (icon::EYEDROPPER, "Farbe", ""),
            LocalTool::Remove => (icon::SPARKLE, "Entfernen", "X"),
        }
    }

    pub fn is_mask_tool(self) -> bool {
        matches!(self, LocalTool::Brush | LocalTool::Eraser | LocalTool::Linear | LocalTool::Radial | LocalTool::Magic)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BrushSettings {
    /// Radius in screen pixels.
    pub size: f32,
    /// 0 (soft) .. 1 (hard edge)
    pub hardness: f32,
    /// The most a stroke covers, 0..1.
    pub opacity: f32,
    /// What each dab adds (builds up within a stroke), 0..1.
    pub flow: f32,
    /// Only where the image resembles what is under the brush's center.
    pub auto_mask: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 40.0, hardness: 0.5, opacity: 1.0, flow: 1.0, auto_mask: false }
    }
}

/// Settings of each brush tool (kept between sessions).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Brushes {
    pub paint: BrushSettings,
    pub eraser: BrushSettings,
    pub remove: BrushSettings,
    /// Heal and clone (size, hardness, opacity).
    pub spot: BrushSettings,
    /// Magic Brush (size only).
    pub magic: BrushSettings,
}

impl Default for Brushes {
    fn default() -> Self {
        Self {
            paint: BrushSettings::default(),
            eraser: BrushSettings::default(),
            remove: BrushSettings { size: 30.0, hardness: 0.8, ..Default::default() },
            spot: BrushSettings { size: 25.0, hardness: 0.6, ..Default::default() },
            magic: BrushSettings { size: 30.0, ..Default::default() },
        }
    }
}

impl Brushes {
    /// Settings of a tool (the paint brush's for tools without own ones).
    pub fn of(&mut self, tool: Option<LocalTool>) -> &mut BrushSettings {
        match tool {
            Some(LocalTool::Eraser) => &mut self.eraser,
            Some(LocalTool::Remove) => &mut self.remove,
            Some(LocalTool::Heal | LocalTool::Clone) => &mut self.spot,
            Some(LocalTool::Magic) => &mut self.magic,
            _ => &mut self.paint,
        }
    }

    pub fn defaults_of(tool: Option<LocalTool>) -> BrushSettings {
        let mut d = Brushes::default();
        d.of(tool).clone()
    }
}

/// Distance (screen pixels) between recorded points of a stroke: dense, for smooth curves.
pub fn point_spacing(size: f32) -> f32 {
    (size * 0.05).max(1.5)
}

/// Controls of a brush: size, hardness, opacity, flow, automatic masking. Returns whether
/// something changed.
pub fn brush_controls(ui: &mut Ui, b: &mut BrushSettings, tool: Option<LocalTool>, flow_and_auto: bool) -> bool {
    let d = Brushes::defaults_of(tool);
    let before = b.clone();
    slider_row(ui, "Größe", &mut b.size, 2.0..=400.0, d.size, 0);
    slider_row(ui, "Härte", &mut b.hardness, 0.0..=1.0, d.hardness, 2);
    slider_row(ui, "Deckkraft", &mut b.opacity, 0.05..=1.0, d.opacity, 2);
    if flow_and_auto {
        slider_row(ui, "Fluss", &mut b.flow, 0.02..=1.0, d.flow, 2);
        ui.checkbox(&mut b.auto_mask, "Automatisch maskieren")
            .on_hover_text("Wirkt nur dort, wo das Bild der Stelle unter der Pinselmitte ähnelt (Kanten werden respektiert)");
    }
    ui.label(RichText::new("[ und ] ändern die Größe · Doppelklick auf einen Regler: Standard").small().color(crate::theme::TEXT_FAINT));
    *b != before
}

/// Maps normalized source coordinates to the screen (see `App::uv_mapper`).
pub(crate) struct UvToScreen {
    geo: sk_core::geometry::GeoMap,
    pw: f32,
    ph: f32,
    origin: Pos2,
    scale: f32,
}

impl UvToScreen {
    pub fn map(&self, uv: [f32; 2]) -> Pos2 {
        let (ox, oy) = self.geo.output_point(uv[0] * self.pw - 0.5, uv[1] * self.ph - 0.5);
        self.origin + vec2(ox, oy) * self.scale
    }
}

/// Drag state of the local tools.
#[derive(Clone, Debug, Default)]
pub enum LocalDrag {
    #[default]
    None,
    Stroke(Pos2),
    Gradient,
    /// Moving a retouch area: index and whether the source (true) or target (false) is dragged.
    Spot(usize, bool),
    /// Painting a retouch area or a Magic Brush stroke: its points so far (normalized) and the
    /// last pointer position.
    Paint(Vec<[f32; 2]>, Pos2),
}

impl App {
    fn mapping(&self) -> Option<(sk_core::geometry::GeoMap, f32, f32)> {
        let cur = self.current.as_ref()?;
        let opts = RenderOptions { ignore_crop: self.crop_tool.is_some(), ..Default::default() };
        let p = Processor::with_options(&cur.meta, &cur.adjustments, opts);
        let (pw, ph) = (cur.preview.width as f32, cur.preview.height as f32);
        Some((p.geomap(cur.preview.width, cur.preview.height), pw, ph))
    }

    /// Screen position -> normalized source coordinates.
    pub fn screen_to_uv(&self, pos: Pos2, img_rect: Rect, scale: f32) -> Option<[f32; 2]> {
        let (geo, pw, ph) = self.mapping()?;
        let o = (pos - img_rect.min) / scale;
        let (sx, sy) = geo.source_point(o.x, o.y);
        Some([(sx + 0.5) / pw, (sy + 0.5) / ph])
    }

    /// Normalized source coordinates -> screen positions. Building the geometry mapping costs
    /// about a millisecond, so it is done once per frame and used for all points.
    pub(crate) fn uv_mapper(&self, img_rect: Rect, scale: f32) -> Option<UvToScreen> {
        let (geo, pw, ph) = self.mapping()?;
        Some(UvToScreen { geo, pw, ph, origin: img_rect.min, scale })
    }

    /// Screen radius -> radius relative to the long source edge.
    pub(crate) fn radius_norm(&self, px: f32, scale: f32) -> f32 {
        let long = self.current.as_ref().map_or(1.0, |c| c.preview.width.max(c.preview.height) as f32);
        px / scale / long
    }

    pub(crate) fn radius_screen(&self, r: f32, scale: f32) -> f32 {
        let long = self.current.as_ref().map_or(1.0, |c| c.preview.width.max(c.preview.height) as f32);
        r * long * scale
    }

    /// Index of the active layer, creating one if there is none.
    fn ensure_layer(&mut self) -> Option<usize> {
        let cur = self.current.as_mut()?;
        if let Some(i) = self.active_layer
            && i < cur.adjustments.layers.len()
        {
            return Some(i);
        }
        let n = cur.adjustments.layers.len() + 1;
        cur.adjustments.layers.push(Layer { name: format!("Ebene {n}"), ..Default::default() });
        self.active_layer = Some(cur.adjustments.layers.len() - 1);
        self.active_layer
    }

    pub fn set_local_tool(&mut self, tool: LocalTool) {
        let new = if self.local_tool == Some(tool) { None } else { Some(tool) };
        self.end_viewer_tools();
        self.local_tool = new;
        if self.local_tool.is_some_and(|t| t.is_mask_tool()) {
            self.ensure_layer();
        }
        self.request_render();
    }

    /// Viewer interaction and overlay drawing for the local tools.
    pub fn local_overlay(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let Some(tool) = self.local_tool else { return };
        let hover = response.hover_pos();
        let painter = ui.painter_at(response.rect);

        match tool {
            LocalTool::Brush | LocalTool::Eraser => {
                let b = self.brushes.of(Some(tool)).clone();
                if let Some(p) = hover {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    brush_cursor(&painter, p, &b);
                    if tool == LocalTool::Eraser {
                        painter.text(p, egui::Align2::CENTER_CENTER, "−", egui::FontId::proportional(16.0), Color32::WHITE);
                    }
                }
                // From the press on, so that a click leaves a dab and a slow start still paints.
                if let Some(p) = pressed_on(ui, response)
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                    && let Some(li) = self.ensure_layer()
                {
                    let stroke = BrushStroke {
                        points: vec![uv],
                        radius: self.radius_norm(b.size, scale),
                        hardness: b.hardness,
                        flow: b.flow,
                        erase: tool == LocalTool::Eraser,
                        opacity: b.opacity,
                        auto_mask: b.auto_mask,
                        build_up: true,
                    };
                    self.edit(|a| a.layers[li].mask.strokes.push(stroke));
                    self.local_drag = LocalDrag::Stroke(p);
                }
                if response.dragged()
                    && let (Some(p), LocalDrag::Stroke(last)) = (response.interact_pointer_pos(), self.local_drag.clone())
                    && (p - last).length() > point_spacing(b.size)
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                    && let Some(li) = self.active_layer
                {
                    self.edit(|a| {
                        if let Some(s) = a.layers.get_mut(li).and_then(|l| l.mask.strokes.last_mut()) {
                            s.points.push(uv);
                        }
                    });
                    self.local_drag = LocalDrag::Stroke(p);
                }
            }
            LocalTool::Linear | LocalTool::Radial => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if response.drag_started()
                    && let Some(p) = response.interact_pointer_pos()
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                    && let Some(li) = self.ensure_layer()
                {
                    self.edit(|a| {
                        let m = &mut a.layers[li].mask;
                        if tool == LocalTool::Linear {
                            m.linear.push(LinearGradient { start: uv, end: uv });
                        } else {
                            m.radial.push(RadialGradient { center: uv, radius_x: 0.001, radius_y: 0.001, feather: 0.5, invert: false });
                        }
                    });
                    self.local_drag = LocalDrag::Gradient;
                }
                if response.dragged()
                    && matches!(self.local_drag, LocalDrag::Gradient)
                    && let Some(p) = response.interact_pointer_pos()
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                    && let Some(li) = self.active_layer
                {
                    let aspect = self.current.as_ref().map_or(1.0, |c| c.preview.width as f32 / c.preview.height as f32);
                    self.edit(|a| {
                        let m = &mut a.layers[li].mask;
                        if tool == LocalTool::Linear {
                            if let Some(g) = m.linear.last_mut() {
                                g.end = uv;
                            }
                        } else if let Some(g) = m.radial.last_mut() {
                            // Radii relative to the long edge, from the normalized offsets.
                            let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
                            let (dx, dy) = ((uv[0] - g.center[0]) * kx, (uv[1] - g.center[1]) * ky);
                            let r = (dx * dx + dy * dy).sqrt().max(0.002);
                            g.radius_x = r;
                            g.radius_y = r;
                        }
                    });
                }
                self.draw_gradients(&painter, img_rect, scale);
            }
            LocalTool::Heal | LocalTool::Clone => {
                self.spot_interaction(ui, response, img_rect, scale, tool);
            }
            LocalTool::Remove => self.remove_interaction(ui, response, img_rect, scale),
            LocalTool::Magic => self.magic_interaction(ui, response, img_rect, scale),
            LocalTool::ColorPick => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if let Some(p) = pressed_on(ui, response)
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                {
                    self.pick_color_range(uv);
                }
            }
        }
        if response.drag_stopped() || ui.input(|i| i.pointer.primary_released()) {
            self.local_drag = LocalDrag::None;
        }
    }

    fn draw_gradients(&self, painter: &egui::Painter, img_rect: Rect, scale: f32) {
        let Some(cur) = &self.current else { return };
        let Some(layer) = self.active_layer.and_then(|i| cur.adjustments.layers.get(i)) else { return };
        let Some(map) = self.uv_mapper(img_rect, scale) else { return };
        let stroke = Stroke::new(1.5, Color32::WHITE);
        for g in &layer.mask.linear {
            let (a, b) = (map.map(g.start), map.map(g.end));
            let d = b - a;
            let n = vec2(-d.y, d.x).normalized() * 2000.0;
            painter.line_segment([a - n, a + n], stroke);
            painter.line_segment([b - n, b + n], Stroke::new(1.0, Color32::from_white_alpha(120)));
            painter.line_segment([a, b], Stroke::new(1.0, Color32::from_rgb(255, 210, 0)));
            painter.circle_filled(a, 4.0, Color32::WHITE);
        }
        for g in &layer.mask.radial {
            let c = map.map(g.center);
            let r = self.radius_screen(g.radius_x, scale);
            painter.circle_stroke(c, r, stroke);
            painter.circle_stroke(c, r * (1.0 - g.feather), Stroke::new(1.0, Color32::from_white_alpha(120)));
            painter.circle_filled(c, 3.0, Color32::WHITE);
        }
    }

    /// Extends the stroke being painted (`LocalDrag::Paint`) and returns its points in
    /// screen coordinates for the preview.
    fn extend_paint(&mut self, response: &Response, img_rect: Rect, scale: f32, spacing: f32) -> Vec<Pos2> {
        let LocalDrag::Paint(mut pts, last) = self.local_drag.clone() else { return Vec::new() };
        if let Some(p) = response.interact_pointer_pos()
            && (p - last).length() > spacing
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            pts.push(uv);
            self.local_drag = LocalDrag::Paint(pts.clone(), p);
        }
        let Some(map) = self.uv_mapper(img_rect, scale) else { return Vec::new() };
        pts.iter().map(|uv| map.map(*uv)).collect()
    }

    /// Takes the finished stroke when the mouse button is released.
    fn finished_paint(&mut self, ui: &Ui) -> Option<Vec<[f32; 2]>> {
        if !ui.input(|i| i.pointer.primary_released()) {
            return None;
        }
        match std::mem::take(&mut self.local_drag) {
            LocalDrag::Paint(pts, _) => Some(pts),
            other => {
                self.local_drag = other;
                None
            }
        }
    }

    /// Heal and clone, painted like a brush: the stroke becomes a retouch area whose source is
    /// chosen automatically. The pins move an area (white) or its source (yellow).
    fn spot_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32, tool: LocalTool) {
        let Some(cur) = &self.current else { return };
        let spots = cur.adjustments.retouch.clone();
        let painter = ui.painter_at(response.rect);
        let Some(map) = self.uv_mapper(img_rect, scale) else { return };
        // Screen geometry of each area: its points, the offset to the source and the radius.
        let shown: Vec<Option<(Vec<Pos2>, Vec2, f32)>> = spots
            .iter()
            .map(|s| {
                let pts: Vec<Pos2> = std::iter::once(&s.target).chain(&s.path).map(|p| map.map(*p)).collect();
                let offset = map.map(s.source) - pts[0];
                Some((pts, offset, self.radius_screen(s.radius, scale)))
            })
            .collect();
        let selected = self.selected_spot;
        let pin_hit = |p: Pos2| -> Option<(usize, bool)> {
            for (i, area) in shown.iter().enumerate().rev() {
                let Some((pts, offset, _)) = area else { continue };
                if selected == Some(i) && (p - (pts[0] + *offset)).length() < PIN_HIT {
                    return Some((i, true));
                }
                if (p - pts[0]).length() < PIN_HIT {
                    return Some((i, false));
                }
            }
            None
        };
        // Over a pin the cursor shows that it can be grabbed; elsewhere the brush outline.
        let moving = matches!(self.local_drag, LocalDrag::Spot(..));
        let hovered_pin = response.hover_pos().and_then(|p| pin_hit(p));
        for (i, area) in shown.iter().enumerate() {
            let Some((pts, offset, r)) = area else { continue };
            let sel = selected == Some(i);
            paint_area(&painter, pts, *r, if sel { Color32::WHITE } else { Color32::from_white_alpha(150) });
            if sel {
                let src: Vec<Pos2> = pts.iter().map(|p| *p + *offset).collect();
                paint_area(&painter, &src, *r, Color32::from_rgb(255, 210, 0));
                painter.line_segment([pts[0], pts[0] + *offset], Stroke::new(1.0, Color32::from_rgb(255, 210, 0)));
                pin(&painter, pts[0] + *offset, Color32::from_rgb(255, 210, 0), hovered_pin == Some((i, true)));
            }
            pin(&painter, pts[0], if sel { Color32::WHITE } else { Color32::from_white_alpha(170) }, hovered_pin == Some((i, false)));
        }
        let b = self.brushes.spot.clone();
        if moving {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovered_pin.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if let Some(p) = response.hover_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            brush_cursor(&painter, p, &b);
        }
        if let Some(p) = pressed_on(ui, response) {
            if let Some((i, is_source)) = pin_hit(p) {
                self.selected_spot = Some(i);
                self.local_drag = LocalDrag::Spot(i, is_source);
            } else if let Some(uv) = self.screen_to_uv(p, img_rect, scale) {
                self.local_drag = LocalDrag::Paint(vec![uv], p);
            }
        }
        if response.dragged()
            && let LocalDrag::Spot(i, is_source) = self.local_drag
            && let Some(p) = response.interact_pointer_pos()
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            self.edit(|a| {
                if let Some(s) = a.retouch.get_mut(i) {
                    if is_source {
                        s.source = uv;
                    } else {
                        // The whole area moves; it keeps taking from the same relative place.
                        let d = [uv[0] - s.target[0], uv[1] - s.target[1]];
                        for q in std::iter::once(&mut s.target).chain(s.path.iter_mut()).chain(std::iter::once(&mut s.source)) {
                            *q = [q[0] + d[0], q[1] + d[1]];
                        }
                    }
                }
            });
        }
        let preview = self.extend_paint(response, img_rect, scale, (b.size * 0.3).max(3.0));
        if !preview.is_empty() {
            paint_area(&painter, &preview, b.size, Color32::WHITE);
        }
        if let Some(pts) = self.finished_paint(ui) {
            let radius = self.radius_norm(b.size, scale);
            let source = self.current.as_ref().map_or(pts[0], |c| mask::auto_source_area(&c.preview, &pts, radius));
            let mode = if tool == LocalTool::Heal { SpotMode::Heal } else { SpotMode::Clone };
            let (feather, opacity) = (1.0 - b.hardness.clamp(0.0, 1.0), b.opacity);
            let spot = Spot { mode, target: pts[0], source, path: pts[1..].to_vec(), radius, feather, opacity };
            self.edit(|a| a.retouch.push(spot));
            self.selected_spot = self.current.as_ref().map(|c| c.adjustments.retouch.len() - 1);
        }
    }

    /// Magic Brush: paint over what should be selected; the region of similar, connected
    /// colors around the stroke becomes the layer's selection.
    fn magic_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let painter = ui.painter_at(response.rect);
        let b = self.brushes.magic.clone();
        if let Some(p) = response.hover_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            brush_cursor(&painter, p, &b);
        }
        if let Some(p) = pressed_on(ui, response)
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            self.local_drag = LocalDrag::Paint(vec![uv], p);
        }
        let preview = self.extend_paint(response, img_rect, scale, (b.size * 0.3).max(3.0));
        if !preview.is_empty() {
            paint_area(&painter, &preview, b.size, Color32::from_rgb(255, 150, 60));
        }
        if let Some(pts) = self.finished_paint(ui) {
            // Seeds along the stroke and across its width, so a wide brush takes in more tones.
            let (w, h) = self.current.as_ref().map_or((1.0, 1.0), |c| (c.preview.width as f32, c.preview.height as f32));
            let r = self.radius_norm(b.size, scale) * 0.6 * w.max(h);
            let (dx, dy) = (r / w, r / h);
            let seeds: Vec<[f32; 2]> = pts
                .iter()
                .flat_map(|p| [*p, [p[0] - dx, p[1]], [p[0] + dx, p[1]], [p[0], p[1] - dy], [p[0], p[1] + dy]])
                .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
                .collect();
            self.request_ai(crate::workers::AiKind::Magic { seeds, tolerance: self.magic_tolerance });
        }
    }

    /// Starts a content-based mask computation for the active layer.
    pub fn request_ai(&mut self, kind: crate::workers::AiKind) {
        let Some(layer) = self.ensure_layer() else { return };
        let Some(cur) = &self.current else { return };
        self.ai_busy = true;
        self.status = "Berechne Maske…".into();
        self.workers.ai_mask(crate::workers::AiJob {
            generation: self.generation(),
            layer,
            kind,
            meta: cur.meta.clone(),
            preview: cur.preview.clone(),
            adjustments: cur.adjustments.clone(),
        });
    }

    /// Sets the color range of the active layer from the color at `uv`.
    fn pick_color_range(&mut self, uv: [f32; 2]) {
        let Some(li) = self.active_layer else { return };
        let Some(cur) = &self.current else { return };
        let p = &cur.preview;
        let (sx, sy) = ((uv[0] * p.width as f32) as usize, (uv[1] * p.height as f32) as usize);
        // Average a 5x5 neighborhood of camera values and develop it with the background settings.
        let mut acc = [0.0f32; 3];
        let mut n = 0.0;
        for y in sy.saturating_sub(2)..(sy + 3).min(p.height) {
            for x in sx.saturating_sub(2)..(sx + 3).min(p.width) {
                let v = p.data[y * p.width + x];
                acc = [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2]];
                n += 1.0;
            }
        }
        if n == 0.0 {
            return;
        }
        let px = sk_core::image::LinearImage::new(3, 3, vec![acc.map(|v| v / n); 9]);
        let mut adj = cur.adjustments.clone();
        adj.geometry = Default::default();
        adj.lens = Default::default();
        adj.rotation = 0;
        adj.layers.clear();
        adj.retouch.clear();
        adj.detail.sharpen_amount = 0.0;
        adj.detail.nr_color = 0.0;
        adj.detail.nr_luma = 0.0;
        let r = Processor::new(&cur.meta, &adj).render(&px, None, None);
        let rgb = [r.rgba[16], r.rgba[17], r.rgba[18]].map(|v| sk_core::color::srgb_decode(v as f32 / 255.0));
        // sRGB -> working, then Oklab hue (the same space the pipeline uses).
        let to_working = sk_core::color::mat_inv(&sk_core::color::OutputSpace::Srgb.from_working()).unwrap_or(sk_core::color::IDENTITY);
        let [_, a, b] = sk_core::color::Oklab::default().from_linear(sk_core::color::mat_vec(&to_working, rgb));
        let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
        let chroma = (a * a + b * b).sqrt();
        self.edit(|adj| {
            if let Some(l) = adj.layers.get_mut(li) {
                let mut cr = l.mask.color_range.unwrap_or_default();
                cr.hue = hue;
                cr.min_chroma = (chroma * 0.4).clamp(0.01, 0.1);
                l.mask.color_range = Some(cr);
                if l.mask.is_empty() {
                    l.mask.fill = true;
                }
            }
        });
        self.local_tool = None;
        self.status = format!("Farbbereich: Farbton {hue:.0}°");
    }

    /// Creates a new layer with a style brush preset and activates the brush.
    fn add_style_brush(&mut self, name: &str, adj: LayerAdjustments) {
        if self.current.is_none() {
            return;
        }
        self.end_viewer_tools();
        let Some(cur) = self.current.as_mut() else { return };
        cur.adjustments.layers.push(Layer { name: name.to_string(), adjustments: adj, ..Default::default() });
        self.active_layer = Some(cur.adjustments.layers.len() - 1);
        self.local_tool = Some(LocalTool::Brush);
        self.request_render();
    }

    /// Luma range, color range and AI mask controls of a layer.
    fn mask_refinement_ui(&mut self, ui: &mut Ui, li: usize, changed: &mut bool) {
        let mut ai_action = None;
        let mut pick = false;
        let busy = self.ai_busy;
        let model_missing = self.model_missing;
        let downloading = self.model_downloading;
        let tol = &mut self.magic_tolerance;
        let tool = self.local_tool;
        let Some(cur) = self.current.as_mut() else { return };
        let Some(layer) = cur.adjustments.layers.get_mut(li) else { return };
        section(ui, "Auswahl aus Bildinhalt", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("\u{E4C2}  Motiv auswählen")).clicked() {
                    ai_action = Some(1);
                }
                if ui.add_enabled(!busy, egui::Button::new("\u{E1AA}  Himmel auswählen")).clicked() {
                    ai_action = Some(2);
                }
                if ui.selectable_label(tool == Some(LocalTool::Magic), LocalTool::Magic.label()).clicked() {
                    ai_action = Some(3);
                }
                if busy {
                    ui.spinner();
                }
            });
            slider_row(ui, "Toleranz", tol, 0.0..=1.0, 0.3, 2);
            if layer.mask.ai.is_some() && ui.button("Auswahl entfernen").clicked() {
                layer.mask.ai = None;
                *changed = true;
            }
            if model_missing {
                ui.horizontal(|ui| {
                    if downloading {
                        ui.spinner();
                        ui.label("Lade Modell…");
                    } else if ui.button("KI-Modell herunterladen (4,6 MB)").on_hover_text("U²-Net-p, Apache-2.0").clicked() {
                        ai_action = Some(4);
                    }
                });
            }
        });
        section(ui, "Maske verfeinern", None, |ui| {
            let m = &mut layer.mask;
            let mut luma_on = m.luma_range.is_some();
            if ui.checkbox(&mut luma_on, "Luma Range").changed() {
                m.luma_range = luma_on.then(Default::default);
                *changed = true;
            }
            if let Some(r) = &mut m.luma_range {
                *changed |= slider_row(ui, "Von", &mut r.low, 0.0..=1.0, 0.0, 2);
                *changed |= slider_row(ui, "Bis", &mut r.high, 0.0..=1.0, 1.0, 2);
                *changed |= slider_row(ui, "Weichheit", &mut r.falloff, 0.0..=0.5, 0.1, 2);
                *changed |= ui.checkbox(&mut r.invert, "Bereich umkehren").changed();
            }
            let mut color_on = m.color_range.is_some();
            if ui.checkbox(&mut color_on, "Farbbereich").changed() {
                m.color_range = color_on.then(Default::default);
                *changed = true;
            }
            if let Some(c) = &mut m.color_range {
                if ui.selectable_label(tool == Some(LocalTool::ColorPick), LocalTool::ColorPick.label()).clicked() {
                    pick = true;
                }
                *changed |= slider_row(ui, "Farbton", &mut c.hue, 0.0..=360.0, 30.0, 0);
                *changed |= slider_row(ui, "Breite", &mut c.width, 2.0..=90.0, 25.0, 0);
                *changed |= slider_row(ui, "Weichheit", &mut c.falloff, 0.0..=60.0, 15.0, 0);
                *changed |= slider_row(ui, "Min. Chroma", &mut c.min_chroma, 0.0..=0.2, 0.03, 3);
            }
        });
        match ai_action {
            Some(1) => self.request_ai(crate::workers::AiKind::Subject),
            Some(2) => self.request_ai(crate::workers::AiKind::Sky),
            Some(3) => self.set_local_tool(LocalTool::Magic),
            Some(4) => {
                self.model_downloading = true;
                self.workers.download_model();
            }
            _ => {}
        }
        if pick {
            self.set_local_tool(LocalTool::ColorPick);
        }
    }

    /// List of the heal and clone areas: select one to edit it (also in the viewer), change
    /// its kind, opacity and softness, find a new source or delete it.
    fn retouch_section(&mut self, ui: &mut Ui) {
        enum Act {
            Select(usize),
            Delete,
            NewSource,
            Clear,
        }
        let mut act = None;
        let mut changed = false;
        let selected = self.selected_spot;
        let Some(cur) = self.current.as_mut() else { return };
        let spots = &mut cur.adjustments.retouch;
        section(ui, "Reparieren & Klonen", None, |ui| {
            if spots.is_empty() {
                crate::widgets::hint(ui, "Noch keine. Mit Reparieren (H) oder Klonen (S) über eine Stelle malen.");
                return;
            }
            for (i, s) in spots.iter().enumerate() {
                let (ic, kind) = match s.mode {
                    SpotMode::Heal => (crate::theme::icon::BANDAIDS, "Reparieren"),
                    SpotMode::Clone => (crate::theme::icon::STAMP, "Klonen"),
                };
                let text = format!("{ic}  {kind} {}", i + 1);
                let w = ui.available_width();
                if ui.add_sized([w, 24.0], egui::Button::selectable(selected == Some(i), text)).clicked() {
                    act = Some(Act::Select(i));
                }
            }
            if let Some(s) = selected.and_then(|i| spots.get_mut(i)) {
                ui.add_space(6.0);
                crate::widgets::field(ui, "Art", |ui| {
                    changed |= ui.selectable_value(&mut s.mode, SpotMode::Heal, "Reparieren").changed();
                    changed |= ui.selectable_value(&mut s.mode, SpotMode::Clone, "Klonen").changed();
                });
                let mut opacity = s.opacity * 100.0;
                if slider_row(ui, "Deckkraft", &mut opacity, 0.0..=100.0, 100.0, 0) {
                    s.opacity = opacity / 100.0;
                    changed = true;
                }
                let mut feather = s.feather * 100.0;
                if slider_row(ui, "Weichheit", &mut feather, 0.0..=100.0, 40.0, 0) {
                    s.feather = feather / 100.0;
                    changed = true;
                }
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Neue Quelle", crate::theme::icon::MAGIC_WAND)).on_hover_text("Quelle erneut automatisch wählen").clicked() {
                        act = Some(Act::NewSource);
                    }
                    if ui.button(format!("{}  Löschen", crate::theme::icon::TRASH)).on_hover_text("Entf").clicked() {
                        act = Some(Act::Delete);
                    }
                });
            }
            ui.add_space(4.0);
            if ui.button(format!("{}  Alle löschen", crate::theme::icon::TRASH)).clicked() {
                act = Some(Act::Clear);
            }
        });
        if changed {
            self.request_render();
        }
        match act {
            Some(Act::Select(i)) => {
                self.selected_spot = Some(i);
                // The pins are shown with the heal and clone tools.
                if !matches!(self.local_tool, Some(LocalTool::Heal | LocalTool::Clone)) {
                    let heal = self.current.as_ref().and_then(|c| c.adjustments.retouch.get(i)).is_some_and(|s| s.mode == SpotMode::Heal);
                    self.set_local_tool(if heal { LocalTool::Heal } else { LocalTool::Clone });
                }
            }
            Some(Act::Delete) => self.delete_selected_spot(),
            Some(Act::NewSource) => {
                let Some(i) = self.selected_spot else { return };
                let Some(cur) = self.current.as_ref() else { return };
                let Some(s) = cur.adjustments.retouch.get(i) else { return };
                let pts: Vec<[f32; 2]> = std::iter::once(s.target).chain(s.path.iter().copied()).collect();
                let source = mask::auto_source_area(&cur.preview, &pts, s.radius);
                self.edit(|a| a.retouch[i].source = source);
            }
            Some(Act::Clear) => {
                self.selected_spot = None;
                self.edit(|a| a.retouch.clear());
            }
            None => {}
        }
    }

    pub fn delete_selected_spot(&mut self) {
        if let Some(i) = self.selected_spot.take() {
            self.edit(|a| {
                if i < a.retouch.len() {
                    a.retouch.remove(i);
                }
            });
        }
    }

    // ---------------------------------------------------------------- tab

    pub fn layers_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        let mut tool_click = None;
        let mut brushes_changed = false;
        section(ui, "Werkzeuge", None, |ui| {
            let tools = [
                [LocalTool::Brush, LocalTool::Eraser, LocalTool::Linear, LocalTool::Radial],
                [LocalTool::Magic, LocalTool::Heal, LocalTool::Clone, LocalTool::Remove],
            ];
            let gap = 4.0;
            let w = ((ui.available_width() - 3.0 * gap) / 4.0).floor();
            for row in tools {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for t in row {
                        let (ic, name, key) = t.tile();
                        let hover = if key.is_empty() { name.to_string() } else { format!("{name} ({key})") };
                        if crate::widgets::tool_tile(ui, ic, name, key, self.local_tool == Some(t), w).on_hover_text(hover).clicked() {
                            tool_click = Some(t);
                        }
                    }
                });
            }
            ui.add_space(4.0);
            match self.local_tool {
                Some(t @ (LocalTool::Brush | LocalTool::Eraser)) => {
                    brushes_changed |= brush_controls(ui, self.brushes.of(Some(t)), Some(t), true);
                }
                Some(t @ (LocalTool::Heal | LocalTool::Clone)) => {
                    brushes_changed |= brush_controls(ui, self.brushes.of(Some(t)), Some(t), false);
                    crate::widgets::hint(ui, "Über die Stelle malen, die Quelle wird automatisch gewählt. Pins ziehen: Bereich (weiß) oder Quelle (gelb) verschieben · Entf: löschen");
                }
                Some(LocalTool::Magic) => {
                    let d = Brushes::defaults_of(Some(LocalTool::Magic));
                    brushes_changed |= slider_row(ui, "Größe", &mut self.brushes.magic.size, 2.0..=400.0, d.size, 0);
                    slider_row(ui, "Toleranz", &mut self.magic_tolerance, 0.0..=1.0, 0.3, 2);
                    crate::widgets::hint(ui, "Über den Bereich malen: ähnliche, zusammenhängende Farben werden ausgewählt. Weitere Striche erweitern die Auswahl.");
                }
                _ => {}
            }
        });
        if let Some(t) = tool_click {
            self.set_local_tool(t);
        }
        if brushes_changed {
            self.brushes_changed_at = Some(std::time::Instant::now());
        }
        self.remove_section(ui);

        let mut style = None;
        crate::widgets::section_with(ui, "Style Brushes", false, None, |ui| {
            crate::widgets::hint(ui, "Neue Ebene mit fertigen Einstellungen, dann malen");
            ui.horizontal_wrapped(|ui| {
                for (name, adj) in STYLE_BRUSHES {
                    if ui.button(*name).clicked() {
                        style = Some((*name, *adj));
                    }
                }
            });
        });
        if let Some((name, adj)) = style {
            self.add_style_brush(name, adj);
        }

        // Layer list.
        let mut action: Option<LayerAction> = None;
        let Some(cur) = self.current.as_mut() else { return };
        let layers = &mut cur.adjustments.layers;
        let mut changed = false;
        section(ui, "Ebenen", None, |ui| {
            if ui.selectable_label(self.active_layer.is_none(), "Hintergrund").clicked() {
                action = Some(LayerAction::Select(None));
            }
            for (i, l) in layers.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut l.enabled, "").on_hover_text("Ebene ein/aus").changed();
                    let edited = if l.mask.is_empty() { " (leere Maske)" } else { "" };
                    if ui.selectable_label(self.active_layer == Some(i), format!("{}{edited}", l.name)).clicked() {
                        action = Some(LayerAction::Select(Some(i)));
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.button("+ Ebene").clicked() {
                    action = Some(LayerAction::Add(false));
                }
                if ui.button("+ Gefüllt").on_hover_text("Ebene mit gefüllter Maske").clicked() {
                    action = Some(LayerAction::Add(true));
                }
                if self.active_layer.is_some() && ui.button("\u{E4A6}  Löschen").clicked() {
                    action = Some(LayerAction::Delete);
                }
            });
        });

        if let Some(li) = self.active_layer.filter(|i| *i < layers.len()) {
            let layer = &mut layers[li];
            let show_mask = &mut self.show_mask;
            section(ui, "Ebene", None, |ui| {
                crate::widgets::field(ui, "Name", |ui| ui.text_edit_singleline(&mut layer.name));
                changed |= slider_row(ui, "Deckkraft", &mut layer.opacity, 0.0..=100.0, 100.0, 0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Füllen").clicked() {
                        layer.mask = Mask { fill: true, ..Default::default() };
                        changed = true;
                    }
                    if ui.button("Leeren").clicked() {
                        layer.mask = Mask::default();
                        changed = true;
                    }
                    changed |= ui.checkbox(&mut layer.mask.invert, "Invertieren").changed();
                    if ui.selectable_label(*show_mask, "Maske anzeigen (M)").clicked() {
                        *show_mask = !*show_mask;
                        changed = true;
                    }
                });
            });
            let mut reset = false;
            section(ui, "Ebenen-Anpassungen", Some(&mut || reset = true), |ui| {
                let a = &mut layer.adjustments;
                changed |= slider_row(ui, "Belichtung", &mut a.exposure, -4.0..=4.0, 0.0, 2);
                changed |= slider_row(ui, "Kontrast", &mut a.contrast, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Helligkeit", &mut a.brightness, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Sättigung", &mut a.saturation, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Lichter", &mut a.highlights, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Tiefen", &mut a.shadows, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Klarheit", &mut a.clarity, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Struktur", &mut a.structure, -100.0..=100.0, 0.0, 0);
                changed |= crate::widgets::slider_row_with(ui, "Temperatur", &mut a.temperature, -100.0..=100.0, 0.0, 0, crate::widgets::SliderOpts { rail: Some(crate::widgets::RAIL_KELVIN), ..Default::default() });
                changed |= crate::widgets::slider_row_with(ui, "Tint", &mut a.tint, -100.0..=100.0, 0.0, 0, crate::widgets::SliderOpts { rail: Some(crate::widgets::RAIL_TINT), ..Default::default() });
                changed |= slider_row(ui, "Dunst", &mut a.dehaze, -100.0..=100.0, 0.0, 0);
            });
            if reset {
                layer.adjustments = LayerAdjustments::default();
                changed = true;
            }
            self.mask_refinement_ui(ui, li, &mut changed);
        }

        if changed {
            self.request_render();
        }
        self.retouch_section(ui);
        match action {
            Some(LayerAction::Select(i)) => {
                self.active_layer = i;
                self.request_render();
            }
            Some(LayerAction::Add(fill)) => {
                if let Some(cur) = self.current.as_mut() {
                    let n = cur.adjustments.layers.len() + 1;
                    let mask = Mask { fill, ..Default::default() };
                    cur.adjustments.layers.push(Layer { name: format!("Ebene {n}"), mask, ..Default::default() });
                    self.active_layer = Some(cur.adjustments.layers.len() - 1);
                    self.request_render();
                }
            }
            Some(LayerAction::Delete) => {
                if let Some(i) = self.active_layer.take() {
                    self.edit(|a| {
                        if i < a.layers.len() {
                            a.layers.remove(i);
                        }
                    });
                }
            }
            None => {}
        }
    }
}

enum LayerAction {
    Select(Option<usize>),
    Add(bool),
    Delete,
}

/// Brush outline: the size, and inside it where the hard core ends.
pub fn brush_cursor(painter: &egui::Painter, p: Pos2, b: &BrushSettings) {
    painter.circle_stroke(p, b.size, Stroke::new(2.5, Color32::from_black_alpha(120)));
    painter.circle_stroke(p, b.size, Stroke::new(1.2, Color32::WHITE));
    if b.hardness < 0.97 {
        painter.circle_stroke(p, b.size * b.hardness.max(0.05), Stroke::new(1.0, Color32::from_white_alpha(110)));
    }
}

/// Pointer distance (screen points) within which a pin of a retouch area is grabbed.
const PIN_HIT: f32 = 9.0;

/// Where the primary button went down on the viewer this frame. Tools act from the press on
/// instead of on `clicked()`, which egui drops when the mouse moves a little or the button is
/// held a moment (it then counts as a drag).
fn pressed_on(ui: &Ui, response: &Response) -> Option<Pos2> {
    if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) { response.interact_pointer_pos() } else { None }
}

/// Outline of a painted area: the edge of the round-capped band of radius `r` along `pts`
/// (a circle for one point). Only the outer edge is drawn, so strokes that go back and forth
/// stay one clean shape; a dark halo keeps it visible on any image.
fn paint_area(painter: &egui::Painter, pts: &[Pos2], r: f32, color: Color32) {
    let pieces = cached_outline(painter.ctx(), pts, r);
    for s in pieces.iter() {
        painter.line_segment(*s, Stroke::new(3.0, Color32::from_black_alpha(110)));
    }
    for s in pieces.iter() {
        painter.line_segment(*s, Stroke::new(1.3, color));
    }
}

/// Outlines computed in recent frames, by their points and radius: while nothing is edited,
/// zoomed or panned, the areas look the same every frame.
#[derive(Clone, Default)]
struct OutlineCache(std::collections::HashMap<u64, (u64, std::sync::Arc<Vec<[Pos2; 2]>>)>);

fn cached_outline(ctx: &egui::Context, pts: &[Pos2], r: f32) -> std::sync::Arc<Vec<[Pos2; 2]>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    r.to_bits().hash(&mut h);
    for p in pts {
        (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
    }
    let key = h.finish();
    let frame = ctx.cumulative_pass_nr();
    let id = egui::Id::new("area_outlines");
    if let Some(hit) = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<OutlineCache>(id);
        cache.0.get_mut(&key).map(|e| {
            e.0 = frame;
            e.1.clone()
        })
    }) {
        return hit;
    }
    let outline = std::sync::Arc::new(area_outline(pts, r));
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<OutlineCache>(id);
        // Drop what was not drawn in the last frames.
        cache.0.retain(|_, e| frame.saturating_sub(e.0) <= 2);
        cache.0.insert(key, (frame, outline.clone()));
    });
    outline
}

/// The pieces of the outline of [`paint_area`].
fn area_outline(pts: &[Pos2], r: f32) -> Vec<[Pos2; 2]> {
    if pts.is_empty() || r < 0.5 {
        return Vec::new();
    }
    // Long strokes: every few points is enough for the outline (they lie 0.3 r apart).
    let step = pts.len().div_ceil(96);
    let mut pts: Vec<Pos2> = pts.iter().step_by(step).copied().collect();
    if step > 1 {
        pts.extend(pts.last().copied());
    }
    let pts = &pts;
    // Segments (a single point counts as one) with their bounding boxes grown by the radius:
    // a sample only needs the few segments whose box contains it.
    let segments: Vec<(Pos2, Pos2, Rect)> = if pts.len() == 1 {
        vec![(pts[0], pts[0], Rect::from_center_size(pts[0], Vec2::splat(2.0 * r)))]
    } else {
        pts.windows(2).map(|w| (w[0], w[1], Rect::from_two_pos(w[0], w[1]).expand(r))).collect()
    };
    let inside = |p: Pos2| {
        let limit = (r - 0.75).powi(2);
        segments.iter().any(|(a, b, bounds)| bounds.contains(p) && (p - closest_on_segment(p, *a, *b)).length_sq() < limit)
    };
    let mut pieces: Vec<[Pos2; 2]> = Vec::new();
    let mut trace = |samples: &[Pos2]| {
        let mut prev_out = None;
        for &q in samples {
            let out = !inside(q);
            if out && let Some(Some(prev)) = prev_out {
                pieces.push([prev, q]);
            }
            prev_out = Some(out.then_some(q));
        }
    };
    // Candidate edges: a circle around each point and both sides of each segment; the parts
    // inside the area are dropped.
    let n = ((std::f32::consts::TAU * r) / 3.0).clamp(16.0, 128.0) as usize;
    for &c in pts {
        let circle: Vec<Pos2> = (0..=n).map(|k| c + Vec2::angled(k as f32 / n as f32 * std::f32::consts::TAU) * r).collect();
        trace(&circle);
    }
    for w in pts.windows(2) {
        let d = w[1] - w[0];
        let len = d.length();
        if len < 0.5 {
            continue;
        }
        let normal = vec2(-d.y, d.x) / len * r;
        let m = (len / 3.0).ceil().max(1.0) as usize;
        for side in [normal, -normal] {
            let line: Vec<Pos2> = (0..=m).map(|k| w[0] + side + d * (k as f32 / m as f32)).collect();
            trace(&line);
        }
    }
    pieces
}

/// Closest point to `p` on the segment `a`–`b`.
fn closest_on_segment(p: Pos2, a: Pos2, b: Pos2) -> Pos2 {
    let d = b - a;
    let len2 = d.length_sq();
    if len2 < 1e-6 {
        return a;
    }
    a + d * ((p - a).dot(d) / len2).clamp(0.0, 1.0)
}

/// Pin of a retouch area (grab it to move the area or its source); larger under the mouse.
fn pin(painter: &egui::Painter, p: Pos2, color: Color32, hovered: bool) {
    let r = if hovered { 6.0 } else { 4.0 };
    painter.circle_filled(p, r + 1.5, Color32::from_black_alpha(120));
    painter.circle_filled(p, r, color);
    if hovered {
        painter.circle_stroke(p, r + 3.5, Stroke::new(1.0, color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The outline of a stroke that goes back and forth is only its outer edge: no piece of it
    /// lies inside the area (the old filled band drew spikes and overlaps there).
    #[test]
    fn area_outline_is_only_the_outer_edge() {
        let zigzag: Vec<Pos2> = (0..20).map(|i| egui::pos2(100.0 + (i % 2) as f32 * 30.0, 100.0 + i as f32 * 3.0)).collect();
        let pieces = area_outline(&zigzag, 20.0);
        assert!(!pieces.is_empty());
        for [a, b] in pieces {
            let mid = a.lerp(b, 0.5);
            let d = zigzag.windows(2).map(|w| (mid - closest_on_segment(mid, w[0], w[1])).length()).fold(f32::MAX, f32::min);
            assert!(d > 19.0, "Kante innerhalb der Fläche: Abstand {d}");
        }
        // A single point is a plain circle.
        let circle = area_outline(&[egui::pos2(0.0, 0.0)], 10.0);
        assert!(circle.iter().all(|[a, _]| (a.to_vec2().length() - 10.0).abs() < 0.01));
    }
}

