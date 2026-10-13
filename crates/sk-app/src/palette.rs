//! Command palette (Ctrl+K): finds every command, tool and tool section by name, like the
//! command palettes of VS Code or Figma. Typing a few letters of what one wants ("weiß",
//! "rausch", "hdr") is often quicker than knowing where it lives.

use egui::{Align2, Color32, Id, Key, Modifiers, Order, Rect, RichText, Sense, Ui, vec2};

use crate::app::{App, Tab};
use crate::develop_tools::Auto;
use crate::guides::Guides;
use crate::theme::{self, icon};

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    /// Highlighted row in the result list.
    selected: usize,
    /// Labels of recently run commands, newest first (shown for an empty query).
    recent: Vec<String>,
    /// Set when the palette opens, so the text field takes the focus.
    just_opened: bool,
}

impl Palette {
    pub fn show(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.just_opened = true;
    }

    /// Opens the palette with a search already typed (for screenshots).
    pub fn show_with(&mut self, query: &str) {
        self.show();
        self.query = query.to_string();
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.open = false;
        } else {
            self.show();
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    /// A command of the command list, by id.
    Command(&'static str),
    Tab(Tab),
    /// Opens the tab and jumps to the section with this title.
    Section(Tab, &'static str),
    Run(fn(&mut App, &egui::Context)),
    Guides(Guides),
    Auto(Auto),
}

struct Command {
    icon: &'static str,
    label: String,
    group: &'static str,
    shortcut: String,
    enabled: bool,
    /// Further search terms (synonyms, English names).
    keywords: &'static str,
    action: Action,
}

/// Tool sections per tab with search terms; titles as passed to `widgets::section`.
const SECTIONS: &[(Tab, &str, &str)] = &[
    (Tab::Library, "Gesichter & Sichtung", "culling augen geschlossen unscharf personen faces"),
    (Tab::Library, "Alben", "album smart filteralbum sammlung"),
    (Tab::Library, "Ordner", "folder verzeichnis"),
    (Tab::Library, "Filter & Suche", "suchen filter sterne bewertung sortieren sort"),
    (Tab::Lens, "Zuschnitt", "crop beschneiden seitenverhältnis format"),
    (Tab::Lens, "Drehen & Spiegeln", "rotate flip geraderichten horizont straighten"),
    (Tab::Lens, "Perspektive", "perspektive stürzende linien upright keystone"),
    (Tab::Lens, "Objektivkorrektur", "lens verzeichnung chromatische aberration ca vignette lensfun profil fisheye fischauge entzerren defish"),
    (Tab::Exposure, "Belichtung", "exposure kontrast helligkeit sättigung"),
    (Tab::Exposure, "Lichter und Tiefen", "hdr lichter tiefen weiß schwarz highlights shadows"),
    (Tab::Exposure, "Tonwerte", "levels schwarzpunkt weißpunkt"),
    (Tab::Exposure, "Gradationskurve", "curve kurve tonkurve"),
    (Tab::Color, "Basismerkmale", "basiskurve kameraprofil"),
    (Tab::Color, "Weißabgleich", "white balance wb kelvin temperatur tint"),
    (Tab::Color, "Farbe", "sättigung dynamik vibrance saturation"),
    (Tab::Color, "Farbeditor", "hsl farbbereich farbton color editor"),
    (Tab::Color, "Hauttöne", "skin haut porträt"),
    (Tab::Color, "Farbbalance", "color grading color balance farbräder tonung"),
    (Tab::Color, "Schwarzweiß", "black white monochrom sw"),
    (Tab::Details, "Schärfen", "sharpen schärfe"),
    (Tab::Details, "Rauschen", "noise denoise ki ai entrauschen rauschreduzierung luminanz farbrauschen"),
    (Tab::Details, "Klarheit", "clarity struktur"),
    (Tab::Details, "Dunst entfernen", "dehaze dunst nebel"),
    (Tab::Details, "Vignettierung", "vignette"),
    (Tab::Details, "Körnung", "grain film korn"),
    (Tab::Details, "Objektivunschärfe", "lens blur bokeh tiefe tiefenschärfe hintergrund unschärfe ki"),
    (Tab::Layers, "Maske", "pinsel radierer verlauf zauberpinsel motiv himmel tiefe helligkeitsbereich farbbereich eingrenzen brush luma range"),
    (Tab::Layers, "Anpassungen", "ebenen-anpassungen lokal belichtung"),
    (Tab::Retouch, "Entfernen (KI)", "remove objekt retusche ki"),
    (Tab::Layers, "Ebenen", "layer maske lokal"),
    (Tab::Retouch, "Reparieren & Klonen", "heal clone stempel flecken staub"),
    (Tab::Metadata, "Keywords", "schlagworte stichworte tags"),
    (Tab::Metadata, "Aufnahme", "exif kamera objektiv iso"),
    (Tab::Styles, "Filmsimulationen", "film look preset"),
    (Tab::Styles, "Eigene Stile", "preset stil"),
    (Tab::Styles, "Neuer Stil", "preset speichern"),
    (Tab::History, "Schnappschüsse", "snapshot stand sichern"),
    (Tab::History, "Verlauf", "history rückgängig schritte"),
    (Tab::Output, "Ausgaberezepte", "export rezept jpeg tiff png"),
    (Tab::Output, "Wasserzeichen", "watermark logo copyright"),
    (Tab::Output, "Softproof", "drucker icc profil farbumfang gamut"),
];

impl App {
    fn commands(&self) -> Vec<Command> {
        let has = self.current.is_some();
        let mut v = Vec::new();
        let mut add = |icon, label: &str, group, shortcut: &str, enabled, keywords, action| {
            v.push(Command { icon, label: label.to_string(), group, shortcut: shortcut.to_string(), enabled, keywords, action });
        };
        use Action::Run;

        // Tools and their sections first: they are what one looks for most often.
        for tab in Tab::ALL {
            add(tab.icon(), tab.title(), "Werkzeug", &tab_shortcut(tab), true, "tab", Action::Tab(tab));
        }
        for &(tab, title, keywords) in SECTIONS {
            add(tab.icon(), title, tab.title(), "", true, keywords, Action::Section(tab, title));
        }

        // Everything in the command list (see `commands`), with the same shortcuts as the keyboard.
        for c in crate::commands::COMMANDS {
            add(c.icon, c.label, c.group, &c.shortcut(), (c.enabled)(self), c.keywords, Action::Command(c.id));
        }

        // Entries that only the palette offers.
        let import = if self.session.is_some() { "In Session importieren …" } else { "Ordner in Katalog importieren …" };
        add(icon::FOLDER_PLUS, import, "Datei", "", true, "import", Run(|a, _| a.import_folder_dialog()));
        let g = "Bild";
        add(icon::MAGIC_WAND, "Auto-Belichtung", g, "", has, "automatisch exposure", Action::Auto(Auto::Exposure));
        add(icon::MAGIC_WAND, "Auto-Tonwerte", g, "", has, "automatisch tonwerte", Action::Auto(Auto::Levels));
        add(icon::MAGIC_WAND, "Auto-Weißabgleich", g, "", has, "automatisch wb white balance", Action::Auto(Auto::WhiteBalance));
        for gd in Guides::ALL {
            let label = if gd == Guides::Off { "Hilfslinien aus".to_string() } else { format!("Hilfslinien: {}", gd.label()) };
            add(icon::GRID_NINE, &label, "Ansicht", "", true, "guides raster drittel", Action::Guides(gd));
        }
        v
    }

    fn run_command(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Command(id) => self.run_command_id(ctx, id),
            Action::Tab(tab) => self.tab = tab,
            Action::Section(tab, title) => {
                self.tab = tab;
                crate::widgets::reveal_section(ctx, title);
            }
            Action::Run(f) => f(self, ctx),
            Action::Guides(g) => {
                self.guides = g;
                self.save_settings();
            }
            Action::Auto(kind) => self.run_auto(kind),
        }
    }

    /// Draws the palette while it is open.
    pub fn palette_ui(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let screen = ctx.content_rect();
        // Dim the window behind; a click there closes the palette.
        let backdrop = egui::Area::new(Id::new("palette_backdrop")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(110));
            resp.clicked()
        });
        if backdrop.inner {
            self.palette.open = false;
            return;
        }

        let commands = self.commands();
        let hits = rank(&commands, &self.palette.query, &self.palette.recent);
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.palette.open = false;
            return;
        }
        let n = hits.len();
        let mut moved = false;
        if n > 0 {
            if down {
                self.palette.selected = (self.palette.selected + 1) % n;
                moved = true;
            }
            if up {
                self.palette.selected = (self.palette.selected + n - 1) % n;
                moved = true;
            }
        }
        self.palette.selected = self.palette.selected.min(n.saturating_sub(1));

        let width = 600.0f32.min(screen.width() - 40.0);
        let mut run: Option<usize> = enter.then(|| hits.get(self.palette.selected).copied()).flatten();
        egui::Area::new(Id::new("palette")).order(Order::Foreground).anchor(Align2::CENTER_TOP, vec2(0.0, 64.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(12).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(icon::MAGNIFYING_GLASS).size(17.0).color(theme::TEXT_DIM));
                    let edit = egui::TextEdit::singleline(&mut self.palette.query)
                        .hint_text("Funktion oder Werkzeug suchen …")
                        .font(egui::FontId::proportional(16.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(4, 8));
                    let resp = ui.add(edit);
                    if self.palette.just_opened {
                        resp.request_focus();
                        self.palette.just_opened = false;
                    }
                    if resp.changed() {
                        self.palette.selected = 0;
                    }
                });
                ui.add_space(2.0);
                let (sep, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, theme::BORDER);
                ui.add_space(4.0);
                if hits.is_empty() {
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("Nichts gefunden").color(theme::TEXT_DIM)));
                    ui.add_space(10.0);
                    return;
                }
                if self.palette.query.trim().is_empty() && !self.palette.recent.is_empty() {
                    ui.add_space(2.0);
                    ui.label(RichText::new("   ZULETZT VERWENDET · ALLE BEFEHLE").size(10.0).color(theme::TEXT_FAINT));
                }
                egui::ScrollArea::vertical().max_height(420.0f32.min(screen.height() - 200.0)).auto_shrink([false, true]).show(ui, |ui| {
                    for (row, &ci) in hits.iter().enumerate() {
                        let c = &commands[ci];
                        let selected = row == self.palette.selected;
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            self.palette.selected = row;
                        }
                        if selected && moved {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() {
                            run = Some(ci);
                        }
                        command_row(ui, rect, c, selected);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let k = |t: &str| RichText::new(t).size(11.0).color(theme::TEXT_FAINT);
                    ui.label(k(&format!("{}{} auswählen    {} ausführen    Esc schließen", icon::ARROW_UP, icon::ARROW_DOWN, icon::KEY_RETURN)));
                });
            });
        });

        if let Some(ci) = run
            && let Some(c) = commands.get(ci)
            && c.enabled
        {
            self.palette.open = false;
            let label = c.label.clone();
            self.palette.recent.retain(|l| *l != label);
            self.palette.recent.insert(0, label);
            self.palette.recent.truncate(6);
            self.run_command(ctx, c.action);
        }
    }
}

fn command_row(ui: &Ui, rect: Rect, c: &Command, selected: bool) {
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 6.0, theme::ACCENT_SOFT);
    }
    let fg = if !c.enabled {
        theme::TEXT_FAINT
    } else if selected {
        Color32::WHITE
    } else {
        theme::TEXT
    };
    let icon_color = if !c.enabled {
        theme::TEXT_FAINT
    } else if selected {
        Color32::WHITE
    } else {
        theme::TEXT_DIM
    };
    p.text(rect.left_center() + vec2(14.0, 0.0), Align2::CENTER_CENTER, c.icon, egui::FontId::proportional(16.0), icon_color);
    let label = p.text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, &c.label, egui::FontId::proportional(13.5), fg);
    let mut right = rect.right() - 10.0;
    if !c.shortcut.is_empty() {
        let galley = p.layout_no_wrap(c.shortcut.clone(), egui::FontId::proportional(11.0), theme::TEXT_DIM);
        let chip = Rect::from_min_size(egui::pos2(right - galley.size().x - 12.0, rect.center().y - 10.0), vec2(galley.size().x + 12.0, 20.0));
        p.rect_filled(chip, 4.0, Color32::from_rgb(56, 56, 62));
        p.galley(chip.center() - galley.size() / 2.0, galley, theme::TEXT_DIM);
        right = chip.left() - 10.0;
    }
    if right - label.right() > 60.0 {
        p.text(egui::pos2(right, rect.center().y), Align2::RIGHT_CENTER, c.group, egui::FontId::proportional(11.5), theme::TEXT_FAINT);
    }
}

/// "Strg+1" … "Strg+0" for the tool tabs in rail order.
pub fn tab_shortcut(tab: Tab) -> String {
    tab.key().map(|k| crate::commands::key_text((egui::Modifiers::COMMAND, k))).unwrap_or_default()
}

/// Lowercase without diacritics, so "weiss" finds "Weiß" and "farbung" finds "Färbung".
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars().flat_map(char::to_lowercase) {
        match ch {
            'ä' => out.push('a'),
            'ö' => out.push('o'),
            'ü' => out.push('u'),
            'ß' => out.push_str("ss"),
            c => out.push(c),
        }
    }
    out
}

/// Indices of the matching commands, best first. Every word of the query has to occur in the
/// label, group or keywords; matches at the start of the label or of a word rank higher.
/// An empty query lists the recent commands first, then everything.
fn rank(commands: &[Command], query: &str, recent: &[String]) -> Vec<usize> {
    let words: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        let mut out: Vec<usize> = recent.iter().filter_map(|r| commands.iter().position(|c| &c.label == r)).collect();
        let rest: Vec<usize> = (0..commands.len()).filter(|i| !out.contains(i)).collect();
        out.extend(rest);
        return out;
    }
    let mut scored: Vec<(i32, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let label = fold(&c.label);
            let rest = fold(&format!("{} {}", c.group, c.keywords));
            let mut score = 0;
            for w in &words {
                score += if label.starts_with(w.as_str()) {
                    100
                } else if label.split(|ch: char| !ch.is_alphanumeric()).any(|part| part.starts_with(w.as_str())) {
                    60
                } else if label.contains(w.as_str()) {
                    35
                } else if rest.split_whitespace().any(|part| part.starts_with(w.as_str())) {
                    20
                } else if rest.contains(w.as_str()) {
                    10
                } else {
                    return None;
                };
            }
            if c.enabled {
                score += 5;
            }
            if recent.contains(&c.label) {
                score += 8;
            }
            Some((score, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(label: &str, keywords: &'static str) -> Command {
        Command { icon: "", label: label.into(), group: "Test", shortcut: String::new(), enabled: true, keywords, action: Action::Command("reset") }
    }

    #[test]
    fn finds_by_label_synonym_and_without_umlauts() {
        let cmds = [cmd("Weißabgleich", "white balance wb"), cmd("Rauschreduzierung", "noise"), cmd("Belichtung", "exposure")];
        assert_eq!(rank(&cmds, "weiss", &[]), vec![0]);
        assert_eq!(rank(&cmds, "noise", &[]), vec![1]);
        assert_eq!(rank(&cmds, "wb", &[]), vec![0]);
        assert!(rank(&cmds, "xyz", &[]).is_empty());
    }

    #[test]
    fn label_start_ranks_before_keyword_match() {
        let cmds = [cmd("Weißabgleich-Pipette", "kelvin"), cmd("Kelvin", "")];
        assert_eq!(rank(&cmds, "kel", &[]), vec![1, 0]);
    }

    #[test]
    fn empty_query_lists_recent_first() {
        let cmds = [cmd("A", ""), cmd("B", ""), cmd("C", "")];
        assert_eq!(rank(&cmds, " ", &["C".into()]), vec![2, 0, 1]);
    }
}
