//! Depth maps: how near each part of the image is, predicted by a neural network (in `sk-ai`)
//! once per file and kept in a small disk cache. Depth masks and the lens blur use them.
//!
//! The values are relative: 0 is the farthest part of the image, 1 the nearest (the network's
//! inverse depth, scaled to the image), in normalized source coordinates like masks.
//!
//! sk-core cannot run networks; the application registers the function that computes a map
//! ([`set_provider`]). Without it, or before a map exists, the lens blur is left out.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, bail};

/// Changes when maps made earlier must not be used any more.
pub const VERSION: u32 = 1;
/// Maps are small (about 1 MB); the oldest are removed beyond this.
pub const CACHE_LIMIT: u64 = 2 << 30;

const MAGIC: &[u8; 4] = b"SKDP";

/// A depth map, 0 (far) .. 1 (near).
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl DepthMap {
    /// Bilinear sample at normalized source coordinates.
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let x = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let at = |xx: usize, yy: usize| self.data[yy * self.width + xx];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bot - top) * ty
    }

    /// The depth below which the share `q` (0..1) of the image lies.
    pub fn quantile(&self, q: f32) -> f32 {
        let mut v: Vec<f32> = self.data.iter().step_by((self.data.len() / 50_000).max(1)).copied().collect();
        if v.is_empty() {
            return 0.5;
        }
        let i = ((v.len() - 1) as f32 * q.clamp(0.0, 1.0)) as usize;
        *v.select_nth_unstable_by(i, f32::total_cmp).1
    }

    /// The map with at most `edge` pixels on the long edge (area average).
    pub fn resized(&self, edge: usize) -> DepthMap {
        let k = edge as f32 / self.width.max(self.height) as f32;
        if k >= 1.0 {
            return self.clone();
        }
        let (w, h) = (((self.width as f32 * k).round() as usize).max(1), ((self.height as f32 * k).round() as usize).max(1));
        let img = crate::image::LinearImage::new(self.width, self.height, self.data.iter().map(|d| [*d; 3]).collect()).resize_area(w, h);
        DepthMap { width: w, height: h, data: img.data.iter().map(|p| p[0]).collect() }
    }

    /// Mask of a depth range, 8 bit at the map's resolution.
    pub fn range_mask(&self, r: &crate::params::DepthRange) -> crate::params::AiMask {
        crate::params::AiMask {
            kind: "depth".into(),
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|d| (r.weight(*d) * 255.0 + 0.5) as u8).collect::<Vec<u8>>().into(),
            depth: Some(*r),
        }
    }
}

/// Masks of depth ranges made recently (relight and atmosphere ask for the same ones with
/// every rendering): by map and range, sharing the bitmap so the mask cache recognizes it.
type RangeKey = (usize, [u32; 3]);
static RANGE_MASKS: Mutex<Vec<(RangeKey, Arc<DepthMap>, crate::params::AiMask)>> = Mutex::new(Vec::new());

/// [`DepthMap::range_mask`], reused while the map and the range stay the same.
pub fn range_mask_cached(map: &Arc<DepthMap>, r: &crate::params::DepthRange) -> crate::params::AiMask {
    let key = (Arc::as_ptr(map) as usize, [r.near, r.far, r.falloff].map(f32::to_bits));
    let mut done = RANGE_MASKS.lock().unwrap();
    // The map is held with its key, so the address can't be reused by another one.
    if let Some(e) = done.iter().find(|(k, m, _)| *k == key && Arc::ptr_eq(m, map)) {
        return e.2.clone();
    }
    let mask = map.range_mask(r);
    done.push((key, map.clone(), mask.clone()));
    if done.len() > 8 {
        done.remove(0);
    }
    mask
}

/// Computes the depth map of an image file (with progress 0..1).
pub type ComputeFn = dyn Fn(&Path, &dyn Fn(f32)) -> Result<DepthMap> + Send + Sync;

struct Provider {
    /// Identifies the model: maps of another tag are not used.
    tag: String,
    compute: Box<ComputeFn>,
}

static PROVIDER: RwLock<Option<Arc<Provider>>> = RwLock::new(None);

/// Registers the function that computes maps; `tag` names the model (e.g. its hash).
pub fn set_provider(tag: &str, compute: Box<ComputeFn>) {
    *PROVIDER.write().unwrap() = Some(Arc::new(Provider { tag: tag.to_string(), compute }));
}

fn provider() -> Option<Arc<Provider>> {
    PROVIDER.read().unwrap().clone()
}

/// The model is registered (the application can compute maps).
pub fn available() -> bool {
    provider().is_some()
}

/// Folder of the maps (`SILBERKORN_DEPTH_CACHE` overrides it).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("SILBERKORN_DEPTH_CACHE").map(PathBuf::from).unwrap_or_else(|| crate::paths::cache_dir().join("depth"))
}

/// The map file of an image, named after the file's identity and the model.
fn map_file(path: &Path) -> Option<PathBuf> {
    let tag = provider()?.tag.clone();
    let id = format!("{}|{tag}|{VERSION}", crate::cache::file_identity(path)?);
    Some(cache_dir().join(format!("{:016x}.skdp", crate::cache::fnv1a(id.as_bytes()))))
}

/// The map exists.
pub fn is_ready(path: &Path) -> bool {
    map_file(path).is_some_and(|f| f.is_file())
}

/// One computation at a time.
static COMPUTE: Mutex<()> = Mutex::new(());

/// Computes and stores the map unless it exists.
pub fn ensure(path: &Path, progress: &dyn Fn(f32)) -> Result<()> {
    let p = provider().context("Die Tiefenkarte ist nicht eingerichtet (Modell fehlt)")?;
    let file = map_file(path).with_context(|| format!("{} nicht lesbar", path.display()))?;
    let _one = COMPUTE.lock().unwrap_or_else(|e| e.into_inner());
    if file.is_file() {
        return Ok(());
    }
    let map = (p.compute)(&crate::variant::file_of(path), progress)?;
    std::fs::create_dir_all(cache_dir())?;
    crate::safe_file::write(&file, encode(&map))?;
    prune(&cache_dir(), CACHE_LIMIT);
    Ok(())
}

/// Recently loaded maps.
static LOADED: Mutex<Vec<(PathBuf, Arc<DepthMap>)>> = Mutex::new(Vec::new());

/// The map of an image, if it has been computed.
pub fn load(path: &Path) -> Option<Arc<DepthMap>> {
    let file = map_file(path)?;
    let mut loaded = LOADED.lock().unwrap();
    if let Some(i) = loaded.iter().position(|(f, _)| *f == file) {
        let e = loaded.remove(i);
        let map = e.1.clone();
        loaded.push(e);
        return Some(map);
    }
    let bytes = std::fs::read(&file).ok()?;
    let map = match decode(&bytes) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            log::warn!("{}: {e:#}", file.display());
            let _ = std::fs::remove_file(&file);
            return None;
        }
    };
    // Recently used: kept longest when the cache is full.
    let _ = std::fs::File::options().append(true).open(&file).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    loaded.push((file, map.clone()));
    if loaded.len() > 8 {
        loaded.remove(0);
    }
    Some(map)
}

/// Removes the least recently used maps until the folder fits in `limit` bytes.
fn prune(dir: &Path, limit: u64) {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "skdp"))
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

/// `SKDP`, version, width, height (u32 little endian), then the depths as u16 (0..65535), low
/// bytes first, then high bytes, zlib-compressed.
fn encode(map: &DepthMap) -> Vec<u8> {
    let n = map.data.len();
    let mut plane = vec![0u8; 2 * n];
    for (i, d) in map.data.iter().enumerate() {
        let [l, h] = ((d.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).to_le_bytes();
        (plane[i], plane[n + i]) = (l, h);
    }
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(MAGIC);
    for v in [VERSION, map.width as u32, map.height as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::default());
    z.write_all(&plane).expect("in memory");
    z.finish().expect("in memory")
}

fn decode(bytes: &[u8]) -> Result<DepthMap> {
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        bail!("Keine Tiefenkarte");
    }
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
    let (version, w, h) = (u32_at(4), u32_at(8), u32_at(12));
    anyhow::ensure!(version == VERSION as usize, "Veraltete Tiefenkarte");
    anyhow::ensure!(w > 0 && h > 0 && w * h <= 64_000_000, "Ungültige Größe der Tiefenkarte");
    let n = w * h;
    let mut plane = Vec::with_capacity(2 * n);
    flate2::read::ZlibDecoder::new(&bytes[16..]).take(2 * n as u64 + 1).read_to_end(&mut plane)?;
    anyhow::ensure!(plane.len() == 2 * n, "Tiefenkarte unvollständig");
    let data = (0..n).map(|i| u16::from_le_bytes([plane[i], plane[n + i]]) as f32 / 65535.0).collect();
    Ok(DepthMap { width: w, height: h, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::DepthRange;

    #[test]
    fn file_format_roundtrip() {
        let map = DepthMap { width: 3, height: 2, data: vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.1] };
        let back = decode(&encode(&map)).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        for (a, b) in map.data.iter().zip(&back.data) {
            assert!((a - b).abs() < 1e-4);
        }
        assert!(decode(b"SKDNxxxxxxxxxxxxxx").is_err());
    }

    #[test]
    fn range_takes_the_near_part() {
        let r = DepthRange { near: 1.0, far: 0.6, falloff: 0.1 };
        assert_eq!(r.weight(1.0), 1.0);
        assert_eq!(r.weight(0.8), 1.0);
        assert!(r.weight(0.55) > 0.2 && r.weight(0.55) < 0.8);
        assert_eq!(r.weight(0.3), 0.0);
        // A middle band, with the ends given either way round.
        let mid = DepthRange { near: 0.4, far: 0.6, falloff: 0.05 };
        assert_eq!(mid.weight(0.5), 1.0);
        assert_eq!(mid.weight(0.9), 0.0);
        assert_eq!(mid.weight(0.1), 0.0);
    }

    #[test]
    fn quantile_and_sample() {
        let map = DepthMap { width: 100, height: 1, data: (0..100).map(|i| i as f32 / 99.0).collect() };
        assert!((map.quantile(0.9) - 0.9).abs() < 0.05);
        assert!((map.sample(0.5, 0.5) - 0.5).abs() < 0.02);
    }
}
