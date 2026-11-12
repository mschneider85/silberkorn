//! The bar at the bottom of the image while a brush, gradient or retouching tool is active: the
//! tools of its group, the most used brush settings and "Fertig". The same settings stay in the
//! panel under the same names; this is where one looks while painting.

use egui::{RichText, Ui};

use crate::app::{App, Tab};
use crate::layers::{Brushes, LocalTool};
use crate::theme;

/// The tools offered side by side in the bar: those of the tool's tab.
const MASK_TOOLS: [LocalTool; 6] = [LocalTool::Brush, LocalTool::Eraser, LocalTool::Linear, LocalTool::Radial, LocalTool::Magic, LocalTool::Object];
const RETOUCH_TOOLS: [LocalTool; 3] = [LocalTool::Heal, LocalTool::Clone, LocalTool::Remove];

/// Width of the sliders in the bar.
const SLIDER: f32 = 72.0;

/// A slider with its name before and its value after it; double-click resets. Whether it changed.
fn bar_slider(ui: &mut Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, text: impl Fn(f32) -> String) -> bool {
    ui.horizontal(|ui| {
        ui.add_sized([58.0, ui.spacing().interact_size.y], egui::Label::new(RichText::new(name).color(theme::TEXT_DIM)));
        let changed = crate::widgets::compact_slider(ui, name, value, range, default, SLIDER);
        ui.add_sized([34.0, ui.spacing().interact_size.y], egui::Label::new(text(*value)));
        changed
    })
    .inner
}

fn percent(v: f32) -> String {
    format!("{:.0} %", v * 100.0)
}

impl App {
    pub(crate) fn local_bar(&mut self, ui: &mut Ui) {
        let Some(tool) = self.local_tool else { return };
        // Picking a color for the range is a moment, not a tool to set up.
        if tool == LocalTool::ColorPick {
            return;
        }
        let tools: &[LocalTool] = if tool.tab() == Tab::Retouch { &RETOUCH_TOOLS } else { &MASK_TOOLS };
        let mask_tools = tool.tab() == Tab::Layers;
        // A gradient has no card to delete it from: here, for the chosen one.
        let selected = self.selected_gradient.is_some();
        let show_mask = self.show_mask;
        let (mut chosen, mut toggle_mask, mut delete, mut brushes_changed) = (None, false, false, false);
        let (area, dock) = (self.bar_area(ui), self.bar_dock);
        let magic_tolerance = &mut self.magic_tolerance;
        let brushes = &mut self.brushes;
        let done = crate::crop::tool_bar(ui, area, &mut self.bar_dock, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for &t in tools {
                    let (ic, name, id) = t.tile();
                    let text = if crate::commands::is_ai(id) { crate::widgets::ai_text(ui, ic) } else { ic.into() };
                    let r = ui.add(egui::Button::new(text).selected(t == tool));
                    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, t == tool, name));
                    if r.on_hover_text(crate::commands::hint(name, id)).clicked() && t != tool {
                        chosen = Some(t);
                    }
                }
            });
            if tool.is_brush() {
                ui.separator();
                let d = Brushes::defaults_of(Some(tool));
                let b = brushes.of(Some(tool));
                brushes_changed |= bar_slider(ui, "Größe", &mut b.size, 2.0..=400.0, d.size, |v| format!("{v:.0}"));
                if tool == LocalTool::Magic {
                    // Its own tolerance, as in the panel (the brush has no hardness or opacity).
                    bar_slider(ui, "Toleranz", magic_tolerance, 0.0..=1.0, 0.3, percent);
                } else {
                    brushes_changed |= bar_slider(ui, "Härte", &mut b.hardness, 0.0..=1.0, d.hardness, percent);
                    brushes_changed |= bar_slider(ui, "Deckkraft", &mut b.opacity, 0.05..=1.0, d.opacity, percent);
                }
                // Flow and automatic masking: set less often, behind a button.
                if matches!(tool, LocalTool::Brush | LocalTool::Eraser | LocalTool::Remove) {
                    let more = crate::widgets::menu_button(ui, RichText::new(theme::icon::SLIDERS_HORIZONTAL).size(14.0), |ui| {
                        ui.set_width(260.0);
                        crate::layers::brush_extras(ui, b, Some(tool))
                    });
                    brushes_changed |= more.inner.unwrap_or(false);
                    let on = if b.auto_mask { " · Kanten beachten an" } else { "" };
                    more.response.on_hover_text(format!("Mehr: Fluss, Kanten beachten, Toleranz{on}"));
                }
            }
            if mask_tools || selected {
                ui.separator();
            }
            if mask_tools && ui.selectable_label(show_mask, "Maske anzeigen").on_hover_text(crate::commands::hint("Maske anzeigen", "show-mask")).clicked() {
                toggle_mask = true;
            }
            // Only the chosen pin or gradient: Entf on a layer deletes the layer, not wanted here.
            if selected && ui.button(format!("{}  Löschen", theme::icon::TRASH)).on_hover_text(crate::commands::hint("Gewählten Verlauf löschen", "delete-spot")).clicked() {
                delete = true;
            }
        });
        self.bar_dock_changed(dock);
        if brushes_changed {
            self.brushes_changed_at = Some(std::time::Instant::now());
        }
        if let Some(t) = chosen {
            self.set_local_tool(t);
        }
        if toggle_mask {
            self.run_command_id(&ui.ctx().clone(), "show-mask");
        }
        if delete {
            self.delete_selected_spot();
        }
        if done {
            self.end_viewer_tools();
        }
    }
}
