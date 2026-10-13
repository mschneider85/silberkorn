//! Start screen in the style of Adobe's splash screens: a black-and-white photograph with the
//! wordmark, shown while the app starts up and again as the "about" dialog.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Id, Order, Pos2, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2};

use crate::app::App;
use crate::theme::{self, icon};

/// Ansel Adams, "The Tetons and the Snake River" (1942), public domain (work of the U.S. government),
/// cropped to the card's aspect ratio. Source: Wikimedia Commons.
const PHOTO: &[u8] = include_bytes!("../assets/splash.jpg");
pub const CREDIT: &str = "Foto: Ansel Adams, The Tetons and the Snake River (1942) · gemeinfrei";

/// Shown at least this long at startup, at most `MAX_SECS` while the first image loads.
const MIN_SECS: f64 = 1.8;
const MAX_SECS: f64 = 6.0;
const FADE_SECS: f64 = 0.35;
const CARD_ASPECT: f32 = 1.52;

pub struct Splash {
    /// Opened from "Über Silberkorn": stays until clicked.
    about: bool,
    texture: Option<TextureHandle>,
    shown_at: Option<f64>,
    closing_at: Option<f64>,
}

impl Splash {
    pub fn startup() -> Self {
        Self { about: false, texture: None, shown_at: None, closing_at: None }
    }

    pub fn about() -> Self {
        Self { about: true, ..Self::startup() }
    }

    fn texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if self.texture.is_none() {
            let img = ::image::load_from_memory(PHOTO).ok()?.to_rgba8();
            let (w, h) = (img.width() as usize, img.height() as usize);
            let mut px = img.into_raw();
            // Darken the lower part so that the wordmark stays legible on the bright river.
            for y in 0..h {
                let t = ((y as f32 / h as f32 - 0.45) / 0.55).clamp(0.0, 1.0);
                let k = 1.0 - 0.72 * t * t * (3.0 - 2.0 * t);
                // Every fourth byte is alpha and stays.
                for (i, c) in px[y * w * 4..(y + 1) * w * 4].iter_mut().enumerate() {
                    if i % 4 != 3 {
                        *c = (*c as f32 * k) as u8;
                    }
                }
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &px);
            self.texture = Some(ctx.load_texture("splash", image, TextureOptions::LINEAR));
        }
        self.texture.as_ref()
    }
}

fn spaced(text: &str, size: f32, spacing: f32, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: FontId::proportional(size), extra_letter_spacing: spacing, color, ..Default::default() });
    job
}

impl App {
    /// What the app is doing, for the start screen.
    fn startup_status(&self) -> String {
        if let Some((done, total)) = self.index_progress {
            format!("Indexiere Bilder … {done} / {total}")
        } else if self.loading.is_some() {
            "Öffne Bild …".into()
        } else if let Some(s) = &self.session {
            format!("Session „{}“", s.name)
        } else {
            "Bereit".into()
        }
    }

    pub(crate) fn splash_ui(&mut self, ctx: &egui::Context) {
        let ready = self.texture.is_some() || self.grid_view || self.load_error.is_some() || (self.loading.is_none() && self.current.is_none());
        let status = self.startup_status();
        let Some(splash) = &mut self.splash else { return };
        let now = ctx.input(|i| i.time);
        let shown_at = *splash.shown_at.get_or_insert(now);
        let age = now - shown_at;
        if !splash.about && splash.closing_at.is_none() && ((age > MIN_SECS && ready) || age > MAX_SECS) {
            splash.closing_at = Some(now);
        }
        let alpha = splash.closing_at.map_or(1.0, |t| 1.0 - ((now - t) / FADE_SECS) as f32);
        if alpha <= 0.0 {
            self.splash = None;
            return;
        }
        ctx.request_repaint();

        let about = splash.about;
        let Some(texture) = splash.texture(ctx).cloned() else {
            self.splash = None;
            return;
        };
        let screen = ctx.content_rect();
        let mut dismiss = ctx.input(|i| i.key_pressed(egui::Key::Escape) || (about && i.key_pressed(egui::Key::Enter)));
        egui::Area::new(Id::new("splash")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            ui.multiply_opacity(alpha);
            // The backdrop takes all clicks; any click closes the start screen early.
            let (_, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            dismiss |= backdrop.clicked();
            let painter = ui.painter();
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(140));

            let w = (screen.width() - 48.0).min(780.0);
            let card = Rect::from_center_size(screen.center(), vec2(w, w / CARD_ASPECT));
            let radius = 8;
            painter.add(egui::Shadow { offset: [0, 14], blur: 40, spread: 0, color: Color32::from_black_alpha(160) }.as_shape(card, radius));
            egui::Image::new(egui::load::SizedTexture::new(texture.id(), card.size())).corner_radius(radius).paint_at(ui, card);
            painter.rect_stroke(card, radius, egui::Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);

            let pad = (w * 0.045).clamp(18.0, 34.0);
            let scale = w / 780.0;
            painter.text(card.min + vec2(pad, pad), Align2::LEFT_TOP, icon::APERTURE, FontId::proportional(26.0 * scale.max(0.8)), Color32::from_white_alpha(230));

            // Wordmark and tagline, bottom left.
            let base = card.left_bottom() + vec2(pad, -pad);
            let small = FontId::proportional(11.5);
            let line = painter.text(base, Align2::LEFT_BOTTOM, if about { "Klicken zum Schließen".into() } else { status }, small.clone(), Color32::from_gray(165));
            let tagline = painter.layout_job(spaced("RAW-ENTWICKLUNG UND BILDVERWALTUNG", 12.5 * scale.max(0.85), 2.2, Color32::from_gray(205)));
            let tag_pos = pos2(base.x, line.top() - 18.0 - tagline.size().y);
            let word = painter.layout_job(spaced("Silberkorn", 58.0 * scale.max(0.7), 1.5, Color32::WHITE));
            let word_pos = pos2(base.x - 3.0, tag_pos.y - 4.0 - word.size().y);
            theme::paint_glow(painter, word_pos, &word, 7.0 * scale.max(0.7), 1.0);
            painter.galley(word_pos, word, Color32::WHITE);
            painter.galley(tag_pos, tagline, Color32::WHITE);
            let rule = Rect::from_min_size(pos2(base.x, line.top() - 10.0), vec2(36.0, 2.0));
            painter.rect_filled(rule, 1.0, theme::ACCENT);

            // Version and photo credit, bottom right.
            let right = card.right_bottom() + vec2(-pad, -pad);
            painter.text(right, Align2::RIGHT_BOTTOM, CREDIT, FontId::proportional(10.5), Color32::from_gray(140));
            painter.text(
                right - vec2(0.0, 16.0),
                Align2::RIGHT_BOTTOM,
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                small,
                Color32::from_gray(175),
            );
            // Attribution required by the lens database's license.
            if about {
                painter.text(right - vec2(0.0, 32.0), Align2::RIGHT_BOTTOM, "Objektivprofile: lensfun, CC BY-SA 3.0", FontId::proportional(10.5), Color32::from_gray(140));
            }

            // Indeterminate progress sweep along the bottom edge while starting up.
            if !about {
                let track = Rect::from_min_max(pos2(card.left() + radius as f32, card.bottom() - 2.0), pos2(card.right() - radius as f32, card.bottom()));
                let t = ((now * 0.6) % 1.0) as f32;
                let seg = track.width() * 0.25;
                let x0 = track.left() + (track.width() + seg) * t - seg;
                let bar = Rect::from_min_max(Pos2::new(x0.max(track.left()), track.top()), Pos2::new((x0 + seg).min(track.right()), track.bottom()));
                if bar.width() > 0.0 {
                    painter.rect_filled(bar, 1.0, theme::ACCENT.gamma_multiply(0.9));
                }
            }
        });
        if dismiss
            && let Some(s) = &mut self.splash
            && s.closing_at.is_none()
        {
            s.closing_at = Some(now);
        }
    }
}
