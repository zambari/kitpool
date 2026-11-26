//! Is an Elektron machine plugged in? A background thread looks at the MIDI
//! ports every [`SCAN_EVERY`], independent of the device worker (which may be
//! busy with a long transfer), so the UI can dim device features the moment
//! the cable comes out and light them again when it goes back in.
//!
//! This only looks at port names; it never talks to the device.
//!
//! Every unit plugged in is listed ([`units`]); one of them is *active*
//! ([`active_unit`]): the one the app shows and sends requests to.
//! [`presence`] describes the active one. Each unit has its own worker and
//! link (see [`super::Device`]), so switching leaves the other one's jobs
//! running. Not saved: a fresh launch starts on the first unit.
//!
//! The user can also switch a unit off (the switch on its name): then
//! [`presence`] says [`Presence::Off`] while it is active, and its worker
//! drops its MIDI ports, so a DAW can have the device to itself.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

const SCAN_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Presence {
    /// No scan has finished yet. Treated as present, so nothing flickers dim
    /// at startup.
    #[default]
    Unknown,
    /// No Elektron-looking MIDI port.
    Absent,
    /// The active unit is plugged in, by port name.
    One(String),
    /// Switched off by the user: no connection, until switched back on.
    Off,
}

impl Presence {
    /// Device features should look usable.
    pub fn available(&self) -> bool {
        !matches!(self, Presence::Absent | Presence::Off)
    }

    /// Hover text for anything drawn dimmed or in error because of this.
    pub fn problem(&self) -> Option<String> {
        match self {
            Presence::Absent => Some(
                "No Elektron device found on USB MIDI. Plug it in (USB config: Overbridge or USB MIDI); \
                 this is checked every 2 seconds."
                    .into(),
            ),
            Presence::Off => Some(
                "Device switched off: no MIDI connection, so other apps (a DAW) can use it. \
                 Click its name to connect again."
                    .into(),
            ),
            _ => None,
        }
    }
}

/// Everything the scan and the workers know about the units, by MIDI
/// input name.
#[derive(Default)]
struct Units {
    /// A scan has finished.
    scanned: bool,
    /// Elektron-looking inputs at the last scan, sorted.
    present: Vec<String>,
    /// The unit the app shows and sends requests to.
    active: Option<String>,
    /// Units switched off by the user.
    off: HashSet<String>,
    /// Units whose worker is busy with a job (see [`unit_busy`]).
    busy: HashSet<String>,
    /// Port discovery alone does not establish that the device can
    /// communicate: each worker's last word on its link.
    connection: HashMap<String, Result<String, String>>,
    /// Input and output each worker has open.
    claimed: HashMap<String, (String, String)>,
    /// Every MIDI input and output at the last scan, for the About box.
    ports: (Vec<String>, Vec<String>),
}

static UNITS: LazyLock<Mutex<Units>> = LazyLock::new(Default::default);

fn with<T>(f: impl FnOnce(&mut Units) -> T) -> T {
    let mut u = UNITS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut u)
}

/// `unit`'s link: its name, or why there is none.
pub fn unit_connection(unit: &str) -> Option<Result<String, String>> {
    with(|u| u.connection.get(unit).cloned())
}

pub(super) fn set_connection_status(unit: &str, status: Result<String, String>) {
    with(|u| u.connection.insert(unit.to_owned(), status));
}

/// The unit the app shows and sends requests to, if any.
pub fn active_unit() -> Option<String> {
    with(|u| u.active.clone())
}

/// Elektron-looking units at the last scan, sorted by port name.
pub fn units() -> Vec<String> {
    with(|u| u.present.clone())
}

/// Make `unit` the one requests go to. Its worker keeps its own link, so
/// the unit left keeps whatever it was doing.
pub(super) fn set_active(unit: Option<String>) {
    with(|u| u.active = unit);
}

/// `unit`'s worker is running a job or project read (and has more
/// queued), for the device rows.
pub fn unit_busy(unit: &str) -> bool {
    with(|u| u.busy.contains(unit))
}

/// Whether that changed anything.
pub(super) fn set_busy(unit: &str, busy: bool) -> bool {
    with(|u| {
        if busy {
            u.busy.insert(unit.to_owned())
        } else {
            u.busy.remove(unit)
        }
    })
}

/// A unit's name for the UI: its port name without the maker.
pub fn short_name(unit: &str) -> &str {
    let t = unit.trim();
    t.strip_prefix("Elektron ").unwrap_or(t)
}

/// Every MIDI port name seen by the last scan, for the About box.
#[derive(Clone, Default)]
pub struct MidiPorts {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// The input and output the active unit's worker has open, if any.
    pub claimed: Option<(String, String)>,
    /// Elektron-looking inputs, one per unit; each pairs with the output of
    /// the same name (`Conn::open_port`).
    pub devices: Vec<String>,
}

/// All MIDI ports from the last scan, and the ones in use.
pub fn midi_ports() -> MidiPorts {
    with(|u| MidiPorts {
        devices: u.present.clone(),
        inputs: u.ports.0.clone(),
        outputs: u.ports.1.clone(),
        claimed: u.active.as_ref().and_then(|a| u.claimed.get(a).cloned()),
    })
}

/// `unit`'s worker opened (`Some`) or closed (`None`) its ports.
pub(super) fn set_claimed(unit: &str, ports: Option<(String, String)>) {
    with(|u| match ports {
        Some(p) => u.claimed.insert(unit.to_owned(), p),
        None => u.claimed.remove(unit),
    });
}

/// The active unit, as the UI should treat it.
pub fn presence() -> Presence {
    with(|u| {
        if !u.scanned {
            return Presence::Unknown;
        }
        match &u.active {
            Some(a) if u.off.contains(a) => Presence::Off,
            Some(a) if u.present.contains(a) => Presence::One(a.clone()),
            _ => Presence::Absent,
        }
    })
}

/// The user switched the active unit off.
pub fn switched_off() -> bool {
    with(|u| u.active.as_ref().is_some_and(|a| u.off.contains(a)))
}

/// The user switched `unit` off.
pub fn unit_off(unit: &str) -> bool {
    with(|u| u.off.contains(unit))
}

/// Switch `unit` off (the caller tells its worker to let go of the ports)
/// or back on (scan right away, so the answer is fresh this frame).
pub(super) fn set_off(unit: &str, off: bool) {
    with(|u| {
        if off {
            u.off.insert(unit.to_owned());
        } else {
            u.off.remove(unit);
        }
        u.connection.remove(unit);
    });
    crate::log::line(format!("device: {unit} switched {}", if off { "off" } else { "on" }));
    if !off {
        rescan();
    }
}

/// Start the scan loop. Repaints the UI whenever the answer changes.
pub fn start(ctx: egui::Context) {
    let _ = std::thread::Builder::new()
        .name("device presence".into())
        .spawn(move || {
            loop {
                if rescan() {
                    ctx.request_repaint();
                }
                std::thread::sleep(SCAN_EVERY);
            }
        });
}

/// Scan now unless a scan has finished, so a request sent at startup goes
/// to the unit plugged in rather than to "none found".
pub(super) fn ensure_scanned() {
    if !with(|u| u.scanned) {
        rescan();
    }
}

/// Scan and store the answer; whether it changed. Only port names are
/// looked at, so switched-off units are left alone.
fn rescan() -> bool {
    let (all_in, all_out) = scan();
    let mut names: Vec<String> = all_in.iter().filter(|n| super::conn::elektron_port(n)).cloned().collect();
    names.sort();
    names.dedup();
    with(|u| {
        let changed = !u.scanned || u.present != names;
        // A unit plugged back in: its worker's last word is about the link
        // that broke ("port went away"). A parked unit gets no request to
        // reconnect until it's shown, so that word would stay up as NOT
        // CONNECTED; forget it, and let the next request tell.
        for n in names.iter().filter(|n| !u.present.contains(n)) {
            u.connection.remove(n);
        }
        u.scanned = true;
        u.ports = (all_in, all_out);
        u.present = names;
        // Nothing picked yet, or the active unit was unplugged while
        // another is still there: use the first one present.
        let gone = u.active.as_ref().is_none_or(|a| !u.present.contains(a));
        if gone {
            if let Some(first) = u.present.first() {
                if u.active.as_ref() != Some(first) {
                    crate::log::line(format!("device: active unit {first}"));
                    u.active = Some(first.clone());
                }
            }
        }
        if changed {
            crate::log::line(format!("device: units {:?}, active {:?}", u.present, u.active));
        }
        changed
    })
}

/// Every MIDI input and output name. A fresh client each time: a
/// long-lived one may not see ports come and go without a run loop.
fn scan() -> (Vec<String>, Vec<String>) {
    let all_in: Vec<String> = midir::MidiInput::new("KitPool presence")
        .map(|i| i.ports().iter().filter_map(|p| i.port_name(p).ok()).collect())
        .unwrap_or_default();
    let all_out: Vec<String> = midir::MidiOutput::new("KitPool presence")
        .map(|o| o.ports().iter().filter_map(|p| o.port_name(p).ok()).collect())
        .unwrap_or_default();
    (all_in, all_out)
}
