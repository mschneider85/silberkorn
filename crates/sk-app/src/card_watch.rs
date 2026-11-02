//! Memory cards: a drive (or a medium in a card reader) with a `DCIM` folder that appears while
//! the app runs opens the import, as in Lightroom and Capture One. Cards that are already there at
//! the start don't (the dialog would come with every start while the card stays in the reader).
//! Only drives with a `DCIM` folder count: phones without a drive letter don't, CFexpress readers
//! that report themselves as hard disks do.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::app::App;

/// How often the drives are looked at.
const INTERVAL: Duration = Duration::from_secs(2);

pub struct CardWatch {
    rx: Option<Receiver<Vec<PathBuf>>>,
    /// The cards there now (their drive roots), `None` until the first look.
    cards: Option<Vec<PathBuf>>,
    /// A new card waiting until no other dialog or export is in the way.
    pub pending: Option<PathBuf>,
}

impl CardWatch {
    /// Starts looking (in a thread of its own: a drive can take a moment to answer).
    pub fn start(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new().name("card-watch".into()).spawn(move || {
            quiet_drive_errors();
            let mut last = None;
            loop {
                let now = cards();
                if last.as_ref() != Some(&now) {
                    if tx.send(now.clone()).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    last = Some(now);
                }
                std::thread::sleep(INTERVAL);
            }
        });
        Self { rx: started.is_ok().then_some(rx), cards: None, pending: None }
    }

    /// Not looking (tests, screenshots).
    pub fn off() -> Self {
        Self { rx: None, cards: None, pending: None }
    }

    /// Whether `source` is on a memory card: the files must be copied, they leave with the card.
    pub fn holds(&self, source: &Path) -> bool {
        self.cards.as_ref().is_some_and(|cards| cards.iter().any(|root| source.starts_with(root)))
    }

    /// The cards that came since the last call, and those that went.
    fn poll(&mut self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let Some(rx) = &self.rx else { return Default::default() };
        let (mut came, mut went) = (Vec::new(), Vec::new());
        while let Ok(now) = rx.try_recv() {
            let (c, w) = changes(self.cards.as_deref(), &now);
            came.retain(|p| !w.contains(p));
            went.retain(|p| !c.contains(p));
            came.extend(c);
            went.extend(w);
            self.cards = Some(now);
        }
        (came, went)
    }
}

/// What came and went from `before` to `now`; nothing comes with the first look.
fn changes(before: Option<&[PathBuf]>, now: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(before) = before else { return Default::default() };
    let came = now.iter().filter(|p| !before.contains(p)).cloned().collect();
    let went = before.iter().filter(|p| !now.contains(p)).cloned().collect();
    (came, went)
}

/// The `DCIM` folder of a card.
pub fn dcim(root: &Path) -> PathBuf {
    root.join("DCIM")
}

fn is_card(root: &Path) -> bool {
    dcim(root).is_dir()
}

/// Drive roots with a `DCIM` folder. Network drives and CD drives are left out (an unreachable
/// network drive can take long to answer).
#[cfg(windows)]
fn cards() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const REMOVABLE: u32 = 2;
    const FIXED: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| {
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            matches!(unsafe { GetDriveTypeW(wide.as_ptr()) }, REMOVABLE | FIXED)
        })
        .map(PathBuf::from)
        .filter(|root| is_card(root))
        .collect()
}

/// Mounted media (Linux under /media and /run/media, macOS under /Volumes) with a `DCIM` folder.
#[cfg(not(windows))]
fn cards() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut bases = vec![PathBuf::from("/media"), PathBuf::from("/Volumes")];
    if !user.is_empty() {
        bases.push(Path::new("/media").join(&user));
        bases.push(Path::new("/run/media").join(&user));
    }
    let mut found: Vec<PathBuf> = bases
        .iter()
        .filter_map(|b| std::fs::read_dir(b).ok())
        .flat_map(|d| d.flatten().map(|e| e.path()))
        .filter(|p| is_card(p))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// A card reader without a card must not bring up "Es befindet sich kein Datenträger im
/// Laufwerk" while it is looked at.
#[cfg(windows)]
fn quiet_drive_errors() {
    use windows_sys::Win32::System::Diagnostics::Debug::SetThreadErrorMode;
    const SEM_FAILCRITICALERRORS: u32 = 1;
    unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, std::ptr::null_mut()) };
}

#[cfg(not(windows))]
fn quiet_drive_errors() {}

impl App {
    /// Each frame: a new card opens the import (when wanted), as soon as no other dialog and no
    /// export is in the way; a card that is taken out closes its import.
    pub(crate) fn pump_card_watch(&mut self, ctx: &egui::Context) {
        let (came, went) = self.card_watch.poll();
        if !went.is_empty() {
            if self.card_watch.pending.as_ref().is_some_and(|p| went.contains(p)) {
                self.card_watch.pending = None;
            }
            if self.import.as_ref().is_some_and(|d| went.iter().any(|root| d.source.starts_with(root))) {
                self.import = None;
                self.status = "Speicherkarte entfernt".into();
            }
        }
        if self.import_options.open_on_card
            && let Some(root) = came.into_iter().last()
        {
            self.status = format!("Speicherkarte erkannt: {}", root.display());
            self.card_watch.pending = Some(root);
        }
        let blocked = ctx.memory(|m| m.top_modal_layer()).is_some() || self.export_progress.is_some() || self.import.is_some();
        if !blocked && let Some(root) = self.card_watch.pending.take() {
            self.open_import(dcim(&root), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cards_that_come_later_count() {
        let (a, b) = (PathBuf::from("E:\\"), PathBuf::from("F:\\"));
        assert_eq!(changes(None, std::slice::from_ref(&a)), (vec![], vec![]), "beim Start schon da");
        assert_eq!(changes(Some(std::slice::from_ref(&a)), &[a.clone(), b.clone()]), (vec![b.clone()], vec![]));
        assert_eq!(changes(Some(&[a.clone(), b.clone()]), std::slice::from_ref(&b)), (vec![], vec![a]));
    }

    /// From a card the files are copied even with "Nicht kopieren" chosen; the choice stays.
    #[test]
    fn a_card_is_always_copied() {
        use crate::import::{ImportOptions, Target};
        let o = ImportOptions::default();
        assert_eq!((o.target, o.target_for(true), o.target_for(false)), (Target::InPlace, Target::Library, Target::InPlace));
        let o = ImportOptions { target: Target::Folder, ..Default::default() };
        assert_eq!(o.target_for(true), Target::Folder);
        assert!(ImportOptions::default().open_on_card, "standardmäßig an");
    }
}
