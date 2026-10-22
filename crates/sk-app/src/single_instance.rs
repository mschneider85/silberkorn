//! One Silberkorn at a time: two would overwrite each other's sidecars, catalog and settings. The
//! first holds a lock on a file in the data folder (the system releases it when the process
//! ends, also after a crash) and listens on a local port. A second start hands what it was
//! started with (a folder or session, or nothing) to the first, which opens it and comes to the
//! front, and ends.
//!
//! The port and a random key are in a file next to the lock; a message without the key is
//! ignored (another program on the machine could connect to the port).

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// What a second start asked for: the folder or session it was started with, if any.
pub type Request = Option<PathBuf>;

/// The lock and its folder, held while the app runs (taken away to release it before a restart).
static LOCK: Mutex<Option<(File, PathBuf)>> = Mutex::new(None);
/// Where the requests of later starts go, and the window to wake for them.
static INBOX: Mutex<Option<(Receiver<Request>, Option<egui::Context>)>> = Mutex::new(None);

fn lock_path(dir: &Path) -> PathBuf {
    dir.join("silberkorn.lock")
}

fn port_path(dir: &Path) -> PathBuf {
    dir.join("silberkorn.port")
}

/// Becomes the one instance (`true`), or hands `arg` to the running one (`false`: end now).
/// If the running one doesn't answer, this one runs as well rather than not at all.
pub fn claim(arg: Option<&Path>) -> bool {
    match claim_in(&sk_core::paths::data_dir(), arg) {
        Claim::First(rx) => {
            *INBOX.lock().unwrap_or_else(|e| e.into_inner()) = rx.map(|rx| (rx, None));
            true
        }
        Claim::HandedOver => false,
    }
}

/// How a start goes on.
enum Claim {
    /// Runs (the requests of later starts arrive here, if it could listen).
    First(Option<Receiver<Request>>),
    /// The running one took over.
    HandedOver,
}

fn claim_in(dir: &Path, arg: Option<&Path>) -> Claim {
    let _ = std::fs::create_dir_all(dir);
    let file = match File::options().create(true).truncate(false).write(true).open(lock_path(dir)) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("Sperrdatei nicht anlegbar ({e}): mehrere Instanzen möglich");
            return Claim::First(None);
        }
    };
    match file.try_lock() {
        Ok(()) => {
            let rx = listen(dir);
            *LOCK.lock().unwrap_or_else(|e| e.into_inner()) = Some((file, dir.to_path_buf()));
            Claim::First(rx)
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            // The first may still be starting (no port yet): a moment's patience.
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                match hand_over(dir, arg) {
                    Ok(()) => return Claim::HandedOver,
                    Err(e) if Instant::now() > until => {
                        log::warn!("Laufendes Silberkorn antwortet nicht ({e}): starte trotzdem");
                        return Claim::First(None);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        }
        Err(std::fs::TryLockError::Error(e)) => {
            log::warn!("Sperre nicht möglich ({e}): mehrere Instanzen möglich");
            Claim::First(None)
        }
    }
}

/// Sends `arg` to the running instance.
fn hand_over(dir: &Path, arg: Option<&Path>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(port_path(dir))?;
    let (port, key) = text.trim().split_once(' ').ok_or_else(|| std::io::Error::other("Portdatei unvollständig"))?;
    let port: u16 = port.parse().map_err(std::io::Error::other)?;
    let mut s = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(500))?;
    s.set_write_timeout(Some(Duration::from_secs(1)))?;
    // Absolute: the running one has another working directory.
    let arg = arg.map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
    writeln!(s, "{key}")?;
    writeln!(s, "{}", arg.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())?;
    s.flush()
}

/// Takes the requests of later starts (on a thread of its own); they arrive at the receiver.
fn listen(dir: &Path) -> Option<Receiver<Request>> {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("Kein Port für weitere Starts ({e})");
            return None;
        }
    };
    let addr = listener.local_addr().ok()?;
    let key = format!("{:016x}{:016x}", rand_u64(), rand_u64());
    if let Err(e) = sk_core::safe_file::write(&port_path(dir), format!("{} {key}\n", addr.port())) {
        log::warn!("Portdatei nicht schreibbar ({e})");
        return None;
    }
    let (tx, rx) = channel();
    std::thread::Builder::new().name("weitere-starts".into()).spawn(move || serve(listener, key, tx)).ok()?;
    Some(rx)
}

fn serve(listener: TcpListener, key: String, tx: Sender<Request>) {
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut lines = BufReader::new(stream).lines();
        if lines.next().and_then(Result::ok).as_deref() != Some(key.as_str()) {
            continue;
        }
        let arg = lines.next().and_then(Result::ok).filter(|l| !l.is_empty()).map(PathBuf::from);
        if tx.send(arg).is_err() {
            return;
        }
        if let Some((_, Some(ctx))) = &*INBOX.lock().unwrap_or_else(|e| e.into_inner()) {
            ctx.request_repaint();
        }
    }
}

/// The window to wake when a request comes.
pub fn set_context(ctx: &egui::Context) {
    if let Some((_, c)) = &mut *INBOX.lock().unwrap_or_else(|e| e.into_inner()) {
        *c = Some(ctx.clone());
    }
}

/// The requests that came since the last call.
pub fn take_requests() -> Vec<Request> {
    INBOX.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(rx, _)| rx.try_iter().collect()).unwrap_or_default()
}

/// Gives the lock up (before starting anew, so that the new process becomes the instance).
pub fn release() {
    if let Some((f, dir)) = LOCK.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = std::fs::remove_file(port_path(&dir));
        let _ = f.unlock();
    }
}

/// A random number from the system's hash seeds (enough for a key nobody can guess).
fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.write_u32(std::process::id());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second claim in the same folder hands its folder to the first.
    #[test]
    fn a_second_start_hands_over_its_folder() {
        let dir = std::env::temp_dir().join(format!("sk-instance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Claim::First(Some(rx)) = claim_in(&dir, None) else { panic!("die erste") };
        let photos = dir.join("Fotos");
        assert!(matches!(claim_in(&dir, Some(&photos)), Claim::HandedOver), "die zweite reicht weiter");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(photos));
        // A message without the key is not taken.
        let text = std::fs::read_to_string(port_path(&dir)).unwrap();
        let port: u16 = text.split(' ').next().unwrap().parse().unwrap();
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        writeln!(s, "falsch\n/etc").unwrap();
        drop(s);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        release();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
