//! "Speicherplatz …": what Silberkorn keeps on disk to be faster (previews, depth maps, Raw-Details
//! and denoised data), how full each cache is, and emptying it.

use egui::RichText;

use crate::ai_products::AiProduct;
use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::theme::{self, icon};

pub const DIALOG_TITLE: &str = "Speicherplatz …";

/// The caches, in the order of the dialog.
#[derive(Clone, Copy)]
enum Cache {
    Previews,
    Depth,
    Product(AiProduct),
}

impl Cache {
    const ALL: [Cache; 4] = [Cache::Previews, Cache::Depth, Cache::Product(AiProduct::Details), Cache::Product(AiProduct::Denoise)];

    fn title(self) -> &'static str {
        match self {
            Cache::Previews => "Vorschauen und Miniaturen",
            Cache::Depth => "Tiefenkarten",
            Cache::Product(k) => k.label(),
        }
    }

    /// What it is for (tooltip of its name).
    fn note(self) -> &'static str {
        match self {
            Cache::Previews => "Für die Bibliothek; Sessions haben ihre eigenen im Session-Ordner",
            Cache::Depth => "Für Tiefen-Auswahl, Atmosphäre und Objektivunschärfe",
            Cache::Product(AiProduct::Details) => "Die Bilddaten von Raw-Details",
            Cache::Product(AiProduct::Denoise) => "Die entrauschten Bilddaten",
        }
    }

    fn limit(self) -> u64 {
        match self {
            Cache::Previews => sk_core::cache::user_cache_limit(),
            Cache::Depth => sk_core::depth::CACHE_LIMIT,
            Cache::Product(k) => k.products().cache_limit,
        }
    }

    fn size(self) -> u64 {
        match self {
            Cache::Previews => sk_core::cache::user_cache_size(),
            Cache::Depth => sk_core::depth::cache_size(),
            Cache::Product(k) => k.products().cache_size(),
        }
    }
}

/// Gigabytes with one decimal (megabytes below 0.1 GB).
fn amount(bytes: u64) -> String {
    if bytes < 100 << 20 { format!("{} MB", bytes >> 20) } else { format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64) }
}

impl App {
    pub(crate) fn storage_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_storage_dialog {
            self.storage_sizes = None;
            return;
        }
        // Measured once on opening and after emptying (reading the folders takes a moment).
        let sizes = *self.storage_sizes.get_or_insert_with(|| Cache::ALL.map(Cache::size));
        let total: u64 = sizes.iter().sum();
        let mut clear: Option<Cache> = None;
        let mut close = false;
        let running = AiProduct::ALL.map(|k| self.product_running(k));
        let title = DIALOG_TITLE.trim_end_matches(" …");
        let subtitle = format!("Zwischenspeicher, um schneller zu sein · zusammen {}", amount(total));
        let modal = Dialog::new("storage", icon::HARD_DRIVES, title).subtitle(&subtitle).width(460.0).show(ctx, |ui| {
            dialog::card(ui, "Zwischenspeicher", None, |ui| {
                dialog::note(ui, "Alles hier lässt sich jederzeit leeren: es wird bei Bedarf neu berechnet. Ist ein Speicher voll, fallen die ältesten Einträge heraus.");
                for (cache, used) in Cache::ALL.into_iter().zip(sizes) {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        // Room for the button on the right.
                        let width = (ui.available_width() - 110.0).max(80.0);
                        ui.vertical(|ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let limit = cache.limit();
                            ui.label(RichText::new(format!("{} · {} von {}", cache.title(), amount(used), amount(limit))).size(12.0).color(theme::TEXT))
                                .on_hover_text(cache.note());
                            ui.add(egui::ProgressBar::new((used as f64 / limit.max(1) as f64).min(1.0) as f32).desired_width(width).desired_height(4.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let busy = matches!(cache, Cache::Product(k) if running[k as usize]);
                            let button = ui.add_enabled(used > 0 && !busy, dialog::secondary("Leeren"));
                            if button.on_disabled_hover_text(if busy { "Während der Berechnung nicht möglich" } else { "Leer" }).clicked() {
                                clear = Some(cache);
                            }
                        });
                    });
                }
            });
            dialog::footer(ui, |ui| {
                if dialog::default_button(ui, true, dialog::primary("Schließen")) {
                    close = true;
                }
            });
        });
        if let Some(cache) = clear {
            self.clear_cache(cache);
            self.storage_sizes = None;
        }
        if close || modal.should_close() {
            self.show_storage_dialog = false;
        }
    }

    fn clear_cache(&mut self, cache: Cache) {
        let result = match cache {
            Cache::Previews => {
                sk_core::cache::clear_user_cache();
                Ok(())
            }
            Cache::Depth => sk_core::depth::clear_cache(),
            Cache::Product(k) => self.clear_product_cache(k),
        };
        self.status = match result {
            Ok(()) => format!("{}: Zwischenspeicher geleert", cache.title()),
            Err(e) => format!("Zwischenspeicher nicht geleert: {e:#}"),
        };
    }
}
