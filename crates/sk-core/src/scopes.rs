//! Waveform and vectorscope of the displayed image (8-bit sRGB, as the histogram).
//!
//! - Waveform: for each column of the image (in [`WAVE_COLUMNS`] bands) how many pixels have
//!   which value, per channel. Tones keep their place in the picture: a bright sky stays on top
//!   at the left if it is at the left.
//! - Vectorscope: the colors on the Cb/Cr plane of BT.709 (from the gamma-encoded values, as in
//!   video): the angle is the hue, the distance from the middle the saturation.

use rayon::prelude::*;

pub const WAVE_COLUMNS: usize = 256;
pub const WAVE_LEVELS: usize = 128;
pub const VECTOR_SIZE: usize = 128;
/// Cb/Cr at the edge of the vectorscope (fully saturated red reaches 0.51).
pub const VECTOR_RANGE: f32 = 0.55;
/// Hue angle of skin tones on the vectorscope ("I line"), degrees counterclockwise from +Cb.
pub const SKIN_ANGLE: f32 = 123.0;

pub struct Scopes {
    /// Counts per channel, by level (0 = black, at the bottom) and column: `level * WAVE_COLUMNS + column`.
    pub waveform: Vec<[u32; 3]>,
    /// Pixels in each column band.
    pub per_column: Vec<u32>,
    /// Counts on the Cb/Cr plane: `row * VECTOR_SIZE + column`, row 0 at +Cr (top).
    pub vector: Vec<u32>,
    pub total: u32,
}

/// Cb and Cr of a gamma-encoded color (0..1 each).
pub fn chroma(rgb: [f32; 3]) -> [f32; 2] {
    let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    [(rgb[2] - y) / 1.8556, (rgb[0] - y) / 1.5748]
}

/// The vectorscope cell of a Cb/Cr pair (clamped to the edge).
pub fn vector_cell(c: [f32; 2]) -> (usize, usize) {
    let at = |v: f32| (((v / VECTOR_RANGE) * 0.5 + 0.5) * VECTOR_SIZE as f32).clamp(0.0, VECTOR_SIZE as f32 - 1.0) as usize;
    (at(c[0]), VECTOR_SIZE - 1 - at(c[1]))
}

/// Above this many pixels only every other pixel (in both directions) is measured: the scopes
/// have far fewer cells, and they look the same.
const FULL_PIXELS: usize = 1 << 20;

/// The vectorscope cell of an 8-bit color, in integers: Cb and Cr scaled by 10 000 × 255, mapped
/// with 24 fraction bits.
#[inline]
fn vector_cell_u8(r: u8, g: u8, b: u8) -> usize {
    const HALF: i32 = VECTOR_SIZE as i32 / 2;
    // HALF / (1.8556 · VECTOR_RANGE · 2 550 000) and HALF / (1.5748 · VECTOR_RANGE · 2 550 000), × 2²⁴.
    const KB: i64 = 413;
    const KR: i64 = 486;
    let (r, g, b) = (r as i64, g as i64, b as i64);
    let y = 2126 * r + 7152 * g + 722 * b;
    let x = (HALF + ((b * 10_000 - y) * KB >> 24) as i32).clamp(0, VECTOR_SIZE as i32 - 1) as usize;
    let up = (HALF + ((r * 10_000 - y) * KR >> 24) as i32).clamp(0, VECTOR_SIZE as i32 - 1) as usize;
    (VECTOR_SIZE - 1 - up) * VECTOR_SIZE + x
}

impl Scopes {
    /// Measures an image of `width` pixels per row; `rgb` gives the 8-bit sRGB of a pixel.
    pub fn measure<P: Sync>(pixels: &[P], width: usize, rgb: impl Fn(&P) -> [u8; 3] + Sync) -> Self {
        let width = width.max(1);
        let step = if pixels.len() > FULL_PIXELS { 2 } else { 1 };
        let column: Vec<u16> = (0..width).map(|x| (x * WAVE_COLUMNS / width) as u16).collect();
        let empty = || Self { waveform: vec![[0; 3]; WAVE_COLUMNS * WAVE_LEVELS], per_column: vec![0; WAVE_COLUMNS], vector: vec![0; VECTOR_SIZE * VECTOR_SIZE], total: 0 };
        // Bands of whole rows, so that the column of each pixel is known.
        let rows = (262_144 / width).max(1).next_multiple_of(step);
        pixels
            .par_chunks(width * rows)
            .fold(empty, |mut s, band| {
                for row in band.chunks(width).step_by(step) {
                    for (x, p) in row.iter().enumerate().step_by(step) {
                        let p = rgb(p);
                        let col = column[x] as usize;
                        for c in 0..3 {
                            s.waveform[(p[c] as usize * WAVE_LEVELS / 256) * WAVE_COLUMNS + col][c] += 1;
                        }
                        s.per_column[col] += 1;
                        s.vector[vector_cell_u8(p[0], p[1], p[2])] += 1;
                        s.total += 1;
                    }
                }
                s
            })
            .reduce(empty, |mut a, b| {
                for (x, y) in a.waveform.iter_mut().zip(&b.waveform) {
                    *x = [x[0] + y[0], x[1] + y[1], x[2] + y[2]];
                }
                for (x, y) in a.per_column.iter_mut().zip(&b.per_column) {
                    *x += y;
                }
                for (x, y) in a.vector.iter_mut().zip(&b.vector) {
                    *x += y;
                }
                a.total += b.total;
                a
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(w: usize, h: usize, color: impl Fn(usize, usize) -> [u8; 3]) -> Scopes {
        let pixels: Vec<[u8; 3]> = (0..w * h).map(|i| color(i % w, i / w)).collect();
        Scopes::measure(&pixels, w, |p| *p)
    }

    #[test]
    fn waveform_keeps_tones_in_their_column() {
        // Left half black, right half white.
        let (w, h) = (100, 30);
        let s = measure(w, h, |x, _| if x < 50 { [0; 3] } else { [255; 3] });
        assert_eq!(s.total, 3000);
        let at = |level: usize, col: usize| s.waveform[level * WAVE_COLUMNS + col];
        assert!(at(0, 10)[1] > 0 && at(WAVE_LEVELS - 1, 10)[1] == 0);
        assert!(at(WAVE_LEVELS - 1, 230)[1] > 0 && at(0, 230)[1] == 0);
        assert_eq!(s.per_column.iter().sum::<u32>(), 3000);
    }

    #[test]
    fn vectorscope_puts_gray_in_the_middle_and_hues_around_it() {
        let gray = measure(10, 10, |_, _| [128; 3]);
        let mid = VECTOR_SIZE / 2;
        let (x, y) = gray.vector.iter().enumerate().find(|(_, n)| **n > 0).map(|(i, _)| (i % VECTOR_SIZE, i / VECTOR_SIZE)).unwrap();
        assert!(x.abs_diff(mid) <= 1 && y.abs_diff(mid) <= 1);
        // Red up and left, blue right, skin tones between red and yellow on the I line.
        let (rx, ry) = vector_cell(chroma([1.0, 0.0, 0.0]));
        assert!(rx < mid && ry < mid / 4);
        let (bx, _) = vector_cell(chroma([0.0, 0.0, 1.0]));
        assert!(bx > mid + mid / 2);
        let skin = chroma([0.87, 0.65, 0.52]);
        let angle = skin[1].atan2(skin[0]).to_degrees();
        assert!((angle - SKIN_ANGLE).abs() < 12.0, "{angle}");
    }

    #[test]
    fn integer_cells_match_the_float_ones() {
        for c in [[0u8, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [222, 166, 133], [40, 90, 200], [128, 128, 128]] {
            let (x, y) = vector_cell(chroma(c.map(|v| v as f32 / 255.0)));
            let i = vector_cell_u8(c[0], c[1], c[2]);
            assert!((i % VECTOR_SIZE).abs_diff(x) <= 1 && (i / VECTOR_SIZE).abs_diff(y) <= 1, "{c:?}");
        }
    }
}
