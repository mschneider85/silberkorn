//! Running an image network with a fixed input size over a larger image: in overlapping tiles,
//! blended where they meet. Used for denoising.

use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::runtime::{Data, Net};

/// How a network is run over an image.
#[derive(Clone, Copy, Debug)]
pub struct Tiling {
    /// The network's input edge (it takes `tile` x `tile`).
    pub tile: usize,
    /// Overlap of neighbouring tiles in input pixels.
    pub overlap: usize,
    /// Output pixels per input pixel (per edge).
    pub scale: usize,
    /// At most this many tiles at once on the processor (each needs a lot of memory).
    pub parallel: usize,
}

/// Matching each tile's output brightness to its input (for networks whose output scale
/// varies): channels weighted by `weights`; `nominal` is the scale for black tiles.
#[derive(Clone, Copy, Debug)]
pub struct Gain {
    pub weights: [f32; 3],
    pub nominal: f32,
}

/// Tile origins along one axis: `0, step, …` with the last tile ending at the border.
pub(crate) fn origins(len: usize, tile: usize, overlap: usize) -> Vec<usize> {
    if len <= tile {
        return vec![0];
    }
    // An overlap of a whole tile or more would never advance.
    let step = tile.saturating_sub(overlap).max(1);
    let mut v: Vec<usize> = (0..).map(|i| i * step).take_while(|&o| o + tile < len).collect();
    v.push(len - tile);
    v
}

/// Blending weight at position `i` (0..`len`) of an output tile whose overlap is `o` pixels:
/// 1 inside, falling linearly over the overlap towards neighbouring tiles (not at the border).
pub(crate) fn ramp(i: usize, len: usize, o: f32, at_start: bool, at_end: bool) -> f32 {
    let mut w = 1.0f32;
    if !at_start {
        w = w.min((i as f32 + 0.5) / o);
    }
    if !at_end {
        w = w.min((len - i) as f32 / o - 0.5 / o);
    }
    w.clamp(1e-3, 1.0)
}

/// Mirrors a position outside `0..len` back in (for images smaller than a tile).
pub(crate) fn mirror(i: usize, len: usize) -> usize {
    if i < len { i } else { (2 * len).saturating_sub(i + 2).min(len - 1) }
}

/// Runs a network over `planes` (`w` x `h` each) in overlapping tiles; returns three planes of
/// `scale * w` x `scale * h`. Progress (and with it cancelling) is reported from the calling
/// thread, after each round of tiles.
/// Runs `run` for every tile, `threads` at a time (in no fixed order), reporting progress.
pub(crate) fn run_all<T: Sync>(tiles: &[T], threads: usize, run: impl Fn(&T) -> Result<()> + Sync, progress: &dyn Fn(f32)) -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    if threads <= 1 {
        for (i, t) in tiles.iter().enumerate() {
            run(t)?;
            progress((i + 1) as f32 / tiles.len() as f32);
        }
        return Ok(());
    }
    // `progress` may not be called from other threads: they count, this one reports.
    let done = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|s| {
        let worker = s.spawn(|| {
            let r = pool.install(|| {
                tiles.par_iter().with_max_len(1).try_for_each(|t| {
                    run(t)?;
                    done.fetch_add(1, Ordering::Relaxed);
                    let _ = tx.send(());
                    Ok(())
                })
            });
            drop(tx);
            r
        });
        while rx.recv().is_ok() {
            progress(done.load(Ordering::Relaxed) as f32 / tiles.len() as f32);
        }
        worker.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}

pub fn run_tiled(net: &Net, planes: &[Vec<f32>], w: usize, h: usize, t: Tiling, gain: Option<Gain>, progress: &dyn Fn(f32)) -> Result<[Vec<f32>; 3]> {
    let (xs, ys) = (origins(w, t.tile, t.overlap), origins(h, t.tile, t.overlap));
    let tiles: Vec<(usize, usize, usize, usize)> =
        ys.iter().enumerate().flat_map(|(yi, &ty)| xs.iter().enumerate().map(move |(xi, &tx)| (xi, yi, tx, ty))).collect();
    let (scale, tile) = (t.scale, t.tile);
    let (ow, oh, ot) = (scale * w, scale * h, scale * tile);
    let o = (scale * t.overlap) as f32;
    let acc = Mutex::new(([vec![0.0f32; ow * oh], vec![0.0f32; ow * oh], vec![0.0f32; ow * oh]], vec![0.0f32; ow * oh]));
    let run = |&(xi, yi, tx, ty): &(usize, usize, usize, usize)| -> Result<()> {
        let c = planes.len();
        let mut input = vec![0.0f32; c * tile * tile];
        for (p, plane) in planes.iter().enumerate() {
            for j in 0..tile {
                let y = mirror(ty + j, h);
                for i in 0..tile {
                    input[p * tile * tile + j * tile + i] = plane[y * w + mirror(tx + i, w)];
                }
            }
        }
        let out = net.run(&[Data::F32(&input)])?;
        let out = out.first().context("Keine Modellausgabe")?;
        let n = ot * ot;
        anyhow::ensure!(out.data.len() == 3 * n, "Unerwartete Modellausgabe {:?}", out.shape);
        let k = match gain {
            Some(g) => {
                let mean_in = input.iter().sum::<f32>() / input.len() as f32;
                let mean_out = (0..3).map(|k| g.weights[k] * out.data[k * n..(k + 1) * n].iter().sum::<f32>() / n as f32).sum::<f32>();
                let k = mean_in / mean_out;
                if mean_in > 1e-4 && k.is_finite() && k != 0.0 { k } else { g.nominal }
            }
            None => 1.0,
        };
        let mut acc = acc.lock().unwrap_or_else(|e| e.into_inner());
        let (sum, wsum) = &mut *acc;
        for j in 0..ot {
            let y = scale * ty + j;
            if y >= oh {
                break;
            }
            let wy = ramp(j, ot, o, yi == 0, yi + 1 == ys.len());
            for i in 0..ot {
                let x = scale * tx + i;
                if x >= ow {
                    break;
                }
                let wt = wy * ramp(i, ot, o, xi == 0, xi + 1 == xs.len());
                for (c, plane) in sum.iter_mut().enumerate() {
                    plane[y * ow + x] += out.data[c * n + j * ot + i] * k * wt;
                }
                wsum[y * ow + x] += wt;
            }
        }
        Ok(())
    };
    // On ONNX Runtime the network runs one tile at a time; a second tile in flight prepares
    // its input and takes in its result meanwhile, so the card doesn't wait for that.
    let threads = if net.prefers_serial() { 2 } else { t.parallel.min(rayon::current_num_threads()).max(1) };
    run_all(&tiles, threads, run, progress)?;
    let (mut sum, wsum) = acc.into_inner().unwrap();
    for plane in &mut sum {
        plane.par_iter_mut().zip(&wsum).for_each(|(v, s)| *v /= s.max(1e-6));
    }
    if sum.iter().any(|p| p.iter().any(|v| !v.is_finite())) {
        bail!("Das Modell lieferte ungültige Werte");
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_image_and_blend_to_one() {
        let (tile, overlap) = (512, 64);
        assert_eq!(origins(100, tile, overlap), vec![0]);
        assert_eq!(origins(tile, tile, overlap), vec![0]);
        let o = origins(3000, tile, overlap);
        assert_eq!(*o.last().unwrap(), 3000 - tile);
        assert!(o.windows(2).all(|w| w[1] - w[0] <= tile - overlap));
        // Inside a tile the weight is 1; at a shared edge both tiles contribute.
        assert_eq!(ramp(tile / 2, tile, 64.0, false, false), 1.0);
        assert!(ramp(0, tile, 64.0, false, false) < 0.1 && ramp(0, tile, 64.0, true, false) == 1.0);
        // Scaled tiles (output twice the input) fall over a doubled overlap.
        assert_eq!(ramp(128, 2 * tile, 128.0, false, false), 1.0);
        assert!(ramp(64, 2 * tile, 128.0, false, false) < 0.6);
        assert_eq!((mirror(5, 10), mirror(10, 10), mirror(11, 10)), (5, 8, 7));
    }

    /// An overlap as large as the tile still ends (step 1) instead of looping forever.
    #[test]
    fn overlap_of_a_whole_tile_terminates() {
        assert_eq!(origins(12, 8, 8), vec![0, 1, 2, 3, 4]);
        assert_eq!(origins(12, 8, 20), vec![0, 1, 2, 3, 4]);
    }
}
