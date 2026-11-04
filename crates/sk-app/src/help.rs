//! The manual: chapters in a small Markdown dialect (`help/*.md`, built into the program), shown
//! in the help window (F1) and written as web pages for `docs/hilfe` (see the test
//! `help_site_is_current`). The info symbol of every tool section links to its place in it.
//!
//! The dialect: `#`, `##`, `###` headings (`##`/`###` are the targets of links, by their text),
//! paragraphs, one level of `- ` and `1. ` lists, `> ` notes, `**bold**`, `` `code` `` and
//! `[text](chapter)` or `[text](chapter#Heading)` links. `{taste:command-id}` stands for the
//! command's current shortcut.

use egui::{Color32, RichText, Ui};

use crate::app::App;
use crate::theme::{self, icon};

pub struct Chapter {
    pub id: &'static str,
    pub icon: &'static str,
    text: &'static str,
}

/// The chapters in reading order. "Tastenkürzel" is made from the command list.
pub const CHAPTERS: &[Chapter] = &[
    Chapter { id: "erste-schritte", icon: icon::SHOOTING_STAR, text: include_str!("../help/erste-schritte.md") },
    Chapter { id: "bibliothek", icon: icon::IMAGES, text: include_str!("../help/bibliothek.md") },
    Chapter { id: "ansicht", icon: icon::MONITOR, text: include_str!("../help/ansicht.md") },
    Chapter { id: "objektiv", icon: icon::APERTURE, text: include_str!("../help/objektiv.md") },
    Chapter { id: "entwickeln", icon: icon::SUN, text: include_str!("../help/entwickeln.md") },
    Chapter { id: "ebenen-masken", icon: icon::STACK, text: include_str!("../help/ebenen-masken.md") },
    Chapter { id: "retusche", icon: icon::BANDAIDS, text: include_str!("../help/retusche.md") },
    Chapter { id: "ki-werkzeuge", icon: icon::SPARKLE, text: include_str!("../help/ki-werkzeuge.md") },
    Chapter { id: "zusammenfuegen", icon: icon::UNITE, text: include_str!("../help/zusammenfuegen.md") },
    Chapter { id: "vergleichen", icon: icon::SQUARE_SPLIT_HORIZONTAL, text: include_str!("../help/vergleichen.md") },
    Chapter { id: "verlauf", icon: icon::CLOCK_COUNTER_CLOCKWISE, text: include_str!("../help/verlauf.md") },
    Chapter { id: "ausgabe", icon: icon::EXPORT, text: include_str!("../help/ausgabe.md") },
    Chapter { id: SHORTCUTS, icon: icon::KEYBOARD, text: "" },
];

/// The chapter made from the command list.
pub const SHORTCUTS: &str = "tastenkuerzel";

impl Chapter {
    /// The text with the current shortcuts filled in.
    pub fn source(&self) -> String {
        if self.id == SHORTCUTS { shortcuts_chapter() } else { fill_keys(self.text) }
    }

    pub fn title(&self) -> String {
        match parse(&self.source()).first() {
            Some(Block::Heading(1, t)) => plain(t),
            _ => self.id.to_string(),
        }
    }
}

#[cfg(test)]
pub fn chapter(id: &str) -> Option<&'static Chapter> {
    CHAPTERS.iter().find(|c| c.id == id)
}

/// `{taste:id}`: the command's shortcut (or "ohne Taste").
pub(crate) fn fill_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{taste:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let key = crate::commands::shortcut_of(&after[..end]);
        out.push_str(if key.is_empty() { "(ohne Taste)" } else { &key });
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// "Tastenkürzel": every command with its shortcut, by menu.
fn shortcuts_chapter() -> String {
    use std::fmt::Write;
    let mut s = String::from(
        "# Tastenkürzel\n\nAlle Befehle mit ihren Tasten, nach Menüs. Jede Taste lässt sich ändern: **Tastenkürzel anpassen …** im Menü **Bearbeiten**. Befehle ohne Taste sind über die Befehlssuche erreichbar (Strg+K).\n",
    );
    let mut groups: Vec<&str> = Vec::new();
    for c in crate::commands::COMMANDS {
        if !groups.contains(&c.group) {
            groups.push(c.group);
        }
    }
    for g in groups {
        let _ = write!(s, "\n## {g}\n\n");
        for c in crate::commands::COMMANDS.iter().filter(|c| c.group == g) {
            let key = c.shortcut();
            if !key.is_empty() {
                let _ = writeln!(s, "- **{}**: {key}", c.label.trim_end_matches(" …"));
            }
        }
    }
    fill_keys(&s)
}

// ---------------------------------------------------------------- parsing

#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(String),
    Bold(String),
    Code(String),
    Link(String, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Bullets(Vec<Vec<Span>>),
    Steps(Vec<Vec<Span>>),
    Note(Vec<Span>),
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut note: Vec<&str> = Vec::new();
    let flush = |blocks: &mut Vec<Block>, para: &mut Vec<&str>, note: &mut Vec<&str>| {
        if !para.is_empty() {
            blocks.push(Block::Para(inline(&para.join(" "))));
            para.clear();
        }
        if !note.is_empty() {
            blocks.push(Block::Note(inline(&note.join(" "))));
            note.clear();
        }
    };
    for line in text.lines() {
        let t = line.trim_end();
        let level = t.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&level) && t[level..].starts_with(' ') {
            flush(&mut blocks, &mut para, &mut note);
            blocks.push(Block::Heading(level as u8, inline(t[level..].trim())));
        } else if let Some(item) = t.strip_prefix("- ") {
            flush(&mut blocks, &mut para, &mut note);
            match blocks.last_mut() {
                Some(Block::Bullets(items)) => items.push(inline(item.trim())),
                _ => blocks.push(Block::Bullets(vec![inline(item.trim())])),
            }
        } else if let Some(item) = step(t) {
            flush(&mut blocks, &mut para, &mut note);
            match blocks.last_mut() {
                Some(Block::Steps(items)) => items.push(inline(item.trim())),
                _ => blocks.push(Block::Steps(vec![inline(item.trim())])),
            }
        } else if let Some(n) = t.strip_prefix('>') {
            if !para.is_empty() {
                flush(&mut blocks, &mut para, &mut Vec::new());
            }
            note.push(n.trim());
        } else if t.trim().is_empty() {
            flush(&mut blocks, &mut para, &mut note);
        } else if line.starts_with("  ") && matches!(blocks.last(), Some(Block::Bullets(_) | Block::Steps(_))) && para.is_empty() {
            // The continuation of a list item.
            if let Some(Block::Bullets(items) | Block::Steps(items)) = blocks.last_mut()
                && let Some(last) = items.last_mut()
            {
                last.push(Span::Text(" ".into()));
                last.extend(inline(t.trim()));
            }
        } else {
            if !note.is_empty() {
                flush(&mut blocks, &mut Vec::new(), &mut note);
            }
            para.push(t.trim());
        }
    }
    flush(&mut blocks, &mut para, &mut note);
    blocks
}

/// "1. text" gives "text".
fn step(t: &str) -> Option<&str> {
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    (digits > 0 && t[digits..].starts_with(". ")).then(|| &t[digits + 2..])
}

pub fn inline(s: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let found = if let Some(bold) = rest.strip_prefix("**") {
            bold.find("**").map(|end| (Span::Bold(bold[..end].to_string()), 4 + end))
        } else if c == '`' {
            rest[1..].find('`').map(|end| (Span::Code(rest[1..1 + end].to_string()), 2 + end))
        } else if c == '[' {
            rest.find("](").and_then(|mid| {
                let end = rest[mid..].find(')')? + mid;
                Some((Span::Link(rest[1..mid].to_string(), rest[mid + 2..end].to_string()), end + 1))
            })
        } else {
            None
        };
        match found {
            Some((span, len)) => {
                if !text.is_empty() {
                    spans.push(Span::Text(std::mem::take(&mut text)));
                }
                spans.push(span);
                rest = &rest[len..];
            }
            None => {
                text.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !text.is_empty() {
        spans.push(Span::Text(text));
    }
    spans
}

pub fn plain(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| match s {
            Span::Text(t) | Span::Bold(t) | Span::Code(t) | Span::Link(t, _) => t.as_str(),
        })
        .collect()
}

/// A link target: chapter and heading (`chapter#Heading`); without a chapter, the current one.
pub fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((c, h)) => (c, Some(h)),
        None => (target, None),
    }
}

/// Headings are found by their text, without regard to case.
fn same_heading(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The `##`/`###` headings of a chapter.
pub fn headings(source: &str) -> Vec<(u8, String)> {
    parse(source)
        .into_iter()
        .filter_map(|b| match b {
            Block::Heading(l @ 2..=3, t) => Some((l, plain(&t))),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- the window

#[derive(Default)]
pub struct HelpView {
    pub open: bool,
    pub chapter: usize,
    /// A heading to scroll to (once).
    scroll_to: Option<String>,
    pub search: String,
    /// Chapters visited before, for "Zurück".
    back: Vec<usize>,
}

impl App {
    /// Opens the manual at `target` ("chapter" or "chapter#Heading").
    pub fn open_help(&mut self, target: &str) {
        let (c, _) = split_target(target);
        let h = &mut self.help;
        if let Some(i) = CHAPTERS.iter().position(|ch| ch.id == c) {
            if h.open && h.chapter != i {
                h.back.push(h.chapter);
            }
            h.chapter = i;
        }
        h.scroll_to = Some(self::split_target(target).1.unwrap_or("").to_string());
        h.open = true;
    }

    pub fn toggle_help(&mut self) {
        self.help.open = !self.help.open;
    }

    /// The help window: chapters on the left, the text on the right. Not modal, so that one can
    /// follow it in the program.
    pub(crate) fn help_window(&mut self, ctx: &egui::Context) {
        // The info symbol of a section asked for its place in the manual.
        if let Some(target) = ctx.data_mut(|d| d.remove_temp::<String>(open_help_id())) {
            self.open_help(&target);
        }
        if !self.help.open {
            return;
        }
        let mut open = true;
        let mut go: Option<String> = None;
        let mut open_editor = false;
        let screen = ctx.content_rect();
        let height = (screen.height() - 140.0).clamp(360.0, 760.0);
        egui::Window::new(format!("{}  Hilfe", icon::QUESTION))
            .id(egui::Id::new("help_window"))
            .open(&mut open)
            .default_size([880.0, height])
            .default_pos(screen.center() - egui::vec2(440.0, height / 2.0))
            .min_width(600.0)
            .min_height(300.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let h = &mut self.help;
                let full = ui.available_height();
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // Chapters and search, on a darker ground.
                    egui::Frame::new().fill(theme::BROWSER).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        ui.set_width(212.0);
                        ui.set_height(full - 16.0);
                        ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(egui::TextEdit::singleline(&mut h.search).hint_text(format!("{}  Suchen", icon::MAGNIFYING_GLASS)).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().id_salt("help_toc").auto_shrink([false, false]).show(ui, |ui| {
                            let query = h.search.trim().to_lowercase();
                            if query.is_empty() {
                                for (i, c) in CHAPTERS.iter().enumerate() {
                                    let selected = h.chapter == i;
                                    if toc_row(ui, selected, Some(c.icon), &c.title(), None).clicked() {
                                        go = Some(c.id.to_string());
                                    }
                                    // The sections of the open chapter, to jump to.
                                    if selected {
                                        for (level, heading) in headings(&c.source()) {
                                            if level == 2 && toc_row(ui, false, None, &heading, None).clicked() {
                                                go = Some(format!("{}#{heading}", c.id));
                                            }
                                        }
                                        ui.add_space(4.0);
                                    }
                                }
                            } else {
                                let hits = search(&query);
                                if hits.is_empty() {
                                    ui.add_space(8.0);
                                    ui.label(RichText::new("Nichts gefunden").color(theme::TEXT_DIM));
                                }
                                for (target, chapter, heading) in hits {
                                    let (title, sub) = match heading {
                                        Some(hd) => (hd, Some(chapter)),
                                        None => (chapter, None),
                                    };
                                    if toc_row(ui, false, None, &title, sub.as_deref()).clicked() {
                                        go = Some(target);
                                    }
                                }
                            }
                        });
                        });
                    });
                    ui.add_space(4.0);
                    // The chapter.
                    ui.vertical(|ui| {
                        let i = h.chapter.min(CHAPTERS.len() - 1);
                        let c = &CHAPTERS[i];
                        let scroll_to = h.scroll_to.take();
                        let mut area = egui::ScrollArea::vertical().id_salt(("help_text", i)).auto_shrink([false, false]);
                        if scroll_to.as_deref() == Some("") {
                            area = area.vertical_scroll_offset(0.0);
                        }
                        area.show(ui, |ui| {
                            egui::Frame::new().inner_margin(egui::Margin { left: 28, right: 20, top: 10, bottom: 24 }).show(ui, |ui| {
                                // A comfortable line length, also in a wide window.
                                let width = ui.available_width().min(TEXT_WIDTH);
                                ui.set_width(width);
                                if !h.back.is_empty() {
                                    if ui.add(egui::Button::new(RichText::new(format!("{}  Zurück", icon::ARROW_LEFT)).color(theme::TEXT_DIM)).frame(false)).clicked()
                                        && let Some(prev) = h.back.pop()
                                    {
                                        h.chapter = prev;
                                        h.scroll_to = Some(String::new());
                                    }
                                    ui.add_space(4.0);
                                }
                                let blocks = parse(&c.source());
                                if let Some(link) = show_blocks(ui, &blocks, c, scroll_to.as_deref()) {
                                    go = Some(link);
                                }
                                if c.id == SHORTCUTS {
                                    ui.add_space(12.0);
                                    if ui.button(format!("{}  Tastenkürzel anpassen …", icon::KEYBOARD)).clicked() {
                                        open_editor = true;
                                    }
                                }
                                // On to the neighbours.
                                ui.add_space(28.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if let Some(prev) = i.checked_sub(1).map(|p| &CHAPTERS[p])
                                        && neighbour(ui, "Vorheriges Kapitel", &format!("{}  {}", icon::ARROW_LEFT, prev.title()), egui::Align::Min).clicked()
                                    {
                                        go = Some(prev.id.to_string());
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                        if let Some(next) = CHAPTERS.get(i + 1)
                                            && neighbour(ui, "Nächstes Kapitel", &format!("{}  {}", next.title(), icon::ARROW_RIGHT), egui::Align::Max).clicked()
                                        {
                                            go = Some(next.id.to_string());
                                        }
                                    });
                                });
                            });
                        });
                    });
                });
            });
        if let Some(target) = go {
            let (c, h) = split_target(&target);
            let target = if c.is_empty() { format!("{}#{}", CHAPTERS[self.help.chapter].id, h.unwrap_or("")) } else { target.clone() };
            self.open_help(&target);
        }
        if open_editor {
            self.show_shortcuts = true;
        }
        self.help.open &= open;
    }
}

/// The longest line of the text, in points.
const TEXT_WIDTH: f32 = 620.0;
/// The text size and the distance of its lines.
const BODY: f32 = 14.0;
const LINE: f32 = 21.0;

/// A row of the chapter list: a chapter (with icon), a section of the open one (indented), or a
/// search hit (with its chapter below).
fn toc_row(ui: &mut Ui, selected: bool, icon: Option<&str>, text: &str, sub: Option<&str>) -> egui::Response {
    let height = match (icon, sub) {
        (_, Some(_)) => 40.0,
        (Some(_), None) => 30.0,
        (None, None) => 24.0,
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        theme::ACCENT_SOFT
    } else if resp.hovered() {
        theme::CONTROL.gamma_multiply(0.6)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 6.0, fill);
    let color = if selected { Color32::WHITE } else if icon.is_some() || sub.is_some() { theme::TEXT } else { theme::TEXT_DIM };
    let mut x = rect.left() + 10.0;
    if let Some(ic) = icon {
        painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, ic, egui::FontId::proportional(15.0), if selected { Color32::WHITE } else { theme::TEXT_DIM });
        x += 26.0;
    } else if sub.is_none() {
        // A section of the open chapter: below its title, with a thin line on the left.
        x += 26.0;
        painter.line_segment([egui::pos2(x - 10.0, rect.top()), egui::pos2(x - 10.0, rect.bottom())], egui::Stroke::new(1.0, theme::BORDER));
    }
    let width = rect.right() - x - 6.0;
    let size = if icon.is_some() || sub.is_some() { 13.5 } else { 12.5 };
    let name = fit(painter, text, size, width);
    match sub {
        Some(sub) => {
            painter.text(egui::pos2(x, rect.top() + 12.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(size), color);
            painter.text(egui::pos2(x, rect.top() + 28.0), egui::Align2::LEFT_CENTER, fit(painter, sub, 11.5, width), egui::FontId::proportional(11.5), theme::TEXT_FAINT);
        }
        None => {
            painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(size), color);
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// `text`, shortened with "…" to `width`.
fn fit(painter: &egui::Painter, text: &str, size: f32, width: f32) -> String {
    let font = egui::FontId::proportional(size);
    let fits = |t: &str| painter.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let t: String = chars[..n].iter().collect::<String>().trim_end().to_string() + " …";
        if fits(&t) {
            return t;
        }
    }
    String::new()
}

/// "Vorheriges Kapitel" / "Nächstes Kapitel" at the end of a chapter.
fn neighbour(ui: &mut Ui, what: &str, title: &str, align: egui::Align) -> egui::Response {
    let font = egui::FontId::proportional(14.0);
    let title_galley = ui.painter().layout_no_wrap(screen_symbols(title), font, theme::ACCENT);
    let what_galley = ui.painter().layout_no_wrap(what.to_string(), egui::FontId::proportional(11.5), theme::TEXT_FAINT);
    let size = egui::vec2(title_galley.size().x.max(what_galley.size().x) + 20.0, 46.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme::CONTROL.gamma_multiply(0.6));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let x = |w: f32| if align == egui::Align::Max { rect.right() - 10.0 - w } else { rect.left() + 10.0 };
    ui.painter().galley(egui::pos2(x(what_galley.size().x), rect.top() + 6.0), what_galley, theme::TEXT_FAINT);
    ui.painter().galley(egui::pos2(x(title_galley.size().x), rect.top() + 22.0), title_galley, theme::ACCENT);
    resp
}

/// Chapters and headings with `query` in their text: (target, chapter, heading).
fn search(query: &str) -> Vec<(String, String, Option<String>)> {
    let mut hits = Vec::new();
    for c in CHAPTERS {
        let source = c.source();
        let title = c.title();
        // The text below each heading belongs to it.
        let mut current: Option<String> = None;
        let mut text = String::new();
        let push = |current: &Option<String>, text: &str, hits: &mut Vec<(String, String, Option<String>)>| {
            let found = text.to_lowercase().contains(query) || current.as_ref().is_some_and(|h| h.to_lowercase().contains(query));
            if found {
                match current {
                    Some(h) => hits.push((format!("{}#{h}", c.id), title.clone(), Some(h.clone()))),
                    None => hits.push((c.id.to_string(), title.clone(), None)),
                }
            }
        };
        for line in source.lines() {
            if let Some(h) = line.strip_prefix("## ").or_else(|| line.strip_prefix("### ")) {
                push(&current, &text, &mut hits);
                current = Some(plain(&inline(h.trim())));
                text.clear();
            } else if !line.starts_with("# ") {
                text.push_str(line);
                text.push('\n');
            }
        }
        push(&current, &text, &mut hits);
    }
    hits
}

/// Draws the blocks; returns a clicked link's target.
fn show_blocks(ui: &mut Ui, blocks: &[Block], chapter: &Chapter, scroll_to: Option<&str>) -> Option<String> {
    let mut clicked = None;
    let mut lead = true;
    for b in blocks {
        let mut link = None;
        match b {
            Block::Heading(1, spans) => {
                // The chapter's title, with its icon.
                ui.horizontal(|ui| {
                    ui.label(RichText::new(chapter.icon).size(24.0).color(theme::ACCENT));
                    ui.add_space(4.0);
                    ui.label(RichText::new(plain(spans)).size(25.0).strong().color(Color32::WHITE));
                });
                ui.add_space(10.0);
            }
            Block::Heading(level, spans) => {
                let title = plain(spans);
                lead = false;
                let r = if *level == 2 {
                    ui.add_space(26.0);
                    let r = ui.label(RichText::new(&title).size(18.5).strong().color(Color32::WHITE));
                    ui.add_space(3.0);
                    let y = ui.cursor().top();
                    ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, theme::BORDER));
                    ui.add_space(10.0);
                    r
                } else {
                    ui.add_space(14.0);
                    let r = ui.label(RichText::new(&title).size(15.0).strong().color(theme::GLOW));
                    ui.add_space(4.0);
                    r
                };
                if scroll_to.is_some_and(|t| !t.is_empty() && same_heading(t, &title)) {
                    r.scroll_to_me(Some(egui::Align::TOP));
                }
            }
            Block::Para(spans) => {
                // The first paragraph introduces the chapter, a little larger.
                let (size, line) = if lead { (15.0, 23.0) } else { (BODY, LINE) };
                lead = false;
                link = text(ui, spans, size, line, theme::TEXT);
                ui.add_space(10.0);
            }
            Block::Bullets(items) if chapter.id == SHORTCUTS => {
                shortcut_rows(ui, items);
                ui.add_space(6.0);
            }
            Block::Bullets(items) | Block::Steps(items) => {
                lead = false;
                let numbered = matches!(b, Block::Steps(_));
                for (i, item) in items.iter().enumerate() {
                    let gutter = if numbered { 30.0 } else { 20.0 };
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let (mark, _) = ui.allocate_exact_size(egui::vec2(gutter, LINE), egui::Sense::hover());
                        let c = egui::pos2(mark.left() + if numbered { 10.0 } else { 7.0 }, mark.center().y);
                        if numbered {
                            ui.painter().circle_filled(c, 9.5, theme::ACCENT_SOFT);
                            ui.painter().text(c, egui::Align2::CENTER_CENTER, (i + 1).to_string(), egui::FontId::proportional(11.5), Color32::WHITE);
                        } else {
                            ui.painter().circle_filled(c, 2.5, theme::ACCENT);
                        }
                        ui.vertical(|ui| {
                            if let Some(l) = text(ui, item, BODY, LINE, theme::TEXT) {
                                link = Some(l);
                            }
                        });
                    });
                    ui.add_space(if numbered { 6.0 } else { 4.0 });
                }
                ui.add_space(6.0);
            }
            Block::Note(spans) => {
                lead = false;
                ui.add_space(2.0);
                let frame = egui::Frame::new().fill(theme::ACCENT_SOFT.gamma_multiply(0.35)).corner_radius(6).inner_margin(egui::Margin { left: 16, right: 12, top: 10, bottom: 10 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_top(|ui| {
                        ui.add_sized([22.0, LINE], egui::Label::new(RichText::new(icon::INFO).size(15.0).color(theme::GLOW)));
                        ui.vertical(|ui| link = text(ui, spans, BODY, LINE, theme::TEXT));
                    });
                });
                // The bar on the left.
                let r = frame.response.rect;
                ui.painter().rect_filled(egui::Rect::from_min_max(r.min, egui::pos2(r.left() + 3.0, r.bottom())), egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, theme::ACCENT);
                ui.add_space(12.0);
            }
        }
        if link.is_some() {
            clicked = link;
        }
    }
    clicked
}

/// The shortcut chapter's lists ("**Name**: key") as a table: the name left, the key right.
fn shortcut_rows(ui: &mut Ui, items: &[Vec<Span>]) {
    for (i, item) in items.iter().enumerate() {
        let (name, key) = match item.as_slice() {
            [Span::Bold(n), Span::Text(k)] => (n.clone(), k.trim_start_matches(':').trim().to_string()),
            _ => (plain(item), String::new()),
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
        if i % 2 == 0 {
            ui.painter().rect_filled(rect, 4.0, Color32::from_white_alpha(6));
        }
        ui.painter().text(egui::pos2(rect.left() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, screen_symbols(&name), egui::FontId::proportional(13.5), theme::TEXT);
        ui.painter().text(egui::pos2(rect.right() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, key, egui::FontId::proportional(13.0), theme::TEXT_DIM);
    }
}

/// Text that wraps, with bold, code and links; returns a clicked link's target.
fn text(ui: &mut Ui, spans: &[Span], size: f32, line: f32, color: Color32) -> Option<String> {
    let width = ui.available_width();
    let layout = |hover: Option<usize>| {
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = width;
        let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        let mut chars = 0;
        for span in spans {
            let mut format = egui::TextFormat { font_id: egui::FontId::proportional(size), color, line_height: Some(line), valign: egui::Align::Center, ..Default::default() };
            let t = match span {
                Span::Text(t) => screen_symbols(t),
                Span::Bold(t) => {
                    format.color = Color32::WHITE;
                    screen_symbols(t)
                }
                Span::Code(t) => {
                    format.font_id = egui::FontId::monospace(size - 1.5);
                    format.background = theme::CONTROL;
                    t.clone()
                }
                Span::Link(t, target) => {
                    format.color = theme::GLOW;
                    let n = t.chars().count();
                    if hover == Some(links.len()) {
                        format.underline = egui::Stroke::new(1.0, theme::GLOW);
                    }
                    links.push((chars..chars + n, target.clone()));
                    screen_symbols(t)
                }
            };
            chars += t.chars().count();
            job.append(&t, 0.0, format);
        }
        (job, links)
    };
    let (job, links) = layout(None);
    let galley = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), if links.is_empty() { egui::Sense::hover() } else { egui::Sense::click() });
    // The link under the pointer.
    let over = resp.hover_pos().and_then(|p| {
        let i = galley.cursor_from_pos(p - rect.min).index.0;
        links.iter().position(|(r, _)| r.contains(&i) || (i > 0 && r.contains(&(i - 1)) && r.end != i))
    });
    let galley = if over.is_some() { ui.painter().layout_job(layout(over).0) } else { galley };
    ui.painter().galley(rect.min, galley, color);
    let over = over?;
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    resp.clicked().then(|| links[over].1.clone())
}

/// A paragraph in other places (the tours): wraps, with bold and links.
pub(crate) fn show_spans(ui: &mut Ui, spans: &[Span]) -> Option<String> {
    text(ui, spans, 13.5, 20.0, theme::TEXT)
}

/// Symbols the font lacks (the web pages keep them): the program's own instead.
pub(crate) fn screen_symbols(s: &str) -> String {
    s.replace('\u{24D8}', icon::INFO).replace('\u{2192}', icon::ARROW_RIGHT).replace('\u{2715}', icon::X)
}

/// Where a section's info symbol leaves the manual target for the help window.
pub fn open_help_id() -> egui::Id {
    egui::Id::new("open_help")
}

/// The short help and the manual target of a tool section (by its title).
pub fn section_help(title: &str) -> Option<(&'static str, &'static str)> {
    // Titles with a count or a name after them ("Bewertung (3 Bilder)", "Session „…“") are found
    // by their start, followed by a space ("Session" is not "Session-Favoriten").
    let base = |t: &str| title == t || title.strip_prefix(t).is_some_and(|rest| rest.starts_with(' '));
    crate::help_sections::SECTION_HELP.iter().find(|(t, ..)| base(t)).map(|(_, short, target)| (*short, *target))
}

// ---------------------------------------------------------------- web pages

#[cfg(test)]
/// The manual as web pages: (file name, HTML), with an index.
pub fn site() -> Vec<(String, String)> {
    let mut pages = Vec::new();
    let mut toc = String::new();
    for c in CHAPTERS {
        toc.push_str(&format!("<li><a href=\"{}.html\">{}</a></li>\n", c.id, esc(&c.title())));
    }
    for c in CHAPTERS {
        let body = html_blocks(&parse(&c.source()));
        pages.push((format!("{}.html", c.id), page(&c.title(), &toc, &body)));
    }
    let index = format!(
        "<h1>Silberkorn – Hilfe</h1>\n<p>Das Handbuch zu Silberkorn. Im Programm öffnet es sich mit F1 (Menü <b>Hilfe</b>), die \u{24D8} an jedem Werkzeug führen an die passende Stelle.</p>\n<ul class=\"chapters\">\n{toc}</ul>\n"
    );
    pages.push(("index.html".into(), page("Hilfe", &toc, &index)));
    pages
}

#[cfg(test)]
fn page(title: &str, toc: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="de">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{t} – Silberkorn</title>
<link rel="stylesheet" href="hilfe.css">
</head>
<body>
<nav><a class="home" href="../index.html">Silberkorn</a> · <a href="index.html">Hilfe</a>
<ul>
{toc}</ul></nav>
<main>
{body}</main>
</body>
</html>
"#,
        t = esc(title)
    )
}

#[cfg(test)]
/// The style of the web pages (written as `hilfe.css`).
pub const CSS: &str = r#"body{margin:0;display:flex;gap:32px;background:#1e1e20;color:#dedee2;font:16px/1.6 system-ui,-apple-system,"Segoe UI",sans-serif}
nav{flex:0 0 220px;padding:24px 16px;background:#18181a;min-height:100vh;box-sizing:border-box}
nav ul{list-style:none;padding:0;margin:16px 0 0}nav li{margin:4px 0}
a{color:#4e92e6;text-decoration:none}a:hover{text-decoration:underline}
nav .home{font-weight:600;color:#fff}
main{max-width:720px;padding:24px 16px 64px}
h1{color:#fff;font-size:28px}h2{color:#fff;margin-top:32px}h3{color:#fff}
code{background:#2a2a2e;padding:1px 4px;border-radius:4px}
.note{background:#2d4666;border-radius:6px;padding:8px 12px;margin:12px 0}
@media (max-width:760px){body{display:block}nav{min-height:0}}
"#;

#[cfg(test)]
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
/// Heading anchor for the web pages.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
fn html_spans(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| match s {
            Span::Text(t) => esc(t),
            Span::Bold(t) => format!("<b>{}</b>", esc(t)),
            Span::Code(t) => format!("<code>{}</code>", esc(t)),
            Span::Link(t, target) => {
                let (c, h) = split_target(target);
                let file = if c.is_empty() { String::new() } else { format!("{c}.html") };
                let anchor = h.map(|h| format!("#{}", slug(h))).unwrap_or_default();
                format!("<a href=\"{file}{anchor}\">{}</a>", esc(t))
            }
        })
        .collect()
}

#[cfg(test)]
fn html_blocks(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        match b {
            Block::Heading(l, s) => out.push_str(&format!("<h{l} id=\"{}\">{}</h{l}>\n", slug(&plain(s)), html_spans(s))),
            Block::Para(s) => out.push_str(&format!("<p>{}</p>\n", html_spans(s))),
            Block::Bullets(items) | Block::Steps(items) => {
                let tag = if matches!(b, Block::Steps(_)) { "ol" } else { "ul" };
                out.push_str(&format!("<{tag}>\n"));
                for i in items {
                    out.push_str(&format!("<li>{}</li>\n", html_spans(i)));
                }
                out.push_str(&format!("</{tag}>\n"));
            }
            Block::Note(s) => out.push_str(&format!("<p class=\"note\">{}</p>\n", html_spans(s))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialect_is_read() {
        let b = parse("# Titel\n\nEin **fetter** Satz mit [Link](objektiv#Zuschnitt)\nund weiter.\n\n- eins\n- zwei\n\n1. erst\n2. dann\n\n> Hinweis\n\n## Abschnitt");
        assert_eq!(b[0], Block::Heading(1, vec![Span::Text("Titel".into())]));
        assert_eq!(
            b[1],
            Block::Para(vec![
                Span::Text("Ein ".into()),
                Span::Bold("fetter".into()),
                Span::Text(" Satz mit ".into()),
                Span::Link("Link".into(), "objektiv#Zuschnitt".into()),
                Span::Text(" und weiter.".into()),
            ])
        );
        assert!(matches!(&b[2], Block::Bullets(i) if i.len() == 2));
        assert!(matches!(&b[3], Block::Steps(i) if i.len() == 2));
        assert_eq!(b[4], Block::Note(vec![Span::Text("Hinweis".into())]));
        assert_eq!(b[5], Block::Heading(2, vec![Span::Text("Abschnitt".into())]));
    }

    #[test]
    fn keys_are_the_current_ones() {
        assert_eq!(fill_keys("mit {taste:undo} zurück"), format!("mit {} zurück", crate::commands::shortcut_of("undo")));
        assert_eq!(slug("Lichter & Tiefen"), "lichter-tiefen");
        // The texts name keys only for commands that have one.
        for c in CHAPTERS {
            assert!(!c.source().contains("(ohne Taste)"), "{}: Platzhalter für einen Befehl ohne Taste", c.id);
        }
        for t in crate::tour::TOURS {
            for s in t.steps {
                assert!(!fill_keys(s.text).contains("(ohne Taste)"), "{}: Platzhalter für einen Befehl ohne Taste", s.title);
            }
        }
    }

    /// Every chapter has a title, and every link and section target leads to a chapter and
    /// heading that exist; every placeholder names a command.
    #[test]
    fn links_lead_somewhere() {
        let mut bad = Vec::new();
        let check = |target: &str, from: &str, bad: &mut Vec<String>| {
            let (c, h) = split_target(target);
            let c = if c.is_empty() { from } else { c };
            match chapter(c) {
                None => bad.push(format!("{from}: Kapitel {target}")),
                Some(ch) => {
                    if let Some(h) = h
                        && !headings(&ch.source()).iter().any(|(_, t)| same_heading(t, h))
                    {
                        bad.push(format!("{from}: Überschrift {target}"));
                    }
                }
            }
        };
        for c in CHAPTERS {
            assert!(matches!(parse(&c.source()).first(), Some(Block::Heading(1, _))), "{}: Titel fehlt", c.id);
            assert!(!c.source().contains("{taste:"), "{}: unbekannter Befehl in {{taste:…}}", c.id);
            for b in parse(&c.source()) {
                let spans: Vec<Span> = match b {
                    Block::Heading(_, s) | Block::Para(s) | Block::Note(s) => s,
                    Block::Bullets(i) | Block::Steps(i) => i.into_iter().flatten().collect(),
                };
                for s in spans {
                    if let Span::Link(_, target) = s {
                        check(&target, c.id, &mut bad);
                    }
                }
            }
        }
        for (title, _, target) in crate::help_sections::SECTION_HELP {
            check(target, title, &mut bad);
        }
        assert!(bad.is_empty(), "Verweise ohne Ziel:\n{}", bad.join("\n"));
    }

    /// The web pages in `docs/hilfe` are those of the chapters (`SILBERKORN_WRITE_HELP=1`
    /// writes them).
    #[test]
    fn help_site_is_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/hilfe");
        let mut files = site();
        files.push(("hilfe.css".into(), CSS.into()));
        if std::env::var_os("SILBERKORN_WRITE_HELP").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            for (name, html) in &files {
                std::fs::write(dir.join(name), html).unwrap();
            }
        }
        for (name, html) in &files {
            let on_disk = std::fs::read_to_string(dir.join(name)).unwrap_or_default();
            assert!(on_disk == *html, "docs/hilfe/{name} ist veraltet: SILBERKORN_WRITE_HELP=1 cargo test help_site schreibt die Seiten neu");
        }
    }
}
