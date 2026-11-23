//! The editable Presets page: two independent +Drive bank browsers,
//! sound copy/paste with the current sound pool.
//! Selection and ordering use slot_list, shared with the Samples page;
//! permutations become grouped native preset moves/swaps only after bank reads.

use crate::device::presets::{Preset, PresetEntry, PresetOp, PresetPush, PresetSlot, tag_names};
use crate::ui::elek_view::{self, ElekIntent, ElekState};
use crate::ui::slot_list::{self, DropTarget};
use crate::ui::sound_tags;
use crate::ui::theme::{self, OLED, OLED_BG, OLED_DIM, OLED_HOVER, oled_font};
use egui::{RichText, Sense, Stroke};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

type Read<T> = Option<Result<T, String>>;

pub struct PresetView {
    pub banks: Read<Vec<String>>,
    asked: bool,
    pub listings: HashMap<String, Read<Vec<PresetEntry>>>,
    pub sounds: HashMap<(String, u32), Read<Preset>>,
    /// Presets kept from background tag reads, whose sample path wasn't looked
    /// up. Fine for edits and push checks; a copy reads them again in full.
    unresolved: HashSet<(String, u32)>,
    /// Row-only metadata, separate from the snapshots needed to edit/push.
    pub tag_masks: HashMap<(String, u32), Read<u32>>,
    bank: [String; 2],
    selected: [Option<u32>; 2],
    sel: BTreeSet<usize>,
    cursor: Option<usize>,
    /// Where the dragged block was grabbed, so it lands the way it's held.
    grab: slot_list::Grab,
    active: usize,
    reveal: Option<usize>,
    undo: Vec<usize>,
    pending: Option<Reorder>,
    filter: String,
    hide_empty: bool,
    ops: Vec<PresetOp>,
    pub pushing: Option<(String, usize, usize)>,
    live: bool,
    live_pushing: bool,
    /// Live was switched on with staged edits: they go out as one checked
    /// manual push first, and Live starts when it succeeds.
    live_after_push: bool,
    edited_at: Option<std::time::Instant>,
    rename: Option<(PresetSlot, String)>,
    pub dumping: Option<(String, usize, usize)>,
    pub importing: bool,
    pub status: String,
    pub push_failed: bool,
    loading: (usize, usize),
    pub read_progress: f32,
    confirm_reload: bool,
    confirm_push: bool,
    /// Copy waiting for its preset; the key is a full re-read it waits on.
    copy_pending: Option<(PresetSlot, PresetSource, Option<(String, u32)>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PresetSource {
    Saved(PresetSlot),
    Imported(usize),
}

#[derive(Clone)]
struct PresetDrag;

struct Reorder {
    banks: Vec<String>,
    order: Vec<usize>,
    sel: BTreeSet<usize>,
    cursor: Option<usize>,
    clear: Option<Vec<usize>>,
    side: usize,
}

#[derive(Clone, Copy)]
enum Edit {
    Drop(DropTarget),
    Nudge(i32),
    InsertEmpty(usize, usize),
    CloseGap(usize),
    Clear(usize),
}

fn location(banks: &[String], index: usize) -> PresetSlot {
    PresetSlot::new(&banks[index / 256], (index % 256 + 1) as u32)
}

impl PresetView {
    pub fn can_load_file(&self) -> bool {
        self.changed() == 0 && self.pending.is_none() && self.pushing.is_none()
            && self.dumping.is_none() && !self.loading() && !self.push_failed
    }

    pub fn load_file(&mut self, loaded: crate::device::preset_file::LoadedPresets) {
        if self.changed() > 0 || self.pending.is_some() || self.pushing.is_some() {
            self.status = "Preset edits changed while loading; reset or push them before loading again".into();
            return;
        }
        // A backup restore is reviewed before its explicit manual push.
        self.live = false;
        self.banks = Some(Ok(loaded.banks));
        self.asked = true;
        for (bank, entries) in loaded.listings {
            self.sounds.retain(|(b, _), _| *b != bank);
            self.listings.insert(bank, Some(Ok(entries)));
        }
        for (loc, preset) in loaded.sounds {
            self.sounds.insert((loc.bank, loc.slot), Some(Ok(preset)));
        }
        self.stage(loaded.ops);
        self.status = format!("Preset backup loaded: {} changed slot(s), including restored empty slots. Review, then Push to device; Undo reverts the load", self.changed());
    }

    pub fn begin_load(&mut self) {
        if self.loading.0 == self.loading.1 {
            self.loading = (0, 0);
        }
        self.loading.1 += 1;
    }

    pub fn end_load(&mut self) {
        self.loading.0 = (self.loading.0 + 1).min(self.loading.1);
        self.read_progress = 0.0;
    }

    fn loading(&self) -> bool {
        self.loading.0 < self.loading.1
    }

    pub fn reload(&mut self) {
        self.banks = None;
        self.asked = false;
        self.listings.clear();
        self.sounds.clear();
        self.unresolved.clear();
        self.tag_masks.clear();
        self.ops.clear();
        self.live_after_push = false;
        self.undo.clear();
        self.pending = None;
        self.copy_pending = None;
        self.rename = None;
        self.edited_at = None;
        self.sel.clear();
        self.cursor = None;
        self.selected = [None; 2];
        self.reveal = None;
        self.push_failed = false;
        self.loading = (0, 0);
        self.read_progress = 0.0;
        self.status.clear();
    }

    pub fn device_reconnected(&mut self, other: bool) {
        self.live = false;
        self.live_after_push = false;
        if !other && (self.push_failed || self.pushing.is_some()) {
            return;
        }
        if self.changed() > 0 && !other {
            if !self.push_failed {
                self.status = "Device reconnected; pending preset edits kept. Push checks for changes on the device before writing.".into();
            }
        } else {
            let dropped = self.changed();
            self.reload();
            if dropped > 0 {
                self.status = format!(
                    "Another device connected; {dropped} pending preset slot change(s) belonged to the previous device."
                );
            }
        }
    }

    pub fn sound_read(&mut self, bank: String, slot: u32, result: Result<Preset, String>) {
        let key = (bank, slot);
        self.unresolved.remove(&key);
        self.sounds.insert(key, Some(result));
    }

    /// Background tag read: keep the whole preset too, so editing it, pushing
    /// it or moving it needs no second read. A reload or a save can discard
    /// the read while it is in flight.
    pub fn tags_read(&mut self, bank: String, slot: u32, result: Result<Preset, String>) {
        let key = (bank, slot);
        let Some(tags) = self.tag_masks.get_mut(&key) else { return; };
        match result {
            Ok(preset) => {
                *tags = Some(Ok(preset.tag_mask()));
                if !self.sounds.contains_key(&key) {
                    self.sounds.insert(key.clone(), Some(Ok(preset)));
                    self.unresolved.insert(key);
                }
            }
            Err(e) => *tags = Some(Err(e)),
        }
    }

    fn entry(&self, loc: &PresetSlot) -> Option<&PresetEntry> {
        self.listings
            .get(&loc.bank)
            .and_then(|r| r.as_ref())
            .and_then(|r| r.as_ref().ok())
            .and_then(|entries| entries.iter().find(|e| e.slot == loc.slot))
    }

    fn base(&self, loc: &PresetSlot) -> Option<PresetSource> {
        self.entry(loc).map(|_| PresetSource::Saved(loc.clone()))
    }

    fn edits(&self) -> BTreeMap<PresetSlot, Option<PresetSource>> {
        let mut cells = BTreeMap::new();
        for (index, op) in self.ops.iter().enumerate() {
            for loc in op.slots() {
                cells.entry(loc.clone()).or_insert_with(|| self.base(&loc));
            }
            match op {
                PresetOp::Move(a, b) => {
                    let src = cells[a].clone();
                    cells.insert(b.clone(), src);
                    cells.insert(a.clone(), None);
                }
                PresetOp::Swap(a, b) => {
                    let src = cells[a].clone();
                    let dst = cells[b].clone();
                    cells.insert(a.clone(), dst);
                    cells.insert(b.clone(), src);
                }
                PresetOp::Clear(a) => {
                    cells.insert(a.clone(), None);
                }
                PresetOp::Write(a, _) => {
                    cells.insert(a.clone(), Some(PresetSource::Imported(index)));
                }
            }
        }
        cells
    }

    fn origin(&self, loc: &PresetSlot) -> Option<PresetSource> {
        self.edits()
            .get(loc)
            .cloned()
            .unwrap_or_else(|| self.base(loc))
    }

    pub(crate) fn changed(&self) -> usize {
        self.edits()
            .iter()
            .filter(|(loc, src)| self.base(loc) != **src)
            .count()
    }

    /// Union of every listed preset's operations word: unlocked presets carry
    /// all of it, locked ones lack some bits.
    fn full_ops(&self) -> u16 {
        self.listings.values()
            .filter_map(|r| r.as_ref()?.as_ref().ok())
            .flatten()
            .filter_map(|e| e.ops)
            .fold(0, |a, b| a | b)
    }

    /// Locked (write-protected) on the device, as far as the listing tells.
    fn locked(&self, loc: &PresetSlot, full: u16) -> bool {
        self.entry(loc).is_some_and(|e| e.locked(full))
    }

    fn name(&self, loc: &PresetSlot) -> String {
        self.entry(loc).map_or_else(|| "—".into(), |e| e.name.clone())
    }

    fn preset(&self, src: &PresetSource) -> Read<&Preset> {
        match src {
            PresetSource::Saved(loc) => self.sounds.get(&(loc.bank.clone(), loc.slot))
                .and_then(|r| r.as_ref()).map(|r| r.as_ref().map_err(Clone::clone)),
            PresetSource::Imported(index) => match self.ops.get(*index) {
                Some(PresetOp::Write(_, preset)) => Some(Ok(preset)),
                _ => None,
            },
        }
    }

    fn source_name(&self, src: &PresetSource) -> String {
        match src {
            PresetSource::Saved(loc) => self.name(loc),
            PresetSource::Imported(_) => self.preset(src).and_then(Result::ok)
                .map_or_else(|| "—".into(), |p| p.sound.name.clone()),
        }
    }

    fn source_tags(&self, src: &PresetSource) -> Read<u32> {
        if let Some(Ok(preset)) = self.preset(src) {
            return Some(Ok(preset.tag_mask()));
        }
        match src {
            PresetSource::Saved(loc) => self.tag_masks.get(&(loc.bank.clone(), loc.slot))
                .cloned().flatten(),
            PresetSource::Imported(_) => None,
        }
    }

    /// Keep at most one background read queued. Visible rows in both panes
    /// come first, then the rest of their banks so tag filtering can fill in.
    /// Tag reads never hold up Reload or Push and never resolve sample paths.
    fn request_tags(&mut self, candidates: [Vec<PresetSource>; 2], out: &mut Vec<ElekIntent>) {
        if !out.is_empty() || self.loading() || self.pushing.is_some()
            || self.dumping.is_some() || self.importing || self.push_failed
            || self.pending.is_some() || self.confirm_reload || self.confirm_push
            || self.tag_masks.values().any(Option::is_none)
        {
            return;
        }
        for src in candidates.into_iter().flatten() {
            if self.source_tags(&src).is_some() {
                continue;
            }
            let PresetSource::Saved(loc) = src else { continue; };
            let key = (loc.bank.clone(), loc.slot);
            if self.sounds.get(&key).is_none_or(|r| matches!(r, Some(Err(_))))
                && !self.tag_masks.contains_key(&key)
            {
                self.tag_masks.insert(key, None);
                out.push(ElekIntent::ReadPresetTags { bank: loc.bank, slot: loc.slot });
                break;
            }
        }
    }

    fn request_sound(&mut self, src: &PresetSource, out: &mut Vec<ElekIntent>) {
        let PresetSource::Saved(loc) = src else { return; };
        let key = (loc.bank.clone(), loc.slot);
        if !self.sounds.contains_key(&key) {
            self.sounds.insert(key, None);
            out.push(ElekIntent::ReadPreset {
                bank: loc.bank.clone(),
                slot: loc.slot,
            });
        }
    }

    fn stage(&mut self, ops: Vec<PresetOp>) {
        if ops.is_empty() {
            return;
        }
        // Live needs no preset data for moves, swaps or clears; a manual push
        // reads what it checks through `request_push_data`.
        self.undo.push(self.ops.len());
        self.ops.extend(ops);
        self.edited_at = Some(std::time::Instant::now());
        if self.changed() == 0 {
            self.ops.clear();
            self.undo.clear();
        }
        self.status.clear();
    }

    fn edit_tags(&mut self, loc: PresetSlot, tags: u32) {
        let Some(src) = self.origin(&loc) else { return; };
        let Some(Ok(mut preset)) = self.preset(&src).map(|r| r.cloned()) else {
            self.status = "Read the preset before editing its tags".into();
            return;
        };
        if preset.tag_mask() == tags { return; }
        if let Err(e) = preset.set_tags(tags) {
            self.status = e;
            return;
        }
        self.stage(vec![PresetOp::Write(loc.clone(), preset)]);
        self.status = if self.live { "Preset tags queued for live save".into() } else {
            format!("Preset {}:{:03} tags changed. Push on Presets to save them to +Drive", loc.bank, loc.slot)
        };
    }

    fn undo(&mut self) {
        if let Some(len) = self.undo.pop() {
            self.ops.truncate(len);
            self.edited_at = Some(std::time::Instant::now());
        }
    }

    fn select(&mut self, banks: &[String], side: usize, index: usize, modifiers: egui::Modifiers) {
        slot_list::click(&mut self.sel, &mut self.cursor, index, modifiers, 0);
        self.active = side;
        self.selected[side] = Some((index % 256 + 1) as u32);
        self.bank[side] = banks[index / 256].clone();
    }

    fn edit(&mut self, banks: &[String], action: Edit, out: &mut Vec<ElekIntent>) {
        if let Edit::Clear(at) = action {
            let pick: Vec<_> = if self.sel.contains(&at) {
                self.sel.iter().copied().collect()
            } else {
                vec![at]
            };
            self.pending = Some(Reorder {
                banks: banks.to_vec(),
                order: Vec::new(),
                sel: self.sel.clone(),
                cursor: self.cursor,
                clear: Some(pick),
                side: self.active,
            });
            self.apply_pending(out);
            return;
        }
        let mut order: Vec<usize> = (0..banks.len() * 256).collect();
        let sel: Vec<_> = self.sel.iter().copied().collect();
        match action {
            // Presets never shift-drop (their plain drop inserts).
            Edit::Drop(DropTarget::Place(at) | DropTarget::Shift(at)) => {
                slot_list::place(&mut order, &sel, at, 0);
            }
            Edit::Drop(DropTarget::Insert(at)) => slot_list::insert(&mut order, &sel, at, 0),
            Edit::Nudge(dir) => slot_list::nudge(&mut order, &sel, dir, 0),
            Edit::InsertEmpty(at, n) => {
                let start = at / 256 * 256;
                let (made, _) =
                    slot_list::insert_empty(&mut order[start..start + 256], at - start, n, |i| {
                        self.origin(&location(banks, *i)).is_none()
                    });
                if made == 0 {
                    self.status = "No empty slot below in this bank to make room".into();
                    return;
                }
            }
            Edit::CloseGap(at) => {
                let start = at / 256 * 256;
                slot_list::close_gap(&mut order[start..start + 256], at - start, |i| {
                    self.origin(&location(banks, *i)).is_none()
                });
            }
            Edit::Clear(_) => unreachable!(),
        }
        if order.iter().enumerate().all(|(i, old)| i == *old) {
            return;
        }
        self.pending = Some(Reorder {
            banks: banks.to_vec(),
            order,
            sel: self.sel.clone(),
            cursor: self.cursor,
            clear: None,
            side: self.active,
        });
        self.apply_pending(out);
    }

    /// Translate the shared permutation into native moves/swaps. Read every
    /// affected bank first, including intermediate banks on cross-bank inserts.
    fn apply_pending(&mut self, out: &mut Vec<ElekIntent>) {
        let Some(edit) = self.pending.as_ref() else {
            return;
        };
        let touched: BTreeSet<_> = if let Some(pick) = &edit.clear {
            pick.iter().map(|i| edit.banks[i / 256].clone()).collect()
        } else {
            edit.order
                .iter()
                .enumerate()
                .filter(|(i, old)| *i != **old)
                .map(|(i, _)| edit.banks[i / 256].clone())
                .collect()
        };
        let mut waiting = false;
        for bank in touched {
            match self.listings.get(&bank) {
                Some(Some(Ok(_))) => {}
                Some(Some(Err(e))) => {
                    self.status = format!("Cannot edit bank {bank}: {e}");
                    self.pending = None;
                    return;
                }
                Some(None) => waiting = true,
                None => {
                    self.listings.insert(bank.clone(), None);
                    out.push(ElekIntent::ListPresets(bank));
                    waiting = true;
                }
            }
        }
        if waiting {
            self.status = "Reading affected banks before staging the group edit…".into();
            return;
        }
        let edit = self.pending.take().unwrap();
        self.status.clear();
        let edits = self.edits();
        if let Some(pick) = edit.clear {
            let ops = pick
                .into_iter()
                .map(|i| location(&edit.banks, i))
                .filter(|loc| {
                    edits
                        .get(loc)
                        .cloned()
                        .unwrap_or_else(|| self.base(loc))
                        .is_some()
                })
                .map(PresetOp::Clear)
                .collect();
            self.stage(ops);
            return;
        }
        let n = edit.order.len();
        let origins: Vec<_> = (0..n)
            .map(|i| {
                let loc = location(&edit.banks, i);
                edits.get(&loc).cloned().unwrap_or_else(|| self.base(&loc))
            })
            .collect();
        let mut current: Vec<_> = (0..n).collect();
        let mut positions = current.clone();
        let mut ops = Vec::new();
        for at in 0..n {
            let from = positions[edit.order[at]];
            if at == from {
                continue;
            }
            let a = location(&edit.banks, at);
            let b = location(&edit.banks, from);
            match (
                origins[current[at]].is_some(),
                origins[current[from]].is_some(),
            ) {
                (true, true) => ops.push(PresetOp::Swap(a, b)),
                (false, true) => ops.push(PresetOp::Move(b, a)),
                (true, false) => ops.push(PresetOp::Move(a, b)),
                (false, false) => {}
            }
            current.swap(at, from);
            positions[current[at]] = at;
            positions[current[from]] = from;
        }
        self.sel = edit
            .order
            .iter()
            .enumerate()
            .filter(|(_, old)| edit.sel.contains(old))
            .map(|(i, _)| i)
            .collect();
        self.cursor = edit
            .cursor
            .and_then(|c| edit.order.iter().position(|old| *old == c));
        self.active = edit.side;
        if let Some(c) = self.cursor {
            self.bank[self.active] = edit.banks[c / 256].clone();
            self.selected[self.active] = Some((c % 256 + 1) as u32);
            self.reveal = Some(c);
        }
        self.stage(ops);
    }

    fn keys(
        &mut self,
        ctx: &egui::Context,
        enabled: bool,
        can_edit: bool,
        out: &mut Vec<ElekIntent>,
    ) {
        if !enabled || ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.pending.is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.pending = None;
                self.status = "Cancelled queued preset edit".into();
            }
            return;
        }
        let Some(Ok(banks)) = self.banks.as_ref() else {
            return;
        };
        let banks = banks.clone();
        if banks.is_empty() {
            return;
        }
        let (m, up, down, pgup, pgdn, a, z, esc) = ctx.input(|i| {
            (
                i.modifiers,
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::Z),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if esc {
            self.sel.clear();
        }
        if m.command && z && can_edit {
            self.undo();
        }
        let start = banks
            .iter()
            .position(|b| *b == self.bank[self.active])
            .unwrap_or(0)
            * 256;
        if m.command && a {
            self.sel = (start..start + 256).collect();
        }
        let step: isize = if up {
            -1
        } else if down {
            1
        } else if pgup {
            -16
        } else if pgdn {
            16
        } else {
            0
        };
        if step == 0 {
            return;
        }
        if m.alt && (up || down) {
            if can_edit {
                self.edit(&banks, Edit::Nudge(step as i32), out);
            }
            return;
        }
        let next = (self.cursor.unwrap_or(start) as isize + step)
            .clamp(0, banks.len() as isize * 256 - 1) as usize;
        if m.shift {
            self.sel.insert(self.cursor.unwrap_or(next));
            self.sel.insert(next);
            self.cursor = Some(next);
            self.bank[self.active] = banks[next / 256].clone();
            self.selected[self.active] = Some((next % 256 + 1) as u32);
        } else {
            self.select(&banks, self.active, next, egui::Modifiers::NONE);
        }
        self.reveal = Some(next);
    }

    fn data(&self, src: &PresetSource) -> Option<&Vec<u8>> {
        self.preset(src)?.ok().map(|p| &p.data)
    }

    fn ready(&self) -> bool {
        self.edits().iter().all(|(loc, src)| {
            (self.base(loc).is_none() || self.data(&PresetSource::Saved(loc.clone())).is_some())
                && src.as_ref().is_none_or(|src| self.data(src).is_some())
        })
    }

    fn edit_locked(&self) -> bool {
        (self.pushing.is_some() && !self.live_pushing) || self.importing || self.push_failed
    }

    /// Show the staged edits as saved: move listing entries, tags and cached
    /// presets along with their sounds, so nothing needs reading again. Live
    /// does this when it sends a batch, a manual push once it is verified.
    /// Every source is snapshotted first so moves/swaps cannot overwrite each other.
    fn commit_edits(&mut self) {
        struct Moved {
            name: String,
            ops: Option<u16>,
            sound: Option<Preset>,
            tags: Option<u32>,
            unresolved: bool,
        }
        let edits = self.edits();
        let cells: Vec<_> = edits.iter().map(|(loc, src)| match src {
            None => None,
            Some(PresetSource::Saved(from)) => {
                let key = (from.bank.clone(), from.slot);
                // Reads still in flight answer for the old slot: leave them
                // out so the new slot is read again when needed.
                Some(Moved {
                    name: self.name(from),
                    ops: self.entry(from).and_then(|e| e.ops),
                    sound: self.sounds.get(&key).cloned().flatten().and_then(Result::ok),
                    tags: self.tag_masks.get(&key).cloned().flatten().and_then(Result::ok),
                    unresolved: self.unresolved.contains(&key),
                })
            }
            Some(src @ PresetSource::Imported(_)) => {
                let sound = self.preset(src).and_then(Result::ok).cloned();
                Some(Moved {
                    name: self.source_name(src),
                    // A written slot keeps its own lock state.
                    ops: self.entry(loc).and_then(|e| e.ops),
                    unresolved: sound.as_ref()
                        .is_some_and(|p| matches!(p.sample, Some((_, None)))),
                    tags: None,
                    sound,
                })
            }
        }).collect();
        for (loc, cell) in edits.into_keys().zip(cells) {
            let key = (loc.bank.clone(), loc.slot);
            self.sounds.remove(&key);
            self.tag_masks.remove(&key);
            self.unresolved.remove(&key);
            if let Some(Some(Ok(entries))) = self.listings.get_mut(&loc.bank) {
                entries.retain(|entry| entry.slot != loc.slot);
                if let Some(cell) = &cell {
                    entries.push(PresetEntry { slot: loc.slot, name: cell.name.clone(), ops: cell.ops });
                    entries.sort_by_key(|entry| entry.slot);
                }
            }
            let Some(cell) = cell else { continue; };
            if let Some(mut preset) = cell.sound {
                preset.sound.index = loc.slot as usize;
                self.sounds.insert(key.clone(), Some(Ok(preset)));
                if cell.unresolved {
                    self.unresolved.insert(key.clone());
                }
            }
            if let Some(tags) = cell.tags {
                self.tag_masks.insert(key, Some(Ok(tags)));
            }
        }
        self.ops.clear();
        self.undo.clear();
    }

    /// A manual push compares every touched slot with the device first, so
    /// it needs each one read. Live skips those checks and reads nothing.
    fn request_push_data(&mut self, out: &mut Vec<ElekIntent>) {
        let locs: Vec<_> = self.edits().into_keys().filter(|loc| self.base(loc).is_some()).collect();
        for loc in locs {
            self.request_sound(&PresetSource::Saved(loc), out);
        }
    }

    /// Background reads skip the sample-path lookup a paste needs. Before a
    /// copy completes, read the preset holding its sample again in full.
    fn resolve_for_copy(
        &mut self,
        src: &PresetSource,
        known: impl Fn(&(u32, u32)) -> bool,
        out: &mut Vec<ElekIntent>,
    ) -> Option<(String, u32)> {
        let key = match src {
            PresetSource::Saved(loc) => Some((loc.bank.clone(), loc.slot))
                .filter(|key| self.unresolved.contains(key)),
            PresetSource::Imported(_) => {
                let sample = self.preset(src).and_then(Result::ok)
                    .and_then(|p| p.sample.as_ref()).map(|(key, _)| *key)
                    .filter(|key| !known(key))?;
                self.unresolved.iter().find(|key| matches!(self.sounds.get(*key),
                    Some(Some(Ok(p))) if p.sample.as_ref().is_some_and(|(k, _)| *k == sample)))
                    .cloned()
            }
        }?;
        self.unresolved.remove(&key);
        self.sounds.remove(&key);
        self.request_sound(&PresetSource::Saved(PresetSlot::new(&key.0, key.1)), out);
        Some(key)
    }

    /// Called by app logic even when this page is hidden. Only one batch is
    /// in flight; subsequent edits stay local until that batch succeeds.
    pub(crate) fn live_tick(&mut self, ctx: &egui::Context) -> Vec<ElekIntent> {
        let mut out = Vec::new();
        if !self.live || !crate::device::presence().available() || self.edit_locked()
            || self.dumping.is_some() || self.confirm_push || self.confirm_reload
        {
            return out;
        }
        // A cross-bank move may still be waiting on listings after leaving
        // Presets. Finish staging it here as those reads land.
        self.apply_pending(&mut out);
        if !out.is_empty() || self.pushing.is_some() || self.loading()
            || self.pending.is_some() || self.copy_pending.is_some() || self.rename.is_some()
            || self.changed() == 0 || self.tag_masks.values().any(Option::is_none)
            // Each tag toggle is a whole preset upload: wait until the tag
            // popup (or any menu) closes.
            || egui::Popup::is_any_open(ctx)
        {
            return out;
        }
        let settle = std::time::Duration::from_millis(400);
        if let Some(edited) = self.edited_at {
            let elapsed = edited.elapsed();
            if elapsed < settle {
                ctx.request_repaint_after(settle - elapsed);
                return out;
            }
        }
        let Some(plan) = self.plan(false) else { return out; };
        self.commit_edits();
        self.live_pushing = true;
        self.pushing = Some(("Queued live preset save".into(), 0, plan.ops.len()));
        self.status = "Saving preset edits to +Drive…".into();
        out.push(ElekIntent::PushPresets(plan));
        out
    }

    pub(crate) fn pushed(&mut self, result: Result<(), String>) {
        self.pushing = None;
        let live = std::mem::take(&mut self.live_pushing);
        match result {
            Ok(()) if live => {
                self.status = if self.changed() > 0 {
                    "Presets saved; more edits are waiting to be pushed.".into()
                } else { "Presets saved to +Drive (Live, without read-back).".into() };
            }
            Ok(()) => {
                // Verified slot by slot: show it as saved without reading again.
                self.commit_edits();
                self.status = if std::mem::take(&mut self.live_after_push) {
                    self.live = true;
                    self.edited_at = Some(std::time::Instant::now());
                    "Presets pushed to +Drive and verified. Live is on.".into()
                } else {
                    "Presets pushed to +Drive and verified.".into()
                };
            }
            Err(e) => {
                self.live = false;
                self.live_after_push = false;
                self.push_failed = true;
                let full = self.full_ops();
                let locked: Vec<_> = self.edits().keys()
                    .filter(|loc| self.locked(loc, full))
                    .map(|loc| format!("{}:{:03}", loc.bank, loc.slot))
                    .collect();
                let e = if locked.is_empty() { e } else {
                    format!("{e} Locked on the device: {}. Unlock them there (or leave them out) and push again.", locked.join(", "))
                };
                self.status = format!("Preset push failed: {e} Some edits may already be on the device. Live is off; queued edits are stopped. Reload presets before trying again.");
            }
        }
    }

    /// Live (`verify` false) sends only the native operations: moves, swaps
    /// and clears by path, and writes carrying their own sound. A manual push
    /// also needs every touched slot's current and new bytes to check.
    fn plan(&self, verify: bool) -> Option<PresetPush> {
        let ops = compact(&self.ops);
        if !verify {
            return Some(PresetPush { verify, ops, before: Vec::new(), after: Vec::new() });
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (loc, src) in self.edits() {
            let old = if self.base(&loc).is_some() {
                Some(self.data(&PresetSource::Saved(loc.clone()))?.clone())
            } else {
                None
            };
            let new = match src {
                Some(src) => Some(self.data(&src)?.clone()),
                None => None,
            };
            before.push((loc.clone(), old));
            after.push((loc, new));
        }
        Some(PresetPush { verify, ops, before, after })
    }
}

/// Drop a write that a later write to the same slot replaces before anything
/// else touches it (tag toggles, rename then tags): each write is a whole
/// preset upload.
fn compact(ops: &[PresetOp]) -> Vec<PresetOp> {
    ops.iter().enumerate().filter(|(i, op)| {
        let PresetOp::Write(loc, _) = op else { return true; };
        !ops[i + 1..].iter().find(|next| next.slots().contains(loc))
            .is_some_and(|next| matches!(next, PresetOp::Write(..)))
    }).map(|(_, op)| op.clone()).collect()
}

impl Default for PresetView {
    fn default() -> Self {
        Self {
            banks: None,
            asked: false,
            listings: HashMap::new(),
            sounds: HashMap::new(),
            unresolved: HashSet::new(),
            tag_masks: HashMap::new(),
            bank: ["A".into(), "B".into()],
            selected: [None; 2],
            sel: BTreeSet::new(),
            cursor: None,
            grab: slot_list::Grab::default(),
            active: 0,
            reveal: None,
            undo: Vec::new(),
            pending: None,
            filter: String::new(),
            hide_empty: false,
            ops: Vec::new(),
            pushing: None,
            live: false,
            live_pushing: false,
            live_after_push: false,
            edited_at: None,
            rename: None,
            dumping: None,
            importing: false,
            status: String::new(),
            push_failed: false,
            loading: (0, 0),
            read_progress: 0.0,
            confirm_reload: false,
            confirm_push: false,
            copy_pending: None,
        }
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut ElekState) -> Vec<ElekIntent> {
    let mut out = Vec::new();
    elek_view::mode_header(ui, st, &mut out);
    let present = crate::device::presence().available();
    if present && !st.presets.asked {
        st.presets.asked = true;
        out.push(ElekIntent::ListPresetBanks);
    }
    if present && !st.presets.edit_locked() {
        st.presets.apply_pending(&mut out);
    }
    let can_edit = present
        && !st.presets.edit_locked()
        && st.presets.pending.is_none();
    st.presets
        .keys(ui.ctx(), st.keys_on && present, can_edit, &mut out);
    finish_copy(st, ui.ctx());
    if st.keys_on && present && !ui.ctx().egui_wants_keyboard_input() {
        let (copy, paste) = ui.input(|i| (
            i.events.iter().any(|e| matches!(e, egui::Event::Copy)) || (i.modifiers.command_only() && i.key_pressed(egui::Key::C)),
            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))) || (i.modifiers.command_only() && i.key_pressed(egui::Key::V)),
        ));
        if let Some(slot) = st.presets.selected[st.presets.active] {
            let loc = PresetSlot::new(&st.presets.bank[st.presets.active], slot);
            if copy { copy_preset(st, loc.clone(), &mut out, ui.ctx()); }
            if paste && can_edit { paste_sound(st, loc); }
        }
    }
    let changed = st.presets.changed();
    if present && changed > 0 && !st.presets.live && !st.presets.edit_locked() {
        st.presets.request_push_data(&mut out);
    }
    let busy = st.presets.pushing.is_some() || st.presets.pending.is_some() || st.presets.importing;
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(present && !busy && !st.presets.loading(), egui::Button::new("⬇ Reload presets")).clicked() {
            if changed > 0 {
                st.presets.confirm_reload = true;
            } else {
                st.presets.reload();
            }
        }
        ui.add_enabled_ui(present && !busy && st.presets.dumping.is_none(), |ui| {
            ui.menu_button("💾 Save", |ui| {
                if ui.button("Save presets… (importable JSON)")
                    .on_hover_text("Back up every saved +Drive preset, preserving bank and slot locations, empty slots and original sound bytes. Staged edits and sample audio are excluded.")
                    .clicked() {
                    out.push(ElekIntent::SavePresetFile);
                    ui.close();
                }
                ui.separator();
                if ui.button("Save preset debug dump")
                    .on_hover_text("Read all saved +Drive banks into ~/Documents/KitPool/preset_dumps: raw containers, decoded sounds, names, strings and a presets.json backup. Can take several minutes.")
                    .clicked() {
                    out.push(ElekIntent::SavePresetDump);
                    ui.close();
                }
            });
        });
        if ui.add_enabled(present && st.presets.can_load_file(), egui::Button::new("Load presets…"))
            .on_hover_text("Open an importable JSON backup (or presets.json from a debug dump). Stages a restore of its banks, including clearing slots saved as empty. Other banks are untouched. Push or reset existing edits before loading.")
            .clicked() {
            out.push(ElekIntent::LoadPresetFile);
        }
        let ready = present && !busy && !st.presets.push_failed && !st.presets.loading()
            && changed > 0 && st.presets.ready();
        if ui.add_enabled(ready, egui::Button::new(RichText::new("⬆ Push to device").color(theme::DEVICE_ACCENT_BRIGHT)))
            .on_hover_text("Save staged preset edits to +Drive after checking the device still has the presets that were read. Preset data must finish loading first.").clicked() {
            st.presets.confirm_push = true;
        }
        ui.add_enabled_ui(present && !st.presets.push_failed && !st.presets.importing
            && st.presets.dumping.is_none() && (st.presets.pushing.is_none() || st.presets.live_pushing), |ui| {
            let was_live = st.presets.live;
            if ui.toggle_value(&mut st.presets.live, "⚡ Live")
                .on_hover_text("Save preset edits after a short pause, including renames, tags, moves, swaps, pastes and clears. Keep editing while saves run. Skips device-change checks and read-back; edits made on the device may be overwritten. Undo / Reset affect only edits not yet sent. Turning Live on with staged edits pushes those first, with the usual checks. Turning Live off leaves unsent edits staged.")
                .changed() {
                if !was_live && changed > 0 {
                    // Edits staged before Live go out once with the usual
                    // checks and read-back; Live starts after that push.
                    st.presets.live = false;
                    st.presets.live_after_push = true;
                    st.presets.confirm_push = true;
                } else {
                    st.presets.edited_at = Some(std::time::Instant::now());
                }
            }
        });
        if ui.add_enabled(can_edit && !st.presets.ops.is_empty(), egui::Button::new("Undo"))
            .on_hover_text("Undo the last edit that has not yet been sent to the device").clicked() {
            st.presets.undo();
        }
        if ui.add_enabled(can_edit && changed > 0, egui::Button::new("Reset")).clicked() {
            st.presets.ops.clear();
            st.presets.undo.clear();
        }
        if changed > 0 {
            ui.colored_label(theme::CHANGED, format!("{changed} changed slot(s) · {}", if st.presets.live { "queued" } else { "not pushed" }));
        }
    });
    if let Some((label, done, total)) = &st.presets.dumping {
        ui.add(
            theme::progress_bar(*done as f32 / (*total).max(1) as f32)
                .text(format!("{label} · {done}/{total}"))
                .desired_width(ui.available_width()),
        );
        ui.ctx().request_repaint();
    } else if let Some((label, done, total)) = &st.presets.pushing {
        ui.add(
            theme::progress_bar(*done as f32 / (*total).max(1) as f32)
                .text(format!("{label} · {done}/{total}"))
                .desired_width(ui.available_width()),
        );
        ui.ctx().request_repaint();
    } else if st.presets.loading() {
        let (done, total) = st.presets.loading;
        ui.add(
            theme::progress_bar((done as f32 + st.presets.read_progress) / total.max(1) as f32)
                .text(format!("Loading presets · {done}/{total}"))
                .desired_width(ui.available_width()),
        );
        ui.ctx().request_repaint();
    } else {
        ui.add(
            theme::progress_bar(0.0)
                .desired_width(ui.available_width()),
        );
    }
    if st.presets.push_failed && !st.presets.status.is_empty() {
        ui.colored_label(theme::CHANGED, &st.presets.status);
    }
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.presets.filter)
                .hint_text("filter names / tags")
                .desired_width(140.0),
        );
        ui.add(theme::soft_toggle(&mut st.presets.hide_empty, "Hide empty"));
       
    });
    ui.separator();
    let mut tag_reads = [Vec::new(), Vec::new()];
    match &st.presets.banks {
        None => {
            ui.label(if present {
                "Reading preset banks…"
            } else {
                "Connect the device to browse its presets."
            });
        }
        Some(Err(e)) => {
            ui.colored_label(theme::CHANGED, e);
        }
        Some(Ok(banks)) if banks.is_empty() => {
            ui.label("No preset banks found.");
        }
        Some(Ok(banks)) => {
            let banks = banks.clone();
            for side in 0..2 {
                if !banks.contains(&st.presets.bank[side]) {
                    st.presets.bank[side] = banks[side.min(banks.len() - 1)].clone();
                    st.presets.selected[side] = None;
                }
            }
            ui.columns(2, |cols| {
                for (side, col) in cols.iter_mut().enumerate() {
                    col.push_id(("preset_pane", side), |ui| {
                        pane(ui, st, side, &banks, present, &mut tag_reads, &mut out)
                    });
                }
            });
        }
    }
    if st.presets.confirm_reload {
        egui::Modal::new(egui::Id::unique("preset_reload")).show(ui.ctx(), |ui| {
            ui.heading("Discard preset edits and reload?");
            ui.label(format!("{changed} changed slot(s) have not been pushed."));
            ui.horizontal(|ui| {
                if ui.button("Discard and reload").clicked() {
                    st.presets.confirm_reload = false;
                    st.presets.reload();
                }
                if ui.button("Cancel").clicked() {
                    st.presets.confirm_reload = false;
                }
            });
        });
    }
    if st.presets.confirm_push {
        let then_live = st.presets.live_after_push;
        let can_push = st.presets.pushing.is_none() && !st.presets.push_failed && st.presets.ready();
        egui::Modal::new(egui::Id::unique("preset_push")).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.heading(if then_live {
                format!("Turn on Live: push {changed} staged slot(s) first?")
            } else {
                format!("Push {changed} changed preset slot(s)?")
            });
            ui.label("This saves the staged tag edits, pastes, moves, swaps and clears directly to the preset banks on +Drive. Cleared presets are removed from their saved slots.");
            if then_live {
                ui.label("Live saves later edits without checking the device. These staged edits go out once with the usual checks and read-back, and Live starts when that push succeeds.");
            }
            if !can_push {
                ui.label(if st.presets.pushing.is_some() { "Waiting for the current save to finish…" } else { "Loading the touched presets…" });
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(can_push, egui::Button::new(if then_live { "Push and turn on Live" } else { "Push presets" })).clicked() {
                    st.presets.confirm_push = false;
                    if let Some(plan) = st.presets.plan(true) {
                        st.presets.pushing = Some(("Queued preset push".into(), 0, 1));
                        st.presets.status.clear();
                        out.push(ElekIntent::PushPresets(plan));
                    }
                }
                if ui.button("Cancel").clicked() {
                    st.presets.confirm_push = false;
                    st.presets.live_after_push = false;
                }
            });
        });
    }
    if let Some((loc, mut name)) = st.presets.rename.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("preset_rename")).show(ui.ctx(), |ui| {
            ui.heading(format!("Rename preset {}:{:03}", loc.bank, loc.slot));
            ui.text_edit_singleline(&mut name);
            ui.label("Up to 15 printable ASCII characters; saved in uppercase.");
            let src = st.presets.origin(&loc);
            let preset = src.as_ref().and_then(|src| st.presets.preset(src));
            match &preset {
                None => { ui.label("Loading preset…"); }
                Some(Err(e)) => { ui.colored_label(theme::CHANGED, e); }
                _ => {}
            }
            let loaded = preset.and_then(Result::ok).cloned();
            ui.horizontal(|ui| {
                if ui.add_enabled(can_edit && loaded.is_some()
                    && !crate::device::patterns::clean_name(&name).trim().is_empty(), egui::Button::new("Rename")).clicked() {
                    let mut preset = loaded.unwrap();
                    match preset.set_name(&name) {
                        Ok(()) => {
                            st.presets.stage(vec![PresetOp::Write(loc.clone(), preset)]);
                            close = true;
                        }
                        Err(e) => st.presets.status = e,
                    }
                }
                if ui.button("Cancel").clicked() { close = true; }
            });
        });
        st.presets.rename = if close { None } else { Some((loc, name)) };
    }
    elek_view::finish(ui, st, &mut out);
    if present {
        st.presets.request_tags(tag_reads, &mut out);
    }
    out
}

fn copy_preset(st: &mut ElekState, loc: PresetSlot, out: &mut Vec<ElekIntent>, ctx: &egui::Context) {
    let Some(src) = st.presets.origin(&loc) else {
        st.presets.status = "Empty preset slot: nothing to copy".into();
        return;
    };
    // A slow read must not leave the previous sound available for Paste.
    st.proj.clear_sound_clipboard();
    let names = &st.names;
    let wait = st.presets.resolve_for_copy(&src, |key| names.contains_key(key), out);
    st.presets.request_sound(&src, out);
    st.presets.copy_pending = Some((loc, src, wait));
    st.presets.status = "Loading preset for the sound clipboard…".into();
    finish_copy(st, ctx);
}

/// Complete queued copies even when the user switches to the Pool page
/// before the device finishes reading the preset.
pub(crate) fn finish_copy(st: &mut ElekState, ctx: &egui::Context) {
    let Some((loc, src, wait)) = st.presets.copy_pending.clone() else { return; };
    if let Some(key) = &wait {
        match st.presets.sounds.get(key) {
            Some(Some(Err(e))) => {
                st.presets.status = format!("Could not copy preset: {e}");
                st.presets.copy_pending = None;
                return;
            }
            Some(Some(Ok(_))) => {}
            _ => return,
        }
    }
    let Some(result) = st.presets.preset(&src) else { return; };
    let preset = match result {
        Ok(p) => p.clone(),
        Err(e) => {
            st.presets.status = format!("Could not copy preset: {e}");
            st.presets.copy_pending = None;
            return;
        }
    };
    let cell = crate::device::library::SoundCell {
        src: crate::device::library::SoundSrc::Preset { model: preset.model, data: preset.data },
        name: None,
        tags: None,
    };
    let track_ok = st.project.as_deref().ok_or_else(|| "No project loaded".to_owned())
        .and_then(|p| p.pool_sound_for_track(&cell).map(|_| ()));
    st.proj.set_pool_clip(crate::ui::elek_project_view::PoolClip {
        at: 0, cell, name: preset.sound.name, track_ok, carry: None,
    });
    st.presets.copy_pending = None;
    let msg = format!("Copied preset {}:{:03}. Paste into Pool, Presets, or an audio track on Patterns; Push saves the edit", loc.bank, loc.slot);
    ctx.copy_text(msg.clone());
    st.status = msg.clone();
    st.presets.status = msg;
}

fn paste_sound(st: &mut ElekState, loc: PresetSlot) {
    let Some(clip) = st.proj.pool_clip.clone() else {
        st.presets.status = "Copy a sound on Pool, Presets, or Patterns first".into();
        return;
    };
    let mut preset = match &clip.cell.src {
        crate::device::library::SoundSrc::Preset { model, data } => {
            let mut data = if let Some(name) = &clip.cell.name {
                let mut d = data.clone();
                let name = crate::device::patterns::clean_name(name);
                d[12..28].fill(0);
                d[12..12 + name.len()].copy_from_slice(name.as_bytes());
                d
            } else { data.clone() };
            if let Some(tags) = clip.cell.tags { data[8..12].copy_from_slice(&tags.to_be_bytes()); }
            crate::device::presets::Preset {
                model: *model, container: Vec::new(),
                sound: crate::device::library::parse_sound(*model, 0, &data),
                sample: crate::device::presets::sample_key(*model, &data).map(|key| (key, None)),
                data,
            }
        }
        _ => {
            let Some(project) = st.project.as_deref() else {
                st.presets.status = "Load the project the copied pool sound came from first".into();
                return;
            };
            match project.export_preset(&clip.cell, &st.fills) {
                Ok(p) => p,
                Err(e) => { st.presets.status = e; return; }
            }
        }
    };
    if let Some((key, path)) = &mut preset.sample {
        *path = st.names.get(key).cloned();
    }
    st.presets.stage(vec![PresetOp::Write(loc.clone(), preset)]);
    st.presets.status = format!("{} pasted into preset {}:{:03}. Push on Presets to save it to +Drive", clip.desc(), loc.bank, loc.slot);
}

fn bank_strip(ui: &mut egui::Ui, bank: &mut String, banks: &[String]) -> bool {
    let (strip, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::empty());
    ui.painter().rect_filled(strip, 0.0, OLED_BG);
    let cur = banks.iter().position(|b| b == bank).unwrap_or(0);
    let cell = (strip.width() / (banks.len() + 2) as f32).clamp(12.0, 26.0);
    let left = strip.center().x - cell * (banks.len() + 2) as f32 / 2.0;
    let mut pick = None;
    for i in 0..banks.len() + 2 {
        let (text, to) = if i == 0 {
            ("<", cur.checked_sub(1))
        } else if i == banks.len() + 1 {
            (">", (cur + 1 < banks.len()).then_some(cur + 1))
        } else {
            (banks[i - 1].as_str(), Some(i - 1))
        };
        let r = egui::Rect::from_min_size(
            egui::pos2(left + i as f32 * cell, strip.top() + 3.0),
            egui::vec2(cell, 20.0),
        );
        let resp = ui.interact(r, ui.scope_id().with(("bank", i)), Sense::click());
        if i > 0 && i <= banks.len() {
            if to == Some(cur) {
                ui.painter().rect_stroke(
                    r.shrink2(egui::vec2(4.0, 0.0)),
                    0.0,
                    Stroke::new(2.0, OLED),
                    egui::StrokeKind::Inside,
                );
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 0.0, OLED_HOVER);
            }
            theme::color_badge(
                ui.painter(),
                r.shrink2(egui::vec2(4.0, 0.0)),
                theme::BadgeEdge::Bottom,
                theme::identity_color(i - 1, banks.len()),
            );
        }
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            text,
            oled_font(16.0),
            if to.is_some() { OLED } else { OLED_DIM },
        );
        if resp.clicked()
            || (i > 0 && i <= banks.len() && resp.dnd_hover_payload::<PresetDrag>().is_some())
        {
            pick = to;
        }
    }
    if let Some(i) = pick {
        let changed = *bank != banks[i];
        *bank = banks[i].clone();
        changed
    } else {
        false
    }
}

/// Small padlock (about 8×10 px) whose left edge sits at `at.x`, centred on `at.y`.
fn padlock(painter: &egui::Painter, at: egui::Pos2, color: egui::Color32) {
    let body = egui::Rect::from_min_size(egui::pos2(at.x, at.y - 1.0), egui::vec2(8.0, 6.0));
    painter.rect_filled(body, 1.0, color);
    let stroke = egui::Stroke::new(1.5, color);
    let (l, r, top) = (body.left() + 1.75, body.right() - 1.75, body.top() - 3.0);
    let arc: Vec<_> = (0..=8).map(|i| {
        let a = std::f32::consts::PI * i as f32 / 8.0;
        egui::pos2((l + r) * 0.5 - a.cos() * (r - l) * 0.5, top - a.sin() * 1.5)
    }).collect();
    painter.line_segment([egui::pos2(l, body.top()), egui::pos2(l, top)], stroke);
    painter.line_segment([egui::pos2(r, body.top()), egui::pos2(r, top)], stroke);
    painter.add(egui::Shape::line(arc, stroke));
}

fn pane(
    ui: &mut egui::Ui,
    st: &mut ElekState,
    side: usize,
    banks: &[String],
    present: bool,
    tag_reads: &mut [Vec<PresetSource>; 2],
    out: &mut Vec<ElekIntent>,
) {
    if bank_strip(ui, &mut st.presets.bank[side], banks) {
        st.presets.selected[side] = None;
        st.presets.active = side;
    }
    let bank = st.presets.bank[side].clone();
    let start = banks.iter().position(|b| *b == bank).unwrap_or(0) * 256;
    if present && st.presets.pushing.is_none() && !st.presets.listings.contains_key(&bank) {
        st.presets.listings.insert(bank.clone(), None);
        out.push(ElekIntent::ListPresets(bank.clone()));
    }
    let Some(Some(listing)) = st.presets.listings.get(&bank) else {
        ui.label("Reading bank…");
        return;
    };
    match listing {
        Ok(_) => {}
        Err(e) => {
            ui.colored_label(theme::CHANGED, e);
            return;
        }
    };
    let edits = st.presets.edits();
    let origin_at = |loc: &PresetSlot| {
        edits
            .get(loc)
            .cloned()
            .unwrap_or_else(|| st.presets.base(loc))
    };
    let sources: Vec<_> = (1..=256)
        .filter_map(|slot| origin_at(&PresetSlot::new(&bank, slot)))
        .collect();
    let count = sources.len();
    let tags_read = sources.iter().filter(|src| st.presets.source_tags(src).is_some()).count();
    tag_reads[1].extend(sources);
    ui.label(
        RichText::new(format!("BANK {bank} · {count} PRESETS{}",
            if tags_read < count { format!(" · tags {tags_read}/{count}") } else { String::new() }))
            .font(oled_font(12.0))
            .color(OLED_DIM),
    );
    let filter = st.presets.filter.to_uppercase();
    let full_ops = st.presets.full_ops();
    let rows: Vec<(u32, String, bool, bool, Read<u32>)> = (1..=256)
        .filter_map(|slot| {
            let loc = PresetSlot::new(&bank, slot);
            let origin = origin_at(&loc);
            let changed = origin != st.presets.base(&loc);
            if st.presets.hide_empty && origin.is_none() && !changed {
                return None;
            }
            let name = origin
                .as_ref()
                .map_or_else(|| "—".into(), |src| st.presets.source_name(src));
            let tags = origin.as_ref().and_then(|src| st.presets.source_tags(src));
            let tag_text = tags.as_ref().and_then(|r| r.as_ref().ok())
                .map_or_else(String::new, |mask| tag_names(*mask).collect::<Vec<_>>().join(", "));
            if !filter.is_empty()
                && !format!("{bank}:{slot:03} {name} {tag_text}")
                    .to_uppercase()
                    .contains(&filter)
            {
                return None;
            }
            Some((slot, name, origin.is_some(), changed, tags))
        })
        .collect();
    let row_sources: Vec<_> = rows.iter()
        .map(|(slot, _, _, _, _)| origin_at(&PresetSlot::new(&bank, *slot)))
        .collect();
    let height = ui.available_height();
    let mut select = None;
    let mut edit = None;
    let mut copy = None;
    let mut paste = None;
    let mut tag_open = None;
    let mut rename = None;
    let mut tag_edit = None;
    let can_edit = present
        && !st.presets.edit_locked()
        && st.presets.pending.is_none();
    let reveal_row = st
        .presets
        .reveal
        .filter(|_| st.presets.active == side)
        .and_then(|index| {
            rows.iter()
                .position(|(slot, _, _, _, _)| start + *slot as usize - 1 == index)
        });
    egui::Frame::new().fill(OLED_BG).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(("presets", &bank))
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false]);
        if let Some(row) = reveal_row {
            scroll = scroll.vertical_scroll_offset(
                (row as f32 * (20.0 + ui.spacing().item_spacing.y) - height * 0.5).max(0.0),
            );
            st.presets.reveal = None;
        }
        scroll.show_rows(ui, 20.0, rows.len(), |ui, range| {
            // Rows sit `pitch` apart; the spacing between them belongs to
            // the row above, so a drag over a gap still has a target.
            let pitch = 20.0 + ui.spacing().item_spacing.y;
            let dragging = can_edit && egui::DragAndDrop::has_payload_of_type::<PresetDrag>(ui.ctx());
            let mut hover = None;
            for row in range {
                let (slot, name, filled, changed, tags) = &rows[row];
                if let Some(src) = &row_sources[row] {
                    tag_reads[0].push(src.clone());
                }
                let index = start + *slot as usize - 1;
                let (r, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 20.0),
                    Sense::hover(),
                );
                let tags_width = if *filled { sound_tags::WIDTH.min(r.width() * 0.5) } else { 0.0 };
                let tags_rect = egui::Rect::from_min_max(
                    egui::pos2(r.right() - tags_width - 6.0, r.top()),
                    egui::pos2(r.right() - 6.0, r.bottom()),
                );
                let grip = egui::Rect::from_min_max(r.min, egui::pos2(tags_rect.left() - 4.0, r.bottom()));
                let over = resp;
                let resp = ui.interact(grip, ui.scope_id().with(("preset_name", &bank, slot)), Sense::click_and_drag());
                let selected = st.presets.sel.contains(&index);
                if selected || over.hovered() {
                    ui.painter().rect_filled(
                        r,
                        0.0,
                        if selected {
                            egui::Color32::from_rgb(42, 53, 51)
                        } else {
                            OLED_HOVER
                        },
                    );
                }
                let color = if *changed { theme::CHANGED } else if *filled { OLED } else { OLED_DIM };
                let mut text = egui::text::LayoutJob::simple_singleline(
                    format!("{bank}:{slot:03}  {name}{}", if *changed { " *" } else { "" }),
                    oled_font(16.0), color,
                );
                text.wrap = egui::text::TextWrapping::truncate_at_width((grip.width() - 6.0).max(0.0));
                let galley = ui.painter().layout_job(text);
                let pos = egui::pos2(r.left() + 6.0, r.center().y - galley.size().y * 0.5);
                let name_right = pos.x + galley.size().x;
                ui.painter().with_clip_rect(grip.intersect(ui.clip_rect())).galley(pos, galley, color);
                // The lock belongs to the slot on the device, so it shows even
                // while a staged edit sits on top of it (the push would be refused).
                let locked = st.presets.locked(&PresetSlot::new(&bank, *slot), full_ops);
                if locked {
                    let at = egui::pos2(name_right + 8.0, r.center().y);
                    if at.x + 8.0 < grip.right() {
                        padlock(ui.painter(), at, if *changed { theme::CHANGED } else { OLED_DIM });
                    }
                }
                let resp = resp.on_hover_text(format!("{bank}:{slot:03} · {name}{}",
                    if locked { "\nLocked on the device: it can't be moved, swapped, cleared or overwritten until unlocked there." } else { "" }));
                if *filled {
                    let source = row_sources[row].as_ref();
                    let loaded = source.and_then(|src| st.presets.preset(src)).is_some_and(|r| r.is_ok());
                    if let Some(Ok(mask)) = tags {
                        let (value, opened) = sound_tags::show_with_open(
                            ui, tags_rect, ui.scope_id().with(("preset_tags", &bank, slot)),
                            *mask, can_edit && loaded, *changed,
                        );
                        if opened && can_edit { tag_open = source.cloned(); }
                        if let Some(value) = value { tag_edit = Some((PresetSlot::new(&bank, *slot), value)); }
                    } else {
                        let tip = match tags {
                            Some(Err(e)) => format!("Could not read tags: {e}. Reload presets to retry."),
                            _ => "Reading tags… Click to load the sound for editing.".into(),
                        };
                        ui.painter().text(tags_rect.right_center() - egui::vec2(0.0, 1.0), egui::Align2::RIGHT_CENTER,
                            if tags.is_some() { "TAGS ?" } else { "…" }, oled_font(10.0), OLED_DIM);
                        let r = ui.interact(tags_rect, ui.scope_id().with(("preset_tags_loading", &bank, slot)), Sense::click()).on_hover_text(tip);
                        if r.clicked() && can_edit { tag_open = source.cloned(); }
                    }
                }
                if resp.clicked() && st.presets.pending.is_none() {
                    select = Some((index, ui.input(|i| i.modifiers)));
                }
                if can_edit && resp.drag_started() {
                    if !selected {
                        st.presets.select(banks, side, index, egui::Modifiers::NONE);
                    }
                    st.presets.active = side;
                    let frac = ui.input(|i| i.pointer.press_origin()).map_or(0.5, |p| (p.y - r.top()) / pitch);
                    st.presets.grab = slot_list::Grab::new(&st.presets.sel, index, frac);
                    resp.dnd_set_drag_payload(PresetDrag);
                }
                if can_edit && resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if dragging && ui.rect_contains_pointer(egui::Rect::from_min_size(r.min, egui::vec2(r.width(), pitch))) {
                    hover = Some((row, r));
                }
                resp.context_menu(|ui| {
                    ui.label(format!("Preset {bank}:{slot:03} · {name}"));
                    if ui.add_enabled(*filled && present, egui::Button::new("Copy sound  (⌘C)")).clicked() {
                        copy = Some(PresetSlot::new(&bank, *slot));
                        ui.close();
                    }
                    if ui.add_enabled(can_edit && *filled, egui::Button::new("Rename…")).clicked() {
                        rename = Some((PresetSlot::new(&bank, *slot), name.clone()));
                        ui.close();
                    }
                    let copied = st.proj.pool_clip.as_ref().map(|c| c.desc());
                    if ui.add_enabled(can_edit && copied.is_some(), egui::Button::new(
                        format!("Paste {}{}  (⌘V)", copied.as_deref().unwrap_or("sound"), if *filled { " over this preset" } else { " here" })
                    )).clicked() {
                        paste = Some((side, index));
                        ui.close();
                    }
                    ui.separator();
                    ui.add_enabled_ui(can_edit, |ui| {
                        if ui
                            .button(if selected {
                                format!("Clear {} selected slot(s)", st.presets.sel.len())
                            } else {
                                "Clear".into()
                            })
                            .clicked()
                        {
                            edit = Some(Edit::Clear(index));
                            ui.close();
                        }
                        if ui.button("Insert empty slot here").clicked() {
                            edit = Some(Edit::InsertEmpty(index, 1));
                            ui.close();
                        }
                        if selected
                            && st.presets.sel.len() > 1
                            && ui
                                .button(format!(
                                    "Insert {} empty slots above selection",
                                    st.presets.sel.len()
                                ))
                                .clicked()
                        {
                            edit = Some(Edit::InsertEmpty(
                                *st.presets.sel.first().unwrap(),
                                st.presets.sel.len(),
                            ));
                            ui.close();
                        }
                        if !filled && ui.button("Remove this empty slot").clicked() {
                            edit = Some(Edit::CloseGap(index));
                            ui.close();
                        }
                        if !st.presets.sel.is_empty() {
                            if ui
                                .button(format!("Move {} selected here", st.presets.sel.len()))
                                .clicked()
                            {
                                st.presets.active = side;
                                edit = Some(Edit::Drop(DropTarget::Insert(index)));
                                ui.close();
                            }
                            if selected {
                                for (label, dir) in [
                                    ("Move selection up  (⌥↑)", -1),
                                    ("Move selection down  (⌥↓)", 1),
                                ] {
                                    if ui.button(label).clicked() {
                                        edit = Some(Edit::Nudge(dir));
                                        ui.close();
                                    }
                                }
                            }
                        }
                    });
                });
            }
            if let Some((row, r)) = hover {
                let slot_of = |i: usize| rows.get(i).map_or(start + 256, |x| start + x.0 as usize - 1);
                let row_of = |s: usize| rows.partition_point(|x| start + (x.0 as usize) - 1 < s);
                let rules = slot_list::DropRules { first: 0, len: banks.len() * 256, plain: slot_list::PlainDrop::Insert };
                let sel = &st.presets.sel;
                let n = sel.len().max(1);
                let hit = slot_list::RowHit { rect: r, pitch, row, slot_of: &slot_of };
                let target = slot_list::drop_target_rows(ui, &hit, n, st.presets.grab, &rules);
                // Locked presets anywhere the drop changes make the push fail.
                let order = slot_list::preview(rules.len, &sel.iter().copied().collect::<Vec<_>>(), target, 0, |_| false);
                let locked = order.iter().enumerate()
                    .filter(|(i, old)| i != *old && st.presets.locked(&location(banks, *i), full_ops))
                    .count();
                let mut note = slot_list::landing_note(target, sel, &rules,
                    |i| { let l = location(banks, i); format!("{}:{:03}", l.bank, l.slot) },
                    |i| { let l = location(banks, i); edits.get(&l).cloned().unwrap_or_else(|| st.presets.base(&l)).is_some() });
                if locked > 0 {
                    note = format!("{} locked on the device: the push will be refused\n{note}", locked);
                }
                // Hidden rows (filter, hide empty) are skipped: the area covers
                // the visible rows the landing slots fall on.
                let span = slot_list::landing_span(target, n);
                let y = |s: usize| r.top() + (row_of(s) as f32 - row as f32) * pitch;
                let x = r.x_range();
                slot_list::paint_landing(ui, x, egui::Rangef::new(y(span.start), y(span.end).max(y(span.start) + 3.0)), target, locked > 0);
                theme::pointer_note(ui.ctx(), &note, if locked > 0 { theme::DENY } else { theme::DEVICE_ACCENT });
                if ui.input(|i| i.pointer.any_released()) {
                    egui::DragAndDrop::clear_payload(ui.ctx());
                    st.presets.active = side;
                    edit = Some(Edit::Drop(target));
                }
            }
            if dragging {
                slot_list::drag_autoscroll(ui, pitch * 1.5);
            }
        });
    });
    if let Some((loc, name)) = rename {
        if let Some(src) = st.presets.origin(&loc) {
            st.presets.request_sound(&src, out);
            st.presets.rename = Some((loc, name));
        }
    }
    if let Some(src) = tag_open {
        st.presets.request_sound(&src, out);
    }
    if let Some((loc, tags)) = tag_edit {
        st.presets.edit_tags(loc, tags);
    }
    if let Some(loc) = copy { copy_preset(st, loc, out, ui.ctx()); }
    if let Some((side, index)) = paste {
        st.presets.select(banks, side, index, egui::Modifiers::NONE);
        paste_sound(st, location(banks, index));
    }
    if let Some(action) = edit {
        st.presets.edit(banks, action, out);
    }
    if let Some((index, modifiers)) = select {
        st.presets.select(banks, side, index, modifiers);
    }
}
