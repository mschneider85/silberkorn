//! Custom widgets: histogram, curve editor, star rating, slider rows.

use sk_core::params::Curve;
use sk_core::pipeline::Histogram;
use egui::{Color32, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::theme;

fn reveal_id() -> egui::Id {
    egui::Id::new("reveal_section")
}

/// Asks the tool section with this title to open, scroll into view and flash briefly the next
/// time it is drawn (used by the command palette).
pub fn reveal_section(ctx: &egui::Context, title: &str) {
    ctx.data_mut(|d| d.insert_temp(reveal_id(), title.to_string()));
}

/// Collapsible tool section with an optional reset button.
pub fn section(ui: &mut Ui, title: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_full(ui, title, true, None, reset, body);
}

/// [`section`] with an explanation behind an info icon in the header instead of text in the panel.
pub fn section_help(ui: &mut Ui, title: &str, help: &str, reset: Option<&mut dyn FnMut()>, body: impl FnOnce(&mut Ui)) {
    section_full(ui, title, true, Some(help), reset, body);
}

/// Section that starts closed unless `default_open`, with an optional help icon.
/// Flat header: a fine line above, the title on the left, help, reset and caret on the right.
pub fn section_full(
    ui: &mut Ui,
    title: &str,
    default_open: bool,
    help: Option<&str>,
    reset: Option<&mut dyn FnMut()>,
    body: impl FnOnce(&mut Ui),
) {
    let id = ui.make_persistent_id(title);
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let reveal = ui.data(|d| d.get_temp::<String>(reveal_id())).is_some_and(|t| t == title);
    let flash_id = id.with("flash");
    let now = ui.input(|i| i.time);
    if reveal {
        open = true;
        ui.data_mut(|d| {
            d.insert_persisted(id, true);
            d.remove::<String>(reveal_id());
            d.insert_temp(flash_id, now);
        });
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if reveal {
        resp.scroll_to_me(Some(egui::Align::TOP));
    }
    let painter = ui.painter();
    let hovered = resp.hovered();
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
    let inner = rect.shrink2(vec2(0.0, 2.0)).translate(vec2(0.0, 1.0));
    if hovered {
        painter.rect_filled(inner, 5.0, Color32::from_rgb(42, 42, 46));
    }
    let title_color = if open || hovered { Color32::WHITE } else { theme::TEXT };
    let galley = painter.layout_no_wrap(title.to_string(), egui::FontId::proportional(13.5), title_color);
    let title_w = galley.size().x;
    painter.galley(pos2(inner.left() + 6.0, inner.center().y - galley.size().y / 2.0), galley, title_color);
    let caret = if open { theme::icon::CARET_DOWN } else { theme::icon::CARET_RIGHT };
    let dim = if hovered { theme::TEXT } else { theme::TEXT_FAINT };
    painter.text(inner.right_center() - vec2(10.0, 0.0), egui::Align2::CENTER_CENTER, caret, egui::FontId::proportional(11.0), dim);
    // Brief accent outline after the command palette jumped here.
    if let Some(t0) = ui.data(|d| d.get_temp::<f64>(flash_id)) {
        let a = 1.0 - ((now - t0) / 1.4) as f32;
        if a > 0.0 {
            painter.rect_stroke(inner, 5.0, Stroke::new(1.5, theme::ACCENT.gamma_multiply(a)), StrokeKind::Inside);
            ui.ctx().request_repaint();
        } else {
            ui.data_mut(|d| d.remove::<f64>(flash_id));
        }
    }
    let mut header_button_clicked = false;
    if let Some(help) = help {
        let r = Rect::from_center_size(pos2(inner.left() + 6.0 + title_w + 14.0, inner.center().y), vec2(20.0, 20.0));
        let hr = ui.interact(r, id.with("help"), Sense::hover());
        let c = if hr.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, theme::icon::INFO, egui::FontId::proportional(13.0), c);
        hr.on_hover_ui(|ui| {
            ui.set_max_width(260.0);
            ui.label(help);
        });
    }
    if let Some(reset) = reset {
        let r = Rect::from_center_size(inner.right_center() - vec2(34.0, 0.0), vec2(24.0, 22.0));
        let color = if hovered || ui.rect_contains_pointer(r) { theme::TEXT } else { theme::TEXT_FAINT };
        let b = egui::Button::new(RichText::new(theme::icon::ARROW_COUNTER_CLOCKWISE).size(13.0).color(color)).frame_when_inactive(false);
        if ui.put(r, b).on_hover_text("Werkzeug zurücksetzen").clicked() {
            reset();
            header_button_clicked = true;
        }
    }
    if resp.clicked() && !header_button_clicked {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    if open {
        ui.add_space(2.0);
        egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 4, top: 0, bottom: 0 }).show(ui, body);
        ui.add_space(8.0);
    }
    ui.add_space(2.0);
}

/// Width of the label column of slider rows and form fields, so they line up.
fn label_width(ui: &Ui) -> f32 {
    (ui.available_width() * 0.32).clamp(70.0, 112.0)
}

/// Form row: a dim label in the label column, then the control filling the rest of the width.
pub fn field<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let w = label_width(ui);
        ui.allocate_ui_with_layout(vec2(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(w);
            ui.add(egui::Label::new(RichText::new(label).color(theme::TEXT_DIM)).truncate());
        });
        ui.spacing_mut().text_edit_width = ui.available_width();
        add(ui)
    })
    .inner
}

/// Drop-down list as a form row; `show` lists the choices.
pub fn combo_field(ui: &mut Ui, label: &str, selected: impl Into<egui::WidgetText>, show: impl FnOnce(&mut Ui)) {
    field(ui, label, |ui| {
        egui::ComboBox::from_id_salt(label).selected_text(selected).width(ui.available_width() - 8.0).show_ui(ui, show);
    });
}

/// Options of a [`slider_row_with`].
#[derive(Clone, Copy, Default)]
pub struct SliderOpts<'a> {
    /// Unit after the value, e.g. " %".
    pub suffix: &'static str,
    /// Colors painted along the rail (left to right) instead of the accent fill, e.g. blue to
    /// yellow for a color temperature.
    pub rail: Option<&'a [Color32]>,
}

/// Rail colors for color temperature (cool to warm).
pub const RAIL_KELVIN: &[Color32] = &[Color32::from_rgb(70, 120, 230), Color32::from_rgb(200, 200, 200), Color32::from_rgb(235, 190, 60)];
/// Rail colors for tint (green to magenta).
pub const RAIL_TINT: &[Color32] = &[Color32::from_rgb(70, 190, 90), Color32::from_rgb(200, 200, 200), Color32::from_rgb(210, 80, 200)];
/// Rail colors for a hue from 0° to 360°.
pub const RAIL_HUE: &[Color32] = &[
    Color32::from_rgb(230, 70, 70),
    Color32::from_rgb(230, 200, 60),
    Color32::from_rgb(80, 200, 80),
    Color32::from_rgb(70, 200, 210),
    Color32::from_rgb(80, 100, 230),
    Color32::from_rgb(210, 80, 210),
    Color32::from_rgb(230, 70, 70),
];
/// Rail colors for saturation (grey to colorful).
pub const RAIL_SATURATION: &[Color32] = &[Color32::from_rgb(140, 140, 140), Color32::from_rgb(220, 90, 170), Color32::from_rgb(240, 160, 40)];

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
    slider_row_with(ui, label, value, range, default, decimals, SliderOpts::default())
}

/// [`slider_row`] with a unit and/or a colored rail.
pub fn slider_row_with(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
    opts: SliderOpts<'_>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let modified = (*value - default).abs() > f32::EPSILON;
        let color = if modified { Color32::WHITE } else { theme::TEXT_DIM };
        let label_w = label_width(ui);
        let lbl = ui
            .allocate_ui_with_layout(vec2(label_w, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_width(label_w);
                ui.add(egui::Label::new(RichText::new(label).color(color)).sense(Sense::click()).truncate())
            })
            .inner;
        if lbl.on_hover_text("Doppelklick: zurücksetzen").double_clicked() && modified {
            *value = default;
            changed = true;
        }
        // Wide enough for the longest value, so the row never overflows (which would make the panel grow).
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_w = [*range.start(), *range.end()]
            .map(|v| ui.painter().layout_no_wrap(format!("{v:.decimals$}{}", opts.suffix), font.clone(), Color32::WHITE).size().x)
            .into_iter()
            .fold(0.0, f32::max);
        let value_w = (text_w + 2.0 * ui.spacing().button_padding.x + 4.0).max(44.0);
        let slider_w = (ui.available_width() - value_w - 8.0).max(60.0);
        changed |= rail_slider(ui, label, value, range.clone(), default, decimals, slider_w, opts.rail);
        // The number: plain text until hovered, then an editable field (drag or type).
        ui.scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            w.inactive.weak_bg_fill = Color32::TRANSPARENT;
            w.inactive.bg_fill = Color32::TRANSPARENT;
            w.inactive.fg_stroke.color = if modified { Color32::WHITE } else { theme::TEXT_DIM };
            let span = range.end() - range.start();
            let drag = egui::DragValue::new(value)
                .range(range.clone())
                .fixed_decimals(decimals)
                .suffix(opts.suffix)
                .speed(span as f64 / 400.0);
            changed |= ui.add_sized(vec2(value_w, 20.0), drag).changed();
        });
    });
    changed
}

/// Slim slider: a thin rail filled from the default to the value, a small round handle.
/// Click or drag to set; Shift while dragging for fine steps; double-click resets.
#[allow(clippy::too_many_arguments)]
fn rail_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    decimals: usize,
    width: f32,
    rail: Option<&[Color32]>,
) -> bool {
    let (lo, hi) = (*range.start(), *range.end());
    let span = (hi - lo).max(f32::EPSILON);
    let fill_from = default.clamp(lo, hi);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click_and_drag());
    if ui.is_enabled() && (resp.hovered() || resp.dragged()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let x0 = rect.left() + 7.0;
    let x1 = rect.right() - 7.0;
    let to_x = |v: f32| x0 + (x1 - x0) * ((v - lo) / span).clamp(0.0, 1.0);
    let from_x = |x: f32| lo + span * ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let before = *value;

    if let Some(p) = resp.interact_pointer_pos() {
        let fine = ui.input(|i| i.modifiers.shift);
        if fine && resp.dragged() && !resp.drag_started() {
            let dx = ui.input(|i| i.pointer.delta().x);
            *value = (*value + dx / (x1 - x0) * span * 0.1).clamp(lo, hi);
        } else {
            *value = from_x(p.x);
        }
        let q = 10f32.powi(decimals as i32);
        *value = (*value * q).round() / q;
    }
    if resp.has_focus() {
        let step = (span / 100.0).max(1.0 / 10f32.powi(decimals as i32));
        let n = ui.input(|i| {
            i.num_presses(egui::Key::ArrowRight) as i32 + i.num_presses(egui::Key::ArrowUp) as i32
                - i.num_presses(egui::Key::ArrowLeft) as i32
                - i.num_presses(egui::Key::ArrowDown) as i32
        });
        if n != 0 {
            *value = (*value + n as f32 * step).clamp(lo, hi);
        }
    }
    // Double-click resets.
    if resp.double_clicked() {
        *value = default;
    }
    resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *value as f64, label));

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let cy = rect.center().y;
        let rail_rect = Rect::from_min_max(pos2(x0, cy - 1.5), pos2(x1, cy + 1.5));
        match rail {
            Some(colors) if colors.len() >= 2 => {
                let mut mesh = egui::Mesh::default();
                let n = colors.len() - 1;
                for (i, c) in colors.iter().enumerate() {
                    let x = x0 + (x1 - x0) * i as f32 / n as f32;
                    let c = c.gamma_multiply(0.85);
                    mesh.colored_vertex(pos2(x, cy - 2.0), c);
                    mesh.colored_vertex(pos2(x, cy + 2.0), c);
                    if i > 0 {
                        let k = (2 * i) as u32;
                        mesh.add_triangle(k - 2, k - 1, k);
                        mesh.add_triangle(k - 1, k, k + 1);
                    }
                }
                painter.add(mesh);
            }
            _ => {
                painter.rect_filled(rail_rect, 1.5, Color32::from_rgb(64, 64, 70));
                let (a, b) = (to_x(fill_from), to_x(*value));
                if (a - b).abs() > 0.5 {
                    let fill = Rect::from_min_max(pos2(a.min(b), cy - 1.5), pos2(a.max(b), cy + 1.5));
                    painter.rect_filled(fill, 1.5, theme::ACCENT);
                }
                // A tick at the default when it lies inside the range (bipolar sliders).
                let d = to_x(default);
                if d > x0 + 1.0 && d < x1 - 1.0 {
                    painter.line_segment([pos2(d, cy - 4.0), pos2(d, cy + 4.0)], Stroke::new(1.0, Color32::from_rgb(90, 90, 98)));
                }
            }
        }
        let active = resp.dragged() || resp.is_pointer_button_down_on();
        let r = if active { 7.0 } else if resp.hovered() { 6.5 } else { 5.5 };
        let r = ui.ctx().animate_value_with_time(resp.id.with("r"), r, 0.08);
        let c = pos2(to_x(*value), cy);
        painter.circle_filled(c + vec2(0.0, 1.0), r + 1.0, Color32::from_black_alpha(90));
        painter.circle_filled(c, r, if ui.is_enabled() { Color32::from_rgb(238, 238, 242) } else { Color32::from_gray(120) });
        if resp.has_focus() {
            painter.circle_stroke(c, r + 2.5, Stroke::new(1.5, theme::ACCENT));
        }
    }
    *value != before
}

/// Slider without label and number, e.g. for the thumbnail size; double-click resets.
pub fn compact_slider(ui: &mut Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, width: f32) -> bool {
    rail_slider(ui, name, value, range, default, 2, width, None)
}

/// Tool tile: icon above the name, the key in the corner; `selected` highlights it.
pub fn tool_tile(ui: &mut Ui, ic: &str, label: &str, key: &str, selected: bool, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    let fill = if selected {
        theme::ACCENT_SOFT
    } else if hovered {
        Color32::from_rgb(54, 54, 60)
    } else {
        Color32::from_rgb(44, 44, 49)
    };
    p.rect_filled(rect, 7.0, fill);
    if selected {
        p.rect_stroke(rect, 7.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
    }
    let color = if selected || hovered { Color32::WHITE } else { theme::TEXT };
    p.text(rect.center_top() + vec2(0.0, 18.0), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(19.0), color);
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), egui::FontId::proportional(11.5), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width - 8.0);
    let galley = p.layout_job(job);
    p.galley(pos2(rect.center().x - galley.size().x / 2.0, rect.bottom() - 18.0), galley, color);
    if !key.is_empty() {
        p.text(rect.right_top() + vec2(-6.0, 5.0), egui::Align2::RIGHT_TOP, key, egui::FontId::proportional(10.0), theme::TEXT_FAINT);
    }
    resp
}

/// Small faint explanatory text.
pub fn hint(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text.into()).small().color(theme::TEXT_FAINT))
}

/// Centered placeholder for an empty view or tool: a large icon, a line of text and a hint.
pub fn empty_state(ui: &mut Ui, ic: &str, title: &str, hint: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(ic).size(34.0).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(14.0).color(theme::TEXT_DIM));
        if !hint.is_empty() {
            ui.add_space(2.0);
            ui.label(RichText::new(hint).small().color(theme::TEXT_FAINT));
        }
    });
    ui.add_space(12.0);
}

/// Placeholder of the develop tools while no image is open.
pub fn no_image(ui: &mut Ui) {
    empty_state(ui, theme::icon::IMAGE_SQUARE, "Kein Bild ausgewählt", "Ein Bild im Filmstreifen oder Raster anklicken");
}

/// Returns the area it was drawn in.
pub fn histogram(ui: &mut Ui, hist: Option<&Histogram>) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::BROWSER);
    let Some(h) = hist else { return rect };
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
    rect
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
    // Grab a point, or click elsewhere to add one.
    if response.dragged() && dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if let Some(pos) = response.hover_pos() {
        ui.ctx().set_cursor_icon(if hit(pos, &curve.points).is_some() { egui::CursorIcon::Grab } else { egui::CursorIcon::Crosshair });
    }

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
                .color(if filled { theme::STAR } else { Color32::from_gray(110) });
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
            theme::STAR,
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

/// "1 Bild" / "n Bilder".
pub fn image_count(n: usize) -> String {
    if n == 1 { "1 Bild".to_string() } else { format!("{n} Bilder") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame with a slider row; returns the slider's screen rect (via AccessKit).
    fn frame(ctx: &egui::Context, value: &mut f32, events: Vec<egui::Event>) -> Option<egui::Rect> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 100.0))),
            events,
            ..Default::default()
        };
        ctx.enable_accesskit();
        let mut out = ctx.run_ui(input, |ui| {
            slider_row(ui, "Belichtung", value, -4.0..=4.0, 0.0, 2);
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update?;
        update.nodes.iter().find(|(_, n)| n.role() == egui::accesskit::Role::Slider).and_then(|(_, n)| {
            let b = n.bounds()?;
            Some(egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))
        })
    }

    #[test]
    fn double_click_on_slider_resets_it() {
        let ctx = egui::Context::default();
        let mut value = 1.5;
        let slider = frame(&ctx, &mut value, vec![]).expect("Regler");
        // Off-center: a single click there moves the value away from the default.
        let pos = slider.lerp_inside(vec2(0.75, 0.5));
        let click = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, &mut value, vec![egui::Event::PointerMoved(pos)]);
        for _ in 0..2 {
            frame(&ctx, &mut value, vec![click(true)]);
            frame(&ctx, &mut value, vec![click(false)]);
        }
        frame(&ctx, &mut value, vec![]);
        assert_eq!(value, 0.0);
    }
}

/// Clipping triangles in the top corners of a histogram (shadows left, highlights right):
/// lit when the image clips there, filled while the warning is shown. True when clicked.
pub fn clip_triangles(ui: &mut Ui, rect: Rect, hist: Option<&Histogram>, warning_on: bool) -> bool {
    // Share of pixels in the outermost bin of any channel.
    let clipped = |bin: usize| {
        hist.is_some_and(|h| {
            let total: u32 = h.luma.iter().sum::<u32>().max(1);
            [&h.r, &h.g, &h.b].iter().any(|c| c.get(bin).is_some_and(|&n| n as f32 / total as f32 > 0.001))
        })
    };
    let mut clicked = false;
    for (left, bin, hover) in [(true, 0, "Abgesoffene Tiefen"), (false, 255, "Ausgefressene Lichter")] {
        let size = vec2(16.0, 14.0);
        let r = if left {
            Rect::from_min_size(rect.min + vec2(4.0, 4.0), size)
        } else {
            Rect::from_min_size(pos2(rect.right() - 4.0 - size.x, rect.top() + 4.0), size)
        };
        let resp = ui.interact(r, ui.id().with(("clip_triangle", left)), Sense::click());
        let is_clipped = clipped(bin);
        let color = if warning_on {
            theme::ACCENT
        } else if is_clipped {
            if left { Color32::from_rgb(110, 160, 255) } else { theme::DANGER }
        } else if resp.hovered() {
            theme::TEXT_DIM
        } else {
            Color32::from_gray(80)
        };
        let c = r.shrink(2.0);
        let pts = if left {
            vec![c.left_top(), c.right_top(), c.left_bottom()]
        } else {
            vec![c.left_top(), c.right_top(), c.right_bottom()]
        };
        let stroke = Stroke::new(1.2, color);
        if warning_on || is_clipped {
            ui.painter().add(Shape::convex_polygon(pts, color, stroke));
        } else {
            ui.painter().add(Shape::closed_line(pts, stroke));
        }
        let state = if is_clipped { "vorhanden" } else { "keine" };
        if resp.on_hover_text(format!("{hover}: {state}\nKlick: Belichtungswarnung im Bild (E)")).clicked() {
            clicked = true;
        }
    }
    clicked
}
