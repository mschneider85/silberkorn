//! Raw file loading on top of rawler.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rawler::RawImage;
use rawler::cfa::CFAColor;
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::{RawImageData, RawPhotometricInterpretation};
use rawler::rawsource::RawSource;
use rayon::prelude::*;

use crate::color::{CameraColor, Mat3};
use crate::image::{LinearImage, Orientation};

pub const RAW_EXTENSIONS: &[&str] = &[
    "arw", "srf", "sr2", "cr2", "cr3", "crw", "nef", "nrw", "raf", "orf", "rw2", "pef", "dng", "srw", "3fr", "iiq", "mos",
    "erf", "kdc", "dcr", "mrw",
];

pub fn is_raw_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Human-readable capture information.
#[derive(Clone, Debug, Default)]
pub struct CaptureInfo {
    pub make: String,
    pub model: String,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    pub exposure_time: Option<String>,
    /// Exposure time in seconds as the camera stored it (numerator, denominator).
    pub exposure_rational: Option<(u32, u32)>,
    pub aperture: Option<String>,
    pub focal_length: Option<String>,
    pub date: Option<String>,
    pub focal_mm: Option<f32>,
    pub fnumber: Option<f32>,
    pub width: usize,
    pub height: usize,
}

/// Everything about a raw file that the pipeline needs besides the pixels.
#[derive(Clone, Debug)]
pub struct RawMeta {
    pub path: PathBuf,
    pub info: CaptureInfo,
    pub color: CameraColor,
    /// As-shot camera multipliers, normalized to green = 1.
    pub as_shot_wb: [f32; 3],
    pub orientation: Orientation,
    /// Floating point linear data (merged HDR, panorama, denoised DNG): values above 1 are real,
    /// nothing is clipped.
    pub hdr: bool,
}

struct Decoded {
    raw: RawImage,
    meta: RawMeta,
}

fn decode(path: &Path) -> Result<Decoded> {
    decode_ext(path, false)
}

/// `dummy` skips decoding the pixel data (metadata only). `path` may be a variant (see
/// `variant`): the file is read, the metadata keeps the variant's path.
fn decode_ext(path: &Path, dummy: bool) -> Result<Decoded> {
    let file = crate::variant::file_of(path);
    let source = RawSource::new(&file).with_context(|| format!("Kann {} nicht öffnen", file.display()))?;
    let decoder: Box<dyn Decoder> = rawler::get_decoder(&source).map_err(|e| anyhow!("Unbekanntes RAW-Format: {e}"))?;
    let params = RawDecodeParams::default();
    let raw = decoder.raw_image(&source, &params, dummy).map_err(|e| anyhow!("RAW-Dekodierung fehlgeschlagen: {e}"))?;
    let md = decoder.raw_metadata(&source, &params).ok();
    let mut meta = build_meta(path, &raw, md.as_ref())?;
    meta.hdr = raw.cpp == 3 && matches!(raw.data, RawImageData::Float(_));
    Ok(Decoded { raw, meta })
}

fn build_meta(path: &Path, raw: &RawImage, md: Option<&RawMetadata>) -> Result<RawMeta> {
    let (_, matrix) = raw
        .color_matrix_find_first([Illuminant::D65, Illuminant::D55, Illuminant::D50, Illuminant::Daylight, Illuminant::A])
        .ok_or_else(|| anyhow!("Keine Farbmatrix für {} {}", raw.clean_make, raw.clean_model))?;
    if matrix.len() != 9 {
        bail!("Kameras mit 4-Farb-Sensor werden noch nicht unterstützt");
    }
    let mut xyz_to_cam: Mat3 = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            xyz_to_cam[i][j] = matrix[i * 3 + j];
        }
    }
    let color = CameraColor::new(xyz_to_cam);

    let wb = raw.wb_coeffs;
    let as_shot_wb = if wb[0].is_nan() || wb[1] <= 0.0 {
        color.wb_for_temp_tint(5500.0, 0.0)
    } else {
        [wb[0] / wb[1], 1.0, wb[2] / wb[1]]
    };

    let mut info = CaptureInfo {
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        width: raw.width,
        height: raw.height,
        ..Default::default()
    };
    let mut orientation = Orientation::from_exif(raw.orientation as u16 + 1);
    if let Some(md) = md {
        let ex = &md.exif;
        info.iso = ex.iso_speed_ratings.map(u32::from).or(ex.iso_speed).or(ex.recommended_exposure_index);
        info.exposure_time = ex.exposure_time.filter(|r| r.d != 0).map(|r| {
            let s = r.n as f64 / r.d as f64;
            if s >= 0.5 { format!("{s:.1} s") } else { format!("1/{:.0} s", 1.0 / s) }
        });
        info.exposure_rational = ex.exposure_time.filter(|r| r.d != 0 && r.n != 0).map(|r| (r.n, r.d));
        info.aperture = ex.fnumber.filter(|r| r.d != 0).map(|r| format!("f/{:.1}", r.n as f64 / r.d as f64));
        info.fnumber = ex.fnumber.filter(|r| r.d != 0).map(|r| r.n as f32 / r.d as f32);
        info.focal_mm = ex.focal_length.filter(|r| r.d != 0).map(|r| r.n as f32 / r.d as f32);
        info.focal_length = ex.focal_length.filter(|r| r.d != 0).map(|r| format!("{:.0} mm", r.n as f64 / r.d as f64));
        info.date = ex.date_time_original.clone().or(ex.create_date.clone());
        info.lens = md
            .lens
            .as_ref()
            .map(|l| format!("{} {}", l.lens_make, l.lens_model).trim().to_string())
            .or(ex.lens_model.clone())
            .filter(|s| !s.is_empty());
        if let Some(o) = ex.orientation {
            orientation = Orientation::from_exif(o);
        }
    }
    Ok(RawMeta { path: path.to_path_buf(), info, color, as_shot_wb, orientation, hdr: false })
}

/// Reads only the metadata, without decoding pixel data.
pub fn read_meta(path: &Path) -> Result<RawMeta> {
    Ok(decode_ext(path, true)?.meta)
}

/// Fast preview: half-size "superpixel" demosaic, then box-downscaled so the
/// long edge is at most `max_edge`. Returns camera-space linear RGB, oriented.
pub fn load_preview(path: &Path, max_edge: usize) -> Result<(RawMeta, LinearImage)> {
    let Decoded { mut raw, meta } = decode(path)?;
    raw.apply_scaling().map_err(|e| anyhow!("Skalierung fehlgeschlagen: {e}"))?;
    let img = match (&raw.photometric, raw.cpp) {
        (RawPhotometricInterpretation::Cfa(cfg), 1) if cfg.cfa.is_rgb() && cfg.cfa.width == 2 && cfg.cfa.height == 2 => {
            superpixel(&raw, &cfg.cfa)?
        }
        // X-Trans and other layouts: fall back to the full demosaic.
        (RawPhotometricInterpretation::Cfa(_), 1) => develop_full(&raw)?,
        (_, 3) => {
            let RawImageData::Float(d) = &raw.data else { bail!("Unerwartetes Datenformat") };
            LinearImage::new(raw.width, raw.height, d.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect())
        }
        _ => bail!("Dieses Sensorlayout wird noch nicht unterstützt"),
    };
    let img = img.fit_within(max_edge).oriented(meta.orientation);
    Ok((meta, img))
}

/// Full-resolution demosaic (PPG) in camera space, oriented.
pub fn load_full(path: &Path) -> Result<(RawMeta, LinearImage)> {
    let Decoded { raw, meta } = decode(path)?;
    let img = develop_full(&raw)?.oriented(meta.orientation);
    Ok((meta, img))
}

fn develop_full(raw: &RawImage) -> Result<LinearImage> {
    let dev = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::CropDefault,
    ]);
    match dev.develop_intermediate(raw).map_err(|e| anyhow!("Entwicklung fehlgeschlagen: {e}"))? {
        Intermediate::ThreeColor(px) => Ok(LinearImage::new(px.width, px.height, px.data)),
        Intermediate::Monochrome(px) => {
            Ok(LinearImage::new(px.width, px.height, px.data.into_iter().map(|v| [v, v, v]).collect()))
        }
        Intermediate::FourColor(_) => bail!("Kameras mit 4-Farb-Sensor werden noch nicht unterstützt"),
    }
}

fn superpixel(raw: &RawImage, cfa: &rawler::CFA) -> Result<LinearImage> {
    let RawImageData::Float(data) = &raw.data else { bail!("Unerwartetes Datenformat") };
    let area = raw.crop_area.or(raw.active_area);
    let (x0, y0, w, h) = match area {
        Some(r) => (r.p.x, r.p.y, r.d.w, r.d.h),
        None => (0, 0, raw.width, raw.height),
    };
    let (ow, oh) = (w / 2, h / 2);
    let stride = raw.width;
    // Channel index for each position of the 2x2 block.
    let mut chan = [[0usize; 2]; 2];
    for (dy, row) in chan.iter_mut().enumerate() {
        for (dx, c) in row.iter_mut().enumerate() {
            *c = match cfa.cfa_color_at(y0 + dy, x0 + dx) {
                CFAColor::RED => 0,
                CFAColor::GREEN => 1,
                CFAColor::BLUE => 2,
                other => bail!("Nicht unterstützte CFA-Farbe {other:?}"),
            };
        }
    }
    let mut out = vec![[0.0f32; 3]; ow * oh];
    out.par_chunks_mut(ow).enumerate().for_each(|(oy, line)| {
        let y = y0 + oy * 2;
        for (ox, px) in line.iter_mut().enumerate() {
            let x = x0 + ox * 2;
            let mut acc = [0.0f32; 3];
            let mut cnt = [0.0f32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let c = chan[dy][dx];
                    acc[c] += data[(y + dy) * stride + x + dx];
                    cnt[c] += 1.0;
                }
            }
            *px = [acc[0] / cnt[0].max(1.0), acc[1] / cnt[1].max(1.0), acc[2] / cnt[2].max(1.0)];
        }
    });
    Ok(LinearImage::new(ow, oh, out))
}

/// Embedded JPEG preview (fast thumbnails), already oriented.
pub fn embedded_preview(path: &Path) -> Result<::image::DynamicImage> {
    let source = RawSource::new(&crate::variant::file_of(path))?;
    let decoder = rawler::get_decoder(&source).map_err(|e| anyhow!("{e}"))?;
    let params = RawDecodeParams::default();
    let img = decoder
        .preview_image(&source, &params)
        .ok()
        .flatten()
        .or_else(|| decoder.thumbnail_image(&source, &params).ok().flatten())
        .ok_or_else(|| anyhow!("Kein eingebettetes Vorschaubild"))?;
    let orientation = decoder
        .raw_metadata(&source, &params)
        .ok()
        .and_then(|m| m.exif.orientation)
        .map(Orientation::from_exif)
        .unwrap_or_default();
    Ok(orientation.apply_dynamic(img))
}
