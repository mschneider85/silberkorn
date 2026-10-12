//! Export ("process recipes"): full-resolution development to JPEG / TIFF / PNG.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{ImageBuffer, ImageEncoder, Rgb, imageops::FilterType};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::{OutputSpace, Trc, mat_inv, mat_mul, mat_vec};
use crate::icc;
use crate::params::Adjustments;
use crate::pipeline::Processor;
use crate::naming::NameFields;
use crate::raw::{self, CaptureInfo};
use crate::watermark::Watermark;

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
    /// File name with placeholders (see `naming`); empty: the image's name.
    pub name_template: String,
    /// Suffix appended to the file stem.
    pub suffix: String,
    pub output_space: OutputSpace,
    pub watermark: Watermark,
    /// Camera, lens, exposure and capture date in the file's EXIF (JPEG, PNG). GPS is never written.
    pub capture_metadata: bool,
    pub artist: String,
    pub copyright: String,
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
            name_template: String::new(),
            suffix: String::new(),
            output_space: OutputSpace::Srgb,
            watermark: Watermark::default(),
            capture_metadata: true,
            artist: String::new(),
            copyright: String::new(),
        }
    }
}

impl ExportRecipe {
    pub fn output_path(&self, f: &NameFields) -> PathBuf {
        let image = f.image;
        let dir = if self.output_dir.is_absolute() {
            self.output_dir.clone()
        } else {
            image.parent().unwrap_or(Path::new(".")).join(&self.output_dir)
        };
        let (file, n) = crate::variant::split(image);
        let stem = if self.name_template.trim().is_empty() {
            file.file_stem().unwrap_or_default().to_string_lossy().to_string()
        } else {
            crate::naming::expand(&self.name_template, f)
        };
        // Further variants get their number, so that they don't overwrite each other.
        let variant = if n > 1 && !self.name_template.to_lowercase().contains("{variante}") { format!("-v{n}") } else { String::new() };
        dir.join(format!("{stem}{variant}{}.{}", self.suffix, self.format.extension()))
    }
}

/// Develops `image` at full resolution and writes it according to `recipe`.
pub fn export(image: &Path, adj: &Adjustments, recipe: &ExportRecipe) -> Result<PathBuf> {
    export_many(image, adj, std::slice::from_ref(recipe), 1, "").remove(0)
}

/// Develops `image` once and writes one file per recipe. `counter` and `session` are for
/// recipes' name templates.
pub fn export_many(image: &Path, adj: &Adjustments, recipes: &[ExportRecipe], counter: u32, session: &str) -> Vec<Result<PathBuf>> {
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
            let fields = NameFields { image, capture: Some(&meta.info), counter, session };
            write_output(&fields, out, recipe)
        })
        .collect()
}

fn write_output(f: &NameFields, out: &crate::image::LinearImage, recipe: &ExportRecipe) -> Result<PathBuf> {
    let info = f.capture.cloned().unwrap_or_default();
    let bytes = encode(&resized(out, recipe)?, recipe, &info)?;
    let path = recipe.output_path(f);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Kann Ordner {} nicht anlegen", dir.display()))?;
    }
    crate::safe_file::write_nosync(&path, bytes)?;
    Ok(path)
}

/// The processed image (output space, encoded 0..1) at the recipe's size, with its watermark.
fn resized(out: &crate::image::LinearImage, recipe: &ExportRecipe) -> Result<ImageBuffer<Rgb<f32>, Vec<f32>>> {
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
    recipe.watermark.apply(&mut buf, recipe.output_space)?;
    Ok(buf)
}

fn to_u8(v: &f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The file contents for the recipe's format, with its color profile and metadata.
fn encode(buf: &ImageBuffer<Rgb<f32>, Vec<f32>>, recipe: &ExportRecipe, info: &CaptureInfo) -> Result<Vec<u8>> {
    let (w, h) = buf.dimensions();
    let profile = icc::profile_for(recipe.output_space);
    let exif = crate::exif::build(&crate::exif::ExifFields {
        capture: recipe.capture_metadata.then_some(info),
        artist: &recipe.artist,
        copyright: &recipe.copyright,
    });
    let mut file = std::io::Cursor::new(Vec::new());
    match recipe.format {
        ExportFormat::Jpeg => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, recipe.quality.clamp(1, 100));
            enc.set_icc_profile(profile).ok();
            enc.set_exif_metadata(exif).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let mut enc = image::codecs::png::PngEncoder::new(&mut file);
            enc.set_icc_profile(profile).ok();
            enc.set_exif_metadata(exif).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff8 => {
            let data: Vec<u8> = buf.as_raw().iter().map(to_u8).collect();
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff16 => {
            let data: Vec<u16> = buf.as_raw().iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect();
            let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_ne_bytes()).collect();
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(&bytes, w, h, image::ExtendedColorType::Rgb16)?;
        }
    }
    Ok(file.into_inner())
}

/// What a recipe produces, for proofing.
pub struct Proof {
    pub width: usize,
    pub height: usize,
    /// The stored pixels (JPEG decoded again, with its artifacts), converted from the
    /// recipe's color space to sRGB for display: 8-bit RGBA.
    pub rgba: Vec<u8>,
    /// Size of the file the recipe writes, in bytes.
    pub file_size: usize,
}

/// Which pixels of a processing in ProPhoto RGB (`Processor::process(.., OutputSpace::ProPhoto)`)
/// lie outside `space`, i.e. are clipped when written in it. Row-major, one entry per pixel.
pub fn clipped_in(wide: &crate::image::LinearImage, space: OutputSpace) -> Result<Vec<bool>> {
    if space == OutputSpace::ProPhoto {
        return Ok(vec![false; wide.data.len()]);
    }
    let m = mat_mul(&mat_inv(&space.to_xyz_d50()).context("Farbraum-Matrix")?, &OutputSpace::ProPhoto.to_xyz_d50());
    let trc = OutputSpace::ProPhoto.trc();
    // Well beyond rounding, so that colors on the edge of the space aren't marked.
    const EPS: f32 = 0.002;
    Ok(wide.data.par_iter().map(|p| mat_vec(&m, p.map(|v| trc.decode(v))).iter().any(|&v| !(-EPS..=1.0 + EPS).contains(&v))).collect())
}

/// Proof of a full-resolution processing in the recipe's color space
/// (`Processor::process(.., recipe.output_space)`): exactly the export path, without the file.
/// With `clipped` (see [`clipped_in`], same size as `processed`), the colors the file clips are
/// shown in [`crate::softproof::GAMUT_ALARM`].
pub fn proof(processed: &crate::image::LinearImage, recipe: &ExportRecipe, info: &CaptureInfo, clipped: Option<&[bool]>) -> Result<Proof> {
    let buf = resized(processed, recipe)?;
    let bytes = encode(&buf, recipe, info)?;
    let (w, h) = buf.dimensions();
    let stored: Vec<f32> = match recipe.format {
        ExportFormat::Jpeg => image::load_from_memory(&bytes)?.to_rgb8().into_raw().into_iter().map(|v| v as f32 / 255.0).collect(),
        ExportFormat::Tiff16 => buf.as_raw().iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0).collect(),
        _ => buf.as_raw().iter().map(|v| to_u8(v) as f32 / 255.0).collect(),
    };
    // Output space -> sRGB (both relative to D50 in our matrices), clipping what sRGB can't show.
    let srgb_from_xyz = mat_inv(&OutputSpace::Srgb.to_xyz_d50()).context("sRGB-Matrix")?;
    let to_srgb = mat_mul(&srgb_from_xyz, &recipe.output_space.to_xyz_d50());
    let trc = recipe.output_space.trc();
    // Proof pixel -> processed pixel (the proof may be reduced to the recipe's long edge).
    let (pw, ph) = (processed.width.max(1), processed.height.max(1));
    let is_clipped = |i: usize| {
        clipped.is_some_and(|m| {
            let (x, y) = (i % w as usize, i / w as usize);
            m.get((y * ph / h.max(1) as usize).min(ph - 1) * pw + (x * pw / w.max(1) as usize).min(pw - 1)).copied().unwrap_or(false)
        })
    };
    let rgba = stored
        .par_chunks(3)
        .enumerate()
        .flat_map_iter(|(i, c)| {
            if is_clipped(i) {
                let [r, g, b] = crate::softproof::GAMUT_ALARM;
                return [r, g, b, 255];
            }
            let lin = mat_vec(&to_srgb, [trc.decode(c[0]), trc.decode(c[1]), trc.decode(c[2])]);
            let [r, g, b] = lin.map(|v| to_u8(&Trc::Srgb.encode(v.clamp(0.0, 1.0))));
            [r, g, b, 255]
        })
        .collect();
    Ok(Proof { width: w as usize, height: h as usize, rgba, file_size: bytes.len() })
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
        let dir = std::env::temp_dir().join(format!("sk-export-{}", std::process::id()));
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
        let res = export_many(&sample, &Adjustments::default(), &recipes, 1, "");
        let paths: Vec<PathBuf> = res.into_iter().map(|r| r.unwrap()).collect();
        let jpg = image::open(&paths[0]).unwrap();
        assert_eq!(jpg.width(), 400);
        // EXIF with the capture data, no GPS.
        let bytes = std::fs::read(&paths[0]).unwrap();
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"Exif\0\0II*\0") && has(b"Silberkorn"));
        let tif = image::open(&paths[1]).unwrap();
        assert_eq!(tif.width(), 300);
        assert!(matches!(tif, image::DynamicImage::ImageRgb16(_)));

        // The proof matches the written files: size, dimensions and (for sRGB) the pixels.
        let (meta, linear) = raw::load_full(&sample).unwrap();
        let processor = Processor::new(&meta, &Adjustments::default());
        let srgb = processor.process(&linear, None, OutputSpace::Srgb);
        let p = proof(&srgb, &recipes[0], &meta.info, None).unwrap();
        assert_eq!((p.width, p.height), (jpg.width() as usize, jpg.height() as usize));
        assert_eq!(p.file_size as u64, std::fs::metadata(&paths[0]).unwrap().len());
        let jpg = jpg.to_rgb8();
        let max_diff = p.rgba.chunks(4).zip(jpg.pixels()).flat_map(|(a, b)| (0..3).map(move |c| (a[c] as i32 - b[c] as i32).abs())).max().unwrap();
        assert!(max_diff <= 1, "Proof = Datei: {max_diff}");
        let adobe = processor.process(&linear, None, OutputSpace::AdobeRgb);
        let p = proof(&adobe, &recipes[1], &meta.info, None).unwrap();
        assert_eq!(p.file_size as u64, std::fs::metadata(&paths[1]).unwrap().len());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn proof_marks_what_the_file_clips() {
        let pp = OutputSpace::ProPhoto;
        // ProPhoto: a neutral grey and a saturated green that sRGB can't hold.
        let grey = pp.trc().encode(0.18);
        let wide = crate::image::LinearImage::new(2, 1, vec![[grey; 3], [0.0, pp.trc().encode(0.6), 0.0]]);
        let mask = clipped_in(&wide, OutputSpace::Srgb).unwrap();
        assert_eq!(mask, vec![false, true]);
        assert_eq!(clipped_in(&wide, OutputSpace::ProPhoto).unwrap(), vec![false, false]);

        let srgb = crate::image::LinearImage::new(2, 1, vec![[0.5; 3], [0.0, 1.0, 0.0]]);
        let recipe = ExportRecipe { format: ExportFormat::Png, ..Default::default() };
        let p = proof(&srgb, &recipe, &CaptureInfo::default(), Some(&mask)).unwrap();
        let alarm = crate::softproof::GAMUT_ALARM;
        assert_eq!(&p.rgba[4..7], &alarm);
        assert_ne!(&p.rgba[0..3], &alarm);
        let p = proof(&srgb, &recipe, &CaptureInfo::default(), None).unwrap();
        assert_ne!(&p.rgba[4..7], &alarm);
    }
}
