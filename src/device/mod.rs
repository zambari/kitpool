//! An Elektron machine's +Drive (Digitakt, Digitakt II, Analog Rytm,
//! Model:Samples), browsed over USB MIDI.
//!
//! Each unit plugged in gets its own worker thread, which owns that unit's
//! MIDI connection and runs its requests in order: a device answers one API
//! call at a time, so there is nothing to gain from parallel requests to
//! one unit and a lot to lose. Requests go to the active unit
//! ([`active_unit`]); switching leaves the other's queue running, and its
//! events come back tagged ([`Ev::FromUnit`]). The UI posts [`Cmd`]s and
//! gets results back as ordinary [`crate::jobs::Ev`]s. While a job runs, the
//! worker answers folder listings between its steps (down to single transfer
//! chunks) and read-only preset / project browser requests so browsing
//! stays responsive. Project reads choose their next request from the
//! current device page's priority; writes wait their turn.
//!
//! Device paths are plain absolute paths (`/samples/kicks`) stored in
//! `PathBuf`s; they never touch the local filesystem.

mod conn;
mod elek;
pub use elek::{file, library, machines, patterns, presets, preset_file, Cell, DumpMsg, ElekCmd, ElekEv, ElekProject, PatState, PoolEntry, ReadCtl, ReadFirst, SavedProject};
mod presence;
pub use presence::{MidiPorts, Presence, active_unit, midi_ports, presence, short_name, start as watch_presence, switched_off, unit_busy, unit_connection, unit_off, units};
mod proto;
pub(crate) mod sample;
mod transfer;
mod worker;

use crate::jobs::{Emitter, Ev};
use crate::ops::Policy;
use conn::{Conn, entries, port_present, rename_checked};
use proto::{Args, DirEntry, Reader};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};
use worker::{Worker, serve_lists};

/// Where elk-herd (and we) put deleted items, so a stray keypress is undoable.
pub const TRASH: &str = "/trash";

/// Allow a busy device time to react without replaying a timed-out write.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Times one read chunk is asked for before the file counts as failed.
const CHUNK_TRIES: u32 = 4;
/// Times a whole file is read before it is reported as failed.
const FILE_TRIES: u32 = 3;
const PROGRESS_EVERY: u64 = 64 * 1024;
/// How often to check that the device is still plugged in.
const PROBE_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    /// Device to device, by rename. Instant.
    Move,
    /// Local file or folder to the device.
    Upload,
    /// Device to a local folder, as WAV.
    Download,
    /// Device to device, by reading and rewriting each file.
    Copy,
}

pub enum Task {
    Transfer {
        dir: Dir,
        /// `(source, destination folder)`.
        items: Vec<(PathBuf, PathBuf)>,
        policy: Policy,
        /// Fold uploads to mono this way.
        mono: Option<crate::audio::stats::Fold>,
        /// Best guess for the progress bar.
        total_bytes: u64,
        /// Downloads only: the local file name to write a source under, in
        /// place of its own (e.g. with a pool slot number in front).
        local_names: HashMap<PathBuf, String>,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Mkdir {
        path: PathBuf,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
    /// Delete outright, folders with everything in them.
    Delete {
        paths: Vec<PathBuf>,
    },
}

enum Cmd {
    List {
        path: PathBuf,
        generation: u64,
    },
    Job {
        job: u64,
        cancel: Arc<AtomicBool>,
        task: Task,
    },
    Elek(ElekCmd),
    /// Close the MIDI ports (the device was switched off).
    Release,
}

/// The device workers, one per unit (by MIDI input name), started when a
/// unit is first asked for something. `None` is the worker used while no
/// unit is plugged in: it answers every request with "not found".
pub struct Device {
    em: Emitter,
    workers: std::cell::RefCell<HashMap<Option<String>, Sender<Cmd>>>,
}

impl Device {
    pub fn new(em: Emitter) -> Self {
        Self { em, workers: Default::default() }
    }

    /// Queue `cmd` on `unit`'s worker, starting it if needed.
    fn send_to(&self, unit: Option<String>, cmd: Cmd) {
        let mut workers = self.workers.borrow_mut();
        let tx = workers.entry(unit.clone()).or_insert_with(|| spawn_worker(unit, &self.em));
        let _ = tx.send(cmd);
    }

    fn send(&self, cmd: Cmd) {
        presence::ensure_scanned();
        self.send_to(active_unit(), cmd);
    }

    pub fn list(&self, path: PathBuf, generation: u64) {
        self.send(Cmd::List { path, generation });
    }

    pub fn run(&self, job: u64, cancel: Arc<AtomicBool>, task: Task) {
        self.send(Cmd::Job { job, cancel, task });
    }

    /// Project dump work (see [`elek`]): runs in turn with other jobs.
    pub fn elek(&self, cmd: ElekCmd) {
        self.send(Cmd::Elek(cmd));
    }

    /// Switch the active unit off or on. Off closes its MIDI ports and
    /// refuses every request until it is switched on again.
    pub fn switch_off(&self, off: bool) {
        let Some(unit) = active_unit() else { return };
        presence::set_off(&unit, off);
        if off {
            self.send_to(Some(unit), Cmd::Release);
        }
    }

    /// Show and talk to `unit` (one of [`units`]) from now on. Its worker
    /// keeps its own link and queue, as does the one left.
    pub fn activate(&self, unit: String) {
        crate::log::line(format!("device: active unit {unit}"));
        presence::set_active(Some(unit));
    }

    /// Send requests to `unit` for a moment, without the UI switching
    /// (the app applying a parked unit's events); put back afterwards.
    pub fn route_to(&self, unit: Option<String>) {
        presence::set_active(unit);
    }
}

fn spawn_worker(unit: Option<String>, em: &Emitter) -> Sender<Cmd> {
    let (tx, rx) = channel();
    let em = em.for_unit(unit.clone());
    let name = format!("device {}", unit.as_deref().unwrap_or("(none)"));
    let _ = std::thread::Builder::new().name(name).spawn(move || {
        Worker {
            unit,
            conn: None,
            em,
            job: None,
            rx,
            later: VecDeque::new(),
            counted: HashMap::new(),
            counting: false,
            busy: Vec::new(),
            held: VecDeque::new(),
            redirects: Vec::new(),
            merged: (0, 0, 0),
        }
        .run()
    });
    tx
}

/// What can go wrong talking to the device. A link error drops the
/// connection so the next request reconnects.
enum Fail {
    Link(String),
    Refused(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Link(s) | Fail::Refused(s) => f.write_str(s),
        }
    }
}

type R<T> = Result<T, Fail>;

fn refused<T>(what: impl Into<String>) -> R<T> {
    Err(Fail::Refused(what.into()))
}

/// A device path as the device (and the user) expect it: rooted, with `/`
/// separators. On Windows, `Path::join` inserts `\`, which the device
/// would read as part of a file name.
/// Error suffix for a read of the device's factory content, which it
/// won't let anything copy off. The app spots it to explain once per batch.
pub const FACTORY_LOCKED: &str = "factory samples can't be copied off the device";

/// Under the device's `/factory` folder.
pub fn is_factory(p: &Path) -> bool {
    dev_str(p)
        .split('/')
        .find(|s| !s.is_empty())
        .is_some_and(|s| s.eq_ignore_ascii_case("factory"))
}

pub fn dev_str(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        s
    } else {
        format!("/{s}")
    }
}

/// Name for a file going to the device: the stem, since the device has no use
/// for `.wav`.
fn device_name(local: &Path) -> String {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if crate::fs_model::is_audio_path(local) {
        local
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(name)
    } else {
        name
    }
}

/// `name 2`, `name 3`, ... not in `taken`.
fn unique_name(taken: &HashSet<String>, name: &str) -> String {
    (2..10_000)
        .map(|n| format!("{name} {n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or_else(|| name.to_owned())
}

/// Where a download of `name` lands.
pub fn wav_name(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".wav") {
        name.to_owned()
    } else {
        format!("{name}.wav")
    }
}

/// Where an upload of `local` lands in `dir`.
pub fn upload_target(local: &Path, dir: &Path) -> PathBuf {
    dir.join(device_name(local))
}
