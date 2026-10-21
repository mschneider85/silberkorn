//! AI denoising without new files: a neural network (in `sk-ai`) cleans the raw data once per
//! file; the pipeline blends the result into the source by `Detail::ai_denoise` (see
//! [`crate::ai_product`]).

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::ai_product::{Blend, ComputeFn, Store};
use crate::image::LinearImage;

/// Products are large (about 4 bytes per pixel): the oldest are removed beyond this.
pub const CACHE_LIMIT: u64 = 20 << 30;

static STORE: Store = Store::new("KI-Entrauschen", "denoise", "SILBERKORN_DENOISE_CACHE", 1, CACHE_LIMIT, Blend::Replace);

/// Registers the function that computes the cleaned data; `tag` names the model (e.g. its hash).
pub fn set_provider(tag: &str, compute: Box<ComputeFn>) {
    STORE.set_provider(tag, compute);
}

/// The cleaned data of the image exists (so rendering uses it).
pub fn is_ready(path: &Path) -> bool {
    STORE.is_ready(path)
}

/// The model is registered (the application can compute products).
pub fn available() -> bool {
    STORE.available()
}

/// Computes and stores the cleaned data unless it exists.
pub fn ensure(path: &Path, progress: &dyn Fn(f32)) -> Result<()> {
    STORE.ensure(path, progress)
}

/// Disk space of the cleaned data.
pub fn cache_size() -> u64 {
    STORE.cache_size()
}

/// Removes all cleaned data.
pub fn clear_cache() -> Result<()> {
    STORE.clear_cache()
}

/// The source with `amount` (0..1) of the cleaned data in place of the plain one, or `None` if
/// there is none (yet) or nothing to blend.
pub fn blended(path: &Path, plain: &LinearImage, amount: f32) -> Option<Arc<LinearImage>> {
    STORE.blended(path, plain, amount)
}
