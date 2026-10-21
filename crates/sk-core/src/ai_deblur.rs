//! Removing camera shake and motion blur without new files: a neural network (in `sk-ai`)
//! sharpens the raw data once per file; what it changed is kept, and the pipeline adds it to
//! the source (after AI denoising) by `Detail::ai_deblur` (see [`crate::ai_product`]).

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::ai_product::{Blend, ComputeFn, Store};
use crate::image::LinearImage;

/// The oldest results are removed beyond this (they are differences and compress well).
pub const CACHE_LIMIT: u64 = 10 << 30;

static STORE: Store = Store::new("Verwacklung entfernen", "deblur", "SILBERKORN_DEBLUR_CACHE", 1, CACHE_LIMIT, Blend::Add);

/// Registers the function that computes the difference; `tag` names the model (e.g. its hash).
pub fn set_provider(tag: &str, compute: Box<ComputeFn>) {
    STORE.set_provider(tag, compute);
}

/// The result for the image exists (so rendering uses it).
pub fn is_ready(path: &Path) -> bool {
    STORE.is_ready(path)
}

/// The model is registered (the application can compute results).
pub fn available() -> bool {
    STORE.available()
}

/// Computes and stores the result unless it exists.
pub fn ensure(path: &Path, progress: &dyn Fn(f32)) -> Result<()> {
    STORE.ensure(path, progress)
}

/// Disk space of the results.
pub fn cache_size() -> u64 {
    STORE.cache_size()
}

/// Removes all results.
pub fn clear_cache() -> Result<()> {
    STORE.clear_cache()
}

/// `base` (the source, maybe denoised) with `amount` (0..1) of the sharpening added, or `None`
/// if there is no result (yet) or nothing to add.
pub fn blended(path: &Path, base: &LinearImage, amount: f32) -> Option<Arc<LinearImage>> {
    STORE.blended(path, base, amount)
}
