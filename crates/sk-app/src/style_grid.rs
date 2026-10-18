//! The styles as a grid of small previews of the current image:
//! every film look and saved style applied to the image, rendered in the background
//! (`workers::StyleThumbJob`) whenever the image or its committed adjustments change.

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

impl App {
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
        self.workers.style_thumbs(crate::workers::StyleThumbJob { key, meta: cur.meta.clone(), source: cur.preview.clone(), variants });
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
        let spacing = 6.0;
        let width = ui.available_width();
        let cols = ((width + spacing) / (MIN_TILE + spacing)).floor().max(2.0) as usize;
        let tile = (width - spacing * (cols - 1) as f32) / cols as f32;
        let image_h = tile * 0.72;
        let label_h = 18.0;
        for row in list.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = spacing;
                for (style, look, help) in row {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(tile, image_h + label_h), Sense::click());
                    let img_rect = Rect::from_min_size(rect.min, Vec2::new(tile, image_h));
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(img_rect, 3.0, Color32::from_gray(24));
                    match self.style_grid.textures.get(&id(style, *look)) {
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
                    if hot {
                        painter.rect_stroke(img_rect, 3.0, egui::Stroke::new(2.0, crate::theme::ACCENT), egui::StrokeKind::Inside);
                    }
                    let name_color = if hot { Color32::WHITE } else { Color32::from_gray(190) };
                    let mut job = egui::text::LayoutJob::simple_singleline(style.name.clone(), egui::FontId::proportional(11.5), name_color);
                    job.wrap = egui::text::TextWrapping::truncate_at_width(tile);
                    let galley = painter.layout_job(job);
                    let name_pos = egui::pos2(rect.center().x - galley.size().x / 2.0, img_rect.max.y + 3.0);
                    painter.galley(name_pos, galley, name_color);
                    let resp = resp.on_hover_text(help.as_str());
                    if hot {
                        hovered = Some((style.clone(), *look));
                    }
                    if resp.clicked() {
                        clicked = Some((style.clone(), *look));
                    }
                    if !look {
                        resp.context_menu(|ui| {
                            if ui.button("Stil löschen …").clicked() {
                                delete = Some(style.name.clone());
                                ui.close();
                            }
                        });
                    }
                }
            });
            ui.add_space(spacing);
        }
        (clicked, hovered, delete)
    }
}

