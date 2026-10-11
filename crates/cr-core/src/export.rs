//! Export ("process recipes"): full-resolution development to JPEG / TIFF / PNG.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{ImageBuffer, ImageEncoder, Rgb, imageops::FilterType};
use serde::{Deserialize, Serialize};

use crate::color::OutputSpace;
use crate::icc;
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
    pub name: String,
    /// Included when processing.
    pub enabled: bool,
    pub format: ExportFormat,
    /// JPEG quality 1..100
    pub quality: u8,
    /// Resize so that the long edge is at most this many pixels (None = original size).
    pub long_edge: Option<u32>,
    /// Output folder; relative paths are resolved against the image's folder.
    pub output_dir: PathBuf,
    /// Suffix appended to the file stem.
    pub suffix: String,
    pub output_space: OutputSpace,
}

impl Default for ExportRecipe {
    fn default() -> Self {
        Self {
            name: "JPEG".into(),
            enabled: true,
            format: ExportFormat::Jpeg,
            quality: 90,
            long_edge: None,
            output_dir: PathBuf::from("Output"),
            suffix: String::new(),
            output_space: OutputSpace::Srgb,
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
    export_many(image, adj, std::slice::from_ref(recipe)).remove(0)
}

/// Develops `image` once and writes one file per recipe.
pub fn export_many(image: &Path, adj: &Adjustments, recipes: &[ExportRecipe]) -> Vec<Result<PathBuf>> {
    let (meta, linear) = match raw::load_full(image) {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("{e:#}");
            return recipes.iter().map(|_| Err(anyhow::anyhow!(msg.clone()))).collect();
        }
    };
    let processor = Processor::new(&meta, adj);
    // Recipes sharing a color space share the processing.
    let mut cache: Vec<(OutputSpace, crate::image::LinearImage)> = Vec::new();
    recipes
        .iter()
        .map(|recipe| {
            if !cache.iter().any(|(s, _)| *s == recipe.output_space) {
                cache.push((recipe.output_space, processor.process(&linear, None, recipe.output_space)));
            }
            let out = &cache.iter().find(|(s, _)| *s == recipe.output_space).unwrap().1;
            write_output(image, out, recipe)
        })
        .collect()
}

fn write_output(image: &Path, out: &crate::image::LinearImage, recipe: &ExportRecipe) -> Result<PathBuf> {

    let mut buf: ImageBuffer<Rgb<f32>, Vec<f32>> =
        ImageBuffer::from_raw(out.width as u32, out.height as u32, out.data.iter().flatten().copied().collect())
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
    let profile = icc::profile_for(recipe.output_space);
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    match recipe.format {
        ExportFormat::Jpeg => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, recipe.quality.clamp(1, 100));
            enc.set_icc_profile(profile).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let mut enc = image::codecs::png::PngEncoder::new(file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff8 => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let mut enc = image::codecs::tiff::TiffEncoder::new(file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff16 => {
            let data: Vec<u16> = buf.as_raw().iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect();
            let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_ne_bytes()).collect();
            let mut enc = image::codecs::tiff::TiffEncoder::new(file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(&bytes, w, h, image::ExtendedColorType::Rgb16)?;
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_many_writes_each_recipe() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0010.ARW");
        if !sample.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cr-export-{}", std::process::id()));
        let recipes = vec![
            ExportRecipe { output_dir: dir.clone(), long_edge: Some(400), ..Default::default() },
            ExportRecipe {
                name: "TIFF".into(),
                format: ExportFormat::Tiff16,
                output_space: OutputSpace::AdobeRgb,
                long_edge: Some(300),
                output_dir: dir.clone(),
                suffix: "_ar".into(),
                ..Default::default()
            },
        ];
        let res = export_many(&sample, &Adjustments::default(), &recipes);
        let paths: Vec<PathBuf> = res.into_iter().map(|r| r.unwrap()).collect();
        let jpg = image::open(&paths[0]).unwrap();
        assert_eq!(jpg.width(), 400);
        let tif = image::open(&paths[1]).unwrap();
        assert_eq!(tif.width(), 300);
        assert!(matches!(tif, image::DynamicImage::ImageRgb16(_)));
        std::fs::remove_dir_all(dir).ok();
    }
}
