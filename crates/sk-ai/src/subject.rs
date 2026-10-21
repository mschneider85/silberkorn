//! Subject selection with exact edges: BiRefNet lite (Peng Zheng et al., MIT), a network for
//! separating the foreground at high resolution. It sees the whole image at 1024 x 1024 and finds
//! hair, fur and thin parts that U²-Net (still used for the automatic adjustments, where speed
//! counts) only blurs.

use anyhow::{Context, Result};
use sk_core::detail::Plane;

use crate::models::{self, SUBJECT_EDGES};
use crate::runtime::{Data, Input, Net};
use crate::{Guide, MaskImage, refine_edges, to_mask};

/// The network's input edge (the image is stretched to the square).
const NET: usize = 1024;
/// The mask's long edge: the network's resolution, more than other masks get.
const EDGE: usize = 1024;

pub fn available() -> bool {
    models::available(&SUBJECT_EDGES)
}

pub struct SubjectNet {
    net: Net,
}

impl SubjectNet {
    pub fn load() -> Result<Self> {
        anyhow::ensure!(available(), "{} ist nicht geladen", SUBJECT_EDGES.label);
        // On the processor: on a card with 4 GB it took minutes, and Windows reset the card.
        Ok(Self { net: Net::load_processor(&models::path(&SUBJECT_EDGES), &[Input::F32([1, 3, NET, NET])])? })
    }

    /// The subject's mask, the image's aspect at most [`EDGE`] pixels long.
    pub fn mask(&self, guide: &Guide) -> Result<MaskImage> {
        let small = guide.at_edge(EDGE);
        let square = sk_core::image::LinearImage::new(small.width, small.height, small.rgb.clone()).resize_area(NET.min(small.width), NET.min(small.height));
        // Normalized like in training (ImageNet mean and deviation), bilinear to the square.
        let mean = [0.485, 0.456, 0.406];
        let std = [0.229, 0.224, 0.225];
        let mut input = vec![0.0f32; 3 * NET * NET];
        for y in 0..NET {
            for x in 0..NET {
                let p = bilinear(&|xx, yy| square.data[yy * square.width + xx], square.width, square.height, (x as f32 + 0.5) / NET as f32, (y as f32 + 0.5) / NET as f32);
                for c in 0..3 {
                    input[c * NET * NET + y * NET + x] = (p[c].clamp(0.0, 1.0) - mean[c]) / std[c];
                }
            }
        }
        let out = self.net.run(&[Data::F32(&input)])?;
        let o = out.into_iter().next().context("Keine Modellausgabe")?;
        anyhow::ensure!(o.data.len() == NET * NET, "Unerwartete Modellausgabe {:?}", o.shape);
        // Logits back to the image's aspect.
        let (w, h) = (small.width, small.height);
        let soft = Plane::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (u, v) = (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
                    let l = bilinear(&|xx, yy| [o.data[yy * NET + xx]; 3], NET, NET, u, v)[0];
                    1.0 / (1.0 + (-l).exp())
                })
                .collect(),
        );
        // A small radius: the edges are the network's already, only aligned to the pixels.
        Ok(to_mask(&refine_edges(&soft, &small.luma(), 1, 1e-4)))
    }
}

/// Bilinear sample at normalized (`u`, `v`) of a `w` x `h` grid read by `at`.
fn bilinear(at: &dyn Fn(usize, usize) -> [f32; 3], w: usize, h: usize, u: f32, v: f32) -> [f32; 3] {
    let fx = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the downloaded model: an object on a plain ground is selected, the ground not.
    #[test]
    #[ignore = "langsam: läuft mit cargo t-all"]
    fn selects_the_subject_if_the_model_is_available() {
        if !available() {
            return;
        }
        let m = SubjectNet::load().unwrap();
        let (w, h) = (600, 400);
        let rgb = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 300.0, (i / w) as f32 - 220.0);
                if (x / 110.0).powi(2) + (y / 140.0).powi(2) < 1.0 { [0.8, 0.45, 0.2] } else { [0.15, 0.3, 0.2] }
            })
            .collect();
        let mask = m.mask(&Guide { width: w, height: h, rgb }).unwrap();
        assert_eq!((mask.width, mask.height), (600, 400), "nicht größer als das Bild");
        let at = |u: f32, v: f32| mask.data[((v * mask.height as f32) as usize) * mask.width + (u * mask.width as f32) as usize];
        assert!(at(0.5, 0.55) > 200, "Motiv {}", at(0.5, 0.55));
        assert!(at(0.05, 0.1) < 50 && at(0.95, 0.9) < 50, "Hintergrund {} {}", at(0.05, 0.1), at(0.95, 0.9));
    }
}
