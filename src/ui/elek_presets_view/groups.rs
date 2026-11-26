//! Preset groups: the shared group engine (`crate::ui::slot_groups`) over
//! the Presets page's slots. Slots are numbered across the bank list
//! (`bank index * 256 + slot - 1`) as shown now, staged edits included, so
//! a group's members are its slot numbers; each staged move renumbers them.
//! Groups are saved per unit by bank and slot (`state::PresetGroup`), as of
//! the device: while edits are staged, the save keeps the layout before
//! them (`base_groups`), and a push (or Live send) saves the new one.

use super::*;
use crate::ui::slot_groups::ui::GroupView;
use crate::ui::slot_groups::{positions, Group, GroupHost, Plan, Slots};
pub use crate::state::PresetGroup;

/// Presets per bank.
pub(super) const BANK: usize = 256;

/// Where a pane scrolls to next draw, worked out against its visible rows.
#[derive(Clone)]
pub(super) enum Align {
    /// Bring these slots into view, from one row above the first, unless
    /// they're all in view already.
    Show(std::ops::Range<usize>),
    /// Put slot `.0` at `.1` px below the top of the view.
    Level(usize, f32),
}

impl PresetView {
    pub(super) fn bank_list(&self) -> &[String] {
        match &self.banks {
            Some(Ok(b)) => b,
            _ => &[],
        }
    }

    /// Recomputes which slots are empty now (staged edits included) for the
    /// group planner. Slots in banks not read yet count as filled, so no
    /// push ever relies on them being free.
    pub(super) fn refresh_slots(&mut self) {
        let banks = self.bank_list().to_vec();
        let n = banks.len() * BANK;
        let mut free = vec![false; n];
        for (b, bank) in banks.iter().enumerate() {
            let Some(Some(Ok(entries))) = self.listings.get(bank) else { continue };
            let filled: HashSet<u32> = entries.iter().map(|e| e.slot).collect();
            for s in 0..BANK {
                free[b * BANK + s] = !filled.contains(&(s as u32 + 1));
            }
        }
        for (loc, src) in self.edits() {
            if let Some(b) = banks.iter().position(|x| *x == loc.bank) {
                let i = b * BANK + loc.slot as usize - 1;
                if i < n {
                    free[i] = src.is_none();
                }
            }
        }
        self.slot_free = free;
        if self.ident.len() != n {
            self.ident = (0..n as u16).collect();
        }
        self.sync_groups();
    }

    pub(super) fn group_view<'a>(&'a self, hues: &'a std::collections::BTreeMap<String, f32>, dragging: bool, pitch: f32) -> GroupView<'a> {
        GroupView { s: self.slots(), hues, drag: self.group_drag, dragging, grab: self.grab, pitch }
    }

    /// The saved groups for this unit, from the app (on start and when
    /// another unit is shown).
    pub fn set_saved_groups(&mut self, unit: String, saved: Vec<PresetGroup>) {
        self.groups_unit = Some(unit);
        self.saved_groups = saved;
        self.groups_banks.clear();
        self.sync_groups();
    }

    /// Shows the saved groups once the bank list is known (again, when it
    /// changed). A saved group whose bank is gone stays saved.
    fn sync_groups(&mut self) {
        let Some(Ok(banks)) = &self.banks else { return };
        if *banks == self.groups_banks {
            return;
        }
        let banks = banks.clone();
        let mut taken: HashSet<usize> = HashSet::new();
        let mut groups = Vec::new();
        for g in &self.saved_groups {
            let Some(b) = banks.iter().position(|x| *x == g.bank) else { continue };
            let (at, len) = (g.at as usize, g.len as usize);
            if len == 0 || at + len > BANK {
                continue;
            }
            let a = b * BANK + at;
            if (a..a + len).any(|s| taken.contains(&s)) {
                continue;
            }
            taken.extend(a..a + len);
            groups.push(Group { name: g.name.clone(), members: (a..a + len).map(|s| s as u16).collect() });
        }
        self.groups = groups;
        self.base_groups = self.groups.clone();
        self.groups_banks = banks;
        self.group_held = None;
        self.group_drag = None;
    }

    /// After the groups changed: with nothing staged they match the device,
    /// so they're the ones to save.
    pub(super) fn groups_changed(&mut self) {
        if self.ops.is_empty() {
            self.base_groups = self.groups.clone();
            self.save_groups();
        }
    }

    /// Writes `base_groups` into the saved list (by bank and slot).
    fn save_groups(&mut self) {
        if self.groups_unit.is_none() || self.groups_banks.is_empty() {
            return;
        }
        let banks = &self.groups_banks;
        let mut kept: Vec<PresetGroup> = self.saved_groups.iter().filter(|g| !banks.contains(&g.bank)).cloned().collect();
        let pos: Vec<usize> = (0..banks.len() * BANK).collect();
        for g in &self.base_groups {
            let span = crate::ui::slot_groups::span_of(g, &pos);
            if span.is_empty() || span.end > pos.len() {
                continue;
            }
            kept.push(PresetGroup {
                name: g.name.clone(),
                bank: banks[span.start / BANK].clone(),
                at: (span.start % BANK) as u16,
                len: span.len() as u16,
            });
        }
        if kept != self.saved_groups {
            self.saved_groups = kept;
            self.groups_dirty = true;
        }
    }

    /// Back to the groups as of the device (Reset, Reload).
    pub(super) fn reset_groups(&mut self) {
        self.groups = self.base_groups.clone();
        self.group_held = None;
        self.group_drag = None;
    }

    /// Groups that survive an edit made without them (nudge, insert or
    /// remove an empty slot), renumbered to the slots they land on.
    pub(super) fn groups_after(&self, order: &[usize]) -> Vec<Group> {
        let order: Vec<u16> = order.iter().map(|&o| o as u16).collect();
        renumber(&self.slots().repair_groups(&order), &order)
    }

    /// Row `slot` (numbered across banks) sits at in pane `side`, as last drawn.
    pub(super) fn row_of(&self, side: usize, slot: usize) -> usize {
        self.visible[side].partition_point(|&s| s < slot)
    }
}

/// Members as slot numbers after `order` (`order[new] = old`).
pub(super) fn renumber(groups: &[Group], order: &[u16]) -> Vec<Group> {
    let pos = positions(order);
    groups
        .iter()
        .map(|g| Group { name: g.name.clone(), members: g.members.iter().map(|&x| pos[x as usize] as u16).collect() })
        .collect()
}

impl GroupHost for PresetView {
    fn slots(&self) -> Slots<'_> {
        let banks = self.bank_list();
        // Groups belong to the bank list they were matched on; until the
        // current one is matched, there are none.
        let ok = *banks == self.groups_banks[..] && self.ident.len() == banks.len() * BANK;
        Slots {
            order: if ok { &self.ident } else { &[] },
            groups: if ok { &self.groups } else { &[] },
            sel: &self.sel,
            bank: BANK,
            first: 0,
            insert: true,
            noun: "presets",
            free_fn: Box::new(move |id| self.slot_free.get(id as usize).copied().unwrap_or(false)),
            label_fn: Box::new(move |i| format!("{}:{:03}", banks.get(i / BANK).map_or("?", |b| b.as_str()), i % BANK + 1)),
            bank_fn: Box::new(move |b| banks.get(b).cloned().unwrap_or_else(|| "?".into())),
        }
    }

    /// Goes through the same staging as any preset move: the affected banks
    /// are read first, then the order becomes native moves and swaps.
    fn commit_plan(&mut self, p: Plan, select: Option<usize>, lead: Option<bool>) {
        let groups = renumber(&p.groups, &p.order);
        self.pending = Some(Reorder {
            banks: self.bank_list().to_vec(),
            order: p.order.iter().map(|&o| o as usize).collect(),
            sel: self.sel.clone(),
            cursor: self.cursor,
            clear: None,
            side: self.active,
            groups: Some(groups),
            note: Some(p.note),
            select,
            lead,
        });
        let mut out = Vec::new();
        self.apply_pending(&mut out);
        self.want.extend(out);
    }

    fn note(&mut self, msg: String) {
        self.status = msg;
    }

    fn select_group(&mut self, g: usize) {
        let Some(span) = self.slots().group_spans().get(g).cloned() else { return };
        self.sel = span.clone().collect();
        self.cursor = Some(span.start);
        self.group_held = Some(g);
        let banks = self.bank_list();
        if banks.get(span.start / BANK) == Some(&self.bank[self.active]) {
            self.selected[self.active] = Some((span.start % BANK + 1) as u32);
        }
    }

    /// Jumps to group `g` from pane `side`: the pane opens its bank and, when
    /// the group isn't all in view, eases to it. Its first slot flashes and
    /// the group is selected.
    fn go_to_group(&mut self, g: usize, side: usize) {
        let Some(span) = self.slots().group_spans().get(g).cloned() else { return };
        self.active = side;
        self.select_group(g);
        self.ping = Some((span.start, std::time::Instant::now()));
        if let Some(b) = self.bank_list().get(span.start / BANK).cloned() {
            self.bank[side] = b;
        }
        self.align[side] = Some(Align::Show(span));
    }

    /// Group `g` was clicked by its frame in pane `side`: scroll the other
    /// pane so its group of the same name (in the bank it shows) sits level
    /// with this one.
    fn align_group(&mut self, g: usize, side: usize) {
        let s = self.slots();
        let spans = s.group_spans();
        let (Some(gr), Some(span)) = (s.groups.get(g), spans.get(g).cloned()) else { return };
        let name = crate::ui::slot_groups::hue_key(&gr.name);
        let other = 1 - side;
        let bank_o = self.bank_list().iter().position(|b| *b == self.bank[other]);
        let same: Vec<usize> = (0..s.groups.len()).filter(|&h| h != g && crate::ui::slot_groups::hue_key(&s.groups[h].name) == name).collect();
        let found = same.iter().copied().find(|&h| Some(spans[h].start / BANK) == bank_o);
        let msg = match found {
            None => {
                let mut elsewhere: Vec<String> = same.iter().map(|&h| s.bname(spans[h].start / BANK)).collect();
                elsewhere.dedup();
                if elsewhere.is_empty() {
                    format!("No other group named \"{}\" to line up with", gr.name)
                } else {
                    format!("No \"{}\" group in bank {}; there's one in {}", gr.name, self.bank[other], elsewhere.join(", "))
                }
            }
            Some(h) => format!("Lined up \"{}\": {} ↔ {}", gr.name, s.label(span.start), s.label(spans[h].start)),
        };
        let target = found.map(|h| spans[h].start);
        drop(s);
        if let Some(t) = target {
            // Where the clicked group sits in its pane now; the other one goes there.
            let y = self.row_of(side, span.start) as f32 * self.pitch - self.scroll[side];
            self.align[other] = Some(Align::Level(t, y));
        }
        self.status = msg;
    }

    fn group_drag_start(&mut self, g: usize, slot: usize, frac: f32, side: usize) {
        self.group_drag = Some(g);
        self.active = side;
        self.select_group(g);
        self.grab = slot_list::Grab::new(&self.sel, slot, frac);
    }

    fn end_group_drag(&mut self) {
        self.group_drag = None;
    }

    fn ask_group_name(&mut self, g: Option<usize>, slots: Vec<usize>, name: String) {
        self.group_naming = Some((g, slots, name, true));
    }

    fn set_hue(&mut self, key: String, h: f32) {
        self.hue_edits.push((key, h));
    }

    fn set_active(&mut self, side: usize) {
        self.active = side;
    }
}
