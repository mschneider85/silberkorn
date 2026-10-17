//! Updates from the GitHub releases: whether a newer version is out, and installing it (Windows).
//!
//! An update runs the release's installer silently. It installs for the current user only (in
//! `%LOCALAPPDATA%\Programs\Silberkorn`, see `installer/silberkorn.iss`), so it never needs
//! administrator rights; a copy run from elsewhere is replaced by the installed one. The installer
//! is checked against the SHA-256 that GitHub lists for it (or a `<asset>.sha256` file next to it)
//! before it runs; without a checksum it is refused.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};

/// `owner/name` of the repository; `SILBERKORN_UPDATE_REPO` overrides it (for testing).
pub const REPO: &str = "mschneider85/silberkorn";

/// The release's installer, which the update runs.
pub const SETUP_ASSET: &str = "silberkorn-setup-x64.exe";

/// Downloads larger than this are refused (the program is well below).
const MAX_BYTES: u64 = 512 << 20;
/// Downloaded in parts of this size, each with its own time limit (see `sk_ai::runtime::http`).
const PART: u64 = 8 << 20;

/// A published release.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Without the leading "v", e.g. "0.3.0".
    pub version: String,
    pub notes: String,
    /// The release's page on GitHub.
    pub page: String,
    pub asset: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    /// Lowercase hex, if GitHub lists it.
    pub sha256: Option<String>,
    /// URL of `<asset>.sha256`, if the release has one.
    pub sha256_url: Option<String>,
}

/// The version of this program.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "1.2.3" (with or without "v") as numbers; pre-release suffixes ("-rc.1") are ignored.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

/// Whether `candidate` is newer than `current`; unparseable versions never are.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

fn repo() -> String {
    std::env::var("SILBERKORN_UPDATE_REPO").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| REPO.to_string())
}

fn http() -> &'static ureq::Agent {
    static AGENT: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .user_agent(concat!("silberkorn/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    });
    &AGENT
}

/// The latest release (GitHub leaves out drafts and pre-releases).
pub fn latest() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
    let mut resp = http()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .context("GitHub nicht erreichbar")?;
    match resp.status().as_u16() {
        200 => {}
        404 => bail!("Noch keine Version veröffentlicht"),
        403 | 429 => bail!("GitHub hat zu viele Anfragen gezählt, bitte später erneut versuchen"),
        s => bail!("GitHub antwortet mit Fehler {s}"),
    }
    let text = resp.body_mut().with_config().limit(4 << 20).read_to_string()?;
    parse_release(&text, SETUP_ASSET)
}

/// The fields of GitHub's release JSON that are needed.
fn parse_release(json: &str, asset_name: &str) -> Result<Release> {
    let v: serde_json::Value = serde_json::from_str(json).context("Antwort von GitHub nicht lesbar")?;
    let tag = v["tag_name"].as_str().context("Antwort von GitHub ohne Versionsnummer")?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    ensure!(parse_version(&version).is_some(), "Unbekannte Versionsnummer „{tag}“");
    let assets = v["assets"].as_array().map(Vec::as_slice).unwrap_or_default();
    let url_of = |name: &str| assets.iter().find(|a| a["name"] == name).and_then(|a| a["browser_download_url"].as_str()).map(str::to_string);
    let asset = assets.iter().find(|a| a["name"] == asset_name).and_then(|a| {
        Some(Asset {
            url: a["browser_download_url"].as_str()?.to_string(),
            size: a["size"].as_u64()?,
            sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase),
            sha256_url: url_of(&format!("{asset_name}.sha256")),
        })
    });
    Ok(Release {
        version,
        notes: v["body"].as_str().unwrap_or_default().replace("\r\n", "\n").trim().to_string(),
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        asset,
    })
}

/// The expected checksum: GitHub's, else the first word of the `.sha256` file.
fn expected_sha256(asset: &Asset) -> Result<String> {
    if let Some(s) = &asset.sha256 {
        return Ok(s.clone());
    }
    let url = asset.sha256_url.as_ref().context("Die Version hat keine Prüfsumme und wird nicht installiert")?;
    let text = http().get(url).call()?.body_mut().with_config().limit(4096).read_to_string()?;
    let hash = text.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    ensure!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "Prüfsummendatei nicht lesbar");
    Ok(hash)
}

/// Where the installer is downloaded to.
fn setup_dir() -> PathBuf {
    std::env::temp_dir().join("silberkorn-update")
}

/// Downloads the installer of `release` and checks it; returns its path, for `run_setup`.
pub fn download_setup(release: &Release, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    if !cfg!(windows) {
        bail!("Automatische Updates gibt es nur unter Windows");
    }
    let asset = release.asset.as_ref().context("Die Version enthält keinen Installer für Windows")?;
    ensure!(asset.size > 0 && asset.size <= MAX_BYTES, "Unerwartete Dateigröße ({} Bytes)", asset.size);
    let expected = expected_sha256(asset)?;
    let data = download(&asset.url, asset.size, progress)?;
    ensure!(sha256(&data) == expected, "Prüfsumme stimmt nicht, das Update wird nicht installiert");
    let dir = setup_dir();
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("silberkorn-setup-{}.exe", release.version));
    std::fs::write(&file, &data).with_context(|| format!("{} nicht schreibbar", file.display()))?;
    Ok(file)
}

/// Starts the installer without questions; it waits for this program to close (call it right
/// before quitting) and shows only its progress. With `start_after` it starts the new version.
pub fn run_setup(setup: &Path, start_after: bool) -> Result<()> {
    std::process::Command::new(setup)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .arg(format!("/start={}", u8::from(start_after)))
        .spawn()
        .context("Installer konnte nicht gestartet werden")?;
    Ok(())
}

/// Removes the installer of an earlier update. It may still be running for a moment (it starts
/// the new version before it ends), so this tries a few times.
pub fn remove_leftovers() {
    let dir = setup_dir();
    if !dir.exists() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..15 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn download(url: &str, size: u64, progress: &dyn Fn(f32)) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(size as usize);
    while (data.len() as u64) < size {
        let start = data.len() as u64;
        let end = (start + PART).min(size) - 1;
        let mut resp = http().get(url).header("Range", format!("bytes={start}-{end}")).call().context("Download fehlgeschlagen")?;
        let part = resp.body_mut().with_config().limit(PART + 1).read_to_vec()?;
        match resp.status().as_u16() {
            206 => {}
            // The server ignored the range and sent the whole file.
            200 if start == 0 && part.len() as u64 == size => return Ok(part),
            s => bail!("Download fehlgeschlagen (Status {s})"),
        }
        ensure!(part.len() as u64 == end - start + 1, "Download unvollständig");
        data.extend_from_slice(&part);
        progress(data.len() as f32 / size as f32);
    }
    Ok(data)
}

fn sha256(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
    }

    #[test]
    fn reads_github_release() {
        let json = r#"{
            "tag_name": "v0.3.0", "html_url": "https://github.com/o/r/releases/tag/v0.3.0", "body": "Neu:\r\n- A\r\n",
            "assets": [
                {"name": "silberkorn-setup-x64.exe", "size": 1234, "digest": "sha256:ABCDEF",
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.3.0/silberkorn-setup-x64.exe"},
                {"name": "silberkorn-setup-x64.exe.sha256", "size": 90,
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.3.0/silberkorn-setup-x64.exe.sha256"}
            ]
        }"#;
        let r = parse_release(json, SETUP_ASSET).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.notes, "Neu:\n- A");
        let a = r.asset.unwrap();
        assert_eq!((a.size, a.sha256.as_deref()), (1234, Some("abcdef")));
        assert!(a.sha256_url.unwrap().ends_with(".exe.sha256"));
        assert!(parse_release(r#"{"tag_name": "v0.4.0", "assets": []}"#, SETUP_ASSET).unwrap().asset.is_none());
        assert!(parse_release(r#"{"tag_name": "latest"}"#, SETUP_ASSET).is_err());
    }
}
