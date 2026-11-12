//! The Ebenen tab: the layers as cards (a picture of the mask, name, what the mask is made of,
//! opacity and eye; the chosen one open with its opacity and its changes as chips), the
//! background below (no layer yet: only it; + and the mask tools make one); the mask tools, Füllen | Leeren | Invertieren, Motiv, Himmel and Tiefe as
//! pills; Eingrenzen by brightness and color as folding rows.

use std::hash::{Hash, Hasher};

use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use sk_core::params::{Layer, Mask};

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::layers::{LayerAction, LocalTool, Rename};
use crate::output::{fold_card, truncated};
use crate::theme::{self, icon};
use crate::widgets::{self, BarButton, SliderOpts};

/// Size of the mask pictures in the cards.
const THUMB: egui::Vec2 = vec2(46.0, 32.0);
/// Edge of the chosen card.
const CHOSEN_EDGE: Color32 = Color32::from_rgb(61, 95, 138);
const CHOSEN_FILL: Color32 = Color32::from_rgb(38, 48, 61);

/// What the pills under "Automatisch auswählen" ask for.
#[derive(Clone, Copy, PartialEq)]
enum Auto {
    Subject,
    Sky,
    Depth,
    /// The AI part of the mask removed.
    Remove,
}

impl App {
    pub fn layers_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            widgets::no_image(ui);
            return;
        }
        self.prepare_subject(&ui.ctx().clone());
        let (action, jump) = self.layer_cards(ui);
        self.mask_section(ui);
        self.narrow_section(ui);
        ui.add_space(12.0);
        if let Some(tool) = jump {
            self.tab = crate::layers::tab_of(tool);
            widgets::reveal_section(ui.ctx(), tool.label());
        }
        self.apply_layer_action(action);
    }

    /// "Ebenen": the cards, top layer first, the background below; with no layer, ways to start.
    fn layer_cards(&mut self, ui: &mut Ui) -> (Option<LayerAction>, Option<sk_core::tools::Tool>) {
        let look_names: Vec<String> = self.film_looks.iter().map(|(s, _)| s.name.clone()).collect();
        let style_names: Vec<String> = self.styles.iter().map(|s| s.name.clone()).collect();
        let active = self.active_layer.filter(|i| self.current.as_ref().is_some_and(|c| *i < c.adjustments.layers.len()));
        let mut action = None;
        widgets::nav_header(ui, "Ebenen", "Ebenen", |ui| {
            let del = egui::Button::new(RichText::new(icon::TRASH).size(14.0).color(theme::TEXT_DIM)).frame_when_inactive(false).min_size(vec2(24.0, 22.0));
            if ui.add_enabled(active.is_some(), del).on_hover_text(crate::commands::hint("Gewählte Ebene löschen", "delete-spot")).clicked()
                && let Some(li) = active
            {
                action = Some(LayerAction::Delete(li));
            }
            let plus = widgets::menu_button(ui, RichText::new(icon::PLUS).size(15.0).color(theme::TEXT_DIM), |ui| {
                if let Some(a) = new_layer_menu(ui, &look_names, &style_names) {
                    action = Some(a);
                }
            });
            let plus = plus.response.on_hover_text("Neue Ebene");
            crate::tour::target(ui.ctx(), "new-layer", plus.rect);
        });

        let aspect = self.current.as_ref().map_or(1.5, |c| c.preview.width as f32 / c.preview.height.max(1) as f32);
        let mut jump = None;
        let mut changed = false;
        let count = self.current.as_ref().map_or(0, |c| c.adjustments.layers.len());
        // The pictures first (they need the app), then the cards.
        let thumbs: Vec<Option<egui::TextureId>> = (0..count)
            .map(|i| {
                let layer = self.current.as_ref().map(|c| c.adjustments.layers[i].clone())?;
                Some(self.mask_thumb(ui.ctx(), &layer, aspect))
            })
            .collect();
        let renaming = &mut self.renaming_layer;
        let Some(cur) = self.current.as_mut() else { return (action, jump) };
        let layers = &mut cur.adjustments.layers;
        if renaming.as_ref().is_some_and(|r| r.index >= layers.len()) {
            *renaming = None;
        }
        for li in (0..layers.len()).rev() {
            let selected = active == Some(li);
            ui.push_id(("layer-card", li), |ui| {
                let edge = selected.then_some(CHOSEN_EDGE);
                let frame = egui::Frame::new()
                    .fill(if selected { CHOSEN_FILL } else { theme::CARD })
                    .corner_radius(10.0)
                    .stroke(Stroke::new(1.0, edge.unwrap_or(Color32::TRANSPARENT)))
                    .inner_margin(egui::Margin { left: 8, right: 6, top: 6, bottom: 6 })
                    .outer_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 0 });
                frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some(a) = layer_row(ui, li, &mut layers[li], selected, thumbs[li], renaming) {
                        action = Some(a);
                    }
                    if selected {
                        ui.add_space(4.0);
                        let l = &mut layers[li];
                        changed |= widgets::percent_row(ui, "Deckkraft", &mut l.opacity, 0.0..=100.0, 100.0);
                        if let Some(t) = changes_chips(ui, l, &mut changed) {
                            jump = Some(t);
                        }
                        ui.add_space(4.0);
                    }
                });
            });
            ui.add_space(6.0);
        }
        let bg = self.thumbs.get(self.current.as_ref().map(|c| &c.meta.path).unwrap_or(&std::path::PathBuf::new())).map(|t| t.id());
        if background_row(ui, active.is_none(), bg) {
            action = Some(LayerAction::Select(None));
        }
        if changed {
            self.request_render();
        }
        (action, jump)
    }

    /// "Maske": the tools, Füllen | Leeren | Invertieren with the mask shown, the automatic
    /// selections; for the background shown, but disabled where they need a layer.
    fn mask_section(&mut self, ui: &mut Ui) {
        let active = self.active_layer.filter(|i| self.current.as_ref().is_some_and(|c| *i < c.adjustments.layers.len()));
        let has_layer = active.is_some();
        let mut reset = false;
        widgets::nav_header(ui, "Maske", "Maske", |ui| {
            let b = egui::Button::new(RichText::new(icon::ARROW_COUNTER_CLOCKWISE).size(13.0).color(theme::TEXT_DIM)).frame_when_inactive(false).min_size(vec2(24.0, 22.0));
            if ui.add_enabled(has_layer, b).on_hover_text("Maske zurücksetzen").clicked() {
                reset = true;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 4.0);
                self.tool_tiles(ui, &[LocalTool::Brush, LocalTool::Eraser, LocalTool::Linear, LocalTool::Radial, LocalTool::Magic, LocalTool::Object]);
            });
        });
        let layer = active.and_then(|li| self.current.as_ref().map(|c| c.adjustments.layers[li].clone()));
        let invert = layer.as_ref().is_some_and(|l| l.mask.invert);
        let picked = ui
            .horizontal(|ui| {
                ui.add_space(4.0);
                let b = [
                    BarButton::new("", "Füllen").hover("Maske aufs ganze Bild; mit Eingrenzen auf Helligkeit oder Farben beschränken").enabled(has_layer),
                    BarButton::new("", "Leeren").hover("Maske löschen").enabled(has_layer),
                    BarButton::new("", "Invertieren").selected(invert).enabled(has_layer),
                    BarButton::new(icon::SELECTION_FOREGROUND, "").name("Maske anzeigen").hover(crate::commands::hint("Maske anzeigen", "show-mask")).selected(self.show_mask).enabled(has_layer).weight(0.5),
                ];
                widgets::button_bar(ui, "mask-ops", &b)
            })
            .inner;
        ui.add_space(8.0);
        let mut edit_mask: Option<Box<dyn FnOnce(&mut Mask)>> = None;
        match picked {
            Some(0) => edit_mask = Some(Box::new(|m| *m = Mask { fill: true, ..Default::default() })),
            Some(1) => edit_mask = Some(Box::new(|m| *m = Mask::default())),
            Some(2) => edit_mask = Some(Box::new(|m| m.invert = !m.invert)),
            Some(3) => self.show_mask = !self.show_mask,
            _ => {}
        }
        if reset {
            edit_mask = Some(Box::new(|m| *m = Mask::default()));
        }

        // Motiv, Himmel, Tiefe.
        let ai = layer.as_ref().and_then(|l| l.mask.ai.clone());
        let busy = self.ai_busy || self.ai_download_shown(AiFeature::Subject).is_some();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Automatisch auswählen").size(11.5).color(theme::TEXT_DIM));
        });
        ui.add_space(2.0);
        let mut auto = None;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = ((ui.available_width() - 8.0 - 12.0) / 3.0).max(50.0);
            let pills = [
                (Auto::Subject, "Motiv", icon::USER, ai.as_ref().is_some_and(|a| a.kind == "subject" && a.depth.is_none()), "Motiv auswählen"),
                (Auto::Sky, "Himmel", icon::CLOUD, ai.as_ref().is_some_and(|a| a.kind == "sky"), "Himmel auswählen"),
                (Auto::Depth, "Tiefe", icon::MOUNTAINS, ai.as_ref().is_some_and(|a| a.depth.is_some()), "Wählt nach Entfernung aus, anfangs den nahen Teil des Bildes"),
            ];
            for (what, name, ic, on, hover) in pills {
                let hover = if on { "Auswahl entfernen".to_string() } else { format!("{hover}\n\n{}", widgets::AI_HINT) };
                if ai_pill(ui, name, ic, on, !busy || on, w).on_hover_text(hover).clicked() {
                    auto = Some(if on { Auto::Remove } else { what });
                }
            }
        });
        // An AI part none of the pills stands for (object, magic brush, mixed): removable too.
        if let Some(a) = ai.as_ref().filter(|a| a.depth.is_none() && a.kind != "subject" && a.kind != "sky") {
            let what = match a.kind.as_str() {
                "object" => "Objekt",
                "magic" => "Zauberpinsel",
                _ => "KI-Auswahl",
            };
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                let b = egui::Button::new(RichText::new(format!("{what}  {}", icon::X)).size(12.0)).corner_radius(11.0);
                if ui.add(b).on_hover_text("Diese Auswahl entfernen; Pinselstriche und Verläufe bleiben").clicked() {
                    auto = Some(Auto::Remove);
                }
            });
        }
        if let Some(range) = ai.as_ref().and_then(|a| a.depth) {
            ui.add_space(4.0);
            let path = self.current_path();
            let depth_map = path.and_then(|p| self.depth_mask_map(&p));
            let bins = self.depth_mask_bins();
            let far = depth_map.as_ref().map_or(sk_core::params::DepthRange::default().far, |m| m.1);
            if let Some(r) = App::depth_mask_controls(ui, &range, far, bins.as_deref().map(|b| b.as_slice()))
                && let Some((map, _)) = &depth_map
            {
                let new = sk_core::depth::range_mask_cached(map, &r);
                edit_mask = Some(Box::new(move |m| m.ai = Some(new)));
            }
        }
        if let Some(p) = self.ai_download_shown(AiFeature::Subject) {
            let cancel = self.job_cancel(None, Some(&AiFeature::Subject.job_label()));
            crate::ai_models::download_progress(ui, AiFeature::Subject, p, cancel);
        }
        if let (Some(f), Some(li)) = (edit_mask, active) {
            self.edit_layer_mask(li, f);
        }
        match auto {
            Some(Auto::Remove) => {
                if let Some(li) = active {
                    self.edit_layer_mask(li, |m| m.ai = None);
                }
            }
            Some(a) => self.auto_select(a),
            None => {}
        }
    }

    /// "Eingrenzen": brightness and color, each a folding row with a switch.
    fn narrow_section(&mut self, ui: &mut Ui) {
        let active = self.active_layer.filter(|i| self.current.as_ref().is_some_and(|c| *i < c.adjustments.layers.len()));
        let has_layer = active.is_some();
        widgets::nav_header(ui, "Eingrenzen", "Eingrenzen", |_| {});
        let luma_bins: Option<Vec<f32>> = self.histogram.as_ref().map(|h| h.luma.iter().map(|&n| n as f32).collect());
        let tool = self.local_tool;
        let mut placeholder = Layer::default();
        let mut pick = false;
        let mut changed = false;
        {
            let Some(cur) = self.current.as_mut() else { return };
            let layer = active.and_then(|li| cur.adjustments.layers.get_mut(li)).unwrap_or(&mut placeholder);
            let m = &mut layer.mask;
            ui.add_enabled_ui(has_layer, |ui| {
                let mut luma_on = m.luma_range.is_some();
                let r = m.luma_range.unwrap_or_default();
                let line = if luma_on { format!("{:.0}–{:.0} %", r.low * 100.0, r.high * 100.0) } else { "Aus".into() };
                let mut r = r;
                let was_on = luma_on;
                fold_card(ui, "Helligkeit", icon::CIRCLE_HALF, &line, Some((&mut luma_on, has_layer, "Helligkeitsbereich")), |ui| {
                    ui.add_enabled_ui(was_on, |ui| {
                        changed |= widgets::luma_range_editor(ui, &mut r, luma_bins.as_deref());
                    });
                });
                if luma_on != m.luma_range.is_some() {
                    changed = true;
                }
                m.luma_range = luma_on.then_some(r);
                ui.add_space(6.0);

                let mut color_on = m.color_range.is_some();
                let mut c = m.color_range.unwrap_or_default();
                let line = if color_on { format!("{} ±{:.0}°", hue_name(c.hue), c.width) } else { "Aus".into() };
                let was_on = color_on;
                fold_card(ui, "Farbe", icon::PALETTE, &line, Some((&mut color_on, has_layer, "Farbbereich")), |ui| {
                    ui.add_enabled_ui(was_on, |ui| {
                        let b = egui::Button::new(format!("{}  {}", icon::EYEDROPPER, LocalTool::ColorPick.label())).selected(tool == Some(LocalTool::ColorPick));
                        if ui.add(b).on_hover_text("Auf eine Farbe im Bild klicken").clicked() {
                            pick = true;
                        }
                        let rail = widgets::hue_rail();
                        changed |= widgets::slider_row_with(ui, "Farbton", &mut c.hue, 0.0..=360.0, 30.0, 0, SliderOpts { suffix: "°", rail: Some(&rail), ..Default::default() });
                        changed |= widgets::slider_row_with(ui, "Breite", &mut c.width, 2.0..=90.0, 25.0, 0, SliderOpts { suffix: "°", ..Default::default() });
                        changed |= widgets::slider_row_with(ui, "Weichheit", &mut c.falloff, 0.0..=60.0, 15.0, 0, SliderOpts { suffix: "°", ..Default::default() });
                        changed |= widgets::slider_row(ui, "Min. Chroma", &mut c.min_chroma, 0.0..=0.2, 0.03, 3);
                    });
                });
                if color_on != m.color_range.is_some() {
                    changed = true;
                }
                m.color_range = color_on.then_some(c);
            });
        }
        if changed {
            self.request_render();
        }
        if pick {
            self.set_local_tool(LocalTool::ColorPick);
        }
    }

    /// Changes the mask of layer `li` (one undo step).
    fn edit_layer_mask(&mut self, li: usize, f: impl FnOnce(&mut Mask)) {
        self.edit(|a| {
            if let Some(l) = a.layers.get_mut(li) {
                f(&mut l.mask);
            }
        });
    }

    /// Motiv, Himmel or Tiefe: on the chosen layer, or a new one named after it.
    fn auto_select(&mut self, what: Auto) {
        match what {
            Auto::Sky => {
                if let Some(layer) = self.ensure_layer_named(Some("Himmel")) {
                    self.request_ai_on(layer, crate::workers::AiKind::Sky);
                }
            }
            Auto::Subject => {
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
            Auto::Depth => {
                if let Some(layer) = self.ensure_layer_named(Some("Tiefe")) {
                    self.object_session = None;
                    self.set_depth_mask(layer, None);
                }
            }
            Auto::Remove => {}
        }
    }

    /// A small picture of a layer's mask (white where it acts), made again when the mask changes.
    fn mask_thumb(&mut self, ctx: &egui::Context, layer: &Layer, aspect: f32) -> egui::TextureId {
        let key = mask_key(&layer.mask, aspect);
        if let Some((k, t)) = self.mask_thumbs.get(&layer.id)
            && *k == key
        {
            return t.id();
        }
        let (w, h) = if aspect >= THUMB.x / THUMB.y { (92usize, (92.0 / aspect).round().max(1.0) as usize) } else { (((64.0 * aspect).round().max(1.0)) as usize, 64usize) };
        let cover = sample_mask(&layer.mask, w, h);
        let pixels: Vec<Color32> = cover.iter().map(|v| Color32::from_gray((14.0 + v * 228.0) as u8)).collect();
        let image = egui::ColorImage::new([w, h], pixels);
        let texture = ctx.load_texture(format!("mask-thumb-{}", layer.id), image, egui::TextureOptions::LINEAR);
        let id = texture.id();
        self.mask_thumbs.insert(layer.id, (key, texture));
        id
    }
}

/// The header row of a layer card: mask picture, name and what the mask is, opacity and eye.
/// Click chooses, a click on the chosen one's name or a double click renames.
fn layer_row(ui: &mut Ui, li: usize, l: &mut Layer, selected: bool, thumb: Option<egui::TextureId>, renaming: &mut Option<Rename>) -> Option<LayerAction> {
    let mut action = None;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let on = l.enabled;
    let eye_rect = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(26.0, 26.0));
    let eye = ui.interact(eye_rect, ui.id().with("eye"), Sense::click());
    eye.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, format!("{} sichtbar", l.name)));
    let p = ui.painter().clone();
    // The mask picture.
    let tr = Rect::from_min_size(pos2(rect.left(), rect.center().y - THUMB.y / 2.0), THUMB);
    p.rect_filled(tr, 5.0, Color32::from_gray(14));
    if let Some(id) = thumb {
        let img = egui::Image::new((id, THUMB)).corner_radius(5.0).tint(if on { Color32::WHITE } else { Color32::from_gray(110) });
        img.paint_at(ui, tr);
    }
    p.rect_stroke(tr, 5.0, Stroke::new(1.0, if selected { theme::ACCENT } else { theme::BORDER_STRONG }), StrokeKind::Outside);
    // Eye and opacity on the right.
    let eye_color = if eye.hovered() {
        Color32::WHITE
    } else if on {
        theme::TEXT_DIM
    } else {
        theme::TEXT_FAINT
    };
    p.text(eye_rect.center(), egui::Align2::CENTER_CENTER, if on { icon::EYE } else { icon::EYE_CLOSED }, egui::FontId::proportional(15.0), eye_color);
    // The opacity of a closed card (the chosen one has its slider).
    let mut right = eye_rect.left() - 4.0;
    if !selected {
        let dim = if on { theme::TEXT_DIM } else { theme::TEXT_FAINT };
        let g = p.layout_no_wrap(format!("{:.0} %", l.opacity), egui::FontId::proportional(11.5), dim);
        right -= g.size().x;
        p.galley(pos2(right, rect.center().y - g.size().y / 2.0), g, dim);
    }
    let x = tr.right() + 10.0;
    let name_rect = Rect::from_min_max(pos2(x, rect.top() + 1.0), pos2(right - 8.0, rect.top() + 20.0));
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
        let color = if on { Color32::WHITE } else { theme::TEXT_FAINT };
        let name = truncated(ui, l.name.clone(), 13.5, color, name_rect.width());
        p.galley(pos2(x, rect.top() + 3.0), name, color);
    }
    // What the mask is made of, with the AI mark.
    let (summary, ai) = mask_summary(l);
    let mut sx = x;
    if ai {
        p.text(pos2(sx + 5.0, rect.top() + 28.0), egui::Align2::CENTER_CENTER, icon::SPARKLE, egui::FontId::proportional(11.0), theme::AI);
        sx += 14.0;
    }
    let g = truncated(ui, summary, 11.5, theme::TEXT_DIM, right - 8.0 - sx);
    p.galley(pos2(sx, rect.top() + 28.0 - g.size().y / 2.0), g, theme::TEXT_DIM);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, format!("Ebene {}", l.name)));

    let on_name = resp.interact_pointer_pos().is_some_and(|pos| name_rect.contains(pos));
    let eye_hint = if on { "Ausblenden · Alt-Klick: nur diese Ebene" } else { "Einblenden · Alt-Klick: nur diese Ebene" };
    if eye.on_hover_text(eye_hint).clicked() {
        action = Some(if ui.input(|i| i.modifiers.alt) { LayerAction::Solo(li) } else { LayerAction::Toggle(li) });
    } else if renaming.is_none() && (resp.double_clicked() || (resp.clicked() && selected && on_name)) {
        *renaming = Some(Rename { index: li, original: l.name.clone(), focus: true });
        action = Some(LayerAction::Select(Some(li)));
    } else if resp.clicked() {
        action = Some(LayerAction::Select(Some(li)));
    }
    widgets::context_menu(&resp, |ui| {
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
    action
}

/// The chosen layer's changes as chips: a click on one goes to its tool, × resets it.
fn changes_chips(ui: &mut Ui, l: &mut Layer, changed: &mut bool) -> Option<sk_core::tools::Tool> {
    let changes = crate::layers::layer_changes(&l.adjustments);
    if changes.is_empty() {
        return None;
    }
    let mut jump = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
        for c in changes {
            let frame = egui::Frame::new()
                .fill(theme::CHROME)
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                .corner_radius(12.0)
                .inner_margin(egui::Margin { left: 9, right: 2, top: 0, bottom: 0 });
            frame.show(ui, |ui| {
                ui.set_height(22.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                let text = egui::text::LayoutJob::default();
                let mut job = text;
                job.append(&c.label, 0.0, egui::TextFormat::simple(egui::FontId::proportional(12.0), theme::TEXT_DIM));
                job.append(&c.value, 5.0, egui::TextFormat::simple(egui::FontId::proportional(12.0), Color32::WHITE));
                let label = ui.add(egui::Label::new(job).sense(Sense::click()).selectable(false));
                if label.on_hover_text(format!("Zu „{}“", c.tool.label())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    jump = Some(c.tool);
                }
                let x = egui::Button::new(RichText::new(icon::X).size(10.0).color(theme::TEXT_DIM)).frame_when_inactive(false).corner_radius(9.0).min_size(vec2(18.0, 18.0));
                if ui.add(x).on_hover_text("Zurücksetzen").clicked() {
                    c.key.reset(&mut l.adjustments);
                    *changed = true;
                }
            });
        }
    });
    jump
}

/// The entries of "+": a new layer, one with a style brush, one with a look.
fn new_layer_menu(ui: &mut Ui, look_names: &[String], style_names: &[String]) -> Option<LayerAction> {
    let mut action = None;
    if ui.button("Neue Ebene").on_hover_text("Leere Maske, dann malen oder auswählen").clicked() {
        action = Some(LayerAction::Add);
    }
    widgets::menu_button(ui, "Neue Ebene mit Stilpinsel", |ui| {
        for (k, (name, _)) in crate::layers::style_brushes().iter().enumerate() {
            if ui.button(*name).clicked() {
                action = Some(LayerAction::AddStyle(k));
            }
        }
    })
    .response
    .on_hover_text("Ebene mit fertigen Anpassungen, der Pinsel ist gleich gewählt");
    widgets::menu_button(ui, "Neue Ebene mit Look", |ui| {
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
    action
}

/// The background as a plain row (chosen: like a chosen card); true when clicked.
fn background_row(ui: &mut Ui, selected: bool, thumb: Option<egui::TextureId>) -> bool {
    let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    let rect = outer.shrink2(vec2(4.0, 0.0));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, "Hintergrund"));
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 10.0, CHOSEN_FILL);
        p.rect_stroke(rect, 10.0, Stroke::new(1.0, CHOSEN_EDGE), StrokeKind::Inside);
    } else {
        p.rect_stroke(rect, 10.0, Stroke::new(1.0, if resp.hovered() { theme::BORDER_STRONG } else { theme::BORDER }), StrokeKind::Inside);
    }
    let tr = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - THUMB.y / 2.0), THUMB);
    match thumb {
        Some(id) => egui::Image::new((id, THUMB)).corner_radius(5.0).paint_at(ui, tr),
        None => {
            ui.painter().rect_filled(tr, 5.0, theme::CONTROL);
        }
    }
    let p = ui.painter();
    p.text(pos2(tr.right() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, "Hintergrund", egui::FontId::proportional(13.5), if selected { Color32::WHITE } else { theme::TEXT });
    p.text(pos2(rect.right() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, "ganzes Bild", egui::FontId::proportional(11.5), theme::TEXT_FAINT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A pill of the automatic selections: the AI mark, icon and name; on, with × to remove.
fn ai_pill(ui: &mut Ui, name: &str, ic: &str, on: bool, enabled: bool, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, name));
    let p = ui.painter();
    let r = rect.height() / 2.0;
    let (fill, edge, text) = if on {
        (Color32::from_rgb(58, 52, 32), Color32::from_rgb(138, 116, 38), Color32::from_rgb(255, 231, 160))
    } else if resp.hovered() && enabled {
        (theme::CONTROL, theme::TEXT_DIM, Color32::WHITE)
    } else {
        (Color32::TRANSPARENT, theme::BORDER_STRONG, if enabled { theme::TEXT } else { theme::TEXT_FAINT })
    };
    p.rect_filled(rect, r, fill);
    p.rect_stroke(rect, r, Stroke::new(1.0, edge), StrokeKind::Inside);
    let mut job = egui::text::LayoutJob::default();
    job.append(icon::SPARKLE, 0.0, egui::TextFormat::simple(egui::FontId::proportional(11.0), theme::AI));
    job.append(&format!("{ic} {name}"), 5.0, egui::TextFormat::simple(egui::FontId::proportional(12.5), text));
    if on {
        job.append(icon::X, 6.0, egui::TextFormat::simple(egui::FontId::proportional(10.0), text));
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width(width - 12.0);
    let g = ui.painter().layout_job(job);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, text);
    resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Default })
}

/// "Pinsel · Helligkeit · 2 Änderungen", and whether an AI made part of the mask.
fn mask_summary(l: &Layer) -> (String, bool) {
    let m = &l.mask;
    let mut parts: Vec<String> = Vec::new();
    if m.fill {
        parts.push("Gefüllt".into());
    }
    if let Some(a) = &m.ai {
        parts.push(
            match (a.depth.is_some(), a.kind.as_str()) {
                (true, _) => "Tiefe",
                (_, "subject") => "Motiv",
                (_, "sky") => "Himmel",
                (_, "object") => "Objekt",
                (_, "magic") => "Zauberpinsel",
                _ => "KI-Auswahl",
            }
            .into(),
        );
    }
    if !m.strokes.is_empty() {
        parts.push("Pinsel".into());
    }
    if !m.linear.is_empty() {
        parts.push("Linear".into());
    }
    if !m.radial.is_empty() {
        parts.push("Radial".into());
    }
    if m.is_empty() {
        parts.push("Leere Maske".into());
    }
    if m.luma_range.is_some() {
        parts.push("Helligkeit".into());
    }
    if m.color_range.is_some() {
        parts.push("Farbe".into());
    }
    if m.invert {
        parts.push("invertiert".into());
    }
    let n = crate::layers::layer_changes(&l.adjustments).len();
    match n {
        0 => {}
        1 => parts.push("1 Änderung".into()),
        n => parts.push(format!("{n} Änderungen")),
    }
    (parts.join(" · "), m.ai.is_some())
}

/// The German name of a hue (degrees).
fn hue_name(h: f32) -> &'static str {
    const NAMES: [&str; 12] = ["Rot", "Orange", "Gelb", "Gelbgrün", "Grün", "Grün", "Cyan", "Himmelblau", "Blau", "Violett", "Magenta", "Pink"];
    NAMES[((h.rem_euclid(360.0) + 15.0) / 30.0) as usize % 12]
}

/// Identifies what a mask picture shows (not the ranges: they depend on the image).
fn mask_key(m: &Mask, aspect: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    aspect.to_bits().hash(&mut h);
    (m.fill, m.invert, m.strokes.len(), m.linear.len(), m.radial.len()).hash(&mut h);
    for s in &m.strokes {
        (s.points.len(), s.radius.to_bits(), s.erase, s.opacity.to_bits()).hash(&mut h);
        if let (Some(a), Some(b)) = (s.points.first(), s.points.last()) {
            (a[0].to_bits(), a[1].to_bits(), b[0].to_bits(), b[1].to_bits()).hash(&mut h);
        }
    }
    for g in &m.linear {
        (g.start[0].to_bits(), g.start[1].to_bits(), g.end[0].to_bits(), g.end[1].to_bits()).hash(&mut h);
    }
    for g in &m.radial {
        (g.center[0].to_bits(), g.center[1].to_bits(), g.radius_x.to_bits(), g.radius_y.to_bits(), g.feather.to_bits(), g.invert, g.angle.to_bits()).hash(&mut h);
    }
    if let Some(a) = &m.ai {
        (std::sync::Arc::as_ptr(&a.data) as usize, a.width, a.height).hash(&mut h);
    }
    h.finish()
}

/// The mask's coverage (0..1) at `w` × `h`, roughly: fill, gradients, AI bitmap and strokes
/// (as discs along their points), inverted if it is.
fn sample_mask(m: &Mask, w: usize, h: usize) -> Vec<f32> {
    let long = w.max(h) as f32;
    let mut out = vec![if m.fill { 1.0f32 } else { 0.0 }; w * h];
    let smooth = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let v = &mut out[y * w + x];
            for g in &m.linear {
                let (sx, sy) = (g.start[0] * w as f32, g.start[1] * h as f32);
                let (dx, dy) = (g.end[0] * w as f32 - sx, g.end[1] * h as f32 - sy);
                let t = ((px - sx) * dx + (py - sy) * dy) / (dx * dx + dy * dy).max(1e-6);
                *v = v.max(1.0 - smooth(t));
            }
            for g in &m.radial {
                let (cx, cy) = (g.center[0] * w as f32, g.center[1] * h as f32);
                let (rx, ry) = ((g.radius_x * long).max(0.5), (g.radius_y * long).max(0.5));
                let (sin, cos) = g.angle.sin_cos();
                let (dx, dy) = (px - cx, py - cy);
                let (lx, ly) = (dx * cos + dy * sin, dy * cos - dx * sin);
                let e = ((lx / rx).powi(2) + (ly / ry).powi(2)).sqrt();
                let f = 1.0 - g.feather.clamp(0.0, 1.0);
                let r = 1.0 - if e <= f { 0.0 } else { smooth((e - f) / (1.0 - f).max(1e-3)) };
                *v = v.max(if g.invert { 1.0 - r } else { r });
            }
            if let Some(a) = &m.ai
                && a.width > 0
                && a.height > 0
            {
                let ax = ((px / w as f32) * a.width as f32) as usize;
                let ay = ((py / h as f32) * a.height as f32) as usize;
                let s = a.data[ay.min(a.height - 1) * a.width + ax.min(a.width - 1)] as f32 / 255.0;
                *v = v.max(s);
            }
        }
    }
    for s in &m.strokes {
        let r = (s.radius * long).max(0.6);
        // Points about half a radius apart are enough at this size.
        let mut last: Option<[f32; 2]> = None;
        for p in &s.points {
            let (cx, cy) = (p[0] * w as f32, p[1] * h as f32);
            if let Some(l) = last
                && ((l[0] - cx).powi(2) + (l[1] - cy).powi(2)).sqrt() < r * 0.5
            {
                continue;
            }
            last = Some([cx, cy]);
            let (x0, x1) = ((cx - r).floor().max(0.0) as usize, ((cx + r).ceil() as usize).min(w));
            let (y0, y1) = ((cy - r).floor().max(0.0) as usize, ((cy + r).ceil() as usize).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                    let c = (1.0 - smooth((d - r * 0.6) / (r * 0.4))) * s.opacity.clamp(0.0, 1.0);
                    let v = &mut out[y * w + x];
                    *v = if s.erase { v.min(1.0 - c) } else { v.max(c) };
                }
            }
        }
    }
    if m.invert {
        for v in &mut out {
            *v = 1.0 - *v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::params::{BrushStroke, LinearGradient};

    #[test]
    fn mask_pictures_follow_the_mask() {
        let empty = sample_mask(&Mask::default(), 20, 10);
        assert!(empty.iter().all(|v| *v == 0.0));
        let filled = sample_mask(&Mask { fill: true, invert: true, ..Default::default() }, 20, 10);
        assert!(filled.iter().all(|v| *v == 0.0), "gefüllt und invertiert: nichts");
        // A linear gradient from the top down: full at the top, nothing at the bottom.
        let lin = sample_mask(&Mask { linear: vec![LinearGradient { start: [0.5, 0.0], end: [0.5, 1.0] }], ..Default::default() }, 20, 10);
        assert!(lin[0] > 0.9 && lin[9 * 20] < 0.1, "{} {}", lin[0], lin[9 * 20]);
        let stroke = BrushStroke { points: vec![[0.5, 0.5]], radius: 0.1, hardness: 1.0, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true, tolerance: 50.0 };
        let s = sample_mask(&Mask { strokes: vec![stroke], ..Default::default() }, 20, 10);
        assert!(s[5 * 20 + 10] > 0.9 && s[0] == 0.0);
        assert_ne!(mask_key(&Mask::default(), 1.5), mask_key(&Mask { fill: true, ..Default::default() }, 1.5));
        assert_eq!(hue_name(240.0), "Blau");
        assert_eq!(hue_name(355.0), "Rot");
    }
}
