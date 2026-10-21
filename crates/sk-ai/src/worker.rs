//! ONNX Runtime and DirectML in a separate process. They are native code: a crash in them or in
//! the graphics driver below (an access violation, not a panic) would end the whole program.
//! In the separate process it ends only that; the networks loaded there go on with tract on the
//! processor (see `runtime::Net`), later ones start a new process. After [`MAX_CRASHES`] the
//! graphics card is not used for networks until the program is started again.
//!
//! There are two such processes ([`Lane`]): one for the networks that answer quickly, and one for
//! those that take many seconds per run (BiRefNet, 10–30 s on the processor), which would
//! otherwise hold up every other network: each process works through one request after the other.
//!
//! The process is the program itself, started with [`ARG`] (see [`set_command`]); without that
//! (tests, examples) ONNX Runtime runs in the calling process as before. Requests and replies go
//! through stdin and stdout, each a length (u64) and the message; numbers are little-endian.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::runtime::{Output, Tensor};

/// The argument that makes the program the worker process.
pub const ARG: &str = "--ki-prozess";

/// Crashes after which networks no longer go to ONNX Runtime in this session.
const MAX_CRASHES: u32 = 2;
/// A reply taking longer means the process hangs (a driver that doesn't come back); loading
/// compiles the network for the graphics card and may take a while.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Largest message accepted (the largest inputs are some tens of MB).
const MAX_MESSAGE: u64 = 2 << 30;

const LOAD: u8 = 1;
const RUN: u8 = 2;
const RELEASE: u8 = 3;
const OK: u8 = 0;
const ERR: u8 = 1;

/// How to start the worker process.
static COMMAND: OnceLock<(PathBuf, Vec<OsString>)> = OnceLock::new();
static CONNS: [Mutex<Option<Conn>>; 2] = [Mutex::new(None), Mutex::new(None)];
/// The generation of each lane's running process (0: none), readable while the other lane waits.
static LIVE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Which worker process a network runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    /// Networks that answer within a second or a few.
    Quick = 0,
    /// Networks that take many seconds per run.
    Long = 1,
}
/// Counts the started processes: sessions belong to one of them.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static CRASHES: AtomicU32 = AtomicU32::new(0);
/// Sessions no longer needed, freed with the next request.
static RELEASED: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

/// Networks run in a process started as `exe args` (the program itself with [`ARG`]).
pub fn set_command(exe: PathBuf, args: Vec<OsString>) {
    let _ = COMMAND.set((exe, args));
}

/// A worker process is set up.
pub(crate) fn configured() -> bool {
    COMMAND.get().is_some()
}

/// ONNX Runtime may still be used (it did not crash too often).
pub(crate) fn usable() -> bool {
    CRASHES.load(Ordering::Relaxed) < MAX_CRASHES
}

/// Ends the worker process (before an update replaces the program file it runs from).
pub fn shutdown() {
    for conn in &CONNS {
        if let Some(mut c) = conn.lock().unwrap_or_else(|e| e.into_inner()).take() {
            c.kill();
        }
    }
}

/// A network loaded in the worker process.
pub(crate) struct Session {
    lane: Lane,
    generation: u64,
    id: u32,
}

impl Drop for Session {
    fn drop(&mut self) {
        RELEASED.lock().unwrap_or_else(|e| e.into_inner()).push((self.generation, self.id));
    }
}

struct Conn {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    replies: Receiver<Vec<u8>>,
    lane: Lane,
    generation: u64,
    /// `runtime::config_generation` when started: the runtime it loaded may be outdated.
    config: u64,
}

impl Conn {
    fn start(lane: Lane, config: u64) -> Result<Conn> {
        let (exe, args) = COMMAND.get().context("Kein KI-Prozess eingerichtet")?;
        let mut cmd = Command::new(exe);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window for it.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().context("KI-Prozess nicht startbar")?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            bail!("KI-Prozess ohne Verbindung");
        };
        let (tx, replies) = std::sync::mpsc::channel();
        // The replies are read in their own thread so that waiting for one can time out. It ends
        // with the process (end of its output), which ends the channel.
        std::thread::Builder::new().name("KI-Prozess lesen".into()).spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Ok(m) = read_message(&mut r) {
                if tx.send(m).is_err() {
                    return;
                }
            }
        })?;
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        LIVE[lane as usize].store(generation, Ordering::Relaxed);
        log::info!("KI-Prozess gestartet ({lane:?})");
        Ok(Conn { child, stdin: BufWriter::new(stdin), replies, lane, generation, config })
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Sends a request and waits for its reply. `Err` only if the process failed (the caller
    /// then drops it); an error reported by it is an `Ok` reply starting with [`ERR`].
    fn request(&mut self, message: &[u8]) -> Result<Vec<u8>> {
        let released: Vec<u32> = {
            // Those of the other process stay for it; those of ended processes go.
            let mut r = RELEASED.lock().unwrap_or_else(|e| e.into_inner());
            let mine = r.iter().filter(|(g, _)| *g == self.generation).map(|(_, id)| *id).collect();
            let other = LIVE[1 - self.lane as usize].load(Ordering::Relaxed);
            r.retain(|(g, _)| *g == other);
            mine
        };
        let sent = released
            .iter()
            .try_for_each(|id| write_message(&mut self.stdin, &[&[RELEASE][..], &id.to_le_bytes()].concat()))
            .and_then(|_| write_message(&mut self.stdin, message))
            .and_then(|_| self.stdin.flush());
        if sent.is_err() {
            return Err(self.crashed());
        }
        match self.replies.recv_timeout(TIMEOUT) {
            Ok(reply) => Ok(reply),
            Err(RecvTimeoutError::Timeout) => bail!("KI-Prozess antwortet nicht"),
            Err(RecvTimeoutError::Disconnected) => Err(self.crashed()),
        }
    }

    /// The process ended by itself: how.
    fn crashed(&mut self) -> anyhow::Error {
        let status = self.child.wait().map(|s| format!(" ({s})")).unwrap_or_default();
        anyhow!("KI-Prozess abgestürzt{status}")
    }
}

/// Sends `message` to the current process (`generation`: only if it is still that one; `None`
/// starts one if needed). The process's generation and the reply without its status byte; an
/// error the process reported is returned as `Err` too.
fn request(lane: Lane, generation: Option<u64>, message: &[u8]) -> Result<(u64, Vec<u8>)> {
    let mut conn = CONNS[lane as usize].lock().unwrap_or_else(|e| e.into_inner());
    let config = crate::runtime::config_generation();
    match (&mut *conn, generation) {
        (Some(c), Some(g)) if c.generation != g => bail!("KI-Prozess wurde neu gestartet"),
        (None, Some(_)) => bail!("KI-Prozess wurde beendet"),
        // Installed or switched on since: its ONNX Runtime is outdated.
        (Some(c), None) if c.config != config => {
            c.kill();
            *conn = None;
        }
        _ => {}
    }
    if conn.is_none() {
        *conn = Some(Conn::start(lane, config)?);
    }
    let Some(c) = conn.as_mut() else { bail!("KI-Prozess fehlt") };
    match c.request(message) {
        Ok(reply) => match reply.split_first() {
            Some((&OK, rest)) => Ok((c.generation, rest.to_vec())),
            Some((&ERR, rest)) => bail!("{}", String::from_utf8_lossy(rest)),
            _ => bail!("Antwort des KI-Prozesses unlesbar"),
        },
        Err(e) => {
            c.kill();
            *conn = None;
            let n = CRASHES.fetch_add(1, Ordering::Relaxed) + 1;
            let msg = if n >= MAX_CRASHES {
                format!("{e:#}; die Grafikkarte wird bis zum nächsten Start nicht mehr für KI verwendet")
            } else {
                format!("{e:#}")
            };
            log::error!("{msg}");
            crate::runtime::set_gpu_error(Some(msg));
            Err(e)
        }
    }
}

/// Loads a network in the worker process: the session, whether it runs on the graphics card,
/// and why DirectML could not be used.
pub(crate) fn load(path: &Path, gpu: bool, lane: Lane) -> Result<(Session, bool, Option<String>)> {
    let mut m = vec![LOAD, gpu as u8];
    m.extend(path.to_string_lossy().as_bytes());
    let (generation, reply) = request(lane, None, &m)?;
    let mut r = Reader(&reply);
    let (id, gpu) = (r.u32()?, r.u8()? != 0);
    let dml_error = (r.u8()? != 0).then(|| String::from_utf8_lossy(r.rest()).into_owned());
    Ok((Session { lane, generation, id }, gpu, dml_error))
}

pub(crate) fn run(session: &Session, data: Vec<Tensor>) -> Result<Vec<Output>> {
    let mut m = vec![RUN];
    m.extend(session.id.to_le_bytes());
    encode_tensors(&mut m, &data);
    drop(data);
    let (_, reply) = request(session.lane, Some(session.generation), &m)?;
    let mut r = Reader(&reply);
    let n = r.u32()? as usize;
    (0..n)
        .map(|_| {
            let rank = r.u32()? as usize;
            let shape = (0..rank).map(|_| r.u64().map(|d| d as usize)).collect::<Result<Vec<_>>>()?;
            let len = r.u64()? as usize;
            let bytes = r.take(len.checked_mul(4).context("Antwort des KI-Prozesses unlesbar")?)?;
            let data = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
            Ok(Output { shape, data })
        })
        .collect()
}

fn encode_tensors(m: &mut Vec<u8>, data: &[Tensor]) {
    m.extend((data.len() as u32).to_le_bytes());
    for t in data {
        let (kind, shape) = match t {
            Tensor::F32(s, _) => (0u8, s),
            Tensor::U8(s, _) => (1u8, s),
        };
        m.push(kind);
        m.extend((shape.len() as u32).to_le_bytes());
        for d in shape {
            m.extend((*d as u64).to_le_bytes());
        }
        match t {
            Tensor::F32(_, v) => {
                m.extend((v.len() as u64).to_le_bytes());
                m.reserve(v.len() * 4);
                for x in v {
                    m.extend(x.to_le_bytes());
                }
            }
            Tensor::U8(_, v) => {
                m.extend((v.len() as u64).to_le_bytes());
                m.extend(v);
            }
        }
    }
}

fn decode_tensors(r: &mut Reader) -> Result<Vec<Tensor>> {
    let n = r.u32()? as usize;
    (0..n)
        .map(|_| {
            let kind = r.u8()?;
            let rank = r.u32()? as usize;
            let shape = (0..rank).map(|_| r.u64().map(|d| d as usize)).collect::<Result<Vec<_>>>()?;
            let len = r.u64()? as usize;
            Ok(match kind {
                0 => Tensor::F32(shape, r.take(len.checked_mul(4).context("Anfrage unlesbar")?)?.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()),
                1 => Tensor::U8(shape, r.take(len)?.to_vec()),
                k => bail!("Unbekannter Datentyp {k}"),
            })
        })
        .collect()
}

fn write_message(w: &mut impl Write, m: &[u8]) -> std::io::Result<()> {
    w.write_all(&(m.len() as u64).to_le_bytes())?;
    w.write_all(m)
}

fn read_message(r: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 8];
    r.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(std::io::Error::other("Nachricht zu groß"));
    }
    let mut m = vec![0u8; len as usize];
    r.read_exact(&mut m)?;
    Ok(m)
}

/// Reads numbers from a message; running past its end is an error.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(anyhow!("Nachricht des KI-Prozesses unvollständig"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

// ------------------------------------------------------------------ the worker process

/// The worker process: serves requests from stdin until it closes (the program ended).
pub fn serve() {
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = BufWriter::new(std::io::stdout().lock());
    let mut sessions: HashMap<u32, ort::session::Session> = HashMap::new();
    let mut next_id = 0u32;
    while let Ok(m) = read_message(&mut input) {
        let Some((&kind, body)) = m.split_first() else { continue };
        if kind == RELEASE {
            if let Ok(id) = Reader(body).u32() {
                sessions.remove(&id);
            }
            continue;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Vec<u8>> {
            match kind {
                LOAD => {
                    let (&gpu, path) = body.split_first().context("Anfrage unlesbar")?;
                    let path = PathBuf::from(String::from_utf8_lossy(path).into_owned());
                    let (session, gpu, dml_error) = crate::runtime::ort_session(&path, gpu != 0)?;
                    next_id += 1;
                    sessions.insert(next_id, session);
                    let mut reply = Vec::new();
                    reply.extend(next_id.to_le_bytes());
                    reply.push(gpu as u8);
                    reply.push(dml_error.is_some() as u8);
                    reply.extend(dml_error.unwrap_or_default().as_bytes());
                    Ok(reply)
                }
                RUN => {
                    let mut r = Reader(body);
                    let id = r.u32()?;
                    let data = decode_tensors(&mut r)?;
                    let session = sessions.get_mut(&id).context("Netz nicht geladen")?;
                    let outputs = crate::runtime::ort_run(session, data)?;
                    let mut reply = Vec::new();
                    reply.extend((outputs.len() as u32).to_le_bytes());
                    for o in outputs {
                        reply.extend((o.shape.len() as u32).to_le_bytes());
                        for d in &o.shape {
                            reply.extend((*d as u64).to_le_bytes());
                        }
                        reply.extend((o.data.len() as u64).to_le_bytes());
                        reply.reserve(o.data.len() * 4);
                        for x in o.data {
                            reply.extend(x.to_le_bytes());
                        }
                    }
                    Ok(reply)
                }
                k => bail!("Unbekannte Anfrage {k}"),
            }
        }));
        let reply = match result {
            Ok(Ok(body)) => [&[OK][..], &body].concat(),
            Ok(Err(e)) => [&[ERR][..], format!("{e:#}").as_bytes()].concat(),
            Err(_) => [&[ERR][..], "Interner Fehler im KI-Prozess".as_bytes()].concat(),
        };
        if write_message(&mut output, &reply).and_then(|_| output.flush()).is_err() {
            return;
        }
    }
}
