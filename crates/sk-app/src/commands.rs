//! Every command in one place: label, menu group, keyboard shortcuts, when it is available and
//! what it does. The keyboard, the command palette and the menus all run commands from here, so
//! they can't drift apart (a shortcut shown in a menu is the one that works).

use std::collections::BTreeMap;
use std::sync::RwLock;

use egui::{Key, Modifiers};
use sk_core::sidecar::ColorLabel;
use sk_core::tools::Tool;

use crate::app::{App, Tab};
use crate::auto_tools::AutoKind;
use crate::layers::LocalTool;
use crate::theme::icon;

pub struct Command {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    /// Group in the palette (and the menu it belongs to).
    pub group: &'static str,
    /// Shortcuts; the first one is shown.
    pub keys: &'static [(Modifiers, Key)],
    /// Further search terms for the palette.
    pub keywords: &'static str,
    pub enabled: fn(&App) -> bool,
    pub run: fn(&mut App, &egui::Context),
}

const NONE: Modifiers = Modifiers::NONE;
const CTRL: Modifiers = Modifiers::COMMAND;
const SHIFT: Modifiers = Modifiers::SHIFT;
const CTRL_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const ALT: Modifiers = Modifiers::ALT;

fn always(_: &App) -> bool {
    true
}
fn has_image(a: &App) -> bool {
    a.current.is_some()
}
fn has_selection(a: &App) -> bool {
    !a.selection.is_empty() || a.current.is_some()
}
fn has_images(a: &App) -> bool {
    !a.visible.is_empty()
}
fn in_grid(a: &App) -> bool {
    a.grid_view && has_images(a)
}
fn several_files(a: &App) -> bool {
    sk_core::variant::files_of(&a.selected_paths()).len() >= 2
}
fn in_session(a: &App) -> bool {
    a.session.is_some() && has_selection(a)
}
/// The trash is a session folder; outside a session only variants can be deleted.
fn can_trash(a: &App) -> bool {
    has_selection(a) && (a.session.is_some() || a.selected_paths().iter().all(|p| sk_core::variant::number(p) > 1))
}
/// Entf: in the grid the selected images go to the trash (as with Strg+Entf), otherwise the
/// chosen spot, gradient or layer is deleted.
fn can_delete_chosen(a: &App) -> bool {
    if a.grid_view {
        return can_trash(a);
    }
    a.selected_spot.is_some() || a.selected_removal.is_some() || a.selected_gradient.is_some() || (a.tab == Tab::Layers && a.active_layer.is_some())
}
/// One export at a time, and only with a recipe switched on.
fn can_export(a: &App) -> bool {
    has_selection(a) && a.export_progress.is_none() && a.recipes.iter().any(|r| r.enabled)
}

/// Changes the whole image: not while a layer is chosen (only what works on it is there).
fn image_tool(a: &App) -> bool {
    has_image(a) && !a.on_layer()
}
/// The "A" of a tool (see `auto_tools`): for the selected images.
fn auto(a: &App, tool: Tool) -> bool {
    has_selection(a) && AutoKind::of_tool(tool).is_some_and(|k| a.can_auto(k))
}
fn has_layers(a: &App) -> bool {
    a.current.as_ref().is_some_and(|c| !c.adjustments.layers.is_empty())
}
fn on_layer(a: &App) -> bool {
    a.on_layer()
}

/// Shift held: the arrow keys extend the selection (Shift+arrow reaches the unshifted command,
/// see `command_shortcuts`).
fn shift(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.modifiers.shift)
}

fn tool(a: &mut App, t: LocalTool) {
    a.tab = t.tab();
    if a.local_tool != Some(t) {
        a.set_local_tool(t);
    }
}

fn brush_size(a: &mut App, factor: f32) {
    let b = a.brushes.of(a.local_tool);
    b.size = (b.size * factor).clamp(2.0, 400.0);
    a.brushes_changed_at = Some(std::time::Instant::now());
}

macro_rules! cmd {
    ($id:literal, $icon:expr, $label:literal, $group:literal, [$($m:expr => $k:expr),*], $kw:literal, $enabled:expr, $run:expr) => {
        Command { id: $id, icon: $icon, label: $label, group: $group, keys: &[$(($m, $k)),*], keywords: $kw, enabled: $enabled, run: $run }
    };
}

pub const COMMANDS: &[Command] = &[
    // Datei
    cmd!("open-folder", icon::FOLDER_OPEN, "Ordner öffnen …", "Datei", [CTRL => Key::O], "open", always, |a, _| a.open_folder_dialog()),
    cmd!("new-session", icon::FOLDER_STAR, "Neue Session …", "Datei", [CTRL => Key::N], "session", always, |a, _| {
        let folder = a.folder.clone();
        a.session_ui.start_new(&folder);
    }),
    cmd!("open-session", icon::FOLDER_OPEN, "Session öffnen …", "Datei", [CTRL_SHIFT => Key::O], "session", always, |a, _| a.open_session_dialog()),
    cmd!("close-session", icon::SIGN_OUT, "Session schließen", "Datei", [CTRL => Key::W], "session katalog zurück verlassen", in_session, |a, _| a.close_session()),
    cmd!("export", icon::EXPORT, "Exportieren", "Datei", [CTRL => Key::D], "export ausgabe speichern verarbeiten", can_export, |a, _| a.export_selection()),
    // Bearbeiten
    cmd!("undo", icon::ARROW_U_UP_LEFT, "Rückgängig", "Bearbeiten", [CTRL => Key::Z], "undo", |a| a.can_undo(), |a, _| a.undo()),
    cmd!("redo", icon::ARROW_U_UP_RIGHT, "Wiederholen", "Bearbeiten", [CTRL => Key::Y, CTRL_SHIFT => Key::Z], "redo", |a| a.can_redo(), |a, _| a.redo()),
    cmd!("copy-adjustments", icon::COPY, "Anpassungen kopieren", "Bearbeiten", [CTRL_SHIFT => Key::C], "copy", has_image, |a, _| a.copy_adjustments()),
    cmd!("copy-adjustments-choose", icon::LIST_CHECKS, "Anpassungen auswählen und kopieren …", "Bearbeiten", [], "copy clipboard zwischenablage werkzeuge auswählen", has_image, |a, _| a.open_copy_dialog()),
    cmd!("paste-adjustments", icon::CLIPBOARD_TEXT, "Anpassungen einfügen", "Bearbeiten", [CTRL_SHIFT => Key::V], "paste anwenden auswahl", |a| a.clipboard.is_some() && !a.on_layer(), |a, _| a.paste_adjustments()),
    cmd!("reset", icon::ARROWS_COUNTER_CLOCKWISE, "Alle Anpassungen zurücksetzen", "Bearbeiten", [], "reset", image_tool, |a, _| a.reset_adjustments()),
    cmd!("select-all", icon::CHECK_SQUARE, "Alle auswählen", "Bearbeiten", [CTRL => Key::A], "select all", has_images, |a, _| a.select_all()),
    cmd!("gpu", icon::LIGHTNING, "Grafikkarte …", "Bearbeiten", [], "gpu grafikkarte ki beschleunigung directml zwischenspeicher", always, |a, _| a.show_gpu_dialog = true),
    // Foto und Entwickeln
    cmd!("rotate-left", icon::ARROW_COUNTER_CLOCKWISE, "Links drehen", "Entwickeln", [CTRL => Key::OpenBracket], "rotate", image_tool, |a, _| a.rotate(3)),
    cmd!("rotate-right", icon::ARROW_CLOCKWISE, "Rechts drehen", "Entwickeln", [CTRL => Key::CloseBracket], "rotate", image_tool, |a, _| a.rotate(1)),
    cmd!("crop", icon::CROP, "Zuschneiden", "Entwickeln", [NONE => Key::C], "crop", image_tool, |a, _| a.toggle_crop()),
    cmd!("finish-crop", icon::CHECK, "Zuschnitt bzw. Trapezkorrektur übernehmen", "Entwickeln", [NONE => Key::Enter], "crop fertig trapez anwenden", |a| a.crop_tool.is_some() || a.keystone_guides_active(), |a, _| a.finish_viewer_tool()),
    cmd!("straighten", icon::RULER, "Geraderichten (Linie ziehen)", "Entwickeln", [], "straighten horizont", image_tool, |a, _| a.toggle_straighten()),
    cmd!("auto-adjust", icon::MAGIC_WAND, "Automatisch anpassen", "Entwickeln", [CTRL => Key::L], "auto adjust automatisch belichtung weißabgleich tonwerte", |a| has_selection(a) && a.can_auto(AutoKind::Adjust(a.auto_options)), |a, _| a.auto_adjust_selection()),
    cmd!("auto-wb", icon::MAGIC_WAND, "Weißabgleich automatisch", "Entwickeln", [], "auto wb white balance weißabgleich automatisch", |a| auto(a, Tool::WhiteBalance), |a, _| a.auto_tool(Tool::WhiteBalance)),
    cmd!("auto-exposure", icon::MAGIC_WAND, "Belichtung automatisch", "Entwickeln", [], "auto exposure belichtung kontrast automatisch", |a| auto(a, Tool::Exposure), |a, _| a.auto_tool(Tool::Exposure)),
    cmd!("auto-tone", icon::MAGIC_WAND, "Lichter & Tiefen automatisch", "Entwickeln", [], "auto hdr lichter tiefen weiß schwarz automatisch", |a| auto(a, Tool::HighlightsShadows), |a, _| a.auto_tool(Tool::HighlightsShadows)),
    cmd!("auto-vibrance", icon::MAGIC_WAND, "Dynamik automatisch", "Entwickeln", [], "auto vibrance dynamik farbe automatisch", |a| auto(a, Tool::Color), |a, _| a.auto_tool(Tool::Color)),
    cmd!("auto-levels", icon::MAGIC_WAND, "Tonwerte automatisch", "Entwickeln", [], "auto levels tonwerte schwarzpunkt weißpunkt automatisch", |a| auto(a, Tool::Levels), |a, _| a.auto_tool(Tool::Levels)),
    cmd!("auto-straighten", icon::MAGIC_WAND, "Geraderichten automatisch", "Entwickeln", [], "auto straighten horizont geraderichten drehen automatisch", |a| auto(a, Tool::Rotation), |a, _| a.auto_tool(Tool::Rotation)),
    cmd!("auto-upright", icon::PERSPECTIVE, "Perspektive automatisch", "Entwickeln", [SHIFT => Key::A], "upright geraderichten automatisch stürzende linien trapez", |a| auto(a, Tool::Perspective), |a, _| a.auto_tool(Tool::Perspective)),
    cmd!("auto-lens", icon::MAGIC_WAND, "Objektivprofil automatisch", "Entwickeln", [], "auto lens objektiv profil lensfun automatisch", |a| auto(a, Tool::LensCorrection), |a, _| a.auto_tool(Tool::LensCorrection)),
    cmd!("wb-picker", icon::EYEDROPPER, "Weißabgleich-Pipette", "Entwickeln", [NONE => Key::W], "white balance wb aufnehmen", image_tool, |a, _| a.toggle_wb_picker()),
    cmd!("rename", icon::PENCIL_SIMPLE, "Umbenennen …", "Foto", [CTRL_SHIFT => Key::R], "rename", has_selection, |a, _| a.open_rename()),
    cmd!("process-update", icon::ARROW_CIRCLE_UP, "Auf aktuelle Prozessversion heben", "Entwickeln", [], "prozessversion engine update", has_selection, |a, _| a.update_process_version()),
    cmd!("rating-0", icon::STAR, "Bewertung entfernen", "Foto", [NONE => Key::Num0], "sterne rating", has_selection, |a, _| a.set_rating(0)),
    cmd!("rating-1", icon::STAR, "Bewertung ★", "Foto", [NONE => Key::Num1], "sterne rating", has_selection, |a, _| a.set_rating(1)),
    cmd!("rating-2", icon::STAR, "Bewertung ★★", "Foto", [NONE => Key::Num2], "sterne rating", has_selection, |a, _| a.set_rating(2)),
    cmd!("rating-3", icon::STAR, "Bewertung ★★★", "Foto", [NONE => Key::Num3], "sterne rating", has_selection, |a, _| a.set_rating(3)),
    cmd!("rating-4", icon::STAR, "Bewertung ★★★★", "Foto", [NONE => Key::Num4], "sterne rating", has_selection, |a, _| a.set_rating(4)),
    cmd!("rating-5", icon::STAR, "Bewertung ★★★★★", "Foto", [NONE => Key::Num5], "sterne rating", has_selection, |a, _| a.set_rating(5)),
    cmd!("label-red", icon::TAG, "Farb-Label: Rot", "Foto", [NONE => Key::Num6], "farbe label", has_selection, |a, _| a.set_color_label(ColorLabel::Red)),
    cmd!("label-yellow", icon::TAG, "Farb-Label: Gelb", "Foto", [NONE => Key::Num7], "farbe label", has_selection, |a, _| a.set_color_label(ColorLabel::Yellow)),
    cmd!("label-green", icon::TAG, "Farb-Label: Grün", "Foto", [NONE => Key::Num8], "farbe label", has_selection, |a, _| a.set_color_label(ColorLabel::Green)),
    cmd!("label-blue", icon::TAG, "Farb-Label: Blau", "Foto", [NONE => Key::Num9], "farbe label", has_selection, |a, _| a.set_color_label(ColorLabel::Blue)),
    cmd!("label-none", icon::TAG, "Farb-Label: Keine", "Foto", [], "farbe label", has_selection, |a, _| a.set_color_label(ColorLabel::None)),
    cmd!("new-variant", icon::COPY_SIMPLE, "Neue Variante", "Foto", [NONE => Key::F2], "variant kopie", has_image, |a, _| a.pending = Some(crate::session::PendingAction::NewVariant { clone: false })),
    cmd!("clone-variant", icon::COPY, "Variante klonen", "Foto", [NONE => Key::F3], "variant kopie", has_image, |a, _| a.pending = Some(crate::session::PendingAction::NewVariant { clone: true })),
    cmd!("stack-create", icon::STACK_PLUS, "Stapel bilden", "Foto", [CTRL => Key::G], "stack gruppieren", several_files, |a, _| a.run_stack_action(crate::stacks::StackAction::Create)),
    cmd!("stack-dissolve", icon::STACK_MINUS, "Stapel auflösen", "Foto", [CTRL_SHIFT => Key::G], "stack", |a| !a.selected_stack_ids().is_empty(), |a, _| a.run_stack_action(crate::stacks::StackAction::Dissolve)),
    cmd!("stack-toggle", icon::STACK, "Stapel auf-/zuklappen", "Foto", [SHIFT => Key::G], "stack", has_selection, |a, _| a.run_stack_action(crate::stacks::StackAction::Toggle)),
    cmd!("merge-hdr", icon::SUN_HORIZON, "HDR zusammenfügen …", "Entwickeln", [], "merge belichtungsreihe", several_files, |a, _| a.open_merge(crate::merge::MergeKind::Hdr)),
    cmd!("merge-pano", icon::PANORAMA, "Panorama zusammenfügen …", "Entwickeln", [], "merge stitch", several_files, |a, _| a.open_merge(crate::merge::MergeKind::Panorama)),
    cmd!("merge-focus", icon::APERTURE, "Focus Stacking …", "Entwickeln", [], "merge fokus schärfentiefe stack makro focus stacking", several_files, |a, _| a.open_merge(crate::merge::MergeKind::FocusStack)),
    cmd!("denoise", icon::SPARKLE, "KI-Entrauschen", "Entwickeln", [], "denoise rauschen ki entrauschen rauschreduzierung", image_tool, |a, ctx| a.open_denoise(ctx)),
    cmd!("reference", icon::PUSH_PIN, "Referenzbild festlegen / entfernen", "Entwickeln", [SHIFT => Key::Enter], "vergleich compare", has_image, |a, _| a.toggle_reference()),
    cmd!("match-look", icon::SWATCHES, "Look angleichen", "Entwickeln", [], "match look referenz angleichen stil übertragen belichtung weißabgleich", |a| a.can_match_look() && !a.on_layer(), |a, _| {
        a.tab = Tab::Color;
        a.match_look_selection();
    }),
    cmd!("look-add-file", icon::IMAGE, "Look-Referenz aus Bilddatei …", "Entwickeln", [], "match look datei jpeg film still referenz hinzufügen", always, |a, _| {
        if a.pick_look_files() {
            a.tab = Tab::Color;
        }
    }),
    cmd!("look-add-image", icon::PLUS, "Dieses Bild als Look-Referenz", "Entwickeln", [], "match look referenz hinzufügen bibliothek", has_image, |a, _| {
        a.add_current_as_look_ref();
        a.tab = Tab::Color;
    }),
    cmd!("previous", icon::CARET_LEFT, "Vorheriges Bild", "Foto", [NONE => Key::ArrowLeft], "zurück previous", has_images, |a, ctx| a.step(-1, shift(ctx))),
    cmd!("next", icon::CARET_RIGHT, "Nächstes Bild", "Foto", [NONE => Key::ArrowRight], "weiter next", has_images, |a, ctx| a.step(1, shift(ctx))),
    cmd!("row-up", icon::CARET_UP, "Bild darüber", "Foto", [NONE => Key::ArrowUp], "raster zeile hoch up", in_grid, |a, ctx| a.step_row(-1, shift(ctx))),
    cmd!("row-down", icon::CARET_DOWN, "Bild darunter", "Foto", [NONE => Key::ArrowDown], "raster zeile runter down", in_grid, |a, ctx| a.step_row(1, shift(ctx))),
    cmd!("to-selects", icon::FOLDER_SIMPLE_STAR, "Nach „Selects“ verschieben", "Foto", [CTRL => Key::J], "selects session", in_session, |a, _| a.move_to_session_folder(sk_core::session::SessionFolder::Selects)),
    cmd!("trash", icon::TRASH, "In den Papierkorb", "Foto", [CTRL => Key::Delete, CTRL => Key::Backspace], "löschen delete", can_trash, |a, _| a.trash_selection()),
    // Ebenen und Retusche
    cmd!("tool-brush", icon::PAINT_BRUSH, "Pinsel", "Ebenen", [NONE => Key::B], "brush maske malen", has_image, |a, _| tool(a, LocalTool::Brush)),
    cmd!("tool-eraser", icon::ERASER, "Radierer", "Ebenen", [NONE => Key::N], "eraser maske", has_image, |a, _| tool(a, LocalTool::Eraser)),
    cmd!("tool-linear", icon::GRADIENT, "Linearer Verlauf", "Ebenen", [NONE => Key::L], "gradient verlaufsfilter", has_image, |a, _| tool(a, LocalTool::Linear)),
    cmd!("tool-radial", icon::CIRCLE_DASHED, "Radialer Verlauf", "Ebenen", [NONE => Key::R], "radial", has_image, |a, _| tool(a, LocalTool::Radial)),
    cmd!("tool-magic", icon::MAGIC_WAND, "Zauberpinsel", "Ebenen", [NONE => Key::J], "zauberstab magic brush auswahl", has_image, |a, _| tool(a, LocalTool::Magic)),
    cmd!("tool-object", icon::CURSOR_CLICK, "Objekt auswählen", "Ebenen", [SHIFT => Key::J], "objekt auswahl klick sam maske", has_image, |a, _| tool(a, LocalTool::Object)),
    cmd!("find-spots", icon::BROOM, "Staub und Flecken finden", "Retusche", [], "sensorstaub flecken unreinheiten pickel retusche reparieren", |a| image_tool(a) && a.spot_search.is_none(), |a, _| a.find_spots()),
    cmd!("tool-heal", icon::BANDAIDS, "Reparieren", "Retusche", [NONE => Key::H], "heal flecken", image_tool, |a, _| tool(a, LocalTool::Heal)),
    cmd!("tool-clone", icon::STAMP, "Klonen", "Retusche", [NONE => Key::S], "clone stempel", image_tool, |a, _| tool(a, LocalTool::Clone)),
    cmd!("tool-remove", icon::SPARKLE, "Entfernen", "Retusche", [NONE => Key::X], "entfernen remove objekt", image_tool, |a, _| tool(a, LocalTool::Remove)),
    cmd!("brush-smaller", icon::MINUS, "Pinsel kleiner", "Ebenen", [NONE => Key::OpenBracket], "größe", always, |a, _| brush_size(a, 1.0 / 1.2)),
    cmd!("brush-larger", icon::PLUS, "Pinsel größer", "Ebenen", [NONE => Key::CloseBracket], "größe", always, |a, _| brush_size(a, 1.2)),
    cmd!("show-mask", icon::EYE, "Maske anzeigen", "Ebenen", [NONE => Key::M], "mask", always, |a, _| {
        a.show_mask = !a.show_mask;
        if a.show_mask && !a.on_layer() {
            a.status = "Maske anzeigen: keine Ebene gewählt".into();
        }
        a.request_render();
    }),
    cmd!("edit-background", icon::X, "Hintergrund bearbeiten", "Ebenen", [], "ebene abwählen global ganzes bild background", on_layer, |a, _| a.select_layer(None)),
    cmd!("layer-up", icon::ARROW_UP, "Ebene darüber bearbeiten", "Ebenen", [ALT => Key::ArrowUp], "nächste ebene wählen layer", has_layers, |a, _| a.step_layer(true)),
    cmd!("layer-down", icon::ARROW_DOWN, "Ebene darunter bearbeiten", "Ebenen", [ALT => Key::ArrowDown], "vorherige ebene wählen layer hintergrund", has_layers, |a, _| a.step_layer(false)),
    cmd!("delete-spot", icon::TRASH, "Gewähltes löschen (im Raster: in den Papierkorb)", "Bearbeiten", [NONE => Key::Delete, NONE => Key::Backspace], "spot verlauf gradient ebene layer entfernen papierkorb trash", can_delete_chosen, |a, _| {
        if a.grid_view {
            a.trash_selection()
        } else {
            a.delete_selected_spot()
        }
    }),
    // Ansicht
    cmd!("grid", icon::SQUARES_FOUR, "Raster / Einzelbild umschalten", "Ansicht", [NONE => Key::G], "grid browser übersicht viewer", always, |a, _| a.grid_view = !a.grid_view),
    cmd!("filmstrip", icon::FILM_STRIP, "Filmstreifen ein/aus", "Ansicht", [], "filmstrip", always, |a, _| a.show_filmstrip = !a.show_filmstrip),
    cmd!("navigator", icon::MAP_TRIFOLD, "Navigator beim Zoomen", "Ansicht", [], "minimap", always, |a, _| {
        a.show_navigator = !a.show_navigator;
        a.save_settings();
    }),
    cmd!("side-column", icon::SIDEBAR_SIMPLE, "Seitenleiste rechts: wenn Platz ist / immer / aus", "Ansicht", [], "spalte histogramm rechts panel", always, |a, _| {
        use crate::side_column::SideColumn;
        a.side_column = match a.side_column {
            SideColumn::Auto => SideColumn::Always,
            SideColumn::Always => SideColumn::Off,
            SideColumn::Off => SideColumn::Auto,
        };
        a.status = a.side_column.label().into();
        a.save_settings();
    }),
    cmd!("zoom-fit", icon::CORNERS_OUT, "Einpassen", "Ansicht", [], "fit zoom", has_image, |a, _| a.zoom_fit()),
    cmd!("zoom-100", icon::MAGNIFYING_GLASS_PLUS, "100 % / Einpassen", "Ansicht", [NONE => Key::Z], "zoom", has_image, |a, _| a.toggle_zoom_100()),
    cmd!("print-zoom", icon::RULER, "Druckgröße", "Ansicht", [SHIFT => Key::Z], "print zoom kalibrieren", has_image, |a, _| a.toggle_print_zoom()),
    cmd!("before", icon::CIRCLE_HALF, "Vorher / Nachher", "Ansicht", [NONE => Key::Y], "before after vergleich", has_image, |a, _| a.toggle_before()),
    cmd!("split", icon::SQUARE_SPLIT_HORIZONTAL, "Geteilter Vergleich", "Ansicht", [NONE => Key::U], "split vorher nachher", has_image, |a, _| a.toggle_split()),
    cmd!("side-by-side", icon::COLUMNS, "Vorher | Nachher nebeneinander", "Ansicht", [SHIFT => Key::U], "side by side", has_image, |a, _| a.toggle_side_by_side()),
    cmd!("multi-view", icon::SQUARES_FOUR, "Auswahl nebeneinander vergleichen", "Ansicht", [], "compare mehrere", always, |a, _| a.multi_view = !a.multi_view),
    cmd!("clip-warning", icon::WARNING, "Belichtungswarnung", "Ansicht", [NONE => Key::E], "clipping überbelichtet", has_image, |a, _| a.toggle_clip_warning()),
    cmd!("focus-mask", icon::CROSSHAIR, "Schärfemaske", "Ansicht", [NONE => Key::K], "fokusmaske fokus focus peaking", has_image, |a, _| a.toggle_focus_mask()),
    cmd!("proof", icon::FILE_MAGNIFYING_GLASS, "Ausgabevorschau", "Ansicht", [NONE => Key::P], "vorschau export proof rezept", has_image, |a, _| a.toggle_proof()),
    cmd!("softproof", icon::PRINTER, "Softproof", "Ansicht", [SHIFT => Key::P], "drucker profil gamut", has_image, |a, _| a.toggle_softproof()),
    cmd!("print-preview", icon::PRINTER, "Druckvorschau", "Ansicht", [CTRL => Key::P], "drucken papier", has_image, |a, _| a.toggle_print_preview()),
    cmd!("guides-next", icon::GRID_NINE, "Hilfslinien wechseln", "Ansicht", [NONE => Key::O], "guides raster drittel", always, |a, _| {
        a.guides = a.guides.next();
        a.save_settings();
    }),
    cmd!("viewer-window", icon::MONITOR, "Bild in eigenem Fenster", "Ansicht", [SHIFT => Key::F11], "zweitmonitor viewer", always, |a, _| a.toggle_viewer_window()),
    cmd!("fullscreen", icon::ARROWS_OUT, "Vollbild ohne Ablenkung", "Ansicht", [NONE => Key::F11, CTRL_SHIFT => Key::F], "fullscreen", |a| a.fullscreen || has_image(a), |a, ctx| a.toggle_fullscreen(ctx)),
    cmd!("find", icon::MAGNIFYING_GLASS, "Suchen (Bibliothek)", "Ansicht", [CTRL => Key::F], "filter suche", always, |a, _| a.tab = Tab::Library),
    // Hilfe
    cmd!("shortcuts", icon::KEYBOARD, "Tastenkürzel", "Hilfe", [NONE => Key::F1], "shortcuts hilfe", always, |a, _| a.show_shortcuts = !a.show_shortcuts),
    cmd!("update", icon::ARROW_CIRCLE_UP, "Nach Updates suchen …", "Hilfe", [], "update aktualisieren version release neu", always, |a, _| a.update_dialog_open()),
    cmd!("licenses", icon::SCALES, "Lizenzen …", "Hilfe", [], "lizenzen licenses modelle lensfun urheber", always, |a, _| a.show_licenses = true),
    cmd!("about", icon::INFO, "Über Silberkorn", "Hilfe", [], "about version", always, |a, _| a.splash = Some(crate::splash::Splash::about())),
];

/// The shortcut of a command, as shown in menus (empty without one).
pub fn shortcut_of(id: &str) -> String {
    get(id).map(|c| c.shortcut()).unwrap_or_default()
}

/// The shortcut of a command for running text ("Enter übernimmt"), or `fallback` (e.g. the
/// button's name) when it has none.
pub fn key_or(id: &str, fallback: &str) -> String {
    Some(shortcut_of(id)).filter(|k| !k.is_empty()).unwrap_or_else(|| fallback.to_string())
}

pub fn get(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// Shortcuts the user changed, by command id (see [`Command::bindings`]).
static OVERRIDES: RwLock<Bindings> = RwLock::new(BTreeMap::new());

/// Keys handled outside the command list, which a command can't take.
pub fn reserved((m, k): (Modifiers, Key)) -> Option<&'static str> {
    let none = m == NONE;
    match k {
        Key::K if m == CTRL => Some("Befehlssuche"),
        Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 if m == CTRL => {
            Some("Werkzeug-Tabs")
        }
        Key::Escape => Some("Werkzeug beenden"),
        Key::Space => Some("Bild verschieben"),
        Key::Tab => Some("Tastaturnavigation"),
        Key::F4 if m.alt => Some("Beenden"),
        k if none && crate::browser::SPEED_KEYS.iter().any(|(s, _)| *s == k) => Some("Schnellbearbeitung (Taste halten + Mausrad)"),
        _ => None,
    }
}

type Bindings = BTreeMap<String, Vec<(Modifiers, Key)>>;

fn bindings_in(map: &Bindings, c: &Command) -> Vec<(Modifiers, Key)> {
    map.get(c.id).cloned().unwrap_or_else(|| c.keys.to_vec())
}

fn set_in(map: &mut Bindings, id: &str, keys: Vec<(Modifiers, Key)>) {
    let Some(c) = get(id) else { return };
    if keys == c.keys {
        map.remove(id);
    } else {
        map.insert(id.to_string(), keys);
    }
}

fn assign_in(map: &mut Bindings, id: &str, key: (Modifiers, Key), replace: Option<usize>) -> Option<&'static str> {
    let c = get(id)?;
    let mut previous = None;
    for other in COMMANDS.iter().filter(|o| o.id != id) {
        let keys = bindings_in(map, other);
        if keys.contains(&key) {
            set_in(map, other.id, keys.into_iter().filter(|k| *k != key).collect());
            previous = Some(other.label);
        }
    }
    let mut keys = bindings_in(map, c);
    match replace {
        Some(i) if i < keys.len() => keys[i] = key,
        _ => keys.push(key),
    }
    // The same key twice (it was there already at another place) counts once.
    let mut seen = Vec::new();
    keys.retain(|k| !seen.contains(k) && {
        seen.push(*k);
        true
    });
    set_in(map, id, keys);
    previous
}

fn overrides() -> std::sync::RwLockWriteGuard<'static, Bindings> {
    OVERRIDES.write().unwrap_or_else(|e| e.into_inner())
}

/// Sets the shortcuts of a command; the defaults again when they are the same.
pub fn set_bindings(id: &str, keys: Vec<(Modifiers, Key)>) {
    set_in(&mut overrides(), id, keys);
}

/// All shortcuts back to the defaults.
pub fn reset_all_bindings() {
    overrides().clear();
}

/// Whether the shortcuts of a command differ from its defaults.
pub fn is_changed(id: &str) -> bool {
    OVERRIDES.read().unwrap_or_else(|e| e.into_inner()).contains_key(id)
}

/// Assigns `key` to command `id` (as an additional shortcut, or instead of the one at `replace`)
/// and takes it away from a command that had it; returns that command's label.
pub fn assign(id: &str, key: (Modifiers, Key), replace: Option<usize>) -> Option<&'static str> {
    assign_in(&mut overrides(), id, key, replace)
}

/// A shortcut as stored in the settings: "Ctrl+Shift+Alt+" and the key name of egui.
fn key_code((m, k): (Modifiers, Key)) -> String {
    let mut s = String::new();
    if m.command || m.ctrl {
        s += "Ctrl+";
    }
    if m.shift {
        s += "Shift+";
    }
    if m.alt {
        s += "Alt+";
    }
    s + k.name()
}

fn parse_key_code(code: &str) -> Option<(Modifiers, Key)> {
    let mut m = NONE;
    let mut rest = code;
    loop {
        if let Some(r) = rest.strip_prefix("Ctrl+") {
            m = m.plus(CTRL);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift+") {
            m = m.plus(SHIFT);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt+") {
            m = m.plus(Modifiers::ALT);
            rest = r;
        } else {
            break;
        }
    }
    Some((m, Key::from_name(rest)?))
}

/// The changed shortcuts for the settings file.
pub fn saved_bindings() -> BTreeMap<String, Vec<String>> {
    OVERRIDES.read().unwrap_or_else(|e| e.into_inner()).iter().map(|(id, keys)| (id.clone(), keys.iter().map(|k| key_code(*k)).collect())).collect()
}

/// Takes the changed shortcuts from the settings file (unknown commands and keys are skipped).
pub fn load_bindings(saved: &BTreeMap<String, Vec<String>>) {
    *overrides() = parse_bindings(saved);
}

fn parse_bindings(saved: &BTreeMap<String, Vec<String>>) -> Bindings {
    let mut map = Bindings::new();
    for (id, codes) in saved {
        set_in(&mut map, id, codes.iter().filter_map(|c| parse_key_code(c)).collect());
    }
    map
}

/// `text` with the shortcut of command `id` in brackets, e.g. "Rückgängig (Strg+Z)".
/// Commands of AI functions (marked with the yellow sparkle in menus and the palette).
pub fn is_ai(id: &str) -> bool {
    matches!(id, "denoise" | "tool-object" | "tool-remove")
}

pub fn hint(text: &str, id: &str) -> String {
    match get(id).map(|c| c.shortcut()).unwrap_or_default() {
        k if k.is_empty() => text.to_string(),
        k => format!("{text} ({k})"),
    }
}

/// The shortcut pressed in this frame, for recording one: modifiers are normalized (Ctrl is the
/// command key) and a modifier on its own is no shortcut.
pub fn pressed_key(ctx: &egui::Context) -> Option<(Modifiers, Key)> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => {
                let mut m = NONE;
                if modifiers.command || modifiers.ctrl {
                    m = m.plus(CTRL);
                }
                if modifiers.shift {
                    m = m.plus(SHIFT);
                }
                if modifiers.alt {
                    m = m.plus(Modifiers::ALT);
                }
                Some((m, *key))
            }
            _ => None,
        })
    })
}

/// A shortcut as shown in menus and the palette ("Strg+Umschalt+R").
pub fn key_text((m, k): (Modifiers, Key)) -> String {
    let mut s = String::new();
    if m.command || m.ctrl {
        s += "Strg+";
    }
    if m.alt {
        s += "Alt+";
    }
    if m.shift {
        s += "Umschalt+";
    }
    s += &match k {
        Key::Delete => "Entf".to_string(),
        Key::Backspace => "Rücktaste".to_string(),
        Key::OpenBracket => "[".to_string(),
        Key::CloseBracket => "]".to_string(),
        Key::ArrowLeft => icon::ARROW_LEFT.to_string(),
        Key::ArrowRight => icon::ARROW_RIGHT.to_string(),
        Key::ArrowUp => icon::ARROW_UP.to_string(),
        Key::ArrowDown => icon::ARROW_DOWN.to_string(),
        Key::Escape => "Esc".to_string(),
        k => k.symbol_or_name().to_string(),
    };
    s
}

impl Command {
    /// The shortcuts in use: the user's (see [`set_bindings`]) or the defaults.
    pub fn bindings(&self) -> Vec<(Modifiers, Key)> {
        bindings_in(&OVERRIDES.read().unwrap_or_else(|e| e.into_inner()), self)
    }

    /// The first shortcut, as text (empty without one).
    pub fn shortcut(&self) -> String {
        self.bindings().first().map(|k| key_text(*k)).unwrap_or_default()
    }
}

impl App {
    /// Whether a command can run now (for buttons outside menus that run it).
    pub fn command_available(&self, id: &str) -> bool {
        get(id).is_some_and(|c| (c.enabled)(self))
    }

    /// Runs a command by id, if it is available now.
    pub fn run_command_id(&mut self, ctx: &egui::Context, id: &str) {
        if let Some(c) = get(id)
            && (c.enabled)(self)
        {
            (c.run)(self, ctx);
        }
    }

    /// Runs the command whose shortcut was pressed. Shortcuts with more modifiers are checked
    /// first: egui matches a key without modifiers also when Shift is held.
    pub(crate) fn command_shortcuts(&mut self, ctx: &egui::Context) {
        let mut keys: Vec<(&'static Command, (Modifiers, Key))> = COMMANDS.iter().flat_map(|c| c.bindings().into_iter().map(move |k| (c, k))).collect();
        let count = |m: Modifiers| m.command as u8 + m.ctrl as u8 + m.shift as u8 + m.alt as u8;
        keys.sort_by_key(|(_, (m, _))| std::cmp::Reverse(count(*m)));
        // A slider that was just clicked keeps the arrow keys; a click elsewhere
        // or Esc gives them back to "previous/next image".
        let slider = crate::widgets::slider_has_focus(ctx);
        // A focused control presses itself with Enter.
        let focused = ctx.memory(|m| m.focused()).is_some();
        for (c, (m, k)) in keys {
            if slider && matches!(k, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown) {
                continue;
            }
            if focused && k == Key::Enter {
                continue;
            }
            // Checked first: `enabled` can cost a pass over the selection, and this runs every frame.
            if !ctx.input(|i| i.key_pressed(k)) {
                continue;
            }
            // A key of an unavailable command stays for others (Enter of "finish crop" must still
            // reach a dialog); with modifiers it is taken anyway, or Shift+X would run X.
            let enabled = (c.enabled)(self);
            if (enabled || count(m) > 0) && ctx.input_mut(|i| i.consume_key(m, k)) && enabled {
                (c.run)(self, ctx);
            }
        }
    }

    /// A menu entry for a command (its label, icon, shortcut and availability); runs it when
    /// clicked.
    pub(crate) fn command_item(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(c) = get(id) else { return };
        self.command_item_labeled(ui, id, c.label);
    }

    /// Like [`Self::command_item`] with a label of its own (e.g. with a count).
    pub(crate) fn command_item_labeled(&mut self, ui: &mut egui::Ui, id: &str, label: &str) {
        if self.command_button_labeled(ui, id, label)
            && let Some(c) = get(id)
        {
            (c.run)(self, ui.ctx());
        }
    }

    /// The menu entry of a command without running it: true when clicked. For menus drawn
    /// while the command can't run yet (see `PendingAction`).
    pub(crate) fn command_button(&self, ui: &mut egui::Ui, id: &str) -> bool {
        get(id).is_some_and(|c| self.command_button_labeled(ui, id, c.label))
    }

    fn command_button_labeled(&self, ui: &mut egui::Ui, id: &str, label: &str) -> bool {
        let Some(c) = get(id) else { return false };
        let text = format!("{}  {label}", c.icon);
        let text = if is_ai(id) { crate::widgets::ai_text(ui, &text) } else { text.into() };
        let button = egui::Button::new(text).shortcut_text(c.shortcut());
        ui.add_enabled((c.enabled)(self), button).clicked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_shortcuts_are_unique() {
        let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "doppelte Befehls-IDs");
        let mut keys: Vec<String> = COMMANDS.iter().flat_map(|c| c.keys.iter().map(|k| key_text(*k))).collect();
        keys.sort();
        let dups: Vec<&String> = keys.windows(2).filter(|w| w[0] == w[1]).map(|w| &w[0]).collect();
        assert!(dups.is_empty(), "Tastenkürzel doppelt belegt: {dups:?}");
    }

    #[test]
    fn assigning_a_key_takes_it_from_the_other_command() {
        let mut map = Bindings::new();
        let ctrl_e = (CTRL, Key::E);
        // An additional shortcut for "Exportieren".
        assert_eq!(assign_in(&mut map, "export", ctrl_e, None), None);
        assert_eq!(bindings_in(&map, get("export").unwrap()), vec![(CTRL, Key::D), ctrl_e]);
        // E alone moves from the exposure warning to the crop tool, replacing C.
        assert_eq!(assign_in(&mut map, "crop", (NONE, Key::E), Some(0)), Some("Belichtungswarnung"));
        assert_eq!(bindings_in(&map, get("crop").unwrap()), vec![(NONE, Key::E)]);
        assert!(bindings_in(&map, get("clip-warning").unwrap()).is_empty());
        // Back to the default: no longer stored.
        assign_in(&mut map, "crop", (NONE, Key::C), Some(0));
        assert!(!map.contains_key("crop"));
        // Saved and read back.
        let saved: BTreeMap<String, Vec<String>> =
            map.iter().map(|(id, keys)| (id.clone(), keys.iter().map(|k| key_code(*k)).collect())).collect();
        assert_eq!(saved["export"], vec!["Ctrl+D".to_string(), "Ctrl+E".to_string()]);
        assert_eq!(parse_bindings(&saved), map);
    }

    #[test]
    fn key_codes_round_trip() {
        for c in COMMANDS {
            for k in c.keys {
                assert_eq!(parse_key_code(&key_code(*k)), Some(*k), "{}", c.id);
                assert_eq!(reserved(*k), None, "{} belegt eine feste Taste", c.id);
            }
        }
        assert_eq!(parse_key_code("Ctrl+Shift+Alt+F5"), Some((CTRL_SHIFT.plus(Modifiers::ALT), Key::F5)));
        assert_eq!(parse_key_code("Ctrl+Gibtsnicht"), None);
    }

    #[test]
    fn shortcut_text() {
        assert_eq!(get("rename").unwrap().shortcut(), "Strg+Umschalt+R");
        assert_eq!(get("trash").unwrap().shortcut(), "Strg+Entf");
        assert_eq!(get("rotate-left").unwrap().shortcut(), "Strg+[");
        assert_eq!(get("reset").unwrap().shortcut(), "");
    }
}
