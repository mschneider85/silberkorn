//! Custom widgets: histogram, curve editor, star rating, slider rows.

use sk_core::params::Curve;
use sk_core::pipeline::Histogram;
use egui::{Color32, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

/// Collapsible tool section with an optional reset button in the header.
pub fn section(ui: &mut Ui, title: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    let id = ui.make_persistent_id(title);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).strong());
            if let Some(reset) = reset {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_button(ui, "↺", "Werkzeug zurücksetzen").clicked() {
                        reset();
                    }
                });
            }
        })
        .body(body);
    ui.add_space(4.0);
}

/// A labeled slider that resets to `default` on double-click of the label.
/// Returns true when the value changed.
pub fn slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let lbl = ui.add_sized([96.0, 18.0], egui::Label::new(label).sense(Sense::click()));
        if lbl.on_hover_text("Doppelklick: zurücksetzen").double_clicked() && *value != default {
            *value = default;
            changed = true;
        }
        let resp = ui.add(egui::Slider::new(value, range).fixed_decimals(decimals).clamping(egui::SliderClamping::Always));
        changed |= resp.changed();
    });
    changed
}

pub fn histogram(ui: &mut Ui, hist: Option<&Histogram>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Color32::from_gray(18));
    let Some(h) = hist else { return };
    // Ignore the extreme bins for scaling so clipping spikes do not flatten everything.
    let max = [&h.r, &h.g, &h.b]
        .iter()
        .flat_map(|c| c[1..255].iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let channel = |bins: &[u32]| {
        let mut pts = Vec::with_capacity(258);
        pts.push(rect.left_bottom());
        for (i, &v) in bins.iter().enumerate() {
            let x = rect.left() + rect.width() * i as f32 / 255.0;
            let y = rect.bottom() - rect.height() * ((v as f32 / max).sqrt()).min(1.0);
            pts.push(pos2(x, y));
        }
        pts.push(rect.right_bottom());
        pts
    };
    for (bins, color) in [
        (&h.r, Color32::from_rgba_unmultiplied(230, 60, 60, 90)),
        (&h.g, Color32::from_rgba_unmultiplied(60, 200, 60, 90)),
        (&h.b, Color32::from_rgba_unmultiplied(70, 110, 240, 90)),
    ] {
        let pts = channel(bins);
        // Draw as vertical bars to avoid concave polygon filling artifacts.
        for w in pts.windows(2).skip(1).take(255) {
            painter.add(Shape::convex_polygon(
                vec![pos2(w[0].x, rect.bottom()), w[0], w[1], pos2(w[1].x, rect.bottom())],
                color,
                Stroke::NONE,
            ));
        }
    }
    let luma = channel(&h.luma);
    painter.add(Shape::line(luma[1..luma.len() - 1].to_vec(), Stroke::new(1.0, Color32::from_gray(200))));
    // clipping indicators
    let total: u32 = h.luma.iter().sum::<u32>().max(1);
    let hi = h.r[255].max(h.g[255]).max(h.b[255]) as f32 / total as f32;
    let lo = h.luma[0] as f32 / total as f32;
    if hi > 0.001 {
        painter.circle_filled(rect.right_top() + vec2(-8.0, 8.0), 4.0, Color32::from_rgb(255, 60, 60));
    }
    if lo > 0.001 {
        painter.circle_filled(rect.left_top() + vec2(8.0, 8.0), 4.0, Color32::from_rgb(60, 120, 255));
    }
}

/// Interactive curve editor. Click to add a point, drag to move, right-click to remove.
pub fn curve_editor(ui: &mut Ui, curve: &mut Curve, color: Color32, hist: Option<&[u32]>) -> bool {
    let size = ui.available_width().min(280.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(4.0));
    painter.rect_filled(rect, 2.0, Color32::from_gray(22));
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let s = Stroke::new(1.0, Color32::from_gray(45));
        painter.line_segment([pos2(rect.left() + rect.width() * t, rect.top()), pos2(rect.left() + rect.width() * t, rect.bottom())], s);
        painter.line_segment([pos2(rect.left(), rect.top() + rect.height() * t), pos2(rect.right(), rect.top() + rect.height() * t)], s);
    }
    if let Some(bins) = hist {
        let max = bins[1..255].iter().copied().max().unwrap_or(1).max(1) as f32;
        for (i, &v) in bins.iter().enumerate() {
            let x = rect.left() + rect.width() * i as f32 / 255.0;
            let h = rect.height() * (v as f32 / max).sqrt().min(1.0) * 0.6;
            painter.line_segment([pos2(x, rect.bottom()), pos2(x, rect.bottom() - h)], Stroke::new(1.5, Color32::from_gray(38)));
        }
    }
    painter.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_gray(60)));

    let to_screen = |p: [f32; 2]| pos2(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.height());
    let from_screen = |p: Pos2| {
        [((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0), ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0)]
    };

    let mut changed = false;
    let drag_id = response.id.with("drag_point");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    let hit = |pos: Pos2, pts: &[[f32; 2]]| pts.iter().position(|p| to_screen(*p).distance(pos) < 8.0);

    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos()
    {
        dragging = hit(pos, &curve.points).or_else(|| {
            curve.points.push(from_screen(pos));
            changed = true;
            Some(curve.points.len() - 1)
        });
    }
    if response.dragged()
        && let (Some(i), Some(pos)) = (dragging, response.interact_pointer_pos())
        && i < curve.points.len()
    {
        let mut p = from_screen(pos);
        // Endpoints stay on the left/right border and only move vertically.
        let x = curve.points[i][0];
        if x == 0.0 || x == 1.0 {
            p[0] = x;
        } else {
            p[0] = p[0].clamp(0.001, 0.999);
        }
        if curve.points[i] != p {
            curve.points[i] = p;
            changed = true;
        }
    }
    if response.drag_stopped() {
        dragging = None;
    }
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
        && hit(pos, &curve.points).is_none()
    {
        curve.points.push(from_screen(pos));
        changed = true;
    }
    if response.secondary_clicked()
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(i) = hit(pos, &curve.points)
        && curve.points[i][0] > 0.0
        && curve.points[i][0] < 1.0
    {
        curve.points.remove(i);
        changed = true;
    }
    ui.data_mut(|d| match dragging {
        Some(i) => {
            d.insert_temp(drag_id, i);
        }
        None => d.remove::<usize>(drag_id),
    });

    let lut = curve.build_lut(128);
    let line: Vec<Pos2> = lut.iter().enumerate().map(|(i, &y)| to_screen([i as f32 / 127.0, y])).collect();
    painter.add(Shape::line(line, Stroke::new(2.0, color)));
    for p in &curve.points {
        painter.circle_filled(to_screen(*p), 4.0, color);
        painter.circle_stroke(to_screen(*p), 4.0, Stroke::new(1.0, Color32::BLACK));
    }
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(70)), StrokeKind::Inside);
    response.on_hover_text("Klicken: Punkt hinzufügen · Ziehen: verschieben · Rechtsklick: entfernen");
    changed
}

/// Star rating widget (0..5). Clicking the current rating clears it.
pub fn stars(ui: &mut Ui, rating: &mut u8, size: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for i in 1..=5u8 {
            let filled = i <= *rating;
            let text = egui::RichText::new(if filled { "★" } else { "☆" })
                .size(size)
                .color(if filled { Color32::from_rgb(240, 200, 60) } else { Color32::from_gray(110) });
            if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() {
                *rating = if *rating == i { 0 } else { i };
                changed = true;
            }
        }
    });
    changed
}

/// Paints a small rating + color label overlay inside `rect`.
pub fn paint_badges(ui: &Ui, rect: Rect, rating: u8, label: Option<[u8; 3]>) {
    let painter = ui.painter_at(rect);
    if rating > 0 {
        painter.text(
            rect.left_bottom() + vec2(4.0, -3.0),
            egui::Align2::LEFT_BOTTOM,
            "★".repeat(rating as usize),
            egui::FontId::proportional(11.0),
            Color32::from_rgb(240, 200, 60),
        );
    }
    if let Some([r, g, b]) = label {
        painter.circle_filled(rect.right_bottom() + vec2(-8.0, -8.0), 5.0, Color32::from_rgb(r, g, b));
    }
}

/// Clickable color label swatch.
pub fn color_label_dot(ui: &mut Ui, label: sk_core::sidecar::ColorLabel, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let painter = ui.painter_at(rect);
    let c = rect.center();
    match label.rgb() {
        Some([r, g, b]) => {
            painter.circle_filled(c, 7.0, Color32::from_rgb(r, g, b));
        }
        None => {
            painter.circle_stroke(c, 6.5, Stroke::new(1.5, Color32::from_gray(140)));
            painter.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], Stroke::new(1.5, Color32::from_gray(140)));
        }
    }
    if selected || resp.hovered() {
        painter.circle_stroke(c, 10.0, Stroke::new(1.5, if selected { Color32::WHITE } else { Color32::from_gray(120) }));
    }
    resp.on_hover_text(label.label())
}

pub fn small_button(ui: &mut Ui, text: &str, hover: &str) -> Response {
    ui.add(egui::Button::new(text).small()).on_hover_text(hover)
}
