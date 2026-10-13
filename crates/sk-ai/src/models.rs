//! Neural network models that are downloaded on request (they are not shipped with
//! Silberkorn): where they live, where they come from, and the download itself.

use std::io::Write;
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
    /// The model is this file inside the ZIP archive at `url` (`bytes` and `sha256` are the
    /// model's); only it is downloaded.
    pub entry: Option<&'static str>,
}

impl ModelInfo {
    /// Size of the download in MB (rounded up), for the user.
    pub fn size_mb(&self) -> u64 {
        self.bytes.div_ceil(1_000_000)
    }
}

/// AI denoising for Bayer sensors: RawNIND's UtNet2 (Benoit Brummer, UCLouvain) as published
/// for darktable; it takes the raw mosaic and returns demosaiced, clean camera RGB.
pub const DENOISE_BAYER: ModelInfo = ModelInfo {
    label: "KI-Entrauschen (RawNIND, Bayer)",
    file: "rawnind_bayer.onnx",
    url: RAWNIND_ARCHIVE,
    bytes: 31_056_425,
    sha256: "da27509dab6a2915da67e988acd86cf71f9d5bbc8d1aa0ed32933578a887b901",
    license: "GPL-3.0",
    entry: Some("rawdenoise-nind/model_bayer.onnx"),
};

/// The same network for demosaiced linear data (X-Trans, other sensors, linear DNGs).
pub const DENOISE_LINEAR: ModelInfo = ModelInfo {
    label: "KI-Entrauschen (RawNIND, linear)",
    file: "rawnind_linear.onnx",
    url: RAWNIND_ARCHIVE,
    bytes: 31_053_823,
    sha256: "df957efadcc152c007d5d3b0917bdff9e41c0d4a0efe56584ef30b36393cd181",
    license: "GPL-3.0",
    entry: Some("rawdenoise-nind/model_linear.onnx"),
};

/// darktable-ai's release archive with both RawNIND models (pinned release).
const RAWNIND_ARCHIVE: &str = "https://github.com/darktable-org/darktable-ai/releases/download/release-5.6.0/rawdenoise-nind.dtmodel";

/// Upscaling: RealPLKSR (Dongheon Lee et al., University of Seoul), the MSSIM stage
/// without GAN training, so it adds no invented texture; as converted for darktable.
pub const UPSCALE_X2: ModelInfo = ModelInfo {
    label: "KI-Vergrößern 2× (RealPLKSR)",
    file: "realplksr_x2.onnx",
    url: "https://huggingface.co/darktable-org/upscale-realplksr-onnx/resolve/0bb636eeeed5fffd256571c3cc440d9fdf62467e/onnx/model_x2.onnx",
    bytes: 29_627_920,
    sha256: "d7abb65092f3808d3aa255ffdab42b1d672883915d90adbdd321204168b9f293",
    license: "MIT",
    entry: None,
};

pub const UPSCALE_X4: ModelInfo = ModelInfo {
    label: "KI-Vergrößern 4× (RealPLKSR)",
    file: "realplksr_x4.onnx",
    url: "https://huggingface.co/darktable-org/upscale-realplksr-onnx/resolve/0bb636eeeed5fffd256571c3cc440d9fdf62467e/onnx/model_x4.onnx",
    bytes: 29_711_010,
    sha256: "8ce692652494e2e5c31e1f7c7ab61eb084e9a450cde727a374f5d6746cfe054b",
    license: "MIT",
    entry: None,
};

/// Object removal: MI-GAN (Picsart), inpainting at 512 px.
pub const INPAINT: ModelInfo = ModelInfo {
    label: "Entfernen (MI-GAN)",
    file: "migan.onnx",
    url: "https://huggingface.co/andraniksargsyan/migan/resolve/406830d0fa60666da0071c342ad2fbc8f30c5c64/migan.onnx",
    bytes: 29_546_882,
    sha256: "593eba0b7e04730f1b61c0a3cbca68d97d8d6a7ff5c6a44a7b9d7fcd880fc5ae",
    license: "MIT",
    entry: None,
};

/// Face detection: YuNet (OpenCV Zoo).
pub const FACE_DETECT: ModelInfo = ModelInfo {
    label: "Gesichtserkennung (YuNet)",
    file: "face_detection_yunet_2023mar.onnx",
    url: "https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx",
    bytes: 232_589,
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    license: "MIT",
    entry: None,
};

/// Face recognition: SFace (OpenCV Zoo), 128-d embeddings of aligned faces.
pub const FACE_EMBED: ModelInfo = ModelInfo {
    label: "Gesichtsvergleich (SFace)",
    file: "face_recognition_sface_2021dec.onnx",
    url: "https://huggingface.co/opencv/face_recognition_sface/resolve/3d7082438a6e4551e840c9b2bb60b71e8da4b524/face_recognition_sface_2021dec.onnx",
    bytes: 38_696_353,
    sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    license: "Apache-2.0",
    entry: None,
};

/// Eye state: open-closed-eye-0001 (Intel OpenVINO Training Extensions), 32 x 32 eye crops.
pub const EYE_STATE: ModelInfo = ModelInfo {
    label: "Augen offen/geschlossen",
    file: "open_closed_eye.onnx",
    url: "https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx",
    bytes: 46_164,
    sha256: "4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593",
    license: "Apache-2.0",
    entry: None,
};

/// Subject selection: U²-Net-p (rembg's copy of the original weights).
pub const SUBJECT: ModelInfo = ModelInfo {
    label: "Motiv auswählen (U²-Net-p)",
    file: "u2netp.onnx",
    url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx",
    bytes: 4_574_861,
    sha256: "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8",
    license: "Apache-2.0",
    entry: None,
};

/// Object selection by clicks: MobileSAM's image encoder (Chaoning Zhang et al., Apache-2.0),
/// exported to ONNX with its preprocessing by Acly (krita-ai-diffusion).
pub const OBJECT_ENCODER: ModelInfo = ModelInfo {
    label: "Objekt auswählen (MobileSAM, Bild)",
    file: "mobile_sam_image_encoder.onnx",
    url: "https://huggingface.co/Acly/MobileSAM/resolve/0d3b403339b4674a82493d5e97964dd78089ddc8/mobile_sam_image_encoder.onnx",
    bytes: 28_157_093,
    sha256: "580f5fb648ea1062c0aabc26217aed56921985f03f0cbbd852bba81d760cc749",
    license: "Apache-2.0",
    entry: None,
};

/// The mask decoder of Segment Anything (Meta, Apache-2.0) for MobileSAM: one mask from clicks.
pub const OBJECT_DECODER: ModelInfo = ModelInfo {
    label: "Objekt auswählen (MobileSAM, Maske)",
    file: "sam_mask_decoder_single.onnx",
    url: "https://huggingface.co/Acly/MobileSAM/resolve/0d3b403339b4674a82493d5e97964dd78089ddc8/sam_mask_decoder_single.onnx",
    bytes: 16_501_323,
    sha256: "93915fc7c993ab9d59ab8c9ccd3bce37f7509c81ab4150a74abd4d2abbd8570d",
    license: "Apache-2.0",
    entry: None,
};

/// Depth maps: Depth Anything V2 Small (Lihe Yang et al., HKU and TikTok; the small model is
/// Apache-2.0, the larger ones are not), as exported to ONNX by onnx-community.
pub const DEPTH: ModelInfo = ModelInfo {
    label: "Tiefenkarte (Depth Anything V2)",
    file: "depth_anything_v2_small.onnx",
    url: "https://huggingface.co/onnx-community/depth-anything-v2-small/resolve/4472b7362082ad9968fee890ca0f1e5aca36b93d/onnx/model.onnx",
    bytes: 99_060_839,
    sha256: "afb6a5c28f3b6bf1618c6e43f02073ef9dfdc70e937502d51603e57b0a1df10c",
    license: "Apache-2.0",
    entry: None,
};

/// Folder of the downloaded models (`SILBERKORN_MODELS` overrides it).
pub fn models_dir() -> PathBuf {
    std::env::var_os("SILBERKORN_MODELS").map(PathBuf::from).unwrap_or_else(|| sk_core::paths::data_dir().join("models"))
}

pub fn path(m: &ModelInfo) -> PathBuf {
    models_dir().join(m.file)
}

/// Whether the model is on disk at its full size (a file left by an interrupted or older
/// download is not, and is downloaded again).
pub fn available(m: &ModelInfo) -> bool {
    std::fs::metadata(path(m)).is_ok_and(|md| md.is_file() && md.len() == m.bytes)
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
    // One download per file at a time (jobs run in parallel and may want the same model): two
    // writers on one `.part` file would leave a broken model behind.
    let lock = target_lock(target);
    let _only_one = lock.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::metadata(target).is_ok_and(|md| md.len() == m.bytes) {
        // Another job has just finished it (it was checked before it took its place).
        progress(1.0);
        return Ok(());
    }
    if let Some(entry) = m.entry {
        let data = crate::runtime::zip_entry(m.url, entry, m.bytes as usize, progress).with_context(|| format!("Download von {} fehlgeschlagen", m.label))?;
        anyhow::ensure!(data.len() as u64 == m.bytes, "Download von {} unvollständig ({} von {} Bytes)", m.label, data.len(), m.bytes);
        anyhow::ensure!(sha256(&data).eq_ignore_ascii_case(m.sha256), "{}: Prüfsumme stimmt nicht (die Datei wurde verändert oder beschädigt)", m.label);
        sk_core::safe_file::write(target, &data)?;
        progress(1.0);
        return Ok(());
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

fn target_lock(target: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    type Locks = std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Arc<std::sync::Mutex<()>>>>;
    static LOCKS: std::sync::LazyLock<Locks> = std::sync::LazyLock::new(Default::default);
    LOCKS.lock().unwrap_or_else(|e| e.into_inner()).entry(target.to_path_buf()).or_default().clone()
}

/// Bytes per request: small enough to arrive within the time limit of `runtime::http` also
/// on a slow connection.
const PART: u64 = 4 << 20;

/// Appends the rest of the file from byte `have` on, part by part (each part a request of its
/// own: a stalled connection ends with an error, and cancelling takes effect after each part).
fn fetch(m: &ModelInfo, part: &Path, have: u64, progress: &dyn Fn(f32)) -> Result<()> {
    let mut file = std::fs::File::options().create(true).append(true).open(part)?;
    let mut done = have;
    while done < m.bytes {
        let end = (done + PART).min(m.bytes);
        let mut resp = crate::runtime::http().get(m.url).header("Range", &format!("bytes={done}-{}", end - 1)).call()?;
        anyhow::ensure!(resp.status().as_u16() == 206, "Der Server unterstützt keine Teil-Downloads");
        // Never more than asked for, whatever the server sends.
        let data = resp.body_mut().with_config().limit(end - done + 1).read_to_vec()?;
        anyhow::ensure!(data.len() as u64 == end - done, "Unerwartete Antwort des Servers");
        file.write_all(&data)?;
        done = end;
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
        let m = ModelInfo { label: "Test", file: "m.onnx", url: "", bytes: 3, sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", license: "", entry: None };
        std::fs::write(&file, b"abc").unwrap();
        verify(&m, &file).unwrap();
        std::fs::write(&file, b"abd").unwrap();
        assert!(format!("{:#}", verify(&m, &file).unwrap_err()).contains("Prüfsumme"));
        std::fs::write(&file, b"ab").unwrap();
        assert!(format!("{:#}", verify(&m, &file).unwrap_err()).contains("unvollständig"));
        std::fs::remove_dir_all(dir).ok();
    }

    /// A web server on localhost that serves `data` in byte ranges; counts the requests.
    fn range_server(data: Vec<u8>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.onnx", listener.local_addr().unwrap());
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut range = None;
                for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                    let line = line.unwrap_or_default();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(r) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        let (a, b) = r.split_once('-').unwrap();
                        range = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()));
                    }
                }
                let (a, b) = range.unwrap_or((0, data.len() - 1));
                let body = &data[a..=b];
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{b}/{}\r\nConnection: close\r\n\r\n",
                    body.len(),
                    data.len()
                );
                let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body));
            }
        });
        (url, requests)
    }

    #[test]
    fn downloads_in_parts_and_resumes() {
        let data: Vec<u8> = (0..(PART as usize * 2 + 12345)).map(|i| (i * 7 % 251) as u8).collect();
        let (url, requests) = range_server(data.clone());
        let m = ModelInfo { label: "Test", file: "m.onnx", url: Box::leak(url.into_boxed_str()), bytes: data.len() as u64, sha256: Box::leak(sha256(&data).into_boxed_str()), license: "", entry: None };
        let dir = std::env::temp_dir().join(format!("sk-models-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("m.onnx");
        // Interrupted before: the first 1000 bytes are there.
        std::fs::write(dir.join("m.onnx.part"), &data[..1000]).unwrap();
        download_to(&m, &target, &|_| {}).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), data);
        assert!(!dir.join("m.onnx.part").exists());
        // Three parts of at most `PART` bytes, each a request of its own.
        assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 3);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn every_model_is_pinned() {
        for m in [DENOISE_BAYER, DENOISE_LINEAR, UPSCALE_X2, UPSCALE_X4, OBJECT_ENCODER, OBJECT_DECODER, INPAINT, FACE_DETECT, FACE_EMBED, EYE_STATE, SUBJECT, DEPTH] {
            assert!(m.bytes > 0 && m.sha256.len() == 64 && m.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}", m.label);
            assert!(!m.url.contains("/resolve/main/"), "{}: nicht auf einen Stand festgelegt", m.label);
        }
    }
}
