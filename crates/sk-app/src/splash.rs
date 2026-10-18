//! Start screen: a black-and-white photograph under fine film grain with the wordmark in a silver
//! gradient, shown while the app starts up and again as the "about" dialog.

use egui::text::{LayoutJob, TextFormat};
use std::sync::Arc;

use egui::{Align2, Color32, FontId, Galley, Id, Order, Pos2, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2};

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
/// Widest card; everything on it scales with its width.
const CARD_WIDTH: f32 = 620.0;
/// Side of the tiled grain texture, in pixels.
const GRAIN_SIZE: usize = 192;

pub struct Splash {
    /// Opened from "Über Silberkorn": stays until clicked.
    about: bool,
    texture: Option<TextureHandle>,
    grain: Option<TextureHandle>,
    shown_at: Option<f64>,
    closing_at: Option<f64>,
}

impl Splash {
    pub fn startup() -> Self {
        Self { about: false, texture: None, grain: None, shown_at: None, closing_at: None }
    }

    pub fn about() -> Self {
        Self { about: true, ..Self::startup() }
    }

    /// The "Über Silberkorn" card (not the start screen).
    pub fn is_about(&self) -> bool {
        self.about
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

    /// Film grain: a tile of noise that repeats over the card.
    fn grain(&mut self, ctx: &egui::Context) -> TextureHandle {
        self.grain
            .get_or_insert_with(|| {
                // A small xorshift generator: the same grain every time, no dependency.
                let mut x: u32 = 0x9E37_79B9;
                let px: Vec<Color32> = (0..GRAIN_SIZE * GRAIN_SIZE)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        let v = (x >> 24) as u8;
                        Color32::from_gray(v)
                    })
                    .collect();
                let image = egui::ColorImage { size: [GRAIN_SIZE, GRAIN_SIZE], source_size: vec2(GRAIN_SIZE as f32, GRAIN_SIZE as f32), pixels: px };
                let options = TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..TextureOptions::NEAREST };
                ctx.load_texture("splash-grain", image, options)
            })
            .clone()
    }
}

/// `galley` with its glyphs colored from `top` to `bottom`, like polished silver.
fn vertical_gradient(galley: &Arc<Galley>, top: Color32, bottom: Color32) -> Arc<Galley> {
    let mut g = (**galley).clone();
    let (y0, y1) = (g.mesh_bounds.top(), g.mesh_bounds.bottom().max(g.mesh_bounds.top() + 1.0));
    for placed in &mut g.rows {
        let dy = placed.pos.y;
        for v in &mut Arc::make_mut(&mut placed.row).visuals.mesh.vertices {
            let t = ((v.pos.y + dy - y0) / (y1 - y0)).clamp(0.0, 1.0);
            v.color = top.lerp_to_gamma(bottom, t);
        }
    }
    Arc::new(g)
}

fn spaced(text: &str, size: f32, spacing: f32, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: FontId::proportional(size), extra_letter_spacing: spacing, color, ..Default::default() });
    job
}

impl App {
    /// What the app is doing, for the start screen.
    fn startup_status(&self) -> String {
        if let Some(p) = self.ai_preload_progress() {
            format!("KI-Modelle werden geladen … {:.0} %", p * 100.0)
        } else if let Some((done, total)) = self.index_progress {
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
        // Downloading the AI models: the start screen stays its longest time to show it, then
        // the status bar carries on.
        let ai = self.ai_preload_progress();
        let ready = ready && ai.is_none();
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
        let grain = splash.grain(ctx);
        let screen = ctx.content_rect();
        let mut dismiss = ctx.input(|i| i.key_pressed(egui::Key::Escape) || (about && i.key_pressed(egui::Key::Enter)));
        egui::Area::new(Id::new("splash")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            ui.multiply_opacity(alpha);
            // The backdrop takes all clicks; any click closes the start screen early.
            let (_, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            dismiss |= backdrop.clicked();
            let painter = ui.painter();
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(140));

            let w = (screen.width() - 48.0).min(CARD_WIDTH);
            let card = Rect::from_center_size(screen.center(), vec2(w, w / CARD_ASPECT));
            let radius = 10;
            painter.add(egui::Shadow { offset: [0, 18], blur: 48, spread: 0, color: Color32::from_black_alpha(170) }.as_shape(card, radius));
            egui::Image::new(egui::load::SizedTexture::new(texture.id(), card.size())).corner_radius(radius).paint_at(ui, card);
            // Grain over the photograph, as on silver-halide film.
            let tiles = card.size() / GRAIN_SIZE as f32;
            egui::Image::new(egui::load::SizedTexture::new(grain.id(), card.size()))
                .uv(Rect::from_min_size(Pos2::ZERO, tiles))
                .corner_radius(radius)
                .tint(Color32::from_white_alpha(16))
                .paint_at(ui, card);
            painter.rect_stroke(card, radius, egui::Stroke::new(1.0, Color32::from_white_alpha(22)), egui::StrokeKind::Inside);

            let scale = w / CARD_WIDTH;
            let pad = (30.0 * scale).max(18.0);
            painter.text(card.min + vec2(pad, pad), Align2::LEFT_TOP, icon::APERTURE, FontId::proportional(22.0 * scale.max(0.8)), Color32::from_white_alpha(220));

            // Version as a small pill, top right.
            let version = painter.layout_no_wrap(format!("Version {}", env!("CARGO_PKG_VERSION")), FontId::proportional(11.0), Color32::from_gray(215));
            let pill = Rect::from_min_size(pos2(card.right() - pad - version.size().x - 18.0, card.top() + pad), version.size() + vec2(18.0, 8.0));
            painter.rect_filled(pill, 20, Color32::from_black_alpha(90));
            painter.rect_stroke(pill, 20, egui::Stroke::new(1.0, Color32::from_white_alpha(40)), egui::StrokeKind::Inside);
            painter.galley(pill.min + vec2(9.0, 4.0), version, Color32::WHITE);

            // Wordmark, tagline and status, bottom left.
            let base = card.left_bottom() + vec2(pad, -pad);
            let small = FontId::proportional(11.5);
            let line = painter.text(base, Align2::LEFT_BOTTOM, if about { "Klicken zum Schließen".into() } else { status }, small, Color32::from_gray(150));
            let tagline = painter.layout_job(spaced("RAW-Entwicklung und Bildverwaltung", 13.0 * scale.max(0.85), 0.3, Color32::from_gray(200)));
            let tag_pos = pos2(base.x, line.top() - 14.0 - tagline.size().y);
            let size = 64.0 * scale.max(0.65);
            let word = painter.layout_job(spaced("silberkorn", size, -0.035 * size, Color32::WHITE));
            let word = vertical_gradient(&word, Color32::from_rgb(250, 251, 255), Color32::from_rgb(150, 156, 168));
            let word_pos = pos2(base.x - 0.06 * size, tag_pos.y + 0.06 * size - word.size().y);
            painter.add(egui::Shadow { offset: [0, 4], blur: 30, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(Rect::from_min_size(word_pos, word.size()), 8));
            // Drawn twice, a fraction of a pixel apart: a little more weight than the light face.
            painter.galley(word_pos, word.clone(), Color32::WHITE);
            painter.galley(word_pos + vec2(0.6 * scale.max(0.7), 0.0), word.clone(), Color32::WHITE);
            // The grain: an accent dot after the name.
            let dot = Rect::from_min_size(word_pos, word.size()).right_bottom() + vec2(0.075 * size, -0.28 * size);
            painter.circle_filled(dot, 0.075 * size, theme::ACCENT);
            painter.galley(tag_pos, tagline, Color32::WHITE);

            // Credits, bottom right; the lens database's license asks for its attribution.
            let right = card.right_bottom() + vec2(-pad, -pad);
            let credit = FontId::proportional(10.0);
            painter.text(right, Align2::RIGHT_BOTTOM, CREDIT, credit.clone(), Color32::from_gray(125));
            if about {
                painter.text(right - vec2(0.0, 15.0), Align2::RIGHT_BOTTOM, "Objektivprofile: lensfun, CC BY-SA 3.0", credit, Color32::from_gray(125));
            }

            // Progress along the bottom edge while starting up: of the AI download, else a sweep.
            if !about {
                let track = Rect::from_min_max(pos2(card.left() + radius as f32, card.bottom() - 2.0), pos2(card.right() - radius as f32, card.bottom()));
                if let Some(p) = ai {
                    painter.rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * p, track.bottom())), 1.0, theme::ACCENT);
                    return;
                }
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
