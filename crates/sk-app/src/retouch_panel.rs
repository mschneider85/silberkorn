//! The Retusche tab: the three tools as tiles, "Staub und Flecken" as a folding row with
//! "Finden", and the retouches as cards like the layers (a cut-out of the place with the kind on
//! it, name, a summary and the opacity; the chosen one open with its settings). Without a
//! retouch there is no list.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use sk_core::params::SpotMode;

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::layers::{AreaPaint, LocalTool, RETOUCH_SECTION};
use crate::output::truncated;
use crate::theme::{self, icon};
use crate::widgets::{self, BarButton};

/// Size of the cut-outs in the cards (as the layers' mask pictures).
const THUMB: egui::Vec2 = vec2(46.0, 32.0);
const CHOSEN_EDGE: Color32 = Color32::from_rgb(61, 95, 138);
const CHOSEN_FILL: Color32 = Color32::from_rgb(38, 48, 61);

#[derive(Clone, Copy, PartialEq)]
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

/// Where a card's cut-out comes from: the rendered image, the part around the area (texture
/// coordinates), and for a spot its circle there.
struct CutOut {
    texture: egui::TextureId,
    uv: Rect,
    /// Center and radius of the spot, in thumb coordinates (0..1 of its size and of its width).
    ring: Option<(egui::Vec2, f32)>,
}

impl App {
    pub fn retouch_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            widgets::no_image(ui);
            return;
        }
        widgets::nav_header(ui, "Werkzeuge", "Werkzeuge", |_| {});
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 4.0);
                self.tool_tiles(ui, &[LocalTool::Heal, LocalTool::Clone, LocalTool::Remove]);
            });
        });
        if let Some(p) = self.ai_download_shown(AiFeature::Inpaint) {
            let cancel = self.job_cancel(None, Some(&AiFeature::Inpaint.job_label()));
            crate::ai_models::download_progress(ui, AiFeature::Inpaint, p, cancel);
        }
        ui.add_space(4.0);
        self.dust_row(ui);
        self.areas_section(ui);
        ui.add_space(12.0);
    }

    /// "Staub und Flecken": a folding row with the sensitivity and "Finden" on the right.
    fn dust_row(&mut self, ui: &mut Ui) {
        let searching = self.spot_search.is_some();
        let line = if searching { "Suche …".to_string() } else { format!("{:.0} %", self.spot_sensitivity * 100.0) };
        let mut find = false;
        let frame = egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(10.0)
            .inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 4 })
            .outer_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 0 });
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let w = ui.available_width();
            let open = ui
                .horizontal(|ui| {
                    let open = ui.allocate_ui(vec2(w - 70.0, 30.0), |ui| crate::output::fold_row(ui, "Staub und Flecken", icon::BROOM, &line, None)).inner;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if searching {
                            if let Some(c) = &self.spot_cancel {
                                widgets::cancel_button(ui, c);
                            }
                        } else {
                            let hover = format!("Repariert kleine dunkle Flecken auf ruhigen Flächen, etwa Sensorstaub\n\nEmpfindlichkeit {:.0} % (aufklappen zum Ändern)", self.spot_sensitivity * 100.0);
                            if ui.add(egui::Button::new("Finden").min_size(vec2(56.0, 24.0))).on_hover_text(hover).clicked() {
                                find = true;
                            }
                        }
                    });
                    open
                })
                .inner;
            if open {
                egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 2, top: 2, bottom: 6 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut percent = self.spot_sensitivity * 100.0;
                    let r = ui.add_enabled_ui(!searching, |ui| widgets::slider_row(ui, "Empfindlichkeit", &mut percent, 0.0..=100.0, 50.0, 0)).inner;
                    if r {
                        self.spot_sensitivity = percent / 100.0;
                    }
                });
            }
        });
        if find {
            self.find_spots();
        }
    }

    /// "Retuschen": every retouch as a card, removals first (they are applied first), then the
    /// heal and clone areas. Choose one to change it (also in the viewer).
    fn areas_section(&mut self, ui: &mut Ui) {
        let Some(cur) = self.current.as_ref() else { return };
        let areas: Vec<Area> =
            (0..cur.adjustments.removals.len()).map(Area::Removal).chain((0..cur.adjustments.retouch.len()).map(Area::Spot)).collect();
        if areas.is_empty() {
            return;
        }
        let cutouts: Vec<Option<CutOut>> = areas.iter().map(|a| self.cut_out(*a)).collect();
        let (selected_spot, selected_removal) = (self.selected_spot, self.selected_removal);
        // A chosen heal or clone area counts before a chosen removal (as when deleting).
        let chosen = match (selected_spot, selected_removal) {
            (Some(i), _) if i < cur.adjustments.retouch.len() => Some(Area::Spot(i)),
            (_, Some(i)) if i < cur.adjustments.removals.len() => Some(Area::Removal(i)),
            _ => None,
        };
        let mut act = None;
        widgets::nav_header(ui, RETOUCH_SECTION, &format!("{RETOUCH_SECTION} {}", areas.len()), |ui| {
            if widgets::nav_action(ui, icon::ARROW_COUNTER_CLOCKWISE, "Alle Retuschen löschen").clicked() {
                act = Some(Act::Clear);
            }
        });

        let mut changed = false;
        let area_paint = self.area_paint_now();
        let mut new_paint = None;
        let Some(cur) = self.current.as_mut() else { return };
        let adj = &mut cur.adjustments;
        for (n, (area, cut)) in areas.iter().zip(&cutouts).enumerate() {
            let selected = chosen == Some(*area);
            let (kind, ic, opacity) = match *area {
                Area::Removal(i) => ("Entfernen", icon::SCRIBBLE_LOOP, adj.removals[i].opacity),
                Area::Spot(i) => match adj.retouch[i].mode {
                    SpotMode::Heal => ("Reparieren", icon::BANDAIDS, adj.retouch[i].opacity),
                    SpotMode::Clone => ("Klonen", icon::STAMP, adj.retouch[i].opacity),
                },
            };
            let name = format!("{kind} {}", n + 1);
            let summary = match *area {
                Area::Removal(_) => "KI-Füllung".to_string(),
                Area::Spot(i) => format!("Weichheit {:.0}", adj.retouch[i].feather * 100.0),
            };
            ui.push_id(("retouch-card", n), |ui| {
                let frame = egui::Frame::new()
                    .fill(if selected { CHOSEN_FILL } else { theme::CARD })
                    .corner_radius(10.0)
                    .stroke(Stroke::new(1.0, if selected { CHOSEN_EDGE } else { Color32::TRANSPARENT }))
                    .inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 6 })
                    .outer_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 0 });
                frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let resp = card_row(ui, &name, ic, &summary, matches!(area, Area::Removal(_)), (!selected).then_some(opacity), selected, cut.as_ref());
                    if resp.clicked() {
                        act = Some(Act::Select(*area));
                    }
                    widgets::context_menu(&resp, |ui| {
                        if let Area::Spot(i) = area
                            && ui.button(format!("{}  Neue Quelle", icon::MAGIC_WAND)).clicked()
                        {
                            act = Some(Act::SourceOf(*i));
                        }
                        if ui.button(format!("{}  Löschen", icon::TRASH)).clicked() {
                            act = Some(Act::DeleteAt(*area));
                        }
                    });
                    if !selected {
                        return;
                    }
                    ui.add_space(4.0);
                    match *area {
                        Area::Removal(i) => {
                            changed |= widgets::unit_percent_row(ui, "Deckkraft", &mut adj.removals[i].opacity, 0.0..=1.0, 1.0, Default::default());
                        }
                        Area::Spot(i) => {
                            let s = &mut adj.retouch[i];
                            changed |= widgets::unit_percent_row(ui, "Deckkraft", &mut s.opacity, 0.0..=1.0, 1.0, Default::default());
                            let heal = s.mode == SpotMode::Heal;
                            let kinds = [
                                BarButton::new("", "Reparieren").selected(heal).hover("Gleicht Farbe und Helligkeit an die Umgebung an"),
                                BarButton::new("", "Klonen").selected(!heal).hover("Kopiert die Quelle 1:1"),
                            ];
                            ui.add_space(2.0);
                            let picked = widgets::button_bar(ui, "retouch-kind", &kinds);
                            ui.add_space(2.0);
                            let mode = match picked {
                                Some(0) => Some(SpotMode::Heal),
                                Some(1) => Some(SpotMode::Clone),
                                _ => None,
                            };
                            if let Some(m) = mode.filter(|m| *m != s.mode) {
                                s.mode = m;
                                changed = true;
                            }
                            let mut feather = s.feather * 100.0;
                            if widgets::slider_row(ui, "Weichheit", &mut feather, 0.0..=100.0, 40.0, 0) {
                                s.feather = feather / 100.0;
                                changed = true;
                            }
                            widgets::field(ui, "Quelle", |ui| {
                                let b = egui::Button::new(format!("{}  Neu wählen", icon::MAGIC_WAND));
                                if ui.add(b).on_hover_text("Erneut automatisch wählen; im Bild lässt sich der gelbe Pin auch ziehen").clicked() {
                                    act = Some(Act::NewSource);
                                }
                            });
                        }
                    }
                    // Pinsel and Radierer improve this area; off (a click on the one that is on), each stroke makes a new one.
                    let paints = [
                        BarButton::new(icon::PAINT_BRUSH, "Pinsel").selected(area_paint == AreaPaint::Extend).hover("Zu dieser Retusche dazumalen · Umschalt beim Malen\nAus: jeder Strich wird eine neue Retusche"),
                        BarButton::new(icon::ERASER, "Radierer").selected(area_paint == AreaPaint::Shrink).hover("Von dieser Retusche wegnehmen · Alt beim Malen\nAus: jeder Strich wird eine neue Retusche"),
                    ];
                    ui.add_space(2.0);
                    let pick = |m: AreaPaint| if area_paint == m { AreaPaint::New } else { m };
                    match widgets::button_bar(ui, "retouch-paint", &paints) {
                        Some(0) => new_paint = Some(pick(AreaPaint::Extend)),
                        Some(1) => new_paint = Some(pick(AreaPaint::Shrink)),
                        _ => {}
                    }
                    ui.add_space(6.0);
                    crate::output::card_buttons(ui, |ui| {
                        if ui.add(crate::output::card_button(icon::TRASH, "Löschen")).on_hover_text(crate::commands::hint("Diese Retusche löschen", "delete-spot")).clicked() {
                            act = Some(Act::Delete);
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if changed {
            self.request_render();
        }
        if let Some(m) = new_paint {
            self.set_area_paint(m);
        }
        self.apply_retouch_act(act);
    }

    fn apply_retouch_act(&mut self, act: Option<Act>) {
        // The context menu acts on its area as the header buttons act on the chosen one.
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
                // A chosen heal or clone area before a chosen removal.
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
                let source = sk_core::mask::auto_source_area(&cur.preview, &pts, s.radius);
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

    /// The part of the rendered image around an area, a little larger than it, in the shape of
    /// the card's picture.
    fn cut_out(&self, area: Area) -> Option<CutOut> {
        let texture = self.texture.as_ref()?;
        let cur = self.current.as_ref()?;
        let map = self.uv_mapper(Rect::from_min_size(egui::Pos2::ZERO, vec2(1.0, 1.0)), 1.0)?;
        let (out_w, out_h) = map.out_size();
        if out_w < 2.0 || out_h < 2.0 {
            return None;
        }
        let (pts, radius): (Vec<egui::Pos2>, f32) = match area {
            Area::Spot(i) => {
                let s = cur.adjustments.retouch.get(i)?;
                let added = s.strokes.iter().filter(|st| !st.erase).flat_map(|st| st.points.iter());
                let pts = std::iter::once(&s.target).chain(&s.path).chain(added).map(|p| map.map(*p)).collect();
                (pts, self.radius_screen(s.radius, 1.0))
            }
            Area::Removal(i) => {
                let r = cur.adjustments.removals.get(i)?;
                let [x, y, w, h] = r.rect;
                let pts: Vec<egui::Pos2> = if w > 0.0 && h > 0.0 {
                    [[x, y], [x + w, y], [x, y + h], [x + w, y + h]].iter().map(|p| map.map(*p)).collect()
                } else {
                    r.strokes.iter().flat_map(|st| st.points.iter()).map(|p| map.map([p[0], p[1]])).collect()
                };
                (pts, 0.0)
            }
        };
        let bounds = Rect::from_points(&pts).expand(radius);
        if !bounds.is_finite() {
            return None;
        }
        // Some surroundings, at least a little, and the picture's shape.
        let aspect = THUMB.x / THUMB.y;
        let want = bounds.size() * 0.75 + vec2(radius, radius) * 0.5;
        let hy = want.y.max(want.x / aspect).max(14.0);
        let hy = hy * (out_w * 0.5 / (hy * aspect)).min(out_h * 0.5 / hy).min(1.0);
        let half = vec2(hy * aspect, hy);
        let c = bounds.center();
        let c = pos2(c.x.clamp(half.x, out_w - half.x), c.y.clamp(half.y, out_h - half.y));
        let view = Rect::from_center_size(c, half * 2.0);
        let uv = Rect::from_min_max(pos2(view.left() / out_w, view.top() / out_h), pos2(view.right() / out_w, view.bottom() / out_h));
        let ring = matches!(area, Area::Spot(_)).then(|| {
            let p = pts[0];
            (vec2((p.x - view.left()) / view.width(), (p.y - view.top()) / view.height()), radius / view.width())
        });
        Some(CutOut { texture: texture.id(), uv, ring })
    }
}

/// The header row of a card: cut-out with the kind on it, name, summary, and the opacity of a
/// closed card on the right. Named for tests and screen readers like the area.
#[allow(clippy::too_many_arguments)]
fn card_row(ui: &mut Ui, name: &str, ic: &str, summary: &str, ai: bool, opacity: Option<f32>, selected: bool, cut: Option<&CutOut>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    let p = ui.painter().clone();
    let tr = Rect::from_min_size(pos2(rect.left(), rect.center().y - THUMB.y / 2.0), THUMB);
    p.rect_filled(tr, 5.0, Color32::from_gray(14));
    if let Some(c) = cut {
        egui::Image::new((c.texture, THUMB)).uv(c.uv).corner_radius(5.0).paint_at(ui, tr);
        if let Some((at, r)) = c.ring {
            let center = tr.min + vec2(at.x * THUMB.x, at.y * THUMB.y);
            let r = (r * THUMB.x).max(3.0);
            p.with_clip_rect(tr).circle_stroke(center, r, Stroke::new(1.0, Color32::from_white_alpha(200)));
        }
    }
    p.rect_stroke(tr, 5.0, Stroke::new(1.0, if selected { theme::ACCENT } else { theme::BORDER_STRONG }), StrokeKind::Outside);
    // The kind, on the picture's corner.
    let badge = pos2(tr.right() - 1.0, tr.bottom() - 1.0);
    p.circle_filled(badge, 8.0, Color32::from_rgb(30, 30, 33));
    p.circle_stroke(badge, 8.0, Stroke::new(1.0, theme::BORDER_STRONG));
    p.text(badge, egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(10.0), theme::TEXT);

    let mut right = rect.right() - 2.0;
    if let Some(o) = opacity {
        let g = p.layout_no_wrap(format!("{:.0} %", o * 100.0), egui::FontId::proportional(11.5), theme::TEXT_DIM);
        right -= g.size().x;
        p.galley(pos2(right, rect.center().y - g.size().y / 2.0), g, theme::TEXT_DIM);
    }
    let x = tr.right() + 14.0;
    let g = truncated(ui, name.to_string(), 13.5, Color32::WHITE, right - 8.0 - x);
    p.galley(pos2(x, rect.top() + 3.0), g, Color32::WHITE);
    let mut sx = x;
    if ai {
        p.text(pos2(sx + 5.0, rect.top() + 28.0), egui::Align2::CENTER_CENTER, icon::SPARKLE, egui::FontId::proportional(11.0), theme::AI);
        sx += 14.0;
    }
    let g = truncated(ui, summary.to_string(), 11.5, theme::TEXT_DIM, right - 8.0 - sx);
    p.galley(pos2(sx, rect.top() + 28.0 - g.size().y / 2.0), g, theme::TEXT_DIM);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
