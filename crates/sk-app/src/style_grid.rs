//! The styles as a grid of small previews of the current image:
//! every film look and saved style applied to the image, rendered in the background
//! (`workers::StyleThumbJob`) whenever the image or its committed adjustments change. The
//! snapshots are shown the same way (`SnapshotGrid`, a worker of their own).

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use sk_core::params::Adjustments;
use sk_core::styles::{self, Style};

use crate::app::App;

/// Smallest width of a tile in points.
const MIN_TILE: f32 = 104.0;

#[derive(Default)]
pub struct StyleGrid {
    /// What the previews show: image, its adjustments and the saved styles.
    shown: Option<(PathBuf, Adjustments, Vec<Style>)>,
    /// Identifies the current request; older results are dropped.
    key: u64,
    textures: HashMap<String, TextureHandle>,
}

/// The previews of the snapshots of the current image.
#[derive(Default)]
pub struct SnapshotGrid {
    /// What the previews show: image and the snapshots' adjustments.
    shown: Option<(PathBuf, Vec<Adjustments>)>,
    key: u64,
    textures: HashMap<String, TextureHandle>,
}

/// Id of a snapshot's preview (its place and its adjustments: a renamed one keeps it).
fn snapshot_id(i: usize, adj: &Adjustments) -> String {
    format!("{i}:{:016x}", sk_core::cache::fnv1a(serde_json::to_string(adj).unwrap_or_default().as_bytes()))
}

/// Size of the tiles of a grid as wide as `width`: columns, tile width, image height.
pub(crate) fn tile_size(width: f32) -> (usize, f32, f32) {
    let spacing = TILE_SPACING;
    let cols = ((width + spacing) / (MIN_TILE + spacing)).floor().max(2.0) as usize;
    let tile = (width - spacing * (cols - 1) as f32) / cols as f32;
    (cols, tile, tile * 0.72)
}

const TILE_SPACING: f32 = 6.0;

/// One tile: the preview (or "…" while it is rendered) and the name below; framed while the
/// pointer is on it or it is `selected`.
pub(crate) fn thumb_tile(ui: &mut Ui, texture: Option<&TextureHandle>, name: &str, tile: f32, image_h: f32, selected: bool) -> egui::Response {
    let label_h = 18.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(tile, image_h + label_h), Sense::click());
    let img_rect = Rect::from_min_size(rect.min, Vec2::new(tile, image_h));
    let painter = ui.painter_at(rect);
    painter.rect_filled(img_rect, 3.0, Color32::from_gray(24));
    match texture {
        Some(t) => {
            let [w, h] = t.size().map(|v| v as f32);
            let k = (img_rect.width() / w).min(img_rect.height() / h);
            let r = Rect::from_center_size(img_rect.center(), Vec2::new(w * k, h * k));
            painter.image(t.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.text(img_rect.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(14.0), crate::theme::TEXT_FAINT);
        }
    }
    let hot = resp.hovered();
    if hot || selected {
        let c = if hot { crate::theme::ACCENT } else { Color32::from_gray(200) };
        painter.rect_stroke(img_rect, 3.0, egui::Stroke::new(2.0, c), egui::StrokeKind::Inside);
    }
    let name_color = if hot || selected { Color32::WHITE } else { Color32::from_gray(190) };
    let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), egui::FontId::proportional(11.5), name_color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(tile);
    let galley = painter.layout_job(job);
    let name_pos = egui::pos2(rect.center().x - galley.size().x / 2.0, img_rect.max.y + 3.0);
    painter.galley(name_pos, galley, name_color);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name));
    resp
}

/// What a click or the pointer on a tile does: the style and whether it is a film look (see
/// `styles::apply_look`).
pub(crate) type Pick = (Style, bool);
/// The clicked style, the hovered one and one to delete.
pub(crate) type Picks = (Option<Pick>, Option<Pick>, Option<String>);

/// Id of a style's preview.
fn id(style: &Style, look: bool) -> String {
    format!("{}:{}", if look { "film" } else { "own" }, style.name)
}

/// The adjustments the style gives an image with `current`.
pub(crate) fn applied(style: &Style, look: bool, current: &Adjustments) -> Adjustments {
    if look { styles::apply_look(style, current) } else { styles::apply(style, current) }
}

/// [`applied`], or on the layer `layer` if one is chosen (see `styles::apply_on_layer`).
pub(crate) fn applied_to(style: &Style, look: bool, current: &Adjustments, layer: Option<usize>) -> Adjustments {
    match layer.filter(|i| *i < current.layers.len()) {
        Some(i) => {
            let mut a = current.clone();
            a.layers[i].adjustments = styles::apply_on_layer(style, look, &a.layers[i].adjustments).0;
            a
        }
        None => applied(style, look, current),
    }
}

/// What of a style a layer can't take, for messages.
fn left_out_note(left: &[&str]) -> String {
    if left.is_empty() { String::new() } else { format!(" (nur auf dem Hintergrund möglich: {})", left.join(", ")) }
}

impl App {
    /// Puts a style or look (`look`) on the layer `li` of the current image.
    pub(crate) fn apply_style_to_layer(&mut self, style: &Style, look: bool, li: usize) {
        let mut left = Vec::new();
        let mut name = String::new();
        self.edit(|a| {
            if let Some(l) = a.layers.get_mut(li) {
                (l.adjustments, left) = styles::apply_on_layer(style, look, &l.adjustments);
                name = l.name.clone();
            }
        });
        self.commit_and_save();
        self.status = format!("„{}“ auf Ebene „{name}“ angewendet{}", style.name, left_out_note(&left));
    }

    /// A new layer over the whole image with a style or look (`look`): its opacity is then the
    /// strength.
    pub(crate) fn add_look_layer(&mut self, style: &Style, look: bool) {
        if self.current.is_none() {
            return;
        }
        self.end_viewer_tools();
        let (adjustments, left) = styles::apply_on_layer(style, look, &sk_core::params::LayerAdjustments::default());
        let mut index = None;
        self.edit(|a| {
            let mask = sk_core::params::Mask { fill: true, ..Default::default() };
            a.layers.push(sk_core::params::Layer { name: style.name.clone(), adjustments, mask, ..Default::default() });
            index = Some(a.layers.len() - 1);
        });
        self.active_layer = index;
        self.commit_and_save();
        self.status = format!("Ebene „{}“ angelegt: ihre Deckkraft ist die Stärke{}", style.name, left_out_note(&left));
    }

    /// Requests new previews when the image, its committed adjustments or the styles changed.
    pub(crate) fn pump_style_thumbs(&mut self) {
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let g = &self.style_grid;
        if g.shown.as_ref().is_some_and(|(p, a, s)| *p == path && *a == cur.committed && *s == self.styles) {
            return;
        }
        let looks = self.film_looks.iter().map(|(s, _)| (s, true));
        let own = self.styles.iter().map(|s| (s, false));
        let variants = looks.chain(own).map(|(s, look)| (id(s, look), applied(s, look, &cur.committed))).collect();
        let key = self.style_grid.key + 1;
        self.workers.style_thumbs(crate::workers::StyleThumbJob { key, meta: cur.meta.clone(), source: cur.preview.clone(), variants, snapshots: false });
        // The old previews stay until the new ones replace them (no flicker while editing).
        self.style_grid.key = key;
        self.style_grid.shown = Some((path, cur.committed.clone(), self.styles.clone()));
    }

    pub(crate) fn style_thumb_arrived(&mut self, ctx: &egui::Context, key: u64, id: String, image: ColorImage) {
        if key != self.style_grid.key {
            return;
        }
        match self.style_grid.textures.get_mut(&id) {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => {
                let t = ctx.load_texture(format!("style:{id}"), image, TextureOptions::LINEAR);
                self.style_grid.textures.insert(id, t);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn style_grid_key_for_test(&self) -> u64 {
        self.style_grid.key
    }

    #[cfg(test)]
    pub(crate) fn style_grid_count_for_test(&self) -> usize {
        self.style_grid.textures.len()
    }

    /// A grid of styles; returns the clicked one, the hovered one and one to delete (own
    /// styles, from the context menu).
    pub(crate) fn style_tiles(&self, ui: &mut Ui, list: &[(Style, bool, String)]) -> Picks {
        let (mut clicked, mut hovered, mut delete) = (None, None, None);
        let (cols, tile, image_h) = tile_size(ui.available_width());
        for row in list.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = TILE_SPACING;
                for (style, look, help) in row {
                    let resp = thumb_tile(ui, self.style_grid.textures.get(&id(style, *look)), &style.name, tile, image_h, false);
                    let hot = resp.hovered();
                    let resp = resp.on_hover_text(help.as_str());
                    if hot {
                        hovered = Some((style.clone(), *look));
                    }
                    if resp.clicked() {
                        clicked = Some((style.clone(), *look));
                    }
                    if !look {
                        crate::widgets::context_menu(&resp, |ui| {
                            if ui.button("Stil löschen …").clicked() {
                                delete = Some(style.name.clone());
                                ui.close();
                            }
                        });
                    }
                }
            });
            ui.add_space(TILE_SPACING);
        }
        (clicked, hovered, delete)
    }

    /// Requests new previews of the snapshots when the image or its snapshots changed (each
    /// rendered as it was saved, on the current preview).
    pub(crate) fn pump_snapshot_thumbs(&mut self) {
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let adjs: Vec<Adjustments> = cur.snapshots.iter().map(|s| s.adjustments.clone()).collect();
        if self.snapshot_grid.shown.as_ref().is_some_and(|(p, a)| *p == path && *a == adjs) {
            return;
        }
        let variants = adjs.iter().enumerate().map(|(i, a)| (snapshot_id(i, a), a.clone())).collect();
        let key = self.snapshot_grid.key + 1;
        self.workers.snapshot_thumbs(crate::workers::StyleThumbJob { key, meta: cur.meta.clone(), source: cur.preview.clone(), variants, snapshots: true });
        self.snapshot_grid.key = key;
        self.snapshot_grid.shown = Some((path, adjs));
    }

    pub(crate) fn snapshot_thumb_arrived(&mut self, ctx: &egui::Context, key: u64, id: String, image: ColorImage) {
        if key != self.snapshot_grid.key {
            return;
        }
        match self.snapshot_grid.textures.get_mut(&id) {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => {
                let t = ctx.load_texture(format!("snapshot:{id}"), image, TextureOptions::LINEAR);
                self.snapshot_grid.textures.insert(id, t);
            }
        }
    }

    /// The preview of snapshot `i` with these adjustments, once rendered.
    pub(crate) fn snapshot_texture(&self, i: usize, adj: &Adjustments) -> Option<&TextureHandle> {
        self.snapshot_grid.textures.get(&snapshot_id(i, adj))
    }
}

