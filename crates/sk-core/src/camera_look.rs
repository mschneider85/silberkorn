//! The tone of the camera's own JPEG, so that a raw file starts out looking like it (a
//! tone curve fitted per image from the JPEG embedded in the file).
//!
//! The brightness distributions of the raw data (scene-linear luminance before exposure) and of
//! the JPEG (display-linear luminance) are matched quantile by quantile: the n-th darkest raw
//! value maps to the n-th darkest JPEG value. That needs no pixel-exact alignment (the camera may
//! have corrected distortion, or crop the preview slightly), and it captures the camera's curve
//! with its exposure, its highlight roll-off and, as far as a global curve can, its dynamic range
//! optimization.

use crate::color::srgb_decode;
use crate::image::LinearImage;
use crate::params::{Adjustments, BaseCurve, CameraCurve};
use crate::pipeline::Processor;
use crate::raw::RawMeta;

/// Spacing of the curve's points in EV.
const STEP_EV: f32 = 0.5;

static FOR_NEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether never edited images get the camera's look (the user's setting; off unless set).
pub fn for_new() -> bool {
    FOR_NEW.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_for_new(on: bool) {
    FOR_NEW.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether an image with these settings gets the camera's look when it is opened, exported or
/// adjusted automatically: never edited (with `for_new`, the user's choice for new images), or
/// set to the camera's curve without having one yet (pasted, from a style).
pub fn wanted(adj: &Adjustments, for_new: bool) -> bool {
    match adj.base_curve {
        BaseCurve::Camera => adj.camera_curve.is_none(),
        _ => for_new && adj.clone().normalized() == Adjustments::default(),
    }
}

/// `adj` with the camera's look, if [`wanted`] and the file has a usable JPEG.
pub fn apply_if_wanted(meta: &RawMeta, preview: &LinearImage, adj: &Adjustments, for_new: bool) -> Option<Adjustments> {
    if !wanted(adj, for_new) {
        return None;
    }
    let curve = fit_cached(meta, preview)?;
    Some(Adjustments { base_curve: BaseCurve::Camera, camera_curve: Some(curve), ..adj.clone() })
}

/// Curves fitted recently, by file (prefetching fits ahead, opening then finds it here).
type Fitted = (std::path::PathBuf, Option<CameraCurve>);
static CACHE: std::sync::Mutex<Vec<Fitted>> = std::sync::Mutex::new(Vec::new());

/// [`fit_from_file`], remembered for the last few files.
pub fn fit_cached(meta: &RawMeta, preview: &LinearImage) -> Option<CameraCurve> {
    if let Some((_, c)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, _)| *p == meta.path) {
        return c.clone();
    }
    let curve = fit_from_file(meta, preview);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 32 {
        cache.remove(0);
    }
    cache.push((meta.path.clone(), curve.clone()));
    curve
}

/// The camera's tone curve for this image, from the JPEG embedded in its raw file. `None` without
/// one, for Silberkorn's own DNGs (merged, stacked, denoised: their preview is Silberkorn's), and
/// when the two don't fit together.
pub fn fit_from_file(meta: &RawMeta, preview: &LinearImage) -> Option<CameraCurve> {
    if meta.hdr || crate::raw::written_by_silberkorn(&meta.path) {
        return None;
    }
    let jpeg = crate::raw::embedded_preview(&meta.path).ok()?.thumbnail(640, 640).to_rgb8();
    let (w, h) = (jpeg.width() as usize, jpeg.height() as usize);
    let rgb: Vec<[f32; 3]> = jpeg.as_raw().as_chunks::<3>().0.iter().map(|p| p.map(|v| v as f32 / 255.0)).collect();
    fit(meta, preview, &LinearImage::new(w, h, rgb))
}

/// The tone curve that maps the raw `preview` (camera RGB) to the brightness of `jpeg` (sRGB
/// encoded, 0..1).
pub fn fit(meta: &RawMeta, preview: &LinearImage, jpeg: &LinearImage) -> Option<CameraCurve> {
    let jpeg = without_black_bars(jpeg)?;
    // Scene luminance as the pipeline sees it at exposure 0, with the camera's white balance.
    let p = Processor::new(meta, &Adjustments::default());
    let step = (preview.data.len() / 150_000).max(1);
    let mut scene: Vec<f32> = preview.data.iter().step_by(step).map(|c| p.scene_luminance(*c)).collect();
    let step = (jpeg.data.len() / 150_000).max(1);
    let mut shown: Vec<f32> =
        jpeg.data.iter().step_by(step).map(|c| 0.2126 * srgb_decode(c[0]) + 0.7152 * srgb_decode(c[1]) + 0.0722 * srgb_decode(c[2])).collect();
    if scene.len() < 1000 || shown.len() < 1000 {
        return None;
    }
    scene.sort_unstable_by(|a, b| a.total_cmp(b));
    shown.sort_unstable_by(|a, b| a.total_cmp(b));
    let at = |v: &[f32], q: f32| v[((v.len() - 1) as f32 * q) as usize];

    // Pairs of equal quantiles, as (EV, display value); clipped and crushed ends say nothing.
    let mut pairs: Vec<[f32; 2]> = Vec::new();
    for i in 0..=400 {
        let q = 0.002 + 0.996 * i as f32 / 400.0;
        let (s, d) = (at(&scene, q), at(&shown, q));
        if s <= 1e-5 || !(0.003..0.97).contains(&d) {
            continue;
        }
        pairs.push([(s / 0.18).log2(), d]);
    }
    if pairs.len() < 50 {
        return None;
    }
    let (lo, hi) = (pairs[0][0], pairs[pairs.len() - 1][0]);
    if hi - lo < 3.0 {
        return None;
    }

    // Points every half stop: the mean display value of the pairs around each.
    let mut points: Vec<[f32; 2]> = Vec::new();
    let mut ev = (lo / STEP_EV).ceil() * STEP_EV;
    while ev <= hi {
        let near: Vec<f32> = pairs.iter().filter(|p| (p[0] - ev).abs() <= STEP_EV * 0.75).map(|p| p[1]).collect();
        if !near.is_empty() {
            points.push([ev, near.iter().sum::<f32>() / near.len() as f32]);
        }
        ev += STEP_EV;
    }
    // Strictly rising and below white.
    for i in 1..points.len() {
        points[i][1] = points[i][1].max(points[i - 1][1] + 1e-4);
    }
    points.retain(|p| p[1] < 0.995);
    // A JPEG without a range of brightness (or another picture) gives no curve.
    if points.len() < 6 || points[points.len() - 1][1] - points[0][1] < 0.1 {
        return None;
    }
    // The pipeline applies the curve to the color channels (keeping the hue), not to the
    // luminance, which comes out a little different for colored pixels: one pass through the
    // real pipeline corrects each point by how far its quantile still misses the JPEG.
    let mut curve = CameraCurve { points };
    let small = reduced(preview, 400);
    let adj = Adjustments { base_curve: BaseCurve::Camera, camera_curve: Some(curve.clone()), ..Default::default() };
    let r = Processor::new(meta, &adj).render_cpu(&small, None, None);
    let mut ours: Vec<f32> = r
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| 0.2126 * srgb_decode(c[0] as f32 / 255.0) + 0.7152 * srgb_decode(c[1] as f32 / 255.0) + 0.0722 * srgb_decode(c[2] as f32 / 255.0))
        .collect();
    ours.sort_unstable_by(|a, b| a.total_cmp(b));
    for p in &mut curve.points {
        let q = scene.partition_point(|v| *v < 0.18 * p[0].exp2()) as f32 / scene.len() as f32;
        let (want, got) = (at(&shown, q), at(&ours, q));
        if got > 0.003 && want > 0.003 {
            p[1] *= (want / got).clamp(0.8, 1.25);
        }
    }
    let points = &mut curve.points;
    for i in 1..points.len() {
        points[i][1] = points[i][1].max(points[i - 1][1] + 1e-4);
    }
    points.retain(|p| p[1] < 0.995);
    if points.len() < 6 {
        return None;
    }
    // A curve that maps middle grey far from where any camera would (a wrong preview, a
    // monochrome or very creative picture style) is not a camera tone.
    (0.03..0.6).contains(&curve.eval(0.18)).then_some(curve)
}

pub(crate) fn reduced(img: &LinearImage, edge: usize) -> LinearImage {
    let k = edge as f32 / img.width.max(img.height) as f32;
    if k >= 1.0 {
        return img.clone();
    }
    img.resize_area(((img.width as f32 * k).round() as usize).max(1), ((img.height as f32 * k).round() as usize).max(1))
}

/// `img` without rows and columns that are black throughout (previews in another aspect ratio
/// than the sensor).
fn without_black_bars(img: &LinearImage) -> Option<LinearImage> {
    let (w, h) = (img.width, img.height);
    let black = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]) <= 4.0 / 255.0;
    let row_black = |y: usize| img.data[y * w..(y + 1) * w].iter().all(black);
    let col_black = |x: usize| (0..h).all(|y| black(&img.data[y * w + x]));
    let (mut y0, mut y1, mut x0, mut x1) = (0, h, 0, w);
    while y0 < y1 && row_black(y0) {
        y0 += 1;
    }
    while y1 > y0 && row_black(y1 - 1) {
        y1 -= 1;
    }
    while x0 < x1 && col_black(x0) {
        x0 += 1;
    }
    while x1 > x0 && col_black(x1 - 1) {
        x1 -= 1;
    }
    if x1 - x0 < 64 || y1 - y0 < 64 {
        return None;
    }
    let data = (y0..y1).flat_map(|y| img.data[y * w + x0..y * w + x1].iter().copied()).collect();
    Some(LinearImage::new(x1 - x0, y1 - y0, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{CameraColor, srgb_encode};
    use crate::image::Orientation;
    use crate::raw::CaptureInfo;

    fn meta() -> RawMeta {
        let xyz_to_cam = crate::color::mat_inv(&crate::color::REC2020_TO_XYZ).unwrap();
        RawMeta {
            path: Default::default(),
            info: CaptureInfo { width: 300, height: 200, ..Default::default() },
            color: CameraColor::new(xyz_to_cam),
            as_shot_wb: [1.0; 3],
            orientation: Orientation::Normal,
            hdr: false,
            lens: None,
            lens_corrected: false, bayer: false,
        }
    }

    /// A scene spanning about 10 stops, neutral.
    fn scene() -> LinearImage {
        let (w, h) = (300, 200);
        LinearImage::new(w, h, (0..w * h).map(|i| [0.0008 * (1024f32).powf((i % w) as f32 / w as f32) * (0.9 + 0.1 * ((i / w) % 7) as f32 / 7.0); 3]).collect())
    }

    #[test]
    fn fitted_curve_reproduces_the_cameras_rendering() {
        let m = meta();
        let src = scene();
        let p = Processor::new(&m, &Adjustments::default());
        // The "camera": a brighter curve than Film Standard, as a JPEG with black bars on top.
        let camera = |y: f32| (y * 1.6 / (1.0 + y * 1.6)).powf(0.9);
        let (w, h) = (src.width, src.height + 20);
        let jpeg = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| if i < w * 10 || i >= w * (h - 10) { [0.0; 3] } else { [srgb_encode(camera(p.scene_luminance(src.data[i - w * 10]))); 3] })
                .collect(),
        );
        let curve = fit(&m, &src, &jpeg).expect("Kurve");
        for v in [0.02f32, 0.05, 0.18, 0.5, 1.0] {
            let (want, got) = (camera(v), curve.eval(v));
            assert!((want - got).abs() < 0.02 + want * 0.05, "{v}: {want} vs {got}");
        }
        // Developed with it, the image looks like the JPEG.
        let adj = Adjustments { base_curve: crate::params::BaseCurve::Camera, camera_curve: Some(curve), ..Default::default() };
        let r = Processor::new(&m, &adj).render_cpu(&src, None, None);
        let mean = |v: &mut dyn Iterator<Item = f32>| {
            let (s, n) = v.fold((0.0, 0), |(s, n), x| (s + x, n + 1));
            s / n as f32
        };
        let ours = mean(&mut r.rgba.as_chunks::<4>().0.iter().map(|p| p[1] as f32 / 255.0));
        let theirs = mean(&mut jpeg.data[w * 10..w * (h - 10)].iter().map(|p| p[1]));
        assert!((ours - theirs).abs() < 0.02, "{ours} vs {theirs}");
    }

    #[test]
    fn who_gets_the_camera_look() {
        let plain = Adjustments::default();
        assert!(wanted(&plain, true) && !wanted(&plain, false), "nie bearbeitet: nach Einstellung");
        let edited = Adjustments { exposure: 0.5, ..Default::default() };
        assert!(!wanted(&edited, true), "bearbeitet: nie");
        // Chosen (pasted, from a style) without a curve yet: always.
        let chosen = Adjustments { base_curve: BaseCurve::Camera, exposure: 0.5, ..Default::default() };
        assert!(wanted(&chosen, false));
        let fitted = Adjustments { camera_curve: Some(CameraCurve { points: vec![[0.0, 0.2], [1.0, 0.3]] }), ..chosen.clone() };
        assert!(!wanted(&fitted, true));
        // Pasting keeps the target's own curve.
        let mut pasted = chosen.clone();
        pasted.keep_geometry_from(&fitted);
        assert_eq!(pasted.camera_curve, fitted.camera_curve);
    }

    #[test]
    fn no_curve_without_a_matching_preview() {
        let m = meta();
        // A flat grey "preview" has no brightness range to fit.
        let flat = LinearImage::new(300, 200, vec![[0.5; 3]; 60_000]);
        assert!(fit(&m, &scene(), &flat).is_none());
    }

    #[test]
    fn curve_extends_smoothly_beyond_its_points() {
        let c = CameraCurve { points: (0..10).map(|i| [-4.0 + i as f32 * 0.5, 0.02 * 1.5f32.powi(i)]).collect() };
        let mut last = 0.0;
        for i in 0..200 {
            let v = 0.0001 * 1.07f32.powi(i);
            let y = c.eval(v);
            assert!(y >= last && y <= 1.0, "{v}: {y} after {last}");
            last = y;
        }
        assert!(c.eval(100.0) > 0.98);
    }
}
