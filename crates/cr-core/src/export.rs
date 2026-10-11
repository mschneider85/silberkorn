//! Export ("process recipes"): full-resolution development to JPEG / TIFF / PNG.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb, imageops::FilterType};
use serde::{Deserialize, Serialize};

use crate::params::Adjustments;
use crate::pipeline::Processor;
use crate::raw;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportFormat {
    #[default]
    Jpeg,
    Tiff16,
    Tiff8,
    Png,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 4] = [ExportFormat::Jpeg, ExportFormat::Tiff16, ExportFormat::Tiff8, ExportFormat::Png];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Tiff16 => "TIFF 16 Bit",
            ExportFormat::Tiff8 => "TIFF 8 Bit",
            ExportFormat::Png => "PNG",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Tiff16 | ExportFormat::Tiff8 => "tif",
            ExportFormat::Png => "png",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportRecipe {
    pub format: ExportFormat,
    /// JPEG quality 1..100
    pub quality: u8,
    /// Resize so that the long edge is at most this many pixels (None = original size).
    pub long_edge: Option<u32>,
    /// Output folder; relative paths are resolved against the image's folder.
    pub output_dir: PathBuf,
    /// Suffix appended to the file stem.
    pub suffix: String,
}

impl Default for ExportRecipe {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            quality: 90,
            long_edge: None,
            output_dir: PathBuf::from("Output"),
            suffix: String::new(),
        }
    }
}

impl ExportRecipe {
    pub fn output_path(&self, image: &Path) -> PathBuf {
        let dir = if self.output_dir.is_absolute() {
            self.output_dir.clone()
        } else {
            image.parent().unwrap_or(Path::new(".")).join(&self.output_dir)
        };
        let stem = image.file_stem().unwrap_or_default().to_string_lossy();
        dir.join(format!("{stem}{}.{}", self.suffix, self.format.extension()))
    }
}

/// Develops `image` at full resolution and writes it according to `recipe`.
pub fn export(image: &Path, adj: &Adjustments, recipe: &ExportRecipe) -> Result<PathBuf> {
    let (meta, linear) = raw::load_full(image)?;
    let out = Processor::new(&meta, adj).process_f32(&linear);
    drop(linear);

    let mut buf: ImageBuffer<Rgb<f32>, Vec<f32>> =
        ImageBuffer::from_raw(out.width as u32, out.height as u32, out.data.into_iter().flatten().collect())
            .context("Puffergröße ungültig")?;
    if let Some(edge) = recipe.long_edge {
        let (w, h) = buf.dimensions();
        let long = w.max(h);
        if edge > 0 && edge < long {
            let scale = edge as f64 / long as f64;
            let (nw, nh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
            buf = image::imageops::resize(&buf, nw, nh, FilterType::Lanczos3);
        }
    }

    let path = recipe.output_path(image);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Kann Ordner {} nicht anlegen", dir.display()))?;
    }
    let (w, h) = buf.dimensions();
    let to_u8 = |v: &f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    match recipe.format {
        ExportFormat::Jpeg => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, recipe.quality.clamp(1, 100));
            enc.encode(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png | ExportFormat::Tiff8 => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            image::RgbImage::from_raw(w, h, data).context("Puffergröße ungültig")?.save(&path)?;
        }
        ExportFormat::Tiff16 => {
            let data: Vec<u16> = buf.as_raw().iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect();
            ImageBuffer::<Rgb<u16>, _>::from_raw(w, h, data).context("Puffergröße ungültig")?.save(&path)?;
        }
    }
    Ok(path)
}
