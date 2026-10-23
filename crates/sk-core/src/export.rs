//! Export ("process recipes"): full-resolution development to JPEG / TIFF / PNG / WebP / AVIF.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{ImageBuffer, ImageEncoder, Rgb};
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
    WebP,
    Avif,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] =
        [ExportFormat::Jpeg, ExportFormat::Tiff16, ExportFormat::Tiff8, ExportFormat::Png, ExportFormat::WebP, ExportFormat::Avif];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Tiff16 => "TIFF 16 Bit",
            ExportFormat::Tiff8 => "TIFF 8 Bit",
            ExportFormat::Png => "PNG",
            ExportFormat::WebP => "WebP",
            ExportFormat::Avif => "AVIF",
        }
    }

    /// Lossy, with a quality setting.
    pub fn has_quality(self) -> bool {
        matches!(self, ExportFormat::Jpeg | ExportFormat::WebP | ExportFormat::Avif)
    }

    /// The file gets the EXIF block (capture data, author, copyright).
    pub fn has_metadata(self) -> bool {
        !matches!(self, ExportFormat::Tiff16 | ExportFormat::Tiff8)
    }

    /// Whether the format can hold `space` (AVIF is tagged without ICC profile).
    pub fn supports(self, space: OutputSpace) -> bool {
        self != ExportFormat::Avif || crate::web_formats::avif_supports(space)
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Tiff16 | ExportFormat::Tiff8 => "tif",
            ExportFormat::Png => "png",
            ExportFormat::WebP => "webp",
            ExportFormat::Avif => "avif",
        }
    }
}

/// What the recipe's size is measured on. The image is never enlarged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resize {
    #[default]
    LongEdge,
    ShortEdge,
    Width,
    Height,
    Percent,
}

impl Resize {
    pub const ALL: [Resize; 5] = [Resize::LongEdge, Resize::ShortEdge, Resize::Width, Resize::Height, Resize::Percent];

    pub fn label(self) -> &'static str {
        match self {
            Resize::LongEdge => "Lange Kante",
            Resize::ShortEdge => "Kurze Kante",
            Resize::Width => "Breite",
            Resize::Height => "Höhe",
            Resize::Percent => "Prozent",
        }
    }
}

/// Unsharp mask after resizing: resizing softens the picture, and print needs more than a screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputSharpening {
    #[default]
    Off,
    Screen,
    Print,
}

impl OutputSharpening {
    pub const ALL: [OutputSharpening; 3] = [OutputSharpening::Off, OutputSharpening::Screen, OutputSharpening::Print];

    pub fn label(self) -> &'static str {
        match self {
            OutputSharpening::Off => "Aus",
            OutputSharpening::Screen => "Für Bildschirm",
            OutputSharpening::Print => "Für Druck",
        }
    }

    /// Amount and radius (output pixels).
    fn strength(self) -> Option<(f32, f32)> {
        match self {
            OutputSharpening::Off => None,
            OutputSharpening::Screen => Some((0.6, 0.6)),
            OutputSharpening::Print => Some((1.2, 1.0)),
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
    /// Quality 1..100 (JPEG, WebP, AVIF)
    pub quality: u8,
    /// Resizing (see `resize`): the target in pixels, or in percent (None = original size).
    /// Recipes from before the other modes have it as `long_edge`.
    #[serde(alias = "long_edge")]
    pub size: Option<u32>,
    /// What `size` is measured on.
    pub resize: Resize,
    /// Sharpening after resizing, for the medium.
    pub output_sharpening: OutputSharpening,
    /// Output folder; relative paths are resolved against the image's folder.
    pub output_dir: PathBuf,
    /// File name with placeholders (see `naming`); empty: the image's name.
    pub name_template: String,
    /// Suffix appended to the file stem.
    pub suffix: String,
    pub output_space: OutputSpace,
    pub watermark: Watermark,
    /// Camera, lens, exposure and capture date in the file's EXIF (not TIFF). GPS is never written.
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
            size: None,
            resize: Resize::LongEdge,
            output_sharpening: OutputSharpening::Off,
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
    /// The size of an output of `w` x `h` after resizing (never larger).
    pub fn output_size(&self, w: u32, h: u32) -> (u32, u32) {
        let Some(v) = self.size.filter(|v| *v > 0) else { return (w, h) };
        let (v, fw, fh) = (v as f64, w as f64, h as f64);
        let k = match self.resize {
            Resize::LongEdge => v / fw.max(fh),
            Resize::ShortEdge => v / fw.min(fh),
            Resize::Width => v / fw,
            Resize::Height => v / fh,
            Resize::Percent => v / 100.0,
        };
        if k >= 1.0 {
            return (w, h);
        }
        (((fw * k).round() as u32).max(1), ((fh * k).round() as u32).max(1))
    }

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
    export_many_with(image, adj, recipes, counter, session, &mut HashSet::new())
}

/// [`export_many`], where `written` holds the files written so far in this batch: an output whose
/// name is among them gets `-2`, `-3` … instead of replacing it (same file names from two folders,
/// a name template that gives a burst the same name). Names differing only in case count as the
/// same: Windows and macOS would write the same file.
pub fn export_many_with(
    image: &Path,
    adj: &Adjustments,
    recipes: &[ExportRecipe],
    counter: u32,
    session: &str,
    written: &mut HashSet<PathBuf>,
) -> Vec<Result<PathBuf>> {
    let loaded = raw::load_full(image).and_then(|(meta, linear)| {
        // Never silently without the Raw-Details and denoising the image shows.
        if adj.detail.effective_raw_details() > 0.0 && !meta.hdr && meta.bayer {
            crate::ai_products::DETAILS.ensure(image, &|_| {}).context("Raw-Details")?;
        }
        if adj.detail.ai_denoise > 0.0 && !meta.hdr {
            crate::ai_products::DENOISE.ensure(image, &|_| {}).context("KI-Entrauschen")?;
        }
        // Nor without the lens blur, relight or atmosphere.
        if adj.needs_depth() {
            crate::depth::ensure(image, &|_| {}).context("Tiefenkarte")?;
        }
        Ok((meta, linear))
    });
    let (meta, linear) = match loaded {
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
            write_output(&fields, out, recipe, written)
        })
        .collect()
}

fn write_output(f: &NameFields, out: &crate::image::LinearImage, recipe: &ExportRecipe, written: &mut HashSet<PathBuf>) -> Result<PathBuf> {
    let info = f.capture.cloned().unwrap_or_default();
    let bytes = encode(&resized(out, recipe)?, recipe, &info)?;
    let path = unused_in_batch(recipe.output_path(f), written);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Kann Ordner {} nicht anlegen", dir.display()))?;
    }
    crate::safe_file::write_nosync(&path, bytes)?;
    written.insert(batch_key(&path));
    Ok(path)
}

/// `path` as the batch remembers it: case doesn't tell files apart on Windows and macOS.
fn batch_key(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// `path`, or `name-2.ext`, `name-3.ext` … if this batch already wrote a file of that name.
fn unused_in_batch(path: PathBuf, written: &HashSet<PathBuf>) -> PathBuf {
    if !written.contains(&batch_key(&path)) {
        return path;
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| path.with_file_name(format!("{stem}-{n}{ext}"))).find(|p| !written.contains(&batch_key(p))).expect("unbounded")
}

/// The processed image (output space, encoded 0..1) at the recipe's size, with its watermark.
/// `out` as the recipe writes it: scaled, sharpened and with its watermark; as it is (without a
/// copy) where the recipe changes nothing.
fn resized<'a>(out: &'a crate::image::LinearImage, recipe: &ExportRecipe) -> Result<ImageBuffer<Rgb<f32>, Cow<'a, [f32]>>> {
    let (w, h) = (out.width as u32, out.height as u32);
    let (nw, nh) = recipe.output_size(w, h);
    let sharpening = recipe.output_sharpening.strength();
    if (nw, nh) == (w, h) && sharpening.is_none() && recipe.watermark.is_empty() {
        return ImageBuffer::from_raw(w, h, Cow::Borrowed(out.data.as_flattened())).context("Puffergröße ungültig");
    }
    let mut buf: ImageBuffer<Rgb<f32>, Vec<f32>> = ImageBuffer::from_raw(w, h, out.data.as_flattened().to_vec()).context("Puffergröße ungültig")?;
    if (nw, nh) != (w, h) {
        buf = resize_lanczos3(&buf, nw, nh);
    }
    if let Some((amount, radius)) = sharpening {
        sharpen(&mut buf, amount, radius);
    }
    recipe.watermark.apply(&mut buf, recipe.output_space)?;
    let (w, h) = buf.dimensions();
    ImageBuffer::from_raw(w, h, Cow::Owned(buf.into_raw())).context("Puffergröße ungültig")
}

/// Lanczos3 resampling like `image::imageops::resize`, but on all cores (that one runs on one
/// and took seconds for a large image).
fn resize_lanczos3(src: &ImageBuffer<Rgb<f32>, Vec<f32>>, nw: u32, nh: u32) -> ImageBuffer<Rgb<f32>, Vec<f32>> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let (nw, nh) = (nw as usize, nh as usize);
    // Per output position: first source index and normalized weights.
    let weights = |n: usize, out: usize| -> Vec<(usize, Vec<f32>)> {
        let ratio = n as f32 / out as f32;
        let scale = ratio.max(1.0);
        let support = 3.0 * scale;
        (0..out)
            .map(|o| {
                let center = (o as f32 + 0.5) * ratio;
                let lo = ((center - support).floor().max(0.0) as usize).min(n - 1);
                let hi = ((center + support).ceil() as usize).clamp(lo + 1, n);
                let mut ws: Vec<f32> = (lo..hi).map(|i| lanczos3((i as f32 + 0.5 - center) / scale)).collect();
                let sum: f32 = ws.iter().sum();
                if sum != 0.0 {
                    ws.iter_mut().for_each(|v| *v /= sum);
                }
                (lo, ws)
            })
            .collect()
    };
    let (wx, wy) = (weights(w, nw), weights(h, nh));
    let data = src.as_raw();
    // Horizontal pass into an intermediate of nw x h, then vertical.
    let mut mid = vec![0.0f32; nw * h * 3];
    mid.par_chunks_mut(nw * 3).enumerate().for_each(|(y, row)| {
        let line = &data[y * w * 3..(y + 1) * w * 3];
        for (x, (lo, ws)) in wx.iter().enumerate() {
            let mut acc = [0.0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let p = &line[(lo + k) * 3..(lo + k) * 3 + 3];
                acc[0] += p[0] * wt;
                acc[1] += p[1] * wt;
                acc[2] += p[2] * wt;
            }
            row[x * 3..x * 3 + 3].copy_from_slice(&acc);
        }
    });
    let mut out = vec![0.0f32; nw * nh * 3];
    out.par_chunks_mut(nw * 3).zip(&wy).for_each(|(row, (lo, ws))| {
        for (k, wt) in ws.iter().enumerate() {
            let line = &mid[(lo + k) * nw * 3..(lo + k + 1) * nw * 3];
            row.iter_mut().zip(line).for_each(|(o, v)| *o += v * wt);
        }
        // Output-encoded values: the overshoot of the filter at edges is cut off.
        row.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    });
    ImageBuffer::from_raw(nw as u32, nh as u32, out).expect("size")
}

fn lanczos3(x: f32) -> f32 {
    let sinc = |x: f32| if x == 0.0 { 1.0 } else { (std::f32::consts::PI * x).sin() / (std::f32::consts::PI * x) };
    if x.abs() < 3.0 { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// Unsharp mask on the luminance of output-encoded RGB (colors keep their place: no colored halos).
fn sharpen(buf: &mut ImageBuffer<Rgb<f32>, Vec<f32>>, amount: f32, radius: f32) {
    let (w, h) = (buf.width() as usize, buf.height() as usize);
    let luma = |p: &[f32]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let plane = crate::detail::Plane::new(w, h, buf.as_raw().par_chunks(3).map(luma).collect());
    let blurred = crate::detail::gaussian_blur(&plane, radius);
    buf.as_mut().par_chunks_mut(3).enumerate().for_each(|(i, p)| {
        let d = (plane.data[i] - blurred.data[i]) * amount;
        for v in p.iter_mut() {
            *v = (*v + d).clamp(0.0, 1.0);
        }
    });
}

fn to_u8(v: &f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// A JPEG of 8-bit RGB `data` (`w` x `h`) at `quality` (1..100), with an ICC profile and EXIF
/// data (TIFF-structured, see `exif::build`; none if empty). `jpeg-encoder` instead of the
/// `image` crate's encoder: SIMD, about twice as fast (1.35 s for 24 MP before); from quality 90
/// on with full color resolution (4:4:4), below 4:2:0. Large images in strips on all cores.
pub fn encode_jpeg(data: &[u8], w: u32, h: u32, quality: u8, icc: Option<&[u8]>, exif: &[u8]) -> Result<Vec<u8>> {
    let strips = if (w as usize) * (h as usize) < 1_000_000 { 1 } else { rayon::current_num_threads() };
    encode_jpeg_strips(data, w, h, quality, icc, exif, strips)
}

/// Rows of pixels in a JPEG's block row (MCU) at `quality`.
fn jpeg_mcu(quality: u8) -> usize {
    if quality < 90 { 16 } else { 8 }
}

fn jpeg_encoder(out: &mut Vec<u8>, quality: u8) -> jpeg_encoder::Encoder<&mut Vec<u8>> {
    let mut enc = jpeg_encoder::Encoder::new(out, quality);
    enc.set_sampling_factor(if jpeg_mcu(quality) == 16 { jpeg_encoder::SamplingFactor::F_2_2 } else { jpeg_encoder::SamplingFactor::F_1_1 });
    enc
}

/// [`encode_jpeg`] in up to `strips` horizontal strips of whole block rows, each encoded on its
/// own and joined: every block row starts with a restart marker (as the standard allows), so a
/// strip's data doesn't depend on the ones before. The file is the same as from one encoder with
/// that restart interval.
fn encode_jpeg_strips(data: &[u8], w: u32, h: u32, quality: u8, icc: Option<&[u8]>, exif: &[u8], strips: usize) -> Result<Vec<u8>> {
    let (Ok(w16), Ok(h16)) = (u16::try_from(w), u16::try_from(h)) else {
        anyhow::bail!("JPEG erlaubt höchstens 65535 Pixel je Seite ({w}×{h})");
    };
    let quality = quality.clamp(1, 100);
    let (w, h, mcu) = (w as usize, h as usize, jpeg_mcu(quality));
    if data.len() != w * h * 3 {
        anyhow::bail!("Puffergröße ungültig");
    }
    let block_rows = h.div_ceil(mcu);
    let rows = block_rows.div_ceil(strips.clamp(1, block_rows.max(1))) * mcu;
    let restart = (rows < h).then_some(w.div_ceil(mcu) as u16);
    let parts: Vec<Vec<u8>> = (0..h)
        .step_by(rows.max(1))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|y0| {
            let sh = rows.min(h - y0);
            let mut out = Vec::new();
            let mut enc = jpeg_encoder(&mut out, quality);
            if let Some(r) = restart {
                enc.set_restart_interval(r);
            }
            if y0 == 0 {
                // EXIF first, as cameras and other programs write it.
                if !exif.is_empty() {
                    let mut app1 = b"Exif\0\0".to_vec();
                    app1.extend_from_slice(exif);
                    enc.add_app_segment(1, &app1)?;
                }
                if let Some(icc) = icc {
                    enc.add_icc_profile(icc)?;
                }
            }
            enc.encode(&data[y0 * w * 3..(y0 + sh) * w * 3], w16, sh as u16, jpeg_encoder::ColorType::Rgb)?;
            Ok(out)
        })
        .collect::<Result<_>>()?;
    let mut parts = parts.into_iter();
    let Some(mut out) = parts.next() else {
        anyhow::bail!("JPEG: leeres Bild");
    };
    if restart.is_none() {
        return Ok(out);
    }
    // The first strip's header with the whole height, then each strip's entropy-coded data after
    // the restart marker of its first block row, its own markers numbered on (modulo 8).
    let (sof, _) = jpeg_scan(&out)?;
    out[sof + 5..sof + 7].copy_from_slice(&h16.to_be_bytes());
    out.truncate(out.len() - 2);
    for (k, part) in parts.enumerate() {
        let first = (k + 1) * rows / mcu;
        out.extend_from_slice(&[0xFF, 0xD0 + ((first - 1) % 8) as u8]);
        let (_, scan) = jpeg_scan(&part)?;
        let coded = &part[scan..part.len() - 2];
        out.reserve(coded.len());
        let mut marker = false;
        for &b in coded {
            // 0xFF in the coded data is followed by a stuffed 0, or it starts a restart marker.
            out.push(if marker && (0xD0..=0xD7).contains(&b) { 0xD0 + (((b - 0xD0) as usize + first) % 8) as u8 } else { b });
            marker = b == 0xFF;
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// Where the frame header (SOF0 marker) and the entropy-coded data (after the scan header) of
/// one baseline JPEG from `jpeg-encoder` start, which ends with the end marker.
fn jpeg_scan(jpeg: &[u8]) -> Result<(usize, usize)> {
    let mut sof = None;
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        match jpeg[i + 1] {
            0xC0 => sof = Some(i),
            0xDA => {
                let sof = sof.context("JPEG: kein Bildkopf")?;
                anyhow::ensure!(jpeg.ends_with(&[0xFF, 0xD9]) && i + 2 + len <= jpeg.len() - 2, "JPEG: unerwartetes Ende");
                return Ok((sof, i + 2 + len));
            }
            _ => {}
        }
        i += 2 + len;
    }
    anyhow::bail!("JPEG: Bilddaten nicht gefunden")
}

/// The file contents for the recipe's format, with its color profile and metadata.
fn encode(buf: &ImageBuffer<Rgb<f32>, Cow<'_, [f32]>>, recipe: &ExportRecipe, info: &CaptureInfo) -> Result<Vec<u8>> {
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
            let data: Vec<u8> = buf.as_raw().par_iter().map(to_u8).collect();
            return encode_jpeg(&data, w, h, recipe.quality, Some(&profile), &exif);
        }
        ExportFormat::Png => {
            let data: Vec<u8> = buf.as_raw().par_iter().map(to_u8).collect();
            let mut enc = image::codecs::png::PngEncoder::new(&mut file);
            enc.set_icc_profile(profile).ok();
            enc.set_exif_metadata(exif).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff8 => {
            let data: Vec<u8> = buf.as_raw().par_iter().map(to_u8).collect();
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(&data, w, h, image::ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff16 => {
            let data: Vec<u16> = buf.as_raw().par_iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect();
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut file);
            enc.set_icc_profile(profile).ok();
            enc.write_image(bytemuck::cast_slice(&data), w, h, image::ExtendedColorType::Rgb16)?;
        }
        ExportFormat::WebP => {
            let data: Vec<u8> = buf.as_raw().par_iter().map(to_u8).collect();
            return crate::web_formats::encode_webp(&data, w, h, recipe.quality, &profile, &exif);
        }
        ExportFormat::Avif => {
            return crate::web_formats::encode_avif(buf.as_raw(), w, h, recipe.quality, recipe.output_space, &exif);
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
        // AVIF can't be decoded here: the proof shows the pixels before compression.
        ExportFormat::Jpeg | ExportFormat::WebP => image::load_from_memory(&bytes)?.to_rgb8().into_raw().into_iter().map(|v| v as f32 / 255.0).collect(),
        ExportFormat::Tiff16 => buf.as_raw().par_iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0).collect(),
        _ => buf.as_raw().par_iter().map(|v| to_u8(v) as f32 / 255.0).collect(),
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

    /// A JPEG encoded in strips is the file one encoder writes with a restart marker per block
    /// row (for both color resolutions, odd sizes and more strips than markers go round), and
    /// decodes to the same picture as one without restart markers.
    #[test]
    fn jpeg_in_strips_is_one_jpeg() {
        let (w, h) = (1003u32, 1517u32);
        let data: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / w) as u8, (y * 255 / h) as u8, ((x * 7 + y * 3 + x * y / 97) % 256) as u8]
            })
            .collect();
        for quality in [85, 95] {
            let mut whole = Vec::new();
            let mut enc = jpeg_encoder(&mut whole, quality);
            enc.set_restart_interval(w.div_ceil(jpeg_mcu(quality) as u32) as u16);
            enc.add_icc_profile(b"profil").unwrap();
            enc.encode(&data, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).unwrap();
            for strips in [2, 7, 12] {
                let striped = encode_jpeg_strips(&data, w, h, quality, Some(b"profil"), &[], strips).unwrap();
                assert!(striped == whole, "Qualität {quality}, {strips} Streifen");
            }
            let plain = encode_jpeg_strips(&data, w, h, quality, None, &[], 1).unwrap();
            let decoded = |j: &[u8]| image::load_from_memory(j).unwrap().to_rgb8().into_raw();
            assert!(decoded(&whole) == decoded(&plain));
        }
    }

    /// The JPEG keeps profile and EXIF where other programs find them, and the picture differs
    /// only as much as JPEG at that quality does.
    #[test]
    fn jpeg_has_profile_exif_and_the_picture() {
        use image::ImageDecoder;
        let (w, h) = (320u32, 200u32);
        let data: Vec<u8> = (0..w * h).flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 255 / w) as u8, (y * 255 / h) as u8, ((x + y) % 256) as u8]
        }).collect();
        let icc = icc::profile_for(OutputSpace::AdobeRgb);
        let info = CaptureInfo { make: "Testkamera".into(), model: "X1".into(), ..Default::default() };
        let exif = crate::exif::build(&crate::exif::ExifFields { capture: Some(&info), artist: "Ich", copyright: "" });
        let jpeg = encode_jpeg(&data, w, h, 92, Some(&icc), &exif).unwrap();
        let mut dec = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&jpeg)).unwrap();
        assert_eq!(dec.icc_profile().unwrap().as_deref(), Some(icc.as_slice()));
        let read = dec.exif_metadata().unwrap().expect("EXIF");
        assert_eq!(&read[..4], b"II*\0");
        assert!(read.windows(10).any(|s| s == b"Testkamera"));
        let img = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (w, h));
        let err = img.as_raw().iter().zip(&data).map(|(a, b)| (*a as f64 - *b as f64).abs()).sum::<f64>() / data.len() as f64;
        assert!(err < 2.0, "mittlere Abweichung {err}");
    }

    /// The parallel resampling gives what `image` gives (smaller and larger).
    #[test]
    fn resize_matches_image_crate() {
        let (w, h) = (301u32, 199u32);
        let src = ImageBuffer::from_fn(w, h, |x, y| {
            let v = |k: u32| ((x * k + y * (k + 3)) % 97) as f32 / 96.0;
            Rgb([v(5), v(11), if (x / 7 + y / 5) % 2 == 0 { 1.0 } else { 0.0 }])
        });
        for (nw, nh) in [(100, 66), (37, 25), (450, 300), (301, 100)] {
            let ours = resize_lanczos3(&src, nw, nh);
            let theirs = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Lanczos3);
            let max = ours.as_raw().iter().zip(theirs.as_raw()).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(max < 2e-3, "{nw}x{nh}: {max}");
        }
    }

    #[test]
    fn names_are_unique_within_a_batch() {
        let mut written = HashSet::new();
        let a = unused_in_batch(PathBuf::from("/o/IMG_1.jpg"), &written);
        written.insert(batch_key(&a));
        let b = unused_in_batch(PathBuf::from("/o/IMG_1.jpg"), &written);
        written.insert(batch_key(&b));
        // Another folder's `img_1.ARW`: Windows and macOS would overwrite the first file.
        let c = unused_in_batch(PathBuf::from("/o/img_1.jpg"), &written);
        assert_eq!((a, b, c), (PathBuf::from("/o/IMG_1.jpg"), PathBuf::from("/o/IMG_1-2.jpg"), PathBuf::from("/o/img_1-3.jpg")));
    }

    #[test]
    fn export_many_writes_each_recipe() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0010.ARW");
        if !sample.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("sk-export-{}", std::process::id()));
        let recipes = vec![
            ExportRecipe { output_dir: dir.clone(), size: Some(400), ..Default::default() },
            ExportRecipe {
                name: "TIFF".into(),
                format: ExportFormat::Tiff16,
                output_space: OutputSpace::AdobeRgb,
                size: Some(300),
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
    fn resizing_by_each_measure_and_never_larger() {
        let r = |resize, size| ExportRecipe { resize, size: Some(size), ..Default::default() };
        assert_eq!(r(Resize::LongEdge, 1000).output_size(4000, 3000), (1000, 750));
        assert_eq!(r(Resize::ShortEdge, 1000).output_size(4000, 3000), (1333, 1000));
        assert_eq!(r(Resize::Width, 2000).output_size(3000, 4000), (2000, 2667));
        assert_eq!(r(Resize::Height, 2000).output_size(3000, 4000), (1500, 2000));
        assert_eq!(r(Resize::Percent, 25).output_size(4000, 3000), (1000, 750));
        assert_eq!(r(Resize::Width, 8000).output_size(4000, 3000), (4000, 3000));
        assert_eq!(ExportRecipe::default().output_size(4000, 3000), (4000, 3000));
    }

    #[test]
    fn output_sharpening_raises_edge_contrast_without_color() {
        let mut buf: ImageBuffer<Rgb<f32>, Vec<f32>> = ImageBuffer::from_fn(40, 8, |x, _| if x < 20 { Rgb([0.3, 0.3, 0.3]) } else { Rgb([0.6, 0.6, 0.6]) });
        sharpen(&mut buf, 1.2, 1.0);
        let (dark, bright) = (buf.get_pixel(19, 4).0, buf.get_pixel(20, 4).0);
        assert!(dark[0] < 0.3 && bright[0] > 0.6, "{dark:?} {bright:?}");
        assert!(dark.iter().all(|v| (v - dark[0]).abs() < 1e-6), "neutral bleibt neutral");
        assert!((buf.get_pixel(2, 4).0[0] - 0.3).abs() < 1e-4, "Fläche bleibt");
    }

    #[test]
    fn saved_recipes_from_before_keep_working() {
        let old: ExportRecipe = serde_json::from_str(r#"{"name":"Web","long_edge":2048,"upscale":{"factor":4}}"#).unwrap();
        assert_eq!((old.size, old.resize), (Some(2048), Resize::LongEdge));
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
