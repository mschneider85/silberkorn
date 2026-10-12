//! Finds sensor dust and skin blemishes: small, round spots that are darker than their
//! surroundings on all sides, in otherwise calm areas (sky, walls, skin). They become heal spots.
//!
//! On the logarithm of the brightness (dust darkens by a factor, whatever the brightness), a
//! difference of two blurs at several sizes measures how much darker a spot is than its ring. A
//! spot counts if that clearly exceeds the local texture, the ring is brighter in every direction
//! (not an edge or a line) and it is the strongest response nearby.

use rayon::prelude::*;

use crate::detail::{Plane, gaussian_blur};
use crate::image::LinearImage;

/// A spot found: center in normalized source coordinates, radius relative to the long edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub center: [f32; 2],
    pub radius: f32,
    /// How clearly it stands out (for sorting).
    pub score: f32,
}

/// Blob sizes (Gaussian sigma in pixels of the analysed image, at most 1600 px long).
const SCALES: [f32; 4] = [1.2, 2.0, 3.2, 5.0];
/// Long edge of the analysed image.
const EDGE: usize = 1600;

/// Spots in `img` (camera RGB as the preview, any size; `wb` its white balance), the clearest
/// first, at most `max`. `sensitivity` 0..1: higher finds fainter spots.
pub fn find(img: &LinearImage, wb: [f32; 3], sensitivity: f32, max: usize) -> Vec<Found> {
    if img.width < 32 || img.height < 32 {
        return Vec::new();
    }
    let k = EDGE as f32 / img.width.max(img.height) as f32;
    let small = if k < 1.0 { img.resize_area(((img.width as f32 * k).round() as usize).max(1), ((img.height as f32 * k).round() as usize).max(1)) } else { img.clone() };
    let (w, h) = (small.width, small.height);
    let lum = Plane::new(w, h, small.data.par_iter().map(|p| (0.25 * p[0] * wb[0] + 0.5 * p[1] * wb[1] + 0.25 * p[2] * wb[2]).max(1e-4).ln()).collect());
    // Contrast needed (log units, about percent darker) and how far above the texture.
    let s = sensitivity.clamp(0.0, 1.0);
    let min_contrast = 0.03 - 0.02 * s;
    let min_ratio = 7.0 - 3.0 * s;

    let mut candidates: Vec<(usize, usize, f32, f32, f32)> = Vec::new(); // x, y, sigma, contrast, score
    for &sigma in &SCALES {
        let inner = gaussian_blur(&lum, sigma);
        let outer = gaussian_blur(&lum, sigma * 2.5);
        let dog = outer.zip(&inner, |o, i| o - i);
        // Texture: the typical response of this size around here (over a window wide enough that
        // the spot itself hardly counts); high in foliage, low in the sky even with noise.
        let texture = texture_of(&dog, sigma);
        let margin = (sigma * 4.0).ceil() as usize + 2;
        let ring_r = sigma * 2.4;
        let found: Vec<(usize, usize, f32, f32, f32)> = (margin..h.saturating_sub(margin))
            .into_par_iter()
            .flat_map_iter(|y| {
                let (dog, inner, texture) = (&dog, &inner, &texture);
                (margin..w.saturating_sub(margin)).filter_map(move |x| {
                    let i = y * w + x;
                    let d = dog.data[i];
                    if d < min_contrast || d > 0.7 {
                        return None;
                    }
                    // The strongest response in its neighbourhood.
                    let r = sigma.ceil() as usize;
                    for yy in y - r..=y + r {
                        for xx in x - r..=x + r {
                            if dog.data[yy * w + xx] > d {
                                return None;
                            }
                        }
                    }
                    let score = d / (texture.data[i] + 0.002);
                    if score < min_ratio {
                        return None;
                    }
                    // Brighter on every side: a spot, not an edge or a line.
                    let c = inner.data[i];
                    let all_around = (0..12).all(|a| {
                        let t = a as f32 / 12.0 * std::f32::consts::TAU;
                        let (sx, sy) = ((x as f32 + t.cos() * ring_r).round() as usize, (y as f32 + t.sin() * ring_r).round() as usize);
                        inner.data[sy.min(h - 1) * w + sx.min(w - 1)] - c > d * 0.45
                    });
                    all_around.then_some((x, y, sigma, d, score))
                })
            })
            .collect();
        candidates.extend(found);
    }
    // Across sizes: the clearest wins; others overlapping it are the same spot.
    candidates.sort_by(|a, b| b.4.total_cmp(&a.4));
    let mut kept: Vec<(usize, usize, f32, f32, f32)> = Vec::new();
    for c in candidates {
        let overlaps = kept.iter().any(|k| {
            let dist = ((c.0 as f32 - k.0 as f32).powi(2) + (c.1 as f32 - k.1 as f32).powi(2)).sqrt();
            dist < 2.5 * (c.2 + k.2)
        });
        if !overlaps {
            kept.push(c);
        }
        if kept.len() >= max {
            break;
        }
    }
    let long = w.max(h) as f32;
    kept.into_iter()
        .map(|(x, y, sigma, _, score)| Found {
            center: [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32],
            // The heal spot covers the whole darkened area with a soft edge.
            radius: sigma * 2.6 / long,
            score,
        })
        .collect()
}

/// The typical response of the blob filter around each pixel: the mean magnitude over a window
/// much larger than a spot (so a spot hardly raises its own surroundings, while texture with
/// many strong responses does), scaled like a standard deviation.
fn texture_of(dog: &Plane, sigma: f32) -> Plane {
    gaussian_blur(&dog.map(f32::abs), sigma * 8.0).map(|v| v * 1.25)
}

/// Heal spots for the spots found that no existing spot covers yet.
pub fn heal_spots(src: &LinearImage, found: &[Found], existing: &[crate::params::Spot]) -> Vec<crate::params::Spot> {
    let aspect_x = src.width as f32 / src.width.max(src.height) as f32;
    let aspect_y = src.height as f32 / src.width.max(src.height) as f32;
    // Distance in units of the long edge.
    let dist = |a: [f32; 2], b: [f32; 2]| (((a[0] - b[0]) * aspect_x).powi(2) + ((a[1] - b[1]) * aspect_y).powi(2)).sqrt();
    found
        .iter()
        .filter(|f| !existing.iter().any(|s| dist(s.target, f.center) < s.radius + f.radius))
        .map(|f| crate::params::Spot {
            mode: crate::params::SpotMode::Heal,
            target: f.center,
            source: crate::mask::auto_source(src, f.center, f.radius),
            path: Vec::new(),
            radius: f.radius,
            feather: 0.5,
            opacity: 1.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sky-like gradient with noise, three faint dust spots, a horizon edge, a thin dark
    /// line (a cable) and a textured lower part (foliage).
    fn scene() -> (LinearImage, Vec<[f32; 2]>) {
        let (w, h) = (1200, 800);
        let dust = [(300.0, 150.0, 6.0, 0.10), (700.0, 250.0, 10.0, 0.07), (1000.0, 120.0, 4.0, 0.12)];
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let n = crate::detail::hash_noise(x as i64, y as i64, 7) * 0.006;
                let mut v = if y < 450.0 {
                    0.5 + 0.2 * (1.0 - y / 450.0)
                } else {
                    // Foliage: strong texture.
                    0.15 + 0.1 * crate::detail::hash_noise((x / 3.0) as i64, (y / 3.0) as i64, 9)
                };
                for (cx, cy, r, depth) in dust {
                    let d2 = ((x - cx).powi(2) + (y - cy).powi(2)) / (r * r);
                    v *= 1.0 - depth * (-d2).exp();
                }
                // A cable across the sky.
                if (y - (350.0 + x * 0.05)).abs() < 1.0 {
                    v *= 0.6;
                }
                [v + n, v + n, v + n]
            })
            .collect();
        (LinearImage::new(w, h, data), dust.iter().map(|d| [d.0 / w as f32, d.1 / h as f32]).collect())
    }

    #[test]
    fn finds_dust_but_not_edges_lines_or_texture() {
        let (img, dust) = scene();
        let found = find(&img, [1.0; 3], 0.5, 50);
        for d in &dust {
            assert!(found.iter().any(|f| (f.center[0] - d[0]).abs() < 0.01 && (f.center[1] - d[1]).abs() < 0.015), "{d:?} nicht gefunden: {found:?}");
        }
        assert_eq!(found.len(), dust.len(), "nur der Staub: {found:?}");
        // Radii in proportion to the spots (relative to the long edge, 1200 px).
        let big = found.iter().find(|f| (f.center[0] - 700.0 / 1200.0).abs() < 0.01).unwrap();
        let small = found.iter().find(|f| (f.center[0] - 1000.0 / 1200.0).abs() < 0.01).unwrap();
        assert!(big.radius > small.radius && big.radius * 1200.0 > 10.0 && big.radius * 1200.0 < 40.0, "{} {}", big.radius, small.radius);
    }

    #[test]
    fn spots_become_heal_spots_once() {
        let (img, _) = scene();
        let found = find(&img, [1.0; 3], 0.5, 50);
        let spots = heal_spots(&img, &found, &[]);
        assert_eq!(spots.len(), found.len());
        assert!(spots.iter().all(|s| s.mode == crate::params::SpotMode::Heal && s.source != s.target));
        // Searching again adds nothing.
        assert!(heal_spots(&img, &found, &spots).is_empty());
    }
}
