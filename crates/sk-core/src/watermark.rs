//! Watermark for exports: a text or a logo (PNG with transparency), blended into the
//! processed image.

use std::path::PathBuf;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb, imageops::FilterType};
use serde::{Deserialize, Serialize};

use crate::color::{OutputSpace, Trc, mat_inv, mat_mul, mat_vec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WatermarkKind {
    #[default]
    Text,
    Image,
}

/// Where the watermark sits (rows top to bottom, columns left to right).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Anchor::TopLeft => "Oben links",
            Anchor::Top => "Oben Mitte",
            Anchor::TopRight => "Oben rechts",
            Anchor::Left => "Mitte links",
            Anchor::Center => "Mitte",
            Anchor::Right => "Mitte rechts",
            Anchor::BottomLeft => "Unten links",
            Anchor::Bottom => "Unten Mitte",
            Anchor::BottomRight => "Unten rechts",
        }
    }

    /// (column, row), each 0, 1 or 2.
    fn cell(self) -> (usize, usize) {
        let i = Anchor::ALL.iter().position(|a| *a == self).unwrap_or(8);
        (i % 3, i / 3)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub enabled: bool,
    pub kind: WatermarkKind,
    pub text: String,
    /// Logo file (PNG with transparency, or any readable image).
    pub image: PathBuf,
    /// Black text instead of white.
    pub dark: bool,
    /// Height in percent of the image's short edge.
    pub size: f32,
    /// 0..1
    pub opacity: f32,
    pub anchor: Anchor,
    /// Distance from the edges in percent of the short edge.
    pub margin: f32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: WatermarkKind::Text,
            text: "© ".into(),
            image: PathBuf::new(),
            dark: false,
            size: 4.0,
            opacity: 0.6,
            anchor: Anchor::BottomRight,
            margin: 3.0,
        }
    }
}

/// Premultiplied-free RGBA overlay: color (output encoding) and coverage 0..1.
struct Overlay {
    width: usize,
    height: usize,
    pixels: Vec<([f32; 3], f32)>,
}

impl Watermark {
    /// Nothing to draw: off, no text or no logo chosen.
    pub fn is_empty(&self) -> bool {
        !self.enabled
            || match self.kind {
                WatermarkKind::Text => self.text.trim().is_empty(),
                WatermarkKind::Image => self.image.as_os_str().is_empty(),
            }
    }

    /// Blends the watermark into `buf` (processed, encoded 0..1 in `space`).
    pub fn apply(&self, buf: &mut ImageBuffer<Rgb<f32>, Vec<f32>>, space: OutputSpace) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let (w, h) = (buf.width() as usize, buf.height() as usize);
        let short = w.min(h) as f32;
        // The settings file may hold anything.
        let finite = |v: f32, default: f32| if v.is_finite() { v } else { default };
        let height = (short * finite(self.size, 4.0).clamp(0.1, 100.0) / 100.0).max(4.0);
        let margin = short * finite(self.margin, 3.0).clamp(0.0, 50.0) / 100.0;
        let max_width = (w as f32 - 2.0 * margin).max(1.0);
        let Some(overlay) = (match self.kind {
            WatermarkKind::Text => text_overlay(&self.text, height, max_width, if self.dark { 0.0 } else { 1.0 }),
            WatermarkKind::Image => Some(self.logo_overlay(height, max_width, space)?),
        }) else {
            return Ok(());
        };
        let (col, row) = self.anchor.cell();
        let place = |free: f32, k: usize| match k {
            0 => margin,
            1 => (free / 2.0).max(0.0),
            _ => free - margin,
        };
        let x0 = place(w as f32 - overlay.width as f32, col).round() as i64;
        let y0 = place(h as f32 - overlay.height as f32, row).round() as i64;
        let opacity = finite(self.opacity, 0.6).clamp(0.0, 1.0);
        for oy in 0..overlay.height {
            let y = y0 + oy as i64;
            if y < 0 || y >= h as i64 {
                continue;
            }
            for ox in 0..overlay.width {
                let x = x0 + ox as i64;
                if x < 0 || x >= w as i64 {
                    continue;
                }
                let (color, cover) = overlay.pixels[oy * overlay.width + ox];
                let a = cover * opacity;
                if a <= 0.0 {
                    continue;
                }
                let p = buf.get_pixel_mut(x as u32, y as u32);
                for (v, c) in p.0.iter_mut().zip(color) {
                    *v = *v * (1.0 - a) + c * a;
                }
            }
        }
        Ok(())
    }

    /// The logo (taken as sRGB) at `height`, at most `max_width` wide, encoded in `space`.
    fn logo_overlay(&self, height: f32, max_width: f32, space: OutputSpace) -> Result<Overlay> {
        let mut logo = image::open(&self.image).with_context(|| format!("Wasserzeichen-Logo {} nicht lesbar", self.image.display()))?.to_rgba32f();
        // Scaled premultiplied, so that the color of transparent pixels doesn't bleed into the edges.
        for p in logo.pixels_mut() {
            let a = p.0[3];
            p.0[..3].iter_mut().for_each(|v| *v *= a);
        }
        let (lw, lh) = (logo.width() as f32, logo.height() as f32);
        let scale = (height / lh).min(max_width / lw);
        let (nw, nh) = (((lw * scale).round() as u32).max(1), ((lh * scale).round() as u32).max(1));
        let logo = image::imageops::resize(&logo, nw, nh, FilterType::Lanczos3);
        let xyz_from_out = space.to_xyz_d50();
        let out_from_srgb = mat_mul(&mat_inv(&xyz_from_out).context("Farbraum-Matrix")?, &OutputSpace::Srgb.to_xyz_d50());
        let trc = space.trc();
        let pixels = logo
            .pixels()
            .map(|p| {
                let a = p.0[3].clamp(0.0, 1.0);
                if a <= 0.0 {
                    return ([0.0; 3], 0.0);
                }
                let srgb = [p.0[0], p.0[1], p.0[2]].map(|v| Trc::Srgb.decode((v / a).clamp(0.0, 1.0)));
                (mat_vec(&out_from_srgb, srgb).map(|v| trc.encode(v.clamp(0.0, 1.0))), a)
            })
            .collect();
        Ok(Overlay { width: nw as usize, height: nh as usize, pixels })
    }

    /// Changes whenever the drawn watermark may change, including a new logo file under the same name.
    pub fn file_stamp(&self) -> Option<(u64, std::time::SystemTime)> {
        if self.is_empty() || self.kind != WatermarkKind::Image {
            return None;
        }
        let m = std::fs::metadata(&self.image).ok()?;
        Some((m.len(), m.modified().ok()?))
    }
}

/// The text in one line, `height` pixels from ascender to descender (smaller if it would be
/// wider than `max_width`). `None` for empty text.
fn text_overlay(text: &str, height: f32, max_width: f32, gray: f32) -> Option<Overlay> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let font = FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok()?;
    let unit = font.as_scaled(PxScale::from(1.0));
    let mut px = height / (unit.ascent() - unit.descent());
    let width_at_1: f32 = text.chars().map(|c| unit.h_advance(unit.glyph_id(c))).sum();
    if width_at_1 * px > max_width - 2.0 {
        px = ((max_width - 2.0) / width_at_1).max(1.0);
    }
    let sf = font.as_scaled(PxScale::from(px));
    // Room above the ascender for accents on capitals (Å, É).
    let top = (px * 0.15).ceil();
    let mut x = 0.0;
    let mut prev = None;
    let mut glyphs = Vec::new();
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        glyphs.push(id.with_scale_and_position(px, point(x, top + sf.ascent())));
        x += sf.h_advance(id);
        prev = Some(id);
    }
    let (width, height) = (x.ceil() as usize + 2, (top + sf.ascent() - sf.descent()).ceil() as usize + 2);
    let mut cover = vec![0.0f32; width * height];
    for g in glyphs {
        let Some(outline) = font.outline_glyph(g) else { continue };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, c| {
            let (x, y) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
            if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
                let v = &mut cover[y as usize * width + x as usize];
                *v = v.max(c);
            }
        });
    }
    Some(Overlay { width, height, pixels: cover.into_iter().map(|c| ([gray; 3], c)).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(w: u32, h: u32) -> ImageBuffer<Rgb<f32>, Vec<f32>> {
        ImageBuffer::from_pixel(w, h, Rgb([0.5, 0.5, 0.5]))
    }

    #[test]
    fn text_lands_in_the_chosen_corner() {
        let mut buf = gray(400, 300);
        let wm = Watermark { enabled: true, text: "© Silberkorn".into(), opacity: 1.0, size: 10.0, ..Default::default() };
        wm.apply(&mut buf, OutputSpace::Srgb).unwrap();
        let bright = |x0: u32, x1: u32, y0: u32, y1: u32| (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| buf.get_pixel(x, y).0[0] > 0.9).count();
        assert!(bright(200, 400, 150, 300) > 50, "unten rechts");
        assert_eq!(bright(0, 200, 0, 150), 0, "oben links bleibt frei");
    }

    #[test]
    fn disabled_or_empty_changes_nothing() {
        let mut buf = gray(50, 50);
        Watermark { enabled: false, text: "x".into(), ..Default::default() }.apply(&mut buf, OutputSpace::Srgb).unwrap();
        Watermark { enabled: true, text: "  ".into(), ..Default::default() }.apply(&mut buf, OutputSpace::Srgb).unwrap();
        assert!(buf.pixels().all(|p| p.0 == [0.5; 3]));
    }

    #[test]
    fn logo_is_blended_with_its_alpha() {
        let dir = std::env::temp_dir().join(format!("sk-wm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let logo = dir.join("logo.png");
        // Left half opaque red, right half transparent.
        image::RgbaImage::from_fn(20, 10, |x, _| if x < 10 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) }).save(&logo).unwrap();
        let mut buf = gray(200, 100);
        let wm = Watermark { enabled: true, kind: WatermarkKind::Image, image: logo, opacity: 1.0, size: 20.0, margin: 0.0, anchor: Anchor::TopLeft, ..Default::default() };
        wm.apply(&mut buf, OutputSpace::Srgb).unwrap();
        assert!(buf.get_pixel(5, 5).0[0] > 0.95 && buf.get_pixel(5, 5).0[1] < 0.05);
        assert_eq!(buf.get_pixel(35, 5).0, [0.5; 3]);
        // A missing logo is an error, not a silent export without it; no logo chosen draws nothing.
        let missing = Watermark { image: dir.join("fehlt.png"), ..wm.clone() };
        assert!(missing.apply(&mut buf, OutputSpace::Srgb).is_err());
        let none = Watermark { image: PathBuf::new(), ..wm.clone() };
        let before = buf.clone();
        none.apply(&mut buf, OutputSpace::Srgb).unwrap();
        assert_eq!(buf, before);

        // In Adobe RGB the same red is stored with other values.
        let mut adobe = gray(200, 100);
        wm.apply(&mut adobe, OutputSpace::AdobeRgb).unwrap();
        let p = adobe.get_pixel(5, 5).0;
        assert!(p[0] < 0.95 && p[0] > 0.7, "{p:?}");

        // White logo on transparent black, scaled down: no dark fringe.
        let white = dir.join("white.png");
        image::RgbaImage::from_fn(64, 64, |x, y| {
            let inside = (16..48).contains(&x) && (16..48).contains(&y);
            if inside { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([0, 0, 0, 0]) }
        })
        .save(&white)
        .unwrap();
        let mut bright = ImageBuffer::from_pixel(200, 100, Rgb([1.0f32, 1.0, 1.0]));
        let wm = Watermark { image: white, size: 7.0, ..wm };
        wm.apply(&mut bright, OutputSpace::Srgb).unwrap();
        let darkest = bright.pixels().map(|p| p.0[0]).fold(1.0f32, f32::min);
        assert!(darkest > 0.97, "Saum: {darkest}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_text_is_shrunk_to_fit() {
        let mut buf = gray(300, 600);
        let wm = Watermark { enabled: true, text: "© 2026 Ein sehr langer Fotografenname".into(), opacity: 1.0, size: 30.0, margin: 0.0, anchor: Anchor::Center, ..Default::default() };
        wm.apply(&mut buf, OutputSpace::Srgb).unwrap();
        let columns = |x: u32| (0..600).any(|y| buf.get_pixel(x, y).0[0] > 0.9);
        assert!(!columns(0) && !columns(299), "Text bleibt im Bild");
        assert!((100..200).any(columns));
    }
}
