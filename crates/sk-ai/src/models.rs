//! Neural network models that are downloaded on request (they are not shipped with
//! Silberkorn): where they live, where they come from, and the download itself.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A downloadable model. The download is checked against the exact size and SHA-256 of the
/// file the URL served when it was added: a changed or tampered file is rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Shown to the user.
    pub label: &'static str,
    pub file: &'static str,
    /// Pinned to a fixed revision where the host allows it.
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub license: &'static str,
}

impl ModelInfo {
    /// Size of the download in MB (rounded up), for the user.
    pub fn size_mb(&self) -> u64 {
        self.bytes.div_ceil(1_000_000)
    }
}

/// AI denoising: NAFNet (width 32, megvii-research) trained on SIDD real camera noise; about
/// three times faster than the width-64 variant at nearly the same quality.
pub const DENOISE: ModelInfo = ModelInfo {
    label: "Entrauschen (NAFNet-SIDD)",
    file: "nafnet_sidd_w32.onnx",
    url: "https://github.com/AlfredWei/emulsion/releases/download/denoise-model-nafnet-sidd-w32-v1/nafnet_sidd_w32.onnx",
    bytes: 117_316_644,
    sha256: "5ff072283fe8c86b7c990aa7331cdb26183e20b08961769c70057b82260b133d",
    license: "MIT",
};

/// Object removal: MI-GAN (Picsart), inpainting at 512 px.
pub const INPAINT: ModelInfo = ModelInfo {
    label: "Entfernen (MI-GAN)",
    file: "migan.onnx",
    url: "https://huggingface.co/andraniksargsyan/migan/resolve/406830d0fa60666da0071c342ad2fbc8f30c5c64/migan.onnx",
    bytes: 29_546_882,
    sha256: "593eba0b7e04730f1b61c0a3cbca68d97d8d6a7ff5c6a44a7b9d7fcd880fc5ae",
    license: "MIT",
};

/// Face detection: YuNet (OpenCV Zoo).
pub const FACE_DETECT: ModelInfo = ModelInfo {
    label: "Gesichtserkennung (YuNet)",
    file: "face_detection_yunet_2023mar.onnx",
    url: "https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx",
    bytes: 232_589,
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    license: "MIT",
};

/// Face recognition: SFace (OpenCV Zoo), 128-d embeddings of aligned faces.
pub const FACE_EMBED: ModelInfo = ModelInfo {
    label: "Gesichtsvergleich (SFace)",
    file: "face_recognition_sface_2021dec.onnx",
    url: "https://huggingface.co/opencv/face_recognition_sface/resolve/3d7082438a6e4551e840c9b2bb60b71e8da4b524/face_recognition_sface_2021dec.onnx",
    bytes: 38_696_353,
    sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    license: "Apache-2.0",
};

/// Eye state: open-closed-eye-0001 (Intel OpenVINO Training Extensions), 32 x 32 eye crops.
pub const EYE_STATE: ModelInfo = ModelInfo {
    label: "Augen offen/geschlossen",
    file: "open_closed_eye.onnx",
    url: "https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx",
    bytes: 46_164,
    sha256: "4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593",
    license: "Apache-2.0",
};

/// Subject selection: U²-Net-p (rembg's copy of the original weights).
pub const SUBJECT: ModelInfo = ModelInfo {
    label: "Motiv auswählen (U²-Net-p)",
    file: "u2netp.onnx",
    url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx",
    bytes: 4_574_861,
    sha256: "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8",
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
    download_to(m, &target, progress)?;
    Ok(target)
}

/// Downloads a model to `target`. An interrupted download continues where it stopped (kept as
/// `<target>.part`); the file only takes its place after size and SHA-256 were checked.
pub fn download_to(m: &ModelInfo, target: &Path, progress: &dyn Fn(f32)) -> Result<()> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    let part = target.with_file_name(name);
    let mut have = std::fs::metadata(&part).map_or(0, |md| md.len());
    if have > m.bytes {
        std::fs::remove_file(&part)?;
        have = 0;
    }
    if have < m.bytes {
        fetch(m, &part, have, progress).with_context(|| format!("Download von {} fehlgeschlagen", m.label))?;
    }
    if let Err(e) = verify(m, &part) {
        // Corrupt or altered: start from scratch next time.
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, target)?;
    progress(1.0);
    Ok(())
}

/// Appends the rest of the file from byte `have` on (or starts over if the server can't resume).
fn fetch(m: &ModelInfo, part: &Path, have: u64, progress: &dyn Fn(f32)) -> Result<()> {
    let mut req = ureq::get(m.url);
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let mut resp = req.call()?;
    let resumed = have > 0 && resp.status().as_u16() == 206;
    let mut done = if resumed { have } else { 0 };
    let mut file = std::fs::File::options().create(true).write(true).append(resumed).truncate(!resumed).open(part)?;
    // Never more than the expected size, whatever the server sends.
    let mut reader = resp.body_mut().with_config().limit(m.bytes - done + 1).reader();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        anyhow::ensure!(done <= m.bytes, "Die Datei ist größer als erwartet");
        progress((done as f64 / m.bytes.max(1) as f64).min(1.0) as f32);
    }
    file.sync_all()?;
    Ok(())
}

/// Size and SHA-256 of a downloaded file must be the expected ones.
pub fn verify(m: &ModelInfo, file: &Path) -> Result<()> {
    let len = std::fs::metadata(file)?.len();
    anyhow::ensure!(len == m.bytes, "Download von {} unvollständig ({len} von {} Bytes)", m.label, m.bytes);
    let digest = sha256_file(file)?;
    anyhow::ensure!(
        digest.eq_ignore_ascii_case(m.sha256),
        "{}: Prüfsumme stimmt nicht (die Datei wurde verändert oder beschädigt)",
        m.label
    );
    Ok(())
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(file: &Path) -> Result<String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut std::fs::File::open(file)?, &mut hasher)?;
    Ok(sha256_hex(hasher))
}

/// SHA-256 of bytes as lowercase hex.
pub fn sha256(data: &[u8]) -> String {
    use sha2::Digest;
    sha256_hex(sha2::Sha256::new_with_prefix(data))
}

fn sha256_hex(hasher: sha2::Sha256) -> String {
    use sha2::Digest;
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_known_input() {
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn wrong_or_partial_files_are_rejected() {
        let dir = std::env::temp_dir().join(format!("sk-models-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("m.onnx");
        let m = ModelInfo { label: "Test", file: "m.onnx", url: "", bytes: 3, sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", license: "" };
        std::fs::write(&file, b"abc").unwrap();
        verify(&m, &file).unwrap();
        std::fs::write(&file, b"abd").unwrap();
        assert!(format!("{:#}", verify(&m, &file).unwrap_err()).contains("Prüfsumme"));
        std::fs::write(&file, b"ab").unwrap();
        assert!(format!("{:#}", verify(&m, &file).unwrap_err()).contains("unvollständig"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn every_model_is_pinned() {
        for m in [DENOISE, INPAINT, FACE_DETECT, FACE_EMBED, EYE_STATE, SUBJECT] {
            assert!(m.bytes > 0 && m.sha256.len() == 64 && m.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}", m.label);
            assert!(!m.url.contains("/resolve/main/"), "{}: nicht auf einen Stand festgelegt", m.label);
        }
    }
}
