//! Neural network models that are downloaded on request (they are not shipped with
//! Silberkorn): where they live, where they come from, and the download itself.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

/// A downloadable model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Shown to the user.
    pub label: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    /// Approximate size of the download.
    pub size_mb: u32,
    pub license: &'static str,
}

/// AI denoising: NAFNet (width 32, megvii-research) trained on SIDD real camera noise; about
/// three times faster than the width-64 variant at nearly the same quality.
pub const DENOISE: ModelInfo = ModelInfo {
    label: "Entrauschen (NAFNet-SIDD)",
    file: "nafnet_sidd_w32.onnx",
    url: "https://github.com/AlfredWei/emulsion/releases/download/denoise-model-nafnet-sidd-w32-v1/nafnet_sidd_w32.onnx",
    size_mb: 117,
    license: "MIT",
};

/// Object removal: MI-GAN (Picsart), inpainting at 512 px.
pub const INPAINT: ModelInfo = ModelInfo {
    label: "Entfernen (MI-GAN)",
    file: "migan.onnx",
    url: "https://huggingface.co/andraniksargsyan/migan/resolve/main/migan.onnx",
    size_mb: 30,
    license: "MIT",
};

/// Face detection: YuNet (OpenCV Zoo).
pub const FACE_DETECT: ModelInfo = ModelInfo {
    label: "Gesichtserkennung (YuNet)",
    file: "face_detection_yunet_2023mar.onnx",
    url: "https://huggingface.co/opencv/face_detection_yunet/resolve/main/face_detection_yunet_2023mar.onnx",
    size_mb: 1,
    license: "MIT",
};

/// Face recognition: SFace (OpenCV Zoo), 128-d embeddings of aligned faces.
pub const FACE_EMBED: ModelInfo = ModelInfo {
    label: "Gesichtsvergleich (SFace)",
    file: "face_recognition_sface_2021dec.onnx",
    url: "https://huggingface.co/opencv/face_recognition_sface/resolve/main/face_recognition_sface_2021dec.onnx",
    size_mb: 37,
    license: "Apache-2.0",
};

/// Folder of the downloaded models (`SILBERKORN_MODELS` overrides it).
pub fn models_dir() -> PathBuf {
    std::env::var_os("SILBERKORN_MODELS").map(PathBuf::from).unwrap_or_else(|| sk_core::paths::data_dir().join("models"))
}

pub fn path(m: &ModelInfo) -> PathBuf {
    models_dir().join(m.file)
}

pub fn available(m: &ModelInfo) -> bool {
    path(m).is_file()
}

/// Downloads a model (streaming, with progress 0..1); returns its path.
pub fn download(m: &ModelInfo, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let target = path(m);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut resp = ureq::get(m.url).call().with_context(|| format!("Download von {} fehlgeschlagen", m.label))?;
    let total = resp.body().content_length().unwrap_or(m.size_mb as u64 * 1_000_000);
    let mut reader = resp.body_mut().with_config().limit(4 << 30).reader();
    let part = target.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress((done as f64 / total.max(1) as f64).min(1.0) as f32);
    }
    file.flush()?;
    drop(file);
    // A truncated download must not count as the model.
    anyhow::ensure!(done >= total.min(m.size_mb as u64 * 500_000), "Download von {} unvollständig", m.label);
    std::fs::rename(&part, &target)?;
    Ok(target)
}
