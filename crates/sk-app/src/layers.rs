//! Local adjustments: layer list, mask tools (brush, eraser, gradients) and retouching
//! (heal/clone spots), including the viewer interaction.

use sk_core::mask;
use sk_core::params::{BrushStroke, Layer, LayerAdjustments, Mask, Spot, SpotMode, SpotStroke};
use sk_core::pipeline::Processor;
use sk_core::tools::Tool;
use egui::{Color32, Pos2, Rect, Response, RichText, Stroke, Ui, Vec2, pos2, vec2};

use crate::theme::{self, icon};

use crate::ai_models::AiFeature;
use crate::app::{App, CurveChannel, Tab};
use crate::widgets::{SliderOpts, hue_rail, slider_row, slider_row_with, subheading};

/// How long an image stays in the Ebenen tab before its subject mask is computed ahead.
const SUBJECT_AHEAD_AFTER: std::time::Duration = std::time::Duration::from_millis(1500);

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
    /// The object under the clicks (MobileSAM).
    Object,
    /// Picks the color for the color range refinement.
    ColorPick,
    /// Generative removal (see `remove.rs`).
    Remove,
}

/// What painting does with Reparieren, Klonen and Entfernen while an area is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AreaPaint {
    /// A new area (as without a chosen one).
    #[default]
    New,
    /// Adds to the chosen area (Shift while painting).
    Extend,
    /// Takes part of the chosen area away (Alt while painting).
    Shrink,
}

/// Style brushes: brush presets with predefined layer adjustments.
pub fn style_brushes() -> [(&'static str, LayerAdjustments); 12] {
    [
        ("Aufhellen", LayerAdjustments { exposure: 0.7, ..Default::default() }),
        ("Abdunkeln", LayerAdjustments { exposure: -0.7, ..Default::default() }),
        ("Wärmer", LayerAdjustments { temperature: 40.0, ..Default::default() }),
        ("Kühler", LayerAdjustments { temperature: -40.0, ..Default::default() }),
        ("Mehr Farbe", LayerAdjustments { saturation: 40.0, ..Default::default() }),
        ("Entsättigen", LayerAdjustments { saturation: -70.0, ..Default::default() }),
        ("Details betonen", LayerAdjustments { clarity: 40.0, structure: 50.0, ..Default::default() }),
        ("Haut glätten", LayerAdjustments { clarity: -40.0, structure: -60.0, ..Default::default() }),
        ("Lichter zurückholen", LayerAdjustments { highlights: 70.0, ..Default::default() }),
        ("Tiefen aufhellen", LayerAdjustments { shadows: 70.0, ..Default::default() }),
        ("Dunst entfernen", LayerAdjustments { dehaze: 60.0, ..Default::default() }),
        ("Augen aufhellen", LayerAdjustments { exposure: 0.4, clarity: 20.0, saturation: 10.0, ..Default::default() }),
    ]
}

impl LocalTool {
    /// Icon and name, from the tool tiles (one name per tool everywhere).
    pub fn label(self) -> String {
        let (icon, name, _) = self.tile();
        let name = if self == LocalTool::ColorPick { "Farbe aufnehmen" } else { name };
        format!("{icon}  {name}")
    }

    /// Icon, short name and command (for the shortcut) of the tool tiles.
    pub fn tile(self) -> (&'static str, &'static str, &'static str) {
        use crate::theme::icon;
        match self {
            LocalTool::Brush => (icon::PAINT_BRUSH, "Pinsel", "tool-brush"),
            LocalTool::Eraser => (icon::ERASER, "Radierer", "tool-eraser"),
            LocalTool::Linear => (icon::GRADIENT, "Linear", "tool-linear"),
            LocalTool::Radial => (icon::CIRCLE_DASHED, "Radial", "tool-radial"),
            LocalTool::Heal => (icon::BANDAIDS, "Reparieren", "tool-heal"),
            LocalTool::Clone => (icon::STAMP, "Klonen", "tool-clone"),
            LocalTool::Magic => (icon::MAGIC_WAND, "Zauberpinsel", "tool-magic"),
            LocalTool::Object => (icon::CURSOR_CLICK, "Objekt", "tool-object"),
            LocalTool::ColorPick => (icon::EYEDROPPER, "Farbe", ""),
            LocalTool::Remove => (icon::SPARKLE, "Entfernen", "tool-remove"),
        }
    }

    /// Tools painted with a round brush (size, mostly hardness).
    pub fn is_brush(self) -> bool {
        matches!(self, LocalTool::Brush | LocalTool::Eraser | LocalTool::Heal | LocalTool::Clone | LocalTool::Magic | LocalTool::Remove)
    }

    /// The tab with the tool's settings.
    pub fn tab(self) -> Tab {
        match self {
            LocalTool::Heal | LocalTool::Clone | LocalTool::Remove => Tab::Retouch,
            _ => Tab::Layers,
        }
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
    /// How different it may be and still be painted, 0..100 (see `BrushStroke::tolerance`).
    pub tolerance: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 40.0, hardness: 0.5, opacity: 1.0, flow: 1.0, auto_mask: false, tolerance: sk_core::params::DEFAULT_TOLERANCE }
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
    crate::widgets::unit_percent_row(ui, "Härte", &mut b.hardness, 0.0..=1.0, d.hardness, Default::default());
    crate::widgets::unit_percent_row(ui, "Deckkraft", &mut b.opacity, 0.05..=1.0, d.opacity, Default::default());
    if flow_and_auto {
        crate::widgets::unit_percent_row(ui, "Fluss", &mut b.flow, 0.02..=1.0, d.flow, Default::default());
        crate::widgets::checkbox(ui, &mut b.auto_mask, "Kanten beachten")
            .on_hover_text("Wirkt nur, wo das Bild der Pinselmitte ähnelt – hält Kanten wie den Horizont sauber");
        // Without it, the tolerance is shown but disabled.
        ui.add_enabled_ui(b.auto_mask, |ui| {
            slider_row(ui, "Toleranz", &mut b.tolerance, 0.0..=100.0, d.tolerance, 0);
        })
        .response
        .on_hover_text("Wie verschieden das Bild von der Pinselmitte sein darf und trotzdem gemalt wird");
    }
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

/// The clicks of the object being selected: each click computes the object anew from all of
/// them; the result replaces the previous one (added to what the mask held before the first click).
pub struct ObjectSession {
    pub path: std::path::PathBuf,
    /// Layer id.
    pub layer: u32,
    /// The layer's AI mask before the first click.
    pub base: Option<sk_core::params::AiMask>,
    pub clicks: Vec<sk_ai::segment::Click>,
}

/// Drag state of the local tools.
#[derive(Clone, Debug, Default)]
pub enum LocalDrag {
    #[default]
    None,
    Stroke(Pos2),
    /// Drawing a new gradient (`App::selected_gradient`).
    Gradient,
    /// Changing an existing gradient.
    EditGradient(crate::gradients::GradDrag),
    /// Moving a retouch area: index and whether the source (true) or target (false) is dragged.
    Spot(usize, bool),
    /// Painting a retouch area or a Magic Brush stroke: its points so far (normalized) and the
    /// last pointer position.
    Paint(Vec<[f32; 2]>, Pos2),
    /// Changing the brush with the right mouse button: where the drag started (the outline
    /// stays there) and the last pointer position.
    BrushAdjust { anchor: Pos2, last: Pos2 },
}

/// What the geometric map of the viewer image depends on.
struct MapKey {
    path: std::path::PathBuf,
    geometry: sk_core::params::Geometry,
    lens: sk_core::params::LensCorrection,
    rotation: u8,
    hidden: Vec<Tool>,
    size: (usize, usize),
    ignore_crop: bool,
}

impl MapKey {
    fn matches(&self, cur: &crate::app::Current, ignore_crop: bool) -> bool {
        let a = &cur.adjustments;
        self.size == (cur.preview.width, cur.preview.height)
            && self.ignore_crop == ignore_crop
            && self.rotation == a.rotation
            && self.path == cur.meta.path
            && self.hidden == a.hidden
            && self.geometry == a.geometry
            && self.lens == a.lens
    }
}

thread_local! {
    /// The last geometric map: asked for per stroke point and per retouch area, and building
    /// it clones the adjustments while a tool is switched off.
    static MAPPING: std::cell::RefCell<Option<(MapKey, sk_core::geometry::GeoMap)>> = const { std::cell::RefCell::new(None) };
}

impl App {
    fn mapping(&self) -> Option<(sk_core::geometry::GeoMap, f32, f32)> {
        let cur = self.current.as_ref()?;
        let (pw, ph) = (cur.preview.width as f32, cur.preview.height as f32);
        let ignore_crop = self.crop_tool.is_some();
        let geo = MAPPING.with_borrow_mut(|m| {
            if let Some((key, geo)) = m
                && key.matches(cur, ignore_crop)
            {
                return geo.clone();
            }
            let geo = sk_core::pipeline::geomap(&cur.meta, &cur.adjustments, ignore_crop, cur.preview.width, cur.preview.height);
            let a = &cur.adjustments;
            let key = MapKey {
                path: cur.meta.path.clone(),
                geometry: a.geometry.clone(),
                lens: a.lens.clone(),
                rotation: a.rotation,
                hidden: a.hidden.clone(),
                size: (cur.preview.width, cur.preview.height),
                ignore_crop,
            };
            *m = Some((key, geo.clone()));
            geo
        });
        Some((geo, pw, ph))
    }

    /// Screen position -> normalized source coordinates.
    pub fn screen_to_uv(&self, pos: Pos2, img_rect: Rect, scale: f32) -> Option<[f32; 2]> {
        let (geo, pw, ph) = self.mapping()?;
        let o = (pos - img_rect.min) / scale;
        let (sx, sy) = geo.source_point(o.x, o.y);
        Some([(sx + 0.5) / pw, (sy + 0.5) / ph])
    }

    /// Normalized source coordinates -> screen positions.
    pub(crate) fn uv_mapper(&self, img_rect: Rect, scale: f32) -> Option<UvToScreen> {
        let (geo, pw, ph) = self.mapping()?;
        Some(UvToScreen { geo, pw, ph, origin: img_rect.min, scale })
    }

    /// Screen radius -> radius relative to the long source edge (the brush on screen and what
    /// it paints are the same size also with straightening or perspective correction).
    pub(crate) fn radius_norm(&self, px: f32, scale: f32) -> f32 {
        let long = self.current.as_ref().map_or(1.0, |c| c.preview.width.max(c.preview.height) as f32);
        px / scale * self.source_scale() / long
    }

    pub(crate) fn radius_screen(&self, r: f32, scale: f32) -> f32 {
        let long = self.current.as_ref().map_or(1.0, |c| c.preview.width.max(c.preview.height) as f32);
        r * long * scale / self.source_scale()
    }

    /// Source pixels per output pixel (see `GeoMap::source_scale`).
    fn source_scale(&self) -> f32 {
        self.mapping().map_or(1.0, |(geo, ..)| geo.source_scale())
    }

    /// Index of the active layer, creating one if there is none.
    pub(crate) fn ensure_layer(&mut self) -> Option<usize> {
        self.ensure_layer_named(None)
    }

    /// [`Self::ensure_layer`]; a new layer is called `name` (numbered if taken), e.g. after the
    /// automatic selection that made it. The active layer keeps its name.
    pub(crate) fn ensure_layer_named(&mut self, name: Option<&str>) -> Option<usize> {
        let cur = self.current.as_mut()?;
        if let Some(i) = self.active_layer
            && i < cur.adjustments.layers.len()
        {
            return Some(i);
        }
        let layers = &cur.adjustments.layers;
        let taken = |n: &str| layers.iter().any(|l| l.name == n);
        let name = match name {
            Some(base) => (1..).map(|k| if k == 1 { base.to_string() } else { format!("{base} {k}") }).find(|n| !taken(n)).unwrap_or_default(),
            None => format!("Ebene {}", layers.len() + 1),
        };
        cur.adjustments.layers.push(Layer { name, ..Default::default() });
        cur.adjustments.assign_layer_ids();
        self.active_layer = Some(cur.adjustments.layers.len() - 1);
        self.active_layer
    }

    /// Id of layer `i`, which stays with it when other layers are deleted or edits undone.
    pub(crate) fn layer_id(&mut self, i: usize) -> Option<u32> {
        let cur = self.current.as_mut()?;
        cur.adjustments.assign_layer_ids();
        cur.adjustments.layers.get(i).map(|l| l.id)
    }

    /// Name of the layer the develop tools change (`None`: the background).
    pub(crate) fn edit_layer_name(&self) -> Option<String> {
        let cur = self.current.as_ref()?;
        cur.adjustments.layers.get(self.active_layer?).map(|l| l.name.clone())
    }

    /// Whether a layer is chosen: the develop tools change it, what only changes the whole image
    /// is disabled.
    pub(crate) fn on_layer(&self) -> bool {
        self.current.as_ref().zip(self.active_layer).is_some_and(|(c, i)| i < c.adjustments.layers.len())
    }

    /// Whether the tab can be opened: not those that only change the whole image while a layer
    /// is chosen.
    pub(crate) fn tab_available(&self, tab: Tab) -> bool {
        !(tab.edits_image() && self.on_layer())
    }

    /// Goes to a tab if it is there, else says why not.
    pub(crate) fn open_tab(&mut self, tab: Tab) -> bool {
        if self.tab_available(tab) {
            self.tab = tab;
            true
        } else {
            self.status = format!("„{}“ wirkt nur aufs ganze Bild – erst den Hintergrund wählen", tab.title());
            false
        }
    }

    /// Whether a tool section can be reached (palette, side column): its tab is there and, on a
    /// layer, the tool works on it.
    pub(crate) fn section_available(&self, tab: Tab, title: &str) -> bool {
        let background_only = || Tool::ALL.iter().any(|t| t.label() == title && t.layer_support() == sk_core::tools::LayerSupport::None);
        self.tab_available(tab) && !(self.on_layer() && tab != Tab::Layers && background_only())
    }

    /// Chooses what the develop tools change: a layer or (`None`) the background.
    pub(crate) fn select_layer(&mut self, layer: Option<usize>) {
        self.apply_layer_action(Some(LayerAction::Select(layer)));
    }

    /// The next layer above (`up`) or below the active one, the background below the lowest.
    pub(crate) fn step_layer(&mut self, up: bool) {
        let Some(cur) = self.current.as_ref() else { return };
        let n = cur.adjustments.layers.len();
        if n == 0 {
            return;
        }
        // Positions as in the list: the background at 0, layer i at i + 1.
        let pos = self.active_layer.map_or(0, |i| i + 1);
        let pos = if up { (pos + 1).min(n) } else { pos.saturating_sub(1) };
        self.select_layer(pos.checked_sub(1));
    }

    /// Index of the layer with id `id`, if it still exists.
    pub(crate) fn layer_index(&self, id: u32) -> Option<usize> {
        self.current.as_ref()?.adjustments.layer_index(id)
    }

    pub fn set_local_tool(&mut self, tool: LocalTool) {
        let new = if self.local_tool == Some(tool) { None } else { Some(tool) };
        // The chosen retouch area stays chosen only while its pins are shown (heal, clone).
        let (spot, removal) = (self.selected_spot, self.selected_removal);
        self.end_viewer_tools();
        if matches!(new, Some(LocalTool::Heal | LocalTool::Clone)) {
            self.selected_spot = spot;
        }
        if new == Some(LocalTool::Remove) {
            self.selected_removal = removal;
        }
        self.local_tool = new;
        self.object_session = None;
        // No layer yet: the first stroke or click makes one (choosing a tool alone leaves none).
        match new {
            Some(LocalTool::Object) => self.with_ai(AiFeature::Object, |app| {
                if app.local_tool == Some(LocalTool::Object) {
                    app.prepare_object();
                }
            }),
            // Its network takes seconds to load: from now on, not with the first stroke (only for
            // those who use the tool; it takes about 1 GB).
            Some(LocalTool::Remove) if self.subject_ahead => self.with_ai(AiFeature::Inpaint, |app| app.workers.inpaint(crate::workers::InpaintJob::warm_up())),
            Some(LocalTool::Remove) => {
                self.ensure_ai(AiFeature::Inpaint);
            }
            _ => {}
        }
        self.request_render();
    }

    /// Computes the image's subject mask in the background (once per image, without notice), so
    /// that Motiv is there at once when clicked (see `subject_worker`). Only for an image that
    /// stays in the Ebenen tab a moment: leafing through images computes nothing.
    fn prepare_subject(&mut self, ctx: &egui::Context) {
        let Some(cur) = &self.current else { return };
        let key = crate::subject_worker::key(&cur.meta.path, cur.preview.width, cur.preview.height);
        if !self.subject_ahead || self.subject_prepared == Some(key) || !sk_ai::subject::available() {
            return;
        }
        let since = match self.subject_shown {
            Some((k, t)) if k == key => t,
            _ => {
                self.subject_shown = Some((key, std::time::Instant::now()));
                ctx.request_repaint_after(SUBJECT_AHEAD_AFTER);
                return;
            }
        };
        if since.elapsed() < SUBJECT_AHEAD_AFTER {
            ctx.request_repaint_after(SUBJECT_AHEAD_AFTER.saturating_sub(since.elapsed()));
            return;
        }
        self.subject_prepared = Some(key);
        self.workers.ai_mask(crate::workers::AiJob {
            generation: self.generation(),
            layer: 0,
            kind: crate::workers::AiKind::SubjectPrepare,
            meta: cur.meta.clone(),
            preview: cur.preview.clone(),
            adjustments: cur.adjustments.clone(),
        });
    }

    /// Analyzes the image for object selection in the background, before the first click.
    fn prepare_object(&mut self) {
        if !sk_ai::segment::available() {
            return;
        }
        let Some(cur) = &self.current else { return };
        self.workers.ai_mask(crate::workers::AiJob {
            generation: self.generation(),
            // (No result goes to a layer.)
            layer: 0,
            kind: crate::workers::AiKind::ObjectPrepare,
            meta: cur.meta.clone(),
            preview: cur.preview.clone(),
            adjustments: cur.adjustments.clone(),
        });
    }

    /// Settings of the object tool.
    fn object_controls(&mut self, ui: &mut Ui) {
        let faint = |t: &str| egui::RichText::new(t).small().color(crate::theme::TEXT_FAINT);
        if let Some(p) = self.ai_download_progress(AiFeature::Object) {
            crate::ai_models::download_progress(ui, AiFeature::Object, p, self.job_cancel(None, Some(&AiFeature::Object.job_label())));
            return;
        }
        ui.label(faint("Klick: Objekt auswählen · weitere Klicks verfeinern · Umschalt+Klick: weiteres Objekt · Alt+Klick: Teil ausnehmen"));
    }

    /// Object selection: a click adds the object under it, Alt+click takes the part under it away,
    /// Shift+click starts another object (all clicks of one go to the network together).
    fn object_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let painter = ui.painter_at(response.rect);
        if response.hover_pos().is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if let Some(p) = pressed_on(ui, response)
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
            && let Some(index) = self.ensure_layer()
            && let Some(layer) = self.layer_id(index)
        {
            if !self.ensure_ai(AiFeature::Object) {
                self.status = "Objekt auswählen: das KI-Modell wird noch geladen …".into();
                return;
            }
            let (include, another) = ui.input(|i| (!i.modifiers.alt, i.modifiers.shift));
            let path = self.current_path().unwrap_or_default();
            let fresh = another || self.object_session.as_ref().is_none_or(|s| s.layer != layer || s.path != path);
            if fresh {
                if !include {
                    self.status = "Zuerst auf das Objekt klicken; Alt+Klick nimmt danach Teile aus".into();
                    return;
                }
                let base = self.current.as_ref().and_then(|c| c.adjustments.layers.get(index)).and_then(|l| l.mask.ai.clone());
                self.object_session = Some(ObjectSession { path, layer, base, clicks: Vec::new() });
            }
            let session = self.object_session.as_mut().unwrap();
            session.clicks.push(sk_ai::segment::Click { uv, include });
            let clicks = session.clicks.clone();
            self.request_ai(crate::workers::AiKind::Object { clicks });
        }
        // The clicks so far: green adds, red takes away.
        if let (Some(s), Some(map)) = (&self.object_session, self.uv_mapper(img_rect, scale)) {
            for c in &s.clicks {
                let p = map.map(c.uv);
                let color = if c.include { Color32::from_rgb(70, 210, 110) } else { Color32::from_rgb(235, 80, 70) };
                painter.circle(p, 5.0, color, Stroke::new(1.5, Color32::from_black_alpha(160)));
            }
        }
    }

    /// Viewer interaction and overlay drawing for the local tools.
    pub fn local_overlay(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let Some(tool) = self.local_tool else { return };
        let hover = response.hover_pos();
        let painter = ui.painter_at(response.rect);
        self.brush_adjust_gesture(ui, response, &painter, tool);

        match tool {
            LocalTool::Brush | LocalTool::Eraser => {
                // Alt held: the brush erases (with the eraser's settings), as in Lightroom.
                let erasing = tool == LocalTool::Eraser || ui.input(|i| i.modifiers.alt);
                let b = self.brushes.of(Some(if erasing { LocalTool::Eraser } else { tool })).clone();
                if let Some(p) = hover.filter(|_| !self.adjusting_brush()) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    brush_cursor(&painter, p, &b);
                    if erasing {
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
                        erase: erasing,
                        opacity: b.opacity,
                        auto_mask: b.auto_mask,
                        build_up: true,
                        tolerance: b.tolerance,
                    };
                    self.edit(|a| a.layers[li].mask.strokes.push(stroke));
                    self.note_painted(img_rect, &[p], b.size);
                    self.paint_prev = None;
                    self.local_drag = LocalDrag::Stroke(p);
                }
                if response.dragged()
                    && let LocalDrag::Stroke(mut last) = self.local_drag
                    && let Some(li) = self.active_layer
                {
                    // Every movement of the frame, so that fast strokes stay round.
                    let mut screen = Vec::new();
                    let mut uvs = Vec::new();
                    for p in drag_trail(ui, response) {
                        if (p - last).length() > point_spacing(b.size)
                            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                        {
                            screen.push(p);
                            uvs.push(uv);
                            last = p;
                        }
                    }
                    if !uvs.is_empty() {
                        self.edit(|a| {
                            if let Some(s) = a.layers.get_mut(li).and_then(|l| l.mask.strokes.last_mut()) {
                                s.points.extend(uvs);
                            }
                        });
                        let LocalDrag::Stroke(from) = self.local_drag else { unreachable!() };
                        let trail: Vec<Pos2> = self.paint_prev.into_iter().chain([from]).chain(screen.iter().copied()).collect();
                        self.note_painted(img_rect, &trail, b.size);
                        self.paint_prev = Some(if screen.len() >= 2 { screen[screen.len() - 2] } else { from });
                        self.local_drag = LocalDrag::Stroke(last);
                    }
                }
            }
            LocalTool::Linear | LocalTool::Radial => self.gradient_interaction(ui, response, img_rect, scale, tool),
            LocalTool::Heal | LocalTool::Clone => {
                self.spot_interaction(ui, response, img_rect, scale, tool);
            }
            LocalTool::Remove => self.remove_interaction(ui, response, img_rect, scale),
            LocalTool::Magic => self.magic_interaction(ui, response, img_rect, scale),
            LocalTool::Object => self.object_interaction(ui, response, img_rect, scale),
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

    /// Whether the brush is being changed with the right mouse button (see
    /// [`Self::brush_adjust_gesture`]); the tools then leave the outline to it.
    pub(crate) fn adjusting_brush(&self) -> bool {
        matches!(self.local_drag, LocalDrag::BrushAdjust { .. })
    }

    /// Dragging with the right mouse button changes the brush: left/right its size, up/down its hardness (up is harder). Each movement changes
    /// only what its main direction says: a hand moving up and down drifts a little sideways
    /// as well, which must not change the size. The outline stays where the drag started.
    fn brush_adjust_gesture(&mut self, ui: &Ui, response: &Response, painter: &egui::Painter, tool: LocalTool) {
        if !tool.is_brush() {
            return;
        }
        if response.drag_started_by(egui::PointerButton::Secondary)
            && let Some(anchor) = ui.input(|i| i.pointer.press_origin())
        {
            self.local_drag = LocalDrag::BrushAdjust { anchor, last: anchor };
        }
        let LocalDrag::BrushAdjust { anchor, mut last } = self.local_drag else { return };
        // The Magic Brush has no hardness.
        let with_hardness = tool != LocalTool::Magic;
        let b = self.brushes.of(Some(tool));
        let before = last;
        for p in drag_trail(ui, response) {
            let d = p - last;
            if d.x.abs() > d.y.abs() || !with_hardness {
                b.size = (b.size + d.x).clamp(2.0, 400.0);
            } else {
                b.hardness = (b.hardness - d.y / BRUSH_ADJUST_HARDNESS_SPAN).clamp(0.0, 1.0);
            }
            last = p;
        }
        if last != before {
            self.brushes_changed_at = Some(std::time::Instant::now());
        }
        self.local_drag = LocalDrag::BrushAdjust { anchor, last };
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        let b = self.brushes.of(Some(tool)).clone();
        brush_cursor(painter, anchor, &b);
        let text = if with_hardness {
            format!("Größe {:.0} · Härte {:.0} %", b.size, b.hardness * 100.0)
        } else {
            format!("Größe {:.0}", b.size)
        };
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
        let pos = anchor + vec2(-galley.size().x / 2.0, b.size + 8.0);
        painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand(4.0), 4.0, Color32::from_black_alpha(170));
        painter.galley(pos, galley, Color32::WHITE);
    }

    /// Extends the stroke being painted (`LocalDrag::Paint`) and returns its points in
    /// screen coordinates for the preview.
    fn extend_paint(&mut self, ui: &Ui, response: &Response, img_rect: Rect, scale: f32, spacing: f32) -> Vec<Pos2> {
        let LocalDrag::Paint(mut pts, mut last) = self.local_drag.clone() else { return Vec::new() };
        for p in drag_trail(ui, response) {
            if (p - last).length() > spacing
                && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
            {
                pts.push(uv);
                last = p;
            }
        }
        self.local_drag = LocalDrag::Paint(pts.clone(), last);
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
    /// chosen automatically. The pins move an area (white) or its source (yellow). With an area
    /// chosen, Shift paints onto it and Alt takes part of it away (see [`AreaPaint`]). An
    /// Alt-click (as with Photoshop's clone stamp) puts the source there: of the chosen area, or
    /// of the next one painted.
    fn spot_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32, tool: LocalTool) {
        let Some(cur) = &self.current else { return };
        let spots = cur.adjustments.retouch.clone();
        let painter = ui.painter_at(response.rect);
        let Some(map) = self.uv_mapper(img_rect, scale) else { return };
        // Screen geometry of each area: its lines (the first starts at the target), the lines
        // taken away, the offset to the source and the radius.
        let screen = |pts: &mut dyn Iterator<Item = &[f32; 2]>| pts.map(|p| map.map(*p)).collect::<Vec<Pos2>>();
        type Shown = (Vec<Vec<Pos2>>, Vec<Vec<Pos2>>, Vec2, f32);
        let shown: Vec<Shown> = spots
            .iter()
            .map(|s| {
                let mut lines = vec![screen(&mut std::iter::once(&s.target).chain(&s.path))];
                let mut erased = Vec::new();
                for st in s.strokes.iter().filter(|st| !st.points.is_empty()) {
                    if st.erase { erased.push(screen(&mut st.points.iter())) } else { lines.push(screen(&mut st.points.iter())) }
                }
                let offset = map.map(s.source) - lines[0][0];
                (lines, erased, offset, self.radius_screen(s.radius, scale))
            })
            .collect();
        let selected = self.selected_spot.filter(|i| *i < spots.len());
        let pin_hit = |p: Pos2| -> Option<(usize, bool)> {
            for (i, (lines, _, offset, _)) in shown.iter().enumerate().rev() {
                if selected == Some(i) && (p - (lines[0][0] + *offset)).length() < PIN_HIT {
                    return Some((i, true));
                }
                if (p - lines[0][0]).length() < PIN_HIT {
                    return Some((i, false));
                }
            }
            None
        };
        // Over a pin the cursor shows that it can be grabbed; elsewhere the brush outline.
        let moving = matches!(self.local_drag, LocalDrag::Spot(..));
        let hovered_pin = response.hover_pos().and_then(|p| pin_hit(p));
        for (i, (lines, erased, offset, r)) in shown.iter().enumerate() {
            let sel = selected == Some(i);
            for line in lines {
                paint_area(&painter, line, *r, if sel { Color32::WHITE } else { Color32::from_white_alpha(150) });
            }
            if sel {
                for line in erased {
                    paint_area(&painter, line, *r, ERASED);
                }
                for line in lines {
                    let src: Vec<Pos2> = line.iter().map(|p| *p + *offset).collect();
                    paint_area(&painter, &src, *r, crate::theme::GUIDE);
                }
                painter.line_segment([lines[0][0], lines[0][0] + *offset], Stroke::new(1.0, crate::theme::GUIDE));
                pin(&painter, lines[0][0] + *offset, crate::theme::GUIDE, hovered_pin == Some((i, true)));
            }
            pin(&painter, lines[0][0], if sel { Color32::WHITE } else { Color32::from_white_alpha(170) }, hovered_pin == Some((i, false)));
        }
        // The source chosen for the next area.
        if let Some(src) = self.next_source.filter(|_| selected.is_none()) {
            let c = map.map(src);
            let s = Stroke::new(1.5, crate::theme::GUIDE);
            painter.circle_stroke(c, 9.0, s);
            painter.line_segment([c - vec2(14.0, 0.0), c + vec2(14.0, 0.0)], s);
            painter.line_segment([c - vec2(0.0, 14.0), c + vec2(0.0, 14.0)], s);
        }
        // Painting onto the chosen area goes with its size.
        let mode = if matches!(self.local_drag, LocalDrag::Paint(..)) { self.stroke_paint } else { self.stroke_mode(ui, selected.is_some()) };
        let mut b = self.brushes.spot.clone();
        if mode != AreaPaint::New
            && let Some(i) = selected
        {
            b.size = shown[i].3;
        }
        if moving {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovered_pin.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if let Some(p) = response.hover_pos().filter(|_| !self.adjusting_brush()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            brush_cursor(&painter, p, &b);
        }
        if let Some(p) = pressed_on(ui, response) {
            if let Some((i, is_source)) = pin_hit(p) {
                self.selected_spot = Some(i);
                self.selected_removal = None;
                self.local_drag = LocalDrag::Spot(i, is_source);
            } else if let Some(uv) = self.screen_to_uv(p, img_rect, scale) {
                self.stroke_paint = mode;
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
                        let added = s.strokes.iter_mut().flat_map(|st| st.points.iter_mut());
                        for q in std::iter::once(&mut s.target).chain(s.path.iter_mut()).chain(added).chain(std::iter::once(&mut s.source)) {
                            *q = [q[0] + d[0], q[1] + d[1]];
                        }
                    }
                }
            });
        }
        let preview = self.extend_paint(ui, response, img_rect, scale, (b.size * 0.3).max(3.0));
        if !preview.is_empty() {
            paint_area(&painter, &preview, b.size, if self.stroke_paint == AreaPaint::Shrink { ERASED } else { Color32::WHITE });
        }
        if let Some(pts) = self.finished_paint(ui) {
            // An Alt-click (not a stroke): the source goes there.
            let alt_click = ui.input(|i| i.modifiers.alt) && pts.iter().all(|p| (map.map(*p) - map.map(pts[0])).length() < 4.0);
            match (self.stroke_paint, selected) {
                _ if alt_click => match selected {
                    Some(i) => {
                        let at = *pts.last().unwrap_or(&pts[0]);
                        self.edit(|a| {
                            if let Some(s) = a.retouch.get_mut(i) {
                                s.source = at;
                            }
                        });
                    }
                    None => self.next_source = Some(pts[0]),
                },
                (AreaPaint::Extend | AreaPaint::Shrink, Some(i)) => {
                    let erase = self.stroke_paint == AreaPaint::Shrink;
                    self.edit(|a| {
                        if let Some(s) = a.retouch.get_mut(i) {
                            s.strokes.push(SpotStroke { points: pts, erase });
                        }
                    });
                }
                _ => {
                    let radius = self.radius_norm(b.size, scale);
                    let source = match self.next_source.take() {
                        Some(s) => s,
                        None => self.current.as_ref().map_or(pts[0], |c| mask::auto_source_area(&c.preview, &pts, radius)),
                    };
                    let mode = if tool == LocalTool::Heal { SpotMode::Heal } else { SpotMode::Clone };
                    let (feather, opacity) = (1.0 - b.hardness.clamp(0.0, 1.0), b.opacity);
                    let spot = Spot { mode, target: pts[0], source, path: pts[1..].to_vec(), strokes: Vec::new(), radius, feather, opacity };
                    self.edit(|a| a.retouch.push(spot));
                    self.selected_spot = self.current.as_ref().map(|c| c.adjustments.retouch.len() - 1);
                    self.selected_removal = None;
                }
            }
        }
    }

    /// How a stroke starting now acts: on a chosen area (`chosen`) Shift extends it and Alt
    /// takes part of it away; otherwise as chosen in the panel.
    pub(crate) fn stroke_mode(&self, ui: &Ui, chosen: bool) -> AreaPaint {
        if !chosen {
            return AreaPaint::New;
        }
        let m = ui.input(|i| i.modifiers);
        if m.alt {
            AreaPaint::Shrink
        } else if m.shift {
            AreaPaint::Extend
        } else {
            self.area_paint
        }
    }

    /// Magic Brush: paint over what should be selected; the region of similar, connected
    /// colors around the stroke becomes the layer's selection.
    fn magic_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let painter = ui.painter_at(response.rect);
        let b = self.brushes.magic.clone();
        if let Some(p) = response.hover_pos().filter(|_| !self.adjusting_brush()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            brush_cursor(&painter, p, &b);
        }
        if let Some(p) = pressed_on(ui, response)
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            self.local_drag = LocalDrag::Paint(vec![uv], p);
        }
        let preview = self.extend_paint(ui, response, img_rect, scale, (b.size * 0.3).max(3.0));
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
        if let Some(layer) = self.ensure_layer() {
            self.request_ai_on(layer, kind);
        }
    }

    /// Starts a content-based mask computation for layer `index`.
    fn request_ai_on(&mut self, index: usize, kind: crate::workers::AiKind) {
        let Some(layer) = self.layer_id(index) else { return };
        if !matches!(kind, crate::workers::AiKind::Object { .. }) {
            self.object_session = None;
        }
        let Some(cur) = &self.current else { return };
        self.ai_busy = true;
        self.status = "Berechne Maske …".into();
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

    /// Everything that shapes the mask, top to bottom: painting tools, whole-mask actions,
    /// automatic selection from the image content, limiting to a brightness or color range.
    fn mask_body(&mut self, ui: &mut Ui, changed: &mut bool) {
        self.tool_tiles(ui, &[LocalTool::Brush, LocalTool::Eraser, LocalTool::Linear, LocalTool::Radial, LocalTool::Magic, LocalTool::Object]);
        let mut ai_action = None;
        let mut pick = false;
        let busy = self.ai_busy;
        let subject_download = self.ai_download_progress(AiFeature::Subject);
        let subject_cancel = self.job_cancel(None, Some(&AiFeature::Subject.job_label()));
        let busy = busy || subject_download.is_some();
        let tool = self.local_tool;
        let active = self.active_layer;
        // The map only for a depth mask (looked up once per image, see `depth_mask_map`).
        let depth_mask = self.current.as_ref().is_some_and(|c| {
            active.and_then(|li| c.adjustments.layers.get(li)).is_some_and(|l| l.mask.ai.as_ref().is_some_and(|a| a.depth.is_some()))
        });
        let depth_map = if depth_mask { self.current_path().and_then(|p| self.depth_mask_map(&p)) } else { None };
        let depth_bins = if depth_mask { self.depth_mask_bins() } else { None };
        let luma_bins: Option<Vec<f32>> = self.histogram.as_ref().map(|h| h.luma.iter().map(|&n| n as f32).collect());
        let show_mask = &mut self.show_mask;
        let Some(cur) = self.current.as_mut() else { return };
        // The background has no mask: its controls are shown, but disabled.
        let mut placeholder = Layer::default();
        let has_layer = active.is_some_and(|li| li < cur.adjustments.layers.len());
        let mut layer = Some(active.and_then(|li| cur.adjustments.layers.get_mut(li)).unwrap_or(&mut placeholder));

        if let Some(layer) = layer.as_deref_mut() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if !has_layer {
                    ui.disable();
                }
                if ui.button("Füllen").on_hover_text("Maske aufs ganze Bild; unter „Eingrenzen“ auf Helligkeit oder Farben beschränken").clicked() {
                    layer.mask = Mask { fill: true, ..Default::default() };
                    *changed = true;
                }
                if ui.button("Leeren").clicked() {
                    layer.mask = Mask::default();
                    *changed = true;
                }
                *changed |= crate::widgets::checkbox(ui, &mut layer.mask.invert, "Invertieren").changed();
                if ui.selectable_label(*show_mask, crate::commands::hint("Maske anzeigen", "show-mask")).clicked() {
                    *show_mask = !*show_mask;
                    *changed = true;
                }
            });
        }

        subheading(ui, "Automatisch auswählen");
        ui.horizontal_wrapped(|ui| {
            let ai = |ui: &Ui, text: &str| crate::widgets::ai_text(ui, text);
            if ui.add_enabled(!busy, egui::Button::new(ai(ui, &format!("{}  Motiv", crate::theme::icon::USER)))).on_hover_text(format!("Motiv auswählen\n\n{}", crate::widgets::AI_HINT)).clicked() {
                ai_action = Some(1);
            }
            if ui.add_enabled(!busy, egui::Button::new(ai(ui, &format!("{}  Himmel", crate::theme::icon::CLOUD)))).on_hover_text(format!("Himmel auswählen\n\n{}", crate::widgets::AI_HINT)).clicked() {
                ai_action = Some(2);
            }
            if ui
                .add_enabled(!busy, egui::Button::new(ai(ui, &format!("{}  Tiefe", crate::theme::icon::MOUNTAINS))))
                .on_hover_text(format!("Wählt nach Entfernung aus (anfangs den nahen Teil des Bildes); der Bereich lässt sich danach einstellen\n\n{}", crate::widgets::AI_HINT))
                .clicked()
            {
                ai_action = Some(6);
            }
        });
        if let Some(layer) = layer.as_deref_mut() {
            if let Some(range) = layer.mask.ai.as_ref().and_then(|a| a.depth) {
                let far = depth_map.as_ref().map_or(sk_core::params::DepthRange::default().far, |m| m.1);
                if let Some(r) = App::depth_mask_controls(ui, &range, far, depth_bins.as_deref().map(|b| b.as_slice()))
                    && let Some((map, _)) = &depth_map
                {
                    layer.mask.ai = Some(sk_core::depth::range_mask_cached(map, &r));
                    *changed = true;
                }
            }
            if ui.add_enabled(has_layer && layer.mask.ai.is_some(), egui::Button::new("Auswahl entfernen")).clicked() {
                layer.mask.ai = None;
                *changed = true;
            }
        }
        if let Some(p) = subject_download {
            crate::ai_models::download_progress(ui, AiFeature::Subject, p, subject_cancel);
        }

        if let Some(layer) = layer {
            subheading(ui, "Eingrenzen");
            ui.scope(|ui| {
                if !has_layer {
                    ui.disable();
                }
                let m = &mut layer.mask;
                let mut luma_on = m.luma_range.is_some();
                if crate::widgets::checkbox(ui, &mut luma_on, "Helligkeitsbereich").changed() {
                    m.luma_range = luma_on.then(Default::default);
                    *changed = true;
                }
                // Switched off: the range is shown, but disabled.
                let mut r = m.luma_range.unwrap_or_default();
                ui.add_enabled_ui(luma_on, |ui| {
                    *changed |= crate::widgets::luma_range_editor(ui, &mut r, luma_bins.as_deref());
                });
                if luma_on {
                    m.luma_range = Some(r);
                }
                let mut color_on = m.color_range.is_some();
                if crate::widgets::checkbox(ui, &mut color_on, "Farbbereich").changed() {
                    m.color_range = color_on.then(Default::default);
                    *changed = true;
                }
                let mut c = m.color_range.unwrap_or_default();
                ui.add_enabled_ui(color_on, |ui| {
                    if ui.add(egui::Button::new(LocalTool::ColorPick.label()).selected(tool == Some(LocalTool::ColorPick))).clicked() {
                        pick = true;
                    }
                    let rail = hue_rail();
                    *changed |= slider_row_with(ui, "Farbton", &mut c.hue, 0.0..=360.0, 30.0, 0, SliderOpts { suffix: "°", rail: Some(&rail), ..Default::default() });
                    *changed |= slider_row_with(ui, "Breite", &mut c.width, 2.0..=90.0, 25.0, 0, SliderOpts { suffix: "°", ..Default::default() });
                    *changed |= slider_row_with(ui, "Weichheit", &mut c.falloff, 0.0..=60.0, 15.0, 0, SliderOpts { suffix: "°", ..Default::default() });
                    *changed |= slider_row(ui, "Min. Chroma", &mut c.min_chroma, 0.0..=0.2, 0.03, 3);
                });
                if color_on {
                    m.color_range = Some(c);
                }
            });
        }
        match ai_action {
            // Found without a model.
            Some(2) => {
                if let Some(layer) = self.ensure_layer_named(Some("Himmel")) {
                    self.request_ai_on(layer, crate::workers::AiKind::Sky);
                }
            }
            Some(1) => {
                let kind = crate::workers::AiKind::Subject;
                let generation = self.generation();
                // The layer chosen now, even if another one is active when the model is there.
                let layer = self.ensure_layer_named(Some("Motiv")).and_then(|i| self.layer_id(i));
                self.with_ai(AiFeature::Subject, move |app| {
                    if app.generation() == generation
                        && let Some(i) = layer.and_then(|id| app.layer_index(id))
                    {
                        app.request_ai_on(i, kind);
                    }
                });
            }
            Some(6) => {
                if let Some(layer) = self.ensure_layer_named(Some("Tiefe")) {
                    self.object_session = None;
                    self.set_depth_mask(layer, None);
                }
            }
            _ => {}
        }
        if pick {
            self.set_local_tool(LocalTool::ColorPick);
        }
    }

    /// "Retuschen": everything retouched, removals first (they are applied first), then the heal
    /// and clone areas, as a list like the layers'. Select one to edit it (also in the viewer), change its settings, find a
    /// new source or delete it; settings a kind does not have are shown, but disabled.
    fn areas_section(&mut self, ui: &mut Ui) {
        #[derive(Clone, Copy)]
        enum Area {
            Removal(usize),
            Spot(usize),
        }
        enum Act {
            Select(Area),
            Delete,
            NewSource,
            /// From the context menu of an area (it becomes the chosen one first).
            DeleteAt(Area),
            SourceOf(usize),
            Clear,
        }
        let mut act = None;
        let mut changed = false;
        let (selected_spot, selected_removal) = (self.selected_spot, self.selected_removal);
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        let (spots, removals) = (&mut adj.retouch, &mut adj.removals);
        use crate::commands::hint;
        let help = format!(
            "Alle Retuschen dieses Bildes. Eine wählen, um sie zu ändern; im Bild ist sie dann markiert. ↺ im Kopf löscht alle.\n\nPins ziehen: Bereich (weiß) oder Quelle (gelb) verschieben · {}\n\nGewählten Bereich ändern: mit Umschalt dazumalen, mit Alt wegnehmen; entfernte Bereiche werden dann neu gefüllt.",
            hint("Löschen", "delete-spot")
        );
        let area_paint = &mut self.area_paint;
        let mut reset = false;
        let count = spots.len() + removals.len();
        crate::widgets::list_section(ui, RETOUCH_SECTION, &help, count, Some(&mut || reset = true), |ui| {
            // The settings of the chosen area; without one they are shown, but disabled.
            let mut spot_placeholder = Spot { mode: SpotMode::Heal, target: [0.0; 2], source: [0.0; 2], path: Vec::new(), strokes: Vec::new(), radius: 0.0, feather: 0.4, opacity: 1.0 };
            let mut removal_placeholder = sk_core::params::Removal::default();
            let names: Vec<(Area, &str, &str, f32)> = (0..removals.len())
                .map(|i| (Area::Removal(i), icon::SPARKLE, "Entfernen", removals[i].opacity))
                .chain((0..spots.len()).map(|i| match spots[i].mode {
                    SpotMode::Heal => (Area::Spot(i), icon::BANDAIDS, "Reparieren", spots[i].opacity),
                    SpotMode::Clone => (Area::Spot(i), icon::STAMP, "Klonen", spots[i].opacity),
                }))
                .collect();
            let spot = selected_spot.and_then(|i| spots.get_mut(i));
            let removal = selected_removal.and_then(|i| removals.get_mut(i)).filter(|_| spot.is_none());
            let (is_spot, is_removal) = (spot.is_some(), removal.is_some());
            let has = is_spot || is_removal;
            let s = spot.unwrap_or(&mut spot_placeholder);
            let r = removal.unwrap_or(&mut removal_placeholder);
            // As with the layers: the opacity of the chosen one above the list.
            ui.add_enabled_ui(has, |ui| {
                let opacity = if is_removal { &mut r.opacity } else { &mut s.opacity };
                changed |= crate::widgets::unit_percent_row(ui, "Deckkraft", opacity, 0.0..=1.0, 1.0, Default::default());
            });
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (n, (area, ic, kind, opacity)) in names.iter().enumerate() {
                    let chosen = match area {
                        Area::Removal(i) => selected_removal == Some(*i),
                        Area::Spot(i) => selected_spot == Some(*i),
                    };
                    let name = format!("{kind} {}", n + 1);
                    let (rect, resp) = crate::widgets::list_row(ui, LIST_ROW, chosen, &name);
                    let p = ui.painter();
                    // The kind where the layers have their eye.
                    let icon_rect = Rect::from_min_size(rect.min + vec2(4.0, (LIST_ROW - 22.0) / 2.0), vec2(22.0, 22.0));
                    p.text(icon_rect.center(), egui::Align2::CENTER_CENTER, *ic, egui::FontId::proportional(15.0), theme::TEXT_DIM);
                    p.text(pos2(icon_rect.right() + 6.0, rect.center().y), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(13.0), theme::TEXT);
                    let percent = format!("{:.0} %", opacity * 100.0);
                    p.text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, percent, egui::FontId::proportional(11.5), theme::TEXT_DIM);
                    if resp.clicked() {
                        act = Some(Act::Select(*area));
                    }
                    crate::widgets::context_menu(&resp, |ui| {
                        if let Area::Spot(i) = area
                            && ui.button(format!("{}  Neue Quelle", icon::MAGIC_WAND)).clicked()
                        {
                            act = Some(Act::SourceOf(*i));
                        }
                        if ui.button(format!("{}  Löschen", icon::TRASH)).clicked() {
                            act = Some(Act::DeleteAt(*area));
                        }
                    });
                }
            });
            if names.is_empty() {
                crate::widgets::hint(ui, "Noch keine: mit Reparieren, Klonen oder Entfernen über eine Stelle malen.");
            }
            ui.add_space(4.0);
            // Below the list, as with the layers: what acts on the chosen one.
            ui.horizontal(|ui| {
                let source = ui.add_enabled(is_spot, egui::Button::new(RichText::new(icon::MAGIC_WAND).size(15.0)));
                if source.on_hover_text("Neue Quelle: erneut automatisch wählen").clicked() {
                    act = Some(Act::NewSource);
                }
                let delete = ui.add_enabled(has, egui::Button::new(RichText::new(icon::TRASH).size(15.0)));
                if delete.on_hover_text(crate::commands::hint("Gewählte Retusche löschen", "delete-spot")).clicked() {
                    act = Some(Act::Delete);
                }
            });
            ui.add_space(6.0);
            ui.scope(|ui| {
                if !has {
                    ui.disable();
                }
                ui.add_enabled_ui(!is_removal, |ui| {
                    crate::widgets::field(ui, "Art", |ui| {
                        changed |= ui.selectable_value(&mut s.mode, SpotMode::Heal, "Reparieren").changed();
                        changed |= ui.selectable_value(&mut s.mode, SpotMode::Clone, "Klonen").changed();
                    });
                    let mut feather = s.feather * 100.0;
                    if slider_row(ui, "Weichheit", &mut feather, 0.0..=100.0, 40.0, 0) {
                        s.feather = feather / 100.0;
                        changed = true;
                    }
                });
                area_paint_row(ui, area_paint, has);
            });
        });
        if reset {
            act = Some(Act::Clear);
        }
        if changed {
            self.request_render();
        }
        // The context menu acts on its area as the panel buttons act on the chosen one.
        let act = match act {
            Some(Act::DeleteAt(Area::Removal(i))) => {
                (self.selected_spot, self.selected_removal) = (None, Some(i));
                Some(Act::Delete)
            }
            Some(Act::DeleteAt(Area::Spot(i))) => {
                (self.selected_spot, self.selected_removal) = (Some(i), None);
                Some(Act::Delete)
            }
            Some(Act::SourceOf(i)) => {
                (self.selected_spot, self.selected_removal) = (Some(i), None);
                Some(Act::NewSource)
            }
            a => a,
        };
        match act {
            Some(Act::Select(Area::Removal(i))) => {
                self.selected_removal = Some(i);
                self.selected_spot = None;
                // The outlines are shown with the tool.
                if self.local_tool != Some(LocalTool::Remove) {
                    self.set_local_tool(LocalTool::Remove);
                }
            }
            Some(Act::Select(Area::Spot(i))) => {
                self.selected_spot = Some(i);
                self.selected_removal = None;
                // The pins are shown with the heal and clone tools.
                if !matches!(self.local_tool, Some(LocalTool::Heal | LocalTool::Clone)) {
                    let heal = self.current.as_ref().and_then(|c| c.adjustments.retouch.get(i)).is_some_and(|s| s.mode == SpotMode::Heal);
                    self.set_local_tool(if heal { LocalTool::Heal } else { LocalTool::Clone });
                }
            }
            Some(Act::Delete) => {
                // The list shows a chosen heal or clone area before a chosen removal.
                if self.selected_spot.is_some() {
                    self.selected_removal = None;
                    self.delete_selected_spot();
                } else {
                    self.delete_selected_removal();
                }
            }
            Some(Act::NewSource) => {
                let Some(i) = self.selected_spot else { return };
                let Some(cur) = self.current.as_ref() else { return };
                let Some(s) = cur.adjustments.retouch.get(i) else { return };
                let added = s.strokes.iter().filter(|st| !st.erase).flat_map(|st| st.points.iter().copied());
                let pts: Vec<[f32; 2]> = std::iter::once(s.target).chain(s.path.iter().copied()).chain(added).collect();
                let source = mask::auto_source_area(&cur.preview, &pts, s.radius);
                self.edit(|a| a.retouch[i].source = source);
            }
            Some(Act::Clear) => {
                self.selected_spot = None;
                self.selected_removal = None;
                self.edit(|a| {
                    a.retouch.clear();
                    a.removals.clear();
                });
            }
            // Turned into Delete and NewSource above.
            Some(Act::DeleteAt(_) | Act::SourceOf(_)) | None => {}
        }
    }

    /// Looks for dust and blemishes in the background (see `sk_core::spots`).
    pub fn find_spots(&mut self) {
        let Some(cur) = &self.current else { return };
        if self.spot_search.is_some() {
            return;
        }
        let (path, preview, wb, existing) = (cur.meta.path.clone(), cur.preview.clone(), cur.meta.as_shot_wb, cur.adjustments.retouch.clone());
        let sensitivity = self.spot_sensitivity;
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.workers.context();
        // Once (a failure disconnects the channel, see `poll_spot_search`).
        std::thread::spawn(move || {
            let found = sk_core::spots::find(&preview, wb, sensitivity, 60);
            let _ = tx.send((path, sk_core::spots::heal_spots(&preview, &found, &existing)));
            ctx.request_repaint();
        });
        self.spot_search = Some(rx);
        // Cancelling drops the receiver: the result of the search is not used.
        self.spot_cancel = Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        self.status = "Suche Staub und Flecken …".into();
    }

    /// Adds the spots found once the search is done (on the image it was made for).
    pub(crate) fn poll_spot_search(&mut self) {
        let Some(rx) = &self.spot_search else { return };
        let (path, spots) = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.spot_search = None;
                self.report_error("Suche nach Flecken fehlgeschlagen");
                return;
            }
        };
        self.spot_search = None;
        if self.current_path().as_ref() != Some(&path) {
            return;
        }
        let n = spots.len();
        self.status = match n {
            0 => "Keine Flecken gefunden".into(),
            1 => "1 Fleck gefunden – zum Prüfen sind die Stellen markiert".into(),
            _ => format!("{n} Flecken gefunden – zum Prüfen sind die Stellen markiert"),
        };
        if n > 0 {
            self.edit(|a| a.retouch.extend(spots));
            self.selected_spot = None;
            // The pins show where the spots are.
            if !matches!(self.local_tool, Some(LocalTool::Heal | LocalTool::Clone)) {
                self.set_local_tool(LocalTool::Heal);
            }
        }
    }

    /// Entf: deletes what is chosen – a gradient, a retouch or removal area, or else the chosen
    /// layer in the layer tab. All of it can be undone.
    pub fn delete_selected_spot(&mut self) {
        if self.delete_selected_gradient() {
            return;
        }
        if self.selected_removal.is_some() && (self.tab == Tab::Retouch || self.local_tool == Some(LocalTool::Remove)) {
            self.delete_selected_removal();
            return;
        }
        // A retouch area only while it can be seen (retouch tab or its tools), otherwise an
        // invisible area would go instead of the chosen layer.
        let retouch = self.tab == Tab::Retouch || matches!(self.local_tool, Some(LocalTool::Heal | LocalTool::Clone));
        if let Some(i) = self.selected_spot.take().filter(|_| retouch) {
            self.edit(|a| {
                if i < a.retouch.len() {
                    a.retouch.remove(i);
                }
            });
        } else if self.tab == Tab::Layers
            && let Some(i) = self.active_layer
        {
            self.delete_layer(i);
        }
    }

    /// Removes layer `i` (undoable); the chosen one follows.
    pub fn delete_layer(&mut self, i: usize) {
        self.renaming_layer = None;
        self.active_layer = match self.active_layer {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            a => a,
        };
        self.edit(|a| {
            if i < a.layers.len() {
                a.layers.remove(i);
            }
        });
    }

    /// Layer overview: opacity of the chosen layer on top, one
    /// compact row per layer (topmost first), rename by clicking the name, + and − below.
    pub(crate) fn layer_list(&mut self, ui: &mut Ui) -> Option<LayerAction> {
        use crate::theme::{self, icon};
        let cur = self.current.as_mut()?;
        let layers = &mut cur.adjustments.layers;
        let active = self.active_layer.filter(|i| *i < layers.len());
        let renaming = &mut self.renaming_layer;
        if renaming.as_ref().is_some_and(|r| r.index >= layers.len()) {
            *renaming = None;
        }
        let mut action = None;
        let mut changed = false;
        let count = layers.len();
        let look_names: Vec<String> = self.film_looks.iter().map(|(s, _)| s.name.clone()).collect();
        let style_names: Vec<String> = self.styles.iter().map(|s| s.name.clone()).collect();
        let help = "Oberste Ebene zuerst. Klick wählt, Doppelklick benennt um, das Auge blendet aus.\n\nPinsel, Verlauf und Zauberpinsel legen selbst eine Ebene an, wenn keine gewählt ist.";
        crate::widgets::list_section(ui, "Ebenen", help, count, None, |ui| {
            // Opacity of the chosen layer; greyed out for the background.
            match active {
                Some(li) => changed |= crate::widgets::percent_row(ui, "Deckkraft", &mut layers[li].opacity, 0.0..=100.0, 100.0),
                None => {
                    ui.add_enabled_ui(false, |ui| crate::widgets::percent_row(ui, "Deckkraft", &mut 100.0, 0.0..=100.0, 100.0));
                }
            }
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.y = 1.0;
            let h = LIST_ROW;
            // The topmost layer first, the background at the bottom.
            let rows = layers.len();
            for i in (0..=rows).rev() {
                let layer = (i < rows).then_some(i);
                let selected = if layer.is_some() { active == Some(i) } else { active.is_none() };
                let name = layer.map_or("Hintergrund".to_string(), |li| layers[li].name.clone());
                let (rect, resp) = crate::widgets::list_row(ui, h, selected, &name);
                let eye_rect = Rect::from_min_size(rect.min + vec2(4.0, (h - 22.0) / 2.0), vec2(22.0, 22.0));
                let eye = layer.map(|_| ui.interact(eye_rect, ui.id().with(("eye", i)), egui::Sense::click()));
                let x = eye_rect.right() + 6.0;
                let Some(li) = layer else {
                    let p = ui.painter();
                    p.text(pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, "Hintergrund", egui::FontId::proportional(13.0), theme::TEXT_DIM);
                    if resp.clicked() {
                        action = Some(LayerAction::Select(None));
                    }
                    continue;
                };
                let l = &mut layers[li];
                let on = l.enabled;
                let p = ui.painter().clone();
                if let Some(eye) = &eye {
                    let c = if eye.hovered() { Color32::WHITE } else if on { theme::TEXT_DIM } else { theme::TEXT_FAINT };
                    let ic = if on { icon::EYE } else { icon::EYE_CLOSED };
                    p.text(eye_rect.center(), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(15.0), c);
                }
                let dim = if on { theme::TEXT_DIM } else { theme::TEXT_FAINT };
                let g = p.layout_no_wrap(format!("{:.0} %", l.opacity), egui::FontId::proportional(11.5), dim);
                let right = rect.right() - 8.0 - g.size().x;
                p.galley(pos2(right, rect.center().y - g.size().y / 2.0), g, dim);
                let name_rect = Rect::from_min_max(pos2(x, rect.top() + 3.0), pos2(right - 8.0, rect.bottom() - 3.0));
                if let Some(r) = renaming.as_mut().filter(|r| r.index == li) {
                    let edit = egui::TextEdit::singleline(&mut l.name).margin(vec2(4.0, 2.0)).desired_width(name_rect.width());
                    let mut out = ui.scope_builder(egui::UiBuilder::new().max_rect(name_rect), |ui| edit.show(ui)).inner;
                    if r.focus {
                        r.focus = false;
                        out.response.request_focus();
                        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(l.name.chars().count()));
                        out.state.cursor.set_char_range(Some(all));
                        out.state.store(ui.ctx(), out.response.id);
                    } else if out.response.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) || l.name.trim().is_empty() {
                            l.name = std::mem::take(&mut r.original);
                        }
                        *renaming = None;
                    }
                } else {
                    // Name, then what the mask consists of, faint and truncated.
                    let text_color = if on { theme::TEXT } else { theme::TEXT_FAINT };
                    let mut job = egui::text::LayoutJob::default();
                    job.append(&l.name, 0.0, egui::TextFormat::simple(egui::FontId::proportional(13.0), text_color));
                    job.append(&layer_summary(l), 8.0, egui::TextFormat::simple(egui::FontId::proportional(11.0), theme::TEXT_FAINT));
                    job.wrap = egui::text::TextWrapping::truncate_at_width(name_rect.width());
                    let g = p.layout_job(job);
                    p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, text_color);
                    let on_name = resp.interact_pointer_pos().is_some_and(|pos| name_rect.contains(pos));
                    let eye_hint = if on { "Ausblenden · Alt-Klick: nur diese Ebene" } else { "Einblenden · Alt-Klick: nur diese Ebene" };
                    if eye.as_ref().is_some_and(|r| r.clone().on_hover_text(eye_hint).clicked()) {
                        action = Some(if ui.input(|i| i.modifiers.alt) { LayerAction::Solo(li) } else { LayerAction::Toggle(li) });
                    } else if resp.double_clicked() || (resp.clicked() && selected && on_name) {
                        *renaming = Some(Rename { index: li, original: l.name.clone(), focus: true });
                        action = Some(LayerAction::Select(Some(li)));
                    } else if resp.clicked() {
                        action = Some(LayerAction::Select(Some(li)));
                    }
                    crate::widgets::context_menu(&resp, |ui| {
                        if ui.button("Umbenennen").clicked() {
                            *renaming = Some(Rename { index: li, original: l.name.clone(), focus: true });
                            action = Some(LayerAction::Select(Some(li)));
                        }
                        if ui.button(if on { "Ausblenden" } else { "Einblenden" }).clicked() {
                            action = Some(LayerAction::Toggle(li));
                        }
                        if ui.button(format!("{}  Löschen", icon::TRASH)).clicked() {
                            action = Some(LayerAction::Delete(li));
                        }
                    });
                }
            }
            if rows == 0 {
                crate::widgets::hint(ui, "Nur der Hintergrund: + oder ein Maskenwerkzeug legt eine Ebene an.");
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                crate::widgets::menu_button(ui, egui::RichText::new(icon::PLUS).size(15.0), |ui| {
                    if ui.button("Neue Ebene").on_hover_text("Leere Maske, dann malen oder auswählen").clicked() {
                        action = Some(LayerAction::Add);
                    }
                    crate::widgets::menu_button(ui, "Neue Ebene mit Stilpinsel", |ui| {
                        for (k, (name, _)) in style_brushes().iter().enumerate() {
                            if ui.button(*name).clicked() {
                                action = Some(LayerAction::AddStyle(k));
                            }
                        }
                    })
                    .response
                    .on_hover_text("Ebene mit fertigen Anpassungen, der Pinsel ist gleich gewählt");
                    crate::widgets::menu_button(ui, "Neue Ebene mit Look", |ui| {
                        for (index, name) in look_names.iter().enumerate() {
                            if ui.button(name).clicked() {
                                action = Some(LayerAction::AddLook { look: true, index });
                            }
                        }
                        if !style_names.is_empty() {
                            ui.separator();
                            for (index, name) in style_names.iter().enumerate() {
                                if ui.button(name).clicked() {
                                    action = Some(LayerAction::AddLook { look: false, index });
                                }
                            }
                        }
                    })
                    .response
                    .on_hover_text("Ebene über das ganze Bild mit einem Look oder Stil: ihre Deckkraft ist die Stärke, ihre Maske schränkt ihn ein");
                })
                .response
                .on_hover_text("Neue Ebene");
                let minus = ui.add_enabled(active.is_some(), egui::Button::new(egui::RichText::new(icon::TRASH).size(15.0)));
                if minus.on_hover_text(crate::commands::hint("Gewählte Ebene löschen", "delete-spot")).clicked()
                    && let Some(li) = active
                {
                    action = Some(LayerAction::Delete(li));
                }
            });
        });
        if changed {
            self.request_render();
        }
        action
    }

    /// Bar at the top of the develop tabs: what the tools change, the background or a layer
    /// (with the accent color), with a menu to choose and an X back to the background. Hovering
    /// it shows the layer's mask.
    pub fn edit_target_bar(&mut self, ui: &mut Ui) {
        let Some(cur) = self.current.as_ref() else { return };
        let names: Vec<String> = cur.adjustments.layers.iter().map(|l| l.name.clone()).collect();
        let active = self.active_layer.filter(|i| *i < names.len());
        let shown = active.is_some_and(|i| cur.adjustments.layers[i].enabled);
        let mut pick: Option<Option<usize>> = None;
        let mut add = false;
        let mut toggle_mask = false;
        let mut toggle_layer = false;
        // Over the buttons the mask is not shown, else what they do can't be seen.
        let mut over_buttons = false;
        let (fill, text) = if active.is_some() { (theme::ACCENT_SOFT, Color32::WHITE) } else { (theme::CONTROL, theme::TEXT_DIM) };
        let frame = egui::Frame::new().fill(fill).corner_radius(5).inner_margin(egui::Margin { left: 10, right: 4, top: 2, bottom: 2 });
        let r = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new(icon::STACK).size(14.0).color(if active.is_some() { theme::ACCENT } else { text }));
                let title = match active {
                    Some(i) => format!("Ebene „{}“", names[i]),
                    None => "Hintergrund".to_string(),
                };
                let button = egui::Button::new(RichText::new(format!("{title}  {}", icon::CARET_DOWN)).color(text)).frame_when_inactive(false);
                let hover = if active.is_some() {
                    "Die Werkzeuge ändern diese Ebene. Hier eine andere Ebene oder den Hintergrund wählen."
                } else {
                    "Die Werkzeuge ändern das ganze Bild. Hier eine Ebene wählen, um nur ihren Bereich zu ändern."
                };
                egui::containers::menu::MenuButton::from_button(button)
                    .ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate().rev() {
                            if ui.selectable_label(active == Some(i), format!("{}  {name}", icon::STACK)).clicked() {
                                pick = Some(Some(i));
                                ui.close();
                            }
                        }
                        if !names.is_empty() {
                            ui.separator();
                        }
                        if ui.selectable_label(active.is_none(), "Hintergrund").clicked() {
                            pick = Some(None);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button(format!("{}  Neue Ebene", icon::PLUS)).clicked() {
                            add = true;
                            ui.close();
                        }
                    })
                    .0
                    .on_hover_text(hover);
                if active.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close = egui::Button::new(RichText::new(icon::X).size(13.0).color(text)).frame_when_inactive(false);
                        let close = ui.add(close);
                        over_buttons |= close.hovered();
                        if close.on_hover_text(crate::commands::hint("Hintergrund bearbeiten", "edit-background")).clicked() {
                            pick = Some(None);
                        }
                        let mask = egui::Button::new(RichText::new(icon::SELECTION_FOREGROUND).size(13.0).color(text)).frame_when_inactive(false).selected(self.show_mask);
                        let mask = ui.add(mask);
                        over_buttons |= mask.hovered();
                        if mask.on_hover_text(crate::commands::hint("Maske anzeigen", "show-mask")).clicked() {
                            toggle_mask = true;
                        }
                        let ic = if shown { icon::EYE } else { icon::EYE_CLOSED };
                        let eye = egui::Button::new(RichText::new(ic).size(13.0).color(text)).frame_when_inactive(false);
                        let eye = ui.add(eye);
                        over_buttons |= eye.hovered();
                        if eye.on_hover_text(if shown { "Ebene ausblenden" } else { "Ebene einblenden" }).clicked() {
                            toggle_layer = true;
                        }
                    });
                }
            });
        });
        if active.is_some() {
            let rect = r.response.rect;
            ui.painter().rect_filled(Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())), egui::CornerRadius { nw: 5, sw: 5, ne: 0, se: 0 }, theme::ACCENT);
        }
        ui.add_space(6.0);
        let peek = active.is_some() && ui.rect_contains_pointer(r.response.rect) && !over_buttons;
        self.set_peek_mask(peek);
        if toggle_mask {
            self.show_mask = !self.show_mask;
            self.request_render();
        }
        if toggle_layer && let Some(li) = active {
            self.apply_layer_action(Some(LayerAction::Toggle(li)));
        }
        if let Some(layer) = pick {
            self.select_layer(layer);
        }
        if add {
            // A new layer has no mask yet and changes nothing: on to the mask.
            self.apply_layer_action(Some(LayerAction::Add));
            self.tab = Tab::Layers;
            crate::widgets::reveal_section(ui.ctx(), "Maske");
        }
    }

    /// Shows the active layer's mask while the pointer is over the bar above the tools.
    fn set_peek_mask(&mut self, on: bool) {
        if self.peek_mask != on {
            self.peek_mask = on;
            if !self.show_mask {
                self.request_render();
            }
        }
    }

    // ---------------------------------------------------------------- tab

    /// Three groups, top to bottom: which layer, where it acts (mask), what it does (adjustments).
    pub fn layers_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        self.prepare_subject(&ui.ctx().clone());
        let action = self.layer_list(ui);
        let mut changed = false;
        let help = "Wo die gewählte Ebene wirkt: mit Pinsel, Verlauf, Zauberpinsel oder Objekt malen bzw. auswählen.\n\n[ und ] oder Ziehen mit der rechten Maustaste: Pinselgröße und Härte. Bei Objekt wählt Umschalt+Klick ein weiteres Objekt, Alt+Klick nimmt Teile aus. Eingrenzen beschränkt auf einen Helligkeits- oder Farbbereich.";
        let mut reset_mask = false;
        crate::widgets::section_help(ui, "Maske", help, Some(&mut || reset_mask = true), |ui| self.mask_body(ui, &mut changed));
        if reset_mask && let Some(li) = self.active_layer {
            self.edit(|a| {
                if let Some(l) = a.layers.get_mut(li) {
                    l.mask = Mask::default();
                }
            });
        }

        let mut jump = None;
        let Some(cur) = self.current.as_mut() else { return };
        let mut layer = self.active_layer.and_then(|li| cur.adjustments.layers.get_mut(li));
        let mut reset = false;
        let mut on_reset = || reset = true;
        let on_reset: Option<&mut dyn FnMut()> = if layer.is_some() { Some(&mut on_reset) } else { None };
        let help = "Was die gewählte Ebene ändert. Die Regler in Belichtung, Farbe und Details wirken auf sie; \
                    ein Klick auf einen Eintrag springt zum Werkzeug.";
        crate::widgets::section_full(ui, "Anpassungen", true, Some(help), on_reset, |ui| {
            let Some(layer) = layer.as_deref_mut() else {
                crate::widgets::hint(ui, "Eine Ebene wählen oder anlegen.");
                return;
            };
            let a = &mut layer.adjustments;
            let changes = layer_changes(a);
            if changes.is_empty() {
                crate::widgets::hint(ui, "Noch keine Änderungen. Die Regler in Belichtung, Farbe und Details wirken jetzt auf diese Ebene.");
            }
            for c in changes {
                ui.horizontal(|ui| {
                    let text = format!("{}  {}", c.label, c.value);
                    if ui.add(egui::Button::new(text).frame(false)).on_hover_text(format!("Zu „{}“", c.tool.label())).clicked() {
                        jump = Some(c.tool);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::widgets::small_button(ui, crate::theme::icon::ARROW_COUNTER_CLOCKWISE, "Zurücksetzen").clicked() {
                            c.key.reset(a);
                            changed = true;
                        }
                    });
                });
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (tab, tool) in [(Tab::Exposure, Tool::Exposure), (Tab::Color, Tool::ColorBalance), (Tab::Details, Tool::Clarity)] {
                    if ui.small_button(format!("{}  {}", tab.icon(), tab.title())).on_hover_text("Werkzeuge für diese Ebene").clicked() {
                        jump = Some(tool);
                    }
                }
            });
        });
        if reset && let Some(layer) = layer {
            layer.adjustments = LayerAdjustments::default();
            changed = true;
        }
        if changed {
            self.request_render();
        }
        if let Some(tool) = jump {
            self.tab = tab_of(tool);
            crate::widgets::reveal_section(ui.ctx(), tool.label());
        }
        self.apply_layer_action(action);
    }

    /// Retouching: heal, clone and generative removal work on the image, not on a layer.
    pub fn retouch_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        use crate::commands::{hint, shortcut_of};
        let help = format!(
            "{}: übernimmt Struktur aus der Umgebung und gleicht Farbe und Helligkeit an. {}: kopiert eine Quelle 1:1. {}: die KI füllt die Stelle passend zur Umgebung. Jeweils über die Stelle malen.\n\n{} und {} oder Ziehen mit der rechten Maustaste: Pinselgröße und Härte.\n\nStaub und Flecken finden: repariert kleine dunkle Flecken auf ruhigen Flächen, etwa Sensorstaub.",
            hint("Reparieren", "tool-heal"),
            hint("Klonen", "tool-clone"),
            hint("Entfernen", "tool-remove"),
            shortcut_of("brush-smaller"),
            shortcut_of("brush-larger")
        );
        let downloading = self.ai_download_progress(AiFeature::Inpaint);
        let cancel = self.job_cancel(None, Some(&AiFeature::Inpaint.job_label()));
        let mut find = false;
        crate::widgets::section_help(ui, "Werkzeuge", &help, None, |ui| {
            self.tool_tiles(ui, &[LocalTool::Heal, LocalTool::Clone, LocalTool::Remove]);
            if let Some(p) = downloading {
                crate::ai_models::download_progress(ui, AiFeature::Inpaint, p, cancel);
            }
            ui.add_space(6.0);
            let searching = self.spot_search.is_some();
            ui.horizontal(|ui| {
                if ui.add_enabled(!searching, egui::Button::new(format!("{}  Staub und Flecken finden", crate::theme::icon::BROOM))).clicked() {
                    find = true;
                }
                if searching {
                    if let Some(c) = &self.spot_cancel {
                        crate::widgets::cancel_button(ui, c);
                    }
                }
            });
            let mut percent = self.spot_sensitivity * 100.0;
            if slider_row(ui, "Empfindlichkeit", &mut percent, 0.0..=100.0, 50.0, 0) {
                self.spot_sensitivity = percent / 100.0;
            }
        });
        if find {
            self.find_spots();
        }
        self.areas_section(ui);
    }

    /// The tool tiles (three per row) and the settings of the active one.
    fn tool_tiles(&mut self, ui: &mut Ui, tools: &[LocalTool]) {
        let gap = 4.0;
        let w = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
        let mut tool_click = None;
        for row in tools.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &t in row {
                    let (ic, name, id) = t.tile();
                    let key = crate::commands::get(id).map(|c| c.shortcut()).unwrap_or_default();
                    let hover = crate::commands::hint(name, id);
                    let ai = matches!(t, LocalTool::Object | LocalTool::Remove);
                    let hover = if ai { format!("{hover}\n\n{}", crate::widgets::AI_HINT) } else { hover };
                    if crate::widgets::tool_tile(ui, ic, name, &key, self.local_tool == Some(t), ai, w).on_hover_text(hover).clicked() {
                        tool_click = Some(t);
                    }
                }
            });
        }
        ui.add_space(4.0);
        let mut brushes_changed = false;
        match self.local_tool.filter(|t| tools.contains(t)) {
            Some(t @ (LocalTool::Brush | LocalTool::Eraser)) => {
                brushes_changed |= brush_controls(ui, self.brushes.of(Some(t)), Some(t), true);
            }
            Some(t @ (LocalTool::Heal | LocalTool::Clone)) => {
                brushes_changed |= brush_controls(ui, self.brushes.of(Some(t)), Some(t), false);
            }
            Some(LocalTool::Remove) => {
                brushes_changed |= brush_controls(ui, &mut self.brushes.remove, Some(LocalTool::Remove), true);
            }
            Some(LocalTool::Magic) => {
                let d = Brushes::defaults_of(Some(LocalTool::Magic));
                brushes_changed |= slider_row(ui, "Größe", &mut self.brushes.magic.size, 2.0..=400.0, d.size, 0);
                crate::widgets::unit_percent_row(ui, "Toleranz", &mut self.magic_tolerance, 0.0..=1.0, 0.3, Default::default());
            }
            Some(LocalTool::Object) => self.object_controls(ui),
            _ => {}
        }
        if let Some(t) = tool_click {
            self.set_local_tool(t);
        }
        if brushes_changed {
            self.brushes_changed_at = Some(std::time::Instant::now());
        }
    }

    pub(crate) fn apply_layer_action(&mut self, action: Option<LayerAction>) {
        match action {
            Some(LayerAction::Select(i)) => {
                self.active_layer = i;
                self.request_render();
            }
            Some(LayerAction::Add) => {
                if let Some(cur) = self.current.as_mut() {
                    let n = cur.adjustments.layers.len() + 1;
                    cur.adjustments.layers.push(Layer { name: format!("Ebene {n}"), ..Default::default() });
                    self.active_layer = Some(cur.adjustments.layers.len() - 1);
                    self.request_render();
                }
            }
            Some(LayerAction::AddStyle(k)) => {
                if let Some((name, adj)) = style_brushes().into_iter().nth(k) {
                    self.add_style_brush(name, adj);
                }
            }
            Some(LayerAction::AddLook { look, index }) => {
                let style = if look { self.film_looks.get(index).map(|(s, _)| s.clone()) } else { self.styles.get(index).cloned() };
                if let Some(s) = style {
                    self.add_look_layer(&s, look);
                }
            }
            Some(LayerAction::Toggle(i)) => {
                self.edit(|a| {
                    if let Some(l) = a.layers.get_mut(i) {
                        l.enabled = !l.enabled;
                    }
                });
            }
            Some(LayerAction::Solo(i)) => {
                self.edit(|a| {
                    let alone = a.layers.iter().enumerate().all(|(j, l)| l.enabled == (j == i));
                    for (j, l) in a.layers.iter_mut().enumerate() {
                        l.enabled = alone || j == i;
                    }
                });
            }
            Some(LayerAction::Delete(i)) => self.delete_layer(i),
            None => {}
        }
    }
}

/// A layer name being edited in the list; Escape restores `original`.
pub(crate) struct Rename {
    pub index: usize,
    pub original: String,
    /// Focus the field and select the name on the next frame.
    pub focus: bool,
}

pub(crate) enum LayerAction {
    Select(Option<usize>),
    Add,
    /// New layer with the adjustments of a style brush (index into [`style_brushes`]).
    AddStyle(usize),
    /// New layer over the whole image with a film look (`look`, index into `film_looks`) or a
    /// style of one's own (index into `styles`).
    AddLook { look: bool, index: usize },
    Toggle(usize),
    /// Only this layer shown (Alt-click on its eye, as in Photoshop); all again if it already is.
    Solo(usize),
    Delete(usize),
}

/// The tab with a tool that works on layers.
fn tab_of(tool: Tool) -> Tab {
    match tool {
        Tool::Exposure | Tool::HighlightsShadows | Tool::Curves => Tab::Exposure,
        Tool::Clarity | Tool::Dehaze => Tab::Details,
        _ => Tab::Color,
    }
}

/// One value of a layer's adjustments (see [`layer_changes`]).
#[derive(Clone, Copy)]
enum ChangeKey {
    Field(usize),
    Band(usize),
    Wheel(usize),
    Curve(CurveChannel),
}

impl ChangeKey {
    fn reset(self, a: &mut LayerAdjustments) {
        match self {
            ChangeKey::Field(i) => *(LAYER_FIELDS[i].2)(a) = 0.0,
            ChangeKey::Band(i) => a.color_bands[i] = Default::default(),
            ChangeKey::Wheel(i) => *wheels(a)[i].1 = Default::default(),
            ChangeKey::Curve(ch) => *ch.curve_mut(&mut a.curves) = Default::default(),
        }
    }
}

struct Change {
    label: String,
    value: String,
    tool: Tool,
    key: ChangeKey,
}

type Field = fn(&mut LayerAdjustments) -> &mut f32;

/// The single values of a layer's adjustments: label, tool, field and decimals.
const LAYER_FIELDS: &[(&str, Tool, Field, usize)] = &[
    ("Belichtung", Tool::Exposure, |a| &mut a.exposure, 2),
    ("Kontrast", Tool::Exposure, |a| &mut a.contrast, 0),
    ("Helligkeit", Tool::Exposure, |a| &mut a.brightness, 0),
    ("Lichter", Tool::HighlightsShadows, |a| &mut a.highlights, 0),
    ("Tiefen", Tool::HighlightsShadows, |a| &mut a.shadows, 0),
    ("Weiß", Tool::HighlightsShadows, |a| &mut a.whites, 0),
    ("Schwarz", Tool::HighlightsShadows, |a| &mut a.blacks, 0),
    ("Temperatur", Tool::WhiteBalance, |a| &mut a.temperature, 0),
    ("Tönung", Tool::WhiteBalance, |a| &mut a.tint, 0),
    ("Sättigung", Tool::Color, |a| &mut a.saturation, 0),
    ("Dynamik", Tool::Color, |a| &mut a.vibrance, 0),
    ("Klarheit", Tool::Clarity, |a| &mut a.clarity, 0),
    ("Struktur", Tool::Clarity, |a| &mut a.structure, 0),
    ("Dunst entfernen", Tool::Dehaze, |a| &mut a.dehaze, 0),
];

fn wheels(a: &mut LayerAdjustments) -> [(&'static str, &mut sk_core::params::ColorWheel); 4] {
    let cb = &mut a.color_balance;
    [("Master", &mut cb.master), ("Schatten", &mut cb.shadows), ("Mitteltöne", &mut cb.midtones), ("Lichter", &mut cb.highlights)]
}

/// What a layer changes, in the order of the tools.
fn layer_changes(a: &LayerAdjustments) -> Vec<Change> {
    let mut a = a.clone();
    let mut out = Vec::new();
    for (i, (label, tool, field, decimals)) in LAYER_FIELDS.iter().enumerate() {
        let v = *field(&mut a);
        if v != 0.0 {
            let value = format!("{v:+.decimals$}");
            out.push(Change { label: label.to_string(), value, tool: *tool, key: ChangeKey::Field(i) });
        }
    }
    for (i, b) in a.color_bands.iter().enumerate() {
        if *b != Default::default() {
            let label = format!("Farbeditor · {}", sk_core::params::HUE_BANDS[i].0);
            let value = format!("{:+.0}° {:+.0} {:+.0}", b.hue, b.saturation, b.lightness);
            out.push(Change { label, value, tool: Tool::ColorEditor, key: ChangeKey::Band(i) });
        }
    }
    for (i, (name, w)) in wheels(&mut a).into_iter().enumerate() {
        if *w != Default::default() {
            let value = format!("{:.0} %", w.amount);
            out.push(Change { label: format!("Farbbalance · {name}"), value, tool: Tool::ColorBalance, key: ChangeKey::Wheel(i) });
        }
    }
    for ch in CurveChannel::ALL {
        let curve = ch.curve_mut(&mut a.curves);
        if !curve.is_identity() {
            let value = format!("{} Punkte", curve.points.len());
            out.push(Change { label: format!("Gradationskurve · {}", ch.label()), value, tool: Tool::Curves, key: ChangeKey::Curve(ch) });
        }
    }
    out
}

/// Short description of what a layer's mask consists of, e.g. "Pinsel · Verlauf".
fn layer_summary(l: &Layer) -> String {
    let m = &l.mask;
    let mut parts: Vec<String> = Vec::new();
    if m.fill {
        parts.push("Gefüllt".into());
    }
    if !m.strokes.is_empty() {
        parts.push("Pinsel".into());
    }
    if !m.linear.is_empty() {
        parts.push("Verlauf".into());
    }
    if !m.radial.is_empty() {
        parts.push("Radial".into());
    }
    if m.ai.is_some() {
        parts.push("KI-Maske".into());
    }
    if m.is_empty() {
        parts.push("Leere Maske".into());
    }
    if m.luma_range.is_some() || m.color_range.is_some() {
        parts.push("Bereich".into());
    }
    if m.invert {
        parts.push("Invertiert".into());
    }
    parts.join(" · ")
}

/// Outline of what a stroke takes away from a retouch or removal area.
pub(crate) const ERASED: Color32 = Color32::from_rgb(235, 90, 90);

/// What painting does with a chosen area: a new one, extend it or make it smaller.
pub(crate) fn area_paint_row(ui: &mut Ui, mode: &mut AreaPaint, chosen: bool) {
    crate::widgets::field(ui, "Malen", |ui| {
        ui.selectable_value(mode, AreaPaint::New, "Neu").on_hover_text("Ein neuer Bereich");
        ui.add_enabled_ui(chosen, |ui| {
            ui.selectable_value(mode, AreaPaint::Extend, "Erweitern").on_hover_text("Zum gewählten Bereich hinzufügen · Umschalt beim Malen");
            ui.selectable_value(mode, AreaPaint::Shrink, "Verkleinern").on_hover_text("Vom gewählten Bereich wegnehmen · Alt beim Malen");
        })
        .response
        .on_disabled_hover_text("Erst einen Bereich wählen");
    });
}

/// Brush outline: the size, and inside it where the hard core ends.
pub fn brush_cursor(painter: &egui::Painter, p: Pos2, b: &BrushSettings) {
    painter.circle_stroke(p, b.size, Stroke::new(2.5, Color32::from_black_alpha(120)));
    painter.circle_stroke(p, b.size, Stroke::new(1.2, Color32::WHITE));
    if b.hardness < 0.97 {
        painter.circle_stroke(p, b.size * b.hardness.max(0.05), Stroke::new(1.0, Color32::from_white_alpha(110)));
    }
}

/// Vertical drag (screen points) that takes the hardness over its whole range.
const BRUSH_ADJUST_HARDNESS_SPAN: f32 = 200.0;

/// Pointer distance (screen points) within which a pin of a retouch area is grabbed.
pub(crate) const PIN_HIT: f32 = 9.0;
/// Height of a row of the layer and retouch lists.
const LIST_ROW: f32 = 26.0;
/// Title of the list of retouches in the Retusche tab.
pub(crate) const RETOUCH_SECTION: &str = "Retuschen";

/// Pointer positions of this frame while dragging, in order: every movement the system
/// reported, not only where the pointer ended up (at one point per frame a fast circle comes
/// out as a polygon).
pub(crate) fn drag_trail(ui: &Ui, response: &Response) -> Vec<Pos2> {
    let mut pts: Vec<Pos2> = ui.input(|i| i.events.iter().filter_map(|e| if let egui::Event::PointerMoved(p) = e { Some(*p) } else { None }).collect());
    if let Some(p) = response.interact_pointer_pos()
        && pts.last() != Some(&p)
    {
        pts.push(p);
    }
    pts
}

/// Where the primary button went down on the viewer this frame. Tools act from the press on
/// instead of on `clicked()`, which egui drops when the mouse moves a little or the button is
/// held a moment (it then counts as a drag).
pub(crate) fn pressed_on(ui: &Ui, response: &Response) -> Option<Pos2> {
    if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) { response.interact_pointer_pos() } else { None }
}

/// Outline of a painted area: the edge of the round-capped band of radius `r` along `pts`
/// (a circle for one point). Only the outer edge is drawn, so strokes that go back and forth
/// stay one clean shape; a dark halo keeps it visible on any image.
pub(crate) fn paint_area(painter: &egui::Painter, pts: &[Pos2], r: f32, color: Color32) {
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
pub(crate) fn pin(painter: &egui::Painter, p: Pos2, color: Color32, hovered: bool) {
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

    /// The summary in the layer tab lists what a layer changes and resets each value alone.
    #[test]
    fn layer_summary_lists_and_resets_changes() {
        let mut a = LayerAdjustments { exposure: 0.4, whites: -12.0, ..Default::default() };
        a.color_bands[5].saturation = 30.0;
        a.color_balance.shadows.amount = 25.0;
        a.curves.luma.points = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        let changes = layer_changes(&a);
        let labels: Vec<String> = changes.iter().map(|c| format!("{} {}", c.label, c.value)).collect();
        assert_eq!(
            labels,
            ["Belichtung +0.40", "Weiß -12", "Farbeditor · Blau +0° +30 +0", "Farbbalance · Schatten 25 %", "Gradationskurve · Luma 3 Punkte"]
        );
        assert_eq!(tab_of(changes[2].tool), Tab::Color);
        assert_eq!(tab_of(changes[4].tool), Tab::Exposure);
        changes[0].key.reset(&mut a);
        changes[3].key.reset(&mut a);
        changes[4].key.reset(&mut a);
        assert_eq!(a.exposure, 0.0);
        assert!(a.color_balance.is_neutral());
        assert!(a.curves.is_identity());
        assert_eq!(layer_changes(&a).len(), 2);
    }

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

