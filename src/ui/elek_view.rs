//! The Elek Sample Order view: the device's sample pool, docked as one pane
//! beside the files, or in two side-by-side lists so a slot can be dragged
//! onto any other (in any bank), pushing what's there down to the next
//! empty slot (⌥ swaps). Also the device bar both
//! device pages share.

use crate::device::{ElekProject, PoolEntry, SavedProject};
use crate::ui::drag_drop::ResponseDnd;
use crate::ui::slot_list::{self, DropTarget};
use crate::ui::theme;
use egui::{Color32, RichText, Sense, Stroke};
use std::collections::{BTreeSet, HashMap, HashSet};

mod chrome;
mod dialogs;
mod editing;
mod groups;
mod keys;
mod slot_menu;
mod slots;
mod state;

pub(crate) use chrome::{RowPick, device_bar, device_row, device_rows_below, load_screen, mode_header, status_line, top};
pub(crate) use dialogs::finish;
use chrome::{memory_bar, order_tools, preview_tools, MEM_BAR_H};
use slot_menu::{slot_menu, slot_tooltip};
use slots::{body, slot_pane, SlotRowInfo};

const ROW_H: f32 = 20.0;
const TEXT: f32 = 16.0;
const BANK: usize = 128;

pub use theme::oled_font;
use theme::{OLED, OLED_BG, OLED_DIM, OLED_HOVER};

#[derive(Default)]
pub struct ElekState {
    pub projects: Option<Result<Vec<SavedProject>, String>>,
    pub project: Option<Box<ElekProject>>,
    /// `order[slot] = slot it came from` in the project as read.
    pub order: Vec<u16>,
    /// A slot drag is under way (until the button comes up).
    slot_drag: bool,
    /// Where the dragged block was grabbed, so it lands the way it's held.
    slot_grab: slot_list::Grab,
    /// +Drive paths by `(hash, length)`.
    pub names: HashMap<(u32, u32), String>,
    /// Pool files we have a local copy of (uploaded from it, or fetched to
    /// it), by `(hash, length)`: `(device name when linked, local path,
    /// is the original)`. Filled by the app.
    pub linked: HashMap<(u32, u32), (String, std::path::PathBuf, bool)>,
    /// Pool files whose local file was edited since it was linked, so the
    /// device still plays the older version: the edited file. Filled by
    /// the app.
    pub changed_local: HashMap<(u32, u32), std::path::PathBuf>,
    /// The slot menu asked to send these edited files to their slots.
    want_update: Option<Vec<(u32, u32)>>,
    /// A row asked for its local copy: `(key, open in editor)`.
    want_local: Option<((u32, u32), bool)>,
    /// A row asked to show its original in the source pane.
    want_original: Option<(u32, u32)>,
    /// The slot menu asked to queue these pool samples for the preview cache.
    want_fetch: Option<Vec<(u32, u32)>>,
    /// Renaming a pool sample's +Drive file: `(key, +Drive path, name typed
    /// so far, focus the field next frame)`.
    renaming: Option<((u32, u32), String, String, bool)>,
    want_files: Option<Vec<(u16, std::path::PathBuf)>>,
    pub bank: [usize; 2],
    /// `(label, done, total)` while the device works.
    pub busy: Option<(String, usize, usize)>,
    pub status: String,
    pub confirm_push: bool,
    /// A push was held back because the device's project changed since the
    /// read: asking whether to push anyway.
    pub confirm_stale: bool,
    /// Asking whether to drop unpushed changes and close the project.
    confirm_close: bool,
    /// Asking whether to clear these still-referenced slots (as read).
    confirm_clear: Option<Vec<u16>>,
    /// After a push: the pool the device should now have.
    pub expect: Option<Vec<PoolEntry>>,
    pub asked_projects: bool,
    /// Selected slots (positions in the new order). Slot 0 is never in it.
    sel: BTreeSet<usize>,
    /// Keyboard cursor / shift-click anchor.
    cursor: Option<usize>,
    /// Pane the keyboard drives.
    active: usize,
    /// Earlier orders, fills and groups, for undo.
    undo: Vec<Undo>,
    /// Sample groups shown in this project (see `groups.rs`).
    groups: Vec<groups::Group>,
    /// This unit's saved groups, all projects; `groups_from` are the
    /// indices the shown ones came from. Handed back to the app (which
    /// saves them) when `groups_dirty`.
    pub saved_groups: Vec<groups::PoolGroup>,
    groups_from: Vec<usize>,
    pub groups_unit: Option<String>,
    pub groups_dirty: bool,
    /// The slot drag is a whole group's (by its name or frame).
    group_drag: Option<usize>,
    /// Naming a group: `(group to rename, or None for a new one from these
    /// slots, name typed so far, focus the field next frame)`.
    group_naming: Option<(Option<usize>, Vec<usize>, String, bool)>,
    /// New +Drive files put into slots that were empty, by slot as read.
    pub fills: HashMap<u16, PoolEntry>,
    /// Slots (as read) waiting for a dropped file to upload and be found
    /// on the +Drive: the file's name, for the row.
    pub filling: HashMap<u16, String>,
    /// Scroll offset of each pane last frame, and a slot to bring into view.
    scroll: [f32; 2],
    view_h: [f32; 2],
    /// Scroll offset a pane eases to next frame (group alignment).
    align: [Option<f32>; 2],
    reveal: Option<usize>,
    /// A slot (position now) flashed to point it out without selecting it.
    ping: Option<(usize, std::time::Instant)>,
    /// The pane that should scroll to the pinged slot next frame.
    ping_scroll: Option<usize>,
    /// Clicking a slot (or moving the cursor onto it) plays it.
    pub preview: bool,
    /// Clicking a slot opens its local copy in the editor (fetched first if
    /// needed). Exclusive with `preview`.
    pub open_on_click: bool,
    /// Name of the sample the editor is playing right now, set by the app
    /// each frame: the mode header's Sample Editor title turns into a
    /// PLAYING banner that leads back to it.
    pub playing: Option<String>,
    /// Shown as one pane in the right column, beside the files and editor.
    pub docked: bool,
    /// Pass number (+1) when the dialogs were last drawn. The Patterns page
    /// and the docked pool both call `finish`, so this keeps them to once a frame.
    dialogs_pass: u64,
    /// The view gets the keyboard this frame (set by the app).
    pub keys_on: bool,
    /// Which full-window page is shown.
    pub page: ElekPage,
    /// The full-window pages hide the file panes (remembered by the app).
    pub hide_files: bool,
    /// The project editor's layout and selection.
    pub proj: crate::ui::elek_project_view::ProjEd,
    /// The Pool page: kits, samples, sound pool, +Drive data.
    pub lib: crate::ui::elek_library_view::LibraryView,
    /// Saved +Drive preset banks, independent of the loaded project.
    pub presets: crate::ui::elek_presets_view::PresetView,
    /// After a push with pattern edits: every pattern payload, by slot, the
    /// device should now hold.
    /// (`None`: a slot that wasn't loaded.)
    pub expect_patterns: Option<Vec<Option<Vec<u8>>>>,
    /// After a push with sound pool edits: each pool sound's name the
    /// device should now hold ("" = empty).
    pub expect_sounds: Option<Vec<String>>,
    /// After a push: the empty patterns it left as read, which the read-back
    /// takes from here instead of asking the device (a manual reload reads
    /// everything).
    pub keep_blank: Vec<crate::device::DumpMsg>,
    /// While a read runs: which banks to read first, and its stop flag.
    pub reading: Option<std::sync::Arc<crate::device::ReadCtl>>,
    /// The running read's pool has arrived and is what's shown.
    pub read_shown: bool,
    /// Pool files queued or on their way into the preview cache.
    pub fetching: HashSet<(u32, u32)>,
    /// Background fetch run: `(done, total)`.
    pub fetch: Option<(usize, usize)>,
    /// Preview cache on disk: `(bytes, files)`, once measured.
    pub cache: Option<(u64, u64)>,
    /// Every pool edit goes to the device straight away, every pattern
    /// renumbered as usual (overwriting changes made on the device). Only
    /// with the whole project read.
    pub live: bool,
    /// When the pool was last edited, so live pushes wait for a pause.
    pub edited_at: Option<std::time::Instant>,
    /// Patterns page Live: pattern-only edits go to the device after a
    /// pause and aren't read back; the project is updated from what was sent.
    pub pat_live: bool,
    /// The pattern cells last seen by the Live tick, and when they changed.
    pub pat_seen: Vec<crate::device::Cell>,
    pub pat_edited: Option<std::time::Instant>,
    /// The project as it will be once the Live pattern push under way lands,
    /// taken instead of a read-back.
    pub adopt: Option<Box<ElekProject>>,
    /// Slots cut or copied (⌘X / ⌘C). A cut pastes back into the pool
    /// (moving them); either pastes into a local file pane (downloading them).
    clip: Option<PoolClip>,
    /// Slots were just cut or copied: the app drops its file clipboard.
    clipped: bool,
    /// ⌘V with nothing cut here: paste the file clipboard from this slot.
    want_paste_files: Option<usize>,
}

/// Cut or copied slots, by entry as read, so they stay the same samples
/// while the order changes.
struct PoolClip {
    olds: Vec<u16>,
    cut: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ElekPage {
    #[default]
    Order,
    Project,
    /// Saved +Drive preset banks.
    Presets,
    /// Pool: the project's sound pool.
    Library,
}

pub enum ElekIntent {
    ListPresetBanks,
    ListPresets(String),
    ReadPreset { bank: String, slot: u32 },
    ReadPresetTags { bank: String, slot: u32 },
    PushPresets(crate::device::presets::PresetPush),
    BackToEditor,
    /// A click in a device row (see [`device_row`]).
    DeviceRow(RowPick),
    ListProjects,
    Read,
    /// `force`: send even if the device's project changed since the read.
    Push { force: bool },
    /// Play the local copy of this pool file (`false`), or open it in the
    /// editor (`true`). Fetched into the preview cache first when there is
    /// no local copy.
    Local((u32, u32), bool),
    /// Select the local file this pool sample was uploaded from in the
    /// source pane.
    ShowOriginal((u32, u32)),
    /// Upload the edited local file of each of these pool samples next to
    /// its +Drive file, put it in the same slot and push.
    UpdateChanged(Vec<(u32, u32)>),
    /// Rename the +Drive file behind a pool sample. Projects refer to it by
    /// hash, so nothing that uses it breaks.
    RenameFile { key: (u32, u32), from: String, to: String },
    /// Move into the right column, next to the file panes.
    Dock,
    /// From the docked pool: open the project editor.
    OpenPatterns,
    /// Back to the full-window view.
    Undock,
    /// Fetch every pool file without a local copy, in the background.
    FetchAll,
    /// Queue these pool files for the preview cache (ones with a local copy
    /// are skipped).
    FetchKeys(Vec<(u32, u32)>),
    StopFetch,
    /// Move the preview cache to the Trash.
    PurgeCache,
    /// Local files dropped on the pool: each goes to the empty slot paired
    /// with it (numbered as read).
    AddFiles(Vec<(u16, std::path::PathBuf)>),
    /// Pool slots were cut or copied: drop the file clipboard (one
    /// clipboard at a time).
    Clipped,
    /// ⌘V in the pool with nothing cut there: put the local files on the
    /// file clipboard into the empty slots from this one on.
    PasteFiles(usize),
    /// Write the dump as read to disk, for format research.
    SaveDump,
    /// Read all saved +Drive presets into a separate research dump.
    SavePresetDump,
    SavePresetFile,
    LoadPresetFile,
    /// Save the project as edited to a file / open one (asks where).
    /// `true`: a bundle, the samples' WAVs saved alongside.
    SaveProject(bool),
    LoadProject,
    /// Save the pattern in this slot (with its shown name; bundle as for
    /// `SaveProject`) / load one into it.
    SavePattern(usize, String, bool),
    LoadPattern(usize),
    /// Paste patterns copied in another project (`ProjEd::xclip`) from this slot on.
    PastePatterns(usize),
    /// Paste tracks copied in another project (`ProjEd::xtrack_ev` /
    /// `xtrack_snd`) into these patterns: their events and / or sounds.
    PasteTracks { into: Vec<usize>, events: bool, sound: bool },
    /// Read the project's sound pool (Pool page).
    ReadSounds,
}

/// One undo step of the pool.
struct Undo {
    order: Vec<u16>,
    fills: HashMap<u16, PoolEntry>,
    groups: Vec<groups::Group>,
}

/// A drag of the selected slots.
#[derive(Clone, Copy)]
struct SlotDrag;

enum RowAct {
    Click(usize, egui::Modifiers),
    /// `(slot, how far down the row it was grabbed)`.
    DragStart(usize, f32),
    /// A group grabbed by its name or frame: `(group, slot grabbed, how far
    /// down that row)`.
    GroupDragStart(usize, usize, f32),
    Drop(groups::SlotDrop),
    /// Make a group of these slots (asks for its name).
    NewGroup(Vec<usize>),
    RenameGroup(usize),
    DissolveGroup(usize),
    SelectGroup(usize),
    /// Line the other pane up on its group of the same name.
    AlignGroup(usize),
    /// Make an empty group like this one in every bank without one.
    SpreadGroup(usize),
    /// Move these slots into group `g` at its end ("Move to group").
    JoinGroup(usize, Vec<usize>),
    /// Forget every group that holds no samples.
    RemoveEmptyGroups,
    /// `(slot, how many)`.
    InsertEmpty(usize, usize),
    CloseGap(usize),
    MoveHere(usize),
    Nudge(i32),
    /// Empty these slots (unused ones at once, asks about the rest).
    Clear(Vec<usize>),
    ClearUnused,
    Local((u32, u32), bool),
    ShowOriginal((u32, u32)),
    UpdateChanged(Vec<(u32, u32)>),
    Rename((u32, u32)),
    /// Queue these pool samples for the preview cache.
    Fetch(Vec<(u32, u32)>),
    /// Local files dropped on this slot.
    DropFiles(usize, Vec<std::path::PathBuf>),
    /// Cut (`true`) or copy these slots.
    Clip(Vec<usize>, bool),
    /// Paste the cut slots here; `true`: shifting the samples below down.
    Paste(usize, bool),
    /// Forget the cut.
    DropClip,
}

/// What the Device group's small tabs pick: a centre page, or the +Drive
/// in the right column.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DevicePick {
    Page(ElekPage),
    Drive,
}

pub fn show(ui: &mut egui::Ui, st: &mut ElekState) -> Vec<ElekIntent> {
    let mut out = Vec::new();
    top(ui, st, &mut out, order_tools);
    let busy = st.busy.is_some();
    body(ui, st, &mut out, busy);
    finish(ui, st, &mut out);
    out
}

/// The pool as one pane, for the right column.
pub fn show_docked(ui: &mut egui::Ui, st: &mut ElekState) -> Vec<ElekIntent> {
    let mut out = Vec::new();
    let busy = st.busy.is_some();
    ui.horizontal_wrapped(|ui| {
        if ui.button("⤢").on_hover_text("Open the pool in two side-by-side panes, to drag between banks").clicked() {
            out.push(ElekIntent::Undock);
        }
        if ui.button("Patterns").on_hover_text("Open the Device view on Patterns: move, copy, clear and rename patterns").clicked() {
            out.push(ElekIntent::OpenPatterns);
        }
        ui.separator();
        device_bar(ui, st, &mut out, false);
    });
    if st.project.is_some() {
        ui.horizontal_wrapped(|ui| {
            order_tools(ui, st);
            preview_tools(ui, st, &mut out);
        });
        ui.horizontal(|ui| status_line(ui, st, false));
        if let Some(why) = st.project.as_ref().and_then(|p| p.read_only.clone()).filter(|_| !busy) {
            ui.colored_label(Color32::from_rgb(230, 120, 90), "Read-only").on_hover_text(why);
        }
        st.active = 0;
        st.keys(ui.ctx());
        let size = egui::vec2(ui.available_width(), (ui.available_height() - MEM_BAR_H).max(ROW_H * 4.0));
        ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(size);
            ui.push_id("docked", |ui| slot_pane(ui, st, 0));
        });
        memory_bar(ui, st);
    } else {
        ui.add_space(6.0);
        if let Some((label, done, total)) = &st.busy {
            ui.label(format!("{label}…"));
            ui.add(theme::progress_bar(*done as f32 / (*total).max(1) as f32).text(format!("{done}/{total}")));
        } else {
            ui.label(RichText::new("Reads whatever project is open on the device now. To work on another one, load it on the device first (PROJECT › LOAD).").weak().small());
            if !st.status.is_empty() {
                ui.colored_label(Color32::from_rgb(230, 120, 90), &st.status);
            }
        }
    }
    finish(ui, st, &mut out);
    out
}
