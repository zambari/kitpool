//! The Pool page (sibling of Patterns): the project's 128 pool sounds,
//! read by itself the first time the page shows with a device plugged in,
//! with the sample each plays, its machine and SRC page, and the kit tracks
//! that carry the same sound (name and sample), i.e. where it was probably
//! loaded from the pool. Drawn in the device's pixel font, like the Samples
//! and Patterns pages.
//!
//! The sound pool can be edited with the whole project read: multi-select,
//! move/copy/clear sounds, edit their tags, and arrange named slot groups.
//! Group planning is shared with RAM and Presets. Edits sit in `LibraryView::sounds` and go out with the
//! project push (`ElekProject::build_push`), sound locks renumbered. Data
//! comes from `device::library` and the patterns as read (`Patterns::info`).

use crate::device::machines::{plays_sample, SrcParam};
use crate::device::library::{lock_map, sounds_as_read, sounds_changed, PoolSound, SoundCell, SoundSrc, SOUND_POOL, SRC_NAMES};
use crate::ui::elek_project_view::PoolClip;
use crate::device::patterns::{self, PATTERNS};
use crate::device::ElekProject;
use crate::ui::elek_view::{self, ElekIntent, ElekState};
use crate::ui::sound_tags;
use crate::ui::theme::{self, oled_font, OLED, OLED_BG, OLED_DIM};
use egui::{Color32, RichText};
use std::collections::{HashMap, BTreeSet};
use crate::ui::slot_groups::{self, Group, GroupHost, SlotDrop};
mod groups;

const ROW_H: f32 = 15.0;
/// How long a blocked edit's warning stays up.
const BLOCKED_SECS: f64 = 4.0;
/// Pixel-font sizes (before `oled_font`'s own scaling).
const F_NAME: f32 = 14.0;
const F_TEXT: f32 = 12.0;
const F_SMALL: f32 = 10.0;
const WARN: Color32 = Color32::from_rgb(230, 120, 90);
/// A kit track that differs from the most common kit.
const DIFFERS: Color32 = theme::CHANGED;

#[derive(Default)]
pub struct LibraryView {
    filter: String,
    /// The sound pool as last read; cleared when another project loads.
    pub pool_sounds: Option<Result<Vec<PoolSound>, String>>,
    /// `(done, total)` while the sound pool is being read.
    pub sound_progress: Option<(usize, usize)>,
    hide_blank: bool,
    /// The sound pool as edited, by index (empty: as read).
    pub sounds: Vec<SoundCell>,
    /// Earlier `sounds`, for undo.
    sound_undo: Vec<(Vec<SoundCell>, Vec<Group>, BTreeSet<usize>, Option<usize>)>,
    sel: BTreeSet<usize>,
    groups: Vec<Group>,
    ident: Vec<u16>,
    free: Vec<bool>,
    group_naming: Option<slot_groups::Naming>,
    group_note: Option<String>,
    hue_edits: Vec<(String, f32)>,
    scroll_to: Option<usize>,
    pub groups_unit: Option<String>,
    pub groups_dirty: bool,
    /// Sound-locked steps per pool sound (as read), once counted.
    lock_uses: Option<Vec<u32>>,
    /// An empty sound can be sent in place of a deleted one.
    blank_ok: Option<bool>,
    /// Renaming pool sound `.0` in its row: the name typed so far, focus
    /// next frame.
    renaming: Option<(usize, String, bool)>,
    /// The highlighted pool sound (a click on its row); a click on its name
    /// then renames it.
    selected: Option<usize>,
    /// SRC values at their defaults (TUNE 0, LEV 100, …) are left blank.
    pub(crate) show_defaults: bool,
    /// An edit was tried while editing is locked: why, and when (egui time),
    /// shown by the pointer and in the tool row for a few seconds.
    blocked: Option<(String, f64)>,
    /// The sound pool was read (or tried) by itself for this project.
    auto_read: bool,
    /// A sound paste awaiting permission to add its source sample.
    pending_sample: Option<SamplePaste>,
}

impl LibraryView {
    /// Another project is loaded: what was read of the old one goes, and
    /// the sound pool read with it comes in.
    pub fn project_changed(&mut self, p: &ElekProject) {
        self.pool_sounds = p.pool_sounds().map(Ok);
        self.sound_progress = None;
        self.auto_read = false;
        self.pending_sample = None;
        self.sounds.clear();
        self.sound_undo.clear();
        self.renaming = None;
        self.selected = None;
        self.sel.clear();
        self.group_naming = None;
        self.patterns_changed();
    }

    /// No project any more: nothing of the old sound pool stays on screen.
    pub fn forget(&mut self) {
        self.pool_sounds = None;
        self.sound_progress = None;
        self.auto_read = false;
        self.pending_sample = None;
        self.sounds.clear();
        self.sound_undo.clear();
        self.renaming = None;
        self.selected = None;
        self.sel.clear();
        self.group_naming = None;
        self.blocked = None;
        self.patterns_changed();
    }

    /// The patterns (or pool sounds) as read changed: recount.
    pub fn patterns_changed(&mut self) {
        self.lock_uses = None;
        self.blank_ok = None;
    }

    /// Pool sounds edited since the read.
    pub fn sound_changed(&self) -> usize {
        sounds_changed(&self.sounds)
    }

    pub(crate) fn cells_mut(&mut self) -> &mut Vec<SoundCell> {
        self.snapshot();
        if self.sounds.len() != SOUND_POOL {
            self.sounds = sounds_as_read();
        }
        &mut self.sounds
    }

    fn rename_sound(&mut self, i: usize, name: String) {
        self.cells_mut()[i].name = Some(name);
    }

    fn targets(&self, at: usize) -> Vec<usize> {
        if self.sel.contains(&at) { self.sel.iter().copied().collect() } else { vec![at] }
    }

    fn delete_sound(&mut self, i: usize) {
        let picks = self.targets(i);
        let cells = self.cells_mut();
        for i in picks { cells[i] = SoundCell { src: SoundSrc::Empty, name: None, tags: None }; }
    }

    fn undo_sounds(&mut self) {
        if let Some(prev) = self.sound_undo.pop() {
            (self.sounds, self.groups, self.sel, self.selected) = prev;
            self.groups_dirty = true;
        }
    }
}

/// Slots dragged on the sound pool, including selected empty slots.
#[derive(Clone)]
struct SoundDrag(Vec<usize>);

/// Why the sound pool can't be edited now, if it can't.
fn sounds_locked(st: &ElekState) -> Option<&'static str> {
    let Some(p) = st.project.as_deref() else { return Some("no project loaded") };
    if p.read_only.is_some() {
        Some("this project is read-only")
    } else if st.reading.is_some() {
        Some("the project is still being read; editing unlocks once every pattern is in")
    } else if p.partial || !p.sounds_read {
        Some("editing needs the whole project read (every pattern and the sound pool): reload it")
    } else if !p.locks_known() {
        Some("the sound locks in these patterns aren't understood (pattern version), so sounds can't be moved safely")
    } else if st.edit_locked() {
        Some("the device is busy")
    } else {
        None
    }
}

/// "A01–A16, B03" for a sorted list of pattern slots.
fn ranges(slots: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < slots.len() {
        let mut j = i;
        while j + 1 < slots.len() && slots[j + 1] == slots[j] + 1 {
            j += 1;
        }
        out.push(if j == i {
            patterns::label(slots[i])
        } else {
            format!("{}–{}", patterns::label(slots[i]), patterns::label(slots[j]))
        });
        i = j + 1;
    }
    out.join(", ")
}

fn slot_label(slot: usize, pool_len: usize) -> String {
    if pool_len > 128 {
        format!("{}{:03}", (b'A' + (slot / 128) as u8) as char, slot % 128)
    } else {
        format!("{slot:03}")
    }
}

/// A cell of fixed width, text left-aligned and cut short to fit.
fn cell(ui: &mut egui::Ui, w: f32, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.allocate_ui_with_layout(egui::vec2(w, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(w);
        ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()))
    })
    .inner
}

fn matches(filter: &str, texts: &[&str]) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty() || texts.iter().any(|t| t.to_lowercase().contains(&f))
}

pub(crate) fn view_tools(ui: &mut egui::Ui, st: &mut ElekState) {
    let lib = &mut st.lib;
    ui.add(egui::TextEdit::singleline(&mut lib.filter).hint_text("filter").desired_width(140.0));
    if !lib.filter.is_empty() && ui.small_button(theme::CLOSE_GLYPH).clicked() {
        lib.filter.clear();
    }
    ui.separator();
    ui.add(theme::soft_toggle(&mut lib.hide_blank, "Hide empty"));
    ui.add(theme::soft_toggle(&mut lib.show_defaults, "Show defaults"))
        .on_hover_text("Off: SRC values left at their defaults (TUNE 0, PLAY FWD, BIT 0, STRT 0, LEN 120, LOOP 0, LEV 100, and 0 in places not identified yet) are left blank, so changed ones stand out");
}

pub fn show(ui: &mut egui::Ui, st: &mut ElekState) -> Vec<ElekIntent> {
    let mut out = Vec::new();
    elek_view::top(ui, st, &mut out, |_, _| {});
    if st.project.is_none() {
        ui.horizontal_wrapped(|ui| view_tools(ui, st));
        elek_view::load_screen(ui, st, &mut out);
        elek_view::finish(ui, st, &mut out);
        return out;
    }
    let mut ping = None;
    sound_pool(ui, st, &mut ping, &mut out);
    // Clicking a sample points it out in the docked pool and plays it
    // (fetched into the preview cache first when there's no local copy).
    if let Some(key) = ping {
        st.ping_key(|k| k == key);
        out.push(ElekIntent::Local(key, false));
    }
    elek_view::finish(ui, st, &mut out);
    out
}

/// Short file name of pool slot `slot` (as read), and its key.
fn sample(p: &ElekProject, fills: &HashMap<u16, crate::device::PoolEntry>, names: &HashMap<(u32, u32), String>, slot: usize) -> (String, Option<(u32, u32)>) {
    if slot == 0 {
        return ("—".into(), None);
    }
    let Some(e) = fills.get(&(slot as u16)).or_else(|| p.pool.get(slot)).filter(|e| !e.is_empty()) else {
        return (format!("{} (empty slot)", slot_label(slot, p.pool.len())), None);
    };
    let name = names
        .get(&e.key())
        .map(|n| {
            let f = n.rsplit('/').next().unwrap_or(n);
            f.rsplit_once('.').map_or(f, |(stem, _)| stem).to_owned()
        })
        .unwrap_or_else(|| "?".into());
    (format!("{} {name}", slot_label(slot, p.pool.len())), Some(e.key()))
}

/// Kit tracks carrying pool sound `s` (same name and sample), by pattern:
/// `(pattern, track)` pairs. Empty for a sound without a name.
fn kit_uses(p: &ElekProject, s: &PoolSound) -> Vec<(usize, usize)> {
    let Ok(pats) = &p.patterns else { return Vec::new() };
    if s.name.trim().is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, info) in pats.info.iter().enumerate().take(PATTERNS) {
        if !pats.loaded.get(i).copied().unwrap_or(false) {
            continue;
        }
        for (t, tr) in info.tracks.iter().enumerate() {
            if tr.slot == s.slot && tr.name == s.name {
                out.push((i, t));
            }
        }
    }
    out
}

/// SRC page places shown as columns (place 3, SMP, is the sample column).
pub(crate) const SRC_SHOWN: [usize; 7] = [0, 1, 2, 4, 5, 6, 7];
const SRC_W: f32 = 40.0;
const MACHINE_W: f32 = 70.0;
const NUM_W: f32 = 20.0;
const NAME_W: f32 = 100.0;
const SAMPLE_W: f32 = 115.0;
const KIT_W: f32 = 30.0;
const PATS_W: f32 = 120.0;
const LOCKS_W: f32 = 36.0;

/// The parameter the sound's machine keeps in SRC place `place`, if any.
pub(crate) fn param_at(s: &PoolSound, place: usize) -> Option<&SrcParam> {
    s.params.iter().find(|q| q.place == place)
}

/// The value a fresh one-shot sound has for this parameter (stored, fine
/// 0). Guessed names (`TUNE?`) count as the parameter they guess.
pub(crate) fn is_default(q: &SrcParam) -> bool {
    let want = match q.name.trim_end_matches('?') {
        "TUNE" => 64,
        "PLAY" | "BIT" | "STRT" | "LOOP" => 0,
        "LEN" => 120,
        "LEV" => 100,
        // Places not identified yet (P1? … P8?): 0 means nothing to see.
        n if n.len() == 2 && n.starts_with('P') && n.as_bytes()[1].is_ascii_digit() => 0,
        _ => return false,
    };
    q.raw == (want, 0)
}

/// The machine name, or "—" when its byte isn't known for this sound version.
pub(crate) fn machine_text(s: &PoolSound) -> String {
    s.machine.as_ref().map(|(_, n)| n.clone()).unwrap_or_else(|| "—".into())
}

/// The whole SRC page, with fine bytes, for a tooltip.
pub(crate) fn src_detail(s: &PoolSound) -> String {
    if s.src.is_empty() {
        return "SRC page layout not known for this sound version".into();
    }
    let head = match &s.machine {
        Some((id, n)) => format!("Machine {n} (byte {id}). SRC page as stored; names ending in ? are guesses, see src/device/elek/machines.rs:"),
        None => "SRC page as stored, read as a one-shot sound (machine byte not known for this sound version):".into(),
    };
    let vals: Vec<String> = s
        .src
        .iter()
        .enumerate()
        .map(|(i, (v, f))| match param_at(s, i) {
            Some(q) => format!("{} {}  (stored {v}, fine {f})", q.name, q.text),
            None => format!("place {}: unused  (stored {v}, fine {f})", i + 1),
        })
        .collect();
    format!("{head}\n{}", vals.join("\n"))
}

fn sound_pool(ui: &mut egui::Ui, st: &mut ElekState, ping: &mut Option<(u32, u32)>, out: &mut Vec<ElekIntent>) {
    let present = crate::device::presence().available();
    let reading = st.lib.sound_progress.is_some();
    // First look at this project's sound pool: read it without being asked.
    if present && !reading && !st.lib.auto_read && st.lib.pool_sounds.is_none() && st.busy.is_none() && st.project.is_some() {
        st.lib.auto_read = true;
        out.push(ElekIntent::ReadSounds);
    }
    let locked = sounds_locked(st);
    let editable = locked.is_none();
    if editable && st.lib.lock_uses.is_none() {
        if let Some(p) = st.project.as_deref() {
            st.lib.lock_uses = Some(p.sound_lock_uses());
            st.lib.blank_ok = Some(p.blank_sound().is_some());
        }
    }
    // ⌘V: paste copied track sounds into the highlighted sound (replacing
    // a filled one; ↶ Undo brings it back).
    if editable && st.keys_on && st.lib.renaming.is_none() && !ui.ctx().egui_wants_keyboard_input() {
        let paste = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))) || (i.modifiers.command_only() && i.key_pressed(egui::Key::V)));
        let copy = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)) || (i.modifiers.command_only() && i.key_pressed(egui::Key::C)));
        if ui.input(|i| i.modifiers.command_only() && i.key_pressed(egui::Key::A)) {
            let p = st.project.as_deref().unwrap();
            st.lib.sel = p.edited_sounds(&st.lib.sounds).iter()
                .filter(|s| !st.lib.hide_blank || !s.blank)
                .filter(|s| matches(&st.lib.filter, &[&s.name, &sample(p, &st.fills, &st.names, s.slot).0, &crate::device::presets::tag_names(s.tags).collect::<Vec<_>>().join(" ")]))
                .map(|s| s.index).collect();
            st.lib.selected = st.lib.sel.first().copied();
        }
        if ui.input(|i| i.modifiers.command_only() && i.key_pressed(egui::Key::Z)) { st.lib.undo_sounds(); }
        if st.lib.blank_ok == Some(true) && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            if let Some(at) = st.lib.sel.first().copied() {
                let n = st.lib.sel.len();
                st.lib.delete_sound(at);
                st.status = format!("Cleared {n} sound slot(s). Push to send them");
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) { st.lib.sel.clear(); st.lib.selected = None; }
        if copy {
            st.status = match (st.lib.sel.first().copied(), st.project.as_deref()) {
                (Some(at), Some(p)) => {
                    let sounds = p.edited_sounds(&st.lib.sounds);
                    let msg = copy_pool_sound(st, at, &sounds);
                    ui.ctx().copy_text(msg.clone());
                    msg
                }
                _ => "Click a pool sound first, then ⌘C copies it".into(),
            };
        }
        if paste {
            st.status = match (st.lib.sel.first().copied(), st.project.as_deref()) {
                (Some(at), Some(p)) => {
                    let sounds = p.edited_sounds(&st.lib.sounds);
                    paste_track_sounds(st, at, &sounds, true)
                }
                _ => "Click a pool sound first, then ⌘V pastes the copied track sound there".into(),
            };
        }
    }
    ui.horizontal(|ui| {
        if editable {
            let lib = &mut st.lib;
            let changed = lib.sound_changed();
            if ui.add_enabled(!lib.sound_undo.is_empty(), egui::Button::new("↶ Undo")).on_hover_text("Undo the last sound pool edit").clicked() {
                lib.undo_sounds();
            }
            if ui.add_enabled(changed > 0, egui::Button::new("↺ Reset sounds")).on_hover_text("Back to the sound pool as read").clicked() {
                lib.snapshot();
                lib.sounds.clear();
                lib.groups.clear();
                lib.groups_dirty = true;
            }
            if changed > 0 {
                ui.label(RichText::new(format!("{changed} pool sound(s) changed: ⬆ Push to device sends them")).color(DIFFERS));
            }
        } else {
            let label = if st.lib.pool_sounds.is_some() { "⟳ Read sound pool again" } else { "⬇ Read sound pool" };
            if ui
                .add_enabled(present && !reading && st.busy.is_none(), egui::Button::new(label))
                .on_hover_text("Asks the device for each of the project's 128 pool sounds. Read-only")
                .clicked()
            {
                out.push(ElekIntent::ReadSounds);
            }
            let now = ui.input(|i| i.time);
            let flash = st.lib.blocked.as_ref().filter(|(_, t)| now - t < BLOCKED_SECS);
            if let Some((msg, _)) = flash {
                ui.colored_label(WARN, msg);
            } else if let Some(why) = locked {
                ui.label(RichText::new(format!("Not editable: {why}")).weak().small());
            }
        }
        if let Some((done, total)) = st.lib.sound_progress {
            ui.add(theme::progress_bar(done as f32 / total.max(1) as f32).desired_width(200.0).text(format!("{done}/{total}")));
        }
    });
    let Some(p) = st.project.as_deref() else { return };
    let sounds: Vec<PoolSound> = if editable {
        p.edited_sounds(&st.lib.sounds)
    } else {
        match &st.lib.pool_sounds {
            None if reading => {
                ui.label(RichText::new("Reading the sound pool…").weak());
                return;
            }
            None => {
                ui.label(RichText::new(if present { "Not read yet." } else { "Not read yet. Plug the device in to read it." }).weak());
                return;
            }
            Some(Err(e)) => {
                ui.colored_label(WARN, e);
                return;
            }
            Some(Ok(s)) => s.clone(),
        }
    };
    st.lib.ident = (0..SOUND_POOL as u16).collect();
    st.lib.free = sounds.iter().map(|s| s.blank).collect();
    if editable {
        let mut acts = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} selected", st.lib.sel.len()));
            if ui.button("Select all").clicked() {
                st.lib.sel = sounds.iter().filter(|s| !st.lib.hide_blank || !s.blank)
                    .filter(|s| matches(&st.lib.filter, &[&s.name, &sample(p, &st.fills, &st.names, s.slot).0, &crate::device::presets::tag_names(s.tags).collect::<Vec<_>>().join(" ")]))
                    .map(|s| s.index).collect();
                st.lib.selected = st.lib.sel.first().copied();
            }
            let picks: Vec<_> = st.lib.sel.iter().copied().collect();
            if ui.add_enabled(st.lib.slots().groupable(&picks).is_ok(), egui::Button::new("Group selected…")).clicked() {
                acts.push(slot_groups::GroupAct::New(picks));
            }
            let v = st.lib.group_view(&st.group_hues);
            for g in slot_groups::ui::bank_groups(&v, 0) {
                let r = ui.add(egui::Label::new(RichText::new(&v.s.groups[g].name).color(v.color(&v.s.groups[g].name, true))).sense(egui::Sense::click_and_drag()));
                if r.clicked() { acts.push(slot_groups::GroupAct::Select(g)); }
                if r.drag_started() {
                    acts.push(slot_groups::GroupAct::Select(g));
                    egui::DragAndDrop::set_payload(ui.ctx(), SoundDrag(v.s.groups[g].members.iter().map(|&i| i as usize).collect()));
                }
                r.context_menu(|ui| slot_groups::ui::menu(ui, &v, g, &mut acts));
            }
        });
        for a in acts { st.lib.group_act(a, 0); }
    }
    let filled = sounds.iter().filter(|s| !s.blank).count();
    let with_sample = sounds.iter().filter(|s| !s.blank && s.slot != 0).count();
    let tip = if editable {
        "Shift-click for a range; ⌘-click to toggle slots. Drag selected sounds to move them; ⌥/Ctrl-drop swaps in place; click a row to highlight it, then click its name to rename it; right-click to rename or delete. A track sound copied on the Patterns page (Copy ▸ track sound) pastes in with right-click or ⌘V on a highlighted empty sound. Click a sample to play it. ⬆ Push to device sends the changes, with every pattern's sound locks following their sounds."
    } else {
        "Read-only until the whole project is read"
    };
    ui.label(RichText::new(format!("{filled} OF {SOUND_POOL} SOUNDS IN USE, {with_sample} WITH A SAMPLE")).font(oled_font(F_TEXT)).color(OLED_DIM)).on_hover_text(tip);
    if p.partial {
        ui.colored_label(WARN, "Not every pattern was read: the kit column is short.");
    }
    let frame = egui::Frame::new().fill(OLED_BG).corner_radius(2.0).inner_margin(4.0);
    let drop = frame
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
            sound_rows(ui, st, &sounds, editable, ping)
        })
        .inner;
    // The dragged thing follows the pointer.
    let ctx = ui.ctx().clone();
    if let (Some(d), Some(pos)) = (egui::DragAndDrop::payload::<SoundDrag>(&ctx), ctx.pointer_latest_pos()) {
        let i = d.0[0];
        let label = format!("{} slot(s) · {:03} {}", d.0.len(), i + 1, sounds.get(i).map(|s| s.name.as_str()).unwrap_or(""));
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("sound_drag")));
        let font = oled_font(F_NAME);
        let galley = painter.layout_no_wrap(label, font, OLED);
        let r = egui::Rect::from_min_size(pos + egui::vec2(14.0, -8.0), galley.size() + egui::vec2(8.0, 4.0));
        painter.rect_filled(r, 2.0, theme::OLED_BG);
        painter.galley(r.min + egui::vec2(4.0, 2.0), galley, OLED);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if let Some((what, to)) = drop {
        drop_sound(st, what, to, ui.input(|i| i.modifiers.alt || i.modifiers.ctrl));
    }
    if editable {
        let mut naming = st.lib.group_naming.take();
        slot_groups::finish_naming(ui.ctx(), &mut naming, "Groups are saved per device, including empty slots.", &mut st.lib);
        st.lib.group_naming = naming;
    }
    for (key, h) in st.lib.hue_edits.drain(..) {
        st.group_hues.insert(key, h);
        st.hues_dirty = true;
    }
    if let Some(note) = st.lib.group_note.take() { st.status = note; }
}

/// The pool sound rows. Returns what was dropped on which row.
fn sound_rows(ui: &mut egui::Ui, st: &mut ElekState, sounds: &[PoolSound], editable: bool, ping: &mut Option<(u32, u32)>) -> Option<(SoundDrag, usize)> {
    let p = st.project.as_deref()?;
    let filter = st.lib.filter.clone();
    let lock_uses = st.lib.lock_uses.clone().unwrap_or_default();
    let blank_ok = st.lib.blank_ok.unwrap_or(false);
    let cells = st.lib.sounds.clone();
    let show_defaults = st.lib.show_defaults;
    let mut renaming = st.lib.renaming.take();
    // `Some(Some(name))`: rename committed; `Some(None)`: cancelled.
    let mut rename_done: Option<Option<String>> = None;
    let mut rename_start: Option<usize> = None;
    let mut selected = st.lib.selected;
    let mut sel = st.lib.sel.clone();
    let modifiers = ui.input(|i| i.modifiers);
    let mut clicked = None;
    let mut acts = Vec::new();
    // An edit gesture on a locked sound pool: what was tried.
    let mut blocked: Option<&str> = None;
    let mut tag_edit = None;
    let sel_bg = Color32::from_rgba_unmultiplied(255, 255, 255, 22);
    let cell_of = |i: usize| cells.get(i).cloned().unwrap_or_else(|| SoundCell::as_read(i));
    // Locks follow the sound: count them by where it was read.
    // Where each sound's locks end up (`lock_map`, as the push does it).
    let lmap = lock_map(&cells);
    let locks_of = |i: usize| -> u32 {
        if !editable {
            return 0;
        }
        (0..SOUND_POOL).filter(|&o| lmap[o] as usize == i).map(|o| lock_uses.get(o).copied().unwrap_or(0)).sum()
    };
    let pool_clip = st.proj.pool_clip.clone();
    let copied = st.proj.copied_sounds();
    let paste_what = match (&pool_clip, copied.as_slice()) {
        (Some(c), _) => c.desc(),
        (None, []) => String::new(),
        (None, [(t, Some(pat))]) => format!("track {} sound of {}", *t as usize + 1, patterns::label(*pat as usize)),
        (None, [(t, None)]) => format!("track {} sound (empty pattern)", *t as usize + 1),
        (None, many) => format!("{} track sounds", many.len()),
    };
    let n_copied = if pool_clip.is_some() { pool_batch(st).len().max(1) } else { copied.len() };
    let paste_what = if n_copied > 1 && pool_clip.is_some() { format!("{n_copied} pool sounds") } else { paste_what };
    let shown: Vec<(&PoolSound, Vec<(usize, usize)>)> = sounds
        .iter()
        .filter(|s| !st.lib.hide_blank || !s.blank)
        .filter(|s| {
            let n = sample(p, &st.fills, &st.names, s.slot).0;
            matches(&filter, &[&s.name, &n, &crate::device::presets::tag_names(s.tags).collect::<Vec<_>>().join(" ")])
        })
        .map(|s| (s, kit_uses(p, s)))
        .collect();
    let head = |t: &str| RichText::new(t).font(oled_font(F_SMALL)).color(OLED_DIM);
    ui.horizontal(|ui| {
        cell(ui, NUM_W, head("#"));
        cell(ui, NAME_W, head("SOUND"));
        cell(ui, sound_tags::WIDTH, head("TAGS"));
        cell(ui, SAMPLE_W, head("SAMPLE"));
        cell(ui, MACHINE_W, head("MACHINE")).on_hover_text("The sound's machine, from its machine byte. Names ending in ? are guesses");
        for i in SRC_SHOWN {
            cell(ui, SRC_W, head(&format!("SRC{}", i + 1))).on_hover_text(format!(
                "SRC page place {} (one-shot: {}). Each machine keeps its own parameter here, named in the cell; ? = not confirmed yet",
                i + 1,
                SRC_NAMES[i]
            ));
        }
        cell(ui, KIT_W, head("KIT"))
            .on_hover_text("Kit tracks with the same sound name and sample: probably loaded from this pool sound (or saved into it)");
        cell(ui, PATS_W, head("IN PATTERNS"));
        if editable {
            cell(ui, LOCKS_W, head("LOCKS")).on_hover_text("Steps sound-locked to this pool sound, over all patterns. They follow it when it moves");
        }
    });
    ui.add_space(2.0);
    let mut dropped = None;
    let mut menu: Option<(usize, MenuPick)> = None;
    theme::always_show_scrollbar(ui);
    let mut scroll = egui::ScrollArea::vertical().id_salt("sound_pool_rows").auto_shrink([false; 2]);
    if let Some(at) = st.lib.scroll_to.take() {
        if let Some(row) = shown.iter().position(|(s, _)| s.index == at) {
            scroll = scroll.vertical_scroll_offset(row as f32 * ROW_H);
        }
    }
    scroll.show_rows(ui, ROW_H, shown.len(), |ui, range| {
        for (s, uses) in &shown[range] {
            let edited = editable && cell_of(s.index) != SoundCell::as_read(s.index);
            let is_sel = sel.contains(&s.index);
            // Highlight goes under the row, so it's painted into this slot later.
            let bg = ui.painter().add(egui::Shape::Noop);
            // A click on the row, outside the sample (which plays) and the
            // number / name (handled below).
            let mut hit = false;
            let mut name_hit = false;
            let row = ui.horizontal(|ui| {
                let color = if s.blank { OLED_DIM } else if edited { DIFFERS } else { OLED };
                let txt = |t: String, c: Color32| RichText::new(t).font(oled_font(F_TEXT)).color(c);
                hit |= cell(ui, NUM_W, txt(format!("{:03}", s.index + 1), OLED_DIM)).clicked();
                match renaming.as_mut().filter(|r| r.0 == s.index) {
                    Some((_, text, focus)) => {
                        let r = ui.add_sized(
                            egui::vec2(NAME_W, ROW_H),
                            egui::TextEdit::singleline(text)
                                .char_limit(patterns::NAME_MAX)
                                .font(oled_font(F_TEXT))
                                .text_color(OLED)
                                .background_color(theme::OLED_BG)
                                .margin(egui::vec2(2.0, 0.0)),
                        );
                        if std::mem::take(focus) {
                            r.request_focus();
                        }
                        if ui.input(|x| x.key_pressed(egui::Key::Escape)) {
                            rename_done = Some(None);
                        } else if r.lost_focus() {
                            // Enter or a click elsewhere keeps what was typed.
                            rename_done = Some(Some(text.clone()));
                        }
                    }
                    None => {
                        let name = if s.name.is_empty() { "—" } else { &s.name };
                        name_hit = cell(ui, NAME_W, RichText::new(name).font(oled_font(F_NAME)).color(color)).on_hover_text(src_detail(s)).clicked();
                    }
                }
                let (tr, _) = ui.allocate_exact_size(egui::vec2(sound_tags::WIDTH, ROW_H), egui::Sense::hover());
                if !s.blank {
                    if let Some(tags) = sound_tags::show(ui, tr, ui.scope_id().with(("pool_tags", s.index)), s.tags, editable, edited) {
                        tag_edit = Some((s.index, tags));
                    }
                }
                let (n, key) = sample(p, &st.fills, &st.names, s.slot);
                // Blank slots carry whatever machine byte was left there.
                let no_sample = !s.blank && s.machine.as_ref().is_some_and(|(id, _)| !plays_sample(p.model, *id));
                let n = if s.blank { String::new() } else if no_sample { "— (synth)".to_owned() } else { n };
                let r = cell(ui, SAMPLE_W, txt(n, if s.slot == 0 || no_sample { OLED_DIM } else { color }));
                if let (true, Some(k)) = (r.on_hover_text("Click: play it once and point it out in the docked pool (fetched from the device first when there is no local copy)").clicked(), key) {
                    *ping = Some(k);
                }
                hit |= cell(ui, MACHINE_W, txt(if s.blank { String::new() } else { machine_text(s) }, color)).on_hover_text(src_detail(s)).clicked();
                for i in SRC_SHOWN {
                    match param_at(s, i).filter(|q| !s.blank && (show_defaults || !is_default(q))) {
                        Some(q) => {
                            let mut job = egui::text::LayoutJob::default();
                            let (style, fs, al) = (ui.style().clone(), egui::FontSelection::Default, egui::Align::Center);
                            // PLAY's values (FWD LP, …) say what they are.
                            if q.name != "PLAY" {
                                RichText::new(format!("{} ", q.name)).font(oled_font(F_SMALL)).color(OLED_DIM).append_to(&mut job, &style, fs, al);
                            }
                            RichText::new(&q.text).font(oled_font(F_TEXT)).color(color).append_to(&mut job, &style, egui::FontSelection::Default, al);
                            hit |= cell(ui, SRC_W, job).on_hover_text(format!("{} {}: stored {}, fine {}", q.name, q.text, q.raw.0, q.raw.1)).clicked();
                        }
                        None => {
                            hit |= cell(ui, SRC_W, "").clicked();
                        }
                    }
                }
                hit |= cell(ui, KIT_W, txt(if uses.is_empty() { "—".into() } else { uses.len().to_string() }, color)).clicked();
                let mut pats: Vec<usize> = uses.iter().map(|u| u.0).collect();
                pats.dedup();
                let pl = ranges(&pats);
                let detail: Vec<String> = uses.iter().map(|(i, t)| format!("{} T{}", patterns::label(*i), t + 1)).collect();
                let r = cell(ui, PATS_W, txt(if pl.is_empty() { "—".into() } else { pl }, color));
                let r = if detail.is_empty() { r } else { r.on_hover_text(detail.join(", ")) };
                hit |= r.clicked();
                if editable {
                    let n = locks_of(s.index);
                    hit |= cell(ui, LOCKS_W, txt(if n == 0 { "—".into() } else { n.to_string() }, color)).clicked();
                }
            });
            let rect = row.response.rect;
            if is_sel {
                ui.painter().set(bg, egui::Shape::rect_filled(rect.expand2(egui::vec2(2.0, 0.0)), 0.0, sel_bg));
            }
            if let Some(g) = st.lib.slots().group_at(s.index) {
                let c = slot_groups::group_color(&st.group_hues, &st.lib.groups[g].name, is_sel);
                ui.painter().vline(rect.left() - 2.0, rect.y_range(), egui::Stroke::new(2.0, c));
            }
            // Name clicked on the highlighted row: rename (or say why not).
            let click_name = |name_click: bool| {
                if name_click && is_sel && sel.len() == 1 && !modifiers.shift && !modifiers.command && !s.blank {
                    Some(s.index)
                } else {
                    None
                }
            };
            if hit {
                clicked = Some(s.index);
            }
            let in_edit = renaming.as_ref().is_some_and(|r| r.0 == s.index);
            if !editable {
                // Same gestures as an editable row, answered with why not.
                let _ = name_hit;
                if !s.blank && !in_edit {
                    let grip = egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + NUM_W + NAME_W + 6.0, rect.max.y));
                    let r = ui.interact(grip, ui.scope_id().with(("pool_sound_ro", s.index)), egui::Sense::click_and_drag());
                    if r.clicked() {
                        let on_name = r.interact_pointer_pos().is_some_and(|q| q.x > rect.min.x + NUM_W + 3.0);
                        rename_start = rename_start.or(click_name(on_name));
                        clicked = Some(s.index);
                    }
                    if r.drag_started() {
                        blocked = Some("Can't move pool sounds yet");
                    }
                    if r.secondary_clicked() {
                        blocked = Some("Can't rename or delete pool sounds yet");
                    }
                } else if name_hit {
                    clicked = Some(s.index);
                }
                continue;
            }
            if in_edit {
                continue;
            }
            // The row as a whole: dragged by its number and name, dropped
            // on anywhere, right-clicked anywhere.
            let rect = row.response.rect;
            let grip = egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + NUM_W + NAME_W + 6.0, rect.max.y));
            let id = ui.scope_id().with(("pool_sound", s.index));
            let r = ui.interact(grip, id, egui::Sense::click_and_drag());
            if r.hovered() && !s.blank && egui::DragAndDrop::payload::<SoundDrag>(ui.ctx()).is_none() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if r.clicked() {
                let on_name = r.interact_pointer_pos().is_some_and(|q| q.x > rect.min.x + NUM_W + 3.0);
                rename_start = rename_start.or(click_name(on_name));
                clicked = Some(s.index);
            }
            if r.drag_started() {
                if !sel.contains(&s.index) { sel = BTreeSet::from([s.index]); selected = Some(s.index); }
                egui::DragAndDrop::set_payload(ui.ctx(), SoundDrag(sel.iter().copied().collect()));
            }
            let over = ui.interact(rect, id.with("drop"), egui::Sense::hover());
            if let Some(d) = over.dnd_hover_payload::<SoundDrag>() {
                let ok = !d.0.contains(&s.index);
                let c = if ok { theme::DEVICE_ACCENT_BRIGHT } else { WARN };
                ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, c), egui::StrokeKind::Inside);
            }
            if let Some(d) = over.dnd_release_payload::<SoundDrag>() {
                dropped = Some(((*d).clone(), s.index));
            }
            let r = if s.blank {
                ui.interact(rect, id.with("menu"), egui::Sense::click_and_drag())
            } else {
                r.on_hover_text("Drag to move the selection; ⌥/Ctrl-drop to swap. Shift-click selects a range; ⌘-click toggles; click the highlighted name to rename it. Right-click to copy, rename, clear the sound slot or paste a sound over it")
            };
            if r.clicked() { clicked = Some(s.index); }
            if r.drag_started() {
                if !sel.contains(&s.index) { sel = BTreeSet::from([s.index]); selected = Some(s.index); }
                egui::DragAndDrop::set_payload(ui.ctx(), SoundDrag(sel.iter().copied().collect()));
            }
            r.context_menu(|ui| {
                let label = if s.blank { format!("Pool sound {:03} (empty)", s.index + 1) } else { format!("Pool sound {:03} “{}”", s.index + 1, s.name) };
                ui.label(RichText::new(label).weak());
                if st.lib.targets(s.index).iter().any(|&i| sounds.get(i).is_some_and(|s| !s.blank)) {
                    if ui.add(egui::Button::new(format!("Copy {} sound(s)", st.lib.targets(s.index).iter().filter(|&&i| sounds.get(i).is_some_and(|s| !s.blank)).count())).shortcut_text("⌘C")).on_hover_text("Paste it into another pool sound here, into a slot on Presets, or onto kit tracks on the Patterns page (select a track, ⌘V or right-click → Paste here)").clicked() {
                        menu = Some((s.index, MenuPick::Copy));
                        ui.close();
                    }
                }
                let ok = n_copied > 0;
                let (text, tip) = match (ok, s.blank) {
                    (false, _) => (
                        "Paste track sound".to_owned(),
                        "Nothing copied: right-click a sound on Pool or Presets → Copy (⌘C), or copy a track sound on Patterns".to_owned(),
                    ),
                    (true, true) => (
                        format!("Paste {paste_what}"),
                        if n_copied > 1 { "Fills this sound and the next empty ones below it".to_owned() } else { "Puts the copied sound here as a pool sound".to_owned() },
                    ),
                    (true, false) => {
                        let locks = locks_of(s.index);
                        let more = if n_copied > 1 { "; the rest fill the next empty sounds below" } else { "" };
                        let lk = if locks > 0 { format!(". Its {locks} sound-locked step(s) will play the pasted sound") } else { String::new() };
                        (format!("Paste {paste_what} over this one"), format!("Replaces “{}”{lk}{more}", s.name))
                    }
                };
                let (text, tip) = if pool_batch(st).len() > 1 {
                    (format!("Paste {paste_what} here"), "Replaces the selected slots (same count required), or consecutive slots starting here. Undo restores replaced sounds.".to_owned())
                } else { (text, tip) };
                if ui.add_enabled(ok, egui::Button::new(text).shortcut_text("⌘V")).on_hover_text(tip.clone()).on_disabled_hover_text(tip).clicked() {
                    menu = Some((s.index, MenuPick::Paste));
                    ui.close();
                }
                if ui.add(egui::Button::new("Select all shown").shortcut_text("⌘A")).clicked() {
                    sel = shown.iter().map(|(s, _)| s.index).collect();
                    selected = sel.first().copied();
                    ui.close();
                }
                ui.separator();
                slot_groups::ui::slot_menu(ui, &st.lib.group_view(&st.group_hues), s.index, is_sel, &mut acts);
                if st.lib.targets(s.index).iter().any(|&i| sounds.get(i).is_some_and(|s| !s.blank)) {
                    ui.separator();
                    if ui.add_enabled(!s.blank && (!is_sel || sel.len() <= 1), egui::Button::new("Rename…")).clicked() {
                        menu = Some((s.index, MenuPick::Rename));
                        ui.close();
                    }
                    ui.separator();
                    let picks = st.lib.targets(s.index);
                    let locks: u32 = picks.iter().map(|&i| locks_of(i)).sum();
                    let tip = if !blank_ok {
                        "No empty sound to send in its place: this project has no empty pattern or empty pool sound to copy one from".to_owned()
                    } else if locks > 0 {
                        format!("{locks} sound-locked step(s) in the patterns point at it; those locks are removed")
                    } else {
                        "Empties this pool sound. Kit tracks keep their own copies".to_owned()
                    };
                    let label = format!("Clear {} sound slot(s) ({locks} locks removed)", picks.len());
                    if ui.add_enabled(blank_ok, egui::Button::new(label)).on_hover_text(tip.clone()).on_disabled_hover_text(tip).clicked() {
                        menu = Some((s.index, MenuPick::Delete));
                        ui.close();
                    }
                }
            });
        }
    });
    match rename_done {
        Some(Some(text)) => {
            if let Some((i, _, _)) = renaming.take() {
                let name = patterns::clean_name(&text);
                if sounds.get(i).is_none_or(|s| s.name != name) {
                    st.lib.rename_sound(i, name.clone());
                    st.status = format!("Pool sound {:03} renamed “{name}”. Push to send it", i + 1);
                }
            }
        }
        Some(None) => renaming = None,
        None => {}
    }
    if let Some((i, tags)) = tag_edit {
        let picks = st.lib.targets(i);
        let cells = st.lib.cells_mut();
        let source = sounds.get(i).map_or(0, |s| s.tags);
        let added = tags & !source;
        let removed = source & !tags;
        let mut n = 0;
        for i in picks {
            if let Some(s) = sounds.get(i).filter(|s| !s.blank) {
                cells[i].tags = Some((s.tags | added) & !removed);
                n += 1;
            }
        }
        st.status = format!("Tags changed on {n} sound(s). Push to send them");
    }
    st.lib.renaming = renaming;
    if let Some(i) = clicked {
        crate::ui::slot_list::click(&mut sel, &mut selected, i, modifiers, 0);
        // Range selection follows the filtered list.
        if modifiers.shift { sel.retain(|i| shown.iter().any(|(s, _)| s.index == *i)); }
    }
    st.lib.selected = selected;
    st.lib.sel = sel;
    for a in acts { st.lib.group_act(a, 0); }
    if let Some((i, MenuPick::Rename)) = menu {
        rename_start = Some(i);
    }
    if let Some(i) = rename_start {
        match sounds_locked(st) {
            None => {
                let name = sounds.get(i).map(|s| s.name.clone()).unwrap_or_default();
                st.lib.renaming = Some((i, name, true));
            }
            Some(_) => blocked = Some("Can't rename pool sounds yet"),
        }
    }
    if let (Some(what), Some(why)) = (blocked, sounds_locked(st)) {
        let msg = format!("{what}: {why}");
        st.status = msg.clone();
        st.lib.blocked = Some((msg, ui.input(|i| i.time)));
    }
    blocked_tip(ui.ctx(), st);
    match menu {
        Some((_, MenuPick::Rename)) => {}
        Some((i, MenuPick::Paste)) => st.status = paste_track_sounds(st, i, sounds, true),
        Some((i, MenuPick::Copy)) => {
            st.status = copy_pool_sound(st, i, sounds);
            ui.ctx().copy_text(st.status.clone());
        }
        Some((i, MenuPick::Delete)) => {
            st.lib.delete_sound(i);
            st.status = format!("Cleared {} sound slot(s). Push to send them", st.lib.targets(i).len());
        }
        None => {}
    }
    dropped
}

enum MenuPick {
    Rename,
    Delete,
    Paste,
    Copy,
}

/// Copy pool sound `at` (as edited) for pasting into another pool sound or
/// onto kit tracks. Returns the status line.
fn copy_pool_sound(st: &mut ElekState, at: usize, sounds: &[PoolSound]) -> String {
    let picks = st.lib.targets(at);
    let Some(p) = st.project.as_deref() else { return "No project loaded".into() };
    let clips: Vec<PoolClip> = picks.into_iter().filter_map(|at| {
        let s = sounds.get(at).filter(|s| !s.blank)?;
        let cell = st.lib.sounds.get(at).cloned().unwrap_or_else(|| SoundCell::as_read(at));
        let track_ok = p.pool_sound_for_track(&cell).map(|_| ());
        let carry = crate::ui::elek_project_view::carry_sound(st, &cell);
        Some(PoolClip { at, cell, name: s.name.clone(), track_ok, carry, rest: Vec::new() })
    }).collect();
    let Some(first) = clips.first().cloned() else { return "No filled sounds selected to copy".into() };
    let n = clips.len();
    let msg = if n == 1 {
        format!("Copied {}: paste on Pool, Presets or Patterns", first.desc())
    } else {
        format!("Copied {n} sounds: paste together on Pool or Presets")
    };
    st.proj.set_pool_clips(clips);
    msg
}

fn pool_batch(st: &ElekState) -> Vec<PoolClip> {
    st.proj.pool_clip.as_ref().map(PoolClip::items).unwrap_or_default()
}

/// Put the Patterns page's copied track sounds into the sound pool: the
/// first at `at` (only over a filled sound with `over`), the rest into the
/// next empty sounds below it. Returns the status line.
fn paste_track_sounds(st: &mut ElekState, at: usize, sounds: &[PoolSound], over: bool) -> String {
    let message = paste_sounds(st, at, sounds, over);
    crate::log::line(format!("elek-lib: paste into sound {:03}: {message}", at + 1));
    message
}

fn paste_sounds(st: &mut ElekState, at: usize, sounds: &[PoolSound], over: bool) -> String {
    let mut batch = pool_batch(st).to_vec();
    if batch.len() <= 1 && st.lib.targets(at).len() > 1 {
        if let Some(c) = &st.proj.pool_clip { batch = vec![c.clone(); st.lib.targets(at).len()]; }
    }
    if batch.len() > 1 {
        let selected = st.lib.targets(at);
        let targets = if selected.len() > 1 {
            if selected.len() != batch.len() {
                return format!("Select {} destination slots, or one starting slot. Nothing pasted", batch.len());
            }
            selected
        } else {
            if at + batch.len() > SOUND_POOL { return "Not enough slots below this destination. Nothing pasted".into(); }
            (at..at + batch.len()).collect()
        };
        return paste_pool_batch(st, batch, targets, false).unwrap_or_else(|e| e);
    }
    if let Some(c) = st.proj.pool_clip.clone() {
        if !over && sounds.get(at).is_some_and(|s| !s.blank) {
            return format!("Pool sound {:03} isn't empty: right-click it → Paste … over this one to replace it", at + 1);
        }
        return paste_pool_clip(st, c, at, false).unwrap_or_else(|e| e);
    }
    let copied = st.proj.copied_sounds();
    if copied.is_empty() {
        return "No track sound copied: on the Patterns page, select a track and right-click → Copy ▸ track sound".into();
    }
    let Some(p) = st.project.as_deref() else { return "No project loaded".into() };
    let blank = |i: usize| sounds.get(i).is_none_or(|s| s.blank);
    if !over && !blank(at) {
        return format!("Pool sound {:03} isn't empty: right-click it → Paste … over this one to replace it", at + 1);
    }
    let mut problems = Vec::new();
    let good: Vec<(u8, u8, Option<SoundCell>)> = copied
        .iter()
        .filter_map(|&(t, pat)| {
            let cell = st.proj.copied_sound_cell(t);
            let valid = match &cell {
                Some(c) => p.pool_sound_for_track(c).map(|_| ()),
                None => p.track_sound_for_pool(pat, t, &[]).map(|_| ()),
            };
            match valid {
                Ok(_) => pat.map(|pat| (pat, t, cell)),
                Err(e) => {
                    problems.push(e);
                    None
                }
            }
        })
        .collect();
    let mut targets = vec![at];
    let mut next = at + 1;
    while targets.len() < good.len() {
        match (next..SOUND_POOL).find(|&i| blank(i)) {
            Some(i) => {
                targets.push(i);
                next = i + 1;
            }
            None => break,
        }
    }
    let n = targets.len().min(good.len());
    if n == 0 {
        return format!("Nothing pasted: {}", problems.join("; "));
    }
    let cells = st.lib.cells_mut();
    for (&i, (pattern, track, cell)) in targets.iter().zip(&good) {
        cells[i] = cell.clone().unwrap_or(SoundCell { src: SoundSrc::Track { pattern: *pattern, track: *track }, name: None, tags: None });
    }
    st.lib.selected = Some(at);
    st.lib.sel = BTreeSet::from([at]);
    let mut msg = if n == 1 {
        let (pat, t, _) = &good[0];
        format!("Track {} sound of {} pasted into pool sound {:03}. Push to send it", *t as usize + 1, patterns::label(*pat as usize), at + 1)
    } else {
        format!("{n} track sounds pasted into pool sounds {}. Push to send them", targets[..n].iter().map(|i| format!("{:03}", i + 1)).collect::<Vec<_>>().join(", "))
    };
    if good.len() > n {
        msg += &format!("; {} didn't fit (no empty sounds left below)", good.len() - n);
    }
    if !problems.is_empty() {
        msg += &format!("; skipped: {}", problems.join("; "));
    }
    msg
}

/// Paste several copied sounds into pool sounds `targets` (same order).
/// Sounds whose sample isn't in this project (presets, other projects) get
/// it added to the sample pool; that is asked once for the whole batch.
fn paste_pool_batch(st: &mut ElekState, batch: Vec<PoolClip>, targets: Vec<usize>, confirmed: bool) -> Result<String, String> {
    if let Some(why) = sounds_locked(st) { return Err(why.into()); }
    if targets.iter().any(|&at| at >= SOUND_POOL) { return Err("Sound destination is no longer available".into()); }
    // Check every sound first; samples shared by several sounds count once.
    let mut keys = BTreeSet::new();
    for c in &batch {
        if let (_, Some((_, entry))) = prepare_sound_paste(st, c)? { keys.insert(entry.key()); }
    }
    let free = (1..st.order.len()).filter(|&pos| st.free(st.order[pos])).count();
    if keys.len() > free {
        return Err(format!("The copied sounds need {} more samples but only {free} sample-pool slots are empty. Nothing pasted", keys.len()));
    }
    if !keys.is_empty() && !confirmed {
        let mut clip = batch[0].clone();
        clip.rest = batch[1..].to_vec();
        let sample_name = if keys.len() == 1 {
            keys.first().and_then(|k| st.names.get(k).cloned()).unwrap_or_else(|| "Unknown sample".into())
        } else {
            format!("{} samples", keys.len())
        };
        let slot = st.first_free().map_or(0, |(pos, _)| pos);
        st.lib.pending_sample = Some(SamplePaste { clip, target: SoundPasteTarget::PoolBatch(targets), sample_name, slot });
        return Ok("Waiting for confirmation to add the sounds' source samples".into());
    }
    // Each added sample takes the next free slot, so prepare one at a time.
    let mut cells = Vec::with_capacity(batch.len());
    for c in &batch {
        let (cell, added) = prepare_sound_paste(st, c)?;
        if let Some((old, entry)) = added { st.fill(old, entry); }
        cells.push(cell);
    }
    let lib = st.lib.cells_mut();
    for (&at, cell) in targets.iter().zip(cells) { lib[at] = cell; }
    st.lib.sel = targets.iter().copied().collect();
    st.lib.selected = targets.first().copied();
    let note = match keys.len() {
        0 => String::new(),
        1 => " Their source sample was added to the sample pool.".into(),
        n => format!(" {n} source samples were added to the sample pool."),
    };
    Ok(format!("Pasted {} sounds.{note} Push to send them", batch.len()))
}

/// A sound let go over pool sound `to`.
fn drop_sound(st: &mut ElekState, SoundDrag(from): SoundDrag, to: usize, swap: bool) {
    if sounds_locked(st).is_some() || from.is_empty() || from.contains(&to) { return; }
    st.lib.sel = from.into_iter().collect();
    let slots = st.lib.slots();
    let group = (0..slots.groups.len()).find(|&g| slots.group_selected(g));
    let d = if let Some(g) = group { SlotDrop::Group { g, top: to } }
        else if swap { SlotDrop::Place(to) }
        else if let Some(g) = slots.group_at(to) { SlotDrop::Into { g, before: Some(to as u16), line: to } }
        else { SlotDrop::Shift(to) };
    drop(slots);
    st.lib.apply_drop(d);
}

/// The latest blocked edit's warning, by the pointer, for a few seconds.
fn blocked_tip(ctx: &egui::Context, st: &ElekState) {
    let Some((msg, t)) = &st.lib.blocked else { return };
    let age = ctx.input(|i| i.time) - t;
    if age >= BLOCKED_SECS {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(BLOCKED_SECS - age));
    theme::pointer_note(ctx, msg, WARN);
}

/// Resolve a shared sound clipboard without changing the project. The caller
/// stages the optional sample only after its destination accepts the sound.
pub(crate) fn prepare_sound_paste(st: &ElekState, clip: &PoolClip) -> Result<(SoundCell, Option<(u16, crate::device::PoolEntry)>), String> {
    let mut c = clip.clone();
    let mut added_sample = None;
    if let SoundSrc::Preset { model, data } = &c.cell.src {
        let p = st.project.as_deref().ok_or("No project loaded")?;
        p.accepts_preset(*model, data)?;
        let mut translated = data.clone();
        // A sound carried over from another project may have no file ID of
        // its own: its sample's key came along.
        if let Some(key) = crate::device::presets::sample_key(*model, data).or(clip.carry.as_ref().and_then(|c| c.1)) {
            let old = (1..st.order.len()).map(|pos| st.order[pos])
                .find(|&old| { let e = st.entry(old as usize); !e.is_empty() && e.key() == key });
            let slot = if let Some(old) = old { old as usize } else {
                if !st.names.contains_key(&key) {
                    return Err(format!("Cannot paste {}: its source sample is missing from +Drive. Restore that sample on the device, then reload presets and copy it again. Nothing was pasted", c.desc()));
                }
                let Some((_, old)) = st.first_free() else {
                    return Err("No empty sample-pool slot for the preset's source sample".into());
                };
                added_sample = Some((old, crate::device::PoolEntry::new_file(key.0, key.1)));
                old as usize
            };
            crate::device::presets::translate_slot(*model, &mut translated, slot);
        } else {
            let sound = crate::device::library::parse_sound(*model, 0, data);
            if crate::device::machines::plays_sample(*model, sound.machine.as_ref().map_or(0, |(id, _)| *id)) && sound.slot != 0 {
                return Err(format!("Cannot paste {}: its source sample cannot be identified (the preset has no usable sample file ID). Nothing was pasted", c.desc()));
            }
        }
        c.cell.src = SoundSrc::Preset { model: *model, data: translated };
    }
    Ok((c.cell, added_sample))
}

/// The original paste destination is retained while its sample load is confirmed.
#[derive(Clone)]
pub(crate) enum SoundPasteTarget {
    Pool(usize),
    /// A multi-sound paste: the clip's sounds go to these pool sounds.
    PoolBatch(Vec<usize>),
    Tracks { pattern: usize, tracks: Vec<u8> },
}

struct SamplePaste {
    clip: PoolClip,
    target: SoundPasteTarget,
    sample_name: String,
    slot: usize,
}

pub(crate) fn ask_sample_paste(
    st: &mut ElekState,
    clip: PoolClip,
    target: SoundPasteTarget,
    sample: (u16, crate::device::PoolEntry),
) -> String {
    let (old, entry) = sample;
    let sample_name = st.names.get(&entry.key()).cloned().unwrap_or_else(|| "Unknown sample".into());
    let slot = st.order.iter().position(|&o| o == old).unwrap_or(old as usize);
    st.lib.pending_sample = Some(SamplePaste { clip, target, sample_name, slot });
    "Waiting for confirmation to add the sound's source sample".into()
}

fn paste_pool_clip(st: &mut ElekState, mut c: PoolClip, at: usize, confirmed: bool) -> Result<String, String> {
    if let Some(why) = sounds_locked(st) { return Err(why.into()); }
    if at >= SOUND_POOL { return Err("Sound destination is no longer available".into()); }
    let (cell, added_sample) = prepare_sound_paste(st, &c)?;
    if cell == st.lib.sounds.get(at).cloned().unwrap_or_else(|| SoundCell::as_read(at)) {
        return Ok(format!("Pool sound {:03} already is {}", at + 1, c.desc()));
    }
    if !confirmed {
        if let Some(sample) = added_sample {
            return Ok(ask_sample_paste(st, c, SoundPasteTarget::Pool(at), sample));
        }
    }
    c.cell = cell;
    if let Some((old, entry)) = added_sample { st.fill(old, entry); }
    st.lib.cells_mut()[at] = c.cell.clone();
    st.lib.selected = Some(at);
    st.lib.sel = BTreeSet::from([at]);
    let sample_note = if added_sample.is_some() { " Its source sample was added to the sample pool." } else { "" };
    Ok(format!("{} pasted into pool sound {:03}.{sample_note} Push on Pool to send it", c.desc(), at + 1))
}

/// Called with the other device dialogs, once per frame, on every device page.
pub(crate) fn confirm_sample_paste(ctx: &egui::Context, st: &mut ElekState) {
    let Some(pending) = &st.lib.pending_sample else { return };
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("elek_sample_paste_modal")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading("Add a sample to project memory?");
        ui.add_space(6.0);
        ui.label(format!("{} uses a sample that isn't in this project's sample pool:", pending.clip.desc()));
        ui.label(RichText::new(&pending.sample_name).strong());
        ui.label(if pending.clip.rest.is_empty() {
            format!("It will be added to sample slot {:03}.", pending.slot)
        } else {
            format!("They go into the empty sample slots from {:03} on.", pending.slot)
        });
        let destination = match &pending.target {
            SoundPasteTarget::Pool(at) => format!("pool sound {:03}", at + 1),
            SoundPasteTarget::PoolBatch(ts) => format!("{} pool sounds from {:03}", ts.len(), ts.first().map_or(0, |t| t + 1)),
            SoundPasteTarget::Tracks { pattern, tracks } => format!("track(s) {} of {}", tracks.iter().map(|t| (usize::from(*t) + 1).to_string()).collect::<Vec<_>>().join(", "), patterns::label(*pattern)),
        };
        ui.label(format!("Commit adds the sample and pastes the sound into {destination}. Cancel leaves both unchanged."));
        ui.label(RichText::new(if st.live {
            "Live is on: committed changes will be sent to the device automatically."
        } else {
            "Changes stay local until you push to the device."
        }).weak());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() { answer = Some(false); }
            if ui.button(RichText::new("Commit").color(theme::DEVICE_ACCENT_BRIGHT)).clicked() { answer = Some(true); }
        });
    });
    if modal.should_close() && answer.is_none() { answer = Some(false); }
    let Some(commit) = answer else { return };
    let pending = st.lib.pending_sample.take().expect("pending sample paste");
    if !commit {
        st.status = "Paste cancelled; no sample added".into();
        return;
    }
    // Resolve again against the current pool and validate before applying any edit.
    let result = match pending.target {
        SoundPasteTarget::Pool(at) => paste_pool_clip(st, pending.clip, at, true),
        SoundPasteTarget::PoolBatch(targets) => paste_pool_batch(st, pending.clip.items(), targets, true),
        SoundPasteTarget::Tracks { pattern, tracks } =>
            crate::ui::elek_project_view::paste_sound_on_tracks(st, pending.clip, pattern, tracks, true),
    };
    st.status = result.unwrap_or_else(|e| format!("Cannot paste sound: {e}"));
    crate::log::line(format!("elek: confirmed sample paste: {}", st.status));
}
