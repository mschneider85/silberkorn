//! Last line of defense against bugs: a panic in a background thread (a broken raw file, a bug in
//! the pipeline) must not silently kill the worker for the rest of the session. Worker threads
//! catch panics and keep serving, every panic is logged to `panic.log`, and the app shows it in
//! the status bar.
//!
//! What ends the process without a panic leaves traces too: running out of memory is written to
//! `panic.log` before Rust aborts (see `GuardedAlloc` in main.rs), warnings and errors go to
//! `silberkorn.log` (the release build on Windows has no console), and a marker file tells the
//! next start that the last session did not end normally.

use std::fs::File;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Panics not shown to the user yet.
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `panic.log` is started anew beyond this size.
const LOG_LIMIT: u64 = 1 << 20;

/// `panic.log`, opened at the start: writing to it when memory has run out must not allocate.
static CRASH_LOG: OnceLock<File> = OnceLock::new();

/// The last session ended without closing the app (see [`mark_running`]).
static PREVIOUS_CRASH: AtomicBool = AtomicBool::new(false);

/// The window's graphics device was lost (driver reset, waking from hibernation, video memory
/// exhausted). egui-wgpu can't go on with it and panics on its next frame; `main` then starts
/// the app anew instead of ending it.
static DISPLAY_LOST: AtomicBool = AtomicBool::new(false);

pub fn set_display_lost() {
    DISPLAY_LOST.store(true, Ordering::Relaxed);
}

pub fn display_lost() -> bool {
    DISPLAY_LOST.load(Ordering::Relaxed)
}

/// Logs every panic (with thread, location and backtrace) and remembers it for [`take_panics`];
/// the default output on stderr stays.
pub fn install_hook() {
    let dir = sk_core::paths::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(f) = File::options().create(true).append(true).open(dir.join("panic.log")) {
        let _ = CRASH_LOG.set(f);
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("unbenannt").to_string();
        let message = message(info.payload());
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("Absturz in Thread „{thread}“ ({place}): {message}");
        write_log(&format!("{} Thread „{thread}“ ({place}): {message}\n{backtrace}\n", timestamp()));
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(format!("Interner Fehler in „{thread}“: {message}"));
        default(info);
    }));
}

/// Called when an allocation failed, right before Rust aborts the process: one line in
/// `panic.log`, written without allocating.
pub fn out_of_memory(size: usize) {
    let Some(mut f) = CRASH_LOG.get() else { return };
    let mut buf = [0u8; 20];
    let mut n = size;
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let _ = f.write_all(b"Speicher erschoepft: Anforderung von ");
    let _ = f.write_all(&buf[i..]);
    let _ = f.write_all(b" Bytes abgelehnt, Silberkorn wird beendet\n");
}

/// Marks the session as running; true when the last one did not end normally (the marker was
/// still there).
pub fn mark_running() -> bool {
    let marker = sk_core::paths::data_dir().join("running");
    let crashed = marker.exists();
    PREVIOUS_CRASH.store(crashed, Ordering::Relaxed);
    let _ = std::fs::write(&marker, timestamp());
    crashed
}

/// The session ends normally.
pub fn mark_clean_exit() {
    let _ = std::fs::remove_file(sk_core::paths::data_dir().join("running"));
}

/// Whether the last session ended unexpectedly (once).
pub fn take_previous_crash() -> bool {
    PREVIOUS_CRASH.swap(false, Ordering::Relaxed)
}

/// Writes log messages to stderr and to `silberkorn.log` (started anew beyond 1 MB).
pub fn log_target() -> Box<dyn Write + Send> {
    let path = sk_core::paths::data_dir().join("silberkorn.log");
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT);
    let file = File::options().create(true).append(!too_big).write(true).truncate(too_big).open(&path).ok();
    Box::new(Tee { file })
}

struct Tee {
    file: Option<File>,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(f) = &mut self.file {
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = &mut self.file {
            let _ = f.flush();
        }
        Ok(())
    }
}

/// Panics since the last call, as messages for the user.
pub fn take_panics() -> Vec<String> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Runs a worker loop in a named thread. If `body` panics, it is started again (the job that
/// caused the panic is lost, later ones are served); it ends when `body` returns.
pub fn spawn(name: &str, mut body: impl FnMut() + Send + 'static) {
    let spawned = std::thread::Builder::new().name(name.to_string()).spawn(move || {
        while std::panic::catch_unwind(AssertUnwindSafe(&mut body)).is_err() {}
    });
    if let Err(e) = spawned {
        log::error!("Thread „{name}“ kann nicht gestartet werden: {e}");
    }
}

/// Runs `f`; a panic becomes an error (for one-off jobs whose caller waits for a result).
pub fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| format!("Interner Fehler: {}", message(&*p)))
}

fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unbekannte Ursache".into())
}

fn write_log(text: &str) {
    let dir = sk_core::paths::data_dir();
    let path = dir.join("panic.log");
    let _ = std::fs::create_dir_all(&dir);
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT);
    let file = std::fs::File::options().create(true).append(!too_big).write(true).truncate(too_big).open(&path);
    if let Ok(mut f) = file {
        let _ = f.write_all(text.as_bytes());
    }
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("[{secs}]")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

    #[test]
    fn worker_survives_a_panicking_job() {
        let (tx, rx) = mpsc::channel::<u32>();
        let (done_tx, done_rx) = mpsc::channel::<u32>();
        let served = Arc::new(AtomicUsize::new(0));
        let s = served.clone();
        super::spawn("test-worker", move || {
            while let Ok(n) = rx.recv() {
                assert!(n != 2, "Job 2 ist kaputt");
                s.fetch_add(1, Ordering::SeqCst);
                let _ = done_tx.send(n);
            }
        });
        for n in 1..=3 {
            tx.send(n).unwrap();
        }
        let got: Vec<u32> = (0..2).map(|_| done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).collect();
        assert_eq!(got, [1, 3]);
        assert_eq!(served.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn catch_turns_a_panic_into_an_error() {
        assert_eq!(super::catch(|| 5), Ok(5));
        let err = super::catch(|| -> u32 { panic!("kaputt") }).unwrap_err();
        assert!(err.contains("kaputt"));
    }
}
