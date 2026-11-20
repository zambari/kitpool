//! Is an Elektron machine plugged in? A background thread looks at the MIDI
//! ports every [`SCAN_EVERY`], independent of the device worker (which may be
//! busy with a long transfer), so the UI can dim device features the moment
//! the cable comes out and light them again when it goes back in.
//!
//! This only looks at port names; it never talks to the device.
//!
//! The user can also switch the device off (the DEVICE switch): then
//! [`presence`] says [`Presence::Off`], the scan stops, and the worker
//! drops its MIDI ports, so a DAW can have the device to itself.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Exactly one, by port name.
    One(String),
    /// More than one: we can't yet tell which one the user means.
    Many(Vec<String>),
    /// Switched off by the user: no scan, no connection, until switched
    /// back on.
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
            Presence::Many(names) => Some(format!(
                "More than one Elektron device is connected ({}). Only one at a time is supported: unplug the others.",
                names.join(", ")
            )),
            Presence::Off => Some(
                "Device switched off: no MIDI connection, so other apps (a DAW) can use it. \
                 Click DEVICE to connect again."
                    .into(),
            ),
            _ => None,
        }
    }
}

static STATE: Mutex<Presence> = Mutex::new(Presence::Unknown);
// Port discovery alone does not establish that the device can communicate.
static CONNECTION: Mutex<Option<Result<String, String>>> = Mutex::new(None);

pub fn connection_status() -> Option<Result<String, String>> {
    CONNECTION.lock().ok().and_then(|s| s.clone())
}

pub(super) fn set_connection_status(status: Result<String, String>) {
    if let Ok(mut s) = CONNECTION.lock() {
        *s = Some(status);
    }
}

static OFF: AtomicBool = AtomicBool::new(false);

/// Every MIDI port name seen by the last scan, for the About box.
#[derive(Clone, Default)]
pub struct MidiPorts {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// The input and output the device worker has open, if any.
    pub claimed: Option<(String, String)>,
}

static PORTS: Mutex<(Vec<String>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));
static CLAIMED: Mutex<Option<(String, String)>> = Mutex::new(None);

/// All MIDI ports from the last scan (the scan pauses while switched off),
/// and the ones in use.
pub fn midi_ports() -> MidiPorts {
    let (inputs, outputs) = PORTS.lock().map(|p| p.clone()).unwrap_or_default();
    MidiPorts {
        inputs,
        outputs,
        claimed: CLAIMED.lock().ok().and_then(|c| c.clone()),
    }
}

/// The worker opened (`Some`) or closed (`None`) its ports.
pub(super) fn set_claimed(ports: Option<(String, String)>) {
    if let Ok(mut c) = CLAIMED.lock() {
        *c = ports;
    }
}

/// The latest scan, or [`Presence::Off`] while switched off.
pub fn presence() -> Presence {
    if switched_off() {
        return Presence::Off;
    }
    STATE.lock().map(|s| s.clone()).unwrap_or_default()
}

/// The user switched the device off.
pub fn switched_off() -> bool {
    OFF.load(Ordering::Relaxed)
}

/// Switch the device off (stop scanning; the caller tells the worker to
/// let go of the ports) or back on (scan right away, so the answer is
/// fresh this frame).
pub fn set_off(off: bool) {
    OFF.store(off, Ordering::Relaxed);
    if let Ok(mut s) = CONNECTION.lock() {
        *s = None;
    }
    crate::log::line(format!("device: switched {}", if off { "off" } else { "on" }));
    if !off {
        let now = scan();
        if let Ok(mut s) = STATE.lock() {
            *s = now;
        }
    }
}

/// Start the scan loop. Repaints the UI whenever the answer changes.
pub fn start(ctx: egui::Context) {
    let _ = std::thread::Builder::new()
        .name("device presence".into())
        .spawn(move || {
            loop {
                if switched_off() {
                    std::thread::sleep(SCAN_EVERY);
                    continue;
                }
                let now = scan();
                let changed = match STATE.lock() {
                    Ok(mut s) if *s != now => {
                        crate::log::line(format!("device: presence {now:?}"));
                        *s = now;
                        true
                    }
                    _ => false,
                };
                if changed {
                    ctx.request_repaint();
                }
                std::thread::sleep(SCAN_EVERY);
            }
        });
}

fn scan() -> Presence {
    // A fresh client each time: a long-lived one may not see ports come and
    // go without a run loop.
    let Ok(input) = midir::MidiInput::new("KitPool presence") else {
        return Presence::Unknown;
    };
    let all_in: Vec<String> = input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect();
    let all_out: Vec<String> = midir::MidiOutput::new("KitPool presence")
        .map(|o| o.ports().iter().filter_map(|p| o.port_name(p).ok()).collect())
        .unwrap_or_default();
    let mut names: Vec<String> = all_in
        .iter()
        .filter(|n| super::conn::elektron_port(n))
        .cloned()
        .collect();
    if let Ok(mut p) = PORTS.lock() {
        *p = (all_in, all_out);
    }
    names.sort();
    names.dedup();
    match names.len() {
        0 => Presence::Absent,
        1 => Presence::One(names.remove(0)),
        _ => Presence::Many(names),
    }
}
