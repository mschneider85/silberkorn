//! Compare view: the selected images side by side with a pinned reference image and
//! synchronized zoom, and matching exposure / white balance to the reference.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

/// At most this many images are shown side by side.
pub const MAX_CELLS: usize = 12;

/// A zoomed-in part of a compare image at a higher resolution than its preview.
pub struct CompareDetail {
    /// Edit version of the image being edited it was rendered for (`None`: from the sidecar).
    pub version: Option<u64>,
    /// The part, 0..1 of the image.
    pub uv: Rect,
    /// Width of the whole image at this resolution, and whether that is the full resolution.
    pub width: f32,
    pub full_res: bool,
    pub texture: egui::TextureHandle,
    pub focus: Option<crate::focus::FocusMap>,
}

/// A part of a compare image wanted at a higher resolution (see `App::details_requested`).
#[derive(Clone, PartialEq)]
pub struct DetailWant {
    path: PathBuf,
    version: Option<u64>,
    /// The visible part, and the larger part requested (so that panning needs no new one).
    visible: Rect,
    padded: Rect,
    width: f32,
    focus: bool,
}

/// `outer` contains `inner` (up to rounding).
fn covers(outer: Rect, inner: Rect) -> bool {
    outer.expand(1e-4).contains_rect(inner)
}

impl CompareDetail {
    /// Shows `visible` sharply enough for a whole-image width of `width` on screen.
    fn serves(&self, visible: Rect, width: f32, focus: bool) -> bool {
        covers(self.uv, visible) && (self.full_res || self.width >= width * 0.9) && (!focus || self.focus.is_some())
    }
}

/// Columns and rows for `n` images of aspect `aspect` that make them as large as possible.
fn grid_shape(n: usize, avail: Vec2, aspect: f32) -> (usize, usize) {
    (1..=n.max(1))
        .map(|cols| {
            let rows = n.div_ceil(cols);
            let (w, h) = (avail.x / cols as f32, avail.y / rows as f32);
            (cols, rows, (w / aspect).min(h))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map_or((1, 1), |(c, r, _)| (c, r))
}

impl App {
    /// Images of the compare view in display order (the reference first), or none for the
    /// ordinary single view.
    pub fn compare_cells(&self) -> Vec<PathBuf> {
        let Some(primary) = self.primary_path() else { return Vec::new() };
        // Tools, before/after and proof work in the single view.
        let single = self.crop_tool.is_some()
            || self.straighten_tool.is_some()
            || self.local_tool.is_some()
            || self.picking_wb
            || self.picking_skin
            || self.picking_focus
            || self.split_view
            || self.show_before
            || self.proof_view;
        if single {
            return Vec::new();
        }
        let mut cells: Vec<PathBuf> = self.compare_ref.iter().filter(|r| self.entries.iter().any(|e| e.path == **r)).cloned().collect();
        let mut shown = if self.multi_view && self.selection.len() > 1 { self.selected_paths() } else { Vec::new() };
        if !shown.contains(&primary) {
            shown.push(primary.clone());
        }
        for p in shown {
            if !cells.contains(&p) {
                cells.push(p);
            }
        }
        if cells.len() > MAX_CELLS {
            cells.truncate(MAX_CELLS);
            if !cells.contains(&primary) {
                cells[MAX_CELLS - 1] = primary;
            }
        }
        if cells.len() < 2 {
            cells.clear();
        }
        cells
    }

    /// Pins the current image as reference, or unpins it if it already is.
    pub fn toggle_reference(&mut self) {
        let Some(path) = self.primary_path() else { return };
        if self.compare_ref.as_ref() == Some(&path) {
            self.compare_ref = None;
            self.status = "Referenzbild entfernt".into();
        } else {
            self.status = format!("Referenzbild: {}", file_name(&path));
            self.compare_ref = Some(path);
        }
    }

    /// What to show for an image of the compare view, its size (for the aspect ratio), and
    /// whether it is a preview rendering (not a stand-in such as the thumbnail).
    fn cell_texture(&self, path: &PathBuf) -> Option<(egui::TextureId, Vec2, bool)> {
        let is_current = self.current_path().as_ref() == Some(path) && self.loading.is_none();
        let live = if is_current { self.texture.as_ref().map(|t| (t.id(), self.view_size.unwrap_or(t.size_vec2()), true)) } else { None };
        let placeholder = (self.primary_path().as_ref() == Some(path)).then_some(self.placeholder.as_ref()).flatten();
        live.or_else(|| {
            [(self.compare_textures.get(path), true), (self.prepared_texture(path), true), (placeholder, false), (self.thumbs.get(path), false)]
                .into_iter()
                .find_map(|(t, preview)| t.map(|t| (t.id(), t.size_vec2(), preview)))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn detail_received(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        version: Option<u64>,
        uv: [f32; 4],
        width: f32,
        full_res: bool,
        image: egui::ColorImage,
        focus: Option<Arc<Vec<u8>>>,
    ) {
        // Answered: if the part gets dropped (an edit, memory), it is asked for again.
        self.details_requested.retain(|r| r.path != path);
        // Rendered with live adjustments that are no longer those of the image.
        let editing = self.current_path().as_ref() == Some(&path) && self.loading.is_none();
        if version.is_some_and(|v| !editing || v != self.edit_version) {
            return;
        }
        let focus = focus.map(|f| crate::focus::FocusMap::new(image.size, f));
        let uv = Rect::from_min_max(pos2(uv[0], uv[1]), pos2(uv[2], uv[3]));
        match self.compare_details.get_mut(&path) {
            Some(d) => {
                d.texture.set(image, egui::TextureOptions::LINEAR);
                (d.version, d.uv, d.width, d.full_res, d.focus) = (version, uv, width, full_res, focus);
            }
            None => {
                let texture = ctx.load_texture(format!("compare detail:{}", path.display()), image, egui::TextureOptions::LINEAR);
                self.compare_details.insert(path, CompareDetail { version, uv, width, full_res, texture, focus });
            }
        }
    }

    /// Asks for the zoomed-in parts that aren't shown sharply yet, once zooming and panning
    /// pause. Each request replaces the previous one.
    fn request_details(&mut self, wants: Vec<DetailWant>, busy: bool) {
        let pending = |w: &DetailWant| {
            self.details_requested.iter().any(|r| {
                r.path == w.path && r.version == w.version && r.focus == w.focus && covers(r.padded, w.visible) && r.width >= w.width * 0.9
            })
        };
        if busy || wants.iter().all(pending) {
            return;
        }
        let current = self.current_path().filter(|_| self.loading.is_none());
        let jobs = wants
            .iter()
            .map(|w| crate::workers::DetailJob {
                path: w.path.clone(),
                live: self
                    .current
                    .as_ref()
                    .filter(|_| current.as_ref() == Some(&w.path))
                    .map(|c| (c.meta.clone(), c.full.clone(), c.adjustments.clone(), self.edit_version)),
                uv: [w.padded.min.x, w.padded.min.y, w.padded.max.x, w.padded.max.y],
                width: w.width,
                focus: w.focus,
            })
            .collect();
        self.workers.compare_details(jobs);
        self.details_requested = wants;
    }

    /// Asks for renderings of the images that have none yet.
    pub(crate) fn request_compare_images(&mut self, cells: &[PathBuf]) {
        let current = self.primary_path();
        let missing: Vec<PathBuf> = cells
            .iter()
            .filter(|p| {
                let image = self.compare_textures.contains_key(*p) || self.prepared_texture(p).is_some();
                let focus = !self.focus_mask || self.focus_maps.contains_key(*p) || self.focus_pending.contains(*p);
                Some(*p) != current.as_ref() && !(image && focus)
            })
            .cloned()
            .collect();
        if missing.iter().any(|p| !self.compare_requested.contains(p)) {
            self.compare_requested.extend(missing.iter().cloned());
            self.workers.compare(missing, self.session.as_ref().map(|s| s.root.clone()));
        }
    }

    /// Draws the compare view into `rect`: one cell per image, all with the same zoom and
    /// image point in the center. Clicking a cell makes it the image being edited.
    pub(crate) fn compare_view(&mut self, ui: &mut Ui, rect: Rect, cells: &[PathBuf]) {
        self.request_compare_images(cells);
        self.ensure_focus_of_current();
        self.zoom_pct = None;
        self.view_magnified = false;
        let gap = 6.0;
        let aspect = self.image_size().map_or(1.5, |s| s.x / s.y.max(1.0));
        let (cols, rows) = grid_shape(cells.len(), rect.size() - Vec2::splat(gap), aspect);
        let cell_size = vec2(
            (rect.width() - gap * (cols as f32 + 1.0)) / cols as f32,
            (rect.height() - gap * (rows as f32 + 1.0)) / rows as f32,
        );
        let primary = self.primary_path();
        let ppp = self.pixels_per_point;
        let mut clicked: Option<PathBuf> = None;
        let mut toggle_ref: Option<PathBuf> = None;
        let mut wants: Vec<DetailWant> = Vec::new();
        for (i, path) in cells.iter().enumerate() {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            let cell = Rect::from_min_size(rect.min + vec2(gap + c * (cell_size.x + gap), gap + r * (cell_size.y + gap)), cell_size);
            let resp = ui.interact(cell, ui.id().with(("compare cell", i)), Sense::click_and_drag());
            let painter = ui.painter_at(cell);
            painter.rect_filled(cell, 4.0, Color32::from_gray(24));
            let area = Rect::from_min_max(cell.min + vec2(4.0, 4.0), cell.max - vec2(4.0, 22.0));
            if let Some((texture, size, preview)) = self.cell_texture(path) {
                let fit = (area.width() / size.x).min(area.height() / size.y).max(0.001);
                let scale = fit * self.compare_zoom;
                let center = if self.compare_zoom > 1.0 { self.compare_center } else { Vec2::splat(0.5) };
                let img = Rect::from_min_size(area.center() - center * size * scale, size * scale);
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                let clip = ui.painter_at(area);
                clip.image(texture, img, uv, Color32::WHITE);
                let mask = self.focus_overlay(ui.ctx(), path, size.x / size.y);
                if let Some(mask) = mask {
                    clip.image(mask, img, uv, Color32::WHITE);
                }
                // Zoomed in beyond the preview: the visible part from the full resolution.
                let width = img.width() * ppp;
                if preview && width > size.x * 1.1 {
                    let is_current = self.current_path().as_ref() == Some(path) && self.loading.is_none();
                    let version = is_current.then_some(self.edit_version);
                    let to_uv = |p: egui::Pos2| ((p - img.min) / img.size()).to_pos2();
                    let vis = area.intersect(img);
                    let visible = Rect::from_min_max(to_uv(vis.min), to_uv(vis.max));
                    let detail = self.compare_details.get_mut(path).filter(|d| !is_current || d.version == version);
                    let served = detail.as_ref().is_some_and(|d| d.serves(visible, width, self.focus_mask));
                    if let Some(d) = detail {
                        let r = Rect::from_min_max(img.min + d.uv.min.to_vec2() * img.size(), img.min + d.uv.max.to_vec2() * img.size());
                        clip.image(d.texture.id(), r, uv, Color32::WHITE);
                        let own = d.focus.as_mut().filter(|_| self.focus_mask).map(|f| f.overlay(ui.ctx(), &format!("focus detail:{}", path.display()), self.focus_sensitivity));
                        if let Some(m) = own {
                            clip.image(m, r, uv, Color32::WHITE);
                        } else if let Some(m) = mask {
                            clip.image(m, r, d.uv, Color32::WHITE);
                        }
                    }
                    if !served {
                        let pad = visible.size() * 0.5;
                        let padded = visible.expand2(pad).intersect(uv);
                        wants.push(DetailWant { path: path.clone(), version, visible, padded, width, focus: self.focus_mask });
                    }
                }
                // The image point under the pointer, 0..1.
                let point_at = |p: egui::Pos2, zoom: f32| center + (p - area.center()) / (size * fit * zoom);
                if resp.hovered()
                    && let Some(cursor) = resp.hover_pos()
                {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        let q = point_at(cursor, self.compare_zoom);
                        let zoom = (self.compare_zoom * (1.0 + scroll * 0.0015)).clamp(1.0, 16.0);
                        self.compare_center = (q - (cursor - area.center()) / (size * fit * zoom)).clamp(Vec2::ZERO, Vec2::splat(1.0));
                        self.compare_zoom = zoom;
                    }
                }
                if resp.dragged() && self.compare_zoom > 1.0 {
                    self.compare_center = (self.compare_center - resp.drag_delta() / (size * scale)).clamp(Vec2::ZERO, Vec2::splat(1.0));
                }
                if resp.double_clicked()
                    && let Some(p) = resp.interact_pointer_pos()
                {
                    if self.compare_zoom > 1.01 {
                        self.compare_zoom = 1.0;
                    } else {
                        // One preview pixel per screen pixel, at least twice the fit.
                        self.compare_center = point_at(p, 1.0).clamp(Vec2::ZERO, Vec2::splat(1.0));
                        self.compare_zoom = (1.0 / (fit * ppp)).clamp(2.0, 16.0);
                    }
                }
            }
            let is_primary = primary.as_ref() == Some(path);
            let is_ref = self.compare_ref.as_ref() == Some(path);
            let stroke = if is_primary { egui::Stroke::new(2.0, theme::ACCENT) } else { egui::Stroke::new(1.0, Color32::from_gray(60)) };
            painter.rect_stroke(cell.shrink(1.0), 4.0, stroke, egui::StrokeKind::Inside);
            let entry = self.entries.iter().find(|e| e.path == *path);
            let name = entry.map_or_else(|| file_name(path), |e| e.name.clone());
            let label = if is_ref { format!("{}  Referenz  ·  {name}", icon::PUSH_PIN) } else { name };
            painter.text(
                pos2(cell.center().x, cell.bottom() - 11.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(11.5),
                if is_primary { Color32::WHITE } else { theme::TEXT_DIM },
            );
            if let Some(e) = entry {
                widgets::paint_badges(ui, area, e.rating, e.color_label.rgb());
            }
            if resp.clicked() {
                clicked = Some(path.clone());
            }
            resp.context_menu(|ui| {
                let text = if is_ref { "Referenzbild entfernen" } else { "Als Referenzbild festlegen" };
                if ui.button(format!("{}  {text}", icon::PUSH_PIN)).clicked() {
                    toggle_ref = Some(path.clone());
                }
            });
        }
        // Not while zooming or panning: the request would be outdated a moment later.
        let busy = self.pointer_down || ui.input(|i| i.smooth_scroll_delta != Vec2::ZERO);
        self.request_details(wants, busy);
        if let Some(p) = toggle_ref {
            self.compare_ref = if self.compare_ref.as_ref() == Some(&p) { None } else { Some(p) };
        }
        if let Some(p) = clicked {
            self.make_primary(&p);
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    sk_core::variant::display_name(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_prefers_large_cells() {
        // Two landscape images on a wide screen: side by side.
        assert_eq!(grid_shape(2, vec2(1600.0, 900.0), 1.5), (2, 1));
        // ... on a tall area: one above the other.
        assert_eq!(grid_shape(2, vec2(600.0, 1200.0), 1.5), (1, 2));
        assert_eq!(grid_shape(4, vec2(1600.0, 1000.0), 1.5), (2, 2));
    }
}
