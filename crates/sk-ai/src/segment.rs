//! Object selection by clicks: MobileSAM, a small Segment
//! Anything model. The image encoder runs once per image (about a second); each click then only
//! runs the small mask decoder with all clicks so far (included and excluded points).

use std::path::Path;

use anyhow::{Context, Result};
use sk_core::detail::Plane;

use crate::models::{self, OBJECT_DECODER, OBJECT_ENCODER};
use crate::runtime::{Data, Input, Net};
use crate::{Guide, MaskImage, refine_edges, to_mask};

/// The encoder's input edge: the image is scaled to fit and padded.
const SIZE: usize = 1024;
/// The decoder always gets this many points (unused ones are marked as padding).
pub const MAX_POINTS: usize = 8;
/// The color the encoder pads with (its normalization mean, so padding is zero inside).
const PAD: [f32; 3] = [123.675, 116.28, 103.53];

/// Both models are downloaded.
pub fn available() -> bool {
    models::available(&OBJECT_ENCODER) && models::available(&OBJECT_DECODER)
}

/// Downloads both models.
pub fn download(progress: &dyn Fn(f32)) -> Result<()> {
    let total = (OBJECT_ENCODER.bytes + OBJECT_DECODER.bytes) as f32;
    let mut done = 0.0;
    for m in [OBJECT_ENCODER, OBJECT_DECODER] {
        if !models::available(&m) {
            models::download(&m, &|p| progress((done + p * m.bytes as f32) / total))?;
        }
        done += m.bytes as f32;
    }
    Ok(())
}

/// Download size of both models in MB.
pub fn download_mb() -> u64 {
    (OBJECT_ENCODER.bytes + OBJECT_DECODER.bytes).div_ceil(1_000_000)
}

/// A click: position in normalized source coordinates, and whether it adds or takes away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub uv: [f32; 2],
    pub include: bool,
}

/// What the encoder found in one image (reused for every click on it).
pub struct Embedding {
    data: Vec<f32>,
    /// The guide at mask resolution (masks are made at its size and refined against it).
    guide: Guide,
}

pub struct ObjectModel {
    encoder: Net,
    decoder: Net,
}

impl ObjectModel {
    pub fn load() -> Result<Self> {
        anyhow::ensure!(available(), "Das Modell für „Objekt auswählen“ ist nicht geladen");
        Self::load_from(&models::path(&OBJECT_ENCODER), &models::path(&OBJECT_DECODER))
    }

    pub fn load_from(encoder: &Path, decoder: &Path) -> Result<Self> {
        Ok(Self {
            encoder: Net::load(encoder, &[Input::F32Shape(&[SIZE, SIZE, 3])]).context("Bild-Modell")?,
            // The low-resolution mask only (output 2): the full one is scaled to a size tract can't
            // know in advance.
            decoder: Net::load_outputs(
                decoder,
                &[
                    Input::F32([1, 256, 64, 64]),
                    Input::F32Shape(&[1, MAX_POINTS, 2]),
                    Input::F32Shape(&[1, MAX_POINTS]),
                    Input::F32([1, 1, 256, 256]),
                    Input::F32Shape(&[1]),
                    Input::F32Shape(&[2]),
                ],
                Some(&[2]),
            )
            .context("Masken-Modell")?,
        })
    }

    /// Runs the encoder on the image (display-referred guide, any resolution).
    pub fn embed(&self, guide: &Guide) -> Result<Embedding> {
        let small = guide.at_mask_resolution();
        let k = SIZE as f32 / guide.width.max(guide.height) as f32;
        let (w, h) = (((guide.width as f32 * k).round() as usize).clamp(1, SIZE), ((guide.height as f32 * k).round() as usize).clamp(1, SIZE));
        let img = sk_core::image::LinearImage::new(guide.width, guide.height, guide.rgb.clone()).resize_area(w, h);
        let mut input = vec![0.0f32; SIZE * SIZE * 3];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = if x < w && y < h { img.data[y * w + x].map(|v| v.clamp(0.0, 1.0) * 255.0) } else { PAD };
                input[(y * SIZE + x) * 3..(y * SIZE + x) * 3 + 3].copy_from_slice(&p);
            }
        }
        let out = self.encoder.run(&[Data::F32(&input)])?;
        let o = out.into_iter().next().context("Keine Modellausgabe")?;
        anyhow::ensure!(o.data.len() == 256 * 64 * 64, "Unerwartete Modellausgabe {:?}", o.shape);
        Ok(Embedding { data: o.data, guide: small })
    }

    /// The object at the clicks (at most the last [`MAX_POINTS`]), at mask resolution.
    pub fn mask(&self, emb: &Embedding, clicks: &[Click]) -> Result<MaskImage> {
        anyhow::ensure!(clicks.iter().any(|c| c.include), "Mindestens ein Klick auf das Objekt");
        let (w, h) = (emb.guide.width, emb.guide.height);
        // Point coordinates in the encoder's 1024 frame.
        let k = SIZE as f32 / w.max(h) as f32;
        let mut coords = vec![0.0f32; MAX_POINTS * 2];
        let mut labels = vec![-1.0f32; MAX_POINTS];
        for (i, c) in clicks.iter().rev().take(MAX_POINTS).rev().enumerate() {
            coords[2 * i] = c.uv[0] * w as f32 * k;
            coords[2 * i + 1] = c.uv[1] * h as f32 * k;
            labels[i] = if c.include { 1.0 } else { 0.0 };
        }
        let no_mask = vec![0.0f32; 256 * 256];
        let out = self.decoder.run(&[
            Data::F32(&emb.data),
            Data::F32(&coords),
            Data::F32(&labels),
            Data::F32(&no_mask),
            Data::F32(&[0.0]),
            Data::F32(&[h as f32, w as f32]),
        ])?;
        // The 256 x 256 logits cover the encoder's padded square: the image is its upper left part.
        let low = out.into_iter().find(|o| o.data.len() == 256 * 256).context("Keine Modellausgabe")?;
        let (lw, lh) = (w as f32 * k / 4.0, h as f32 * k / 4.0);
        let logits: Vec<f32> = (0..w * h)
            .map(|i| {
                let fx = (((i % w) as f32 + 0.5) / w as f32 * lw - 0.5).clamp(0.0, 255.0);
                let fy = (((i / w) as f32 + 0.5) / h as f32 * lh - 0.5).clamp(0.0, 255.0);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(255), (y0 + 1).min(255));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let at = |x: usize, y: usize| low.data[y * 256 + x];
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                top + (bot - top) * ty
            })
            .collect();
        // Logits to a soft mask, then edges along the image's own edges.
        let soft = Plane::new(w, h, logits.iter().map(|v| 1.0 / (1.0 + (-v * 2.0).exp())).collect());
        Ok(to_mask(&refine_edges(&soft, &emb.guide.luma(), 2, 1e-3)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the downloaded models: a click on a dark disc on a light ground selects the disc,
    /// not the ground.
    #[test]
    fn selects_the_clicked_object_if_the_model_is_available() {
        if !available() {
            return;
        }
        let m = ObjectModel::load().unwrap();
        let (w, h) = (640, 480);
        let rgb = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 400.0, (i / w) as f32 - 240.0);
                if x * x + y * y < 90.0 * 90.0 { [0.15, 0.2, 0.6] } else { [0.85, 0.8, 0.7] }
            })
            .collect();
        let emb = m.embed(&Guide { width: w, height: h, rgb }).unwrap();
        let mask = m.mask(&emb, &[Click { uv: [400.0 / 640.0, 0.5], include: true }]).unwrap();
        let at = |u: f32, v: f32| mask.data[((v * mask.height as f32) as usize) * mask.width + (u * mask.width as f32) as usize];
        assert!(at(400.0 / 640.0, 0.5) > 200, "Objekt");
        assert!(at(0.1, 0.1) < 50 && at(0.2, 0.8) < 50, "Hintergrund");
    }
}
