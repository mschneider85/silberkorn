//! AI denoising without new files: a neural network (in `sk-ai`) cleans the raw data once per
//! file; the result ("product": camera RGB like [`crate::raw::load_full`] returns it, oriented) is
//! kept in a disk cache, and the pipeline blends it into the source by `Detail::ai_denoise`.
//! The raw file stays untouched, the amount can be changed at any time and is undoable.
//!
//! sk-core cannot run networks; the application registers the function that computes a product
//! ([`set_provider`]). Without it, or before a product exists, images are rendered without.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::image::LinearImage;

/// Changes when products made earlier must not be used any more.
pub const VERSION: u32 = 1;
/// Products are large (about 4 bytes per pixel): the oldest are removed beyond this.
pub const CACHE_LIMIT: u64 = 20 << 30;

const MAGIC: &[u8; 4] = b"SKDN";

/// Computes the product of a raw file (with progress 0..1).
pub type ComputeFn = dyn Fn(&Path, &dyn Fn(f32)) -> Result<LinearImage> + Send + Sync;

struct Provider {
    /// Identifies the model and its settings: products of another tag are not used.
    tag: String,
    compute: Box<ComputeFn>,
}

static PROVIDER: RwLock<Option<Arc<Provider>>> = RwLock::new(None);

/// Registers the function that computes products; `tag` names the model (e.g. its hash).
pub fn set_provider(tag: &str, compute: Box<ComputeFn>) {
    *PROVIDER.write().unwrap() = Some(Arc::new(Provider { tag: tag.to_string(), compute }));
}

fn provider() -> Option<Arc<Provider>> {
    PROVIDER.read().unwrap().clone()
}

/// Folder of the products (`SILBERKORN_DENOISE_CACHE` overrides it).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("SILBERKORN_DENOISE_CACHE").map(PathBuf::from).unwrap_or_else(|| crate::paths::cache_dir().join("denoise"))
}

/// The product file of an image: named after the raw file's identity (path, size, modification
/// time) and the model, so an edited file or another model never finds an old product.
pub fn product_file(path: &Path) -> Option<PathBuf> {
    let tag = provider()?.tag.clone();
    let id = format!("{}|{tag}|{VERSION}", crate::cache::file_identity(path)?);
    Some(cache_dir().join(format!("{:016x}.skdn", crate::cache::fnv1a(id.as_bytes()))))
}

/// The product exists (so rendering uses it).
pub fn is_ready(path: &Path) -> bool {
    product_file(path).is_some_and(|f| f.is_file())
}

/// The model is registered (the application can compute products).
pub fn available() -> bool {
    provider().is_some()
}

/// One computation at a time: each needs a lot of memory and all cores already.
static COMPUTE: Mutex<()> = Mutex::new(());

/// Computes and stores the product unless it exists.
pub fn ensure(path: &Path, progress: &dyn Fn(f32)) -> Result<()> {
    let p = provider().context("KI-Entrauschen ist nicht eingerichtet (Modell fehlt)")?;
    let file = product_file(path).with_context(|| format!("{} nicht lesbar", path.display()))?;
    let _one = COMPUTE.lock().unwrap_or_else(|e| e.into_inner());
    if file.is_file() {
        return Ok(());
    }
    let img = (p.compute)(&crate::variant::file_of(path), progress)?;
    std::fs::create_dir_all(cache_dir())?;
    crate::safe_file::write(&file, encode(&img))?;
    prune(&cache_dir(), CACHE_LIMIT);
    Ok(())
}

/// Disk space of the products.
pub fn cache_size() -> u64 {
    std::fs::read_dir(cache_dir()).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// Removes all products.
pub fn clear_cache() -> Result<()> {
    *LOADED.lock().unwrap() = None;
    BLENDS.lock().unwrap().clear();
    for e in std::fs::read_dir(cache_dir()).into_iter().flatten().flatten() {
        if e.path().extension().is_some_and(|x| x == "skdn") {
            std::fs::remove_file(e.path())?;
        }
    }
    Ok(())
}

/// Removes the least recently used products until the folder fits in `limit` bytes.
fn prune(dir: &Path, limit: u64) {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "skdn"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

// ------------------------------------------------------------------ file format

/// `SKDN`, version, width, height (u32 little endian), then the pixels as half floats, low bytes
/// first, then high bytes (they compress far better apart), zlib-compressed (with checksum).
fn encode(img: &LinearImage) -> Vec<u8> {
    let n = img.data.len() * 3;
    let mut plane = vec![0u8; 2 * n];
    let (lo, hi) = plane.split_at_mut(n);
    lo.par_chunks_mut(3).zip(hi.par_chunks_mut(3)).zip(img.data.par_iter()).for_each(|((l, h), p)| {
        for c in 0..3 {
            let [a, b] = half::f16::from_f32(p[c]).to_le_bytes();
            (l[c], h[c]) = (a, b);
        }
    });
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(MAGIC);
    for v in [VERSION, img.width as u32, img.height as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::fast());
    z.write_all(&plane).expect("in memory");
    z.finish().expect("in memory")
}

fn decode(bytes: &[u8]) -> Result<Product> {
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        bail!("Kein entrauschtes Bild");
    }
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
    let (version, w, h) = (u32_at(4), u32_at(8), u32_at(12));
    anyhow::ensure!(version == VERSION as usize, "Veraltetes entrauschtes Bild");
    anyhow::ensure!(w > 0 && h > 0 && w * h <= 400_000_000, "Ungültige Bildgröße");
    let n = w * h * 3;
    let mut plane = Vec::with_capacity(2 * n);
    flate2::read::ZlibDecoder::new(&bytes[16..]).take(2 * n as u64 + 1).read_to_end(&mut plane)?;
    anyhow::ensure!(plane.len() == 2 * n, "Entrauschtes Bild unvollständig");
    let (lo, hi) = plane.split_at(n);
    let data = lo.par_iter().zip(hi).map(|(l, h)| u16::from_le_bytes([*l, *h])).collect();
    Ok(Product { width: w, height: h, data })
}

/// A product in memory: half floats, three per pixel (half the memory of a `LinearImage`).
pub struct Product {
    pub width: usize,
    pub height: usize,
    data: Vec<u16>,
}

impl Product {
    fn at(&self, i: usize) -> [f32; 3] {
        [0, 1, 2].map(|c| half::f16::from_bits(self.data[3 * i + c]).to_f32())
    }

    /// Area average to `w` x `h` (for previews; the same as `LinearImage::resize_area`).
    fn resized(&self, w: usize, h: usize) -> LinearImage {
        if (w, h) == (self.width, self.height) {
            return LinearImage::new(w, h, (0..w * h).into_par_iter().map(|i| self.at(i)).collect());
        }
        // Rows first, then columns, each with fractional box weights.
        let sx = self.width as f32 / w as f32;
        let sy = self.height as f32 / h as f32;
        let spans = |len: usize, s: f32, n: usize| -> Vec<Vec<(usize, f32)>> {
            (0..n)
                .map(|o| {
                    let (a, b) = (o as f32 * s, ((o + 1) as f32 * s).min(len as f32));
                    (a.floor() as usize..(b.ceil() as usize).min(len))
                        .map(|i| (i, ((i + 1) as f32).min(b) - (i as f32).max(a)))
                        .filter(|(_, wt)| *wt > 0.0)
                        .collect()
                })
                .collect()
        };
        let (xs, ys) = (spans(self.width, sx, w), spans(self.height, sy, h));
        let data = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (ox, oy) = (i % w, i / w);
                let mut acc = [0.0f32; 3];
                let mut sum = 0.0;
                for &(y, wy) in &ys[oy] {
                    for &(x, wx) in &xs[ox] {
                        let p = self.at(y * self.width + x);
                        let wt = wx * wy;
                        acc = [acc[0] + p[0] * wt, acc[1] + p[1] * wt, acc[2] + p[2] * wt];
                        sum += wt;
                    }
                }
                acc.map(|v| v / sum.max(1e-12))
            })
            .collect();
        LinearImage::new(w, h, data)
    }
}

// ------------------------------------------------------------------ blending

/// The most recently used product (one: a full-resolution one takes hundreds of MB).
static LOADED: Mutex<Option<(PathBuf, Arc<Product>)>> = Mutex::new(None);

fn product(file: &Path) -> Option<Arc<Product>> {
    let mut loaded = LOADED.lock().unwrap();
    if let Some((f, p)) = loaded.as_ref()
        && f == file
    {
        return Some(p.clone());
    }
    let bytes = std::fs::read(file).ok()?;
    let p = match decode(&bytes) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            log::warn!("{}: {e:#}", file.display());
            let _ = std::fs::remove_file(file);
            return None;
        }
    };
    // Recently used: kept longest when the cache is full.
    let _ = std::fs::File::options().append(true).open(file).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    *loaded = Some((file.to_path_buf(), p.clone()));
    Some(p)
}

/// Blended sources by (plain source, product, amount): renderings of the same source with the same
/// amount reuse one, so the pipeline's stage caches (keyed by the source) stay valid.
type BlendKey = (usize, usize, usize, u64, PathBuf, u32);
static BLENDS: Mutex<Vec<(BlendKey, Arc<LinearImage>)>> = Mutex::new(Vec::new());
/// Memory for blended sources (the newest are kept).
const BLEND_BUDGET: usize = 600 << 20;

/// A few pixels of the source: tells a freed and newly allocated buffer at the same address apart.
pub(crate) fn fingerprint(img: &LinearImage) -> u64 {
    let n = img.data.len();
    let samples: Vec<u8> = (0..64).flat_map(|i| img.data[i * n / 64].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()).collect();
    crate::cache::fnv1a(&samples)
}

/// The source with `amount` (0..1) of the product blended in, or `None` if there is no product
/// (yet) or nothing to blend. `plain` is the image as `raw::load_full` or `raw::load_preview`
/// return it (any size: the product is resampled to it).
pub fn blended(path: &Path, plain: &LinearImage, amount: f32) -> Option<Arc<LinearImage>> {
    if amount <= 0.0 || plain.data.is_empty() {
        return None;
    }
    let file = product_file(path)?;
    let key: BlendKey = (plain.data.as_ptr() as usize, plain.width, plain.height, fingerprint(plain), file.clone(), amount.to_bits());
    {
        let mut blends = BLENDS.lock().unwrap();
        if let Some(i) = blends.iter().position(|(k, _)| *k == key) {
            let e = blends.remove(i);
            let img = e.1.clone();
            blends.push(e);
            return Some(img);
        }
    }
    let product = product(&file)?;
    let t = amount.clamp(0.0, 1.0);
    let img = if (product.width, product.height) == (plain.width, plain.height) {
        let data = plain
            .data
            .par_iter()
            .enumerate()
            .map(|(i, p)| {
                let d = product.at(i);
                [0, 1, 2].map(|c| p[c] + (d[c] - p[c]) * t)
            })
            .collect();
        LinearImage::new(plain.width, plain.height, data)
    } else {
        let small = resized(&file, &product, plain.width, plain.height);
        let data = plain.data.par_iter().zip(&small.data).map(|(p, d)| [0, 1, 2].map(|c| p[c] + (d[c] - p[c]) * t)).collect();
        LinearImage::new(plain.width, plain.height, data)
    };
    let img = Arc::new(img);
    let mut blends = BLENDS.lock().unwrap();
    blends.push((key, img.clone()));
    let mut total: usize = blends.iter().map(|(_, i)| i.data.len() * 12).sum();
    while total > BLEND_BUDGET && blends.len() > 1 {
        total -= blends.remove(0).1.data.len() * 12;
    }
    Some(img)
}

/// Products resampled to preview sizes (dragging the amount blends again, but resamples once).
type Resized = (PathBuf, usize, usize);
static RESIZED: Mutex<Vec<(Resized, Arc<LinearImage>)>> = Mutex::new(Vec::new());

fn resized(file: &Path, product: &Product, w: usize, h: usize) -> Arc<LinearImage> {
    let key = (file.to_path_buf(), w, h);
    if let Some((_, img)) = RESIZED.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return img.clone();
    }
    let img = Arc::new(product.resized(w, h));
    let mut r = RESIZED.lock().unwrap();
    r.push((key, img.clone()));
    if r.len() > 6 {
        r.remove(0);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize) -> LinearImage {
        LinearImage::new(w, h, (0..w * h).map(|i| [(i % 7) as f32 / 7.0, (i % 5) as f32 * 0.3, 1.5]).collect())
    }

    #[test]
    fn products_survive_storage_at_half_precision() {
        let img = image(31, 17);
        let p = decode(&encode(&img)).unwrap();
        assert_eq!((p.width, p.height), (31, 17));
        let back = p.resized(31, 17);
        for (a, b) in img.data.iter().zip(&back.data) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() <= a[c].abs() * 1e-3 + 1e-4, "{a:?} {b:?}");
            }
        }
        let mut bad = encode(&img);
        let n = bad.len();
        bad[n - 10] ^= 0xff;
        assert!(decode(&bad).is_err(), "checksum");
        assert!(decode(b"SKDNxxxx").is_err());
    }

    #[test]
    fn resampling_averages_areas() {
        let img = LinearImage::new(4, 2, vec![[1.0; 3], [3.0; 3], [5.0; 3], [7.0; 3], [1.0; 3], [3.0; 3], [5.0; 3], [7.0; 3]]);
        let p = decode(&encode(&img)).unwrap();
        let small = p.resized(2, 1);
        assert_eq!(small.data, vec![[2.0; 3], [6.0; 3]]);
        // Fractional boxes: 3 pixels into 2.
        let img = LinearImage::new(3, 1, vec![[0.0; 3], [3.0; 3], [6.0; 3]]);
        let r = decode(&encode(&img)).unwrap().resized(2, 1);
        assert!((r.data[0][0] - 1.0).abs() < 1e-3 && (r.data[1][0] - 5.0).abs() < 1e-3, "{:?}", r.data);
    }

    /// With a stand-in network: the product is computed once, stored, and blended into sources
    /// of any size by the amount.
    #[test]
    fn computes_once_and_blends_by_amount() {
        let dir = std::env::temp_dir().join(format!("sk-denoise-blend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var("SILBERKORN_DENOISE_CACHE", dir.join("cache")) };
        let raw = dir.join("bild.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        set_provider("test", Box::new(move |_, progress| {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            progress(1.0);
            Ok(LinearImage::new(8, 4, vec![[0.5, 0.25, 1.0]; 32]))
        }));
        assert!(!is_ready(&raw));
        let plain = LinearImage::new(8, 4, vec![[0.1; 3]; 32]);
        assert!(blended(&raw, &plain, 0.5).is_none(), "nothing before the product exists");
        ensure(&raw, &|_| {}).unwrap();
        ensure(&raw, &|_| {}).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(is_ready(&raw));
        let half = blended(&raw, &plain, 0.5).unwrap();
        assert!(half.data.iter().all(|p| (p[0] - 0.3).abs() < 1e-3 && (p[1] - 0.175).abs() < 1e-3 && (p[2] - 0.55).abs() < 1e-3));
        // The same source and amount: the same image (the pipeline's stage caches stay valid).
        assert!(Arc::ptr_eq(&half, &blended(&raw, &plain, 0.5).unwrap()));
        assert!(blended(&raw, &plain, 0.0).is_none());
        // A preview-sized source gets the product resampled.
        let small = LinearImage::new(4, 2, vec![[0.1; 3]; 8]);
        let full = blended(&raw, &small, 1.0).unwrap();
        assert_eq!((full.width, full.height), (4, 2));
        assert!(full.data.iter().all(|p| (p[0] - 0.5).abs() < 1e-3));
        // An edited raw file is another file: its old product is not used.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&raw, b"raw2").unwrap();
        assert!(!is_ready(&raw));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("sk-denoise-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let f = dir.join(format!("{name}.skdn"));
            std::fs::write(&f, [0u8; 10]).unwrap();
            std::fs::File::options().append(true).open(&f).unwrap().set_modified(t0 + std::time::Duration::from_secs(i as u64)).unwrap();
        }
        prune(&dir, 20);
        assert!(!dir.join("a.skdn").exists() && dir.join("b.skdn").exists() && dir.join("c.skdn").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
