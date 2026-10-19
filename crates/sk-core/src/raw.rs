//! Raw file loading on top of rawler.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rawler::RawImage;
use rawler::cfa::CFAColor;
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::xyz::Illuminant;
use rawler::imgop::Rect;
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
    /// Lens correction stored by the camera (oriented like the image).
    pub lens: Option<crate::camera_lens::CameraLens>,
    /// The lens is already corrected (a stitched panorama): no lens profile applies.
    pub lens_corrected: bool,
}

#[derive(Clone)]
struct Decoded {
    raw: RawImage,
    meta: RawMeta,
}

/// The raw file last decoded by [`load_preview_and_keep`], until [`load_full`] takes it: by the
/// image's path and the identity of its file (a changed file is decoded again).
static KEPT: std::sync::Mutex<Option<(String, Decoded)>> = std::sync::Mutex::new(None);

/// Raw data larger than this is not kept (float or multi-channel files).
const KEEP_MAX_BYTES: usize = 256 << 20;

fn kept_key(path: &Path) -> Option<String> {
    Some(format!("{}|{}", path.display(), crate::cache::file_identity(path)?))
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
    let mut raw = decoder.raw_image(&source, &params, dummy).map_err(|e| anyhow!("RAW-Dekodierung fehlgeschlagen: {e}"))?;
    // Float data may hold NaN or infinity (damaged or foreign DNGs): they would spread through
    // every neighborhood operation of the pipeline.
    if let RawImageData::Float(d) = &mut raw.data {
        d.par_iter_mut().filter(|v| !v.is_finite()).for_each(|v| *v = 0.0);
    }
    sanitize_areas(&mut raw, &file);
    if !dummy {
        apply_gain_maps(decoder.as_ref(), &mut raw)?;
    }
    let md = decoder.raw_metadata(&source, &params).ok();
    let mut meta = build_meta(path, &raw, md.as_ref())?;
    let size = image_size(&raw);
    meta.lens = crate::camera_lens::read(&file, size.0, size.1).map(|l| l.oriented(meta.orientation));
    meta.lens_corrected = matches!(
        decoder.ifd(rawler::decoders::WellKnownIFD::Root).ok().flatten().as_deref().and_then(|ifd| ifd.get_entry(50740u16)).map(|e| &e.value),
        Some(rawler::formats::tiff::Value::Byte(d) | rawler::formats::tiff::Value::Undefined(d)) if crate::dng::lens_corrected(d)
    );
    // Merged HDR data goes beyond the white level; float data within it (e.g. denoised raws)
    // is clipped like any raw file.
    meta.hdr = raw.cpp == 3 && matches!(&raw.data, RawImageData::Float(d) if d.par_iter().any(|v| *v > 1.0001));
    Ok(Decoded { raw, meta })
}

/// Drops an active area or default crop that does not fit (damaged or foreign DNGs): the code
/// working with them indexes the pixel data by them, and the crop must lie in the active area.
fn sanitize_areas(raw: &mut RawImage, file: &Path) {
    let (active, crop) = valid_areas(raw.width, raw.height, raw.active_area, raw.crop_area);
    if active != raw.active_area {
        log::warn!("{}: ungültiger aktiver Sensorbereich, wird ignoriert", file.display());
    }
    if crop != raw.crop_area {
        log::warn!("{}: ungültiger Standard-Beschnitt, wird ignoriert", file.display());
    }
    (raw.active_area, raw.crop_area) = (active, crop);
}

/// Active area and crop of a `width` x `height` sensor, each `None` if it doesn't fit.
fn valid_areas(width: usize, height: usize, active: Option<Rect>, crop: Option<Rect>) -> (Option<Rect>, Option<Rect>) {
    let fits = |r: &Rect, x0: usize, y0: usize, x1: usize, y1: usize| {
        r.d.w > 0 && r.d.h > 0 && r.p.x >= x0 && r.p.y >= y0 && r.p.x.checked_add(r.d.w).is_some_and(|e| e <= x1) && r.p.y.checked_add(r.d.h).is_some_and(|e| e <= y1)
    };
    let active = active.filter(|a| fits(a, 0, 0, width, height));
    let (x0, y0, x1, y1) = active.map_or((0, 0, width, height), |a| (a.p.x, a.p.y, a.p.x + a.d.w, a.p.y + a.d.h));
    (active, crop.filter(|c| fits(c, x0, y0, x1, y1)))
}

/// DNG lens shading (GainMap opcodes in OpcodeList2), on the scaled data of the active area.
fn apply_gain_maps(decoder: &dyn Decoder, raw: &mut RawImage) -> Result<()> {
    let Ok(Some(ifd)) = decoder.ifd(rawler::decoders::WellKnownIFD::Raw) else { return Ok(()) };
    let Some(rawler::formats::tiff::Value::Undefined(list)) = ifd.get_entry(0xC741u16).map(|e| &e.value) else { return Ok(()) };
    let maps = crate::camera_lens::gain_maps(list);
    // rawler can't scale other layouts (it panics on BlackIsZero).
    if maps.is_empty() || !matches!(raw.photometric, RawPhotometricInterpretation::Cfa(_) | RawPhotometricInterpretation::LinearRaw) {
        return Ok(());
    }
    raw.apply_scaling().map_err(|e| anyhow!("Skalierung fehlgeschlagen: {e}"))?;
    let (w, h, cpp) = (raw.width, raw.height, raw.cpp);
    let area = raw.active_area.map_or((0, 0, w, h), |r| (r.p.x, r.p.y, r.d.w, r.d.h));
    let RawImageData::Float(data) = &mut raw.data else { return Ok(()) };
    // The maps address the active area: work on a copy of it.
    let mut active: Vec<f32> = (0..area.3).flat_map(|y| data[((area.1 + y) * w + area.0) * cpp..((area.1 + y) * w + area.0 + area.2) * cpp].to_vec()).collect();
    for m in &maps {
        m.apply(&mut active, area.2, area.3, cpp);
    }
    for (y, row) in active.chunks_exact(area.2 * cpp).enumerate() {
        let start = ((area.1 + y) * w + area.0) * cpp;
        data[start..start + row.len()].copy_from_slice(row);
    }
    Ok(())
}

fn build_meta(path: &Path, raw: &RawImage, md: Option<&RawMetadata>) -> Result<RawMeta> {
    let (illuminant, matrix) = raw
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
    let mut color = CameraColor::new(xyz_to_cam);
    color.illuminant = illuminant as u16;
    // A second matrix for tungsten light, when the first is a daylight one.
    if illuminant != Illuminant::A
        && let Some((_, a)) = raw.color_matrix_find_first([Illuminant::A])
        && a.len() == 9
    {
        color.xyz_to_cam_a = Some([[a[0], a[1], a[2]], [a[3], a[4], a[5]], [a[6], a[7], a[8]]]);
    }

    let wb = raw.wb_coeffs;
    let as_shot_wb = if wb[0].is_nan() || wb[1] <= 0.0 {
        color.wb_for_temp_tint(5500.0, 0.0)
    } else {
        [wb[0] / wb[1], 1.0, wb[2] / wb[1]]
    };

    let mut info = CaptureInfo { make: raw.clean_make.clone(), model: raw.clean_model.clone(), ..Default::default() };
    let mut orientation = Orientation::from_exif(raw.orientation as u16 + 1);
    if let Some(md) = md {
        let ex = &md.exif;
        info.iso = ex.iso_speed_ratings.map(u32::from).or(ex.iso_speed).or(ex.recommended_exposure_index);
        info.exposure_time = ex.exposure_time.filter(|r| r.d != 0 && r.n != 0).map(|r| {
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
    // The size of the developed image: default crop (or active area), oriented.
    let (w, h) = image_size(raw);
    (info.width, info.height) = if orientation.swaps_dimensions() { (h, w) } else { (w, h) };
    Ok(RawMeta { path: path.to_path_buf(), info, color, as_shot_wb, orientation, hdr: false, lens: None, lens_corrected: false })
}

/// Size of the image [`load_full`] returns, before orientation: the default crop, else the
/// active area, else the whole sensor.
fn image_size(raw: &RawImage) -> (usize, usize) {
    raw.crop_area.or(raw.active_area).map_or((raw.width, raw.height), |r| (r.d.w, r.d.h))
}

/// Scales to 0..1 with the black level subtracted. Skips data that already is: rawler would
/// copy float data for nothing.
fn scale(raw: &mut RawImage) -> Result<()> {
    let scaled = matches!(raw.data, RawImageData::Float(_)) && raw.blacklevel.levels.iter().all(|l| l.n == 0) && raw.whitelevel.0.iter().all(|w| *w == 1);
    if !scaled {
        raw.apply_scaling().map_err(|e| anyhow!("Skalierung fehlgeschlagen: {e}"))?;
    }
    Ok(())
}

/// Reads only the metadata, without decoding pixel data.
pub fn read_meta(path: &Path) -> Result<RawMeta> {
    Ok(decode_ext(path, true)?.meta)
}

/// Fast preview: half-size "superpixel" demosaic, then box-downscaled so the
/// long edge is at most `max_edge`. Returns camera-space linear RGB, oriented.
pub fn load_preview(path: &Path, max_edge: usize) -> Result<(RawMeta, LinearImage)> {
    preview_of(decode(path)?, max_edge)
}

/// [`load_preview`] that keeps the decoded raw data for a [`load_full`] of the same image soon
/// after (opening an image): decoding is a good part of loading.
pub fn load_preview_and_keep(path: &Path, max_edge: usize) -> Result<(RawMeta, LinearImage)> {
    let decoded = decode(path)?;
    let bytes = match &decoded.raw.data {
        RawImageData::Integer(d) => d.len() * 2,
        RawImageData::Float(d) => d.len() * 4,
    };
    let key = kept_key(path).filter(|_| bytes <= KEEP_MAX_BYTES);
    *KEPT.lock().unwrap() = key.map(|k| (k, decoded.clone()));
    preview_of(decoded, max_edge)
}

fn preview_of(Decoded { mut raw, meta }: Decoded, max_edge: usize) -> Result<(RawMeta, LinearImage)> {
    scale(&mut raw)?;
    let img = match (&raw.photometric, raw.cpp) {
        (RawPhotometricInterpretation::Cfa(cfg), 1) if let Some(k) = superpixel_size(&cfg.cfa) && raw.fuji_rotation_width.is_none() => {
            superpixel(&raw, &cfg.cfa, k)?
        }
        // Other layouts: full demosaic (one Markesteijn pass is enough for a preview).
        (RawPhotometricInterpretation::Cfa(_), 1) => develop_full(raw, XTransQuality::Fast)?,
        // Demosaiced (linear DNG): the same area as `load_full`.
        (_, 3) => {
            let RawImageData::Float(d) = &raw.data else { bail!("Unerwartetes Datenformat") };
            rgb_area(d, raw.width, raw.height, raw.crop_area.or(raw.active_area))?
        }
        _ => bail!("Dieses Sensorlayout wird noch nicht unterstützt"),
    };
    let img = img.fit_within(max_edge).oriented(meta.orientation);
    Ok((meta, img))
}

/// The `area` (whole image if `None`) of interleaved RGB data `width` x `height`.
fn rgb_area(d: &[f32], width: usize, height: usize, area: Option<Rect>) -> Result<LinearImage> {
    let (x0, y0, w, h) = area.map_or((0, 0, width, height), |r| (r.p.x, r.p.y, r.d.w, r.d.h));
    anyhow::ensure!(x0 + w <= width && y0 + h <= height && d.len() >= width * height * 3, "Bildbereich ungültig");
    let px = d.as_chunks::<3>().0;
    Ok(LinearImage::new(w, h, (0..h).flat_map(|y| px[(y0 + y) * width + x0..][..w].iter().map(|c| [c[0], c[1], c[2]])).collect()))
}

/// Full-resolution demosaic (PPG) in camera space, oriented.
pub fn load_full(path: &Path) -> Result<(RawMeta, LinearImage)> {
    let kept = {
        let mut kept = KEPT.lock().unwrap();
        match (kept.as_ref(), kept_key(path)) {
            (Some((k, _)), Some(key)) if *k == key => kept.take().map(|(_, d)| d),
            _ => None,
        }
    };
    let Decoded { raw, meta } = match kept {
        Some(d) => d,
        None => decode(path)?,
    };
    let img = develop_full(raw, XTransQuality::Best)?.oriented(meta.orientation);
    Ok((meta, img))
}

/// Passes of the Markesteijn demosaic for X-Trans sensors.
#[derive(Clone, Copy)]
enum XTransQuality {
    Fast = 1,
    Best = 3,
}

fn develop_full(raw: RawImage, xtrans: XTransQuality) -> Result<LinearImage> {
    let raw = match develop_xtrans(raw, xtrans)? {
        Ok(img) => return Ok(img),
        Err(raw) => raw,
    };
    let raw = match develop_bayer(raw)? {
        Ok(img) => return Ok(img),
        Err(raw) => raw,
    };
    let dev = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::CropDefault,
    ]);
    match dev.develop_intermediate(&raw).map_err(|e| anyhow!("Entwicklung fehlgeschlagen: {e}"))? {
        Intermediate::ThreeColor(px) => Ok(LinearImage::new(px.width, px.height, px.data)),
        Intermediate::Monochrome(px) => {
            Ok(LinearImage::new(px.width, px.height, px.data.into_iter().map(|v| [v, v, v]).collect()))
        }
        Intermediate::FourColor(_) => bail!("Kameras mit 4-Farb-Sensor werden noch nicht unterstützt"),
    }
}

/// Bayer sensors with AHD (see `ahd`) instead of rawler's PPG, which leaves zipper edges and
/// maze patterns along fine lines. Every other sensor is handed back.
fn develop_bayer(mut raw: RawImage) -> Result<Result<LinearImage, RawImage>> {
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else { return Ok(Err(raw)) };
    if raw.cpp != 1
        || config.sensor != rawler::imgop::sensor::SensorType::Bayer
        || !config.cfa.is_rgb()
        || config.cfa.width != 2
        || config.cfa.height != 2
        || raw.fuji_rotation_width.is_some()
    {
        return Ok(Err(raw));
    }
    let config = config.clone();
    scale(&mut raw)?;
    let RawImageData::Float(data) = std::mem::replace(&mut raw.data, RawImageData::Float(Vec::new())) else { bail!("Unerwartetes Datenformat") };
    let pixels = rawler::pixarray::PixF32::new_with(data, raw.width, raw.height);
    let roi = raw.active_area.unwrap_or(pixels.rect());
    let input = pixels.into_crop(roi);
    let mut pattern = [[0usize; 2]; 2];
    for (dy, row) in pattern.iter_mut().enumerate() {
        for (dx, c) in row.iter_mut().enumerate() {
            *c = match config.cfa.cfa_color_at(roi.p.y + dy, roi.p.x + dx) {
                CFAColor::RED => 0,
                CFAColor::GREEN => 1,
                _ => 2,
            };
        }
    }
    // As-shot white balance relative to green (only while interpolating).
    let wb = raw.wb_coeffs;
    let wb = if wb[1].is_finite() && wb[1] > 0.0 { [wb[0] / wb[1], 1.0, wb[2] / wb[1]] } else { [1.0; 3] };
    let mosaic = crate::ahd::Mosaic { data: &input.data, width: input.width, height: input.height, pattern };
    let mut rgb = rawler::pixarray::Color2D::<f32, 3>::new_with(crate::ahd::demosaic(&mosaic, wb), input.width, input.height);
    if let Some(mut crop) = raw.crop_area.or(raw.active_area) {
        if let Some(active) = &raw.active_area {
            crop = crop.adapt(active);
        }
        if crop.d != rgb.dim() {
            rgb = rgb.crop(crop);
        }
    }
    Ok(Ok(LinearImage::new(rgb.width, rgb.height, rgb.data)))
}

/// The scaled (black level subtracted, 0..1) photosites of a Bayer sensor within the area that
/// [`load_full`] returns, not oriented, for networks that work on the mosaic itself.
pub struct BayerMosaic {
    /// `width * height` photosites of the default crop.
    pub data: Vec<f32>,
    pub width: usize,
    pub height: usize,
    /// Color (0 red, 1 green, 2 blue) of the 2×2 pattern at the crop's corner.
    pub pattern: [[usize; 2]; 2],
}

impl BayerMosaic {
    /// Offset of the first red photosite, so that a 2×2 block from there is red, green, green, blue.
    pub fn rggb_offset(&self) -> Option<(usize, usize)> {
        (0..2).flat_map(|dy| (0..2).map(move |dx| (dx, dy))).find(|&(dx, dy)| {
            let at = |x: usize, y: usize| self.pattern[(dy + y) % 2][(dx + x) % 2];
            at(0, 0) == 0 && at(1, 0) == 1 && at(0, 1) == 1 && at(1, 1) == 2
        })
    }
}

/// The Bayer mosaic of a raw file (`None` for other sensors and for demosaiced data).
pub fn load_bayer(path: &Path) -> Result<Option<(RawMeta, BayerMosaic)>> {
    let Decoded { mut raw, meta } = decode(path)?;
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else { return Ok(None) };
    if raw.cpp != 1
        || config.sensor != rawler::imgop::sensor::SensorType::Bayer
        || !config.cfa.is_rgb()
        || config.cfa.width != 2
        || config.cfa.height != 2
        || raw.fuji_rotation_width.is_some()
    {
        return Ok(None);
    }
    let cfa = config.cfa.clone();
    raw.apply_scaling().map_err(|e| anyhow!("Skalierung fehlgeschlagen: {e}"))?;
    let RawImageData::Float(data) = &raw.data else { bail!("Unerwartetes Datenformat") };
    // The same area as the demosaic in `develop_bayer`: the default crop (absolute coordinates).
    let crop = raw.crop_area.or(raw.active_area).unwrap_or(rawler::imgop::Rect::new(
        rawler::imgop::Point::new(0, 0),
        rawler::imgop::Dim2::new(raw.width, raw.height),
    ));
    let (x0, y0, w, h) = (crop.p.x, crop.p.y, crop.d.w, crop.d.h);
    anyhow::ensure!(x0 + w <= raw.width && y0 + h <= raw.height && data.len() >= raw.width * raw.height, "Bildbereich ungültig");
    let mut pattern = [[0usize; 2]; 2];
    for (dy, row) in pattern.iter_mut().enumerate() {
        for (dx, c) in row.iter_mut().enumerate() {
            *c = match cfa.cfa_color_at(y0 + dy, x0 + dx) {
                CFAColor::RED => 0,
                CFAColor::GREEN => 1,
                _ => 2,
            };
        }
    }
    let out = (0..h).flat_map(|y| data[(y0 + y) * raw.width + x0..(y0 + y) * raw.width + x0 + w].iter().copied()).collect();
    Ok(Some((meta, BayerMosaic { data: out, width: w, height: h, pattern })))
}

/// X-Trans sensors with Markesteijn's demosaic instead of rawler's bilinear default, which
/// leaves color moiré and soft detail. Every other sensor is handed back.
fn develop_xtrans(mut raw: RawImage, quality: XTransQuality) -> Result<Result<LinearImage, RawImage>> {
    use rawler::imgop::sensor::Demosaic;
    use rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic;
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else { return Ok(Err(raw)) };
    // Rotated (SuperCCD) layouts stay with rawler.
    if raw.cpp != 1 || config.sensor != rawler::imgop::sensor::SensorType::Xtrans || !config.cfa.is_rgb() || raw.fuji_rotation_width.is_some() {
        return Ok(Err(raw));
    }
    let config = config.clone();
    scale(&mut raw)?;
    let RawImageData::Float(data) = std::mem::replace(&mut raw.data, RawImageData::Float(Vec::new())) else { bail!("Unerwartetes Datenformat") };
    let pixels = rawler::pixarray::PixF32::new_with(data, raw.width, raw.height);
    let roi = raw.active_area.unwrap_or(pixels.rect());
    let mut rgb = XTransMarkesteijnDemosaic::new(quality as usize).demosaic(&pixels, &config.cfa, &config.colors, roi);
    // Default crop, relative to the active area the demosaic already cut out.
    if let Some(mut crop) = raw.crop_area.or(raw.active_area) {
        if let Some(active) = &raw.active_area {
            crop = crop.adapt(active);
        }
        if crop.d != rgb.dim() {
            rgb = rgb.crop(crop);
        }
    }
    Ok(Ok(LinearImage::new(rgb.width, rgb.height, rgb.data)))
}

/// Block size for [`superpixel`]: 2 for Bayer, 3 for X-Trans (each 3×3 block of its 6×6 pattern
/// holds all three colors); `None` for other layouts.
fn superpixel_size(cfa: &rawler::CFA) -> Option<usize> {
    let k = match (cfa.width, cfa.height) {
        (2, 2) => 2,
        (6, 6) => 3,
        _ => return None,
    };
    let all_colors = (0..cfa.height / k).all(|by| {
        (0..cfa.width / k).all(|bx| {
            let colors: Vec<CFAColor> = (0..k * k).map(|i| cfa.cfa_color_at(by * k + i / k, bx * k + i % k)).collect();
            [CFAColor::RED, CFAColor::GREEN, CFAColor::BLUE].iter().all(|c| colors.contains(c))
                && colors.iter().all(|c| matches!(*c, CFAColor::RED | CFAColor::GREEN | CFAColor::BLUE))
        })
    });
    (cfa.is_rgb() && all_colors).then_some(k)
}

/// Fast preview without demosaic: each k×k block of the mosaic becomes one pixel holding the
/// mean of its red, green and blue photosites.
fn superpixel(raw: &RawImage, cfa: &rawler::CFA, k: usize) -> Result<LinearImage> {
    let RawImageData::Float(data) = &raw.data else { bail!("Unerwartetes Datenformat") };
    let area = raw.crop_area.or(raw.active_area);
    let (x0, y0, w, h) = match area {
        Some(r) => (r.p.x, r.p.y, r.d.w, r.d.h),
        None => (0, 0, raw.width, raw.height),
    };
    anyhow::ensure!(x0 + w <= raw.width && y0 + h <= raw.height && data.len() >= raw.width * raw.height, "Bildbereich ungültig");
    let (ow, oh) = (w / k, h / k);
    let stride = raw.width;
    // Channel of every position of one repetition of the pattern, starting at the area's corner.
    let (pw, ph) = (cfa.width, cfa.height);
    let chan: Vec<usize> = (0..ph * pw)
        .map(|i| match cfa.cfa_color_at(y0 + i / pw, x0 + i % pw) {
            CFAColor::RED => 0,
            CFAColor::GREEN => 1,
            _ => 2,
        })
        .collect();
    let mut out = vec![[0.0f32; 3]; ow * oh];
    out.par_chunks_mut(ow.max(1)).enumerate().for_each(|(oy, line)| {
        let y = y0 + oy * k;
        for (ox, px) in line.iter_mut().enumerate() {
            let x = x0 + ox * k;
            let mut acc = [0.0f32; 3];
            let mut cnt = [0.0f32; 3];
            for dy in 0..k {
                let row = (oy * k + dy) % ph * pw;
                for dx in 0..k {
                    let c = chan[row + (ox * k + dx) % pw];
                    acc[c] += data[(y + dy) * stride + x + dx];
                    cnt[c] += 1.0;
                }
            }
            *px = [acc[0] / cnt[0].max(1.0), acc[1] / cnt[1].max(1.0), acc[2] / cnt[2].max(1.0)];
        }
    });
    Ok(LinearImage::new(ow, oh, out))
}

/// Whether the file is a DNG that Silberkorn wrote (merged, stacked, denoised): its embedded
/// preview is Silberkorn's rendering, not the camera's.
pub fn written_by_silberkorn(path: &Path) -> bool {
    let file = crate::variant::file_of(path);
    if !file.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng")) {
        return false;
    }
    let Ok(source) = RawSource::new(&file) else { return false };
    let Ok(decoder) = rawler::get_decoder(&source) else { return false };
    let root = decoder.ifd(rawler::decoders::WellKnownIFD::Root).ok().flatten();
    let software = root.as_deref().and_then(|ifd| ifd.get_entry(305u16)).and_then(|e| e.value.as_string().cloned());
    software.is_some_and(|s| s.starts_with("Silberkorn"))
}

/// Embedded JPEG preview (fast thumbnails), already oriented. Reads only the JPEG where the
/// file's structure allows it (see `raw_preview`), else decodes the file's metadata with rawler.
pub fn embedded_preview(path: &Path) -> Result<::image::DynamicImage> {
    embedded_preview_at_least(path, usize::MAX)
}

/// [`embedded_preview`] from the smallest embedded JPEG with at least `min_edge` pixels on its
/// longer side, where the file holds several (much faster for thumbnails).
pub fn embedded_preview_at_least(path: &Path, min_edge: usize) -> Result<::image::DynamicImage> {
    let file = crate::variant::file_of(path);
    if let Some((jpeg, orientation)) = crate::raw_preview::read_at_least(&file, min_edge)
        && let Ok(img) = ::image::load_from_memory_with_format(&jpeg, ::image::ImageFormat::Jpeg)
    {
        return Ok(Orientation::from_exif(orientation).apply_dynamic(img));
    }
    embedded_preview_rawler(&file)
}

/// [`embedded_preview`] through rawler (reads the whole file).
fn embedded_preview_rawler(path: &Path) -> Result<::image::DynamicImage> {
    let source = RawSource::new(path)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_outside_the_sensor_are_dropped() {
        let r = |x, y, w, h| Rect::new(rawler::imgop::Point::new(x, y), rawler::imgop::Dim2::new(w, h));
        let (active, crop) = (r(8, 8, 100, 80), r(10, 10, 90, 70));
        assert_eq!(valid_areas(120, 100, Some(active), Some(crop)), (Some(active), Some(crop)));
        // Active area beyond the image, crop beyond the active area or larger than it.
        assert_eq!(valid_areas(100, 100, Some(active), None), (None, None));
        assert_eq!(valid_areas(120, 100, Some(active), Some(r(4, 10, 90, 70))), (Some(active), None));
        assert_eq!(valid_areas(120, 100, Some(active), Some(r(10, 10, 120, 70))), (Some(active), None));
        assert_eq!(valid_areas(120, 100, None, Some(r(usize::MAX, 0, 2, 2))), (None, None));
    }

    #[test]
    fn rgb_area_cuts_out_the_crop() {
        let d: Vec<f32> = (0..4 * 3 * 3).map(|i| i as f32).collect();
        let r = Rect::new(rawler::imgop::Point::new(1, 1), rawler::imgop::Dim2::new(2, 2));
        let img = rgb_area(&d, 4, 3, Some(r)).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.data, [[15.0, 16.0, 17.0], [18.0, 19.0, 20.0], [27.0, 28.0, 29.0], [30.0, 31.0, 32.0]]);
        assert_eq!(rgb_area(&d, 4, 3, None).unwrap().data.len(), 12);
        assert!(rgb_area(&d, 4, 3, Some(Rect::new(rawler::imgop::Point::new(3, 0), rawler::imgop::Dim2::new(2, 1)))).is_err());
    }

    /// The size in the capture info is that of the developed image (on the sample raws in
    /// `samples/`, skipped without them).
    #[test]
    fn capture_size_is_the_developed_size() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
        for name in ["_DSC0010.ARW", "DSC09192.ARW"] {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let meta = read_meta(&path).unwrap();
            let (_, img) = load_full(&path).unwrap();
            assert_eq!((meta.info.width, meta.info.height), (img.width, img.height), "{name}");
            assert!(!written_by_silberkorn(&path));
        }
    }

    /// The full image from the raw data kept by `load_preview_and_keep` is that of decoding
    /// again, and it is taken only once (on a sample raw, skipped without it).
    #[test]
    fn kept_raw_gives_the_same_full_image() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0010.ARW");
        if !path.exists() {
            return;
        }
        let (_, small) = load_preview_and_keep(&path, 256).unwrap();
        assert_eq!(small.width.max(small.height), 256);
        assert!(KEPT.lock().unwrap().is_some());
        let (_, kept) = load_full(&path).unwrap();
        assert!(KEPT.lock().unwrap().is_none());
        let (_, fresh) = load_full(&path).unwrap();
        assert_eq!((kept.width, kept.height), (fresh.width, fresh.height));
        assert!(kept.data == fresh.data);
    }

    /// The fast preview reader finds the same embedded image as rawler (on the sample raws in
    /// `samples/`, skipped without them).
    #[test]
    fn fast_embedded_preview_matches_rawler() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
        let files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| !x.eq_ignore_ascii_case("json")))
            .collect();
        for f in files {
            let Some((jpeg, orientation)) = crate::raw_preview::read(&f) else { continue };
            let fast = Orientation::from_exif(orientation).apply_dynamic(::image::load_from_memory(&jpeg).unwrap());
            let slow = embedded_preview_rawler(&f).unwrap();
            assert_eq!((fast.width(), fast.height()), (slow.width(), slow.height()), "{}", f.display());
        }
    }
}
