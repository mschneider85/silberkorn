//! Local adjustments: layer list, mask tools (brush, eraser, gradients) and retouching
//! (heal/clone spots), including the viewer interaction.

use cr_core::mask;
use cr_core::params::{BrushStroke, Layer, LayerAdjustments, LinearGradient, Mask, RadialGradient, Spot, SpotMode};
use cr_core::pipeline::{Processor, RenderOptions};
use egui::{Color32, Pos2, Rect, Response, RichText, Stroke, Ui, vec2};

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
            LocalTool::Brush => "✏ Pinsel (B)",
            LocalTool::Eraser => "✖ Radierer (N)",
            LocalTool::Linear => "↕ Linear (L)",
            LocalTool::Radial => "◎ Radial (R)",
            LocalTool::Heal => "✚ Reparieren (H)",
            LocalTool::Clone => "⎘ Klonen (S)",
            LocalTool::Magic => "✨ Magic Brush (J)",
            LocalTool::ColorPick => "💧 Farbe wählen",
        }
    }

    pub fn is_mask_tool(self) -> bool {
        matches!(self, LocalTool::Brush | LocalTool::Eraser | LocalTool::Linear | LocalTool::Radial | LocalTool::Magic)
    }
}

#[derive(Clone, Debug)]
pub struct BrushSettings {
    /// Radius in screen pixels.
    pub size: f32,
    pub hardness: f32,
    pub flow: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 40.0, hardness: 0.5, flow: 1.0 }
    }
}

/// Drag state of the local tools.
#[derive(Clone, Debug, Default)]
pub enum LocalDrag {
    #[default]
    None,
    Stroke(Pos2),
    Gradient,
    /// Moving a spot: index and whether the source (true) or target (false) is dragged.
    Spot(usize, bool),
}

impl App {
    fn mapping(&self) -> Option<(cr_core::geometry::GeoMap, f32, f32)> {
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

    /// Normalized source coordinates -> screen position.
    pub fn uv_to_screen(&self, uv: [f32; 2], img_rect: Rect, scale: f32) -> Option<Pos2> {
        let (geo, pw, ph) = self.mapping()?;
        let (ox, oy) = geo.output_point(uv[0] * pw - 0.5, uv[1] * ph - 0.5);
        Some(img_rect.min + vec2(ox, oy) * scale)
    }

    /// Screen radius -> radius relative to the long source edge.
    fn radius_norm(&self, px: f32, scale: f32) -> f32 {
        let long = self.current.as_ref().map_or(1.0, |c| c.preview.width.max(c.preview.height) as f32);
        px / scale / long
    }

    fn radius_screen(&self, r: f32, scale: f32) -> f32 {
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
        self.local_tool = if self.local_tool == Some(tool) { None } else { Some(tool) };
        self.finish_crop_tool();
        self.straighten_tool = None;
        self.picking_wb = false;
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
                if let Some(p) = hover {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    let r = self.brush.size;
                    painter.circle_stroke(p, r, Stroke::new(1.5, Color32::WHITE));
                    painter.circle_stroke(p, r * self.brush.hardness.max(0.05), Stroke::new(1.0, Color32::from_white_alpha(90)));
                    if tool == LocalTool::Eraser {
                        painter.text(p, egui::Align2::CENTER_CENTER, "−", egui::FontId::proportional(16.0), Color32::WHITE);
                    }
                }
                if response.drag_started()
                    && let Some(p) = response.interact_pointer_pos()
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                    && let Some(li) = self.ensure_layer()
                {
                    let stroke = BrushStroke {
                        points: vec![uv],
                        radius: self.radius_norm(self.brush.size, scale),
                        hardness: self.brush.hardness,
                        flow: self.brush.flow,
                        erase: tool == LocalTool::Eraser,
                    };
                    self.edit(|a| a.layers[li].mask.strokes.push(stroke));
                    self.local_drag = LocalDrag::Stroke(p);
                }
                if response.dragged()
                    && let (Some(p), LocalDrag::Stroke(last)) = (response.interact_pointer_pos(), self.local_drag.clone())
                    && (p - last).length() > (self.brush.size * 0.15).max(2.0)
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
            LocalTool::Magic | LocalTool::ColorPick => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if response.clicked()
                    && let Some(p) = response.interact_pointer_pos()
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                {
                    if tool == LocalTool::Magic {
                        self.request_ai(crate::workers::AiKind::Magic { seed: uv, tolerance: self.magic_tolerance });
                    } else {
                        self.pick_color_range(uv);
                    }
                }
            }
        }
        if response.drag_stopped() {
            self.local_drag = LocalDrag::None;
        }
    }

    fn draw_gradients(&self, painter: &egui::Painter, img_rect: Rect, scale: f32) {
        let Some(cur) = &self.current else { return };
        let Some(layer) = self.active_layer.and_then(|i| cur.adjustments.layers.get(i)) else { return };
        let stroke = Stroke::new(1.5, Color32::WHITE);
        for g in &layer.mask.linear {
            let (Some(a), Some(b)) = (self.uv_to_screen(g.start, img_rect, scale), self.uv_to_screen(g.end, img_rect, scale)) else { continue };
            let d = b - a;
            let n = vec2(-d.y, d.x).normalized() * 2000.0;
            painter.line_segment([a - n, a + n], stroke);
            painter.line_segment([b - n, b + n], Stroke::new(1.0, Color32::from_white_alpha(120)));
            painter.line_segment([a, b], Stroke::new(1.0, Color32::from_rgb(255, 210, 0)));
            painter.circle_filled(a, 4.0, Color32::WHITE);
        }
        for g in &layer.mask.radial {
            let Some(c) = self.uv_to_screen(g.center, img_rect, scale) else { continue };
            let r = self.radius_screen(g.radius_x, scale);
            painter.circle_stroke(c, r, stroke);
            painter.circle_stroke(c, r * (1.0 - g.feather), Stroke::new(1.0, Color32::from_white_alpha(120)));
            painter.circle_filled(c, 3.0, Color32::WHITE);
        }
    }

    fn spot_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32, tool: LocalTool) {
        let Some(cur) = &self.current else { return };
        let spots = cur.adjustments.retouch.clone();
        let painter = ui.painter_at(response.rect);
        let mut screen: Vec<(Pos2, Pos2, f32)> = Vec::new();
        for s in &spots {
            if let (Some(t), Some(q)) = (self.uv_to_screen(s.target, img_rect, scale), self.uv_to_screen(s.source, img_rect, scale)) {
                screen.push((t, q, self.radius_screen(s.radius, scale)));
            } else {
                screen.push((Pos2::ZERO, Pos2::ZERO, 0.0));
            }
        }
        // Draw spots (selected one with its source).
        for (i, (t, q, r)) in screen.iter().enumerate() {
            let sel = self.selected_spot == Some(i);
            let col = if sel { Color32::WHITE } else { Color32::from_white_alpha(140) };
            painter.circle_stroke(*t, *r, Stroke::new(if sel { 2.0 } else { 1.0 }, col));
            if sel {
                painter.circle_stroke(*q, *r, Stroke::new(1.0, Color32::from_rgb(255, 210, 0)));
                painter.line_segment([*t, *q], Stroke::new(1.0, Color32::from_rgb(255, 210, 0)));
            }
        }
        if let Some(p) = response.hover_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            painter.circle_stroke(p, self.spot_size, Stroke::new(1.0, Color32::from_white_alpha(160)));
        }
        let selected = self.selected_spot;
        let hit = |p: Pos2| -> Option<(usize, bool)> {
            for (i, (t, q, r)) in screen.iter().enumerate().rev() {
                if (p - *q).length() < *r && selected == Some(i) {
                    return Some((i, true));
                }
                if (p - *t).length() < *r {
                    return Some((i, false));
                }
            }
            None
        };
        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
            && let Some((i, is_source)) = hit(p)
        {
            self.selected_spot = Some(i);
            self.local_drag = LocalDrag::Spot(i, is_source);
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
                        let d = [s.source[0] - s.target[0], s.source[1] - s.target[1]];
                        s.target = uv;
                        s.source = [uv[0] + d[0], uv[1] + d[1]];
                    }
                }
            });
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            if let Some((i, _)) = hit(p) {
                self.selected_spot = Some(i);
            } else if let Some(uv) = self.screen_to_uv(p, img_rect, scale) {
                let radius = self.radius_norm(self.spot_size, scale);
                let source = self.current.as_ref().map_or(uv, |c| mask::auto_source(&c.preview, uv, radius));
                let mode = if tool == LocalTool::Heal { SpotMode::Heal } else { SpotMode::Clone };
                self.edit(|a| a.retouch.push(Spot { mode, target: uv, source, radius, feather: 0.4 }));
                self.selected_spot = self.current.as_ref().map(|c| c.adjustments.retouch.len() - 1);
            }
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
        let px = cr_core::image::LinearImage::new(3, 3, vec![acc.map(|v| v / n); 9]);
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
        let rgb = [r.rgba[16], r.rgba[17], r.rgba[18]].map(|v| cr_core::color::srgb_decode(v as f32 / 255.0));
        // sRGB -> working, then Oklab hue (the same space the pipeline uses).
        let to_working = cr_core::color::mat_inv(&cr_core::color::OutputSpace::Srgb.from_working()).unwrap_or(cr_core::color::IDENTITY);
        let [_, a, b] = cr_core::color::Oklab::default().from_linear(cr_core::color::mat_vec(&to_working, rgb));
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
                if ui.add_enabled(!busy, egui::Button::new("👤 Motiv auswählen")).clicked() {
                    ai_action = Some(1);
                }
                if ui.add_enabled(!busy, egui::Button::new("☁ Himmel auswählen")).clicked() {
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
            ui.label("Kein Bild ausgewählt.");
            return;
        }
        let mut tool_click = None;
        section(ui, "Werkzeuge", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                for t in [LocalTool::Brush, LocalTool::Eraser, LocalTool::Linear, LocalTool::Radial, LocalTool::Heal, LocalTool::Clone] {
                    if ui.selectable_label(self.local_tool == Some(t), t.label()).clicked() {
                        tool_click = Some(t);
                    }
                }
            });
            match self.local_tool {
                Some(LocalTool::Brush | LocalTool::Eraser) => {
                    slider_row(ui, "Größe", &mut self.brush.size, 2.0..=400.0, 40.0, 0);
                    slider_row(ui, "Härte", &mut self.brush.hardness, 0.0..=1.0, 0.5, 2);
                    slider_row(ui, "Fluss", &mut self.brush.flow, 0.05..=1.0, 1.0, 2);
                    ui.label(RichText::new("[ und ] ändern die Größe").small().color(Color32::from_gray(140)));
                }
                Some(LocalTool::Heal | LocalTool::Clone) => {
                    slider_row(ui, "Größe", &mut self.spot_size, 3.0..=300.0, 25.0, 0);
                    ui.label(RichText::new("Klicken: Punkt setzen · Ziehen: verschieben · Entf: löschen").small().color(Color32::from_gray(140)));
                }
                _ => {}
            }
        });
        if let Some(t) = tool_click {
            self.set_local_tool(t);
        }

        let mut style = None;
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id("style_brushes"), false)
            .show_header(ui, |ui| ui.label(RichText::new("Style Brushes").strong()))
            .body(|ui| {
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
                if self.active_layer.is_some() && ui.button("🗑 Löschen").clicked() {
                    action = Some(LayerAction::Delete);
                }
            });
        });

        if let Some(li) = self.active_layer.filter(|i| *i < layers.len()) {
            let layer = &mut layers[li];
            let show_mask = &mut self.show_mask;
            section(ui, "Ebene", None, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut layer.name);
                });
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
                changed |= slider_row(ui, "Temperatur", &mut a.temperature, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Tint", &mut a.tint, -100.0..=100.0, 0.0, 0);
                changed |= slider_row(ui, "Dunst", &mut a.dehaze, -100.0..=100.0, 0.0, 0);
            });
            if reset {
                layer.adjustments = LayerAdjustments::default();
                changed = true;
            }
            self.mask_refinement_ui(ui, li, &mut changed);
        }

        let n_spots = self.current.as_ref().map_or(0, |c| c.adjustments.retouch.len());
        let mut clear_spots = false;
        let mut delete_spot = false;
        section(ui, "Reparieren & Klonen", None, |ui| {
            ui.label(format!("{n_spots} Punkt(e)"));
            ui.horizontal(|ui| {
                if self.selected_spot.is_some() && ui.button("Ausgewählten löschen").clicked() {
                    delete_spot = true;
                }
                if n_spots > 0 && ui.button("Alle löschen").clicked() {
                    clear_spots = true;
                }
            });
        });

        if changed {
            self.request_render();
        }
        if delete_spot {
            self.delete_selected_spot();
        }
        if clear_spots {
            self.selected_spot = None;
            self.edit(|a| a.retouch.clear());
        }
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
